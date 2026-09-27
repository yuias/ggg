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
use crate::download::manager::DownloadManager;
use crate::download::task::{DownloadStatus, DownloadTask};
use crate::ipc::control::protocol::{
    AddedTask, ControlErrorCode, ControlRequest, ControlResponse, DaemonStatus, SkippedUrl,
    CONTROL_PROTOCOL_VERSION,
};
use uuid::Uuid;

/// How long `Pause`/`Remove` wait for a concurrent `start_download` call
/// (the beforeRequest script round trip) to finish before giving up. The
/// claim only spans that round trip, so a short wait makes the common case
/// -- a user pausing a task that a script just started -- succeed instead of
/// forcing them to retry by hand; the bound keeps a stuck script from
/// hanging the request indefinitely. `start_download` reads the task before
/// the script round trip and writes it back after (`manager.rs`), so a
/// pause/remove that lands in that window can still be overwritten by the
/// eventual write-back; waiting out the claim narrows that window but does
/// not close it, since a claim that clears one poll tick after our deadline
/// still loses the race (see `wait_for_start_claim_clear`, and the comment
/// on `dispatch_pause` for the remaining gap after the wait succeeds).
const START_CLAIM_WAIT_BUDGET: std::time::Duration = std::time::Duration::from_secs(2);
const START_CLAIM_POLL_INTERVAL: std::time::Duration = std::time::Duration::from_millis(25);

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
    /// Serializes every `save_queue_to_folders()` call the daemon makes:
    /// one from each mutating connection's `after_response` (these run on
    /// detached per-connection tasks that outlive `server_handle.abort()`),
    /// one from the periodic auto-save, and the final save on shutdown.
    /// `FolderQueue::save` writes a fixed `queue.toml.tmp` and renames it,
    /// so two concurrent saves of the same folder race on that rename.
    pub save_lock: Arc<Mutex<()>>,
}

/// Answers one request against the current daemon state. Variants not yet
/// implemented answer `Error{Unsupported}` rather than panicking or hanging,
/// so an older client talking to this daemon gets a clear message.
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
        _ => ControlResponse::Error {
            code: ControlErrorCode::Unsupported,
            message: "not supported by this daemon yet".to_string(),
        },
    }
}

