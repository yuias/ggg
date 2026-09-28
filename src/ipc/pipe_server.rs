/// Windows Named Pipe server for receiving URLs from ggg-dnd GUI.
///
/// Listens on `\\.\pipe\ggg-dnd` (default) or `\\.\pipe\ggg-dnd-{pid}` (fallback).
/// Each client connection is handled in a separate tokio task.
use ggg_ipc::{IpcRequest, IpcResponse, DEFAULT_PIPE_NAME, PIPE_NAME_PREFIX};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::ServerOptions;
use tokio::sync::mpsc;

/// Message sent from the pipe server to the TUI event loop
#[derive(Debug, Clone)]
pub enum IpcEvent {
    /// A URL was received from the GUI and should be added to the current folder
    UrlReceived(String),
}

/// Attempt to create a Named Pipe server, trying the default name first.
/// Returns the pipe name that was successfully bound.
fn resolve_pipe_name() -> String {
    // Try default name first by attempting to create a pipe instance
    match ServerOptions::new()
        .first_pipe_instance(true)
        .create(DEFAULT_PIPE_NAME)
    {
        Ok(_) => {
            // Successfully reserved the default name.
            // The instance is dropped here but that's fine —
            // the accept loop will re-create it.
            tracing::info!("IPC pipe bound to default name: {}", DEFAULT_PIPE_NAME);
            DEFAULT_PIPE_NAME.to_string()
        }
        Err(_) => {
            let pid = std::process::id();
            let fallback = format!("{}{}", PIPE_NAME_PREFIX, pid);
            tracing::warn!(
                "Default pipe name occupied, using fallback: {}",
                fallback
            );
            fallback
        }
    }
}

/// Start the Named Pipe server.
///
/// Returns `(pipe_name, join_handle)` so the caller can display the pipe name
/// and await shutdown.
pub fn start_pipe_server(
    event_tx: mpsc::Sender<IpcEvent>,
) -> (String, tokio::task::JoinHandle<()>) {
    let pipe_name = resolve_pipe_name();
    let name = pipe_name.clone();

    let handle = tokio::spawn(async move {
        accept_loop(&name, event_tx).await;
    });

    (pipe_name, handle)
}

/// Main accept loop: continuously accept client connections on the named pipe.
async fn accept_loop(pipe_name: &str, event_tx: mpsc::Sender<IpcEvent>) {
    loop {
        // Create a new pipe instance for the next client
        let server = match ServerOptions::new()
            .first_pipe_instance(false)
            .create(pipe_name)
        {
            Ok(s) => s,
            Err(e) => {
                tracing::error!("Failed to create pipe instance: {}", e);
                // Brief pause before retrying to avoid busy-loop on persistent errors
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
                continue;
            }
        };

        // Wait for a client to connect
        if let Err(e) = server.connect().await {
            tracing::error!("Failed to accept pipe connection: {}", e);
            // Back off before retrying so a persistent connect failure does
            // not spin the accept loop.
            tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            continue;
        }

        tracing::debug!("IPC client connected on {}", pipe_name);

        let tx = event_tx.clone();
        tokio::spawn(async move {
            handle_client(server, tx).await;
        });
    }
}

/// Handle a single client connection.
///
/// Reads newline-delimited JSON messages, processes each request,
/// and writes back a JSON response.
async fn handle_client(
    pipe: tokio::net::windows::named_pipe::NamedPipeServer,
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
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

    /// A pipe created by this same process, opened through `ggg_ipc`'s
    /// blocking client, passes the client-side owner check: this exercises
    /// `ggg-ipc`'s client end to end, not just the pure `decide_owner` rule.
    /// Uses a unique name, not `DEFAULT_PIPE_NAME`, so it cannot collide with
    /// a real ggg-dnd server already holding that name.
    #[tokio::test]
    async fn ggg_ipc_client_accepts_pipe_created_by_same_user() {
        let pipe_name = format!(r"\\.\pipe\ggg-dnd-test-{}", uuid::Uuid::new_v4());
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
}
