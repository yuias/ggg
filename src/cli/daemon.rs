use crate::app::config::Config;
use crate::app::state::AppState;
use crate::cli::daemon_ops::{ControlBinding, DaemonContext};
use crate::download::manager::{DownloadEvent, DownloadManager};
use crate::script::message::ScriptRequest;
use anyhow::Result;
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};
use tokio::signal;
use tokio::sync::{broadcast, Mutex, Notify, RwLock};
use uuid::Uuid;

/// How often the auto-save task ticks, and re-runs the auto-start sweep so a
/// task that was refused a folder slot earlier gets another chance.
const AUTO_SAVE_INTERVAL: Duration = Duration::from_secs(30);

/// `AppState` is `Send + Sync`, but sweeping and resuming only need the
/// config and the script sender, so those two fields are threaded through
/// separately instead of the whole `AppState`.
async fn auto_start_sweep(
    config: &Arc<RwLock<Config>>,
    script_sender: &Option<mpsc::Sender<ScriptRequest>>,
    manager: &DownloadManager,
) {
    let folder_ids: Vec<String> = {
        let cfg = config.read().await;
        cfg.folders
            .iter()
            .filter(|(_, folder)| folder.auto_start_downloads)
            .map(|(id, _)| id.clone())
            .collect()
    };

    for folder_id in folder_ids {
        manager
            .start_folder_tasks(&folder_id, script_sender.clone(), config.clone())
            .await;
    }
}

/// Resumes a task that was interrupted by the previous process, but only if
/// its folder auto-starts. Otherwise it stays `Paused`, as
/// `recover_interrupted` left it, until a user or command starts it.
async fn resume_if_auto_start(
    id: Uuid,
    config: &Arc<RwLock<Config>>,
    script_sender: &Option<mpsc::Sender<ScriptRequest>>,
    manager: &DownloadManager,
) {
    let folder_id = match manager.get_by_id(id).await {
        Some(task) => task.folder_id,
        None => return,
    };
    let auto_start = {
        let cfg = config.read().await;
        cfg.folders
            .get(&folder_id)
            .map(|folder| folder.auto_start_downloads)
            .unwrap_or(false)
    };
    if !auto_start {
        return;
    }
    if let Err(e) = manager
        .start_download(id, script_sender.clone(), config.clone())
        .await
    {
        tracing::warn!("Failed to resume interrupted download {}: {}", id, e);
    }
}

