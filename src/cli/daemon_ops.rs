//! Pure dispatch logic for the daemon control protocol.
//!
//! `dispatch` turns a `ControlRequest` into a `ControlResponse` without
//! touching a pipe, so it is unit-testable without a running server.
//! `DaemonContext` is the shared state a pipe connection handler needs; it
//! implements `ControlHandler` (Windows only, since that trait and its
//! `BoxFuture` return types are only used by the Windows pipe server).

use std::sync::Arc;
use std::time::Instant;

#[cfg(windows)]
use futures_util::future::BoxFuture;
use tokio::sync::{Mutex, Notify};

use crate::app::state::AppState;
use crate::download::enqueue::{
    build_task, enqueue, resolve_folder_id, resolve_target_folder, EnqueueOptions, EnqueueOutcome,
};
use crate::download::manager::{DownloadEvent, DownloadManager, StartOutcome, TaskBusy};
use crate::download::task::{DownloadStatus, DownloadTask};
use crate::ipc::control::protocol::{
    AddedTask, ControlErrorCode, ControlRequest, ControlResponse, DaemonStatus, HeaderEdit,
    MoveOp, SkippedUrl, WaitOutcome, CONTROL_PROTOCOL_VERSION,
};
use tokio::sync::broadcast;
use uuid::Uuid;

/// How often `dispatch_wait`'s poll loop re-checks state, covering a `Lagged`
/// broadcast receiver and any event that arrived before the subscription.
const WAIT_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_secs(1);

/// The daemon's control pipe binding, threaded from `main.rs`'s single-
/// instance `bind` into `run_daemon`. `BoundPipe` on Windows; an empty enum
/// elsewhere, so `Option<ControlBinding>` is always `None` on non-Windows
/// targets and no `#[cfg]` is needed at `run_daemon`'s call sites.
#[cfg(windows)]
pub type ControlBinding = crate::ipc::control::server::BoundPipe;
#[cfg(not(windows))]
pub enum ControlBinding {}

/// Shared state for every control-pipe connection. Cheap to clone/share via
/// `Arc`: `AppState`, `DownloadManager`, and `Notify` are all `Arc`-backed.
pub struct DaemonContext {
    pub state: AppState,
    pub manager: DownloadManager,
    pub shutdown: Arc<Notify>,
    pub started_at: Instant,
    pub pipe_name: String,
    /// Orders the daemon's own `save_queue_to_folders()` call sites relative
    /// to each other: one from each mutating connection's `after_response`
    /// (these run on detached per-connection tasks that outlive
    /// `server_handle.abort()`), one from the periodic auto-save, and the
    /// final save on shutdown. Per-file write safety (unique temp names,
    /// serializing concurrent writers of the same folder) now lives in
    /// `FolderQueue::save` itself, so this lock only needs to keep the
    /// daemon's saves in a sane order, e.g. the final save running after
    /// the others.
    pub save_lock: Arc<Mutex<()>>,
}

