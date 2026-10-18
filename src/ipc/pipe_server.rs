/// Windows Named Pipe server for receiving URLs from ggg-dnd and the browser bridge.
///
/// Listens on `\\.\pipe\ggg` (default, or `GGG_PIPE` when set) or
/// `\\.\pipe\ggg-{pid}` (fallback).
/// Each client connection is handled in a separate tokio task.
use ggg_ipc::{IpcRequest, IpcResponse, PIPE_NAME_PREFIX};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{NamedPipeServer, ServerOptions};
use tokio::sync::mpsc;

/// Message sent from the pipe server to the TUI / GUI event loop
#[derive(Debug, Clone)]
pub enum IpcEvent {
    /// A URL was received from the GUI and should be added to the current folder
    UrlReceived(String),
    /// From `IpcRequest::Activate`: bring the window forward and add `urls`.
    Activate(Vec<String>),
}

/// A pipe name this process holds the first instance of.
///
/// Keeping the instance (instead of probing and dropping it) leaves no gap in
/// which another process could take the name before the accept loop starts.
pub struct ClaimedPipe {
    name: String,
    first: NamedPipeServer,
}

impl ClaimedPipe {
    pub fn name(&self) -> &str {
        &self.name
    }
}

/// Claims `name` with `first_pipe_instance(true)`. Needs a tokio runtime
/// context.
pub fn claim_pipe(name: &str) -> std::io::Result<ClaimedPipe> {
    let first = ServerOptions::new().first_pipe_instance(true).create(name)?;
    Ok(ClaimedPipe {
        name: name.to_string(),
        first,
    })
}

/// `ERROR_ACCESS_DENIED`: what `first_pipe_instance(true)` returns when an
/// instance of the name already exists.
pub fn is_already_claimed(e: &std::io::Error) -> bool {
    e.raw_os_error() == Some(5)
}

/// Claims the default name, else the per-process fallback name.
fn claim_default_or_fallback() -> std::io::Result<ClaimedPipe> {
    let default_name = ggg_ipc::default_pipe_name();
    match claim_pipe(&default_name) {
        Ok(claimed) => {
            tracing::info!("IPC pipe bound to default name: {}", default_name);
            Ok(claimed)
        }
        Err(_) => {
            let fallback = format!("{}{}", PIPE_NAME_PREFIX, std::process::id());
            tracing::warn!(
                "Default pipe name occupied, using fallback: {}",
                fallback
            );
            claim_pipe(&fallback)
        }
    }
}

/// Spawns the accept loop for a claimed pipe; returns immediately. The loop
/// runs until the process exits or the returned handle is aborted.
pub fn serve_pipe(
    claimed: ClaimedPipe,
    event_tx: mpsc::Sender<IpcEvent>,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(accept_loop(claimed.name, claimed.first, event_tx))
}

/// Start the Named Pipe server.
///
/// Returns `(pipe_name, join_handle)` so the caller can display the pipe name
/// and await shutdown.
pub fn start_pipe_server(
    event_tx: mpsc::Sender<IpcEvent>,
) -> (String, tokio::task::JoinHandle<()>) {
    match claim_default_or_fallback() {
        Ok(claimed) => {
            let name = claimed.name.clone();
            (name, serve_pipe(claimed, event_tx))
        }
        Err(e) => {
            // Without a pipe the app still works; only link hand-over is lost.
            tracing::error!("Failed to create the IPC pipe: {}", e);
            let name = format!("{}{}", PIPE_NAME_PREFIX, std::process::id());
            (name, tokio::spawn(async {}))
        }
    }
}

fn create_next_instance(name: &str) -> std::io::Result<NamedPipeServer> {
    ServerOptions::new().first_pipe_instance(false).create(name)
}

