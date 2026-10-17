//! Queue operations shared by the front ends (start/pause, retry, delete,
//! undo, add). None of them saves the queue or the config; callers decide
//! when to persist, because they know whether a batch is complete.

use crate::app::state::AppState;
use crate::download::enqueue::{build_task, enqueue, EnqueueOptions, EnqueueOutcome};
use crate::download::manager::{DownloadManager, StartOutcome};
use crate::download::task::{DownloadStatus, DownloadTask};
use uuid::Uuid;

/// How many delete batches a front end keeps for undo.
pub const MAX_UNDO_BATCHES: usize = 10;

/// Outcome counts of an operation over several targets.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct OpReport {
    /// Targets the operation changed.
    pub affected: usize,
    /// Targets skipped because another operation held their claim.
    pub busy: usize,
    /// Targets whose start was refused (folder limit, circuit breaker, ...).
    pub failed: usize,
}

/// Start or pause `targets`, decided by `reference` (the focused row).
///
/// If the reference is in flight (downloading, waiting for a permit, in retry
/// backoff or mid-start), in-flight targets are paused in one batch so N busy
/// targets wait one claim-wait budget in total, not N budgets. Otherwise the
/// targets in Pending/Paused/Error are started. A busy claim or a refused
/// start is an expected outcome, so it is counted and logged, not an error.
pub async fn toggle_tasks(
    manager: &DownloadManager,
    state: &AppState,
    reference: Option<Uuid>,
    targets: &[Uuid],
) -> OpReport {
    let mut report = OpReport::default();
    let reference_in_flight = match reference {
        Some(id) => manager.is_in_flight(id).await,
        None => false,
    };

    if reference_in_flight {
        // Pausing only the in-flight targets lets a target still in retry
        // backoff be paused too, and leaves idle ones untouched.
        let mut in_flight_ids = Vec::new();
        for &id in targets {
            if manager.is_in_flight(id).await {
                in_flight_ids.push(id);
            }
        }
        for (id, result) in in_flight_ids
            .iter()
            .copied()
            .zip(manager.pause_many_if_in_flight(&in_flight_ids).await)
        {
            match result {
                Ok(true) => report.affected += 1,
                Ok(false) => {}
                Err(e) => {
                    tracing::warn!("Failed to pause download {}: {}", id, e);
                    report.busy += 1;
                }
            }
        }
    } else {
        for &id in targets {
            let Some(task) = manager.get_by_id(id).await else {
                continue;
            };
            if !matches!(
                task.status,
                DownloadStatus::Pending | DownloadStatus::Paused | DownloadStatus::Error
            ) {
                continue;
            }
            match manager
                .start_download(id, state.script_sender.clone(), state.config.clone())
                .await
            {
                Ok(StartOutcome::Started) => report.affected += 1,
                Ok(StartOutcome::AlreadyRunning | StartOutcome::Busy) => {}
                Err(e) => {
                    tracing::warn!("Failed to start download {}: {}", id, e);
                    report.failed += 1;
                }
            }
        }
    }
    report
}

/// Start the targets whose status is Error.
pub async fn retry_tasks(manager: &DownloadManager, state: &AppState, targets: &[Uuid]) -> OpReport {
    let mut report = OpReport::default();
    for &id in targets {
        let Some(task) = manager.get_by_id(id).await else {
            continue;
        };
        if task.status != DownloadStatus::Error {
            continue;
        }
        match manager
            .start_download(id, state.script_sender.clone(), state.config.clone())
            .await
        {
            Ok(StartOutcome::Started) => report.affected += 1,
            Ok(StartOutcome::AlreadyRunning | StartOutcome::Busy) => {}
            Err(e) => {
                tracing::warn!("Failed to retry download {}: {}", id, e);
                report.failed += 1;
            }
        }
    }
    report
}

/// Remove `targets` as one batch. Each removed task goes to history marked
/// Deleted. The returned batch, kept by the caller for undo, holds the tasks
/// as they were when removed: `restore_tasks` needs their pre-delete status.
pub async fn delete_tasks(manager: &DownloadManager, targets: &[Uuid]) -> (Vec<DownloadTask>, OpReport) {
    let mut batch = Vec::new();
    let mut report = OpReport::default();
    let results = manager.remove_many(targets).await;
    for (id, result) in targets.iter().copied().zip(results) {
        match result {
            Ok(Some(task)) => {
                let mut history_entry = task.clone();
                history_entry.status = DownloadStatus::Deleted;
                manager.add_to_history(history_entry).await;
                batch.push(task);
            }
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("Failed to remove download {}: {}", id, e);
                report.busy += 1;
            }
        }
    }
    report.affected = batch.len();
    (batch, report)
}

