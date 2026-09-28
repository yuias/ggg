//! Windows Named Pipe server for the daemon control protocol.
//!
//! Frames `ControlRequest`/`ControlResponse` instead of the ggg-dnd bridge
//! protocol (`crate::ipc::pipe_server`), and adds the single-instance bind
//! used for the "only one daemon per config dir" guarantee. Unlike
//! `pipe_server.rs`, which creates the next instance only after accepting a
//! client on the current one, this module creates the next instance right
//! after `connect()` returns and before handing the connected instance to a
//! task, so there is never a moment with zero instances of the pipe name.
use std::sync::Arc;
use std::time::Duration;

use futures_util::future::BoxFuture;
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};

use super::client::{self, DaemonProbe};
use super::protocol::{ControlErrorCode, ControlRequest, ControlResponse};

/// Caps a single request line, matching the bridge pipe's cap
/// (`pipe_server.rs`).
const MAX_REQUEST_BYTES: u64 = 64 * 1024;

pub trait ControlHandler: Send + Sync + 'static {
    fn handle(&self, req: ControlRequest) -> BoxFuture<'_, ControlResponse>;
    /// Runs after the response line has been written (or failed to write),
    /// so a shutdown or a queue save never races the client's reply. `resp`
    /// is what `handle` returned, so implementations can skip work that only
    /// makes sense for a successful reply (e.g. skip a queue save after an
    /// `Error` response).
    fn after_response(&self, req: &ControlRequest, resp: &ControlResponse) -> BoxFuture<'_, ()>;
}

#[derive(Debug)]
pub enum BindError {
    AlreadyRunning { pid: Option<u32> },
    Io(std::io::Error),
}

#[derive(Debug)]
pub struct BoundPipe {
    name: String,
    first: NamedPipeServer,
}

impl BoundPipe {
    pub fn name(&self) -> &str {
        &self.name
    }
}

fn create_first_instance(name: &str) -> std::io::Result<NamedPipeServer> {
    ServerOptions::new()
        .first_pipe_instance(true)
        .reject_remote_clients(true)
        .create(name)
}

fn create_next_instance(name: &str) -> std::io::Result<NamedPipeServer> {
    ServerOptions::new()
        .first_pipe_instance(false)
        .reject_remote_clients(true)
        .create(name)
}

/// `ERROR_ACCESS_DENIED`: what `first_pipe_instance(true)` returns when an
/// instance of `name` already exists.
fn is_already_claimed(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(5)
}

