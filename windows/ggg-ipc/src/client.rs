//! Synchronous Named Pipe client for ggg.
//!
//! Each call opens a transient connection, sends one request, reads one
//! response, then closes. This avoids holding pipe handles long-term and
//! keeps the TUI server free to accept other clients.

use crate::protocol::{IpcRequest, IpcResponse};
use std::fmt;
use std::fs::{File, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::windows::io::AsRawHandle;
use std::time::Duration;

/// Default time to wait for a server response before giving up. Prevents a
/// hung or half-open server from blocking the caller forever.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(10);

/// Errors returned from the synchronous pipe client.
#[derive(Debug)]
pub enum ClientError {
    /// Could not open the pipe (server not running, permission denied, etc.)
    Connect(std::io::Error),
    /// I/O failed mid-transaction
    Io(std::io::Error),
    /// Outgoing request could not be serialized
    Serialize(serde_json::Error),
    /// Incoming response could not be parsed
    Deserialize(serde_json::Error),
    /// Server returned an `IpcResponse::Error`
    Server(String),
    /// Server returned a response of an unexpected variant
    Unexpected(String),
    /// The pipe's owner SID is not one of ours; see `ggg_ipc::owner`.
    Untrusted(String),
}

impl fmt::Display for ClientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Connect(e) => write!(f, "Connection failed: {}", e),
            Self::Io(e) => write!(f, "I/O failed: {}", e),
            Self::Serialize(e) => write!(f, "Failed to serialize request: {}", e),
            Self::Deserialize(e) => write!(f, "Invalid response: {}", e),
            Self::Server(msg) => write!(f, "Server error: {}", msg),
            Self::Unexpected(msg) => write!(f, "Unexpected response: {}", msg),
            Self::Untrusted(msg) => write!(f, "Refusing to use pipe: {}", msg),
        }
    }
}

impl std::error::Error for ClientError {}

/// Send a single request and return the parsed response, bounded by
/// `DEFAULT_TIMEOUT` so a hung server cannot block the caller indefinitely.
pub fn send_request(pipe_name: &str, request: &IpcRequest) -> Result<IpcResponse, ClientError> {
    send_request_timeout(pipe_name, request, DEFAULT_TIMEOUT)
}

/// Like [`send_request`] but with an explicit timeout. The blocking transaction
/// runs on a worker thread; if it does not complete within `timeout`, a
/// `TimedOut` error is returned (the worker is abandoned and exits with the
/// process — std pipe reads cannot be cancelled mid-flight).
pub fn send_request_timeout(
    pipe_name: &str,
    request: &IpcRequest,
    timeout: Duration,
) -> Result<IpcResponse, ClientError> {
    send_with_timeout(pipe_name, request, timeout, false)
}

/// Like [`send_request_timeout`], but after the owner check and before
/// writing, reads the server's pid with `GetNamedPipeServerProcessId` and
/// calls `AllowSetForegroundWindow(pid)`, so the server may take the
/// foreground. A failure of either call is ignored (the request is still
/// sent). The pid is granted rather than `ASFW_ANY` so that only the
/// owner-checked server gets the right.
pub fn send_request_allowing_foreground(
    pipe_name: &str,
    request: &IpcRequest,
    timeout: Duration,
) -> Result<IpcResponse, ClientError> {
    send_with_timeout(pipe_name, request, timeout, true)
}

fn send_with_timeout(
    pipe_name: &str,
    request: &IpcRequest,
    timeout: Duration,
    allow_foreground: bool,
) -> Result<IpcResponse, ClientError> {
    let pipe_name = pipe_name.to_string();
    let request = request.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let _ = tx.send(send_request_blocking(&pipe_name, &request, allow_foreground));
    });

    match rx.recv_timeout(timeout) {
        Ok(result) => result,
        Err(_) => Err(ClientError::Io(std::io::Error::new(
            std::io::ErrorKind::TimedOut,
            "IPC request timed out waiting for a server response",
        ))),
    }
}