/// Status a deleted task gets when it is queued again. The removal aborted a
/// running download, so it comes back Paused rather than claiming to run.
fn restored_status(status: DownloadStatus) -> DownloadStatus {
    match status {
        DownloadStatus::Pending | DownloadStatus::Paused | DownloadStatus::Error => status,
        DownloadStatus::Downloading => DownloadStatus::Paused,
        // Not produced by `delete_tasks`; a startable status is the safe fallback.
        DownloadStatus::Completed | DownloadStatus::Deleted => DownloadStatus::Pending,
    }
}

/// Re-enqueue a deleted batch (undo) and drop the history entries the delete
/// created for it. `affected` counts tasks that were added again; a task
/// refused as a duplicate is not counted and keeps its history entry, and a
/// refused auto-start counts as `failed` while the task stays queued.
pub async fn restore_tasks(manager: &DownloadManager, state: &AppState, batch: Vec<DownloadTask>) -> OpReport {
    let mut report = OpReport::default();
    for mut task in batch {
        let id = task.id;
        let folder_id = task.folder_id.clone();
        task.status = restored_status(task.status);
        match enqueue(manager, state, task, false, true).await {
            EnqueueOutcome::Duplicate => {}
            EnqueueOutcome::Added { started, start_error, .. } => {
                // Only the entry the delete wrote: if auto-start already ran
                // the task to completion, that newer entry must stay.
                if manager
                    .get_history_item(id)
                    .await
                    .is_some_and(|entry| entry.status == DownloadStatus::Deleted)
                {
                    manager.remove_from_history(id).await;
                }
                report.affected += 1;
                if started {
                    tracing::debug!("Auto-started download in folder '{}'", folder_id);
                } else if let Some(e) = start_error {
                    // The task stays queued with its restored status; a refused
                    // start must not abort the rest of the batch.
                    tracing::warn!("Auto-start failed in folder '{}': {}", folder_id, e);
                    report.failed += 1;
                }
            }
        }
    }
    report
}

/// Folder "Delete all": remove every task of `folder_id` without writing
/// history and without an undo batch.
pub async fn delete_folder_tasks(manager: &DownloadManager, folder_id: &str) -> OpReport {
    let mut report = OpReport::default();
    let ids: Vec<Uuid> = manager
        .get_folder_downloads(folder_id)
        .await
        .iter()
        .map(|t| t.id)
        .collect();
    for (id, result) in ids.iter().copied().zip(manager.remove_many(&ids).await) {
        match result {
            Ok(Some(_)) => report.affected += 1,
            Ok(None) => {}
            Err(e) => {
                tracing::warn!("Failed to remove download {}: {}", id, e);
                report.busy += 1;
            }
        }
    }
    report
}

/// User-chosen location for a single-URL add. `filename` is a bare file name
/// (already validated: `sanitize_filename(&filename) == filename`). `dir ==
/// None` keeps the folder's save path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SaveAs {
    pub dir: Option<std::path::PathBuf>,
    pub filename: String,
}

/// URLs to register in one go.
pub struct AddRequest {
    pub urls: Vec<String>,
    pub folder_id: String,
    pub options: EnqueueOptions,
    /// Start the tasks even when the folder does not auto-start.
    pub immediate: bool,
    pub save_as: Option<SaveAs>,
}

#[derive(Debug, Default)]
pub struct AddReport {
    /// Ids of the tasks that were queued, in input order.
    pub added: Vec<Uuid>,
    /// URLs refused because the folder prevents duplicate URLs.
    pub duplicates: usize,
    /// Start failures of added tasks; they stay queued as Pending.
    pub start_errors: Vec<String>,
}