/// Binds the first instance of `pipe_name`, the single-instance claim for a
/// config directory. If the name is already claimed, probes the holder to
/// tell an active daemon apart from a pipe left behind by a process that
/// died without cleanup, giving the latter one retry before giving up.
pub async fn bind(pipe_name: &str) -> Result<BoundPipe, BindError> {
    match create_first_instance(pipe_name) {
        Ok(first) => Ok(BoundPipe { name: pipe_name.to_string(), first }),
        Err(_) => {
            if let DaemonProbe::Running { pid, .. } = client::probe(pipe_name).await {
                return Err(BindError::AlreadyRunning { pid: Some(pid) });
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
            match create_first_instance(pipe_name) {
                Ok(first) => Ok(BoundPipe { name: pipe_name.to_string(), first }),
                Err(e2) if is_already_claimed(&e2) => Err(BindError::AlreadyRunning { pid: None }),
                Err(e2) => Err(BindError::Io(e2)),
            }
        }
    }
}

/// Spawns the accept loop; returns immediately. The loop runs until the
/// process exits or the returned handle is aborted.
pub fn serve(bound: BoundPipe, handler: Arc<dyn ControlHandler>) -> tokio::task::JoinHandle<()> {
    tokio::spawn(accept_loop(bound.name, bound.first, handler))
}

async fn accept_loop(pipe_name: String, mut server: NamedPipeServer, handler: Arc<dyn ControlHandler>) {
    // Tracking connection tasks here (instead of a bare `tokio::spawn`) means
    // dropping this future -- which is what happens when `serve`'s returned
    // handle is aborted -- also aborts every in-flight connection, instead of
    // leaving them to run detached forever. This doesn't interrupt a save
    // already inside `spawn_blocking` file IO (aborting a task never does),
    // so it's a courtesy, not the guard against overlapping saves; that's
    // `DaemonContext::save_lock`.
    let mut connections = tokio::task::JoinSet::new();
    loop {
        if let Err(e) = server.connect().await {
            tracing::error!("control pipe accept failed on {pipe_name}: {e}");
            // A connect error can leave `server` in a broken state (e.g. a
            // client that connected and disconnected before this awaited).
            // Replace it with a fresh instance before backing off, so a
            // client never sees ERROR_PIPE_BUSY forever from a dead
            // instance. Create-then-assign avoids a window with zero
            // instances of the pipe name.
            if let Ok(fresh) = create_next_instance(&pipe_name) {
                server = fresh;
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            continue;
        }

        tracing::debug!("control pipe client connected on {pipe_name}");

        // Create the next instance before handing off the connected one, so
        // the pipe name always has an instance a client can open.
        let next = match create_next_instance(&pipe_name) {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("failed to create next control pipe instance on {pipe_name}: {e}");
                tokio::time::sleep(Duration::from_secs(1)).await;
                continue;
            }
        };
        let connected = std::mem::replace(&mut server, next);
        let h = handler.clone();
        connections.spawn(async move {
            handle_connection(connected, h).await;
        });
        // Reap finished connections so the set doesn't grow for the life of
        // the daemon; this never blocks (no pending connection is awaited).
        while connections.try_join_next().is_some() {}
    }
}

async fn handle_connection(pipe: NamedPipeServer, handler: Arc<dyn ControlHandler>) {
    let (reader, mut writer) = tokio::io::split(pipe);
    let mut reader = BufReader::new(reader);

    loop {
        let mut buf = Vec::new();
        let n = match (&mut reader).take(MAX_REQUEST_BYTES).read_until(b'\n', &mut buf).await {
            Ok(0) => break, // clean EOF
            Ok(n) => n,
            Err(e) => {
                tracing::warn!("control pipe read error: {e}");
                break;
            }
        };
        if n as u64 == MAX_REQUEST_BYTES && !buf.ends_with(b"\n") {
            tracing::warn!("control pipe request exceeds {MAX_REQUEST_BYTES} bytes; closing connection");
            break;
        }

        let line = String::from_utf8_lossy(&buf).trim().to_string();
        if line.is_empty() {
            continue;
        }

        let req: ControlRequest = match serde_json::from_str(&line) {
            Ok(r) => r,
            Err(e) => {
                let resp = ControlResponse::Error {
                    code: ControlErrorCode::InvalidInput,
                    message: format!("invalid request: {e}"),
                };
                if !write_response(&mut writer, &resp).await {
                    break;
                }
                continue;
            }
        };

        let resp = if req.cancel_on_disconnect() {
            match run_until_disconnect(handler.handle(req.clone()), &mut reader).await {
                Some(resp) => resp,
                None => {
                    // The client is gone and the request was only ever going
                    // to reply to it, so there is nothing left to write or
                    // save.
                    tracing::debug!("control pipe client disconnected while a cancellable request was pending");
                    break;
                }
            }
        } else {
            handler.handle(req.clone()).await
        };
        let write_ok = write_response(&mut writer, &resp).await;
        // Run after_response even if the reply couldn't be delivered: the
        // handler already applied the request in memory (e.g. a mutation
        // that needs saving, or a Shutdown), and that must not be lost just
        // because the client disconnected before the reply went out.
        handler.after_response(&req, &resp).await;
        if !write_ok {
            break;
        }
    }

    tracing::debug!("control pipe client disconnected");
}

/// Races a cancellable handler future against the reader hitting EOF (the
/// client disconnected). `None` means the client is gone; the caller must
/// then skip writing a reply and calling `after_response`, because the
/// request only existed to answer a connection that no longer exists.
async fn run_until_disconnect<R: AsyncBufRead + Unpin>(
    fut: BoxFuture<'_, ControlResponse>,
    reader: &mut R,
) -> Option<ControlResponse> {
    let mut fut = fut;
    let mut watching = true;
    loop {
        tokio::select! {
            resp = &mut fut => return Some(resp),
            eof = async { reader.fill_buf().await.map(|b| b.is_empty()) }, if watching => match eof {
                Ok(true) | Err(_) => return None,   // client gone
                Ok(false) => watching = false,      // pipelined bytes stay buffered for the next read
            },
        }
    }
}

async fn write_response<W: AsyncWrite + Unpin>(writer: &mut W, resp: &ControlResponse) -> bool {
    let mut json = match serde_json::to_string(resp) {
        Ok(j) => j,
        Err(e) => {
            tracing::error!("failed to serialize control response: {e}");
            return false;
        }
    };
    json.push('\n');
    if let Err(e) = writer.write_all(json.as_bytes()).await {
        tracing::warn!("failed to write control response: {e}");
        return false;
    }
    if let Err(e) = writer.flush().await {
        tracing::warn!("failed to flush control response: {e}");
        return false;
    }
    true
}
