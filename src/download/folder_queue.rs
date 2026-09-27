//! Per-folder download queue management
//!
//! Each folder maintains its own queue of download tasks with:
//! - Independent task list (VecDeque for efficient operations)
//! - Per-folder concurrency semaphore
//! - Task count tracking (pending/downloading)
//!
//! This enables fair round-robin scheduling across folders while
//! respecting both per-folder and global concurrent download limits.

use crate::download::task::{DownloadStatus, DownloadTask};
use std::collections::VecDeque;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::{RwLock, Semaphore};
use uuid::Uuid;

/// Task counts for a folder queue
#[derive(Debug, Clone, Default)]
pub struct FolderTaskCounts {
    /// Number of pending tasks waiting to be downloaded
    pub pending: usize,
    /// Number of tasks currently downloading
    pub downloading: usize,
}

impl FolderTaskCounts {
    /// Returns true if folder has any active tasks (pending or downloading)
    pub fn has_active_tasks(&self) -> bool {
        self.pending > 0 || self.downloading > 0
    }

    /// Returns total number of active tasks
    pub fn total(&self) -> usize {
        self.pending + self.downloading
    }
}

/// TOML serialization wrapper for queue persistence
#[derive(serde::Serialize, serde::Deserialize)]
struct QueueFile {
    tasks: Vec<DownloadTask>,
}

/// Per-folder download queue with concurrency control
#[derive(Clone)]
pub struct FolderQueue {
    /// Folder identifier
    folder_id: String,
    /// Tasks in this folder's queue
    tasks: Arc<RwLock<VecDeque<DownloadTask>>>,
    /// Semaphore for per-folder concurrent download limit
    semaphore: Arc<Semaphore>,
    /// Task counts (pending/downloading) for efficient status checks
    counts: Arc<RwLock<FolderTaskCounts>>,
}

impl FolderQueue {
    /// Create a new empty folder queue
    ///
    /// # Arguments
    /// * `folder_id` - Unique folder identifier
    /// * `max_concurrent` - Maximum concurrent downloads for this folder
    pub fn new(folder_id: impl Into<String>, max_concurrent: usize) -> Self {
        Self {
            folder_id: folder_id.into(),
            tasks: Arc::new(RwLock::new(VecDeque::new())),
            semaphore: Arc::new(Semaphore::new(max_concurrent)),
            counts: Arc::new(RwLock::new(FolderTaskCounts::default())),
        }
    }

    /// Get the folder ID
    pub fn folder_id(&self) -> &str {
        &self.folder_id
    }

    /// Get the semaphore for this folder's concurrent downloads
    pub fn semaphore(&self) -> Arc<Semaphore> {
        Arc::clone(&self.semaphore)
    }

    /// Check if a URL already exists in the queue
    pub async fn contains_url(&self, url: &str) -> bool {
        let tasks = self.tasks.read().await;
        tasks.iter().any(|t| t.url == url)
    }

    /// Add a task to the queue
    pub async fn add(&self, task: DownloadTask) {
        let is_pending = task.status == DownloadStatus::Pending;
        let is_downloading = task.status == DownloadStatus::Downloading;

        let mut tasks = self.tasks.write().await;
        tasks.push_back(task);

        // Update counts
        if is_pending || is_downloading {
            let mut counts = self.counts.write().await;
            if is_pending {
                counts.pending += 1;
            } else if is_downloading {
                counts.downloading += 1;
            }
        }
    }

    /// Get all tasks in this queue
    pub async fn get_all(&self) -> Vec<DownloadTask> {
        let tasks = self.tasks.read().await;
        tasks.iter().cloned().collect()
    }

    /// Get task count
    pub async fn len(&self) -> usize {
        let tasks = self.tasks.read().await;
        tasks.len()
    }

    /// Check if queue is empty
    pub async fn is_empty(&self) -> bool {
        let tasks = self.tasks.read().await;
        tasks.is_empty()
    }