/// Answers one request against the current daemon state. The match below is
/// exhaustive (no `_ =>` arm), so a new `ControlRequest` variant fails to
/// compile here until it is given a handler.
///
/// Contract: a handler for a mutating request (`ControlRequest::is_mutating`)
/// returns `ControlResponse::Error` only on paths that changed no state.
/// `after_response` relies on this to skip the queue save on an error reply
/// (see `should_save`); returning `Error` after a real mutation would leave
/// that change unsaved. `dispatch_config_set` mutates a clone of the config
/// and only assigns it back to the shared lock after `Config::save()`
/// succeeds, so the in-memory config and the file on disk never diverge:
/// a failed save (`Error{Internal}`) leaves both at the pre-set value.
pub async fn dispatch(req: ControlRequest, ctx: &DaemonContext) -> ControlResponse {
    match req {
        ControlRequest::Ping => ControlResponse::Pong {
            protocol: CONTROL_PROTOCOL_VERSION,
            pid: std::process::id(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        },
        ControlRequest::Status => {
            let tasks = ctx.manager.get_all_downloads().await;
            let mut pending = 0;
            let mut downloading = 0;
            let mut paused = 0;
            let mut error = 0;
            for task in &tasks {
                match task.status {
                    DownloadStatus::Pending => pending += 1,
                    DownloadStatus::Downloading => downloading += 1,
                    DownloadStatus::Paused => paused += 1,
                    DownloadStatus::Error => error += 1,
                    // Completed tasks move to history; Deleted never appears
                    // in a folder queue. Neither is counted here.
                    DownloadStatus::Completed | DownloadStatus::Deleted => {}
                }
            }
            let history = ctx.manager.history_len().await;
            let config_dir = crate::util::paths::find_config_directory()
                .map(|dir| dir.to_string_lossy().to_string())
                .unwrap_or_default();

            ControlResponse::Status {
                status: DaemonStatus {
                    pid: std::process::id(),
                    version: env!("CARGO_PKG_VERSION").to_string(),
                    protocol: CONTROL_PROTOCOL_VERSION,
                    uptime_secs: ctx.started_at.elapsed().as_secs(),
                    pipe_name: ctx.pipe_name.clone(),
                    config_dir,
                    scripts_active: ctx.state.script_sender.is_some(),
                    pending,
                    downloading,
                    paused,
                    error,
                    history,
                },
            }
        }
        // The actual shutdown happens in `after_response`, once the client
        // already has this reply.
        ControlRequest::Shutdown => ControlResponse::Ok {
            message: "shutting down".to_string(),
        },
        ControlRequest::Add {
            url,
            folder,
            referrer,
            headers,
            immediate,
        } => dispatch_add(url, folder, referrer, headers, immediate, ctx).await,
        ControlRequest::List { statuses, folder } => dispatch_list(statuses, folder, ctx).await,
        ControlRequest::Get { id } => dispatch_get(id, ctx).await,
        ControlRequest::Start { id } => dispatch_start(id, ctx).await,
        ControlRequest::Pause { id } => dispatch_pause(id, ctx).await,
        ControlRequest::Remove { id } => dispatch_remove(id, ctx).await,
        ControlRequest::StartAll { folder } => dispatch_start_all(folder, ctx).await,
        ControlRequest::PauseAll { folder } => dispatch_pause_all(folder, ctx).await,
        ControlRequest::Wait { id, timeout_secs } => dispatch_wait(id, timeout_secs, ctx).await,
        ControlRequest::ReloadScripts => dispatch_reload_scripts(ctx).await,
        ControlRequest::SetScriptEnabled { name, enabled } => {
            dispatch_set_script_enabled(name, enabled, ctx).await
        }
        ControlRequest::Modify { id, edit } => dispatch_modify(id, edit, ctx).await,
        ControlRequest::Move { id, op } => dispatch_move(id, op, ctx).await,
        ControlRequest::Priority { id, priority } => dispatch_priority(id, priority, ctx).await,
        ControlRequest::ConfigGet { key } => dispatch_config_get(key, ctx).await,
        ControlRequest::ConfigSet { key, value } => dispatch_config_set(key, value, ctx).await,
    }
}

/// Reloads scripts from disk on the executor thread.
///
/// The executor is a single OS thread draining one channel in FIFO order, and
/// `Reload` is just another message on that channel: any `BeforeRequest` (or
/// other hook) request already queued ahead of it runs to completion, against
/// the pre-reload handler set, before `Reload` is even picked up. Anything
/// queued after `Reload` sees the reloaded handlers. Nothing in flight is
/// ever dropped or answered twice, and no lock or generation counter is
/// needed for this ordering guarantee. `reload_scripts()` waits on that
/// channel without its own timeout, so a slow script queued ahead of the
/// reload can make this call outlast the client's request timeout even
/// though the daemon eventually still reloads.
async fn dispatch_reload_scripts(ctx: &DaemonContext) -> ControlResponse {
    if ctx.state.script_sender.is_none() {
        return ControlResponse::Error {
            code: ControlErrorCode::Unsupported,
            message: "scripts are not enabled".to_string(),
        };
    }
    match ctx.state.reload_scripts().await {
        Ok(()) => ControlResponse::Ok {
            message: "Scripts reloaded".to_string(),
        },
        // A bad script (or a bad script directory) must not take the daemon
        // down: it stays up, and the client gets a clear, non-zero-exit error
        // instead. Scripts that loaded fine before this reload (or earlier in
        // the same reload) keep running, so the message says "with errors"
        // rather than implying nothing is active.
        Err(e) => ControlResponse::Error {
            code: ControlErrorCode::Internal,
            message: format!("scripts reloaded with errors; {e} (scripts that loaded successfully remain active)"),
        },
    }
}

/// Mirrors the standalone `script enable`/`disable` handling in
/// `handler.rs`: verify the script file exists, flip its entry in
/// `scripts.script_files`, then save. Holds the config write lock across both
/// steps so a concurrent config write (there is currently no other one in the
/// daemon, but a future one would share this lock) can't interleave with the
/// read-modify-save and lose this change.
async fn dispatch_set_script_enabled(name: String, enabled: bool, ctx: &DaemonContext) -> ControlResponse {
    let mut config = ctx.state.config.write().await;

    let script_path = config.scripts.directory.join(&name);
    if !script_path.exists() {
        return not_found(&format!("script not found: {name}"));
    }

    config.scripts.script_files.insert(name.clone(), enabled);
    if let Err(e) = config.save() {
        return ControlResponse::Error {
            code: ControlErrorCode::Internal,
            message: format!("failed to save config: {e}"),
        };
    }

    let verb = if enabled { "Enabled" } else { "Disabled" };
    ControlResponse::Ok {
        message: format!("{verb} script: {name}"),
    }
}

/// Applies a `Modify` edit to a task's headers, in the same order `add`
/// applies its own header/referrer overrides (`enqueue::build_task`):
/// `set_headers` first, so `--referrer` wins over a `Referer` passed
/// through `--header`. `remove_headers` runs last of all, after
/// `clear_referrer`/`referrer`, so an explicit `--remove-header` always
/// wins, even over a name this same edit just set. Shared by the daemon's
/// `Modify` handler and the standalone `modify` command so both mutate a
/// task the same way.
pub(crate) fn apply_header_edit(task: &mut DownloadTask, edit: &HeaderEdit) {
    for (name, value) in &edit.set_headers {
        task.set_header(name, value);
    }
    if edit.clear_referrer {
        task.remove_header("Referer");
    }
    if let Some(referrer) = &edit.referrer {
        task.set_header("Referer", referrer);
    }
    for name in &edit.remove_headers {
        task.remove_header(name);
    }
}

async fn dispatch_modify(id: Uuid, edit: HeaderEdit, ctx: &DaemonContext) -> ControlResponse {
    match ctx
        .manager
        .edit_task_headers(id, move |task| apply_header_edit(task, &edit))
        .await
    {
        Ok(task) => ControlResponse::Task { task, in_history: false },
        Err(e) => modify_error_response(e),
    }
}

/// Maps `edit_task_headers`'s typed error to a `ControlResponse`: `NotFound`
/// stays `NotFound` (exit 2), `Busy` maps to `InvalidInput` (exit 3, "pause
/// the download first", per the Command surface).
fn modify_error_response(e: crate::download::manager::EditHeadersError) -> ControlResponse {
    use crate::download::manager::EditHeadersError;
    let code = match &e {
        EditHeadersError::NotFound => ControlErrorCode::NotFound,
        EditHeadersError::Busy(..) => ControlErrorCode::InvalidInput,
    };
    ControlResponse::Error { code, message: e.to_string() }
}

/// Mirrors the standalone `move` command (`handle_move` in `handler.rs`):
/// same messages, same folder resolution, and the same `TaskBusy`/not-found
/// mapping for `change_folder`. Unlike `handle_move`, `to_top`/`to_bottom`
/// check the id up front, because `move_to_top`/`move_to_bottom` do not
/// distinguish "not found" from other failures the way `change_folder` does.
async fn dispatch_move(id: Uuid, op: MoveOp, ctx: &DaemonContext) -> ControlResponse {
    if ctx.manager.get_by_id(id).await.is_none() {
        return not_found("Download not found");
    }
    match op {
        MoveOp::ToTop => match ctx.manager.move_to_top(id).await {
            Ok(()) => ControlResponse::Ok {
                message: format!("Moved download {id} to top of queue"),
            },
            Err(e) => not_found(&e.to_string()),
        },
        MoveOp::ToBottom => match ctx.manager.move_to_bottom(id).await {
            Ok(()) => ControlResponse::Ok {
                message: format!("Moved download {id} to bottom of queue"),
            },
            Err(e) => not_found(&e.to_string()),
        },
        MoveOp::Before { id: before_id } => {
            if ctx.manager.get_by_id(before_id).await.is_none() {
                return not_found("Target download not found");
            }
            match ctx.manager.move_before(id, before_id).await {
                Ok(()) => ControlResponse::Ok {
                    message: format!("Moved download {id} before {before_id}"),
                },
                Err(e) => ControlResponse::Error {
                    code: ControlErrorCode::InvalidInput,
                    message: e.to_string(),
                },
            }
        }
        MoveOp::Folder { folder } => {
            // Accept a display name as well as the UUID key, as `add` does.
            // The read guard is dropped before `change_folder`, which takes
            // the config lock itself.
            let folder_id = {
                let config = ctx.state.config.read().await.clone();
                match resolve_folder_id(&config, &folder) {
                    Some(f) => f,
                    None => {
                        return ControlResponse::Error {
                            code: ControlErrorCode::InvalidInput,
                            message: format!("Folder '{folder}' not found"),
                        }
                    }
                }
            };
            match ctx
                .manager
                .change_folder(id, folder_id.clone(), Some(&ctx.state.config))
                .await
            {
                Ok(()) => ControlResponse::Ok {
                    message: format!("Moved download {id} to folder '{folder_id}'"),
                },
                Err(e) => {
                    if let Some(busy) = e.downcast_ref::<TaskBusy>() {
                        task_busy_response(TaskBusy(busy.0))
                    } else if e.to_string() == "Task not found" {
                        not_found(&e.to_string())
                    } else {
                        ControlResponse::Error {
                            code: ControlErrorCode::InvalidInput,
                            message: e.to_string(),
                        }
                    }
                }
            }
        }
    }
}

/// Mirrors the standalone `priority` command's message. `set_priority`'s
/// only error is "not found", so any `Err` maps to `NotFound` without
/// touching state.
async fn dispatch_priority(id: Uuid, priority: u8, ctx: &DaemonContext) -> ControlResponse {
    match ctx.manager.set_priority(id, priority).await {
        Ok(()) => ControlResponse::Ok {
            message: format!("Set priority to {priority} for download {id}"),
        },
        Err(e) => not_found(&e.to_string()),
    }
}

/// Mirrors the standalone `config get` command (`handle_config` in
/// `handler.rs`), sharing its key parsing via `get_config_value`.
async fn dispatch_config_get(key: String, ctx: &DaemonContext) -> ControlResponse {
    let config = ctx.state.config.read().await;
    match super::handler::get_config_value(&config, &key) {
        Ok(value) => ControlResponse::Value { value },
        Err(e) => ControlResponse::Error {
            code: ControlErrorCode::InvalidInput,
            message: e.to_string(),
        },
    }
}

/// Mirrors the standalone `config set` command, but validates and saves a
/// clone before touching the shared config, so a bad key/value or a failed
/// save leaves the running daemon's in-memory config exactly as it was.
/// Takes the write lock for the whole operation, like
/// `dispatch_set_script_enabled`, so a concurrent `ConfigSet` can't
/// interleave with this one's clone/validate/save sequence.
async fn dispatch_config_set(key: String, value: String, ctx: &DaemonContext) -> ControlResponse {
    let mut guard = ctx.state.config.write().await;
    let mut clone = guard.clone();

    if let Err(e) = super::handler::set_config_value(&mut clone, &key, &value) {
        return ControlResponse::Error {
            code: ControlErrorCode::InvalidInput,
            message: e.to_string(),
        };
    }
    if let Err(errors) = crate::app::settings::validate_folder_config(&clone) {
        return ControlResponse::Error {
            code: ControlErrorCode::InvalidInput,
            message: errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", "),
        };
    }
    if let Err(e) = clone.save() {
        return ControlResponse::Error {
            code: ControlErrorCode::Internal,
            message: format!("failed to save config: {e}"),
        };
    }

    *guard = clone;
    ControlResponse::Ok {
        message: format!(
            "Configuration updated: {key} = {value} (the running daemon may apply this only after a restart)"
        ),
    }
}

