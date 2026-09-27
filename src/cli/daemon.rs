use crate::app::config::Config;
use crate::app::state::AppState;
use crate::download::manager::{DownloadEvent, DownloadManager};
use crate::script::message::ScriptRequest;
use anyhow::Result;
use std::sync::{mpsc, Arc};
use std::time::Duration;
use tokio::signal;
use tokio::sync::{broadcast, Notify, RwLock};
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
/// `control` is `None` on non-Windows targets, or when a control pipe binding
/// isn't wired up yet; the daemon still runs downloads and scripts without a
/// pipe to control it from.
pub async fn run_daemon(
    state: AppState,
    manager: DownloadManager,
    recovered: Vec<Uuid>,
    control: Option<()>,
) -> Result<()> {
    tracing::info!("Starting daemon mode...");
    tracing::info!("Press Ctrl+C to stop");

    let config = state.config.clone();
    let script_sender = state.script_sender.clone();

    // Start tasks already pending in auto-start folders.
    auto_start_sweep(&config, &script_sender, &manager).await;

    // Resume tasks the previous process left running.
    for id in recovered {
        resume_if_auto_start(id, &config, &script_sender, &manager).await;
    }

    let shutdown = Arc::new(Notify::new());

    // Auto-save and the event-triggered sweep share one task, selecting
    // between the tick and the event receiver, so at most one sweep runs at
    // a time. `start_download` only flips a task to `Downloading` after
    // awaiting the beforeRequest script round-trip, so two sweeps running
    // concurrently in separate tasks could both see the same task as
    // `Pending` and start it twice.
    let sweep_manager = manager.clone();
    let sweep_config = config.clone();
    let sweep_sender = script_sender.clone();
    let background_handle = tokio::spawn(async move {
        let mut interval = tokio::time::interval(AUTO_SAVE_INTERVAL);
        interval.tick().await; // first tick fires immediately; skip it
        let mut rx = sweep_manager.subscribe();

        loop {
            tokio::select! {
                _ = interval.tick() => {
                    auto_start_sweep(&sweep_config, &sweep_sender, &sweep_manager).await;

                    match sweep_manager.save_queue_to_folders().await {
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

    match control {
        Some(()) => println!("ggg daemon running (pid {})", std::process::id()),
        None => println!(
            "ggg daemon running (pid {}, no control pipe)",
            std::process::id()
        ),
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

    // Abort and await so an in-flight auto-save can't race the final save's
    // tmp-file write and rename below.
    background_handle.abort();
    let _ = background_handle.await;

    // Running tasks are saved as `Downloading` on purpose: the next process's
    // `recover_interrupted` turns them into `Paused`, so a crash and a
    // graceful stop resume the same way.
    tracing::info!("Saving queue to folder files...");
    manager.save_queue_to_folders().await?;

    tracing::info!("Daemon stopped");

    Ok(())
}