    /// Remove a task by ID
    pub async fn remove(&self, id: Uuid) -> Option<DownloadTask> {
        let mut tasks = self.tasks.write().await;
        if let Some(pos) = tasks.iter().position(|t| t.id == id) {
            let task = tasks.remove(pos)?;

            // Update counts
            let mut counts = self.counts.write().await;
            match task.status {
                DownloadStatus::Pending => {
                    counts.pending = counts.pending.saturating_sub(1);
                }
                DownloadStatus::Downloading => {
                    counts.downloading = counts.downloading.saturating_sub(1);
                }
                _ => {}
            }

            Some(task)
        } else {
            None
        }
    }

    /// Get a task by ID
    pub async fn get_by_id(&self, id: Uuid) -> Option<DownloadTask> {
        let tasks = self.tasks.read().await;
        tasks.iter().find(|t| t.id == id).cloned()
    }

    /// Update an existing task
    pub async fn update(&self, task: DownloadTask) {
        let mut tasks = self.tasks.write().await;
        if let Some(pos) = tasks.iter().position(|t| t.id == task.id) {
            let old_status = tasks[pos].status;
            let new_status = task.status;

            tasks[pos] = task;

            // Update counts if status changed
            if old_status != new_status {
                let mut counts = self.counts.write().await;
                // Decrement old status count
                match old_status {
                    DownloadStatus::Pending => {
                        counts.pending = counts.pending.saturating_sub(1);
                    }
                    DownloadStatus::Downloading => {
                        counts.downloading = counts.downloading.saturating_sub(1);
                    }
                    _ => {}
                }
                // Increment new status count
                match new_status {
                    DownloadStatus::Pending => {
                        counts.pending += 1;
                    }
                    DownloadStatus::Downloading => {
                        counts.downloading += 1;
                    }
                    _ => {}
                }
            }
        }
    }

    /// Finds `id`, rejects a `Downloading` task, applies `f`, and returns a
    /// clone of the updated task -- all under one `tasks.write()` lock, so
    /// no concurrent mutation of the same task (a pause, a priority change,
    /// ...) can land between the check and the apply. `None` covers both
    /// "not in this queue" and "rejected, still Downloading"; the caller
    /// (`DownloadManager::edit_task_headers`) already rules the latter out
    /// before calling this, via the start claim and `is_running`. Only
    /// headers change here, so unlike `update` the counts need no
    /// bookkeeping.
    pub async fn modify<F: FnOnce(&mut DownloadTask)>(&self, id: Uuid, f: F) -> Option<DownloadTask> {
        let mut tasks = self.tasks.write().await;
        let pos = tasks.iter().position(|t| t.id == id)?;
        if tasks[pos].status == DownloadStatus::Downloading {
            return None;
        }
        f(&mut tasks[pos]);
        Some(tasks[pos].clone())
    }

    /// Get current task counts
    pub async fn get_counts(&self) -> FolderTaskCounts {
        let counts = self.counts.read().await;
        counts.clone()
    }

    /// Returns true if folder has any active tasks
    pub async fn has_active_tasks(&self) -> bool {
        let counts = self.counts.read().await;
        counts.has_active_tasks()
    }

    /// Increment pending count
    pub async fn increment_pending(&self) {
        let mut counts = self.counts.write().await;
        counts.pending += 1;
    }

    /// Decrement pending count
    pub async fn decrement_pending(&self) {
        let mut counts = self.counts.write().await;
        counts.pending = counts.pending.saturating_sub(1);
    }

    /// Increment downloading count
    pub async fn increment_downloading(&self) {
        let mut counts = self.counts.write().await;
        counts.downloading += 1;
    }

    /// Decrement downloading count
    pub async fn decrement_downloading(&self) {
        let mut counts = self.counts.write().await;
        counts.downloading = counts.downloading.saturating_sub(1);
    }