/// Main accept loop: continuously accept client connections on the named pipe.
async fn accept_loop(
    pipe_name: String,
    mut server: NamedPipeServer,
    event_tx: mpsc::Sender<IpcEvent>,
) {
    loop {
        // Wait for a client to connect
        if let Err(e) = server.connect().await {
            tracing::error!("Failed to accept pipe connection: {}", e);
            // A failed connect can leave the instance unusable; replace it so
            // clients do not see a dead instance forever.
            if let Ok(fresh) = create_next_instance(&pipe_name) {
                server = fresh;
            }
            // Back off before retrying so a persistent connect failure does
            // not spin the accept loop.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            continue;
        }

        tracing::debug!("IPC client connected on {}", pipe_name);

        // Create the next instance right away, so the name always has an
        // instance a client can open while this one is being served.
        let next = loop {
            match create_next_instance(&pipe_name) {
                Ok(s) => break s,
                Err(e) => {
                    tracing::error!("Failed to create pipe instance: {}", e);
                    tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                }
            }
        };
        let connected = std::mem::replace(&mut server, next);

        let tx = event_tx.clone();
        tokio::spawn(async move {
            handle_client(connected, tx).await;
        });
    }
}

/// Handle a single client connection.
///
/// Reads newline-delimited JSON messages, processes each request,
/// and writes back a JSON response.
async fn handle_client(
    pipe: NamedPipeServer,
    event_tx: mpsc::Sender<IpcEvent>,
) {
    // Cap a single message so a client that never sends a newline can't make
    // the server buffer unboundedly (a local DoS).
    const MAX_LINE_BYTES: u64 = 64 * 1024;

    let (reader, mut writer) = tokio::io::split(pipe);
    let mut reader = BufReader::new(reader);

    loop {
        let mut buf = Vec::new();
        let n = match (&mut reader).take(MAX_LINE_BYTES).read_until(b'\n', &mut buf).await {
            Ok(0) => break, // clean EOF
            Ok(n) => n,
            Err(e) => {
                // Distinguish a read error from EOF instead of silently exiting.
                tracing::warn!("IPC read error: {}", e);
                break;
            }
        };

        // Hit the cap without a terminating newline: the message is too long.
        if n as u64 == MAX_LINE_BYTES && !buf.ends_with(b"\n") {
            tracing::warn!("IPC message exceeds {} bytes; closing connection", MAX_LINE_BYTES);
            break;
        }

        let line = String::from_utf8_lossy(&buf).trim().to_string();
        if line.is_empty() {
            continue;
        }

        let response = match serde_json::from_str::<IpcRequest>(&line) {
            Ok(request) => process_request(request, &event_tx).await,
            Err(e) => {
                tracing::warn!("Invalid IPC message: {} — raw: {}", e, line);
                IpcResponse::Error {
                    message: format!("Invalid message: {}", e),
                }
            }
        };

        // Serialize and send response
        let mut resp_json = match serde_json::to_string(&response) {
            Ok(j) => j,
            Err(e) => {
                tracing::error!("Failed to serialize IPC response: {}", e);
                continue;
            }
        };
        resp_json.push('\n');

        if let Err(e) = writer.write_all(resp_json.as_bytes()).await {
            tracing::warn!("Failed to write IPC response: {}", e);
            break;
        }
    }

    tracing::debug!("IPC client disconnected");
}

