//! Client side of the daemon control pipe: `probe`, used by `bind` itself
//! to tell a live daemon apart from a stale pipe name, and later by CLI
//! routing to decide remote vs. standalone. `send` (request/response for
//! real commands) is added together with CLI routing.

use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

use super::protocol::{ControlRequest, ControlResponse, CONTROL_PROTOCOL_VERSION};

/// `ERROR_PIPE_BUSY`: every instance is currently connected to another client.
const ERROR_PIPE_BUSY: i32 = 231;
/// How long `probe` retries a busy pipe before giving up.
const PIPE_BUSY_RETRY_BUDGET: Duration = Duration::from_secs(2);
const PIPE_BUSY_RETRY_INTERVAL: Duration = Duration::from_millis(50);
/// Bounds the ping/pong round trip once connected.
const PING_TIMEOUT: Duration = Duration::from_secs(2);
/// A `Pong` reply is tiny; this only guards against a misbehaving peer.
const MAX_PROBE_RESPONSE_BYTES: u64 = 64 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DaemonProbe {
    Running { pid: u32, protocol: u32 },
    NotRunning,
    Unreachable(String),
}

/// Tells an actively-serving daemon from an absent or unresponsive one by
/// opening the pipe and requiring a valid `Ping`/`Pong` round trip; a pipe
/// name that exists but never answers is `Unreachable`, not `Running`.
pub async fn probe(pipe_name: &str) -> DaemonProbe {
    let deadline = Instant::now() + PIPE_BUSY_RETRY_BUDGET;
    let client = loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(c) => break c,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return DaemonProbe::NotRunning,
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                if Instant::now() >= deadline {
                    return DaemonProbe::Unreachable("pipe busy".to_string());
                }
                tokio::time::sleep(PIPE_BUSY_RETRY_INTERVAL).await;
            }
            Err(e) => return DaemonProbe::Unreachable(format!("failed to open pipe: {e}")),
        }
    };

    match tokio::time::timeout(PING_TIMEOUT, ping(client)).await {
        Ok(Ok(ControlResponse::Pong { protocol, pid, .. })) => {
            if protocol != CONTROL_PROTOCOL_VERSION {
                DaemonProbe::Unreachable(format!(
                    "protocol mismatch: daemon speaks {protocol}, client speaks {CONTROL_PROTOCOL_VERSION}"
                ))
            } else {
                DaemonProbe::Running { pid, protocol }
            }
        }
        Ok(Ok(other)) => DaemonProbe::Unreachable(format!("unexpected reply to ping: {other:?}")),
        Ok(Err(e)) => DaemonProbe::Unreachable(e.to_string()),
        Err(_) => DaemonProbe::Unreachable("ping timed out".to_string()),
    }
}

async fn ping(mut client: NamedPipeClient) -> std::io::Result<ControlResponse> {
    let mut line = serde_json::to_string(&ControlRequest::Ping).map_err(std::io::Error::other)?;
    line.push('\n');
    client.write_all(line.as_bytes()).await?;
    client.flush().await?;

    let mut reader = BufReader::new(client);
    let mut buf = Vec::new();
    let n = (&mut reader)
        .take(MAX_PROBE_RESPONSE_BYTES)
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Err(std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "pipe closed before responding to ping",
        ));
    }
    serde_json::from_str(String::from_utf8_lossy(&buf).trim()).map_err(std::io::Error::other)
}