fn not_found(message: &str) -> ControlResponse {
    ControlResponse::Error {
        code: ControlErrorCode::NotFound,
        message: message.to_string(),
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

/// Starts a single task. Distinguishes "this call actually started it" from
/// "it was already downloading, or another caller's start already claimed
/// it" so the reply does not falsely claim a fresh start; the manager's
/// `StartClaim` can still collapse a genuinely concurrent call into this
/// one's no-op after the check below, which is the accepted race documented
/// on `start_download`.
async fn dispatch_start(id: Uuid, ctx: &DaemonContext) -> ControlResponse {
    let Some(task) = ctx.manager.get_by_id(id).await else {
        return not_found("Download not found");
    };
    let already_in_flight = task.status == DownloadStatus::Downloading
        || ctx.manager.is_start_in_progress(id)
        || ctx.manager.is_running(id).await;
    if already_in_flight {
        return ControlResponse::Ok {
            message: format!("Download already running: {}", task.filename),
        };
    }
    match ctx
        .manager
        .start_download(id, ctx.state.script_sender.clone(), ctx.state.config.clone())
        .await
    {
        Ok(()) => ControlResponse::Ok {
            message: format!("Started download: {}", task.filename),
        },
        Err(e) => ControlResponse::Error {
            code: ControlErrorCode::Internal,
            message: e.to_string(),
        },
    }
}

/// Waits for `id`'s start claim (if any) to clear, so `Pause`/`Remove` never
/// race `start_download`'s post-script write-back (see the doc comment on
/// `START_CLAIM_WAIT_BUDGET` above). `budget` is a parameter (rather than
/// always `START_CLAIM_WAIT_BUDGET`) so a test can exercise the timeout
/// branch without waiting out the real, multi-second budget.
async fn wait_for_start_claim_clear(
    manager: &DownloadManager,
    id: Uuid,
    budget: std::time::Duration,
) -> Result<(), ControlResponse> {
    let deadline = tokio::time::Instant::now() + budget;
    while manager.is_start_in_progress(id) {
        if tokio::time::Instant::now() >= deadline {
            // Not a malformed request -- the id and the operation are both
            // fine, the start just hasn't finished yet. `InvalidInput` is
            // reused because the protocol has no dedicated "retryable"
            // error code (out of scope here); the message says so plainly
            // so the caller does not read this as bad input.
            return Err(ControlResponse::Error {
                code: ControlErrorCode::InvalidInput,
                message: format!(
                    "download {id} is still starting (beforeRequest script running); retry in a moment"
                ),
            });
        }
        tokio::time::sleep(START_CLAIM_POLL_INTERVAL).await;
    }
    Ok(())
}

async fn dispatch_pause(id: Uuid, ctx: &DaemonContext) -> ControlResponse {
    let Some(task) = ctx.manager.get_by_id(id).await else {
        return not_found("Download not found");
    };
    if let Err(resp) = wait_for_start_claim_clear(&ctx.manager, id, START_CLAIM_WAIT_BUDGET).await {
        return resp;
    }
    // The wait above only proves no start was in progress at that instant.
    // Nothing stops another caller (a concurrent `Start`, or the daemon's
    // own auto-start sweep) from acquiring a fresh claim for `id` right
    // after the wait returns and before `pause_download` runs below; that
    // caller's `start_download` can then set the task back to `Downloading`
    // after we set it to `Paused`, silently losing this pause. Not fixed
    // here: closing it needs a lock held across both the check and the
    // pause, which would serialize every pause/remove behind every start.
    let _ = ctx.manager.pause_download(id).await;
    ControlResponse::Ok {
        message: format!("Paused download: {}", task.filename),
    }
}

async fn dispatch_remove(id: Uuid, ctx: &DaemonContext) -> ControlResponse {
    if let Err(resp) = wait_for_start_claim_clear(&ctx.manager, id, START_CLAIM_WAIT_BUDGET).await {
        return resp;
    }
    // Same residual window as `dispatch_pause` above: a start claimed
    // between the wait and this call can still spawn a download for a task
    // this removes right after.
    match ctx.manager.remove_download(id).await {
        Some(task) => ControlResponse::Ok {
            message: format!("Removed download: {}", task.filename),
        },
        None => not_found("Download not found"),
    }
}

/// Starts every `Pending`/`Paused` task in scope (all folders, or one
/// resolved folder), skipping a task that is already starting or running.
/// Does not use `DownloadManager::start_folder_tasks`/`start_all_tasks`: those
/// count a start collapsed into another caller's in-flight start as started
/// (an accepted side effect for their existing callers), which would make
/// this response overstate how many downloads this call actually started.
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
        if ctx.manager.is_start_in_progress(task.id) || ctx.manager.is_running(task.id).await {
            continue;
        }
        match ctx
            .manager
            .start_download(task.id, ctx.state.script_sender.clone(), ctx.state.config.clone())
            .await
        {
            Ok(()) => started += 1,
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

#[cfg(windows)]
impl crate::ipc::control::server::ControlHandler for DaemonContext {
    fn handle(&self, req: ControlRequest) -> BoxFuture<'_, ControlResponse> {
        Box::pin(dispatch(req, self))
    }

    fn after_response(&self, req: &ControlRequest) -> BoxFuture<'_, ()> {
        let is_mutating = req.is_mutating();
        let is_shutdown = matches!(req, ControlRequest::Shutdown);
        Box::pin(async move {
            if is_mutating {
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
            state: AppState::new(Config::default(), "en"),
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
        ctx.after_response(&ControlRequest::Shutdown).await;
        let notified = tokio::time::timeout(
            std::time::Duration::from_millis(200),
            ctx.shutdown.notified(),
        )
        .await;
        assert!(notified.is_ok(), "shutdown was not notified");
    }

    #[tokio::test]
    async fn dispatch_unhandled_is_unsupported() {
        let ctx = test_context(DownloadManager::new());
        let resp = dispatch(ControlRequest::ReloadScripts, &ctx).await;
        match resp {
            ControlResponse::Error { code, .. } => assert_eq!(code, ControlErrorCode::Unsupported),
            other => panic!("expected Error{{Unsupported}}, got {other:?}"),
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
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(manager.is_start_in_progress(id), "start must have claimed the id by now");

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

    /// `wait_for_start_claim_clear` must give up and report a retryable
    /// error once its budget is exhausted, rather than hang the connection
    /// behind an arbitrarily slow script.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn wait_for_start_claim_clear_times_out_while_start_in_progress() {
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

        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        assert!(manager.is_start_in_progress(id), "start must have claimed the id by now");

        let result =
            wait_for_start_claim_clear(&manager, id, std::time::Duration::from_millis(50)).await;
        match result {
            Err(ControlResponse::Error { code, .. }) => assert_eq!(code, ControlErrorCode::InvalidInput),
            other => panic!("expected a retryable Error{{InvalidInput}}, got {other:?}"),
        }
    }
}