/// Run in headless daemon mode.
///
/// `control` is `None` on non-Windows targets, or when the control pipe
/// couldn't be bound; the daemon still runs downloads and scripts without a
/// pipe to control it from. When `Some`, `serve` is started as the very
/// first thing below, before the auto-start sweep, so the gap between
/// `bind` (in `main.rs`, before `Config::load`) and the pipe actually
/// accepting connections stays as short as possible. The first instance is
/// already listening during that gap, so a client that opens it there just
/// waits for the `Ping` reply; it only sees `Unreachable` if the gap ends up
/// longer than the client's 2 s ping timeout.
pub async fn run_daemon(
    state: AppState,
    manager: DownloadManager,
    recovered: Vec<Uuid>,
    control: Option<ControlBinding>,
) -> Result<()> {
    tracing::info!("Starting daemon mode...");
    tracing::info!("Press Ctrl+C to stop");

    let config = state.config.clone();
    let script_sender = state.script_sender.clone();
    let shutdown = Arc::new(Notify::new());
    // Orders the daemon's own `save_queue_to_folders()` calls relative to
    // each other: `DaemonContext`'s `after_response` (on detached
    // per-connection tasks that outlive aborting the server), the auto-save
    // below, and the final save. Per-file write safety now lives in
    // `FolderQueue::save` itself.
    let save_lock = Arc::new(Mutex::new(()));

    #[cfg(windows)]
    let pipe_name = control.as_ref().map(|bound| bound.name().to_string());
    #[cfg(not(windows))]
    let pipe_name: Option<String> = None;
    let has_control = pipe_name.is_some();

    let ctx = Arc::new(DaemonContext {
        state,
        manager: manager.clone(),
        shutdown: shutdown.clone(),
        started_at: Instant::now(),
        pipe_name: pipe_name
            .unwrap_or_else(crate::ipc::control::protocol::default_control_pipe_name),
        save_lock: save_lock.clone(),
    });

    #[cfg(windows)]
    let server_handle = control.map(|bound| crate::ipc::control::server::serve(bound, ctx.clone()));
    #[cfg(not(windows))]
    let server_handle: Option<tokio::task::JoinHandle<()>> = {
        let _ = control;
        None
    };

    // Start tasks already pending in auto-start folders.
    auto_start_sweep(&config, &script_sender, &manager).await;

    // Resume tasks the previous process left running.
    for id in recovered {
        resume_if_auto_start(id, &config, &script_sender, &manager).await;
    }

    // Auto-save and the event-triggered sweep share one task so that at most
    // one sweep runs at a time; this is a convenience, not the guard against
    // double starts. `start_download` holds an in-memory claim per id across
    // the whole call, including the beforeRequest script round-trip, so pipe
    // handlers can call `start_download` / `start_folder_tasks` /
    // `start_all_tasks` directly, concurrently with this sweep, without a
    // daemon-level lock.
    let sweep_manager = manager.clone();
    let sweep_config = config.clone();
    let sweep_sender = script_sender.clone();
    let sweep_save_lock = save_lock.clone();
    let background_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(AUTO_SAVE_INTERVAL);
        interval.tick().await; // first tick fires immediately; skip it
        let mut rx = sweep_manager.subscribe();

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    auto_start_sweep(&sweep_config, &sweep_sender, &sweep_manager).await;

                    let save_result = {
                        let _guard = sweep_save_lock.lock().await;
                        sweep_manager.save_queue_to_folders().await
                    };
                    match save_result {
                        Ok(_) => tracing::debug!("Queue auto-saved to folder files"),
                        Err(e) => tracing::error!("Failed to auto-save queue: {}", e),
                    }
                }
                event = rx.recv() => {
                    match event {
                        Ok(DownloadEvent::Completed { .. })
                        | Ok(DownloadEvent::Failed { .. })
                        | Ok(DownloadEvent::Removed { .. }) => {
                            auto_start_sweep(&sweep_config, &sweep_sender, &sweep_manager).await;
                        }
                        Ok(_) => {}
                        // A lagging receiver may have missed a trigger; sweep to be safe.
                        Err(broadcast::error::RecvError::Lagged(_)) => {
                            auto_start_sweep(&sweep_config, &sweep_sender, &sweep_manager).await;
                        }
                        Err(broadcast::error::RecvError::Closed) => break,
                    }
                }
            }
        }
    });

    if has_control {
        println!(
            "ggg daemon running (pid {}, pipe {})",
            std::process::id(),
            ctx.pipe_name
        );
    } else {
        println!(
            "ggg daemon running (pid {}, no control pipe)",
            std::process::id()
        );
    }

    tokio::select! {
        result = signal::ctrl_c() => {
            match result {
                Ok(()) => tracing::info!("Received Ctrl+C, shutting down..."),
                Err(e) => tracing::error!("Error waiting for Ctrl+C: {}", e),
            }
        }
        _ = shutdown.notified() => {
            tracing::info!("Shutdown requested, shutting down...");
        }
    }

    // Shutdown via the control pipe (`daemon stop`) goes through the same
    // path as Ctrl+C from here on: stop accepting new requests, abort and
    // await the background sweep/save task so it can't race the final save
    // below, then do the final save.
    if let Some(h) = server_handle {
        h.abort();
        let _ = h.await;
    }

    // Abort and await so an in-flight auto-save can't race the final save's
    // tmp-file write and rename below.
    background_handle.abort();
    let _ = background_handle.await;

    // Running tasks are saved as `Downloading` on purpose: the next process's
    // `recover_interrupted` turns them into `Paused`, so a crash and a
    // graceful stop resume the same way.
    tracing::info!("Saving queue to folder files...");
    {
        // Waits out any save still in flight from a connection task that
        // outlived `server_handle.abort()` above (aborting never interrupts
        // `spawn_blocking` file IO), so this save is never racing that one.
        let _guard = save_lock.lock().await;
        manager.save_queue_to_folders().await?;
    }

    tracing::info!("Daemon stopped");

    Ok(())
}
