//! One GUI per configuration directory. The per-directory instance pipe doubles
//! as the lock: whoever creates its first instance is the primary, and any
//! later start hands its request over that pipe and exits.

#[cfg(windows)]
pub struct GuiIpc {
    pub tx: tokio::sync::mpsc::Sender<ggg::ipc::pipe_server::IpcEvent>,
    pub rx: tokio::sync::mpsc::Receiver<ggg::ipc::pipe_server::IpcEvent>,
}

#[cfg(not(windows))]
pub struct GuiIpc;

/// What start-up decided.
pub enum Start {
    /// Carry on and open the window.
    Primary(GuiIpc),
    /// Another instance took over; leave with this code.
    #[cfg_attr(not(windows), allow(dead_code))]
    Done(std::process::ExitCode),
}

#[cfg(not(windows))]
pub fn acquire(_rt: &tokio::runtime::Runtime, _urls: &[String]) -> Start {
    Start::Primary(GuiIpc)
}

/// The instance pipe carries requests of at most this size; the server stops
/// reading a line at 64 KiB, so keep clear of it.
#[cfg(windows)]
const MAX_REQUEST_BYTES: usize = 60 * 1024;

/// `ERROR_PIPE_BUSY`: every instance of the pipe is in use, which happens for
/// a moment while the primary creates the next one.
#[cfg(windows)]
const ERROR_PIPE_BUSY: i32 = 231;

#[cfg(windows)]
const BUSY_RETRIES: u32 = 5;
#[cfg(windows)]
const BUSY_RETRY_DELAY: std::time::Duration = std::time::Duration::from_millis(100);
#[cfg(windows)]
const FORWARD_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

#[cfg(windows)]
enum Forward {
    Delivered,
    /// The primary went away before it could answer; claiming may work now.
    PrimaryGone,
    Failed,
}

/// Becomes the primary instance for the current config directory or hands the
/// start over to the running one. Needs the runtime only for its context: the
/// pipe types register with its reactor.
#[cfg(windows)]
pub fn acquire(rt: &tokio::runtime::Runtime, urls: &[String]) -> Start {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    let _enter = rt.enter();
    match claim_or_forward(&tx, urls) {
        Some(code) => Start::Done(code),
        None => Start::Primary(GuiIpc { tx, rx }),
    }
}

/// `None`: this process is the primary (serving the instance pipe when it
/// could claim it). `Some(code)`: another instance took over; exit with `code`.
#[cfg(windows)]
fn claim_or_forward(
    tx: &tokio::sync::mpsc::Sender<ggg::ipc::pipe_server::IpcEvent>,
    urls: &[String],
) -> Option<std::process::ExitCode> {
    use ggg::ipc::{
        instance::gui_instance_pipe_name,
        pipe_server::{claim_pipe, is_already_claimed, serve_pipe},
    };
    use std::process::ExitCode;

    let config_dir = match ggg::util::paths::find_config_directory() {
        Ok(dir) => dir,
        Err(e) => {
            tracing::warn!("Single-instance check skipped: {e:#}");
            return None;
        }
    };
    let name = gui_instance_pipe_name(&config_dir);

    match claim_pipe(&name) {
        Ok(claimed) => {
            // Served before the core loads so a second start gets its answer
            // at once; its request waits in the channel.
            serve_pipe(claimed, tx.clone());
            None
        }
        Err(e) if is_already_claimed(&e) => match forward_to_primary(&name, urls) {
            Forward::Delivered => Some(ExitCode::SUCCESS),
            Forward::PrimaryGone => match claim_pipe(&name) {
                Ok(claimed) => {
                    serve_pipe(claimed, tx.clone());
                    None
                }
                Err(e) => {
                    tracing::error!("Cannot reach or replace the running instance: {e}");
                    Some(ExitCode::FAILURE)
                }
            },
            Forward::Failed => Some(ExitCode::FAILURE),
        },
        Err(e) => {
            tracing::warn!("Cannot claim the instance pipe {name}; starting without it: {e}");
            None
        }
    }
}

#[cfg(windows)]
fn forward_to_primary(name: &str, urls: &[String]) -> Forward {
    use ggg_ipc::{ClientError, IpcRequest, IpcResponse, send_request_allowing_foreground};

    let request = IpcRequest::Activate { urls: urls.to_vec() };
    match serde_json::to_string(&request) {
        Ok(json) if json.len() <= MAX_REQUEST_BYTES => {}
        Ok(_) => {
            tracing::error!("too many URLs on the command line");
            return Forward::Failed;
        }
        Err(e) => {
            tracing::error!("Cannot encode the activation request: {e}");
            return Forward::Failed;
        }
    }

    let mut busy_retries = 0;
    loop {
        match send_request_allowing_foreground(name, &request, FORWARD_TIMEOUT) {
            Ok(IpcResponse::Ok { .. }) => {
                tracing::info!(
                    "another ggg-gui instance owns this config directory; \
                     forwarded activation with {} URL(s)",
                    urls.len()
                );
                return Forward::Delivered;
            }
            Err(ClientError::Connect(e))
                if e.raw_os_error() == Some(ERROR_PIPE_BUSY) && busy_retries < BUSY_RETRIES =>
            {
                busy_retries += 1;
                std::thread::sleep(BUSY_RETRY_DELAY);
            }
            Err(ClientError::Connect(e)) => {
                tracing::warn!("The running instance is not answering: {e}");
                return Forward::PrimaryGone;
            }
            Ok(other) => {
                tracing::error!("The running instance refused the activation: {other:?}");
                return Forward::Failed;
            }
            Err(e) => {
                tracing::error!("Cannot hand over to the running instance: {e}");
                return Forward::Failed;
            }
        }
    }
}
