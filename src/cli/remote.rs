//! CLI routing: decides whether a command runs against a daemon over the
//! control pipe or standalone, and renders the daemon's response.
//!
//! `classify` is the only place that assigns a `RemoteClass` to a `Commands`
//! variant, so a new subcommand forces a decision (the match has no wildcard
//! arm). `route` probes the pipe once for every class but `ReadOnly`, then
//! decides among a `Running`, `NotRunning`, and `Unreachable` probe result.
//!
//! There is no control pipe at all on non-Windows targets: every class other
//! than `ReadOnly` behaves as if no daemon were reachable, so `RequiresDaemon`
//! commands exit with `DAEMON_UNREACHABLE` and everything else runs
//! standalone.

use super::{
    BridgeAction, Commands, ConfigAction, DaemonAction, DebugAction, ExportAction, FolderAction,
    ImportAction, ScriptAction, TestAction,
};
use super::error;
#[cfg(windows)]
use crate::ipc::control::protocol::{ControlErrorCode, ControlRequest, ControlResponse};
#[cfg(windows)]
use std::time::Duration;

/// How long a routed request waits for a reply. `daemon ping`/`status` and
/// `daemon stop`'s own `Shutdown` request are all cheap round trips.
#[cfg(windows)]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// How long `daemon stop` polls `probe` for the daemon to actually exit.
#[cfg(windows)]
const STOP_POLL_BUDGET: Duration = Duration::from_secs(10);
#[cfg(windows)]
const STOP_POLL_INTERVAL: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RemoteClass {
    /// Goes through the daemon when one is running, standalone otherwise.
    Routed,
    /// Never probes; always standalone. Safe because these commands only
    /// read state a daemon does not exclusively own (log files, completion
    /// history, `settings.toml` reads, ...).
    ReadOnly,
    /// Only meaningful against a daemon; standalone is never correct for it.
    RequiresDaemon,
    /// Would corrupt daemon-owned queue state if it ran standalone next to a
    /// live daemon, and is not routed (yet, or ever): refuse instead of
    /// racing the daemon's own writes.
    RefuseWhileRunning,
}

