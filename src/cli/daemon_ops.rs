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
use crate::download::enqueue::{build_task, enqueue, resolve_target_folder, EnqueueOptions, EnqueueOutcome};
use crate::download::manager::DownloadManager;
use crate::download::task::DownloadStatus;
use crate::ipc::control::protocol::{
    AddedTask, ControlErrorCode, ControlRequest, ControlResponse, DaemonStatus, SkippedUrl,
    CONTROL_PROTOCOL_VERSION,
};

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
        _ => ControlResponse::Error {
            code: ControlErrorCode::Unsupported,
            message: "not supported by this daemon yet".to_string(),
        },
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
}