fn not_found(message: &str) -> ControlResponse {
    ControlResponse::Error {
        code: ControlErrorCode::NotFound,
        message: message.to_string(),
    }
}

/// `TaskBusy` maps to `InvalidInput`: the id and the operation are both
/// fine, another operation just holds the claim right now.
fn task_busy_response(e: TaskBusy) -> ControlResponse {
    ControlResponse::Error {
        code: ControlErrorCode::InvalidInput,
        message: e.to_string(),
    }
}

fn unknown_folder() -> ControlResponse {
    ControlResponse::Error {
        code: ControlErrorCode::InvalidInput,
        message: "unknown folder".to_string(),
    }
}

/// Resolves a `list`/`start-all`/`pause-all` `folder` argument (a UUID key or
/// a display name, the same as `add`) to a folder key. `None` means "every
/// folder" and is returned as-is without touching the config.
async fn resolve_optional_folder(
    folder: &Option<String>,
    ctx: &DaemonContext,
) -> Result<Option<String>, ControlResponse> {
    match folder {
        Some(f) => {
            let config = ctx.state.config.read().await.clone();
            resolve_folder_id(&config, f).map(Some).ok_or_else(unknown_folder)
        }
        None => Ok(None),
    }
}

/// Applies `list`'s optional status/folder filters. `folder_id` must already
/// be a resolved folder key, not a display name. Preserves the manager's
/// task order.
pub(crate) fn filter_tasks(
    tasks: Vec<DownloadTask>,
    statuses: &[DownloadStatus],
    folder_id: Option<&str>,
) -> Vec<DownloadTask> {
    tasks
        .into_iter()
        .filter(|t| statuses.is_empty() || statuses.contains(&t.status))
        .filter(|t| match folder_id {
            Some(f) => t.folder_id == f,
            None => true,
        })
        .collect()
}

async fn dispatch_list(
    statuses: Vec<DownloadStatus>,
    folder: Option<String>,
    ctx: &DaemonContext,
) -> ControlResponse {
    let folder_id = match resolve_optional_folder(&folder, ctx).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    let tasks = ctx.manager.get_all_downloads().await;
    let tasks = filter_tasks(tasks, &statuses, folder_id.as_deref());
    ControlResponse::Tasks { tasks }
}

/// Looks a task up in the live queue first, then in history, so a routed
/// `status`/`get` can also report a completed or failed task. Standalone
/// `status` cannot do this: it never consults `manager`'s in-memory history.
async fn dispatch_get(id: Uuid, ctx: &DaemonContext) -> ControlResponse {
    if let Some(task) = ctx.manager.get_by_id(id).await {
        return ControlResponse::Task { task, in_history: false };
    }
    if let Some(task) = ctx.manager.get_history_item(id).await {
        return ControlResponse::Task { task, in_history: true };
    }
    not_found("Download not found")
}

/// Starts a single task and reports `start_download`'s `StartOutcome`:
/// started, already running (including retry backoff), or busy (another
/// operation holds the claim).
async fn dispatch_start(id: Uuid, ctx: &DaemonContext) -> ControlResponse {
    let Some(task) = ctx.manager.get_by_id(id).await else {
        return not_found("Download not found");
    };
    match ctx
        .manager
        .start_download(id, ctx.state.script_sender.clone(), ctx.state.config.clone())
        .await
    {
        Ok(StartOutcome::Started) => ControlResponse::Ok {
            message: format!("Started download: {}", task.filename),
        },
        Ok(StartOutcome::AlreadyRunning) => ControlResponse::Ok {
            message: format!("Download already running: {}", task.filename),
        },
        Ok(StartOutcome::Busy) => ControlResponse::Ok {
            message: format!("Download is busy (being started or modified): {}", task.filename),
        },
        Err(e) => ControlResponse::Error {
            code: ControlErrorCode::Internal,
            message: e.to_string(),
        },
    }
}

async fn dispatch_pause(id: Uuid, ctx: &DaemonContext) -> ControlResponse {
    let Some(task) = ctx.manager.get_by_id(id).await else {
        return not_found("Download not found");
    };
    match ctx.manager.pause_download(id).await {
        Ok(()) => ControlResponse::Ok {
            message: format!("Paused download: {}", task.filename),
        },
        Err(e) => task_busy_response(e),
    }
}

async fn dispatch_remove(id: Uuid, ctx: &DaemonContext) -> ControlResponse {
    match ctx.manager.remove_download(id).await {
        Ok(Some(task)) => ControlResponse::Ok {
            message: format!("Removed download: {}", task.filename),
        },
        Ok(None) => not_found("Download not found"),
        Err(e) => task_busy_response(e),
    }
}

