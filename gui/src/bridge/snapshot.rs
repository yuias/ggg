//! Core -> UI snapshots.
//!
//! The UI thread must never wait on the async core, so a runtime task rebuilds
//! an immutable [`Snapshot`] when the core changes and publishes it over a
//! `watch` channel. The UI reads the latest `Arc<Snapshot>` once per frame.

use ggg::{
    AppState, Config,
    download::{
        enqueue::resolve_target_folder,
        manager::DownloadManager,
        task::{DownloadStatus, DownloadTask},
    },
};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    runtime::Handle,
    sync::{Notify, broadcast::error::RecvError, watch},
    time::{Instant, sleep, sleep_until},
};

/// Lower bound between two rebuilds; a burst of core events produces one.
const MIN_REBUILD_INTERVAL: Duration = Duration::from_millis(100);
/// Rebuild period while a transfer runs, so byte counters keep moving.
const TICK_ACTIVE: Duration = Duration::from_millis(250);
/// Rebuild period otherwise; catches changes that emit no event.
const TICK_IDLE: Duration = Duration::from_secs(1);

/// Immutable view of the core, safe to read from the UI thread.
#[derive(Debug, Default)]
pub struct Snapshot {
    // Read by the settings and dialog views.
    #[allow(dead_code)]
    pub revision: u64,
    #[allow(dead_code)]
    pub config: Arc<Config>,
    /// `(folder_id, display name)` in display order.
    pub folders: Vec<(String, String)>,
    pub default_folder_id: String,
    /// Queued and active tasks by folder id, in queue order.
    pub folder_downloads: HashMap<String, Vec<DownloadTask>>,
    pub history: Vec<DownloadTask>,
}

impl Snapshot {
    /// Number of queued and active tasks over all folders (history excluded).
    pub fn task_count(&self) -> usize {
        self.folder_downloads.values().map(Vec::len).sum()
    }

    fn any_downloading(&self) -> bool {
        self.folder_downloads
            .values()
            .flatten()
            .any(|t| t.status == DownloadStatus::Downloading)
    }
}

pub async fn build_snapshot(
    manager: &DownloadManager,
    state: &AppState,
    revision: u64,
) -> Snapshot {
    let config = Arc::new(state.config.read().await.clone());

    let mut folder_downloads: HashMap<String, Vec<DownloadTask>> = HashMap::new();
    for task in manager.get_all_downloads().await {
        folder_downloads
            .entry(task.folder_id.clone())
            .or_default()
            .push(task);
    }

    let history = manager.get_history().await;
    let folders = config.sorted_folder_entries();
    // Resolving with no folder name cannot fail; the fallback keeps that local.
    let default_folder_id =
        resolve_target_folder(&config, None).unwrap_or_else(|_| "default".to_string());

    Snapshot {
        revision,
        config,
        folders,
        default_folder_id,
        folder_downloads,
        history,
    }
}

pub struct SnapshotHandle {
    pub rx: watch::Receiver<Arc<Snapshot>>,
    /// Asks the task for a rebuild without waiting for the next core event.
    pub refresh: Arc<Notify>,
}

/// Starts the task that keeps the snapshot current. `wake` runs after every
/// publish and must not block (it posts an event to the UI loop). The task
/// ends once every receiver is dropped.
pub fn spawn_snapshot_task(
    rt: &Handle,
    manager: DownloadManager,
    state: AppState,
    wake: impl Fn() + Send + Sync + 'static,
) -> SnapshotHandle {
    let (tx, rx) = watch::channel(Arc::new(Snapshot::default()));
    let refresh = Arc::new(Notify::new());
    let task_refresh = refresh.clone();

    rt.spawn(async move {
        // Subscribe before the first build so an event in between is not lost.
        let mut events = manager.subscribe();
        let mut revision: u64 = 0;
        let mut last_build: Option<Instant> = None;

        // Each pass publishes first, so the UI has data before any core event;
        // the select below only decides when the next pass happens.
        loop {
            if let Some(at) = last_build {
                sleep_until(at + MIN_REBUILD_INTERVAL).await;
            }
            revision += 1;
            let snapshot = build_snapshot(&manager, &state, revision).await;
            last_build = Some(Instant::now());
            let downloading = snapshot.any_downloading();
            if revision == 1 {
                tracing::info!(
                    "Initial snapshot: {} task(s) in {} folder(s)",
                    snapshot.task_count(),
                    snapshot.folders.len()
                );
            }
            if tx.send(Arc::new(snapshot)).is_err() {
                break;
            }
            wake();

            let tick = if downloading { TICK_ACTIVE } else { TICK_IDLE };
            tokio::select! {
                _ = tx.closed() => break,
                event = events.recv() => {
                    // A lagged receiver missed events; the rebuild resyncs it.
                    if matches!(event, Err(RecvError::Closed)) {
                        break;
                    }
                }
                _ = task_refresh.notified() => {}
                _ = sleep(tick) => {}
            }
        }
    });

    SnapshotHandle { rx, refresh }
}

#[cfg(test)]
mod tests {
    use super::*;
    use ggg::app::config::FolderConfig;

    fn config_with_folders(names: &[&str]) -> Config {
        let mut config = Config::default();
        config.folders.clear();
        for name in names {
            let folder = FolderConfig {
                name: (*name).to_string(),
                ..FolderConfig::default()
            };
            config.folders.insert(format!("id-{name}"), folder);
        }
        config
    }

    fn task_in(config: &Config, folder_id: &str, url: &str) -> DownloadTask {
        DownloadTask::new_with_folder(url.to_string(), folder_id.to_string(), config)
    }

    #[tokio::test]
    async fn build_snapshot_groups_by_folder() {
        let config = config_with_folders(&["alpha", "beta"]);
        let manager = DownloadManager::new();
        for url in ["http://example.com/a1", "http://example.com/a2"] {
            manager.add_download(task_in(&config, "id-alpha", url)).await;
        }
        manager
            .add_download(task_in(&config, "id-beta", "http://example.com/b1"))
            .await;
        let state = AppState::new(config);

        let snapshot = build_snapshot(&manager, &state, 7).await;

        assert_eq!(snapshot.revision, 7);
        assert_eq!(snapshot.folder_downloads["id-alpha"].len(), 2);
        assert_eq!(snapshot.folder_downloads["id-beta"].len(), 1);
        assert_eq!(snapshot.task_count(), 3);
        assert_eq!(snapshot.folders.len(), 2);
        assert!(snapshot.history.is_empty());
        assert!(snapshot.folders.iter().any(|(id, _)| *id == snapshot.default_folder_id));
    }

    #[tokio::test]
    async fn snapshot_task_publishes_after_add() {
        let config = config_with_folders(&["alpha"]);
        let manager = DownloadManager::new();
        let state = AppState::new(config.clone());
        let mut handle = spawn_snapshot_task(&Handle::current(), manager.clone(), state, || {});

        // Drain the initial publish so the next change is the one we add.
        tokio::time::timeout(Duration::from_secs(2), handle.rx.changed())
            .await
            .expect("initial snapshot within 2 s")
            .unwrap();
        assert_eq!(handle.rx.borrow_and_update().task_count(), 0);

        manager
            .add_download(task_in(&config, "id-alpha", "http://example.com/a"))
            .await;

        tokio::time::timeout(Duration::from_secs(2), handle.rx.changed())
            .await
            .expect("snapshot after add within 2 s")
            .unwrap();
        assert_eq!(handle.rx.borrow().task_count(), 1);
    }
}