/// `build_task` + `enqueue` for each URL (starting allowed).
///
/// When `save_as` is Some and `urls.len() == 1`, after `build_task` and before
/// `enqueue`: `task.save_path = dir` (if Some) and
/// `task.set_user_filename(filename)`. When `save_as` is Some but
/// `urls.len() != 1`, it is ignored with a warning: one name for many URLs
/// would only produce deduplicated siblings, and the dialog never sends that.
pub async fn add_urls(manager: &DownloadManager, state: &AppState, req: AddRequest) -> AddReport {
    let AddRequest { urls, folder_id, options, immediate, save_as } = req;
    let mut report = AddReport::default();

    let save_as = match save_as {
        Some(save_as) if urls.len() == 1 => Some(save_as),
        Some(_) => {
            tracing::warn!("Ignoring 'save as' for a registration of {} URLs", urls.len());
            None
        }
        None => None,
    };

    // Build every task under one config read, then release the lock before
    // the awaits in `enqueue`.
    let tasks: Vec<DownloadTask> = {
        let config = state.config.read().await;
        urls.iter()
            .map(|url| {
                let mut task = build_task(url, &folder_id, &config, &options);
                if let Some(save_as) = &save_as {
                    if let Some(dir) = &save_as.dir {
                        task.save_path = dir.clone();
                    }
                    task.set_user_filename(save_as.filename.clone());
                }
                task
            })
            .collect()
    };

    for task in tasks {
        match enqueue(manager, state, task, immediate, true).await {
            EnqueueOutcome::Duplicate => report.duplicates += 1,
            EnqueueOutcome::Added { id, started, start_error } => {
                report.added.push(id);
                if started {
                    tracing::debug!("Auto-started download in folder '{}'", folder_id);
                } else if let Some(e) = start_error {
                    // The task stays queued as Pending; refusing to start it
                    // must not abort the whole add.
                    tracing::warn!("Auto-start failed in folder '{}': {}", folder_id, e);
                    report.start_errors.push(e);
                }
            }
        }
    }
    report
}