/// Starts every `Pending`/`Paused` task in scope (all folders, or one
/// resolved folder). Does not use
/// `DownloadManager::start_folder_tasks`/`start_all_tasks`: those only
/// attempt `Pending` tasks, while daemon `start-all` also attempts `Paused`
/// ones, matching the standalone `start --all` command's status filter.
async fn start_scope(ctx: &DaemonContext, folder_id: Option<&str>) -> usize {
    let tasks = ctx.manager.get_all_downloads().await;
    let mut started = 0;
    for task in tasks {
        if let Some(f) = folder_id {
            if task.folder_id != f {
                continue;
            }
        }
        if !matches!(task.status, DownloadStatus::Pending | DownloadStatus::Paused) {
            continue;
        }
        match ctx
            .manager
            .start_download(task.id, ctx.state.script_sender.clone(), ctx.state.config.clone())
            .await
        {
            Ok(StartOutcome::Started) => started += 1,
            Ok(_) => {}
            Err(e) => tracing::warn!("Failed to start {}: {}", task.filename, e),
        }
    }
    started
}

async fn dispatch_start_all(folder: Option<String>, ctx: &DaemonContext) -> ControlResponse {
    let folder_id = match resolve_optional_folder(&folder, ctx).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    let started = start_scope(ctx, folder_id.as_deref()).await;
    ControlResponse::Ok {
        message: format!("Started {started} download(s)"),
    }
}

async fn dispatch_pause_all(folder: Option<String>, ctx: &DaemonContext) -> ControlResponse {
    let folder_id = match resolve_optional_folder(&folder, ctx).await {
        Ok(f) => f,
        Err(resp) => return resp,
    };
    let paused = match folder_id {
        Some(id) => ctx.manager.stop_folder_tasks(&id).await,
        None => ctx.manager.pause_all().await,
    };
    ControlResponse::Ok {
        message: format!("Paused {paused} download(s)"),
    }
}

/// Checks whether `id` has already reached a final state: a history entry
/// always means `Completed`, even if the id also somehow lingers in a queue;
/// missing from both means `Removed`; otherwise the live task's status
/// decides. `None` means "keep waiting" -- a `Pending`/`Downloading` task, a
/// `Paused` task with a handle still alive (retry backoff, not a real
/// pause), or a task whose start is still in progress. Shared by the
/// daemon's `Wait` dispatch and standalone `start --wait`, so both agree on
/// what counts as terminal.
pub(crate) async fn wait_state(
    id: Uuid,
    manager: &DownloadManager,
) -> Option<(WaitOutcome, Option<DownloadTask>)> {
    if let Some(task) = manager.get_history_item(id).await {
        return Some((WaitOutcome::Completed, Some(task)));
    }
    let Some(task) = manager.get_by_id(id).await else {
        // A completing download is removed from its folder queue, then
        // awaits `queue.save()` (disk IO), and only then is added to history
        // (`manager.rs`), so a queue miss right after a history miss can
        // still be that gap, not an actual removal. Checking history again
        // here narrows the window; `is_running` closes it: the handle stays
        // in `active_downloads` until that same completion path removes it,
        // so a live handle means the task is still finishing, never really
        // removed. A real `remove_download` drops the handle before removing
        // the task from the queue, so this check is never fooled by an
        // actual removal.
        if let Some(task) = manager.get_history_item(id).await {
            return Some((WaitOutcome::Completed, Some(task)));
        }
        if manager.is_running(id).await {
            return None;
        }
        return Some((WaitOutcome::Removed, None));
    };
    match task.status {
        DownloadStatus::Error => Some((WaitOutcome::Failed, Some(task))),
        // A start claimed after the task was paused (a sweep, or another
        // client) is about to make it Downloading again; report it only
        // once nothing -- a live handle or an in-flight start -- can still
        // flip it back.
        DownloadStatus::Paused
            if !manager.is_running(id).await && !manager.is_start_in_progress(id) =>
        {
            Some((WaitOutcome::Paused, Some(task)))
        }
        _ => None,
    }
}

/// Wraps `wait_state` for the daemon's own `Wait` dispatch.
async fn check_wait_state(id: Uuid, manager: &DownloadManager) -> Option<ControlResponse> {
    wait_state(id, manager)
        .await
        .map(|(outcome, task)| ControlResponse::Waited { outcome, task })
}

/// Answers `Wait`. Subscribes before the first state check so a result that
/// lands in the gap between subscribing and checking is never missed (it
/// still shows up as a broadcast event that re-triggers the check). The
/// per-connection task this runs on is what makes a long wait safe: the
/// server spawns one task per pipe connection (`server.rs`'s accept loop), so
/// this holds only its own connection open and never blocks another client.
/// `timeout_secs = None` waits until the task reaches a final state or the
/// connection itself is torn down (for example by a daemon shutdown aborting
/// this task, which just closes the pipe -- the client sees that as a send
/// error, not a hang). A client that disconnects while this is pending (e.g.
/// Ctrl+C) also ends the wait: `server.rs` races this future against the
/// connection's EOF and drops it without writing a reply.
async fn dispatch_wait(id: Uuid, timeout_secs: Option<u64>, ctx: &DaemonContext) -> ControlResponse {
    let mut rx = ctx.manager.subscribe();
    if let Some(resp) = check_wait_state(id, &ctx.manager).await {
        return resp;
    }

    let wait_loop = wait_for_final_state(id, &ctx.manager, &mut rx);
    match timeout_secs {
        Some(secs) => {
            match tokio::time::timeout(std::time::Duration::from_secs(secs), wait_loop).await {
                Ok(resp) => resp,
                Err(_) => ControlResponse::Waited {
                    outcome: WaitOutcome::Timeout,
                    task: None,
                },
            }
        }
        None => wait_loop.await,
    }
}

/// The unbounded part of `dispatch_wait`: re-checks state on every event that
/// mentions `id`, on `Lagged` (a missed event might have been the one that
/// mattered), and also on a fixed 1 s interval, which catches any event this
/// receiver never saw at all. The caller applies the optional deadline
/// around this future.
async fn wait_for_final_state(
    id: Uuid,
    manager: &DownloadManager,
    rx: &mut broadcast::Receiver<DownloadEvent>,
) -> ControlResponse {
    let mut interval = tokio::time::interval(WAIT_POLL_INTERVAL);
    interval.tick().await; // first tick fires immediately; skip it
    // Once the channel closes, `rx.recv()` resolves immediately forever;
    // the `if !rx_closed` guard turns that branch off so the loop falls back
    // to the 1 s interval instead of spinning.
    let mut rx_closed = false;
    loop {
        tokio::select! {
            event = rx.recv(), if !rx_closed => {
                let relevant = match &event {
                    Ok(DownloadEvent::Completed { task_id })
                    | Ok(DownloadEvent::Failed { task_id, .. })
                    | Ok(DownloadEvent::Paused { task_id })
                    | Ok(DownloadEvent::Removed { task_id }) => *task_id == id,
                    Ok(_) => false,
                    Err(broadcast::error::RecvError::Lagged(_)) => true,
                    Err(broadcast::error::RecvError::Closed) => {
                        rx_closed = true;
                        true
                    }
                };
                if relevant {
                    if let Some(resp) = check_wait_state(id, manager).await {
                        return resp;
                    }
                }
            }
            _ = interval.tick() => {
                if let Some(resp) = check_wait_state(id, manager).await {
                    return resp;
                }
            }
        }
    }
}