/// Exhaustive; no wildcard arm, so a new `Commands` variant fails to compile
/// here until it is assigned a class.
pub fn classify(cmd: &Commands) -> RemoteClass {
    match cmd {
        // Not yet wired to apply their writes through the daemon;
        // `RefuseWhileRunning` is the safe class until they are, since
        // running them standalone next to a live daemon would race its
        // writes to the same queue files.
        Commands::Add { .. } => RemoteClass::RefuseWhileRunning,
        Commands::List { .. } => RemoteClass::RefuseWhileRunning,
        Commands::Start { .. } => RemoteClass::RefuseWhileRunning,
        Commands::Pause { .. } => RemoteClass::RefuseWhileRunning,
        Commands::Remove { .. } => RemoteClass::RefuseWhileRunning,
        Commands::Status { .. } => RemoteClass::RefuseWhileRunning,
        Commands::StartAll { .. } => RemoteClass::RefuseWhileRunning,
        Commands::PauseAll { .. } => RemoteClass::RefuseWhileRunning,

        Commands::Config { action } => match action {
            ConfigAction::Get { .. } => RemoteClass::ReadOnly,
            ConfigAction::Show { .. } => RemoteClass::ReadOnly,
            ConfigAction::Set { .. } => RemoteClass::RefuseWhileRunning,
        },
        Commands::Logs { .. } => RemoteClass::ReadOnly,
        Commands::History { .. } => RemoteClass::ReadOnly,
        Commands::Stats { .. } => RemoteClass::ReadOnly,
        Commands::Debug { action } => match action {
            DebugAction::ManagerState { .. }
            | DebugAction::FolderSlots { .. }
            | DebugAction::Task { .. }
            | DebugAction::ValidateConfig
            | DebugAction::CheckQueue { .. } => RemoteClass::ReadOnly,
        },
        Commands::Script { action } => match action {
            ScriptAction::List { .. } => RemoteClass::ReadOnly,
            ScriptAction::Test { .. } => RemoteClass::ReadOnly,
            // Not yet wired to apply through the daemon; see the comment
            // above `Commands::Add`.
            ScriptAction::Reload => RemoteClass::RefuseWhileRunning,
            ScriptAction::Enable { .. } => RemoteClass::RefuseWhileRunning,
            ScriptAction::Disable { .. } => RemoteClass::RefuseWhileRunning,
        },
        Commands::Folder { action } => match action {
            FolderAction::List { .. } => RemoteClass::ReadOnly,
            FolderAction::Show { .. } => RemoteClass::ReadOnly,
            FolderAction::Create { .. } => RemoteClass::RefuseWhileRunning,
            FolderAction::Config { .. } => RemoteClass::RefuseWhileRunning,
            FolderAction::Delete { .. } => RemoteClass::RefuseWhileRunning,
        },
        Commands::Clear { .. } => RemoteClass::RefuseWhileRunning,
        Commands::BatchAdd { .. } => RemoteClass::RefuseWhileRunning,
        Commands::Priority { .. } => RemoteClass::RefuseWhileRunning,
        Commands::Move { .. } => RemoteClass::RefuseWhileRunning,
        Commands::Export { action } => match action {
            ExportAction::Queue { .. } | ExportAction::Config { .. } => RemoteClass::ReadOnly,
        },
        Commands::Import { action } => match action {
            ImportAction::Queue { .. } | ImportAction::Config { .. } => {
                RemoteClass::RefuseWhileRunning
            }
        },
        Commands::Test { action } => match action {
            TestAction::GenerateTasks { .. }
            | TestAction::ResetQueue
            | TestAction::ResetConfig => RemoteClass::RefuseWhileRunning,
        },
        Commands::Bridge { action } => match action {
            BridgeAction::Install { .. } | BridgeAction::Uninstall | BridgeAction::Status => {
                RemoteClass::ReadOnly
            }
        },

        Commands::Daemon { action } => match action {
            DaemonAction::Ping | DaemonAction::Status { .. } | DaemonAction::Stop => {
                RemoteClass::RequiresDaemon
            }
        },
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RouteDecision {
    /// The command was fully handled here (via the daemon, or refused); exit
    /// with this code without running the standalone handler.
    Handled(i32),
    /// No daemon is in the way; run the standalone handler as usual.
    Standalone,
}

/// Routes one command. `ReadOnly` never probes. Every other class probes
/// `pipe_name` once: a `Running` result sends the request over the pipe, a
/// `NotRunning` result returns `Standalone` (except for `RequiresDaemon`,
/// which has no standalone meaning), and `Unreachable` always exits with
/// `DAEMON_UNREACHABLE` rather than falling back to standalone.
///
/// There is a small window between the probe and a standalone write in which
/// a daemon could start: `RouteDecision::Standalone` still means "write to
/// the queue files directly," even if a daemon binds its pipe a moment
/// later. This race is accepted; closing it would need a lock shared between
/// the CLI and the daemon.
pub async fn route(cmd: &Commands, pipe_name: &str) -> RouteDecision {
    if classify(cmd) == RemoteClass::ReadOnly {
        return RouteDecision::Standalone;
    }

    #[cfg(windows)]
    {
        route_windows(cmd, pipe_name).await
    }
    #[cfg(not(windows))]
    {
        let _ = pipe_name;
        route_no_control_pipe(cmd)
    }
}

/// No control pipe exists on this target, so this behaves as if probing
/// always found no daemon: `RequiresDaemon` fails, everything else runs
/// standalone.
#[cfg(not(windows))]
fn route_no_control_pipe(cmd: &Commands) -> RouteDecision {
    match classify(cmd) {
        RemoteClass::RequiresDaemon => {
            eprintln!("no ggg daemon running");
            RouteDecision::Handled(error::DAEMON_UNREACHABLE)
        }
        _ => RouteDecision::Standalone,
    }
}

#[cfg(windows)]
async fn route_windows(cmd: &Commands, pipe_name: &str) -> RouteDecision {
    use crate::ipc::control::client::{self, DaemonProbe};

    let class = classify(cmd);
    let probe = client::probe(pipe_name).await;

    match (class, probe) {
        (RemoteClass::RefuseWhileRunning, DaemonProbe::Running { .. }) => {
            eprintln!("ggg daemon is running; use `ggg daemon stop` first");
            RouteDecision::Handled(error::DAEMON_RUNNING)
        }
        (RemoteClass::RefuseWhileRunning, DaemonProbe::NotRunning) => RouteDecision::Standalone,
        (RemoteClass::RefuseWhileRunning, DaemonProbe::Unreachable(detail)) => {
            eprintln!("daemon unreachable: {detail}");
            RouteDecision::Handled(error::DAEMON_UNREACHABLE)
        }

        (RemoteClass::Routed, DaemonProbe::Running { .. }) => send_and_render(cmd, pipe_name).await,
        (RemoteClass::Routed, DaemonProbe::NotRunning) => RouteDecision::Standalone,
        (RemoteClass::Routed, DaemonProbe::Unreachable(detail)) => {
            eprintln!("daemon unreachable: {detail}");
            RouteDecision::Handled(error::DAEMON_UNREACHABLE)
        }

        (RemoteClass::RequiresDaemon, DaemonProbe::Running { .. }) => {
            send_and_render(cmd, pipe_name).await
        }
        (RemoteClass::RequiresDaemon, DaemonProbe::NotRunning) => {
            // `daemon ping` prints its own "not running" line with no error
            // prefix; every other RequiresDaemon command gets the generic
            // message.
            if matches!(cmd, Commands::Daemon { action: DaemonAction::Ping }) {
                println!("not running");
            } else {
                eprintln!("no ggg daemon running");
            }
            RouteDecision::Handled(error::DAEMON_UNREACHABLE)
        }
        (RemoteClass::RequiresDaemon, DaemonProbe::Unreachable(detail)) => {
            eprintln!("daemon unreachable: {detail}");
            RouteDecision::Handled(error::DAEMON_UNREACHABLE)
        }

        (RemoteClass::ReadOnly, _) => unreachable!("route() returns before probing ReadOnly commands"),
    }
}

/// Sends the request a `Routed`/`RequiresDaemon` command maps to and renders
/// the reply. Only reached once a `Running` probe makes the round trip worth
/// attempting.
#[cfg(windows)]
async fn send_and_render(cmd: &Commands, pipe_name: &str) -> RouteDecision {
    use crate::ipc::control::client;

    match cmd {
        Commands::Daemon { action: DaemonAction::Ping } => {
            match client::send(pipe_name, &ControlRequest::Ping, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Pong { protocol, pid, .. }) => {
                    println!("ggg daemon running (pid {pid}, protocol {protocol})");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
        Commands::Daemon { action: DaemonAction::Status { json } } => {
            match client::send(pipe_name, &ControlRequest::Status, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Status { status }) => {
                    print_daemon_status(&status, *json);
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
        Commands::Daemon { action: DaemonAction::Stop } => stop_daemon(pipe_name).await,

        // No other command is mapped to a request yet: classify() only
        // marks the ones above as Routed/RequiresDaemon so far.
        _ => {
            debug_assert!(
                false,
                "send_and_render reached for a command with no request mapping"
            );
            eprintln!("this command is not yet routed to the daemon");
            RouteDecision::Handled(error::ERROR)
        }
    }
}

/// `daemon stop`: send `Shutdown`, then poll `probe` until the pipe is gone.
/// The daemon answers `Ok` before it actually shuts down (`after_response`
/// runs after the reply is flushed), so
/// `Running`/`Unreachable` right after the reply just means shutdown hasn't
/// finished draining connections yet.
#[cfg(windows)]
async fn stop_daemon(pipe_name: &str) -> RouteDecision {
    use crate::ipc::control::client::{self, DaemonProbe};
    use std::time::Instant;

    let deadline = Instant::now() + STOP_POLL_BUDGET;

    match client::send(pipe_name, &ControlRequest::Shutdown, Some(REQUEST_TIMEOUT)).await {
        Ok(ControlResponse::Ok { .. }) => {}
        Ok(other) => return render_unexpected(other),
        Err(e) => {
            // The daemon can shut down before answering (e.g. a racing
            // second `daemon stop`); one more probe tells that apart from a
            // real send failure instead of always reporting unreachable.
            if matches!(client::probe(pipe_name).await, DaemonProbe::NotRunning) {
                println!("ggg daemon stopped");
                return RouteDecision::Handled(error::SUCCESS);
            }
            return render_send_error(&e);
        }
    }

    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        // A timed-out probe is treated the same as `Running`/`Unreachable`:
        // the daemon just hasn't finished draining connections yet.
        let probe = tokio::time::timeout(remaining, client::probe(pipe_name)).await;
        match probe {
            Ok(DaemonProbe::NotRunning) => {
                println!("ggg daemon stopped");
                return RouteDecision::Handled(error::SUCCESS);
            }
            Ok(DaemonProbe::Running { .. }) | Ok(DaemonProbe::Unreachable(_)) | Err(_) => {
                if Instant::now() >= deadline {
                    eprintln!("timed out waiting for the daemon to stop");
                    return RouteDecision::Handled(error::TIMEOUT);
                }
                tokio::time::sleep(STOP_POLL_INTERVAL).await;
            }
        }
    }
}

#[cfg(windows)]
fn render_unexpected(resp: ControlResponse) -> RouteDecision {
    if let ControlResponse::Error { code, message } = resp {
        eprintln!("Error: {message}");
        return RouteDecision::Handled(exit_code_for(code));
    }
    eprintln!("unexpected response from daemon: {resp:?}");
    RouteDecision::Handled(error::ERROR)
}

#[cfg(windows)]
fn render_send_error(e: &anyhow::Error) -> RouteDecision {
    // Every `send` failure -- including the owner mismatch check -- means
    // the daemon is required but unreachable; never fall back to standalone,
    // since that would race a squatter or a daemon that is mid-shutdown.
    eprintln!("Error: {e}");
    RouteDecision::Handled(error::DAEMON_UNREACHABLE)
}

#[cfg(windows)]
fn exit_code_for(code: ControlErrorCode) -> i32 {
    match code {
        ControlErrorCode::NotFound => error::NOT_FOUND,
        ControlErrorCode::InvalidInput => error::INVALID_INPUT,
        ControlErrorCode::Unsupported | ControlErrorCode::Internal => error::ERROR,
    }
}

#[cfg(windows)]
fn print_daemon_status(status: &crate::ipc::control::protocol::DaemonStatus, json: bool) {
    if json {
        match serde_json::to_string_pretty(status) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to format daemon status: {e}"),
        }
        return;
    }
    println!("pid: {}", status.pid);
    println!("version: {}", status.version);
    println!("protocol: {}", status.protocol);
    println!("uptime_secs: {}", status.uptime_secs);
    println!("pipe: {}", status.pipe_name);
    println!("config_dir: {}", status.config_dir);
    println!("scripts_active: {}", status.scripts_active);
    println!("pending: {}", status.pending);
    println!("downloading: {}", status.downloading);
    println!("paused: {}", status.paused);
    println!("error: {}", status.error);
    println!("history: {}", status.history);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_smoke() {
        assert_eq!(
            classify(&Commands::List { json: false }),
            RemoteClass::RefuseWhileRunning
        );
        assert_eq!(
            classify(&Commands::Logs { follow: false, level: None, lines: None }),
            RemoteClass::ReadOnly
        );
        assert_eq!(
            classify(&Commands::Clear { status: "completed".to_string(), folder: None }),
            RemoteClass::RefuseWhileRunning
        );
        assert_eq!(
            classify(&Commands::Daemon { action: DaemonAction::Ping }),
            RemoteClass::RequiresDaemon
        );
    }
}