/// The blocking request/response transaction (open, write, read one line).
fn send_request_blocking(
    pipe_name: &str,
    request: &IpcRequest,
    allow_foreground: bool,
) -> Result<IpcResponse, ClientError> {
    let pipe = open_pipe(pipe_name).map_err(ClientError::Connect)?;
    // The pipe name is predictable, so another local user could squat it
    // before we connect; check the pipe's owner SID before writing anything.
    crate::owner::verify_pipe_owner(pipe.as_raw_handle())
        .map_err(|e| ClientError::Untrusted(e.user_message()))?;
    if allow_foreground {
        // Best effort: without the grant the server just may not come to
        // the front, which must not fail the request itself.
        if let Some(pid) = pipe_server_pid(&pipe) {
            // SAFETY: AllowSetForegroundWindow takes a plain process id and
            // has no pointer arguments.
            unsafe {
                win32::AllowSetForegroundWindow(pid);
            }
        }
    }
    let mut reader = BufReader::new(&pipe);
    let mut writer = &pipe;

    let mut json = serde_json::to_string(request).map_err(ClientError::Serialize)?;
    json.push('\n');
    writer.write_all(json.as_bytes()).map_err(ClientError::Io)?;
    writer.flush().map_err(ClientError::Io)?;

    let mut line = String::new();
    reader.read_line(&mut line).map_err(ClientError::Io)?;

    serde_json::from_str::<IpcResponse>(&line).map_err(ClientError::Deserialize)
}

/// Send an `add_url` request. Returns the server's success message on `Ok`,
/// or a `ClientError::Server` if the server rejected the URL.
pub fn send_url(pipe_name: &str, url: &str) -> Result<String, ClientError> {
    let request = IpcRequest::AddUrl { url: url.to_string() };
    match send_request(pipe_name, &request)? {
        IpcResponse::Ok { message } => Ok(message),
        IpcResponse::Error { message } => Err(ClientError::Server(message)),
        IpcResponse::Pong => Err(ClientError::Unexpected(
            "pong received in response to add_url".to_string(),
        )),
    }
}

/// Send a `ping` request. Returns `Ok(())` if the server replied with `Pong`.
pub fn ping(pipe_name: &str) -> Result<(), ClientError> {
    match send_request(pipe_name, &IpcRequest::Ping)? {
        IpcResponse::Pong => Ok(()),
        IpcResponse::Ok { message } | IpcResponse::Error { message } => {
            Err(ClientError::Unexpected(message))
        }
    }
}

/// Opens the pipe, runs the owner check, returns the server's process id and
/// closes the pipe again. Lets tests find (and kill) a server process they
/// did not spawn directly.
pub fn server_process_id(pipe_name: &str) -> Result<u32, ClientError> {
    let pipe = open_pipe(pipe_name).map_err(ClientError::Connect)?;
    crate::owner::verify_pipe_owner(pipe.as_raw_handle())
        .map_err(|e| ClientError::Untrusted(e.user_message()))?;
    let mut pid: u32 = 0;
    // SAFETY: `pipe` is a live handle owned by this function and `pid` is a
    // valid out-param.
    let ok = unsafe { win32::GetNamedPipeServerProcessId(pipe.as_raw_handle().cast(), &mut pid) };
    if ok == 0 {
        return Err(ClientError::Io(std::io::Error::last_os_error()));
    }
    Ok(pid)
}

/// Server pid of an open pipe client handle, or `None` if the query fails.
fn pipe_server_pid(pipe: &File) -> Option<u32> {
    let mut pid: u32 = 0;
    // SAFETY: `pipe` is a live handle borrowed for the duration of the call
    // and `pid` is a valid out-param.
    let ok = unsafe { win32::GetNamedPipeServerProcessId(pipe.as_raw_handle().cast(), &mut pid) };
    (ok != 0).then_some(pid)
}

mod win32 {
    type Handle = *mut core::ffi::c_void;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        pub fn GetNamedPipeServerProcessId(pipe: Handle, pid: *mut u32) -> i32;
    }
    #[link(name = "user32")]
    unsafe extern "system" {
        pub fn AllowSetForegroundWindow(pid: u32) -> i32;
    }
}

/// Open a Named Pipe for reading and writing using standard file I/O.
///
/// On Windows, named pipes appear as files under the `\\.\pipe\` namespace.
fn open_pipe(pipe_name: &str) -> std::io::Result<File> {
    OpenOptions::new()
        .read(true)
        .write(true)
        .open(pipe_name)
}