/// Expands `url`'s range patterns and queues each one, exactly as the
/// standalone `add` handler does, so folder rules (`save_path`,
/// `default_headers`, auto-start) apply the same way through the daemon.
/// No extra lock: a concurrent auto-start sweep is made safe by the start
/// claim (`DownloadManager::start_download`).
async fn dispatch_add(
    url: String,
    folder: Option<String>,
    referrer: Option<String>,
    headers: Vec<(String, String)>,
    immediate: bool,
    ctx: &DaemonContext,
) -> ControlResponse {
    let config = ctx.state.config.read().await.clone();
    let folder_id = match resolve_target_folder(&config, folder.as_deref()) {
        Ok(id) => id,
        Err(_) => {
            return ControlResponse::Error {
                code: ControlErrorCode::InvalidInput,
                message: "unknown folder".to_string(),
            };
        }
    };

    let opts = EnqueueOptions { referrer, headers };
    let mut added = Vec::new();
    let mut skipped = Vec::new();

    for u in crate::util::url_expansion::expand_url(&url) {
        if !crate::cli::handler::is_valid_download_url(&u) {
            skipped.push(SkippedUrl {
                url: u,
                reason: "invalid_url".to_string(),
            });
            continue;
        }

        let task = build_task(&u, &folder_id, &config, &opts);
        match enqueue(&ctx.manager, &ctx.state, task, immediate, true).await {
            EnqueueOutcome::Added {
                id,
                started,
                start_error,
            } => added.push(AddedTask {
                id,
                url: u,
                folder_id: folder_id.clone(),
                started,
                start_error,
            }),
            EnqueueOutcome::Duplicate => skipped.push(SkippedUrl {
                url: u,
                reason: "duplicate".to_string(),
            }),
        }
    }

    ControlResponse::Added { added, skipped }
}

/// Error replies mean the handler changed nothing, so there is nothing to
/// save (see `dispatch`'s contract doc comment).
pub(crate) fn should_save(req: &ControlRequest, resp: &ControlResponse) -> bool {
    req.is_mutating() && !matches!(resp, ControlResponse::Error { .. })
}

