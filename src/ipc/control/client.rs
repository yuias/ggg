//! Client side of the daemon control pipe: `probe`, used by `bind` itself to
//! tell a live daemon apart from a stale pipe name and by CLI routing to
//! decide remote vs. standalone, and `send`, used by CLI routing to carry out
//! a routed request. `send` runs the same `owner::verify_server_owner` check
//! as `probe` does here: right after opening the pipe and before its first
//! write, returning `Err(anyhow!(owner::OWNER_MISMATCH_MSG))` on mismatch so
//! `route` exits 5.

use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::net::windows::named_pipe::{ClientOptions, NamedPipeClient};

use super::owner::{self, OWNER_MISMATCH_MSG};
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
/// Caps a single `send` response line; larger than the probe cap because a
/// `Tasks`/`List` response can carry many `DownloadTask`s.
const MAX_SEND_RESPONSE_BYTES: u64 = 16 * 1024 * 1024;

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

    if let Err(detail) = owner::verify_server_owner(&client) {
        tracing::warn!("control pipe {pipe_name} failed server owner check: {detail}");
        return DaemonProbe::Unreachable(OWNER_MISMATCH_MSG.to_string());
    }

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

/// Opens a fresh connection, verifies the server-owner check, sends one
/// request line, and reads one response line. Every call opens its own
/// connection: the accept loop always keeps a spare instance available
/// (`server.rs`), so there is no benefit to holding a connection open across
/// CLI commands, and it keeps this function stateless. `timeout = None`
/// waits without a limit; routing decides how long to wait per request.
pub async fn send(
    pipe_name: &str,
    req: &ControlRequest,
    timeout: Option<Duration>,
) -> anyhow::Result<ControlResponse> {
    let client = open_for_send(pipe_name).await?;

    if let Err(detail) = owner::verify_server_owner(&client) {
        tracing::warn!("control pipe {pipe_name} failed server owner check: {detail}");
        return Err(anyhow::anyhow!(OWNER_MISMATCH_MSG));
    }

    let round_trip = send_request(client, req);
    match timeout {
        Some(d) => tokio::time::timeout(d, round_trip)
            .await
            .map_err(|_| anyhow::anyhow!("control pipe request timed out"))?,
        None => round_trip.await,
    }
}

async fn open_for_send(pipe_name: &str) -> anyhow::Result<NamedPipeClient> {
    let deadline = Instant::now() + PIPE_BUSY_RETRY_BUDGET;
    loop {
        match ClientOptions::new().open(pipe_name) {
            Ok(c) => return Ok(c),
            Err(e) if e.raw_os_error() == Some(ERROR_PIPE_BUSY) => {
                if Instant::now() >= deadline {
                    return Err(anyhow::anyhow!("control pipe busy"));
                }
                tokio::time::sleep(PIPE_BUSY_RETRY_INTERVAL).await;
            }
            Err(e) => return Err(anyhow::anyhow!("failed to open control pipe: {e}")),
        }
    }
}

async fn send_request(
    mut client: NamedPipeClient,
    req: &ControlRequest,
) -> anyhow::Result<ControlResponse> {
    let mut line = serde_json::to_string(req)?;
    line.push('\n');
    client.write_all(line.as_bytes()).await?;
    client.flush().await?;

    let mut reader = BufReader::new(client);
    let mut buf = Vec::new();
    let n = (&mut reader)
        .take(MAX_SEND_RESPONSE_BYTES)
        .read_until(b'\n', &mut buf)
        .await?;
    if n == 0 {
        return Err(anyhow::anyhow!("control pipe closed before responding"));
    }
    if !buf.ends_with(b"\n") {
        return if n as u64 == MAX_SEND_RESPONSE_BYTES {
            Err(anyhow::anyhow!("daemon response exceeds 16 MiB"))
        } else {
            Err(anyhow::anyhow!("control pipe closed mid-response"))
        };
    }
    let resp: ControlResponse = serde_json::from_str(String::from_utf8_lossy(&buf).trim())?;
    Ok(resp)
}