/// Open `path` in the platform file manager. Spawns and returns; the child is
/// not waited for.
pub fn open_in_file_manager(path: &std::path::Path) {
    #[cfg(target_os = "windows")]
    {
        let _ = std::process::Command::new("explorer")
            .arg(path.to_string_lossy().to_string())
            .spawn();
    }
    #[cfg(target_os = "macos")]
    {
        let _ = std::process::Command::new("open")
            .arg(path.to_string_lossy().to_string())
            .spawn();
    }
    #[cfg(target_os = "linux")]
    {
        let _ = std::process::Command::new("xdg-open")
            .arg(path.to_string_lossy().to_string())
            .spawn();
    }
    tracing::info!("Opening folder: {}", path.display());
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::{Config, FolderConfig};
    use std::path::PathBuf;

    const FOLDER: &str = "downloads";

    // Auto-start is off, so nothing here spawns a download.
    fn config_with_folder(prevent_duplicate: bool) -> Config {
        let mut config = Config::default();
        let folder = FolderConfig {
            name: FOLDER.to_string(),
            save_path: PathBuf::from("folder-default-dir"),
            auto_start_downloads: false,
            prevent_duplicate_url: prevent_duplicate,
            ..FolderConfig::default()
        };
        config.folders.clear();
        config.folders.insert(FOLDER.to_string(), folder);
        config
    }

    fn request(urls: &[&str]) -> AddRequest {
        AddRequest {
            urls: urls.iter().map(|u| u.to_string()).collect(),
            folder_id: FOLDER.to_string(),
            options: EnqueueOptions::default(),
            immediate: false,
            save_as: None,
        }
    }

    async fn queued(prevent_duplicate: bool, urls: &[&str]) -> (DownloadManager, AppState, Vec<Uuid>) {
        let state = AppState::new(config_with_folder(prevent_duplicate));
        let manager = DownloadManager::new();
        let report = add_urls(&manager, &state, request(urls)).await;
        assert_eq!(report.added.len(), urls.len());
        (manager, state, report.added)
    }

    #[tokio::test]
    async fn delete_returns_batch_with_original_status_and_marks_history_deleted() {
        let (manager, _state, ids) = queued(false, &["https://example.com/a.bin", "https://example.com/b.bin"]).await;

        let (batch, report) = delete_tasks(&manager, &ids).await;

        assert_eq!(report, OpReport { affected: 2, ..Default::default() });
        assert_eq!(batch.len(), 2);
        assert!(batch.iter().all(|t| t.status == DownloadStatus::Pending));
        for id in &ids {
            assert!(manager.get_by_id(*id).await.is_none());
            let in_history = manager.get_history_item(*id).await.expect("deleted task is in history");
            assert_eq!(in_history.status, DownloadStatus::Deleted);
        }
    }

    #[tokio::test]
    async fn delete_counts_busy_target() {
        let state = AppState::new(config_with_folder(false));
        let mut manager = DownloadManager::new();
        manager.set_claim_wait_budget(std::time::Duration::from_millis(100));
        let report = add_urls(&manager, &state, request(&["https://example.com/a.bin"])).await;
        let id = report.added[0];

        // A live handle alone does not make a removal busy (removal stops it);
        // only a held claim does.
        let claim = manager.hold_claim_for_test(id);
        let (batch, report) = delete_tasks(&manager, &[id]).await;
        drop(claim);

        assert!(batch.is_empty());
        assert_eq!(report, OpReport { busy: 1, ..Default::default() });
        assert!(manager.get_by_id(id).await.is_some(), "a busy target must stay queued");
        assert!(manager.get_history_item(id).await.is_none());
    }

    #[tokio::test]
    async fn restore_requeues_batch() {
        let (manager, state, ids) = queued(false, &["https://example.com/a.bin", "https://example.com/b.bin"]).await;
        let (batch, _) = delete_tasks(&manager, &ids).await;
        assert!(manager.get_folder_downloads(FOLDER).await.is_empty());

        let report = restore_tasks(&manager, &state, batch).await;

        assert_eq!(report, OpReport { affected: 2, ..Default::default() });
        let restored: Vec<Uuid> = manager.get_folder_downloads(FOLDER).await.iter().map(|t| t.id).collect();
        assert_eq!(restored, ids);
    }

    // Queues one task per status; adding directly keeps `Downloading` free of
    // a real download handle.
    async fn queued_with_statuses(
        manager: &DownloadManager,
        state: &AppState,
        statuses: &[DownloadStatus],
    ) -> Vec<Uuid> {
        let config = state.config.read().await.clone();
        let mut ids = Vec::new();
        for (i, status) in statuses.iter().enumerate() {
            let url = format!("https://example.com/{i}.bin");
            let mut task = build_task(&url, FOLDER, &config, &EnqueueOptions::default());
            task.status = *status;
            ids.push(task.id);
            manager.add_download(task).await;
        }
        ids
    }

    #[tokio::test]
    async fn restore_resets_status_from_deleted() {
        let state = AppState::new(config_with_folder(false));
        let manager = DownloadManager::new();
        let statuses = [
            DownloadStatus::Pending,
            DownloadStatus::Paused,
            DownloadStatus::Error,
            DownloadStatus::Downloading,
        ];
        let ids = queued_with_statuses(&manager, &state, &statuses).await;
        let (batch, _) = delete_tasks(&manager, &ids).await;

        restore_tasks(&manager, &state, batch).await;

        let expected = [
            DownloadStatus::Pending,
            DownloadStatus::Paused,
            DownloadStatus::Error,
            DownloadStatus::Paused,
        ];
        for (id, expected) in ids.iter().zip(expected) {
            let task = manager.get_by_id(*id).await.expect("restored");
            assert_eq!(task.status, expected);
        }
    }

    #[tokio::test]
    async fn restore_removes_history_entries_of_the_batch() {
        let (manager, state, ids) = queued(false, &["https://example.com/a.bin", "https://example.com/b.bin"]).await;
        let (batch, _) = delete_tasks(&manager, &ids).await;
        assert_eq!(manager.history_len().await, 2);

        restore_tasks(&manager, &state, batch).await;

        assert_eq!(manager.history_len().await, 0);
        for id in &ids {
            assert!(manager.get_history_item(*id).await.is_none());
        }
    }

    #[tokio::test]
    async fn restore_keeps_unrelated_history() {
        let (manager, state, ids) = queued(false, &["https://example.com/a.bin", "https://example.com/b.bin"]).await;
        let mut finished = DownloadTask::new("https://example.com/done.bin".to_string(), PathBuf::from("dir"));
        finished.status = DownloadStatus::Completed;
        let finished_id = finished.id;
        manager.add_to_history(finished).await;
        // Only the first task is undone; the second stays deleted.
        let (mut batch, _) = delete_tasks(&manager, &ids).await;
        batch.truncate(1);

        restore_tasks(&manager, &state, batch).await;

        assert!(manager.get_history_item(finished_id).await.is_some());
        assert!(manager.get_history_item(ids[0]).await.is_none());
        assert!(manager.get_history_item(ids[1]).await.is_some());
        assert_eq!(manager.history_len().await, 2);
    }

    #[tokio::test]
    async fn restore_keeps_history_of_a_refused_duplicate() {
        let state = AppState::new(config_with_folder(true));
        let manager = DownloadManager::new();
        let added = add_urls(&manager, &state, request(&["https://example.com/a.bin"])).await;
        let (batch, _) = delete_tasks(&manager, &added.added).await;
        // The same URL is queued again before the undo.
        add_urls(&manager, &state, request(&["https://example.com/a.bin"])).await;

        let report = restore_tasks(&manager, &state, batch).await;

        assert_eq!(report.affected, 0);
        assert!(manager.get_history_item(added.added[0]).await.is_some());
    }

    #[tokio::test]
    async fn add_urls_applies_referrer_and_headers() {
        let state = AppState::new(config_with_folder(false));
        let manager = DownloadManager::new();
        let mut req = request(&["https://example.com/a.bin"]);
        req.options = EnqueueOptions {
            referrer: Some("https://ref.example/".to_string()),
            headers: vec![("X-Extra".to_string(), "1".to_string())],
        };

        let report = add_urls(&manager, &state, req).await;

        let task = manager.get_by_id(report.added[0]).await.expect("queued");
        assert_eq!(task.headers.get("Referer").map(String::as_str), Some("https://ref.example/"));
        assert_eq!(task.headers.get("X-Extra").map(String::as_str), Some("1"));
        assert_eq!(task.status, DownloadStatus::Pending);
    }

    #[tokio::test]
    async fn add_urls_reports_duplicates_when_folder_prevents_them() {
        let state = AppState::new(config_with_folder(true));
        let manager = DownloadManager::new();

        let report = add_urls(
            &manager,
            &state,
            request(&["https://example.com/a.bin", "https://example.com/a.bin", "https://example.com/b.bin"]),
        )
        .await;

        assert_eq!(report.added.len(), 2);
        assert_eq!(report.duplicates, 1);
        assert!(report.start_errors.is_empty());
        assert_eq!(manager.get_folder_downloads(FOLDER).await.len(), 2);
    }

    #[tokio::test]
    async fn delete_folder_tasks_removes_all() {
        let (manager, _state, ids) = queued(false, &["https://example.com/a.bin", "https://example.com/b.bin"]).await;

        let report = delete_folder_tasks(&manager, FOLDER).await;

        assert_eq!(report, OpReport { affected: 2, ..Default::default() });
        assert!(manager.get_folder_downloads(FOLDER).await.is_empty());
        assert_eq!(manager.history_len().await, 0, "folder delete-all writes no history");
        assert!(manager.get_by_id(ids[0]).await.is_none());
    }

    #[tokio::test]
    async fn add_urls_single_url_applies_save_as() {
        let state = AppState::new(config_with_folder(false));
        let manager = DownloadManager::new();
        let dir = PathBuf::from("chosen-dir");
        let mut req = request(&["https://example.com/server-name.bin"]);
        req.save_as = Some(SaveAs { dir: Some(dir.clone()), filename: "mine.bin".to_string() });

        let report = add_urls(&manager, &state, req).await;

        let task = manager.get_by_id(report.added[0]).await.expect("queued");
        assert_eq!(task.filename, "mine.bin");
        assert_eq!(task.user_filename.as_deref(), Some("mine.bin"));
        assert_eq!(task.save_path, dir);
    }

    #[tokio::test]
    async fn add_urls_save_as_without_dir_keeps_folder_path() {
        let state = AppState::new(config_with_folder(false));
        let manager = DownloadManager::new();
        let mut req = request(&["https://example.com/server-name.bin"]);
        req.save_as = Some(SaveAs { dir: None, filename: "mine.bin".to_string() });

        let report = add_urls(&manager, &state, req).await;

        let task = manager.get_by_id(report.added[0]).await.expect("queued");
        assert_eq!(task.filename, "mine.bin");
        assert_eq!(task.save_path, PathBuf::from("folder-default-dir"));
    }

    #[tokio::test]
    async fn add_urls_ignores_save_as_for_multiple_urls() {
        let state = AppState::new(config_with_folder(false));
        let manager = DownloadManager::new();
        let mut req = request(&["https://example.com/a.bin", "https://example.com/b.bin"]);
        req.save_as = Some(SaveAs { dir: Some(PathBuf::from("chosen-dir")), filename: "mine.bin".to_string() });

        let report = add_urls(&manager, &state, req).await;

        let mut names = Vec::new();
        for id in &report.added {
            let task = manager.get_by_id(*id).await.expect("queued");
            assert_eq!(task.user_filename, None);
            assert_eq!(task.save_path, PathBuf::from("folder-default-dir"));
            names.push(task.filename);
        }
        assert_eq!(names, ["a.bin", "b.bin"]);
    }

    #[tokio::test]
    async fn retry_skips_targets_that_are_not_in_error() {
        let (manager, state, ids) = queued(false, &["https://example.com/a.bin"]).await;

        let report = retry_tasks(&manager, &state, &ids).await;

        assert_eq!(report, OpReport::default());
        let task = manager.get_by_id(ids[0]).await.expect("queued");
        assert_eq!(task.status, DownloadStatus::Pending, "a Pending target must not be started");
    }
}
