//! Chrome Native Messaging host that bridges browser extensions to ggg.
//!
//! Wire protocol on stdio (per Chrome Native Messaging spec):
//!   - 4-byte little-endian length prefix
//!   - UTF-8 JSON payload of `length` bytes
//!
//! Each incoming message is forwarded to the ggg Named Pipe, and the
//! response is wrapped and sent back over stdout. The host stays alive
//! until stdin reaches EOF, so it works with both `sendNativeMessage`
//! (one-shot) and `connectNative` (persistent port) on the extension side.
//!
//! When no ggg process serves the default pipe, the bridge starts the
//! `ggg-gui.exe` next to its own executable and retries (see `launch`).

mod launch;

use ggg_ipc::{ClientError, IpcRequest, IpcResponse};
use serde::{Deserialize, Serialize};
use std::io::{self, Read, Write};
use std::time::Duration;

/// Inbound message from the Chrome extension.
///
/// Mirrors the on-the-wire shape used by ggg-ipc so the bridge can forward
/// most requests as-is. An optional `pipe` override is allowed for testing.
#[derive(Debug, Deserialize)]
#[serde(tag = "type")]
enum BridgeRequest {
    #[serde(rename = "add_url")]
    AddUrl {
        url: String,
        #[serde(default)]
        pipe: Option<String>,
    },
    #[serde(rename = "ping")]
    Ping {
        #[serde(default)]
        pipe: Option<String>,
    },
}

/// Outbound message to the Chrome extension.
#[derive(Debug, Serialize)]
#[serde(tag = "type")]
enum BridgeResponse {
    #[serde(rename = "ok")]
    Ok { message: String },
    #[serde(rename = "pong")]
    Pong,
    #[serde(rename = "error")]
    Error { message: String },
}

fn main() {
    // The host runs as a child of Chrome with stdio piped. Read messages
    // until Chrome closes stdin, then exit cleanly.
    let stdin = io::stdin();
    let stdout = io::stdout();
    let mut stdin = stdin.lock();
    let mut stdout = stdout.lock();

    loop {
        match read_message(&mut stdin) {
            Ok(Some(payload)) => {
                let response = handle_payload(&payload);
                if let Err(e) = write_message(&mut stdout, &response) {
                    eprintln!("bridge: failed to write response: {}", e);
                    std::process::exit(1);
                }
            }
            Ok(None) => break, // EOF — Chrome closed the port
            Err(e) => {
                let response = BridgeResponse::Error {
                    message: format!("bridge: read error: {}", e),
                };
                let _ = write_message(&mut stdout, &response);
                std::process::exit(1);
            }
        }
    }
}

/// Parse the incoming JSON payload, dispatch to the pipe, and translate
/// the result into a `BridgeResponse`.
fn handle_payload(payload: &[u8]) -> BridgeResponse {
    let request: BridgeRequest = match serde_json::from_slice(payload) {
        Ok(r) => r,
        Err(e) => {
            return BridgeResponse::Error {
                message: format!("invalid request: {}", e),
            };
        }
    };

    let (pipe_name, ipc_request) = match request {
        BridgeRequest::AddUrl { url, pipe } => (resolve_pipe(pipe), IpcRequest::AddUrl { url }),
        BridgeRequest::Ping { pipe } => (resolve_pipe(pipe), IpcRequest::Ping),
    };

    match send_starting_gui(&pipe_name, &ipc_request) {
        Ok(IpcResponse::Ok { message }) => BridgeResponse::Ok { message },
        Ok(IpcResponse::Pong) => BridgeResponse::Pong,
        Ok(IpcResponse::Error { message }) => BridgeResponse::Error { message },
        Err(ClientError::Connect(_)) => BridgeResponse::Error {
            message: "ggg is not running (named pipe unavailable)".to_string(),
        },
        Err(e) => BridgeResponse::Error {
            message: e.to_string(),
        },
    }
}

/// Send the request; if nothing serves the default pipe, start the sibling GUI
/// once, wait for it to answer and send the request again.
///
/// Only a missing pipe starts the GUI: a busy or foreign-owned pipe means some
/// process is there already, and a `pipe` override from the extension never
/// starts anything.
fn send_starting_gui(
    pipe_name: &str,
    request: &IpcRequest,
) -> Result<IpcResponse, ClientError> {
    let err = match ggg_ipc::send_request(pipe_name, request) {
        Err(ClientError::Connect(e))
            if e.kind() == io::ErrorKind::NotFound && pipe_name == ggg_ipc::default_pipe_name() =>
        {
            e
        }
        other => return other,
    };

    if launch::launch_gui() && wait_for_pong(pipe_name) {
        return ggg_ipc::send_request(pipe_name, request);
    }
    Err(ClientError::Connect(err))
}

fn wait_for_pong(pipe_name: &str) -> bool {
    launch::wait_until(
        || {
            matches!(
                ggg_ipc::send_request_timeout(pipe_name, &IpcRequest::Ping, Duration::from_secs(1)),
                Ok(IpcResponse::Pong)
            )
        },
        launch::LAUNCH_WAIT,
        launch::POLL_INTERVAL,
    )
}

/// Resolve the target pipe name. The `pipe` field comes from untrusted
/// extension-supplied JSON, so honoring it in release builds would let a
/// compromised extension redirect requests to (and probe) arbitrary local
/// named pipes. The override is only allowed in debug builds or when
/// `GGG_BRIDGE_ALLOW_PIPE_OVERRIDE` is set; otherwise the default pipe is used.
fn resolve_pipe(requested: Option<String>) -> String {
    match requested {
        Some(p) if pipe_override_allowed() => p,
        Some(_) => {
            eprintln!(
                "bridge: ignoring 'pipe' override (allowed only in debug builds \
                 or with GGG_BRIDGE_ALLOW_PIPE_OVERRIDE set)"
            );
            ggg_ipc::default_pipe_name()
        }
        None => ggg_ipc::default_pipe_name(),
    }
}

fn pipe_override_allowed() -> bool {
    cfg!(debug_assertions) || std::env::var_os("GGG_BRIDGE_ALLOW_PIPE_OVERRIDE").is_some()
}

/// Read one Native Messaging frame: 4-byte LE length + payload.
/// Returns `Ok(None)` on a clean EOF before any bytes are read.
fn read_message<R: Read>(reader: &mut R) -> io::Result<Option<Vec<u8>>> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf) {
        Ok(()) => {}
        Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => return Ok(None),
        Err(e) => return Err(e),
    }

    let len = u32::from_le_bytes(len_buf) as usize;
    // Chrome's documented max for host→extension is 1 MB. Accept up to
    // a generous 4 MB on the way in to leave headroom for unusual payloads.
    const MAX_INBOUND: usize = 4 * 1024 * 1024;
    if len > MAX_INBOUND {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("inbound message too large: {} bytes", len),
        ));
    }

    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf)?;
    Ok(Some(buf))
}

/// Write one Native Messaging frame: 4-byte LE length + JSON payload.
fn write_message<W: Write>(writer: &mut W, response: &BridgeResponse) -> io::Result<()> {
    let payload = serde_json::to_vec(response)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    let len = u32::try_from(payload.len())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidData, "payload too large"))?;
    writer.write_all(&len.to_le_bytes())?;
    writer.write_all(&payload)?;
    writer.flush()?;
    Ok(())
}