/// Process a single IPC request and return the appropriate response.
async fn process_request(
    request: IpcRequest,
    event_tx: &mpsc::Sender<IpcEvent>,
) -> IpcResponse {
    match request {
        IpcRequest::AddUrl { url } => {
            tracing::debug!("IPC received URL: {}", url);

            // Forward to TUI event loop
            match event_tx.send(IpcEvent::UrlReceived(url.clone())).await {
                Ok(_) => IpcResponse::Ok {
                    message: format!("URL queued: {}", url),
                },
                Err(e) => {
                    tracing::error!("Failed to forward URL to TUI: {}", e);
                    IpcResponse::Error {
                        message: "TUI event channel closed".to_string(),
                    }
                }
            }
        }
        IpcRequest::Ping => IpcResponse::Pong,
        IpcRequest::Activate { urls } => {
            tracing::debug!("IPC activation request with {} URL(s)", urls.len());
            match event_tx.send(IpcEvent::Activate(urls)).await {
                Ok(_) => IpcResponse::Ok {
                    message: "activated".to_string(),
                },
                Err(e) => {
                    tracing::error!("Failed to forward activation: {}", e);
                    IpcResponse::Error {
                        message: "event channel closed".to_string(),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    fn unique_pipe_name() -> String {
        format!(r"\\.\pipe\ggg-test-{}", uuid::Uuid::new_v4())
    }

    /// A pipe created by this same process, opened through `ggg_ipc`'s
    /// blocking client, passes the client-side owner check: this exercises
    /// `ggg-ipc`'s client end to end, not just the pure `decide_owner` rule.
    /// Uses a unique name, not `DEFAULT_PIPE_NAME`, so it cannot collide with
    /// a real server already holding that name.
    #[tokio::test]
    async fn ggg_ipc_client_accepts_pipe_created_by_same_user() {
        let pipe_name = unique_pipe_name();
        let (event_tx, _event_rx) = mpsc::channel(1);

        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .expect("create test pipe");

        let accept_name = pipe_name.clone();
        tokio::spawn(async move {
            if server.connect().await.is_ok() {
                handle_client(server, event_tx).await;
            }
            let _ = accept_name;
        });

        let ping_name = pipe_name.clone();
        let result = tokio::time::timeout(
            TEST_TIMEOUT,
            tokio::task::spawn_blocking(move || ggg_ipc::ping(&ping_name)),
        )
        .await
        .expect("ping did not complete within timeout")
        .expect("spawn_blocking task panicked");

        assert!(result.is_ok(), "expected same-user pipe to be trusted: {result:?}");
    }

    /// The server pid read from an open pipe is the creating process, which
    /// here is the test process itself.
    #[tokio::test]
    async fn server_process_id_is_own_pid() {
        let pipe_name = unique_pipe_name();
        let (event_tx, _event_rx) = mpsc::channel(1);

        let server = ServerOptions::new()
            .first_pipe_instance(true)
            .create(&pipe_name)
            .expect("create test pipe");

        tokio::spawn(async move {
            if server.connect().await.is_ok() {
                handle_client(server, event_tx).await;
            }
        });

        let query_name = pipe_name.clone();
        let result = tokio::time::timeout(
            TEST_TIMEOUT,
            tokio::task::spawn_blocking(move || ggg_ipc::server_process_id(&query_name)),
        )
        .await
        .expect("server_process_id did not complete within timeout")
        .expect("spawn_blocking task panicked");

        assert_eq!(result.expect("server pid"), std::process::id());
    }

    #[tokio::test]
    async fn claiming_a_held_name_is_reported_as_already_claimed() {
        let name = unique_pipe_name();
        let _first = claim_pipe(&name).expect("first claim");
        match claim_pipe(&name) {
            Ok(_) => panic!("second claim of the same name must fail"),
            Err(e) => assert!(is_already_claimed(&e), "unexpected error: {e:?}"),
        }
    }

    #[tokio::test]
    async fn served_pipe_answers_two_sequential_pings() {
        let name = unique_pipe_name();
        let (event_tx, _event_rx) = mpsc::channel(1);
        let handle = serve_pipe(claim_pipe(&name).expect("claim"), event_tx);

        for round in 0..2 {
            let ping_name = name.clone();
            let result = tokio::time::timeout(
                TEST_TIMEOUT,
                tokio::task::spawn_blocking(move || ggg_ipc::ping(&ping_name)),
            )
            .await
            .expect("ping did not complete within timeout")
            .expect("spawn_blocking task panicked");
            assert!(result.is_ok(), "ping {round} failed: {result:?}");
        }
        handle.abort();
    }

    #[tokio::test]
    async fn activate_request_arrives_as_event_with_urls() {
        let name = unique_pipe_name();
        let (event_tx, mut event_rx) = mpsc::channel(4);
        let handle = serve_pipe(claim_pipe(&name).expect("claim"), event_tx);

        let urls = vec![
            "http://127.0.0.1:1/a".to_string(),
            "http://127.0.0.1:1/b".to_string(),
        ];
        let request = IpcRequest::Activate { urls: urls.clone() };
        let send_name = name.clone();
        let response = tokio::time::timeout(
            TEST_TIMEOUT,
            tokio::task::spawn_blocking(move || ggg_ipc::send_request(&send_name, &request)),
        )
        .await
        .expect("send did not complete within timeout")
        .expect("spawn_blocking task panicked")
        .expect("send_request");
        assert!(matches!(response, IpcResponse::Ok { .. }), "{response:?}");

        let event = tokio::time::timeout(TEST_TIMEOUT, event_rx.recv())
            .await
            .expect("no event within timeout")
            .expect("channel closed");
        match event {
            IpcEvent::Activate(got) => assert_eq!(got, urls),
            other => panic!("unexpected event: {other:?}"),
        }
        handle.abort();
    }
}