#[cfg(windows)]
impl crate::ipc::control::server::ControlHandler for DaemonContext {
    fn handle(&self, req: ControlRequest) -> BoxFuture<'_, ControlResponse> {
        Box::pin(dispatch(req, self))
    }

    fn after_response(&self, req: &ControlRequest, resp: &ControlResponse) -> BoxFuture<'_, ()> {
        let needs_save = should_save(req, resp);
        let is_shutdown = matches!(req, ControlRequest::Shutdown);
        Box::pin(async move {
            if needs_save {
                // Held across the save so it can't overlap the auto-save or
                // the final shutdown save; see `save_lock`'s doc comment.
                let _guard = self.save_lock.lock().await;
                if let Err(e) = self.manager.save_queue_to_folders().await {
                    tracing::error!("Failed to save queue after control request: {}", e);
                }
            }
            if is_shutdown {
                self.shutdown.notify_one();
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::Config;
    use crate::download::task::DownloadTask;

    fn test_context(manager: DownloadManager) -> DaemonContext {
        DaemonContext {
            state: AppState::new(Config::default()),
            manager,
            shutdown: Arc::new(Notify::new()),
            started_at: Instant::now(),
            pipe_name: r"\\.\pipe\ggg-daemon-test".to_string(),
            save_lock: Arc::new(Mutex::new(())),
        }
    }

    #[tokio::test]
    async fn dispatch_ping_returns_protocol_version() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(ControlRequest::Ping, &ctx).await;
        match resp {
            ControlResponse::Pong { protocol, pid, version } => {
                assert_eq!(protocol, CONTROL_PROTOCOL_VERSION);
                assert_eq!(pid, std::process::id());
                assert_eq!(version, env!("CARGO_PKG_VERSION"));
            }
            other => panic!("expected Pong, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_status_counts_tasks() {
        let manager = DownloadManager::new();
        let task1 = DownloadTask::new(
            "https://example.com/a".to_string(),
            std::path::PathBuf::from("a"),
        );
        let task2 = DownloadTask::new(
            "https://example.com/b".to_string(),
            std::path::PathBuf::from("b"),
        );
        manager.add_download(task1).await;
        manager.add_download(task2).await;

        let ctx = test_context(manager);
        let resp = dispatch(ControlRequest::Status, &ctx).await;
        match resp {
            ControlResponse::Status { status } => {
                assert_eq!(status.pending, 2);
                assert_eq!(status.downloading, 0);
                assert_eq!(status.paused, 0);
                assert_eq!(status.error, 0);
                assert_eq!(status.history, 0);
                assert_eq!(status.pipe_name, ctx.pipe_name);
            }
            other => panic!("expected Status, got {other:?}"),
        }
    }

    // Proves `daemon stop` reaches the same shutdown signal `run_daemon`
    // selects on: `notify_one` stores a permit even before anyone is
    // waiting, so `notified()` called afterward resolves immediately.
    #[cfg(windows)]
    #[tokio::test]
    async fn after_response_notifies_shutdown() {
        use crate::ipc::control::server::ControlHandler;
        let ctx = test_context(DownloadManager::new());
        let resp = ControlResponse::Ok { message: "shutting down".to_string() };
        ctx.after_response(&ControlRequest::Shutdown, &resp).await;
        let notified = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            ctx.shutdown.notified(),
        )
        .await;
        assert!(notified.is_ok(), "shutdown was not notified");
    }

    #[test]
    fn should_save_mutating_ok_reply() {
        let req = ControlRequest::Pause { id: Uuid::nil() };
        let resp = ControlResponse::Ok { message: String::new() };
        assert!(should_save(&req, &resp));
    }

    #[test]
    fn should_save_skips_error_reply() {
        let req = ControlRequest::Pause { id: Uuid::nil() };
        let resp = ControlResponse::Error {
            code: ControlErrorCode::NotFound,
            message: String::new(),
        };
        assert!(!should_save(&req, &resp));
    }

    #[test]
    fn should_save_skips_non_mutating() {
        let req = ControlRequest::Ping;
        let resp = ControlResponse::Ok { message: String::new() };
        assert!(!should_save(&req, &resp));
    }

    // `AppState::new` (via `test_context`) never starts the script executor
    // thread, so `script_sender` is `None` here.
    #[tokio::test]
    async fn dispatch_reload_without_scripts_is_unsupported() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(ControlRequest::ReloadScripts, &ctx).await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::Unsupported),
            other => panic!("expected Error{{Unsupported}}, got {other:?}"),
        }
    }

    // Verifies the `NotFound` branch without touching the filesystem: no
    // `.save()` is reached because the missing-script check runs first.
    #[tokio::test]
    async fn dispatch_set_script_enabled_missing_script_is_not_found() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(
            ControlRequest::SetScriptEnabled {
                name: "does-not-exist.js".to_string(),
                enabled: true,
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::NotFound),
            other => panic!("expected Error{{NotFound}}, got {other:?}"),
        }
    }

    // `Config::default()`'s "default" folder has `auto_start_downloads =
    // false`, so `immediate: false` here never spawns a real download.
    #[tokio::test]
    async fn dispatch_add_returns_ids_and_applies_referrer() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(
            ControlRequest::Add {
                url: "https://example.com/f.zip".to_string(),
                folder: None,
                referrer: Some("https://example.org/".to_string()),
                headers: vec![("X-Test".to_string(), "1".to_string())],
                immediate: false,
            },
            &ctx,
        )
        .await;

        let added = match resp {
            ControlResponse::Added { added, skipped } => {
                assert!(skipped.is_empty());
                added
            }
            other => panic!("expected Added, got {other:?}"),
        };
        assert_eq!(added.len(), 1);
        assert!(!added[0].started);

        let task = ctx
            .manager
            .get_by_id(added[0].id)
            .await
            .expect("task queued");
        assert_eq!(
            task.headers.get("Referer").map(String::as_str),
            Some("https://example.org/")
        );
        assert_eq!(task.headers.get("X-Test").map(String::as_str), Some("1"));
    }

    #[tokio::test]
    async fn dispatch_add_unknown_folder_is_invalid_input() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(
            ControlRequest::Add {
                url: "https://example.com/f.zip".to_string(),
                folder: Some("no-such-folder".to_string()),
                referrer: None,
                headers: vec![],
                immediate: false,
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::InvalidInput),
            other => panic!("expected Error{{InvalidInput}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_add_skips_invalid_urls() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(
            ControlRequest::Add {
                url: "not a url".to_string(),
                folder: None,
                referrer: None,
                headers: vec![],
                immediate: false,
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Added { added, skipped } => {
                assert!(added.is_empty());
                assert_eq!(skipped.len(), 1);
                assert_eq!(skipped[0].reason, "invalid_url");
            }
            other => panic!("expected Added, got {other:?}"),
        }
    }

    // Proves `DaemonContext` satisfies `ControlHandler: Send + Sync + 'static`,
    // which only holds because `AppState` is `Send + Sync` (its fluent
    // bundles use the concurrent `IntlLangMemoizer`).
    #[cfg(windows)]
    #[test]
    fn daemon_context_is_control_handler() {
        let ctx = test_context(DownloadManager::new());
        let _handler: Arc<dyn crate::ipc::control::server::ControlHandler> = Arc::new(ctx);
    }

    fn task_with_status(status: DownloadStatus) -> DownloadTask {
        let mut task = DownloadTask::new(
            "https://example.com/a".to_string(),
            std::path::PathBuf::from("a"),
        );
        task.status = status;
        task
    }

    #[tokio::test]
    async fn dispatch_list_filters_by_status() {
        let manager = DownloadManager::new();
        manager.add_download(task_with_status(DownloadStatus::Pending)).await;
        manager.add_download(task_with_status(DownloadStatus::Error)).await;
        let ctx = test_context(manager);

        let resp = dispatch(
            ControlRequest::List {
                statuses: vec![DownloadStatus::Pending],
                folder: None,
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Tasks { tasks } => {
                assert_eq!(tasks.len(), 1);
                assert_eq!(tasks[0].status, DownloadStatus::Pending);
            }
            other => panic!("expected Tasks, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_get_finds_history_item() {
        let manager = DownloadManager::new();
        let task = DownloadTask::new(
            "https://example.com/done".to_string(),
            std::path::PathBuf::from("done"),
        );
        let id = task.id;
        manager.add_to_history(task).await;
        let ctx = test_context(manager);

        let resp = dispatch(ControlRequest::Get { id }, &ctx).await;
        match resp {
            ControlResponse::Task { task, in_history } => {
                assert!(in_history);
                assert_eq!(task.id, id);
            }
            other => panic!("expected Task, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_remove_not_found_code() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(ControlRequest::Remove { id: Uuid::new_v4() }, &ctx).await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::NotFound),
            other => panic!("expected Error{{NotFound}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_pause_pending_sets_paused() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Pending);
        let id = task.id;
        manager.add_download(task).await;
        let ctx = test_context(manager);

        let resp = dispatch(ControlRequest::Pause { id }, &ctx).await;
        assert!(matches!(resp, ControlResponse::Ok { .. }), "expected Ok, got {resp:?}");
        let task = ctx.manager.get_by_id(id).await.expect("task still queued");
        assert_eq!(task.status, DownloadStatus::Paused);
    }

    #[tokio::test]
    async fn dispatch_pause_unknown_id_is_not_found() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(ControlRequest::Pause { id: Uuid::new_v4() }, &ctx).await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::NotFound),
            other => panic!("expected Error{{NotFound}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_start_all_reports_zero_when_nothing_pending() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(ControlRequest::StartAll { folder: None }, &ctx).await;
        match resp {
            ControlResponse::Ok { message } => assert_eq!(message, "Started 0 download(s)"),
            other => panic!("expected Ok, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_start_all_unknown_folder_is_invalid_input() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(
            ControlRequest::StartAll {
                folder: Some("no-such-folder".to_string()),
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::InvalidInput),
            other => panic!("expected Error{{InvalidInput}}, got {other:?}"),
        }
    }

    /// Reuses the responder-thread pattern from
    /// `manager::tests::concurrent_start_download_starts_once`: a real
    /// `start_download` call holds the start claim across a slow
    /// `BeforeRequest` round trip, and a concurrent `Pause` must wait it out
    /// instead of racing the eventual write-back.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_pause_waits_out_concurrent_start_then_succeeds() {
        use crate::script::message::ScriptRequest;
        use std::sync::mpsc;

        let manager = DownloadManager::new();
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let (script_tx, script_rx) = mpsc::channel::<ScriptRequest>();
        let responder = std::thread::spawn(move || {
            while let Ok(req) = script_rx.recv() {
                if let ScriptRequest::BeforeRequest { ctx, response, .. } = req {
                    std::thread::sleep(std::time::Duration::from_millis(150));
                    let _ = response.send((ctx, Ok(())));
                }
            }
        });
        let _ = responder;

        let config = std::sync::Arc::new(tokio::sync::RwLock::new(Config::default()));
        let manager_clone = manager.clone();
        tokio::spawn(async move {
            let _ = manager_clone.start_download(id, Some(script_tx), config).await;
        });

        // Let `start_download` acquire the claim before racing it.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !manager.is_start_in_progress(id) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "start did not claim the id within 2s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let ctx = test_context(manager);
        let resp = dispatch(ControlRequest::Pause { id }, &ctx).await;
        assert!(
            matches!(resp, ControlResponse::Ok { .. }),
            "Pause must wait out the claim and then succeed, got {resp:?}"
        );
        assert!(!ctx.manager.is_running(id).await);
        let task = ctx.manager.get_by_id(id).await.expect("task still queued");
        assert_eq!(task.status, DownloadStatus::Paused);
    }

    /// `Pause` must give up and report a retryable error once the claim-wait
    /// budget is exhausted, rather than hang the connection behind an
    /// arbitrarily slow script.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn dispatch_pause_busy_while_start_in_progress() {
        use crate::script::message::ScriptRequest;
        use std::sync::mpsc;

        let mut manager = DownloadManager::new();
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;
        manager.set_claim_wait_budget(std::time::Duration::from_millis(50));

        let (script_tx, script_rx) = mpsc::channel::<ScriptRequest>();
        let responder = std::thread::spawn(move || {
            while let Ok(req) = script_rx.recv() {
                if let ScriptRequest::BeforeRequest { ctx, response, .. } = req {
                    std::thread::sleep(std::time::Duration::from_millis(500));
                    let _ = response.send((ctx, Ok(())));
                }
            }
        });
        let _ = responder;

        let config = std::sync::Arc::new(tokio::sync::RwLock::new(Config::default()));
        let manager_clone = manager.clone();
        tokio::spawn(async move {
            let _ = manager_clone.start_download(id, Some(script_tx), config).await;
        });

        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !manager.is_start_in_progress(id) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "start did not claim the id within 2s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let ctx = test_context(manager);
        let resp = dispatch(ControlRequest::Pause { id }, &ctx).await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::InvalidInput),
            other => panic!("expected Error{{InvalidInput}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_history_item_returns_completed_immediately() {
        let manager = DownloadManager::new();
        let task = DownloadTask::new(
            "https://example.com/done".to_string(),
            std::path::PathBuf::from("done"),
        );
        let id = task.id;
        manager.add_to_history(task).await;
        let ctx = test_context(manager);

        let resp = dispatch(ControlRequest::Wait { id, timeout_secs: Some(3) }, &ctx).await;
        match resp {
            ControlResponse::Waited { outcome, task } => {
                assert_eq!(outcome, WaitOutcome::Completed);
                assert_eq!(task.map(|t| t.id), Some(id));
            }
            other => panic!("expected Waited{{Completed}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_removed_task_returns_removed() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Pending);
        let id = task.id;
        manager.add_download(task).await;
        let ctx = test_context(manager);

        let waiter = tokio::spawn({
            let manager = ctx.manager.clone();
            async move { dispatch(ControlRequest::Wait { id, timeout_secs: Some(5) }, &test_context(manager)).await }
        });

        // Give the wait loop time to subscribe before the remove fires.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let _ = ctx.manager.remove_download(id).await;

        // A tight bound (well under the 1 s poll interval) proves the event
        // branch, not the fallback poll, is what resolved this wait.
        let resp = tokio::time::timeout(std::time::Duration::from_millis(500), waiter)
            .await
            .expect("wait must return promptly after the remove event")
            .expect("wait task must not panic");
        match resp {
            ControlResponse::Waited { outcome, task } => {
                assert_eq!(outcome, WaitOutcome::Removed);
                assert!(task.is_none());
            }
            other => panic!("expected Waited{{Removed}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn wait_manual_pause_returns_paused() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Pending);
        let id = task.id;
        manager.add_download(task).await;
        let ctx = test_context(manager);

        let waiter = tokio::spawn({
            let manager = ctx.manager.clone();
            async move { dispatch(ControlRequest::Wait { id, timeout_secs: Some(5) }, &test_context(manager)).await }
        });

        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let _ = ctx.manager.pause_download(id).await;

        // A tight bound (well under the 1 s poll interval) proves the event
        // branch, not the fallback poll, is what resolved this wait.
        let resp = tokio::time::timeout(std::time::Duration::from_millis(500), waiter)
            .await
            .expect("wait must return promptly after the pause event")
            .expect("wait task must not panic");
        match resp {
            ControlResponse::Waited { outcome, task } => {
                assert_eq!(outcome, WaitOutcome::Paused);
                assert_eq!(task.map(|t| t.status), Some(DownloadStatus::Paused));
            }
            other => panic!("expected Waited{{Paused}}, got {other:?}"),
        }
    }

    /// A `Pending` task that nobody starts must not be mistaken for a final
    /// state; `Wait` keeps polling until `timeout_secs` elapses.
    #[tokio::test]
    async fn wait_timeout() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Pending);
        let id = task.id;
        manager.add_download(task).await;
        let ctx = test_context(manager);

        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            dispatch(ControlRequest::Wait { id, timeout_secs: Some(1) }, &ctx),
        )
        .await
        .expect("dispatch must return once the server-side timeout elapses");
        match resp {
            ControlResponse::Waited { outcome, task } => {
                assert_eq!(outcome, WaitOutcome::Timeout);
                assert!(task.is_none());
            }
            other => panic!("expected Waited{{Timeout}}, got {other:?}"),
        }
    }

    /// A handle can outlive its task's presence in every folder queue (the
    /// completion path removes the task from its queue, then awaits
    /// `queue.save()`, and only then adds it to history and drops the
    /// handle). `id` here is never added to any queue or history at all, so
    /// this simulates that in-between state directly: `Wait` must not read
    /// "missing everywhere" as `Removed` while a handle is still live.
    #[tokio::test]
    async fn wait_running_handle_with_no_queue_entry_is_not_removed() {
        let manager = DownloadManager::new();
        let id = Uuid::new_v4();
        let handle = tokio::spawn(std::future::pending::<()>());
        manager.insert_active_handle_for_test(id, handle).await;

        let resp = tokio::time::timeout(
            std::time::Duration::from_secs(3),
            dispatch(ControlRequest::Wait { id, timeout_secs: Some(1) }, &test_context(manager.clone())),
        )
        .await
        .expect("dispatch must return once the server-side timeout elapses");
        match resp {
            ControlResponse::Waited { outcome, .. } => {
                assert_eq!(outcome, WaitOutcome::Timeout, "a live handle must not be reported as Removed");
            }
            other => panic!("expected Waited{{Timeout}}, got {other:?}"),
        }

        manager.abort_active_handle_for_test(id).await;
    }

    #[tokio::test]
    async fn dispatch_modify_sets_and_clears_referrer() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Pending);
        let id = task.id;
        manager.add_download(task).await;
        let ctx = test_context(manager);

        let set_resp = dispatch(
            ControlRequest::Modify {
                id,
                edit: crate::ipc::control::protocol::HeaderEdit {
                    referrer: Some("https://a.example/".to_string()),
                    ..Default::default()
                },
            },
            &ctx,
        )
        .await;
        match set_resp {
            ControlResponse::Task { task, .. } => {
                assert_eq!(task.headers.get("Referer").map(String::as_str), Some("https://a.example/"));
            }
            other => panic!("expected Task, got {other:?}"),
        }

        let clear_resp = dispatch(
            ControlRequest::Modify {
                id,
                edit: crate::ipc::control::protocol::HeaderEdit {
                    clear_referrer: true,
                    ..Default::default()
                },
            },
            &ctx,
        )
        .await;
        match clear_resp {
            ControlResponse::Task { task, .. } => {
                assert!(!task.headers.contains_key("Referer"));
            }
            other => panic!("expected Task, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn modify_remove_header_case_insensitive() {
        let manager = DownloadManager::new();
        let mut task = task_with_status(DownloadStatus::Pending);
        task.headers.insert("X-Test".to_string(), "1".to_string());
        let id = task.id;
        manager.add_download(task).await;
        let ctx = test_context(manager);

        let resp = dispatch(
            ControlRequest::Modify {
                id,
                edit: crate::ipc::control::protocol::HeaderEdit {
                    remove_headers: vec!["x-test".to_string()],
                    ..Default::default()
                },
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Task { task, .. } => {
                assert!(!task.headers.contains_key("X-Test"), "removal must be case-insensitive");
            }
            other => panic!("expected Task, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_modify_unknown_id_is_not_found() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(
            ControlRequest::Modify {
                id: Uuid::new_v4(),
                edit: crate::ipc::control::protocol::HeaderEdit::default(),
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::NotFound),
            other => panic!("expected Error{{NotFound}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_modify_downloading_is_invalid_input() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Downloading);
        let id = task.id;
        manager.add_download(task).await;
        // `edit_task_headers` rejects via `is_running`, which checks the
        // handle map, not just the persisted status -- so a real live
        // handle is needed here, the same way `wait_running_handle_with_
        // no_queue_entry_is_not_removed` sets one up above.
        let handle = tokio::spawn(std::future::pending::<()>());
        manager.insert_active_handle_for_test(id, handle).await;
        let ctx = test_context(manager);

        let resp = dispatch(
            ControlRequest::Modify {
                id,
                edit: crate::ipc::control::protocol::HeaderEdit {
                    clear_referrer: true,
                    ..Default::default()
                },
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Error { code, message } => {
                assert_eq!(code, ControlErrorCode::InvalidInput);
                assert!(message.contains("pause the download first"), "message: {message}");
            }
            other => panic!("expected Error{{InvalidInput}}, got {other:?}"),
        }

        ctx.manager.abort_active_handle_for_test(id).await;
    }

    /// A task in retry backoff (`Paused` status, live handle still running)
    /// must not be reported as freshly started.
    #[tokio::test]
    async fn dispatch_start_running_task_not_reported_started() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Paused);
        let id = task.id;
        manager.add_download(task).await;
        let handle = tokio::spawn(std::future::pending::<()>());
        manager.insert_active_handle_for_test(id, handle).await;
        let ctx = test_context(manager);

        let resp = dispatch(ControlRequest::Start { id }, &ctx).await;
        match resp {
            ControlResponse::Ok { message } => {
                assert!(message.contains("already running"), "message: {message}");
            }
            other => panic!("expected Ok{{already running}}, got {other:?}"),
        }

        ctx.manager.abort_active_handle_for_test(id).await;
    }

    #[tokio::test]
    async fn dispatch_move_to_top_reorders() {
        let manager = DownloadManager::new();
        let task1 = DownloadTask::new(
            "https://example.com/a".to_string(),
            std::path::PathBuf::from("a"),
        );
        let task2 = DownloadTask::new(
            "https://example.com/b".to_string(),
            std::path::PathBuf::from("b"),
        );
        let id2 = task2.id;
        manager.add_download(task1).await;
        manager.add_download(task2).await;
        let ctx = test_context(manager);

        let resp = dispatch(
            ControlRequest::Move { id: id2, op: MoveOp::ToTop },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Ok { message } => {
                assert!(message.contains("top of queue"), "message: {message}");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
        let tasks = ctx.manager.get_all_downloads().await;
        assert_eq!(tasks[0].id, id2);
    }

    #[tokio::test]
    async fn dispatch_move_unknown_id_not_found() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(
            ControlRequest::Move {
                id: Uuid::new_v4(),
                op: MoveOp::ToTop,
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::NotFound),
            other => panic!("expected Error{{NotFound}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_move_unknown_folder_invalid() {
        let manager = DownloadManager::new();
        let task = DownloadTask::new(
            "https://example.com/a".to_string(),
            std::path::PathBuf::from("a"),
        );
        let id = task.id;
        manager.add_download(task).await;
        let ctx = test_context(manager);

        let resp = dispatch(
            ControlRequest::Move {
                id,
                op: MoveOp::Folder { folder: "no-such-folder".to_string() },
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::InvalidInput),
            other => panic!("expected Error{{InvalidInput}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_move_folder_moves_task() {
        let manager = DownloadManager::new();
        let task = DownloadTask::new(
            "https://example.com/a".to_string(),
            std::path::PathBuf::from("a"),
        );
        let id = task.id;
        manager.add_download(task).await;

        let mut config = Config::default();
        config.folders.insert(
            "archive".to_string(),
            crate::app::config::FolderConfig {
                name: "Archive".to_string(),
                ..Default::default()
            },
        );
        let ctx = DaemonContext {
            state: AppState::new(config),
            manager,
            shutdown: Arc::new(Notify::new()),
            started_at: Instant::now(),
            pipe_name: r"\\.\pipe\ggg-daemon-test".to_string(),
            save_lock: Arc::new(Mutex::new(())),
        };

        let resp = dispatch(
            ControlRequest::Move {
                id,
                op: MoveOp::Folder { folder: "archive".to_string() },
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Ok { message } => {
                assert!(message.contains("archive"), "message: {message}");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
        let task = ctx.manager.get_by_id(id).await.expect("task still queued");
        assert_eq!(task.folder_id, "archive");
    }

    #[tokio::test]
    async fn dispatch_priority_sets_priority() {
        let manager = DownloadManager::new();
        let task = DownloadTask::new(
            "https://example.com/a".to_string(),
            std::path::PathBuf::from("a"),
        );
        let id = task.id;
        manager.add_download(task).await;
        let ctx = test_context(manager);

        let resp = dispatch(ControlRequest::Priority { id, priority: 5 }, &ctx).await;
        match resp {
            ControlResponse::Ok { message } => {
                assert!(message.contains("5"), "message: {message}");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
        let task = ctx.manager.get_by_id(id).await.expect("task still queued");
        assert_eq!(task.priority, 5);
    }

    // Exercises the real `Config::save()`/load path, so it needs an isolated
    // config directory; `#[serial]` keeps it from racing other tests that
    // touch the same process-global override.
    #[tokio::test]
    #[serial_test::serial]
    async fn dispatch_config_set_then_get() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let ctx = test_context(DownloadManager::new());
        let set_resp = dispatch(
            ControlRequest::ConfigSet {
                key: "download.max_concurrent".to_string(),
                value: "7".to_string(),
            },
            &ctx,
        )
        .await;
        match set_resp {
            ControlResponse::Ok { message } => {
                assert!(message.contains("download.max_concurrent"), "message: {message}");
                assert!(message.contains('7'), "message: {message}");
            }
            other => panic!("expected Ok, got {other:?}"),
        }
        // Proves the override actually took effect: without it, `save()`
        // would write to the developer's real config directory instead.
        assert!(temp_dir.path().join("settings.toml").exists());

        let get_resp = dispatch(
            ControlRequest::ConfigGet { key: "download.max_concurrent".to_string() },
            &ctx,
        )
        .await;
        match get_resp {
            ControlResponse::Value { value } => assert_eq!(value, "7"),
            other => panic!("expected Value, got {other:?}"),
        }

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };
    }

    #[tokio::test]
    async fn dispatch_config_set_unknown_key_invalid() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(
            ControlRequest::ConfigSet {
                key: "no.such.key".to_string(),
                value: "1".to_string(),
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::InvalidInput),
            other => panic!("expected Error{{InvalidInput}}, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn dispatch_config_set_bad_value_leaves_config_unchanged() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(
            ControlRequest::ConfigSet {
                key: "download.max_concurrent".to_string(),
                value: "abc".to_string(),
            },
            &ctx,
        )
        .await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::InvalidInput),
            other => panic!("expected Error{{InvalidInput}}, got {other:?}"),
        }
        let config = ctx.state.config.read().await;
        assert_eq!(config.download.max_concurrent, 3);
    }
}
