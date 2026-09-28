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
use super::output;
// `protocol` is compiled on every target (see `src/ipc/control/mod.rs`), so
// the `add` payload types are available to render standalone output too.
use crate::ipc::control::protocol::{AddedTask, MoveOp, SkippedUrl, WaitOutcome};
#[cfg(windows)]
use crate::ipc::control::protocol::{
    ControlErrorCode, ControlRequest, ControlResponse, HeaderEdit,
};
use crate::download::task::{DownloadStatus, DownloadTask};
#[cfg(windows)]
use std::time::Duration;

/// How long a routed request waits for a reply. `daemon ping`/`status` and
/// `daemon stop`'s own `Shutdown` request are all cheap round trips.
#[cfg(windows)]
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
/// Added on top of `wait`'s own `--timeout` for the client-side `send` bound,
/// so the server-side timeout (which answers `Timeout` on its own) always
/// has a chance to reply before the client gives up on the round trip.
#[cfg(windows)]
const WAIT_EXTRA_TIMEOUT: Duration = Duration::from_secs(10);
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
        // Goes through `ControlRequest::Add` / shared `enqueue`, same as the
        // standalone path.
        Commands::Add { .. } => RemoteClass::Routed,

        Commands::List { .. } => RemoteClass::Routed,
        // `--wait` is sent as `Start` followed by a `Wait` request.
        Commands::Start { .. } => RemoteClass::Routed,
        Commands::Pause { .. } => RemoteClass::Routed,
        Commands::Remove { .. } => RemoteClass::Routed,
        Commands::Status { .. } => RemoteClass::Routed,
        Commands::StartAll { .. } => RemoteClass::Routed,
        Commands::PauseAll { .. } => RemoteClass::Routed,
        Commands::Modify { .. } => RemoteClass::Routed,

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
            ScriptAction::Reload => RemoteClass::Routed,
            ScriptAction::Enable { .. } => RemoteClass::Routed,
            ScriptAction::Disable { .. } => RemoteClass::Routed,
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
        Commands::Priority { .. } => RemoteClass::Routed,
        Commands::Move { .. } => RemoteClass::Routed,
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
        Commands::Wait { .. } => RemoteClass::RequiresDaemon,
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
        Commands::Add { url, folder, referrer, header, immediate, quiet, json } => {
            let mut headers = Vec::with_capacity(header.len());
            for h in header {
                match parse_header_arg(h) {
                    Ok(pair) => headers.push(pair),
                    Err(e) => {
                        eprintln!("Error: {e}");
                        return RouteDecision::Handled(error::INVALID_INPUT);
                    }
                }
            }
            // `url` travels unexpanded; the daemon expands range patterns
            // itself so a single request can still report per-URL results.
            let req = ControlRequest::Add {
                url: url.clone(),
                folder: folder.clone(),
                referrer: referrer.as_ref().map(|r| r.0.clone()),
                headers,
                immediate: *immediate,
            };
            match client::send(pipe_name, &req, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Added { added, skipped }) => {
                    RouteDecision::Handled(render_added(&added, &skipped, *quiet, *json))
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
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

        Commands::List { json, status, folder } => {
            let statuses = match parse_status_arg(status.as_deref()) {
                Ok(s) => s,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            let req = ControlRequest::List { statuses, folder: folder.clone() };
            match client::send(pipe_name, &req, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Tasks { tasks }) => {
                    render_tasks(tasks, *json);
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
        Commands::Status { id, json } => {
            let uid = match super::handler::parse_task_id(id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            match client::send(pipe_name, &ControlRequest::Get { id: uid }, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Task { task, in_history }) => {
                    render_task(task, in_history, *json);
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
        Commands::Start { id, wait } => {
            let uid = match super::handler::parse_task_id(id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            match client::send(pipe_name, &ControlRequest::Start { id: uid }, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => println!("{message}"),
                Ok(other) => return render_unexpected(other),
                Err(e) => return render_send_error(&e),
            }
            if *wait {
                // `start --wait` has no `--timeout` flag of its own; it waits
                // without a client-side deadline, same as the standalone path.
                send_wait(pipe_name, uid, None, false).await
            } else {
                RouteDecision::Handled(error::SUCCESS)
            }
        }
        Commands::Wait { id, timeout, json } => {
            let uid = match super::handler::parse_task_id(id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            send_wait(pipe_name, uid, *timeout, *json).await
        }
        Commands::Pause { id } => {
            let uid = match super::handler::parse_task_id(id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            match client::send(pipe_name, &ControlRequest::Pause { id: uid }, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => {
                    println!("{message}");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
        Commands::Remove { id } => {
            let uid = match super::handler::parse_task_id(id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            match client::send(pipe_name, &ControlRequest::Remove { id: uid }, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => {
                    println!("{message}");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
        Commands::StartAll { folder } => {
            let req = ControlRequest::StartAll { folder: folder.clone() };
            match client::send(pipe_name, &req, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => {
                    println!("{message}");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
        Commands::PauseAll { folder } => {
            let req = ControlRequest::PauseAll { folder: folder.clone() };
            match client::send(pipe_name, &req, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => {
                    println!("{message}");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }

        Commands::Modify { id, referrer, clear_referrer, header, remove_header } => {
            let uid = match super::handler::parse_task_id(id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            let mut set_headers = Vec::with_capacity(header.len());
            for h in header {
                match parse_header_arg(h) {
                    Ok(pair) => set_headers.push(pair),
                    Err(e) => {
                        eprintln!("Error: {e}");
                        return RouteDecision::Handled(error::INVALID_INPUT);
                    }
                }
            }
            let edit = HeaderEdit {
                referrer: referrer.as_ref().map(|r| r.0.clone()),
                clear_referrer: *clear_referrer,
                set_headers,
                remove_headers: remove_header.clone(),
            };
            let req = ControlRequest::Modify { id: uid, edit };
            match client::send(pipe_name, &req, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Task { task, .. }) => {
                    render_modified(task);
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }

        Commands::Priority { id, set } => {
            let uid = match super::handler::parse_task_id(id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            let req = ControlRequest::Priority { id: uid, priority: *set };
            match client::send(pipe_name, &req, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => {
                    println!("{message}");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }

        Commands::Move { id, to_top, to_bottom, before, folder } => {
            let uid = match super::handler::parse_task_id(id) {
                Ok(u) => u,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            let op = match move_op_from_args(*to_top, *to_bottom, before.as_deref(), folder.as_deref()) {
                Ok(op) => op,
                Err(e) => {
                    eprintln!("Error: {e}");
                    return RouteDecision::Handled(error::INVALID_INPUT);
                }
            };
            let req = ControlRequest::Move { id: uid, op };
            match client::send(pipe_name, &req, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => {
                    println!("{message}");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }

        Commands::Script { action: ScriptAction::Reload } => {
            match client::send(pipe_name, &ControlRequest::ReloadScripts, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => {
                    println!("{message}");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
        Commands::Script { action: ScriptAction::Enable { name } } => {
            let req = ControlRequest::SetScriptEnabled { name: name.clone(), enabled: true };
            match client::send(pipe_name, &req, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => {
                    println!("{message}");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }
        Commands::Script { action: ScriptAction::Disable { name } } => {
            let req = ControlRequest::SetScriptEnabled { name: name.clone(), enabled: false };
            match client::send(pipe_name, &req, Some(REQUEST_TIMEOUT)).await {
                Ok(ControlResponse::Ok { message }) => {
                    println!("{message}");
                    RouteDecision::Handled(error::SUCCESS)
                }
                Ok(other) => render_unexpected(other),
                Err(e) => render_send_error(&e),
            }
        }

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

/// Sends a `Wait` request and renders its outcome. `timeout_secs` is the
/// server-side deadline (`ControlRequest::Wait`'s own field); the client's
/// `send` bound is `timeout_secs + WAIT_EXTRA_TIMEOUT` when set, or
/// unbounded when not, so the server's own `Timeout` answer always has room
/// to arrive before the client gives up on the connection itself.
#[cfg(windows)]
async fn send_wait(pipe_name: &str, id: uuid::Uuid, timeout_secs: Option<u64>, json: bool) -> RouteDecision {
    use crate::ipc::control::client;

    let req = ControlRequest::Wait { id, timeout_secs };
    // `saturating_add`: an enormous `--timeout` (up to `u64::MAX` seconds)
    // must clamp to `Duration::MAX` instead of panicking on overflow.
    let send_timeout = timeout_secs.map(|secs| Duration::from_secs(secs).saturating_add(WAIT_EXTRA_TIMEOUT));
    match client::send(pipe_name, &req, send_timeout).await {
        Ok(ControlResponse::Waited { outcome, task }) => {
            render_waited(outcome, task, json);
            RouteDecision::Handled(exit_code_for_wait(outcome))
        }
        Ok(other) => render_unexpected(other),
        Err(e) => render_send_error(&e),
    }
}

/// Renders a `Wait` outcome. The JSON form redacts header values the same
/// way `render_task` does; a waited-on task can carry a `Cookie` or
/// `Authorization` header just like any other. Shared by the daemon-routed
/// `Wait` request and standalone `start --wait`.
pub(crate) fn render_waited(outcome: WaitOutcome, task: Option<DownloadTask>, json: bool) {
    if json {
        #[derive(serde::Serialize)]
        struct WaitedPayload {
            outcome: WaitOutcome,
            task: Option<DownloadTask>,
        }
        let payload = WaitedPayload {
            outcome,
            task: task.map(redact_task_headers),
        };
        match serde_json::to_string_pretty(&payload) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to format wait result: {e}"),
        }
        return;
    }
    match outcome {
        WaitOutcome::Completed => println!("Download completed"),
        WaitOutcome::Failed => println!("Download failed"),
        WaitOutcome::Paused => println!("Download paused"),
        WaitOutcome::Removed => println!("Download removed"),
        WaitOutcome::Timeout => println!("Timed out waiting for download"),
    }
}

/// Shared by the daemon-routed `Wait` request and standalone `start --wait`.
pub(crate) fn exit_code_for_wait(outcome: WaitOutcome) -> i32 {
    match outcome {
        WaitOutcome::Completed => error::SUCCESS,
        WaitOutcome::Failed => error::TASK_FAILED,
        WaitOutcome::Paused => error::TASK_PAUSED,
        WaitOutcome::Removed => error::NOT_FOUND,
        WaitOutcome::Timeout => error::TIMEOUT,
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

/// Splits a `--header` argument at its first `:`, trims both sides, and
/// validates them as an HTTP header name/value (rejects CR/LF and other
/// bytes `reqwest`'s header types refuse, which would otherwise let a
/// crafted `--header` inject extra request headers). An empty value stays
/// allowed; an empty name does not. Never echoes `s`, `name`, or `value`
/// back in an error: the value may carry a secret (e.g. a `Cookie` or
/// `Authorization` header) that must not land in stderr.
pub fn parse_header_arg(s: &str) -> anyhow::Result<(String, String)> {
    let (name, value) = s
        .split_once(':')
        .ok_or_else(|| anyhow::anyhow!("invalid header: expected \"Name: value\""))?;
    let name = name.trim();
    if name.is_empty() {
        return Err(anyhow::anyhow!("invalid header: empty name"));
    }
    let value = value.trim();

    reqwest::header::HeaderName::from_bytes(name.as_bytes())
        .map_err(|_| anyhow::anyhow!("invalid header: name is not a valid HTTP header name"))?;
    reqwest::header::HeaderValue::from_str(value)
        .map_err(|_| anyhow::anyhow!("invalid header: value contains bytes not allowed in an HTTP header value"))?;

    Ok((name.to_string(), value.to_string()))
}

/// Parses `list --status`'s comma-separated value into `DownloadStatus`es,
/// using the lowercase serde names from `task.rs`. `None` or an empty string
/// means no filter (every status). Shared by the standalone `list` handler
/// and the daemon-routed one so both accept exactly the same spelling.
pub fn parse_status_arg(s: Option<&str>) -> anyhow::Result<Vec<DownloadStatus>> {
    let Some(s) = s else {
        return Ok(Vec::new());
    };
    s.split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(|s| {
            serde_json::from_value(serde_json::Value::String(s.to_ascii_lowercase()))
                .map_err(|_| anyhow::anyhow!("unknown status '{s}'"))
        })
        .collect()
}

/// Turns `move`'s CLI flags into one `MoveOp`. The clap `move_op` group
/// already rejects more than one flag being set; this only needs to check
/// that at least one was, matching `handle_move`'s own check.
pub fn move_op_from_args(
    to_top: bool,
    to_bottom: bool,
    before: Option<&str>,
    folder: Option<&str>,
) -> Result<MoveOp, String> {
    if to_top {
        Ok(MoveOp::ToTop)
    } else if to_bottom {
        Ok(MoveOp::ToBottom)
    } else if let Some(before) = before {
        let id = uuid::Uuid::parse_str(before).map_err(|_| "Invalid before UUID format".to_string())?;
        Ok(MoveOp::Before { id })
    } else if let Some(folder) = folder {
        Ok(MoveOp::Folder { folder: folder.to_string() })
    } else {
        Err("Must specify one of: --to-top, --to-bottom, --before, --folder".to_string())
    }
}

/// Replaces every header value with a placeholder before a task is printed.
/// Header *names* stay visible (useful to confirm a `--header`/`--referrer`
/// landed), but a value can itself be a secret (`Cookie`, `Authorization`, a
/// signed URL) that must never reach stdout via `list`/`status`, daemon-routed
/// or not.
fn redact_task_headers(mut task: DownloadTask) -> DownloadTask {
    for value in task.headers.values_mut() {
        *value = "<redacted>".to_string();
    }
    task
}

/// Renders a `list` result: same filter output, same redaction, whether the
/// tasks came from the daemon's `Tasks` response or straight from the local
/// manager, so `list --json` never differs by mode.
pub fn render_tasks(tasks: Vec<DownloadTask>, json: bool) {
    let tasks: Vec<DownloadTask> = tasks.into_iter().map(redact_task_headers).collect();
    println!("{}", output::format_downloads(&tasks, json));
}

/// Renders a `status`/`get` result. `in_history` only affects the
/// human-readable form (a small trailing note); the JSON form always matches
/// the standalone shape, which never had an `in_history` field.
pub fn render_task(task: DownloadTask, in_history: bool, json: bool) {
    let task = redact_task_headers(task);
    if json {
        match serde_json::to_string_pretty(&task) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to format task: {e}"),
        }
    } else {
        println!("{}", output::format_download(&task, true));
        if in_history {
            println!("(from history)");
        }
    }
}

/// Renders `modify`'s result: the only thing `modify` can change is
/// headers, so this prints those (redacted, like every other task render)
/// instead of the full `format_download` block.
pub fn render_modified(task: DownloadTask) {
    let task = redact_task_headers(task);
    println!("Modified download: {} (ID: {})", task.filename, task.id);
    if task.headers.is_empty() {
        println!("Headers: (none)");
    } else {
        println!("Headers:");
        let mut names: Vec<&String> = task.headers.keys().collect();
        names.sort();
        for name in names {
            println!("  {}: {}", name, task.headers[name]);
        }
    }
}

/// Renders `add`'s result the same way whether it came from the daemon's
/// `Added` response or was assembled directly in standalone mode, and
/// returns the exit code. `quiet` and `json` are mutually exclusive at the
/// clap level. A per-task `start_error` is a warning, not a failure: the
/// task is still queued, so it never changes the exit code. Neither the
/// wire payload nor this renderer carries header values, only names, so a
/// secret passed via `--header` never reaches stdout/stderr here.
pub fn render_added(added: &[AddedTask], skipped: &[SkippedUrl], quiet: bool, json: bool) -> i32 {
    if json {
        #[derive(serde::Serialize)]
        struct AddedPayload<'a> {
            added: &'a [AddedTask],
            skipped: &'a [SkippedUrl],
        }
        match serde_json::to_string_pretty(&AddedPayload { added, skipped }) {
            Ok(s) => println!("{s}"),
            Err(e) => eprintln!("failed to format add result: {e}"),
        }
    } else if quiet {
        for task in added {
            println!("{}", task.id);
        }
    } else {
        for task in added {
            println!("Added download: {} (ID: {})", task.url, task.id);
        }
    }

    for task in added {
        if let Some(err) = &task.start_error {
            eprintln!("Warning: could not start {}: {}", task.url, err);
        }
    }
    for skip in skipped {
        if skip.reason == "duplicate" {
            eprintln!("Skipped duplicate URL: {}", skip.url);
        } else {
            eprintln!("Skipping invalid URL: {}", skip.url);
        }
    }

    if added.is_empty() {
        let all_invalid = !skipped.is_empty() && skipped.iter().all(|s| s.reason == "invalid_url");
        if all_invalid {
            error::INVALID_INPUT
        } else {
            error::ERROR
        }
    } else {
        error::SUCCESS
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
            classify(&Commands::List { json: false, status: None, folder: None }),
            RemoteClass::Routed
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
        assert_eq!(
            classify(&Commands::Add {
                url: "https://example.com".to_string(),
                folder: None,
                referrer: None,
                header: vec![],
                immediate: false,
                quiet: false,
                json: false,
            }),
            RemoteClass::Routed
        );
        assert_eq!(
            classify(&Commands::Start { id: "x".to_string(), wait: false }),
            RemoteClass::Routed
        );
        assert_eq!(
            classify(&Commands::Start { id: "x".to_string(), wait: true }),
            RemoteClass::Routed
        );
        assert_eq!(
            classify(&Commands::Wait { id: "x".to_string(), timeout: None, json: false }),
            RemoteClass::RequiresDaemon
        );
        assert_eq!(
            classify(&Commands::Priority { id: "x".to_string(), set: 5 }),
            RemoteClass::Routed
        );
        assert_eq!(
            classify(&Commands::Move {
                id: "x".to_string(),
                to_top: true,
                to_bottom: false,
                before: None,
                folder: None,
            }),
            RemoteClass::Routed
        );
    }

    #[test]
    fn move_op_from_args_to_top() {
        assert_eq!(move_op_from_args(true, false, None, None), Ok(MoveOp::ToTop));
    }

    #[test]
    fn move_op_from_args_to_bottom() {
        assert_eq!(move_op_from_args(false, true, None, None), Ok(MoveOp::ToBottom));
    }

    #[test]
    fn move_op_from_args_before() {
        let id = uuid::Uuid::new_v4();
        let id_str = id.to_string();
        assert_eq!(
            move_op_from_args(false, false, Some(&id_str), None),
            Ok(MoveOp::Before { id })
        );
    }

    #[test]
    fn move_op_from_args_folder() {
        assert_eq!(
            move_op_from_args(false, false, None, Some("archive")),
            Ok(MoveOp::Folder { folder: "archive".to_string() })
        );
    }

    #[test]
    fn move_op_from_args_none_is_error() {
        assert_eq!(
            move_op_from_args(false, false, None, None),
            Err("Must specify one of: --to-top, --to-bottom, --before, --folder".to_string())
        );
    }

    #[test]
    fn move_op_from_args_bad_before_uuid_is_error() {
        assert_eq!(
            move_op_from_args(false, false, Some("not-a-uuid"), None),
            Err("Invalid before UUID format".to_string())
        );
    }

    #[test]
    fn parse_status_arg_accepts_known_names_and_rejects_unknown() {
        assert_eq!(parse_status_arg(None).unwrap(), Vec::new());
        assert_eq!(parse_status_arg(Some("")).unwrap(), Vec::new());
        assert_eq!(
            parse_status_arg(Some("pending, Error")).unwrap(),
            vec![DownloadStatus::Pending, DownloadStatus::Error]
        );
        assert!(parse_status_arg(Some("bogus")).is_err());
    }

    #[test]
    fn redact_task_headers_hides_values_but_keeps_names() {
        let mut task = DownloadTask::new(
            "https://example.com/f".to_string(),
            std::path::PathBuf::from("f"),
        );
        task.headers.insert("Cookie".to_string(), "sid=SECRET".to_string());
        let redacted = redact_task_headers(task);
        assert_eq!(redacted.headers.get("Cookie").map(String::as_str), Some("<redacted>"));
    }

    #[test]
    fn parse_header_arg_splits_first_colon() {
        assert_eq!(
            parse_header_arg("X-A: b:c").unwrap(),
            ("X-A".to_string(), "b:c".to_string())
        );
    }

    #[test]
    fn parse_header_arg_rejects_empty_name() {
        assert!(parse_header_arg(": value").is_err());
        assert!(parse_header_arg("no-colon").is_err());
    }

    #[test]
    fn parse_header_arg_allows_empty_value() {
        assert_eq!(
            parse_header_arg("X-Empty:").unwrap(),
            ("X-Empty".to_string(), String::new())
        );
    }

    #[test]
    fn parse_header_arg_rejects_crlf_in_value() {
        let err = parse_header_arg("X-Test: bad\r\nX-Injected: 1")
            .expect_err("CR/LF in a header value must be refused");
        // The error must never carry the value back to the caller.
        assert!(!err.to_string().contains("Injected"));
    }

    #[test]
    fn parse_header_arg_rejects_invalid_name_chars() {
        assert!(parse_header_arg("Bad Name: v").is_err());
        assert!(parse_header_arg("Bad\r\nName: v").is_err());
    }

    #[test]
    fn parse_header_arg_error_never_contains_value() {
        let err = parse_header_arg("Cookie: sid=SUPER-SECRET-VALUE")
            .err()
            .map(|e| e.to_string());
        // A well-formed header does not error at all; this only guards the
        // case where validation does fail while a value is present.
        if let Some(message) = err {
            assert!(!message.contains("SUPER-SECRET-VALUE"));
        }
    }
}
