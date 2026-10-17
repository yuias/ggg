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
use std::{
    collections::{HashMap, hash_map::DefaultHasher},
    hash::Hasher as _,
    io::Write,
    sync::Arc,
    time::Duration,
};
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

    /// Hash of everything the UI can show, so an unchanged core can be told
    /// from a changed one without comparing field by field (the core types
    /// have no `PartialEq`). `revision` is left out on purpose.
    fn content_hash(&self) -> u64 {
        let mut hasher = HashWriter(DefaultHasher::new());
        feed(&mut hasher, &*self.config);
        feed(&mut hasher, &self.folders);
        feed(&mut hasher, &self.default_folder_id);
        // HashMap order is not stable between rebuilds.
        let mut folder_ids: Vec<&String> = self.folder_downloads.keys().collect();
        folder_ids.sort();
        for id in folder_ids {
            feed(&mut hasher, id);
            feed(&mut hasher, &self.folder_downloads[id]);
        }
        feed(&mut hasher, &self.history);
        hasher.0.finish()
    }

    fn any_downloading(&self) -> bool {
        self.folder_downloads
            .values()
            .flatten()
            .any(|t| t.status == DownloadStatus::Downloading)
    }
}

/// `io::Write` that feeds the bytes to a hasher.
struct HashWriter(DefaultHasher);

impl Write for HashWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.write(buf);
        Ok(buf.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

/// Serializes `value` into the hash. A serialization error only makes the
/// content look the same as before, which the next tick during a transfer
/// or the next core event corrects, so it is not reported.
fn feed<T: serde::Serialize + ?Sized>(hasher: &mut HashWriter, value: &T) {
    let _ = serde_json::to_writer(&mut *hasher, value);
    hasher.0.write_u8(0xff);
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
        let mut last_hash: Option<u64> = None;

        // Each pass publishes first, so the UI has data before any core event;
        // the select below only decides when the next pass happens.
        loop {
            if let Some(at) = last_build {
                sleep_until(at + MIN_REBUILD_INTERVAL).await;
            }
            let snapshot = build_snapshot(&manager, &state, revision + 1).await;
            last_build = Some(Instant::now());
            let downloading = snapshot.any_downloading();
            // An idle, unchanged core must not wake the UI every tick. While a
            // transfer runs the UI still needs frames: speed and ETA derive
            // from the clock, not from the snapshot content.
            let hash = snapshot.content_hash();
            if downloading || last_hash != Some(hash) {
                last_hash = Some(hash);
                revision += 1;
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
            }

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
    use std::sync::atomic::{AtomicUsize, Ordering};

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

    fn counting_wake() -> (Arc<AtomicUsize>, impl Fn() + Send + Sync + 'static) {
        let count = Arc::new(AtomicUsize::new(0));
        let counter = count.clone();
        (count, move || {
            counter.fetch_add(1, Ordering::SeqCst);
        })
    }

    #[tokio::test]
    async fn idle_unchanged_core_neither_publishes_nor_wakes() {
        let config = config_with_folders(&["alpha"]);
        let manager = DownloadManager::new();
        manager
            .add_download(task_in(&config, "id-alpha", "http://example.com/a"))
            .await;
        let state = AppState::new(config);
        let (wakes, wake) = counting_wake();
        let mut handle = spawn_snapshot_task(&Handle::current(), manager, state, wake);

        tokio::time::timeout(Duration::from_secs(2), handle.rx.changed())
            .await
            .expect("initial snapshot within 2 s")
            .unwrap();
        handle.rx.borrow_and_update();
        assert_eq!(wakes.load(Ordering::SeqCst), 1);

        // More than two idle ticks pass; none may publish.
        sleep(TICK_IDLE * 2 + Duration::from_millis(300)).await;
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "woke the UI without a change");
        assert!(!handle.rx.has_changed().unwrap());
        assert_eq!(handle.rx.borrow().revision, 1);
    }

    #[tokio::test]
    async fn real_change_publishes_after_an_idle_period() {
        let config = config_with_folders(&["alpha"]);
        let manager = DownloadManager::new();
        let state = AppState::new(config.clone());
        let (wakes, wake) = counting_wake();
        let handle = spawn_snapshot_task(&Handle::current(), manager.clone(), state, wake);

        sleep(Duration::from_millis(300)).await;
        assert_eq!(wakes.load(Ordering::SeqCst), 1);

        manager
            .add_download(task_in(&config, "id-alpha", "http://example.com/a"))
            .await;
        sleep(Duration::from_millis(600)).await;
        assert_eq!(wakes.load(Ordering::SeqCst), 2);
        assert_eq!(handle.rx.borrow().task_count(), 1);
        assert_eq!(handle.rx.borrow().revision, 2);
    }

    #[tokio::test]
    async fn active_download_keeps_publishing_while_unchanged() {
        let config = config_with_folders(&["alpha"]);
        let manager = DownloadManager::new();
        let mut task = task_in(&config, "id-alpha", "http://example.com/a");
        task.status = DownloadStatus::Downloading;
        manager.add_download(task).await;
        let state = AppState::new(config);
        let (wakes, wake) = counting_wake();
        let _handle = spawn_snapshot_task(&Handle::current(), manager, state, wake);

        sleep(Duration::from_millis(1100)).await;
        // The initial publish plus one per active tick.
        assert!(wakes.load(Ordering::SeqCst) >= 3, "{}", wakes.load(Ordering::SeqCst));
    }

    #[test]
    fn content_hash_ignores_revision_and_tracks_content() {
        let config = config_with_folders(&["alpha"]);
        let mut a = Snapshot {
            revision: 1,
            config: Arc::new(config.clone()),
            ..Snapshot::default()
        };
        a.folder_downloads.insert(
            "id-alpha".into(),
            vec![task_in(&config, "id-alpha", "http://example.com/a")],
        );
        let mut b = Snapshot {
            revision: 9,
            config: Arc::new(config),
            folders: a.folders.clone(),
            default_folder_id: a.default_folder_id.clone(),
            folder_downloads: a.folder_downloads.clone(),
            history: a.history.clone(),
        };
        assert_eq!(a.content_hash(), b.content_hash());

        b.folder_downloads.get_mut("id-alpha").unwrap()[0].downloaded += 1;
        assert_ne!(a.content_hash(), b.content_hash());
    }
}