    /// Rebuild counts from actual task statuses
    /// Call this after loading from disk or when counts might be out of sync
    pub async fn rebuild_counts(&self) {
        let tasks = self.tasks.read().await;
        let mut counts = self.counts.write().await;

        counts.pending = 0;
        counts.downloading = 0;

        for task in tasks.iter() {
            match task.status {
                DownloadStatus::Pending => counts.pending += 1,
                DownloadStatus::Downloading => counts.downloading += 1,
                _ => {}
            }
        }
    }

    /// Get all pending tasks (for scheduling)
    pub async fn get_pending_tasks(&self) -> Vec<DownloadTask> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .filter(|t| t.status == DownloadStatus::Pending)
            .cloned()
            .collect()
    }

    /// Get next pending task (for scheduling)
    /// Returns the highest priority pending task
    pub async fn next_pending(&self) -> Option<DownloadTask> {
        let tasks = self.tasks.read().await;
        tasks
            .iter()
            .filter(|t| t.status == DownloadStatus::Pending)
            .max_by_key(|t| t.priority)
            .cloned()
    }

    /// Save queue to TOML file
    ///
    /// Uses the folder-specific queue path: {config_dir}/{folder_id}/queue.toml
    pub async fn save(&self) -> anyhow::Result<()> {
        let queue_path = crate::util::paths::get_folder_queue_path(&self.folder_id)?;

        // Create parent directory if needed
        if let Some(parent) = queue_path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }

        let tasks = self.tasks.read().await;
        let queue_file = QueueFile {
            tasks: tasks.iter().cloned().collect(),
        };
        let toml = toml::to_string_pretty(&queue_file)?;

        // Atomic write: temp file + rename
        let temp_path = queue_path.with_extension("toml.tmp");
        tokio::fs::write(&temp_path, &toml).await?;
        tokio::fs::rename(&temp_path, &queue_path).await?;

        tracing::debug!(
            "Saved {} tasks to folder queue: {}",
            tasks.len(),
            queue_path.display()
        );

        Ok(())
    }

    /// Load queue from TOML file
    ///
    /// Loads from: {config_dir}/{folder_id}/queue.toml
    pub async fn load(&self) -> anyhow::Result<()> {
        let queue_path = crate::util::paths::get_folder_queue_path(&self.folder_id)?;

        if !queue_path.exists() {
            tracing::debug!(
                "No queue file found for folder {}: {}",
                self.folder_id,
                queue_path.display()
            );
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&queue_path).await?;
        let queue_file: QueueFile = toml::from_str(&content)?;

        {
            let mut tasks = self.tasks.write().await;
            tasks.clear();
            tasks.extend(queue_file.tasks);

            tracing::debug!(
                "Loaded {} tasks from folder queue: {}",
                tasks.len(),
                queue_path.display()
            );
        }

        // Rebuild counts after loading
        self.rebuild_counts().await;

        Ok(())
    }

    /// Load queue from a specific path
    pub async fn load_from_path(&self, path: &Path) -> anyhow::Result<()> {
        if !path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(path).await?;
        let queue_file: QueueFile = toml::from_str(&content)?;

        {
            let mut tasks = self.tasks.write().await;
            tasks.clear();
            tasks.extend(queue_file.tasks);
        }

        self.rebuild_counts().await;
        Ok(())
    }

    /// Delete the queue file if it exists
    pub async fn delete_file(&self) -> anyhow::Result<()> {
        let queue_path = crate::util::paths::get_folder_queue_path(&self.folder_id)?;

        if queue_path.exists() {
            tokio::fs::remove_file(&queue_path).await?;
            tracing::debug!("Deleted queue file: {}", queue_path.display());
        }

        Ok(())
    }

    /// Set priority for a task
    pub async fn set_priority(&self, id: Uuid, priority: i32) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(pos) = tasks.iter().position(|t| t.id == id) {
            tasks[pos].priority = priority;
            true
        } else {
            false
        }
    }

    /// Move task to top of queue (highest priority position)
    pub async fn move_to_top(&self, id: Uuid) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(pos) = tasks.iter().position(|t| t.id == id) {
            if pos > 0 {
                let task = tasks.remove(pos).unwrap();
                tasks.push_front(task);
            }
            true
        } else {
            false
        }
    }

    /// Move task to bottom of queue
    pub async fn move_to_bottom(&self, id: Uuid) -> bool {
        let mut tasks = self.tasks.write().await;
        if let Some(pos) = tasks.iter().position(|t| t.id == id) {
            if pos < tasks.len() - 1 {
                let task = tasks.remove(pos).unwrap();
                tasks.push_back(task);
            }
            true
        } else {
            false
        }
    }

    /// Move task before another task
    pub async fn move_before(&self, id: Uuid, before_id: Uuid) -> bool {
        let mut tasks = self.tasks.write().await;

        let from_pos = tasks.iter().position(|t| t.id == id);
        let to_pos = tasks.iter().position(|t| t.id == before_id);

        if let (Some(from), Some(to)) = (from_pos, to_pos) {
            if from != to {
                let task = tasks.remove(from).unwrap();
                let new_to = if from < to { to - 1 } else { to };
                tasks.insert(new_to, task);
                return true;
            }
        }
        false
    }

    /// Count of downloading tasks
    pub async fn downloading_count(&self) -> usize {
        let counts = self.counts.read().await;
        counts.downloading
    }

    /// Count of pending tasks
    pub async fn pending_count(&self) -> usize {
        let counts = self.counts.read().await;
        counts.pending
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::task::DownloadTask;
    use std::path::PathBuf;

    fn create_test_task(status: DownloadStatus) -> DownloadTask {
        let mut task = DownloadTask::new(
            "https://example.com/file.txt".to_string(),
            PathBuf::from("/tmp"),
        );
        task.status = status;
        task.folder_id = "test-folder".to_string();
        task
    }

    #[tokio::test]
    async fn test_folder_queue_new() {
        let queue = FolderQueue::new("test-folder", 3);
        assert_eq!(queue.folder_id(), "test-folder");
        assert!(queue.is_empty().await);
    }

    #[tokio::test]
    async fn test_folder_queue_add_and_get() {
        let queue = FolderQueue::new("test-folder", 3);
        let task = create_test_task(DownloadStatus::Pending);
        let task_id = task.id;

        queue.add(task).await;

        assert_eq!(queue.len().await, 1);
        assert!(!queue.is_empty().await);

        let retrieved = queue.get_by_id(task_id).await;
        assert!(retrieved.is_some());
        assert_eq!(retrieved.unwrap().id, task_id);
    }

    #[tokio::test]
    async fn test_folder_queue_counts() {
        let queue = FolderQueue::new("test-folder", 3);

        queue.add(create_test_task(DownloadStatus::Pending)).await;
        queue.add(create_test_task(DownloadStatus::Pending)).await;
        queue
            .add(create_test_task(DownloadStatus::Downloading))
            .await;
        queue
            .add(create_test_task(DownloadStatus::Completed))
            .await;

        let counts = queue.get_counts().await;
        assert_eq!(counts.pending, 2);
        assert_eq!(counts.downloading, 1);
        assert!(counts.has_active_tasks());
    }

    #[tokio::test]
    async fn test_folder_queue_remove() {
        let queue = FolderQueue::new("test-folder", 3);
        let task = create_test_task(DownloadStatus::Pending);
        let task_id = task.id;

        queue.add(task).await;
        assert_eq!(queue.pending_count().await, 1);

        let removed = queue.remove(task_id).await;
        assert!(removed.is_some());
        assert_eq!(queue.pending_count().await, 0);
        assert!(queue.is_empty().await);
    }

    #[tokio::test]
    async fn test_folder_queue_update_status() {
        let queue = FolderQueue::new("test-folder", 3);
        let mut task = create_test_task(DownloadStatus::Pending);
        let _task_id = task.id;

        queue.add(task.clone()).await;
        assert_eq!(queue.pending_count().await, 1);
        assert_eq!(queue.downloading_count().await, 0);

        // Update status from Pending to Downloading
        task.status = DownloadStatus::Downloading;
        queue.update(task.clone()).await;

        assert_eq!(queue.pending_count().await, 0);
        assert_eq!(queue.downloading_count().await, 1);

        // Update to Completed
        task.status = DownloadStatus::Completed;
        queue.update(task).await;

        assert_eq!(queue.pending_count().await, 0);
        assert_eq!(queue.downloading_count().await, 0);
    }

    #[tokio::test]
    async fn test_folder_queue_next_pending() {
        let queue = FolderQueue::new("test-folder", 3);

        let mut task1 = create_test_task(DownloadStatus::Pending);
        task1.priority = 1;

        let mut task2 = create_test_task(DownloadStatus::Pending);
        task2.priority = 5;

        let mut task3 = create_test_task(DownloadStatus::Pending);
        task3.priority = 3;

        queue.add(task1).await;
        queue.add(task2.clone()).await;
        queue.add(task3).await;

        // Should return highest priority task
        let next = queue.next_pending().await;
        assert!(next.is_some());
        assert_eq!(next.unwrap().priority, 5);
    }

    #[tokio::test]
    async fn test_folder_queue_move_operations() {
        let queue = FolderQueue::new("test-folder", 3);

        let task1 = create_test_task(DownloadStatus::Pending);
        let task2 = create_test_task(DownloadStatus::Pending);
        let task3 = create_test_task(DownloadStatus::Pending);

        let id1 = task1.id;
        let _id2 = task2.id;
        let id3 = task3.id;

        queue.add(task1).await;
        queue.add(task2).await;
        queue.add(task3).await;

        // Move task3 to top
        assert!(queue.move_to_top(id3).await);
        let all = queue.get_all().await;
        assert_eq!(all[0].id, id3);

        // Move task3 to bottom
        assert!(queue.move_to_bottom(id3).await);
        let all = queue.get_all().await;
        assert_eq!(all[2].id, id3);

        // Move task3 before task1
        assert!(queue.move_before(id3, id1).await);
        let all = queue.get_all().await;
        assert_eq!(all[0].id, id3);
        assert_eq!(all[1].id, id1);
    }

    #[tokio::test]
    async fn test_folder_task_counts_operations() {
        let counts = FolderTaskCounts::default();
        assert_eq!(counts.pending, 0);
        assert_eq!(counts.downloading, 0);
        assert!(!counts.has_active_tasks());
        assert_eq!(counts.total(), 0);

        let counts = FolderTaskCounts {
            pending: 3,
            downloading: 2,
        };
        assert!(counts.has_active_tasks());
        assert_eq!(counts.total(), 5);
    }

    #[tokio::test]
    async fn test_folder_queue_rebuild_counts() {
        let queue = FolderQueue::new("test-folder", 3);

        queue.add(create_test_task(DownloadStatus::Pending)).await;
        queue.add(create_test_task(DownloadStatus::Pending)).await;
        queue
            .add(create_test_task(DownloadStatus::Downloading))
            .await;

        // Manually corrupt counts
        {
            let mut counts = queue.counts.write().await;
            counts.pending = 100;
            counts.downloading = 50;
        }

        // Rebuild should fix them
        queue.rebuild_counts().await;

        let counts = queue.get_counts().await;
        assert_eq!(counts.pending, 2);
        assert_eq!(counts.downloading, 1);
    }

    #[tokio::test]
    async fn test_folder_queue_contains_url() {
        let queue = FolderQueue::new("test-folder", 3);

        let task = create_test_task(DownloadStatus::Pending);
        let url = task.url.clone();

        assert!(!queue.contains_url(&url).await);

        queue.add(task).await;
        assert!(queue.contains_url(&url).await);
        assert!(!queue.contains_url("https://example.com/other.txt").await);
    }
}
