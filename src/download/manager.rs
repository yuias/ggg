use super::folder_queue::FolderQueue;
use super::history::DownloadHistory;
use super::http_client::HttpClient;
use super::queue::DownloadQueue;
use super::task::{DownloadStatus, DownloadTask};
use crate::file::metadata::apply_last_modified;
use crate::file::naming::sanitize_filename;
use crate::script::events::BeforeRequestContext;
use crate::script::message::ScriptRequest;
use crate::script::sender;
use anyhow::Result;
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use tokio::sync::{broadcast, RwLock, Semaphore};
use tokio::task::JoinHandle;
use uuid::Uuid;

/// Lifecycle events broadcast by the [`DownloadManager`].
///
/// Consumers (TUI, GUI, …) can `subscribe()` to react to state changes without
/// polling `get_all_downloads()`. Polling still works and remains the source of
/// truth; this stream is an optional, lossy notification layer. A lagging
/// receiver observes `RecvError::Lagged` and should re-sync via a full poll.
#[derive(Debug, Clone)]
pub enum DownloadEvent {
    /// A task was enqueued.
    Added { task_id: Uuid, folder_id: String },
    /// A task entered the `Downloading` state (fresh start or resume).
    Started { task_id: Uuid },
    /// Progress tick. Coalesced and throttled — only the latest per task is meaningful.
    Progress {
        task_id: Uuid,
        downloaded: u64,
        total: Option<u64>,
        speed: f64, // bytes per second
    },
    /// A task finished successfully.
    Completed { task_id: Uuid },
    /// A task failed after exhausting its retries.
    Failed { task_id: Uuid, error: String },
    /// A task was paused (manually or as the result of a stop request).
    Paused { task_id: Uuid },
    /// A task was removed from its queue.
    Removed { task_id: Uuid },
}

/// Capacity of the lifecycle event broadcast buffer. Sized generously so a
/// briefly-stalled subscriber doesn't immediately lag; progress events are the
/// only high-frequency variant and are throttled at the source.
const EVENT_CHANNEL_CAPACITY: usize = 256;

/// Per-folder task counts for O(1) folder status checks
/// Re-exported from folder_queue for backward compatibility
pub use super::folder_queue::FolderTaskCounts;

/// Failure modes for [`DownloadManager::edit_task_headers`].
#[derive(Debug, thiserror::Error)]
pub enum EditHeadersError {
    #[error("Download not found")]
    NotFound,
    /// `&'static str` is the reason folded into the message ("starting" or
    /// "downloading"); both map to exit 3 ("pause the download first") at
    /// the CLI layer.
    #[error("download {0} is {1}; pause the download first")]
    Busy(Uuid, &'static str),
}

/// One running (or finished but not yet released) download.
struct ActiveEntry {
    handle: JoinHandle<()>,
    /// Distinguishes this spawn from a later one for the same id, so a
    /// finishing body never removes a newer entry.
    generation: u64,
    /// True while the retry loop sleeps between attempts.
    in_backoff: Arc<AtomicBool>,
}

/// Point-in-time view of a task's live work in this process.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RunState {
    /// No claim and no live handle.
    Idle,
    /// Some operation (start, pause, remove, move, header edit) holds the claim.
    Claimed,
    /// Live handle, not in backoff (downloading or waiting for a permit).
    Active,
    /// Live handle sleeping between retry attempts; persisted status is Paused.
    RetryBackoff,
}

/// Result of a `start_download` call that did not fail.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[must_use]
pub enum StartOutcome {
    /// This call spawned a new download.
    Started,
    /// Nothing done: a live handle exists (including retry backoff) or the
    /// status is already Downloading.
    AlreadyRunning,
    /// Nothing done: another operation holds the task's claim (a concurrent
    /// start, or a pause/remove/move/header edit).
    Busy,
}

impl StartOutcome {
    pub fn is_started(self) -> bool {
        matches!(self, StartOutcome::Started)
    }
}

/// Returned when a pause/remove/move could not take the task's claim within
/// the wait budget. Nothing was changed.
#[derive(Debug, thiserror::Error)]
#[error("download {0} is busy (being started or modified); retry in a moment")]
pub struct TaskBusy(pub Uuid);

/// Claim ids plus a wake-up for waiters. Keeping both behind one `Arc` keeps
/// `StartClaim::try_acquire(&manager.start_claims, id)` source-compatible.
struct ClaimSet {
    ids: Mutex<HashSet<Uuid>>,
    released: tokio::sync::Notify,
}

/// How long pause/remove/move wait for another operation's claim (normally a
/// start waiting on its beforeRequest script) before giving up. Default for
/// `download.claim_wait_ms` in settings.toml; kept in sync with it through
/// `crate::app::config::DEFAULT_CLAIM_WAIT_MS`.
const CLAIM_WAIT_BUDGET: std::time::Duration =
    std::time::Duration::from_millis(crate::app::config::DEFAULT_CLAIM_WAIT_MS);

#[derive(Clone)]
pub struct DownloadManager {
    /// Per-folder download queues
    folder_queues: Arc<RwLock<HashMap<String, FolderQueue>>>,

    http_client: Arc<HttpClient>,
    active_downloads: Arc<RwLock<HashMap<Uuid, ActiveEntry>>>,
    /// Source of `ActiveEntry::generation`. See `release_active_entry`.
    next_generation: Arc<AtomicU64>,

    // Application-wide concurrent download limit
    max_concurrent: Arc<RwLock<usize>>,
    global_semaphore: Arc<Semaphore>,

    // Per-folder concurrent download limits
    max_concurrent_per_folder: usize, // Maximum downloads per folder
    parallel_folder_count: usize,     // Maximum folders active simultaneously
    active_folders: Arc<RwLock<HashSet<String>>>,

    // Retry settings
    max_retries: u32,
    retry_delay_secs: u64,

    // Download history (completed, failed, deleted)
    history: Arc<RwLock<DownloadHistory>>,

    // Circuit breaker for failing domains
    circuit_breaker: Arc<super::circuit_breaker::CircuitBreaker>,

    // Optional push channel for lifecycle events (see `DownloadEvent`).
    event_tx: broadcast::Sender<DownloadEvent>,

    /// Ids with a `start_download` call currently in flight. See `StartClaim`.
    start_claims: Arc<ClaimSet>,
    /// How long `pause_download`/`remove_download`/`change_folder` wait for
    /// another operation's claim before giving up as `TaskBusy`. Overridden
    /// at startup from `download.claim_wait_ms` via `set_claim_wait_budget`.
    claim_wait_budget: std::time::Duration,
}

/// RAII guard for an in-flight `start_download` call. Removes its id from the
/// claim set on drop, so every return path out of `start_download` (`Ok` or
/// `Err`) releases the claim without dedicated cleanup code.
struct StartClaim {
    set: Arc<ClaimSet>,
    id: Uuid,
}

impl StartClaim {
    /// `None` if `id` is already claimed by another in-flight call. Never
    /// held across an `.await`; the lock is released as soon as the set is
    /// updated.
    fn try_acquire(set: &Arc<ClaimSet>, id: Uuid) -> Option<StartClaim> {
        let mut claims = set.ids.lock().unwrap_or_else(|e| e.into_inner());
        if claims.insert(id) {
            Some(StartClaim { set: set.clone(), id })
        } else {
            None
        }
    }
}

impl Drop for StartClaim {
    fn drop(&mut self) {
        self.set
            .ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .remove(&self.id);
        self.set.released.notify_waiters();
    }
}

impl DownloadManager {
    pub fn new() -> Self {
        // Default values: 3 app-wide, 3 per-folder, 1 active folder
        Self::with_config(3, 3, 1, 3, 5)
    }

    /// Create with full configuration
    ///
    /// # Arguments
    ///
    /// * `max_concurrent` - Application-wide max concurrent downloads (global limit)
    /// * `max_concurrent_per_folder` - Per-folder max concurrent downloads (folder limit)
    /// * `parallel_folder_count` - Max folders that can be active simultaneously (active folder limit)
    /// * `max_retries` - Maximum retry attempts per download
    /// * `retry_delay_secs` - Base retry delay in seconds (uses exponential backoff)
    ///
    /// # Constraints
    ///
    /// Must satisfy: `(folder_limit * active_folder_limit) <= global_limit`
    /// If constraint is violated, values will be adjusted to satisfy it.
    pub fn with_config(
        max_concurrent: usize,
        max_concurrent_per_folder: usize,
        parallel_folder_count: usize,
        max_retries: u32,
        retry_delay_secs: u64,
    ) -> Self {
        // Clamp limits to a sane minimum of 1. A zero limit would create a
        // `Semaphore::new(0)` whose `acquire()` never resolves, permanently
        // wedging every spawned download task.
        let max_concurrent = max_concurrent.max(1);
        let max_concurrent_per_folder = max_concurrent_per_folder.max(1);
        let parallel_folder_count = parallel_folder_count.max(1);

        // Validate and adjust constraint: (folder_limit * active_folder_limit) <= global_limit
        let (adjusted_folder_limit, adjusted_active_limit) =
            if max_concurrent_per_folder * parallel_folder_count > max_concurrent {
                tracing::warn!(
                    "Constraint violation: (per_folder: {} * active_folders: {}) = {} > global: {}. Adjusting values.",
                    max_concurrent_per_folder,
                    parallel_folder_count,
                    max_concurrent_per_folder * parallel_folder_count,
                    max_concurrent
                );
                // Prioritize parallel_folder_count, adjust max_concurrent_per_folder to fit
                let adjusted_folder_limit = max_concurrent / parallel_folder_count.max(1);
                tracing::info!(
                    "Adjusted: per_folder_limit={} (was {}), active_folders={} to satisfy constraint",
                    adjusted_folder_limit,
                    max_concurrent_per_folder,
                    parallel_folder_count
                );
                (adjusted_folder_limit, parallel_folder_count)
            } else {
                (max_concurrent_per_folder, parallel_folder_count)
            };

        Self {
            folder_queues: Arc::new(RwLock::new(HashMap::new())),
            http_client: Arc::new(HttpClient::new_or_fallback()),
            active_downloads: Arc::new(RwLock::new(HashMap::new())),
            next_generation: Arc::new(AtomicU64::new(0)),
            max_concurrent: Arc::new(RwLock::new(max_concurrent)),
            global_semaphore: Arc::new(Semaphore::new(max_concurrent)),
            max_concurrent_per_folder: adjusted_folder_limit,
            parallel_folder_count: adjusted_active_limit,
            active_folders: Arc::new(RwLock::new(HashSet::new())),
            max_retries,
            retry_delay_secs,
            history: Arc::new(RwLock::new(DownloadHistory::new())),
            circuit_breaker: Arc::new(super::circuit_breaker::CircuitBreaker::new()),
            event_tx: broadcast::channel(EVENT_CHANNEL_CAPACITY).0,
            start_claims: Arc::new(ClaimSet {
                ids: Mutex::new(HashSet::new()),
                released: tokio::sync::Notify::new(),
            }),
            claim_wait_budget: CLAIM_WAIT_BUDGET,
        }
    }

    pub fn with_max_concurrent(max_concurrent: usize) -> Self {
        Self::with_config(max_concurrent, max_concurrent, 1, 3, 5)
    }

    pub fn with_retry_settings(max_retries: u32, retry_delay_secs: u64) -> Self {
        Self::with_config(3, 3, 1, max_retries, retry_delay_secs)
    }

    // ========== Event Stream ==========

    /// Subscribe to the lifecycle event stream (see [`DownloadEvent`]).
    ///
    /// Each subscriber gets its own receiver. Events are best-effort: if a
    /// receiver falls behind by more than the channel capacity it will observe
    /// `RecvError::Lagged(n)` and should re-sync via `get_all_downloads()`.
    pub fn subscribe(&self) -> broadcast::Receiver<DownloadEvent> {
        self.event_tx.subscribe()
    }

    /// Broadcast a lifecycle event. Send errors (no active subscribers) are
    /// expected and intentionally ignored — the event stream is optional.
    fn emit(&self, event: DownloadEvent) {
        let _ = self.event_tx.send(event);
    }

    // ========== Folder Queue Management ==========

    /// Get or create a folder queue
    async fn get_or_create_folder_queue(&self, folder_id: &str) -> FolderQueue {
        let mut queues = self.folder_queues.write().await;
        queues
            .entry(folder_id.to_string())
            .or_insert_with(|| FolderQueue::new(folder_id, self.max_concurrent_per_folder))
            .clone()
    }

    /// Get folder queue if it exists
    async fn get_folder_queue(&self, folder_id: &str) -> Option<FolderQueue> {
        let queues = self.folder_queues.read().await;
        queues.get(folder_id).cloned()
    }

    /// Check if a folder has active tasks (O(1) operation)
    async fn folder_has_active_tasks(&self, folder_id: &str) -> bool {
        if let Some(queue) = self.get_folder_queue(folder_id).await {
            queue.has_active_tasks().await
        } else {
            false
        }
    }

    // ========== Download Operations ==========

    pub async fn add_download(&self, mut task: DownloadTask) {
        // Sanitize filename
        task.filename = sanitize_filename(&task.filename);
        let task_id = task.id;
        let folder_id = task.folder_id.clone();
        let queue = self.get_or_create_folder_queue(&folder_id).await;
        queue.add(task).await;
        self.emit(DownloadEvent::Added { task_id, folder_id });
    }

    /// Add a download, checking for duplicate URLs in the folder queue.
    /// Returns `true` if the task was added, `false` if a duplicate URL was found.
    pub async fn add_download_if_unique(&self, mut task: DownloadTask) -> bool {
        task.filename = sanitize_filename(&task.filename);
        let folder_id = task.folder_id.clone();
        let queue = self.get_or_create_folder_queue(&folder_id).await;

        if queue.contains_url(&task.url).await {
            tracing::warn!(
                "Duplicate URL rejected in folder '{}': {}",
                folder_id,
                task.url
            );
            return false;
        }

        let task_id = task.id;
        queue.add(task).await;
        self.emit(DownloadEvent::Added { task_id, folder_id });
        true
    }

    /// Get all downloads from all folder queues
    pub async fn get_all_downloads(&self) -> Vec<DownloadTask> {
        let queues = self.folder_queues.read().await;
        let mut all_tasks = Vec::new();
        for queue in queues.values() {
            all_tasks.extend(queue.get_all().await);
        }
        all_tasks
    }

    /// Get all downloads for a specific folder
    pub async fn get_folder_downloads(&self, folder_id: &str) -> Vec<DownloadTask> {
        if let Some(queue) = self.get_folder_queue(folder_id).await {
            queue.get_all().await
        } else {
            Vec::new()
        }
    }

    /// Removes `id`: stops its live handle (downloading or retry backoff)
    /// and removes it from its folder queue. A caller already holding the
    /// claim must call this, not `remove_download`, which would wait on its
    /// own claim. A missing task returns `None`.
    async fn remove_claimed(&self, id: Uuid, _claim: &StartClaim) -> Option<DownloadTask> {
        self.stop_handle(id).await;

        // Snapshot the folder queues (cheap Arc clones) and drop the outer lock
        // before awaiting on individual queues, so a concurrent writer to
        // `folder_queues` can't be blocked behind this read guard.
        let queues: Vec<FolderQueue> = {
            let guard = self.folder_queues.read().await;
            guard.values().cloned().collect()
        };
        let mut removed = None;
        for queue in queues {
            if let Some(task) = queue.remove(id).await {
                removed = Some(task);
                break;
            }
        }

        if let Some(task) = &removed {
            // queue.remove() already adjusted the per-folder count; release the
            // folder activation slot if the folder is now empty.
            self.deactivate_folder_if_empty(&task.folder_id).await;
            self.emit(DownloadEvent::Removed { task_id: id });
        }
        removed
    }

    /// Waits up to the claim-wait budget for another operation holding
    /// `id`'s claim (typically a start still running its beforeRequest
    /// script) before giving up with `TaskBusy`, in which case nothing is
    /// changed. Otherwise removes the task as `remove_claimed` does; see
    /// that method for what removal itself covers.
    pub async fn remove_download(&self, id: Uuid) -> Result<Option<DownloadTask>, TaskBusy> {
        let claim = self.acquire_claim_waiting(id, self.claim_wait_budget).await?;
        Ok(self.remove_claimed(id, &claim).await)
    }

    /// Batch version of `remove_download`: removes every id concurrently
    /// instead of paying the claim-wait budget once per id, for the same
    /// reason `pause_in_flight` does (see its comment). Results are in `ids`
    /// order.
    pub async fn remove_many(&self, ids: &[Uuid]) -> Vec<Result<Option<DownloadTask>, TaskBusy>> {
        futures_util::future::join_all(ids.iter().map(|&id| self.remove_download(id))).await
    }

    /// Starts a download for `id`. Concurrent calls for the same id collapse
    /// into a single start: only the first caller to claim `id` proceeds, and
    /// the rest return `Ok(Busy)` immediately without touching the task. A
    /// call for a task whose download handle is still running (for example
    /// during retry backoff, where the status is `Paused` but a handle is
    /// still alive) is `Ok(AlreadyRunning)`.
    pub async fn start_download(
        &self,
        id: Uuid,
        script_sender: Option<mpsc::Sender<ScriptRequest>>,
        config: Arc<tokio::sync::RwLock<crate::app::config::Config>>,
    ) -> Result<StartOutcome> {
        let Some(_claim) = StartClaim::try_acquire(&self.start_claims, id) else {
            return Ok(StartOutcome::Busy);
        };

        if self.is_running(id).await {
            return Ok(StartOutcome::AlreadyRunning);
        }

        let mut task = self.get_by_id(id).await
            .ok_or_else(|| anyhow::anyhow!("Task not found"))?;

        if task.status == DownloadStatus::Downloading {
            return Ok(StartOutcome::AlreadyRunning);
        }

        // Check circuit breaker for the domain
        if let Some(domain) = super::circuit_breaker::extract_domain(&task.url) {
            use super::circuit_breaker::CircuitState;
            match self.circuit_breaker.can_request(&domain) {
                CircuitState::Open => {
                    return Err(anyhow::anyhow!(
                        "Circuit breaker open for domain '{}'. Too many consecutive failures.",
                        domain
                    ));
                }
                CircuitState::HalfOpen => {
                    tracing::info!("Testing recovery for domain '{}'", domain);
                }
                CircuitState::Closed => {}
            }
        }

        // Try to activate folder (check active folder limit)
        let folder_id = task.folder_id.clone();
        if !self.try_activate_folder(&folder_id).await {
            return Err(anyhow::anyhow!(
                "Cannot start download: folder '{}' cannot be activated ({} folders already active, max active folders: {})",
                folder_id,
                self.active_folders.read().await.len(),
                self.parallel_folder_count
            ));
        }

        // Get folder queue and its semaphore
        let folder_queue = self.get_or_create_folder_queue(&folder_id).await;
        let folder_semaphore = folder_queue.semaphore();

        // Hook Point 1: beforeRequest - Modify URL, headers, user-agent before HTTP request
        // Execute via message passing BEFORE spawning download task
        if let Some(ref sender) = script_sender {
            // Compute effective script_files (Application + Folder override)
            let effective_script_files = Self::compute_effective_script_files(&config, &task.folder_id).await;

            let ctx = BeforeRequestContext {
                url: task.url.clone(),
                headers: task.headers.clone(),
                user_agent: task.user_agent.clone(),
                download_id: Some(task.id.to_string()),
            };

            // Send request and await response
            match sender::send_script_request_with_context(sender, move |response_tx| {
                ScriptRequest::BeforeRequest {
                    ctx,
                    effective_script_files,
                    response: response_tx,
                }
            }).await {
                Ok((modified_ctx, Ok(()))) => {
                    // Apply modifications from script
                    task.url = modified_ctx.url;
                    task.headers = modified_ctx.headers;
                    task.user_agent = modified_ctx.user_agent;
                    task.log_info("beforeRequest hook executed".to_string());
                }
                Ok((_, Err(e))) => {
                    tracing::error!("beforeRequest hook error: {}", e);
                }
                Err(e) => {
                    tracing::error!("beforeRequest error: {}", e);
                }
            }
        }

        // Update folder task counts based on previous status
        let previous_status = task.status;
        task.status = DownloadStatus::Downloading;
        task.started_at = Some(chrono::Utc::now());
        task.error_message = None; // Clear any previous error
        task.log_info(format!("Starting download: {}", task.url));
        folder_queue.update(task.clone()).await;

        // Update counts: transition from Pending/Paused to Downloading
        // Note: FolderQueue.update() handles count updates internally
        match previous_status {
            DownloadStatus::Pending | DownloadStatus::Paused | DownloadStatus::Error => {
                // Status transition handled by folder_queue.update()
            }
            _ => {}
        }

        // Resume only for interrupted tasks (Paused/Error), not for new downloads
        let is_resuming = matches!(previous_status, DownloadStatus::Paused | DownloadStatus::Error);

        // Clone folder queue for the spawned task
        let queue = folder_queue.clone();
        let http_client = self.http_client.clone();
        let global_semaphore = self.global_semaphore.clone();
        let script_sender_for_error = script_sender.clone();
        let max_retries = self.max_retries;
        let retry_delay_secs = self.retry_delay_secs;
        let manager_for_cleanup = self.clone();
        let circuit_breaker = self.circuit_breaker.clone();
        let history = self.history.clone();
        let task_url = task.url.clone();

        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        let in_backoff = Arc::new(AtomicBool::new(false));
        // Clone before `in_backoff` is moved into `body`; the entry keeps this one.
        let entry_in_backoff = in_backoff.clone();
        let body = async move {
            // Acquire both global and folder semaphore permits. The semaphores
            // are never closed in normal operation, but handle the error path
            // gracefully instead of panicking the download task (which would
            // leave the task stuck `Downloading` and leak its folder slot).
            let _global_permit = match global_semaphore.acquire().await {
                Ok(permit) => permit,
                Err(_) => {
                    tracing::error!("Global semaphore closed; aborting download '{}'", task.filename);
                    manager_for_cleanup.abort_unstarted(&queue, task, &folder_id).await;
                    return;
                }
            };
            let _folder_permit = match folder_semaphore.acquire().await {
                Ok(permit) => permit,
                Err(_) => {
                    tracing::error!("Folder semaphore closed; aborting download '{}'", task.filename);
                    manager_for_cleanup.abort_unstarted(&queue, task, &folder_id).await;
                    return;
                }
            };

            tracing::debug!(
                "Acquired slots for '{}' (folder: {})",
                task.filename,
                folder_id
            );

            let mut current_task = task.clone();

            // Retry loop
            loop {
                // Clone Arc-wrapped types (cheap) and task for retry attempt
                match Self::download_task(current_task.clone(), http_client.clone(), queue.clone(), script_sender.clone(), config.clone(), is_resuming, manager_for_cleanup.event_tx.clone()).await {
                    Ok(completed_task) => {
                        // Add completed task to history for display in CompletedNode
                        history.write().await.add(completed_task);

                        // Download succeeded - record success for circuit breaker
                        if let Some(domain) = super::circuit_breaker::extract_domain(&task_url) {
                            circuit_breaker.record_success(&domain);
                        }
                        manager_for_cleanup.emit(DownloadEvent::Completed { task_id: id });
                        break;
                    }
                    Err(e) => {
                        tracing::error!("Download failed for {}: {}", current_task.filename, e);
                        current_task.error_message = Some(e.to_string());
                        current_task.retry_count += 1;
                        current_task.log_error(format!("Download failed (attempt {}): {}", current_task.retry_count, e));

                        // Check if we should retry
                        if current_task.retry_count < max_retries {
                            // Calculate exponential backoff delay: base_delay * 2^(retry_count - 1)
                            // Cap the exponent so a large retry_count can't overflow the shift.
                            let exponent = current_task.retry_count.saturating_sub(1).min(16);
                            let backoff_delay = retry_delay_secs.saturating_mul(2_u64.pow(exponent));
                            tracing::info!(
                                "Retrying download {} in {} seconds (attempt {}/{})",
                                current_task.filename,
                                backoff_delay,
                                current_task.retry_count + 1,
                                max_retries
                            );
                            current_task.status = DownloadStatus::Paused;
                            current_task.log_info(format!("Retrying in {} seconds...", backoff_delay));
                            // Flag after the write, so a reader that sees backoff
                            // also sees this attempt's retry_count.
                            queue.update(current_task.clone()).await;
                            in_backoff.store(true, Ordering::Relaxed);

                            // Wait before retry with exponential backoff
                            tokio::time::sleep(tokio::time::Duration::from_secs(backoff_delay)).await;

                            // Prepare for retry
                            current_task.status = DownloadStatus::Downloading;
                            current_task.error_message = None;
                            queue.update(current_task.clone()).await;
                            in_backoff.store(false, Ordering::Relaxed);
                        } else {
                            // Max retries exceeded, mark as error
                            current_task.status = DownloadStatus::Error;
                            current_task.log_error(format!("Max retries ({}) exceeded", max_retries));

                            // Hook Point 4: error - fire-and-forget, but queued before
                            // the queue write and the Failed event, so a `wait`
                            // unblocked by either one, or a standalone process exiting
                            // right after them, still has the request in the channel
                            // by the time it flushes. The send is inline (not
                            // spawn_blocking): this is an unbounded std channel, so
                            // send() never blocks.
                            if let Some(ref sender) = script_sender_for_error {
                                let effective_script_files = Self::compute_effective_script_files(&config, &current_task.folder_id).await;

                                let ctx = crate::script::events::ErrorContext {
                                    url: current_task.url.clone(),
                                    filename: Some(current_task.filename.clone()),
                                    error: current_task.error_message.as_deref().unwrap_or("Unknown error").to_string(),
                                    retry_count: current_task.retry_count,
                                    status_code: current_task.last_status_code,
                                };

                                if let Err(e) = sender.send(ScriptRequest::Error {
                                    ctx,
                                    effective_script_files,
                                }) {
                                    tracing::error!("Failed to send error hook: {}", e);
                                }
                            }

                            queue.update(current_task.clone()).await;
                            manager_for_cleanup.emit(DownloadEvent::Failed {
                                task_id: id,
                                error: current_task.error_message.clone().unwrap_or_default(),
                            });

                            // Record failure for circuit breaker
                            if let Some(domain) = super::circuit_breaker::extract_domain(&task_url) {
                                circuit_breaker.record_failure(&domain);
                            }

                            break;
                        }
                    }
                }
            }

            // Cleanup: the terminal queue operation already adjusted the count
            // (download_task removes the task on success, or marks it Error on
            // failure), so only the folder slot needs releasing here. Calling
            // decrement_downloading here would double-count and wrongly free a
            // sibling download's slot when several run in the same folder.
            manager_for_cleanup.deactivate_folder_if_empty(&folder_id).await;
        };

        // Insert happens while holding the write lock, so the spawned body's
        // release (which takes the same lock) cannot run before the entry
        // exists, even if the body finishes immediately.
        let releaser = self.clone();
        let mut active = self.active_downloads.write().await;
        let handle = tokio::spawn(async move {
            body.await;
            releaser.release_active_entry(id, generation).await;
        });
        active.insert(id, ActiveEntry { handle, generation, in_backoff: entry_in_backoff });
        drop(active);

        self.emit(DownloadEvent::Started { task_id: id });

        Ok(StartOutcome::Started)
    }

    /// Encode Basic authentication credentials
    fn encode_basic_auth(username: &str, password: &str) -> String {
        use base64::{Engine as _, engine::general_purpose::STANDARD};
        let credentials = format!("{}:{}", username, password);
        format!("Basic {}", STANDARD.encode(credentials.as_bytes()))
    }

    /// Compute effective script files by merging application-level and folder-level settings
    ///
    /// Folder-level settings override application-level settings for the same script file.
    async fn compute_effective_script_files(
        config: &tokio::sync::RwLock<crate::app::config::Config>,
        folder_id: &str,
    ) -> HashMap<String, bool> {
        let cfg = config.read().await;
        let mut script_files = cfg.scripts.script_files.clone();

        // Apply folder override if present
        if let Some(folder_cfg) = cfg.folders.get(folder_id) {
            if let Some(ref folder_script_files) = folder_cfg.script_files {
                // Clone is necessary here as we're borrowing from the config RwLock
                for (filename, enabled) in folder_script_files {
                    script_files.insert(filename.clone(), *enabled);
                }
            }
        }

        script_files
    }

    async fn download_task(
        mut task: DownloadTask,
        http_client: Arc<HttpClient>,
        queue: FolderQueue,
        script_sender: Option<mpsc::Sender<ScriptRequest>>,
        config: Arc<tokio::sync::RwLock<crate::app::config::Config>>,
        is_resuming: bool,
        event_tx: broadcast::Sender<DownloadEvent>,
    ) -> Result<DownloadTask> {
        // Compute effective script_files (Application + Folder override)
        let effective_script_files = Self::compute_effective_script_files(&config, &task.folder_id).await;

        // Resolve referrer from policy (folder > app), unless task.headers already has one
        let has_task_referer = task.headers.keys().any(|k| k.eq_ignore_ascii_case("referer"));
        let policy_referer = if has_task_referer {
            None
        } else {
            let cfg = config.read().await;
            let policy = cfg.folders.get(&task.folder_id)
                .and_then(|f| f.referrer_policy.clone())
                .unwrap_or_else(|| cfg.download.referrer_policy.clone());
            policy.compute(&task.url)
        };

        // Build headers
        let headers = HttpClient::build_headers(
            task.user_agent.as_deref(),
            policy_referer.as_deref(),
            &task.headers,
        )?;

        // Get download info
        let mut info = http_client.get_info(&task.url, &headers).await?;

        // Update task with server info
        task.size = info.size;
        task.resume_supported = info.resume_supported;
        task.etag = info.etag.clone();
        task.last_modified = info.last_modified.clone();
        task.last_status_code = Some(info.status);

        // Log server info
        let size_str = info.size.map(|s| format!("{} bytes", s)).unwrap_or("unknown".to_string());
        task.log_info(format!("Server info: size={}, resume={}", size_str, info.resume_supported));

        // Use filename from Content-Disposition if available (highest priority)
        if let Some(server_filename) = info.filename {
            task.filename = sanitize_filename(&server_filename);
            task.log_info(format!("Filename from server: {}", task.filename));
        } else if let Some(ref final_url) = info.final_url {
            // Fallback: extract filename from redirect destination URL
            if final_url != &task.url {
                let redirect_filename = final_url
                    .split('/')
                    .last()
                    .unwrap_or("")
                    .split('?')
                    .next()
                    .unwrap_or("");
                if !redirect_filename.is_empty() {
                    let sanitized = sanitize_filename(redirect_filename);
                    task.log_info(format!("Filename from redirect: {} -> {}", task.filename, sanitized));
                    task.filename = sanitized;
                }
            }
        }

        queue.update(task.clone()).await;

        // Hook Point: authRequired - Handle authentication if needed
        if info.auth_required {
            task.log_info(format!("Authentication required (HTTP {}): realm={}",
                info.status,
                info.auth_realm.as_deref().unwrap_or("unknown")));

            if let Some(ref sender) = script_sender {
                // Trust boundary note: scripts loaded from the user-controlled
                // scripts directory (see ScriptLoader, which skips symlinks and
                // caps size) may supply credentials for a download. We never
                // surface *existing* credentials to scripts — username/password
                // are sent as None — so a script can only inject, not read,
                // credentials. Gating injection per-script would require a
                // permissions config; deferred intentionally.
                let ctx = crate::script::events::AuthRequiredContext {
                    url: task.url.clone(),
                    realm: info.auth_realm.clone(),
                    username: None,
                    password: None,
                };

                let effective_files = effective_script_files.clone();

                // Send request and await response
                match sender::send_script_request_with_context(sender, move |response_tx| {
                    ScriptRequest::AuthRequired {
                        ctx,
                        effective_script_files: effective_files,
                        response: response_tx,
                    }
                }).await {
                    Ok((modified_ctx, Ok(()))) => {
                        // Check if script provided credentials
                        if let (Some(username), Some(password)) =
                            (modified_ctx.username, modified_ctx.password)
                        {
                            // Encode and add Authorization header
                            let auth_header = Self::encode_basic_auth(&username, &password);
                            task.headers.insert("Authorization".to_string(), auth_header);
                            task.log_info("Authentication credentials provided by script".to_string());

                            // Retry get_info with auth
                            let headers = HttpClient::build_headers(
                                task.user_agent.as_deref(),
                                policy_referer.as_deref(),
                                &task.headers,
                            )?;

                            let retry_info = http_client.get_info(&task.url, &headers).await?;

                            if retry_info.auth_required {
                                task.log_error("Authentication failed (credentials rejected)".to_string());
                                return Err(anyhow::anyhow!("Authentication failed: Invalid credentials"));
                            }

                            // Success! Update task with new info
                            task.size = retry_info.size;
                            task.resume_supported = retry_info.resume_supported;
                            task.etag = retry_info.etag.clone();
                            task.last_modified = retry_info.last_modified.clone();
                            task.last_status_code = Some(retry_info.status);
                            task.log_info("Authentication successful".to_string());

                            // Update info for downstream code
                            info = retry_info;
                        } else {
                            task.log_warn("authRequired hook executed but no credentials provided".to_string());
                            return Err(anyhow::anyhow!(
                                "Authentication required (HTTP {}) but no credentials provided",
                                info.status
                            ));
                        }
                    }
                    Ok((_, Err(e))) => {
                        tracing::error!("authRequired hook error: {}", e);
                        return Err(anyhow::anyhow!("authRequired hook failed: {}", e));
                    }
                    Err(e) => {
                        tracing::error!("authRequired error: {}", e);
                        return Err(anyhow::anyhow!("authRequired error: {}", e));
                    }
                }
            } else {
                // No script sender - cannot handle auth
                task.log_error("Authentication required but scripting is disabled".to_string());
                return Err(anyhow::anyhow!(
                    "Authentication required (HTTP {}) but scripting is disabled",
                    info.status
                ));
            }

            queue.update(task.clone()).await;
        }

        // Hook Point 2: headersReceived - Inspect server response
        if let Some(ref sender) = script_sender {
            let ctx = crate::script::events::HeadersReceivedContext {
                url: task.url.clone(),
                status: info.status,
                headers: info.headers.clone(),
                content_length: info.size,
                etag: info.etag.clone(),
                last_modified: info.last_modified.clone(),
                content_type: info.content_type.clone(),
            };

            let effective_files = effective_script_files.clone();

            // Send request and await response
            match sender::send_script_request_no_context(sender, move |response_tx| {
                ScriptRequest::HeadersReceived {
                    ctx,
                    effective_script_files: effective_files,
                    response: response_tx,
                }
            }).await {
                Ok(Ok(())) => {
                    task.log_info("headersReceived hook executed".to_string());
                }
                Ok(Err(e)) => {
                    tracing::error!("headersReceived hook error: {}", e);
                }
                Err(e) => {
                    tracing::error!("headersReceived error: {}", e);
                }
            }
        }

        // Resolve settings (applies auto-date directory, etc.)
        let resolved_save_path = {
            let cfg = config.read().await;
            crate::app::settings::ResolvedSettings::resolve(&cfg, &task.folder_id, &task)
                .save_path
        };
        // Ensure directory exists (handles auto-date subdirectories)
        tokio::fs::create_dir_all(&resolved_save_path).await?;

        // Resume: only for interrupted tasks (Paused/Error) with an existing
        // partial file. A single async `metadata` call avoids the exists()+
        // metadata() TOCTOU and the blocking std::fs call on the runtime;
        // any error (missing/unreadable) means "start fresh".
        let mut file_path = resolved_save_path.join(&task.filename);
        let resume_from = if is_resuming && task.resume_supported {
            match tokio::fs::metadata(&file_path).await {
                Ok(meta) => Some(meta.len()),
                Err(_) => None,
            }
        } else {
            None
        };

        if let Some(offset) = resume_from {
            task.downloaded = offset;
            task.log_info(format!("Resuming download from {} bytes", offset));
            queue.update(task.clone()).await;
        } else {
            // New download: ensure unique filename to avoid overwriting existing files
            let unique_name = crate::file::naming::ensure_unique_filename(
                &resolved_save_path, &task.filename,
            );
            if unique_name != task.filename {
                task.log_info(format!("Filename conflict resolved: {} -> {}", task.filename, unique_name));
                task.filename = unique_name;
                file_path = resolved_save_path.join(&task.filename);
                queue.update(task.clone()).await;
            }
            task.log_info("Starting fresh download".to_string());
        }

        // Progress is delivered over a bounded channel to a single consumer
        // task, rather than spawning a task per update. The sync callback uses
        // `try_send`, so when the consumer is behind the update is dropped and
        // superseded by the next one (natural coalescing) — and the terminal
        // 100% update is never silently throttled away. The script progress
        // hook is rate-limited inside the consumer.
        let task_id = task.id;
        let task_url = task.url.clone();
        let queue_for_progress = queue.clone();
        let start_time = std::time::Instant::now();
        let script_sender_for_progress = script_sender.clone();
        let effective_script_files_for_progress = effective_script_files.clone();
        let event_tx_for_progress = event_tx.clone();

        let (prog_tx, mut prog_rx) = tokio::sync::mpsc::channel::<(u64, Option<u64>)>(4);

        let progress_consumer = tokio::spawn(async move {
            let mut last_script_ms: u64 = 0;
            let mut last_event_ms: u64 = 0;
            while let Some((downloaded, total)) = prog_rx.recv().await {
                let Some(mut task) = queue_for_progress.get_by_id(task_id).await else {
                    continue;
                };
                task.downloaded = downloaded;
                task.size = total.or(task.size);

                // Push a throttled progress event for subscribers (e.g. a GUI).
                // Independent from the script-hook throttle below so the two can
                // evolve separately; ~200ms keeps a GUI smooth without flooding.
                let now_ms = start_time.elapsed().as_millis() as u64;
                if now_ms.saturating_sub(last_event_ms) >= 200 {
                    last_event_ms = now_ms;
                    let elapsed = start_time.elapsed().as_secs_f64();
                    let speed = if elapsed > 0.0 { downloaded as f64 / elapsed } else { 0.0 };
                    let _ = event_tx_for_progress.send(DownloadEvent::Progress {
                        task_id,
                        downloaded,
                        total: task.size,
                        speed,
                    });
                }

                // Hook Point 5: progress - fire-and-forget, throttled to ~2/sec
                if let Some(ref sender) = script_sender_for_progress {
                    let elapsed_ms = start_time.elapsed().as_millis() as u64;
                    if elapsed_ms.saturating_sub(last_script_ms) >= 500 {
                        last_script_ms = elapsed_ms;
                        let elapsed = start_time.elapsed().as_secs_f64();
                        let speed_value = if elapsed > 0.0 { downloaded as f64 / elapsed } else { 0.0 };
                        let ctx = crate::script::events::ProgressContext {
                            url: task_url.clone(),
                            filename: task.filename.clone(),
                            downloaded,
                            total,
                            speed: Some(speed_value),
                            percentage: None, // Calculated by script engine
                        };
                        let sender_clone = (*sender).clone();
                        let effective_files = effective_script_files_for_progress.clone();
                        tokio::task::spawn_blocking(move || {
                            if let Err(e) = sender_clone.send(ScriptRequest::Progress {
                                ctx,
                                effective_script_files: effective_files,
                            }) {
                                tracing::error!("Failed to send progress hook: {}", e);
                            }
                        });
                    }
                }

                queue_for_progress.update(task).await;
            }
        });

        let progress_callback = move |downloaded: u64, total: Option<u64>| {
            // Non-blocking: drop if the consumer is behind; the next update wins.
            let _ = prog_tx.try_send((downloaded, total));
        };

        // Rebuild headers to include any auth header from authRequired hook
        let headers = HttpClient::build_headers(
            task.user_agent.as_deref(),
            policy_referer.as_deref(),
            &task.headers,
        )?;

        // Perform download
        let download_info = http_client
            .download_to_file(
                &task.url,
                &file_path,
                &headers,
                resume_from,
                Some(progress_callback),
            )
            .await?;

        // The callback (and its sender) is now dropped; drain the remaining
        // progress updates so the final value is applied before finalizing.
        let _ = progress_consumer.await;

        // Apply last modified time if available
        if let Some(ref last_modified) = download_info.last_modified {
            let _ = apply_last_modified(&file_path, Some(last_modified));
        }

        // Hook Point 3: completed - File operations after download
        if let Some(ref sender) = script_sender {
            // Calculate download duration
            let duration = task.started_at.map(|start| {
                let end = chrono::Utc::now();
                (end - start).num_milliseconds() as f64 / 1000.0
            });

            let ctx = crate::script::events::CompletedContext {
                url: task.url.clone(),
                filename: task.filename.clone(),
                save_path: task.save_path.to_string_lossy().to_string(),
                new_filename: None,
                move_to_path: None,
                size: task.size.unwrap_or(0),
                duration,
            };

            let effective_files = effective_script_files.clone();
            let file_path_for_ops = file_path.clone();

            // Send request and await response
            match sender::send_script_request_with_context(sender, move |response_tx| {
                ScriptRequest::Completed {
                    ctx,
                    effective_script_files: effective_files,
                    response: response_tx,
                }
            }).await {
                Ok((modified_ctx, Ok(()))) => {
                    let file_dir = file_path_for_ops.parent()
                        .unwrap_or(&task.save_path)
                        .to_path_buf();

                    // Apply file rename if script set newFilename. Scripts are
                    // untrusted, so sanitize the name to a bare filename:
                    // sanitize_filename strips path separators and other unsafe
                    // characters, preventing the rename from escaping file_dir.
                    if let Some(new_name) = modified_ctx.new_filename {
                        let safe_name = sanitize_filename(&new_name);
                        if safe_name.is_empty() {
                            task.log_warn(format!(
                                "Script newFilename '{}' rejected (empty after sanitization)",
                                new_name
                            ));
                        } else {
                            // Check for collision with existing files before renaming
                            let final_name =
                                crate::file::naming::ensure_unique_filename(&file_dir, &safe_name);
                            let new_path = file_dir.join(&final_name);
                            tracing::debug!(
                                from = ?file_path_for_ops,
                                to = ?new_path,
                                "Renaming file by script"
                            );
                            if let Err(e) = std::fs::rename(&file_path_for_ops, &new_path) {
                                tracing::error!(
                                    from = ?file_path_for_ops,
                                    to = ?new_path,
                                    "Failed to rename file: {}", e
                                );
                            } else {
                                task.filename = final_name;
                                task.log_info("File renamed by script".to_string());
                            }
                        }
                    }

                    // Apply file move if script set moveToPath. Reject paths
                    // containing `..` components so an untrusted script cannot
                    // traverse out to arbitrary locations.
                    if let Some(new_dir_str) = modified_ctx.move_to_path {
                        let new_dir = std::path::PathBuf::from(&new_dir_str);
                        let has_parent_traversal = new_dir
                            .components()
                            .any(|c| matches!(c, std::path::Component::ParentDir));
                        if has_parent_traversal {
                            task.log_warn(format!(
                                "Script moveToPath '{}' rejected (contains '..')",
                                new_dir_str
                            ));
                        } else {
                            let current_path = file_dir.join(&task.filename);
                            let new_path = new_dir.join(&task.filename);
                            if let Err(e) = std::fs::rename(&current_path, &new_path) {
                                tracing::error!("Failed to move file: {}", e);
                            } else {
                                task.save_path = new_dir;
                                task.log_info("File moved by script".to_string());
                            }
                        }
                    }
                    task.log_info("completed hook executed".to_string());
                }
                Ok((_, Err(e))) => {
                    tracing::error!("completed hook error: {}", e);
                }
                Err(e) => {
                    tracing::error!("completed error: {}", e);
                }
            }
        }

        // Mark as completed. Use the actual bytes written rather than the
        // advertised size, so the record stays correct when the size was
        // unknown or the server rejected the Range and restarted from 0.
        task.status = DownloadStatus::Completed;
        task.completed_at = Some(chrono::Utc::now());
        task.downloaded = download_info.bytes_written;
        if task.size.is_none() {
            task.size = Some(download_info.bytes_written);
        }
        task.log_info(format!("Download completed successfully: {}", task.filename));

        // Append to completion log
        if let Err(e) = crate::download::completion_log::append_completion(&task).await {
            tracing::error!("Failed to append completion log: {}", e);
            // Continue anyway - don't fail download on log error
        }

        // Remove from queue (completed tasks are logged to completion log)
        queue.remove(task.id).await;

        // Persist queue to disk immediately so completed tasks are not
        // resurrected if the process exits before the next save.
        if let Err(e) = queue.save().await {
            tracing::error!("Failed to persist queue after completion: {}", e);
        }

        tracing::debug!("Download completed and logged: {}", task.filename);

        Ok(task)
    }

    /// Takes `id`'s claim, waiting up to `budget` for the current holder to
    /// release it (a start's beforeRequest script, or another pause/remove/
    /// move/header edit). `Err(TaskBusy)` if `budget` elapses first; nothing
    /// is changed in that case.
    async fn acquire_claim_waiting(
        &self,
        id: Uuid,
        budget: std::time::Duration,
    ) -> Result<StartClaim, TaskBusy> {
        let deadline = tokio::time::Instant::now() + budget;
        loop {
            let notified = self.start_claims.released.notified();
            tokio::pin!(notified);
            // Register before checking, so a release between the check and
            // the await is not missed.
            notified.as_mut().enable();
            if let Some(claim) = StartClaim::try_acquire(&self.start_claims, id) {
                return Ok(claim);
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return Err(TaskBusy(id));
            }
        }
    }

    /// Overrides how long `pause_download`/`remove_download`/`change_folder`
    /// wait for another operation's claim (default `CLAIM_WAIT_BUDGET`).
    /// Applied once at startup from `download.claim_wait_ms`; tests use a
    /// short budget instead of waiting out the real default.
    pub fn set_claim_wait_budget(&mut self, budget: std::time::Duration) {
        self.claim_wait_budget = budget;
    }

    /// Removes and stops `id`'s download, waiting for it to actually end so
    /// no late queue write from it can land after the caller's own write.
    /// Returns whether a live entry was actually found (and stopped).
    async fn stop_handle(&self, id: Uuid) -> bool {
        let entry = self.active_downloads.write().await.remove(&id); // lock dropped here
        let found = entry.is_some();
        if let Some(entry) = entry {
            entry.handle.abort();
            // Bounded: the completion path has a synchronous file rename.
            if tokio::time::timeout(std::time::Duration::from_secs(5), entry.handle)
                .await
                .is_err()
            {
                tracing::warn!("download {id} did not stop within 5s after abort");
            }
        }
        found
    }

    /// Marks `id` `Paused` and updates counts/events. A missing task is a
    /// no-op.
    async fn write_paused_status(&self, id: Uuid) {
        if let Some(mut task) = self.get_by_id(id).await {
            let folder_id = task.folder_id.clone();
            task.status = DownloadStatus::Paused;
            if let Some(queue) = self.get_folder_queue(&folder_id).await {
                // `update()` owns the count: a Downloading/Pending -> Paused
                // transition decrements the corresponding count exactly once.
                queue.update(task).await;
            }
            // Release the folder activation slot if nothing else is active.
            self.deactivate_folder_if_empty(&folder_id).await;
            self.emit(DownloadEvent::Paused { task_id: id });
        }
    }

    /// Stops `id`'s live handle (including retry backoff) and marks it
    /// `Paused`. A caller already holding the claim must call this, not
    /// `pause_download`, which would wait on its own claim. A missing task
    /// is a no-op.
    async fn pause_claimed(&self, id: Uuid, _claim: &StartClaim) {
        self.stop_handle(id).await;
        self.write_paused_status(id).await;
    }

    /// Pauses `id`: stops its live handle (downloading or retry backoff) and
    /// marks it `Paused`. Waits up to the claim-wait budget for another
    /// operation holding `id`'s claim (typically a start still running its
    /// beforeRequest script) before giving up with `TaskBusy`, in which case
    /// nothing is changed. A missing task is `Ok` -- there is nothing to
    /// pause.
    pub async fn pause_download(&self, id: Uuid) -> Result<(), TaskBusy> {
        let claim = self.acquire_claim_waiting(id, self.claim_wait_budget).await?;
        self.pause_claimed(id, &claim).await;
        Ok(())
    }

    /// Batch-pause version of `pause_download`/`pause_claimed`: takes `id`'s
    /// claim, then re-checks whether the task is still in flight if no live
    /// handle was found before touching it. A candidate can leave the
    /// in-flight set between the batch's snapshot and this call taking the
    /// claim -- for example a backoff task whose retries were just
    /// exhausted and became `Error` -- and without the re-check this would
    /// overwrite that terminal status with `Paused`. Returns `Ok(true)` if
    /// the task was paused, `Ok(false)` if it was skipped as no longer in
    /// flight.
    async fn pause_if_in_flight(&self, id: Uuid) -> Result<bool, TaskBusy> {
        let claim = self.acquire_claim_waiting(id, self.claim_wait_budget).await?;
        let had_handle = self.stop_handle(id).await;
        // Can't use `task_in_flight`/`run_state` here: this call itself holds
        // `id`'s only claim, so `is_start_in_progress` would always report
        // `Claimed`. Decide from the persisted status instead. A handle can
        // still be found after its body wrote a final `Error` and before it
        // released its entry, so a handle alone doesn't make the task
        // pausable: only `Downloading`, or `Paused` with a handle (retry
        // backoff), is.
        let pausable = match self.get_by_id(id).await {
            Some(task) => {
                task.status == DownloadStatus::Downloading
                    || (had_handle && task.status == DownloadStatus::Paused)
            }
            None => false,
        };
        if !pausable {
            return Ok(false);
        }
        self.write_paused_status(id).await;
        drop(claim);
        Ok(true)
    }

    /// Batch version of `pause_if_in_flight`: pauses every id concurrently
    /// instead of paying the claim-wait budget once per id, for the same
    /// reason `pause_in_flight` does (see its comment). Results are in `ids`
    /// order.
    pub async fn pause_many_if_in_flight(&self, ids: &[Uuid]) -> Vec<Result<bool, TaskBusy>> {
        futures_util::future::join_all(ids.iter().map(|&id| self.pause_if_in_flight(id))).await
    }

    /// Applies a header edit to a queued task. Takes the same start claim
    /// `start_download` uses -- for the whole call, not just a point-in-time
    /// check -- and finds+checks+applies the edit under one `FolderQueue`
    /// write lock (`FolderQueue::modify`). Together these close the races a
    /// separate read-then-write would have: a concurrent `start_download`
    /// can't take the claim mid-edit and write back its own pre-edit
    /// snapshot, and no other same-task mutation (a pause, `set_priority`,
    /// `change_folder`) can land between this call's own check and apply.
    ///
    /// A `start_download` call for `id` that arrives while this call holds
    /// the claim returns `Ok(StartOutcome::Busy)` without starting -- the
    /// same collapse two concurrent `start_download` calls already get. The
    /// window is only a few in-memory lock operations, not the beforeRequest
    /// script round trip `start_download` itself awaits while holding its
    /// own claim.
    pub async fn edit_task_headers(
        &self,
        id: Uuid,
        edit: impl FnOnce(&mut DownloadTask) + Send,
    ) -> Result<DownloadTask, EditHeadersError> {
        let Some(_claim) = StartClaim::try_acquire(&self.start_claims, id) else {
            return Err(EditHeadersError::Busy(id, "starting"));
        };
        // Covers a genuinely `Downloading` task and one in retry backoff
        // (status `Paused`, handle still alive, no claim held): either would
        // otherwise overwrite this edit once it settles or writes back.
        if self.is_running(id).await {
            return Err(EditHeadersError::Busy(id, "downloading"));
        }

        // Locate the queue fresh, the way `get_by_id` does, instead of
        // trusting a `folder_id` read earlier in this call: a concurrent
        // `change_folder` could have moved the task since.
        let queues: Vec<FolderQueue> = {
            let guard = self.folder_queues.read().await;
            guard.values().cloned().collect()
        };
        for queue in &queues {
            if queue.get_by_id(id).await.is_some() {
                return queue.modify(id, edit).await.ok_or(EditHeadersError::NotFound);
            }
        }
        Err(EditHeadersError::NotFound)
    }

    /// Move a task to another folder queue, adopting the destination folder's
    /// settings for every value the task had inherited from its old folder.
    ///
    /// A task that was downloading or sleeping in retry backoff is re-queued
    /// as `Pending`, not `Paused`: it has to start over anyway, and only
    /// `Pending` tasks are picked up by the folder-scoped start actions. The
    /// destination folder's `auto_start_downloads` is deliberately not
    /// consulted — a move never starts a download on its own.
    ///
    /// Held for the whole move: `id`'s claim, so a concurrent start cannot
    /// write this task back into the source queue after it is gone, and no
    /// other pause/remove/move can interleave with this one. `Err` downcasts
    /// to `TaskBusy` when another operation still holds the claim after the
    /// wait budget; nothing is changed in that case.
    ///
    /// `config` may be `None`, which skips both the settings adoption and the
    /// destination-folder check.
    pub async fn change_folder(
        &self,
        id: Uuid,
        new_folder_id: String,
        config: Option<&tokio::sync::RwLock<crate::app::config::Config>>,
    ) -> Result<()> {
        let current = self
            .get_by_id(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Task not found"))?;

        // A task already in the destination folder has nothing to move, and the
        // stop below would interrupt its download for no reason.
        if current.folder_id == new_folder_id {
            return Ok(());
        }

        // Reject an unknown destination rather than letting the queue creation
        // below invent a folder that no config entry backs: such a queue is
        // reachable by task id only, so its tasks disappear from the folder
        // tree and from every folder-scoped operation.
        if let Some(config_lock) = config {
            let cfg = config_lock.read().await;
            if !cfg.folders.contains_key(&new_folder_id) {
                return Err(anyhow::anyhow!("Folder '{}' not found", new_folder_id));
            }
        }

        let claim = self.acquire_claim_waiting(id, self.claim_wait_budget).await?;

        // Re-read under the claim: `current` above may be stale by the time
        // the claim was granted, e.g. a concurrent move already landed it in
        // the destination.
        let current = self
            .get_by_id(id)
            .await
            .ok_or_else(|| anyhow::anyhow!("Task not found"))?;
        if current.folder_id == new_folder_id {
            return Ok(());
        }

        // An in-flight download (downloading, or sleeping in retry backoff)
        // holds a clone of its source folder's queue, so moving it would send
        // every later count update to the wrong queue and leave it stuck.
        // Stop it first: that aborts the spawned future (or the retry sleep)
        // and settles the source queue's counts before the move.
        let must_stop = current.status == DownloadStatus::Downloading || self.is_running(id).await;
        if must_stop {
            self.pause_claimed(id, &claim).await;
        }

        // Find and remove from old folder queue. Snapshot the queues (cheap Arc
        // clones) and drop the outer lock before awaiting on individual queues,
        // so a concurrent writer to `folder_queues` can't be blocked behind
        // this read guard.
        let queues: Vec<FolderQueue> = {
            let guard = self.folder_queues.read().await;
            guard.values().cloned().collect()
        };
        let mut task = None;
        for queue in queues {
            if let Some(t) = queue.remove(id).await {
                task = Some(t);
                break;
            }
        }

        if let Some(mut task) = task {
            let old_folder_id = task.folder_id.clone();
            task.folder_id = new_folder_id.clone();

            // Propagate new folder settings to the task
            if let Some(config_lock) = config {
                let cfg = config_lock.read().await;
                let old_folder = cfg.folders.get(&old_folder_id);
                let new_folder = cfg.folders.get(&new_folder_id);

                // Reset save_path: if it matches the old folder's (or app default),
                // update to the new folder's save_path so ResolvedSettings won't
                // treat it as a manual override.
                let old_base = old_folder
                    .map(|f| f.save_path.clone())
                    .unwrap_or_else(|| cfg.download.default_directory.clone());
                if task.save_path == old_base || task.save_path == cfg.download.default_directory {
                    task.save_path = new_folder
                        .map(|f| f.save_path.clone())
                        .unwrap_or_else(|| cfg.download.default_directory.clone());
                }

                // Reset user_agent: clear if it came from the old folder config
                if let Some(old_ua) = old_folder.and_then(|f| f.user_agent.as_ref()) {
                    if task.user_agent.as_ref() == Some(old_ua) {
                        task.user_agent = new_folder.and_then(|f| f.user_agent.clone());
                    }
                }

                // Reset headers: remove old folder's default_headers, add new ones
                if let Some(old_f) = old_folder {
                    for key in old_f.default_headers.keys() {
                        // Only remove if the value still matches the old folder default
                        if task.headers.get(key) == old_f.default_headers.get(key) {
                            task.headers.remove(key);
                        }
                    }
                }
                if let Some(new_f) = new_folder {
                    for (k, v) in &new_f.default_headers {
                        // Insert new folder defaults (task-level overrides will win at resolve time)
                        task.headers.entry(k.clone()).or_insert_with(|| v.clone());
                    }
                }
            }

            // An interrupted download cannot be resumed across the move:
            // `start_download` only sends a Range request for a `Paused`/`Error`
            // task, and the partial file may not even live under the destination
            // folder's save_path. Re-queue it as `Pending` — that is the state
            // the folder-scoped start actions look for — and drop the progress
            // that no longer describes anything on disk.
            if must_stop {
                task.status = DownloadStatus::Pending;
                task.downloaded = 0;
                task.log_info(format!(
                    "Re-queued as pending after moving to folder '{}'",
                    new_folder_id
                ));
            }

            // Add to new folder queue
            let new_queue = self.get_or_create_folder_queue(&new_folder_id).await;
            new_queue.add(task).await;

            // The source folder may have just been drained. Release its
            // activation slot, otherwise `parallel_folder_count` keeps the
            // destination folder from ever being activated and its tasks stay
            // `Pending`.
            self.deactivate_folder_if_empty(&old_folder_id).await;
            Ok(())
        } else {
            Err(anyhow::anyhow!("Task not found"))
        }
    }

    /// Rename a folder: update folder_id on all tasks in the old folder queue,
    /// then move the queue entry to the new key.
    pub async fn rename_folder(&self, old_id: &str, new_id: &str) -> Result<()> {
        let mut queues = self.folder_queues.write().await;
        if let Some(queue) = queues.remove(old_id) {
            // Update folder_id on every task in the queue
            let tasks = queue.get_all().await;
            for mut task in tasks {
                task.folder_id = new_id.to_string();
                queue.update(task).await;
            }
            queues.insert(new_id.to_string(), queue);
        }
        Ok(())
    }

    pub async fn change_save_path(&self, id: Uuid, new_path: std::path::PathBuf) -> Result<()> {
        if let Some(mut task) = self.get_by_id(id).await {
            // Only allow changing path if download hasn't started or is paused
            if matches!(task.status, DownloadStatus::Pending | DownloadStatus::Paused | DownloadStatus::Error) {
                let folder_id = task.folder_id.clone();
                task.save_path = new_path;
                if let Some(queue) = self.get_folder_queue(&folder_id).await {
                    queue.update(task).await;
                }
                Ok(())
            } else {
                Err(anyhow::anyhow!("Cannot change path of active or completed download"))
            }
        } else {
            Err(anyhow::anyhow!("Task not found"))
        }
    }

    /// Resume all paused and error downloads
    /// Returns the number of downloads resumed
    pub async fn resume_all(
        &self,
        script_sender: Option<mpsc::Sender<ScriptRequest>>,
        config: Arc<tokio::sync::RwLock<crate::app::config::Config>>,
    ) -> usize {
        let downloads = self.get_all_downloads().await;
        let resumable: Vec<Uuid> = downloads
            .iter()
            .filter(|t| matches!(t.status, DownloadStatus::Paused | DownloadStatus::Error))
            .map(|t| t.id)
            .collect();
        
        let mut resumed = 0;
        for id in resumable {
            if matches!(self.start_download(id, script_sender.clone(), config.clone()).await, Ok(StartOutcome::Started)) {
                resumed += 1;
            }
        }
        resumed
    }

    /// Pause every task that is in flight (downloading, waiting for a
    /// permit, in retry backoff, or mid-start).
    /// Returns the number of downloads paused
    pub async fn pause_all(&self) -> usize {
        self.pause_in_flight(None).await
    }

    /// Pauses every in-flight task, optionally limited to one folder.
    /// Returns how many pauses succeeded; a task whose claim stays busy is
    /// logged and skipped.
    async fn pause_in_flight(&self, folder_id: Option<&str>) -> usize {
        let candidates = match folder_id {
            Some(folder_id) => match self.get_folder_queue(folder_id).await {
                Some(queue) => queue.get_all().await,
                None => return 0,
            },
            None => self.get_all_downloads().await,
        };

        let mut in_flight = Vec::new();
        for task in &candidates {
            if self.task_in_flight(task).await {
                in_flight.push(task.id);
            }
        }

        // Different ids use different claims, so pausing them concurrently is
        // safe -- and required: sequentially, N busy tasks would each pay the
        // claim-wait budget plus up to 5s for the handle to stop, easily
        // exceeding the daemon client's request timeout.
        let results = futures_util::future::join_all(
            in_flight.into_iter().map(|id| self.pause_if_in_flight(id)),
        )
        .await;

        let mut paused = 0;
        for result in results {
            match result {
                Ok(true) => paused += 1,
                Ok(false) => {}
                Err(e) => tracing::warn!("skipped during a batch pause: {e}"),
            }
        }
        paused
    }

    /// Get count of paused downloads
    pub async fn get_paused_count(&self) -> usize {
        let downloads = self.get_all_downloads().await;
        downloads.iter()
            .filter(|t| t.status == DownloadStatus::Paused)
            .count()
    }

    /// Get count of active (downloading) downloads
    pub async fn get_downloading_count(&self) -> usize {
        let downloads = self.get_all_downloads().await;
        downloads.iter()
            .filter(|t| t.status == DownloadStatus::Downloading)
            .count()
    }

    // ========== Circuit Breaker ==========

    /// Get the circuit breaker for accessing domain status
    pub fn circuit_breaker(&self) -> &super::circuit_breaker::CircuitBreaker {
        &self.circuit_breaker
    }

    /// Reset circuit breaker for a specific domain
    pub fn reset_circuit(&self, domain: &str) {
        self.circuit_breaker.reset(domain);
    }

    /// Reset all circuit breakers
    pub fn reset_all_circuits(&self) {
        self.circuit_breaker.clear_all();
    }

    /// Get list of domains with open circuits
    pub fn get_blocked_domains(&self) -> Vec<String> {
        self.circuit_breaker.get_open_circuits()
    }

    /// Try to activate a folder if active folder limit allows
    ///
    /// Returns true if folder was activated or already active
    async fn try_activate_folder(&self, folder_id: &str) -> bool {
        let mut active = self.active_folders.write().await;

        // Already active
        if active.contains(folder_id) {
            return true;
        }

        // Check if we can activate more folders
        if active.len() < self.parallel_folder_count {
            active.insert(folder_id.to_string());
            tracing::trace!(
                "Activated folder '{}' ({}/{} active folders)",
                folder_id,
                active.len(),
                self.parallel_folder_count
            );
            true
        } else {
            tracing::debug!(
                "Cannot activate folder '{}': {} folders already active (max active folders: {})",
                folder_id,
                active.len(),
                self.parallel_folder_count
            );
            false
        }
    }

    /// Release bookkeeping for a task that was scheduled but failed to acquire a
    /// concurrency permit. This is a defensive path: the semaphores are never
    /// closed in normal operation, so this should be unreachable.
    async fn abort_unstarted(&self, queue: &FolderQueue, mut task: DownloadTask, folder_id: &str) {
        task.status = DownloadStatus::Error;
        task.error_message = Some("Internal error: concurrency semaphore closed".to_string());
        queue.update(task).await; // Downloading -> Error releases the downloading count
        self.deactivate_folder_if_empty(folder_id).await;
    }

    /// Deactivate folder if it has no pending or active downloads (O(1) operation)
    async fn deactivate_folder_if_empty(&self, folder_id: &str) {
        // Use O(1) counter check instead of O(n) queue iteration
        if !self.folder_has_active_tasks(folder_id).await {
            let mut active = self.active_folders.write().await;
            if active.remove(folder_id) {
                tracing::trace!(
                    "Deactivated folder '{}' ({}/{} active folders)",
                    folder_id,
                    active.len(),
                    self.parallel_folder_count
                );
            }
        }
    }

    /// Adjust the application-wide concurrency limit at runtime by growing or
    /// shrinking the global semaphore's permit pool. Clamped to a minimum of 1
    /// (a zero limit would deadlock every acquirer). Shrinking only removes
    /// currently-available permits; permits held by in-flight downloads take
    /// effect as they complete.
    pub async fn set_max_concurrent(&self, max: usize) {
        let max = max.max(1);
        let mut current = self.max_concurrent.write().await;
        match max.cmp(&current) {
            std::cmp::Ordering::Greater => self.global_semaphore.add_permits(max - *current),
            std::cmp::Ordering::Less => {
                self.global_semaphore.forget_permits(*current - max);
            }
            std::cmp::Ordering::Equal => {}
        }
        *current = max;
    }

    /// Number of live handles (downloading, waiting for a permit, or in
    /// retry backoff). A finished-but-not-yet-released entry (see
    /// `release_active_entry`) does not count.
    pub async fn get_active_count(&self) -> usize {
        self.active_downloads
            .read()
            .await
            .values()
            .filter(|e| !e.handle.is_finished())
            .count()
    }

    /// Save queue to file (legacy single-file format)
    pub async fn save_queue(&self, path: &std::path::Path) -> Result<()> {
        // Collect all tasks from folder queues into legacy queue format
        let all_tasks = self.get_all_downloads().await;
        let json = serde_json::to_string_pretty(&all_tasks)?;
        std::fs::write(path, json)?;
        Ok(())
    }

    /// Load queue from file (legacy single-file format)
    pub async fn load_queue(&self, path: &std::path::Path) -> Result<()> {
        let temp = DownloadQueue::new();
        temp.load_from_file(path).await?;
        let tasks = temp.get_all().await;
        for task in tasks {
            let folder_id = task.folder_id.clone();
            let queue = self.get_or_create_folder_queue(&folder_id).await;
            queue.add(task).await;
        }
        Ok(())
    }

    /// Save queue partitioned by folder to folder-specific TOML files
    pub async fn save_queue_to_folders(&self) -> Result<()> {
        let queues = self.folder_queues.read().await;
        for queue in queues.values() {
            queue.save().await?;
        }
        Ok(())
    }

    /// Load queue from all folder-specific TOML files
    pub async fn load_queue_from_folders(&self) -> Result<()> {
        let temp = DownloadQueue::new();
        temp.load_from_folder_files().await?;
        let tasks = temp.get_all().await;

        for task in tasks {
            let folder_id = task.folder_id.clone();
            let queue = self.get_or_create_folder_queue(&folder_id).await;
            queue.add(task).await;
        }

        Ok(())
    }

    /// Marks every persisted `Downloading` task as `Paused`. Called once at
    /// process start, before any download handle can exist, so a persisted
    /// `Downloading` status can only mean the previous process was
    /// interrupted (crash or unclean shutdown).
    pub async fn recover_interrupted(&self) -> Vec<Uuid> {
        let queues: Vec<FolderQueue> = {
            let guard = self.folder_queues.read().await;
            guard.values().cloned().collect()
        };

        let mut recovered = Vec::new();
        for queue in queues {
            for mut task in queue.get_all().await {
                if task.status == DownloadStatus::Downloading {
                    recovered.push(task.id);
                    task.status = DownloadStatus::Paused;
                    queue.update(task).await;
                }
            }
        }
        recovered
    }

    /// True if a download handle exists for `id` and has not finished yet.
    /// Release happens on a normal exit (`release_active_entry`) or when
    /// pause/remove take the entry out of the map; `is_finished()` is kept as
    /// a filter for the case a panic in the spawned body skips that release.
    pub async fn is_running(&self, id: Uuid) -> bool {
        match self.active_downloads.read().await.get(&id) {
            Some(entry) => !entry.handle.is_finished(),
            None => false,
        }
    }

    /// Removes `id`'s entry from `active_downloads` if it still has the
    /// generation this call spawned -- a pause/remove that already took the
    /// entry out (and possibly let a later `start_download` insert a new one)
    /// must not have its replacement deleted from under it.
    async fn release_active_entry(&self, id: Uuid, generation: u64) {
        let mut active = self.active_downloads.write().await;
        if active.get(&id).is_some_and(|e| e.generation == generation) {
            active.remove(&id);
        }
    }

    /// Claim held -> `Claimed`; else a live handle -> `RetryBackoff` if
    /// sleeping between retry attempts, else `Active`; else `Idle`.
    pub async fn run_state(&self, id: Uuid) -> RunState {
        if self.is_start_in_progress(id) {
            return RunState::Claimed;
        }
        match self.active_downloads.read().await.get(&id) {
            Some(entry) if !entry.handle.is_finished() => {
                if entry.in_backoff.load(Ordering::Relaxed) {
                    RunState::RetryBackoff
                } else {
                    RunState::Active
                }
            }
            _ => RunState::Idle,
        }
    }

    /// True if `id` has live work in this process: a claim held, a live
    /// handle (including retry backoff), or a persisted status of
    /// `Downloading` (a safety net for a stale status with no handle --
    /// pausing such a task repairs it).
    pub async fn is_in_flight(&self, id: Uuid) -> bool {
        if self.run_state(id).await != RunState::Idle {
            return true;
        }
        matches!(self.get_by_id(id).await, Some(task) if task.status == DownloadStatus::Downloading)
    }

    /// Same rule as `is_in_flight`, for a caller that already has the task in
    /// hand (snapshot loops) and wants to avoid a second `get_by_id`.
    async fn task_in_flight(&self, task: &DownloadTask) -> bool {
        task.status == DownloadStatus::Downloading || self.run_state(task.id).await != RunState::Idle
    }

    /// Inserts a handle into `active_downloads` for `id` without going
    /// through `start_download`, so a test can make `is_running(id)` true for
    /// an id that is in no folder queue -- simulating the window between a
    /// completing download's `queue.remove` and its `history.add` (see
    /// `manager.rs`'s completion path), which a caller outside this module
    /// (`cli::daemon_ops`) otherwise has no way to construct.
    #[cfg(test)]
    pub(crate) async fn insert_active_handle_for_test(&self, id: Uuid, handle: JoinHandle<()>) {
        let generation = self.next_generation.fetch_add(1, Ordering::Relaxed);
        self.active_downloads.write().await.insert(
            id,
            ActiveEntry {
                handle,
                generation,
                in_backoff: Arc::new(AtomicBool::new(false)),
            },
        );
    }

    /// Aborts and drops a handle inserted by `insert_active_handle_for_test`,
    /// so a test doesn't leak a pending task for the life of the process.
    #[cfg(test)]
    pub(crate) async fn abort_active_handle_for_test(&self, id: Uuid) {
        if let Some(entry) = self.active_downloads.write().await.remove(&id) {
            entry.handle.abort();
        }
    }

    /// Sets or clears the in-memory retry-backoff flag for `id`'s active
    /// entry, for tests that need `run_state` to report `RetryBackoff`
    /// without waiting out a real backoff sleep. No-op if `id` has no entry.
    #[cfg(test)]
    pub(crate) async fn mark_backoff_for_test(&self, id: Uuid, on: bool) {
        if let Some(entry) = self.active_downloads.read().await.get(&id) {
            entry.in_backoff.store(on, Ordering::Relaxed);
        }
    }

    /// True while some operation holds `id`'s claim: a `start_download` call
    /// still running its beforeRequest script, or a pause/remove/move/header
    /// edit in progress.
    pub(crate) fn is_start_in_progress(&self, id: Uuid) -> bool {
        self.start_claims
            .ids
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains(&id)
    }

    /// Get download by ID (searches all folder queues)
    pub async fn get_by_id(&self, id: Uuid) -> Option<DownloadTask> {
        let queues = self.folder_queues.read().await;
        for queue in queues.values() {
            if let Some(task) = queue.get_by_id(id).await {
                return Some(task);
            }
        }
        None
    }

    /// True if any task has live work in this process: a live handle
    /// (including retry backoff and permit waits) or a claim held (a start
    /// or header edit in progress).
    pub async fn has_active_downloads(&self) -> bool {
        let any_live_handle = self
            .active_downloads
            .read()
            .await
            .values()
            .any(|e| !e.handle.is_finished());
        any_live_handle
            || !self
                .start_claims
                .ids
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .is_empty()
    }

    /// Set priority for a download task
    pub async fn set_priority(&self, id: Uuid, priority: u8) -> Result<()> {
        let queues = self.folder_queues.read().await;
        for queue in queues.values() {
            if queue.set_priority(id, priority as i32).await {
                return Ok(());
            }
        }
        Err(anyhow::anyhow!("Download not found"))
    }

    /// Move download to top of queue
    pub async fn move_to_top(&self, id: Uuid) -> Result<()> {
        let queues = self.folder_queues.read().await;
        for queue in queues.values() {
            if queue.move_to_top(id).await {
                return Ok(());
            }
        }
        Err(anyhow::anyhow!("Download not found"))
    }

    /// Move download to bottom of queue
    pub async fn move_to_bottom(&self, id: Uuid) -> Result<()> {
        let queues = self.folder_queues.read().await;
        for queue in queues.values() {
            if queue.move_to_bottom(id).await {
                return Ok(());
            }
        }
        Err(anyhow::anyhow!("Download not found"))
    }

    /// Move download before another download in queue
    pub async fn move_before(&self, id: Uuid, before_id: Uuid) -> Result<()> {
        let queues = self.folder_queues.read().await;
        for queue in queues.values() {
            if queue.move_before(id, before_id).await {
                return Ok(());
            }
        }
        Err(anyhow::anyhow!("Download not found"))
    }

    // ============================================================
    // History Management Methods
    // ============================================================

    /// Add a task to history (for completed/failed/deleted items)
    pub async fn add_to_history(&self, task: DownloadTask) {
        self.history.write().await.add(task);
    }

    /// Remove a task from history by ID
    pub async fn remove_from_history(&self, id: Uuid) -> Option<DownloadTask> {
        self.history.write().await.remove(id)
    }

    /// Get all history items
    pub async fn get_history(&self) -> Vec<DownloadTask> {
        self.history.read().await.all().to_vec()
    }

    /// Get a task from history by ID
    pub async fn get_history_item(&self, id: Uuid) -> Option<DownloadTask> {
        self.history.read().await.get(id).cloned()
    }

    /// Clear all history items
    pub async fn clear_history(&self) {
        self.history.write().await.clear();
    }

    /// Get the number of history items
    pub async fn history_len(&self) -> usize {
        self.history.read().await.len()
    }

    /// Load history from file
    pub async fn load_history(&self, path: &std::path::Path) -> Result<()> {
        let history = DownloadHistory::load(path)?;
        *self.history.write().await = history;
        Ok(())
    }

    /// Save history to file
    pub async fn save_history(&self, path: &std::path::Path) -> Result<()> {
        self.history.read().await.save(path)?;
        Ok(())
    }

    /// Move a task from history back to the download queue
    /// Resets the task status to Pending for re-download
    pub async fn move_from_history_to_queue(&self, id: Uuid, new_folder_id: Option<String>) -> Result<()> {
        let mut task = self.history.write().await.remove(id)
            .ok_or_else(|| anyhow::anyhow!("History item not found"))?;

        // Reset task for re-download
        task.status = DownloadStatus::Pending;
        task.downloaded = 0;
        task.error_message = None;
        task.logs.clear();
        task.retry_count = 0;
        task.started_at = None;
        task.completed_at = None;

        // Update folder if specified
        if let Some(folder_id) = new_folder_id {
            task.folder_id = folder_id;
        }

        // Add to the appropriate folder queue
        let folder_id = task.folder_id.clone();
        let queue = self.get_or_create_folder_queue(&folder_id).await;
        queue.add(task).await;
        Ok(())
    }

    // ========== Batch Operations ==========

    /// Start all pending tasks in a specific folder
    /// Returns the number of tasks started
    pub async fn start_folder_tasks(
        &self,
        folder_id: &str,
        script_sender: Option<mpsc::Sender<ScriptRequest>>,
        config: Arc<tokio::sync::RwLock<crate::app::config::Config>>,
    ) -> usize {
        let queue = match self.get_folder_queue(folder_id).await {
            Some(q) => q,
            None => return 0,
        };

        let pending_tasks = queue.get_pending_tasks().await;
        let mut started = 0;

        for task in pending_tasks {
            if matches!(self.start_download(task.id, script_sender.clone(), config.clone()).await, Ok(StartOutcome::Started)) {
                started += 1;
            }
        }

        started
    }

    /// Stop (pause) every in-flight task in a specific folder (downloading,
    /// waiting for a permit, in retry backoff, or mid-start).
    /// Returns the number of tasks stopped
    pub async fn stop_folder_tasks(&self, folder_id: &str) -> usize {
        self.pause_in_flight(Some(folder_id)).await
    }

    /// Start all pending tasks across all folders
    /// Returns the number of tasks started
    pub async fn start_all_tasks(
        &self,
        script_sender: Option<mpsc::Sender<ScriptRequest>>,
        config: Arc<tokio::sync::RwLock<crate::app::config::Config>>,
    ) -> usize {
        let downloads = self.get_all_downloads().await;
        let pending: Vec<Uuid> = downloads
            .iter()
            .filter(|t| t.status == DownloadStatus::Pending)
            .map(|t| t.id)
            .collect();

        let mut started = 0;
        for id in pending {
            if matches!(self.start_download(id, script_sender.clone(), config.clone()).await, Ok(StartOutcome::Started)) {
                started += 1;
            }
        }

        started
    }

    /// Stop every in-flight task across all folders (downloading, waiting
    /// for a permit, in retry backoff, or mid-start).
    /// Returns the number of tasks stopped
    pub async fn stop_all_tasks(&self) -> usize {
        self.pause_in_flight(None).await
    }

    /// Get folder queue counts for display
    pub async fn get_folder_counts(&self, folder_id: &str) -> FolderTaskCounts {
        if let Some(queue) = self.get_folder_queue(folder_id).await {
            queue.get_counts().await
        } else {
            FolderTaskCounts::default()
        }
    }
}

impl Default for DownloadManager {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::{Config, FolderConfig};
    use crate::download::task::DownloadTask;
    use std::collections::HashMap;

    #[tokio::test]
    async fn test_subscribe_receives_lifecycle_events() {
        let manager = DownloadManager::new();
        let mut rx = manager.subscribe();

        let task = DownloadTask::new("https://example.com/file.bin".to_string(), "/tmp".into());
        let id = task.id;
        manager.add_download(task).await;

        match rx.try_recv() {
            Ok(DownloadEvent::Added { task_id, .. }) => assert_eq!(task_id, id),
            other => panic!("expected Added event, got {other:?}"),
        }

        manager.remove_download(id).await.expect("claim not held");
        match rx.try_recv() {
            Ok(DownloadEvent::Removed { task_id }) => assert_eq!(task_id, id),
            other => panic!("expected Removed event, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn test_emit_without_subscribers_is_noop() {
        // Emitting with no active receivers must not panic or error.
        let manager = DownloadManager::new();
        let task = DownloadTask::new("https://example.com/x".to_string(), "/tmp".into());
        manager.add_download(task).await;
    }

    #[tokio::test]
    async fn test_compute_effective_script_files_application_only() {
        // Setup: Application-level scripts only
        let mut config = Config::default();
        config.scripts.script_files.insert("script1.js".to_string(), true);
        config.scripts.script_files.insert("script2.js".to_string(), false);

        let config = Arc::new(tokio::sync::RwLock::new(config));
        let folder_id = "test_folder";

        // Execute
        let result = DownloadManager::compute_effective_script_files(&config, folder_id).await;

        // Verify
        assert_eq!(result.get("script1.js"), Some(&true));
        assert_eq!(result.get("script2.js"), Some(&false));
    }

    #[tokio::test]
    async fn test_compute_effective_script_files_folder_override() {
        // Setup: Application-level scripts with folder override
        let mut config = Config::default();
        config.scripts.script_files.insert("script1.js".to_string(), true);
        config.scripts.script_files.insert("script2.js".to_string(), false);

        let mut folder_config = FolderConfig::default();
        let mut folder_scripts = HashMap::new();
        folder_scripts.insert("script2.js".to_string(), true);  // Override: enable script2
        folder_scripts.insert("script3.js".to_string(), true);  // Add new script
        folder_config.script_files = Some(folder_scripts);

        config.folders.insert("test_folder".to_string(), folder_config);

        let config = Arc::new(tokio::sync::RwLock::new(config));
        let folder_id = "test_folder";

        // Execute
        let result = DownloadManager::compute_effective_script_files(&config, folder_id).await;

        // Verify
        assert_eq!(result.get("script1.js"), Some(&true));   // From application
        assert_eq!(result.get("script2.js"), Some(&true));   // Overridden by folder
        assert_eq!(result.get("script3.js"), Some(&true));   // Added by folder
    }

    #[tokio::test]
    async fn test_compute_effective_script_files_folder_disable() {
        // Setup: Folder disables application-enabled script
        let mut config = Config::default();
        config.scripts.script_files.insert("script1.js".to_string(), true);

        let mut folder_config = FolderConfig::default();
        let mut folder_scripts = HashMap::new();
        folder_scripts.insert("script1.js".to_string(), false);  // Override: disable script1
        folder_config.script_files = Some(folder_scripts);

        config.folders.insert("test_folder".to_string(), folder_config);

        let config = Arc::new(tokio::sync::RwLock::new(config));
        let folder_id = "test_folder";

        // Execute
        let result = DownloadManager::compute_effective_script_files(&config, folder_id).await;

        // Verify
        assert_eq!(result.get("script1.js"), Some(&false));  // Disabled by folder
    }

    #[tokio::test]
    async fn test_compute_effective_script_files_no_folder_config() {
        // Setup: No folder configuration exists
        let mut config = Config::default();
        config.scripts.script_files.insert("script1.js".to_string(), true);

        let config = Arc::new(tokio::sync::RwLock::new(config));
        let folder_id = "nonexistent_folder";

        // Execute
        let result = DownloadManager::compute_effective_script_files(&config, folder_id).await;

        // Verify: Should return application-level settings
        assert_eq!(result.get("script1.js"), Some(&true));
    }

    #[tokio::test]
    async fn test_compute_effective_script_files_empty_config() {
        // Setup: No scripts configured at all
        let config = Config::default();
        let config = Arc::new(tokio::sync::RwLock::new(config));
        let folder_id = "test_folder";

        // Execute
        let result = DownloadManager::compute_effective_script_files(&config, folder_id).await;

        // Verify: Should return empty HashMap
        assert!(result.is_empty());
    }

    #[tokio::test]
    async fn test_compute_effective_script_files_multiple_folder_overrides() {
        // Setup: Multiple scripts with complex override patterns
        let mut config = Config::default();
        config.scripts.script_files.insert("s1.js".to_string(), true);
        config.scripts.script_files.insert("s2.js".to_string(), true);
        config.scripts.script_files.insert("s3.js".to_string(), false);

        let mut folder_config = FolderConfig::default();
        let mut folder_scripts = HashMap::new();
        folder_scripts.insert("s2.js".to_string(), false);  // Disable s2
        folder_scripts.insert("s3.js".to_string(), true);   // Enable s3
        folder_scripts.insert("s4.js".to_string(), true);   // Add s4
        folder_config.script_files = Some(folder_scripts);

        config.folders.insert("test_folder".to_string(), folder_config);

        let config = Arc::new(tokio::sync::RwLock::new(config));
        let folder_id = "test_folder";

        // Execute
        let result = DownloadManager::compute_effective_script_files(&config, folder_id).await;

        // Verify all expected results
        assert_eq!(result.get("s1.js"), Some(&true));   // Unchanged from app
        assert_eq!(result.get("s2.js"), Some(&false));  // Disabled by folder
        assert_eq!(result.get("s3.js"), Some(&true));   // Enabled by folder
        assert_eq!(result.get("s4.js"), Some(&true));   // Added by folder
        assert_eq!(result.len(), 4);
    }

    #[test]
    fn test_download_manager_creation() {
        // Test that DownloadManager can be created
        let _manager = DownloadManager::new();
        // Manager creation successful if no panic
    }

    #[test]
    fn test_download_manager_default() {
        // Test Default trait implementation
        let _manager = DownloadManager::default();
        // Manager creation successful if no panic
    }

    #[tokio::test]
    async fn test_set_max_concurrent() {
        // Test changing max concurrent downloads
        let manager = DownloadManager::new();

        // Change to different value
        manager.set_max_concurrent(5).await;

        // Verify it was updated
        let current = *manager.max_concurrent.read().await;
        assert_eq!(current, 5);
    }

    #[tokio::test]
    async fn test_set_max_concurrent_zero_is_clamped() {
        // A zero limit is clamped to 1 to avoid deadlocking acquirers.
        let manager = DownloadManager::new();

        manager.set_max_concurrent(0).await;

        let current = *manager.max_concurrent.read().await;
        assert_eq!(current, 1);
    }

    #[tokio::test]
    async fn test_set_max_concurrent_large_value() {
        // Test setting max concurrent to a large value
        let manager = DownloadManager::new();

        manager.set_max_concurrent(100).await;

        let current = *manager.max_concurrent.read().await;
        assert_eq!(current, 100);
    }

    #[tokio::test]
    async fn test_get_active_count_empty() {
        // Test getting active count when no downloads are running
        let manager = DownloadManager::new();

        let count = manager.get_active_count().await;
        assert_eq!(count, 0);
    }

    #[tokio::test]
    async fn test_add_download_creates_task() {
        use std::path::PathBuf;
        let manager = DownloadManager::new();

        let url = "https://example.com/file.zip".to_string();
        let save_path = PathBuf::from("/tmp/downloads");

        let task = DownloadTask::new(url.clone(), save_path);
        let task_id = task.id;

        manager.add_download(task).await;

        // Verify task exists in queue
        let retrieved_task = manager.get_by_id(task_id).await;
        assert!(retrieved_task.is_some());

        let retrieved_task = retrieved_task.unwrap();
        assert_eq!(retrieved_task.url, url);
        assert_eq!(retrieved_task.status, DownloadStatus::Pending);
    }

    #[tokio::test]
    async fn test_add_download_sanitizes_filename() {
        use std::path::PathBuf;
        let manager = DownloadManager::new();

        // URL with invalid filename characters
        let url = "https://example.com/file<>name.zip".to_string();
        let save_path = PathBuf::from("/tmp/downloads");

        let mut task = DownloadTask::new(url.clone(), save_path);
        task.filename = "file<>name.zip".to_string(); // Set invalid filename
        let task_id = task.id;

        manager.add_download(task).await;

        let retrieved_task = manager.get_by_id(task_id).await.unwrap();

        // Filename should be sanitized
        assert!(!retrieved_task.filename.contains('<'));
        assert!(!retrieved_task.filename.contains('>'));
    }

    #[tokio::test]
    async fn test_remove_download_nonexistent() {
        let manager = DownloadManager::new();

        // Try to remove non-existent task
        let fake_id = Uuid::new_v4();
        let result = manager.remove_download(fake_id).await;

        // Should return Ok(None) for non-existent task
        assert!(matches!(result, Ok(None)));
    }

    #[tokio::test]
    async fn test_change_folder_nonexistent_task() {
        let manager = DownloadManager::new();

        // Try to change folder of non-existent task
        let fake_id = Uuid::new_v4();
        let result = manager.change_folder(fake_id, "new_folder".to_string(), None).await;

        // Should return error
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_get_all_downloads_empty() {
        let manager = DownloadManager::new();

        let tasks = manager.get_all_downloads().await;

        // Should return empty vector
        assert_eq!(tasks.len(), 0);
    }

    #[tokio::test]
    async fn test_get_by_id_nonexistent() {
        let manager = DownloadManager::new();

        let fake_id = Uuid::new_v4();
        let result = manager.get_by_id(fake_id).await;

        // Should return None
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_has_active_downloads_empty() {
        let manager = DownloadManager::new();

        let has_active = manager.has_active_downloads().await;

        // Should be false when no downloads are active
        assert!(!has_active);
    }

    #[tokio::test]
    async fn test_set_priority_nonexistent_task() {
        let manager = DownloadManager::new();

        let fake_id = Uuid::new_v4();
        let result = manager.set_priority(fake_id, 100).await;

        // Should return error
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_change_folder_releases_source_activation_slot() {
        // parallel_folder_count = 1: the destination folder can only be
        // activated if the move released the source folder's slot.
        let manager = DownloadManager::with_config(3, 3, 1, 3, 5);

        let mut task = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        assert!(manager.try_activate_folder("folder1").await);

        manager
            .change_folder(id, "folder2".to_string(), None)
            .await
            .expect("move should succeed");

        assert!(
            !manager.active_folders.read().await.contains("folder1"),
            "source folder slot must be released once it is drained"
        );
        assert!(
            manager.try_activate_folder("folder2").await,
            "destination folder must be activatable after the move"
        );
    }

    #[tokio::test]
    async fn test_change_folder_keeps_slot_when_source_still_has_tasks() {
        let manager = DownloadManager::with_config(3, 3, 2, 3, 5);

        let mut moved = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        moved.folder_id = "folder1".to_string();
        let moved_id = moved.id;
        manager.add_download(moved).await;

        let mut stays = DownloadTask::new("https://example.com/b.bin".to_string(), "/tmp".into());
        stays.folder_id = "folder1".to_string();
        manager.add_download(stays).await;

        assert!(manager.try_activate_folder("folder1").await);

        manager
            .change_folder(moved_id, "folder2".to_string(), None)
            .await
            .expect("move should succeed");

        assert!(
            manager.active_folders.read().await.contains("folder1"),
            "source folder still has a pending task, so it stays active"
        );
    }

    #[tokio::test]
    async fn test_change_folder_moves_task_between_queues() {
        let manager = DownloadManager::new();

        let mut task = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        manager
            .change_folder(id, "folder2".to_string(), None)
            .await
            .expect("move should succeed");

        let source = manager.get_folder_queue("folder1").await.expect("source queue");
        assert_eq!(source.get_counts().await.pending, 0);
        assert!(source.get_all().await.is_empty());

        let dest = manager.get_folder_queue("folder2").await.expect("destination queue");
        assert_eq!(dest.get_counts().await.pending, 1);
        assert_eq!(
            manager.get_by_id(id).await.expect("task still exists").folder_id,
            "folder2"
        );
    }

    #[tokio::test]
    async fn test_change_folder_to_same_folder_leaves_task_untouched() {
        let manager = DownloadManager::new();

        let mut task = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        task.status = DownloadStatus::Downloading;
        task.downloaded = 1234;
        let id = task.id;
        manager.add_download(task).await;

        manager
            .change_folder(id, "folder1".to_string(), None)
            .await
            .expect("same-folder move should succeed");

        let moved = manager.get_by_id(id).await.expect("task still exists");
        assert_eq!(
            moved.status,
            DownloadStatus::Downloading,
            "a same-folder move must not interrupt the download"
        );
        assert_eq!(moved.downloaded, 1234);

        let queue = manager.get_folder_queue("folder1").await.expect("queue");
        assert_eq!(queue.len().await, 1);
        assert_eq!(queue.get_counts().await.downloading, 1);
    }

    #[tokio::test]
    async fn test_change_folder_requeues_downloading_task_as_pending() {
        let manager = DownloadManager::new();

        let mut task = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        task.status = DownloadStatus::Downloading;
        task.downloaded = 1234;
        let id = task.id;
        manager.add_download(task).await;

        manager
            .change_folder(id, "folder2".to_string(), None)
            .await
            .expect("move should succeed");

        let moved = manager.get_by_id(id).await.expect("task still exists");
        assert_eq!(
            moved.status,
            DownloadStatus::Pending,
            "a moved in-flight task must be startable from the destination folder"
        );
        assert_eq!(moved.downloaded, 0, "stale progress must not survive the move");

        let source = manager.get_folder_queue("folder1").await.expect("source queue");
        assert_eq!(source.get_counts().await.downloading, 0);

        let dest = manager.get_folder_queue("folder2").await.expect("destination queue");
        let counts = dest.get_counts().await;
        assert_eq!(counts.pending, 1);
        assert_eq!(counts.downloading, 0);
    }

    #[tokio::test]
    async fn test_change_folder_rejects_destination_missing_from_config() {
        let manager = DownloadManager::new();

        let mut task = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let mut cfg = Config::default();
        cfg.folders
            .insert("known".to_string(), FolderConfig::default());
        let config = tokio::sync::RwLock::new(cfg);

        let result = manager
            .change_folder(id, "ghost".to_string(), Some(&config))
            .await;

        assert!(result.is_err(), "an unknown destination must be rejected");
        assert!(
            manager.get_folder_queue("ghost").await.is_none(),
            "a rejected move must not create a queue for the unknown folder"
        );
        assert_eq!(
            manager.get_by_id(id).await.expect("task still exists").folder_id,
            "folder1"
        );

        manager
            .change_folder(id, "known".to_string(), Some(&config))
            .await
            .expect("a destination present in the config must be accepted");
    }

    #[tokio::test]
    async fn change_folder_moves_backoff_task_as_pending() {
        let manager = DownloadManager::with_retry_settings(3, 60);
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        assert_eq!(
            manager
                .start_download(id, None, config)
                .await
                .expect("start must succeed"),
            StartOutcome::Started
        );

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
        loop {
            if manager.run_state(id).await == RunState::RetryBackoff {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "download did not reach RetryBackoff within 10s"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        manager
            .change_folder(id, "folder2".to_string(), None)
            .await
            .expect("move must stop the backoff task and succeed");

        let task = manager.get_by_id(id).await.expect("task still exists");
        assert_eq!(task.folder_id, "folder2");
        assert_eq!(task.status, DownloadStatus::Pending);
        assert_eq!(task.downloaded, 0);
        assert_eq!(manager.run_state(id).await, RunState::Idle);
    }

    #[tokio::test]
    async fn change_folder_busy_leaves_task_in_place() {
        let mut manager = DownloadManager::new();
        manager.set_claim_wait_budget(std::time::Duration::from_millis(100));

        let mut task = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let claim = StartClaim::try_acquire(&manager.start_claims, id).expect("claim must be free");

        let result = manager.change_folder(id, "folder2".to_string(), None).await;
        let err = result.expect_err("a busy claim must block the move");
        assert!(
            err.downcast_ref::<TaskBusy>().is_some(),
            "expected TaskBusy, got {err:?}"
        );

        assert_eq!(
            manager.get_by_id(id).await.expect("task still exists").folder_id,
            "folder1"
        );

        drop(claim);
    }

    #[tokio::test]
    async fn recover_interrupted_marks_downloading_as_paused() {
        let manager = DownloadManager::new();

        let mut task = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        task.status = DownloadStatus::Downloading;
        let id = task.id;
        manager.add_download(task).await;

        let recovered = manager.recover_interrupted().await;
        assert_eq!(recovered, vec![id]);

        let task = manager.get_by_id(id).await.expect("task still exists");
        assert_eq!(task.status, DownloadStatus::Paused);

        let counts = manager.get_folder_queue("folder1").await.expect("queue").get_counts().await;
        assert_eq!(counts.downloading, 0);
        let paused = manager
            .get_all_downloads()
            .await
            .into_iter()
            .filter(|t| t.status == DownloadStatus::Paused)
            .count();
        assert_eq!(paused, 1);
    }

    #[tokio::test]
    async fn is_running_false_for_unknown_id() {
        let manager = DownloadManager::new();

        let running = manager.is_running(Uuid::new_v4()).await;
        assert!(!running);
    }

    #[tokio::test]
    async fn active_entry_released_after_failure() {
        let manager = DownloadManager::with_retry_settings(1, 0);
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        assert_eq!(
            manager
                .start_download(id, None, config)
                .await
                .expect("start must succeed"),
            StartOutcome::Started
        );

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
        loop {
            let task = manager.get_by_id(id).await.expect("task exists");
            if task.status == DownloadStatus::Error && manager.get_active_count().await == 0 {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "download did not reach Error with a released entry within 10s"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        assert!(!manager.has_active_downloads().await);
        assert_eq!(manager.run_state(id).await, RunState::Idle);
    }

    /// A download that exhausts its retries must have already queued the
    /// error hook request by the time the `Failed` event is observable, so a
    /// `wait` unblocked by that event (or a standalone process exiting right
    /// after it) still sees the hook in the channel. Needs a multi-thread
    /// runtime: on a current-thread runtime the test task would only resume
    /// (and observe `Failed`) after the download task itself yields, which
    /// would hide a stale ordering where the hook is sent after `emit`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn error_hook_queued_before_failed_event() {
        let manager = DownloadManager::with_retry_settings(1, 0);
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let mut events = manager.subscribe();
        let (script_tx, script_rx) = mpsc::channel::<ScriptRequest>();
        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));

        // `start_download` awaits the beforeRequest hook's response before it
        // returns, so a single blocking recv answers it and hands the
        // receiver back untouched for the assertion below.
        let responder = tokio::task::spawn_blocking(move || {
            if let Ok(ScriptRequest::BeforeRequest { ctx, response, .. }) = script_rx.recv() {
                let _ = response.send((ctx, Ok(())));
            }
            script_rx
        });

        assert_eq!(
            manager
                .start_download(id, Some(script_tx), config)
                .await
                .expect("start must succeed"),
            StartOutcome::Started
        );

        let script_rx = responder.await.expect("responder task panicked");

        let wait_for_failed = async {
            loop {
                match events.recv().await {
                    Ok(DownloadEvent::Failed { task_id, .. }) if task_id == id => return,
                    Ok(_) => continue,
                    Err(e) => panic!("event channel closed before Failed: {e}"),
                }
            }
        };
        tokio::time::timeout(tokio::time::Duration::from_secs(10), wait_for_failed)
            .await
            .expect("did not observe Failed within timeout");

        match script_rx.try_recv() {
            Ok(ScriptRequest::Error { .. }) => {}
            other => panic!("expected a queued Error hook request, got {other:?}"),
        }
    }

    #[tokio::test]
    async fn run_state_reports_retry_backoff() {
        let manager = DownloadManager::with_retry_settings(3, 60);
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        assert_eq!(
            manager
                .start_download(id, None, config)
                .await
                .expect("start must succeed"),
            StartOutcome::Started
        );

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
        loop {
            if manager.run_state(id).await == RunState::RetryBackoff {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "download did not reach RetryBackoff within 10s"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        let task = manager.get_by_id(id).await.expect("task exists");
        assert_eq!(task.status, DownloadStatus::Paused);
        assert!(manager.is_in_flight(id).await);

        manager.abort_active_handle_for_test(id).await;
    }

    #[tokio::test]
    async fn release_ignores_older_generation() {
        let manager = DownloadManager::new();
        let id = Uuid::new_v4();
        let handle = tokio::spawn(std::future::pending::<()>());
        manager.insert_active_handle_for_test(id, handle).await;

        // The real generation starts at 0 for a fresh manager; any other
        // value simulates a release racing a newer spawn for the same id.
        manager.release_active_entry(id, u64::MAX).await;

        assert!(
            manager.is_running(id).await,
            "an entry with a non-matching generation must not be removed"
        );

        manager.abort_active_handle_for_test(id).await;
    }

    #[tokio::test]
    async fn finished_handle_not_counted_active() {
        let manager = DownloadManager::new();
        let id = Uuid::new_v4();
        let handle = tokio::spawn(async {});
        while !handle.is_finished() {
            tokio::task::yield_now().await;
        }
        manager.insert_active_handle_for_test(id, handle).await;

        assert_eq!(manager.get_active_count().await, 0);
        assert!(!manager.has_active_downloads().await);
    }

    #[tokio::test]
    async fn run_state_claimed_while_claim_held() {
        let manager = DownloadManager::new();
        let id = Uuid::new_v4();
        let claim = StartClaim::try_acquire(&manager.start_claims, id).expect("claim acquired");

        assert_eq!(manager.run_state(id).await, RunState::Claimed);
        assert!(manager.has_active_downloads().await);

        drop(claim);
    }

    /// Two concurrent `start_download` calls for the same id must collapse
    /// into a single start. Uses a multi-threaded runtime and separately
    /// spawned tasks so the two calls genuinely run in parallel.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_start_download_starts_once() {
        let manager = DownloadManager::new();

        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let mut events = manager.subscribe();

        let (script_tx, script_rx) = mpsc::channel::<ScriptRequest>();
        let before_request_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let counter = before_request_count.clone();
        let responder = std::thread::spawn(move || {
            while let Ok(req) = script_rx.recv() {
                if let ScriptRequest::BeforeRequest { ctx, response, .. } = req {
                    counter.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    std::thread::sleep(std::time::Duration::from_millis(100));
                    let _ = response.send((ctx, Ok(())));
                }
            }
        });

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));

        let manager_a = manager.clone();
        let manager_b = manager.clone();
        let tx_a = script_tx.clone();
        let tx_b = script_tx.clone();
        let config_a = config.clone();
        let config_b = config.clone();

        let handle_a = tokio::spawn(async move { manager_a.start_download(id, Some(tx_a), config_a).await });
        let handle_b = tokio::spawn(async move { manager_b.start_download(id, Some(tx_b), config_b).await });

        let (r1, r2) = tokio::join!(handle_a, handle_b);
        let outcome_a = r1.unwrap().expect("start must not error");
        let outcome_b = r2.unwrap().expect("start must not error");
        let started_outcomes = [outcome_a, outcome_b].into_iter().filter(|o| o.is_started()).count();
        assert_eq!(
            started_outcomes, 1,
            "exactly one of the two concurrent starts must be Started, got {outcome_a:?} and {outcome_b:?}"
        );

        // Both `start_download` calls have returned, so every `BeforeRequest`
        // they could send has already been counted and answered; the count is
        // final. Do not join the responder thread here: the retry loop inside
        // the spawned download task holds its own clone of `script_tx`
        // (`script_sender_for_error`), so the channel does not close, and
        // `recv()` would block until that task exhausts retries against
        // `127.0.0.1:1`.
        let _ = responder;

        assert_eq!(
            before_request_count.load(std::sync::atomic::Ordering::SeqCst),
            1,
            "exactly one BeforeRequest must be sent for two concurrent starts of the same id"
        );

        let mut started_count = 0;
        while let Ok(event) = events.try_recv() {
            if matches!(event, DownloadEvent::Started { task_id } if task_id == id) {
                started_count += 1;
            }
        }
        assert_eq!(started_count, 1, "exactly one Started event must be emitted");
    }

    #[tokio::test]
    async fn start_claim_released_on_error() {
        let manager = DownloadManager::new();
        let id = Uuid::new_v4();
        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));

        let result = manager.start_download(id, None, config.clone()).await;
        assert!(result.is_err(), "an unknown id must fail");
        assert!(!manager.is_start_in_progress(id), "the claim must be released on the error path");

        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.id = id;
        task.folder_id = "folder1".to_string();
        manager.add_download(task).await;

        let result = manager.start_download(id, None, config).await;
        assert_eq!(
            result.unwrap(), StartOutcome::Started,
            "a released claim must allow a later start to proceed"
        );

        let task = manager.get_by_id(id).await.expect("task exists");
        assert_eq!(task.status, DownloadStatus::Downloading);
    }

    #[tokio::test]
    async fn start_download_noop_while_handle_running() {
        let manager = DownloadManager::new();

        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let queue = manager.get_folder_queue("folder1").await.expect("queue");
        let mut paused_task = manager.get_by_id(id).await.expect("task exists");
        paused_task.status = DownloadStatus::Paused;
        queue.update(paused_task).await;

        // Simulate a task still in retry backoff: status is `Paused` but a
        // handle is still alive in `active_downloads`.
        let handle = tokio::spawn(std::future::pending::<()>());
        manager.insert_active_handle_for_test(id, handle).await;

        let mut events = manager.subscribe();
        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        let result = manager.start_download(id, None, config).await;
        assert_eq!(result.unwrap(), StartOutcome::AlreadyRunning);
        assert!(events.try_recv().is_err(), "no Started event must be emitted");

        let task = manager.get_by_id(id).await.expect("task exists");
        assert_eq!(task.status, DownloadStatus::Paused);

        manager.abort_active_handle_for_test(id).await;
    }

    #[tokio::test]
    async fn edit_task_headers_rejects_downloading() {
        let manager = DownloadManager::new();
        let mut task = DownloadTask::new("https://example.com/f".to_string(), "/tmp".into());
        task.status = DownloadStatus::Downloading;
        let id = task.id;
        manager.add_download(task).await;
        // `edit_task_headers` rejects via `is_running`, which checks the
        // handle map, not just the persisted status -- so a real live
        // handle is needed to exercise that path, not just the status field.
        let handle = tokio::spawn(std::future::pending::<()>());
        manager.insert_active_handle_for_test(id, handle).await;

        let result = manager
            .edit_task_headers(id, |task| task.set_header("Referer", "https://a.example/"))
            .await;
        match result {
            Err(EditHeadersError::Busy(busy_id, _)) => assert_eq!(busy_id, id),
            other => panic!("expected Busy, got {other:?}"),
        }
        let task = manager.get_by_id(id).await.expect("task still queued");
        assert!(
            !task.headers.contains_key("Referer"),
            "a rejected edit must not be applied"
        );

        manager.abort_active_handle_for_test(id).await;
    }

    /// Holds a `StartClaim` directly (this test lives in the same module, so
    /// it can reach the private type) rather than driving a real
    /// `start_download` call, since only the claim itself -- not the script
    /// round trip -- matters here.
    #[tokio::test]
    async fn edit_task_headers_rejects_while_start_in_progress() {
        let manager = DownloadManager::new();
        let task = DownloadTask::new("https://example.com/f".to_string(), "/tmp".into());
        let id = task.id;
        manager.add_download(task).await;

        let _claim = StartClaim::try_acquire(&manager.start_claims, id).expect("claim must be free");
        assert!(manager.is_start_in_progress(id));

        let result = manager
            .edit_task_headers(id, |task| task.set_header("Referer", "https://a.example/"))
            .await;
        assert!(result.is_err(), "a task with a start in progress must refuse the edit");
        let task = manager.get_by_id(id).await.expect("task still queued");
        assert!(
            !task.headers.contains_key("Referer"),
            "a rejected edit must not be applied"
        );
    }

    /// `edit_task_headers` holds the start claim for its whole call, so a
    /// concurrent `start_download` must collapse into a no-op instead of
    /// reading the pre-edit task and writing it back over the edit. Blocks
    /// the edit closure on a plain (non-tokio) channel so the claim stays
    /// held while `start_download` is attempted from a second worker
    /// thread.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn edit_task_headers_blocks_concurrent_start_until_done() {
        let manager = DownloadManager::new();
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let (release_tx, release_rx) = mpsc::channel::<()>();
        let manager_edit = manager.clone();
        let edit_handle = tokio::spawn(async move {
            manager_edit
                .edit_task_headers(id, move |task| {
                    // Block synchronously (not `.await`: this closure runs
                    // inside `FolderQueue::modify`'s write-lock scope,
                    // which is not an async fn) until the test signals it
                    // to proceed, keeping the claim held while a concurrent
                    // `start_download` is attempted below.
                    let _ = release_rx.recv();
                    task.set_header("Referer", "https://a.example/");
                })
                .await
        });

        // Give `edit_task_headers` time to acquire the claim before racing it.
        let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
        while !manager.is_start_in_progress(id) {
            assert!(
                tokio::time::Instant::now() < deadline,
                "edit did not claim the id within 2s"
            );
            tokio::time::sleep(std::time::Duration::from_millis(5)).await;
        }

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        let start_result = manager.start_download(id, None, config).await;
        assert_eq!(
            start_result.unwrap(), StartOutcome::Busy,
            "a concurrent start must collapse into a no-op, not error"
        );

        release_tx.send(()).expect("edit closure must still be waiting");
        let edit_result = edit_handle.await.expect("edit task must not panic");
        assert!(edit_result.is_ok(), "edit must succeed: {edit_result:?}");

        let task = manager.get_by_id(id).await.expect("task still queued");
        assert_eq!(
            task.headers.get("Referer").map(String::as_str),
            Some("https://a.example/"),
            "the edit must not be overwritten by the collapsed start"
        );
    }

    /// A task in retry backoff (`Paused` with a live handle) must not be
    /// double-counted by `resume_all`: `start_download` reports it as
    /// `AlreadyRunning`, not `Started`.
    #[tokio::test]
    async fn resume_all_does_not_count_running_task() {
        let manager = DownloadManager::new();
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        task.status = DownloadStatus::Paused;
        let id = task.id;
        manager.add_download(task).await;

        let handle = tokio::spawn(std::future::pending::<()>());
        manager.insert_active_handle_for_test(id, handle).await;

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        let resumed = manager.resume_all(None, config).await;
        assert_eq!(resumed, 0, "a task with a live handle must not be counted as resumed");

        manager.abort_active_handle_for_test(id).await;
    }

    /// `pause_download` must wait out a concurrent `start_download`'s
    /// beforeRequest script round trip instead of racing it, then win: the
    /// download it just spawned is stopped and the task ends up `Paused`.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn pause_waits_for_in_flight_start_and_wins() {
        let manager = DownloadManager::new();
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let (script_tx, script_rx) = mpsc::channel::<ScriptRequest>();
        // Never join this thread: nothing else drains the channel after the
        // test's assertions run, and the retry loop's own clone would keep
        // it open, so joining would hang.
        let _responder = std::thread::spawn(move || {
            while let Ok(req) = script_rx.recv() {
                if let ScriptRequest::BeforeRequest { ctx, response, .. } = req {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let _ = response.send((ctx, Ok(())));
                }
            }
        });

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        let manager_start = manager.clone();
        let start_handle = tokio::spawn(async move {
            manager_start.start_download(id, Some(script_tx), config).await
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

        let pause_result = manager.pause_download(id).await;
        assert!(pause_result.is_ok(), "pause must wait out the claim and then succeed");

        let start_outcome = start_handle.await.unwrap().expect("start must not error");
        assert_eq!(start_outcome, StartOutcome::Started);

        let task = manager.get_by_id(id).await.expect("task still queued");
        assert_eq!(task.status, DownloadStatus::Paused);
        assert_eq!(manager.run_state(id).await, RunState::Idle);
        assert_eq!(manager.get_active_count().await, 0);
    }

    /// A pause that cannot take the claim within the budget must leave the
    /// task untouched and report `TaskBusy`.
    #[tokio::test]
    async fn pause_returns_busy_after_budget() {
        let mut manager = DownloadManager::new();
        manager.set_claim_wait_budget(std::time::Duration::from_millis(100));

        let task = DownloadTask::new("https://example.com/f".to_string(), "/tmp".into());
        let id = task.id;
        manager.add_download(task).await;

        let claim = StartClaim::try_acquire(&manager.start_claims, id).expect("claim must be free");

        let result = manager.pause_download(id).await;
        match result {
            Err(TaskBusy(busy_id)) => assert_eq!(busy_id, id),
            other => panic!("expected TaskBusy, got {other:?}"),
        }

        let task = manager.get_by_id(id).await.expect("task still queued");
        assert_eq!(task.status, DownloadStatus::Pending, "a busy pause must not change the task");

        drop(claim);
    }

    /// A task in retry backoff (`Paused` with a live handle) is paused like
    /// a downloading one: the handle is stopped and the status stays
    /// `Paused`, but the handle -- and thus `RetryBackoff` -- is gone.
    #[tokio::test]
    async fn pause_stops_retry_backoff() {
        let manager = DownloadManager::with_retry_settings(3, 60);
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        assert_eq!(
            manager
                .start_download(id, None, config)
                .await
                .expect("start must succeed"),
            StartOutcome::Started
        );

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
        loop {
            if manager.run_state(id).await == RunState::RetryBackoff {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "download did not reach RetryBackoff within 10s"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        manager.pause_download(id).await.expect("pause must succeed");

        assert_eq!(manager.run_state(id).await, RunState::Idle);
        let task = manager.get_by_id(id).await.expect("task still exists");
        assert_eq!(task.status, DownloadStatus::Paused);
        assert_eq!(manager.get_active_count().await, 0);
    }

    /// `remove_download` must wait out a concurrent `start_download`'s
    /// beforeRequest script round trip instead of racing it, then win: the
    /// download it just spawned is stopped and the task is gone, with no
    /// orphan handle left behind.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn remove_waits_for_in_flight_start() {
        let manager = DownloadManager::new();
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let (script_tx, script_rx) = mpsc::channel::<ScriptRequest>();
        // Never join this thread: nothing else drains the channel after the
        // test's assertions run, and the retry loop's own clone would keep
        // it open, so joining would hang.
        let _responder = std::thread::spawn(move || {
            while let Ok(req) = script_rx.recv() {
                if let ScriptRequest::BeforeRequest { ctx, response, .. } = req {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let _ = response.send((ctx, Ok(())));
                }
            }
        });

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        let manager_start = manager.clone();
        let start_handle = tokio::spawn(async move {
            manager_start.start_download(id, Some(script_tx), config).await
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

        let remove_result = manager.remove_download(id).await;
        match remove_result {
            Ok(Some(_)) => {}
            other => panic!("expected Ok(Some(_)), got {other:?}"),
        }

        let start_outcome = start_handle.await.unwrap().expect("start must not error");
        assert_eq!(start_outcome, StartOutcome::Started);

        assert!(manager.get_by_id(id).await.is_none(), "task must be gone");
        assert_eq!(manager.run_state(id).await, RunState::Idle);
        assert_eq!(manager.get_active_count().await, 0);
    }

    /// A remove that cannot take the claim within the budget must leave the
    /// task untouched and report `TaskBusy`.
    #[tokio::test]
    async fn remove_returns_busy_after_budget() {
        let mut manager = DownloadManager::new();
        manager.set_claim_wait_budget(std::time::Duration::from_millis(100));

        let task = DownloadTask::new("https://example.com/f".to_string(), "/tmp".into());
        let id = task.id;
        manager.add_download(task).await;

        let claim = StartClaim::try_acquire(&manager.start_claims, id).expect("claim must be free");

        let result = manager.remove_download(id).await;
        match result {
            Err(TaskBusy(busy_id)) => assert_eq!(busy_id, id),
            other => panic!("expected TaskBusy, got {other:?}"),
        }

        assert!(
            manager.get_by_id(id).await.is_some(),
            "a busy remove must not change the task"
        );

        drop(claim);
    }

    /// `pause_all` uses the in-flight rule, so a task sleeping in retry
    /// backoff (persisted status `Paused`, live handle) is paused too.
    #[tokio::test]
    async fn pause_all_pauses_retry_backoff_task() {
        let manager = DownloadManager::with_retry_settings(3, 60);
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        assert_eq!(
            manager
                .start_download(id, None, config)
                .await
                .expect("start must succeed"),
            StartOutcome::Started
        );

        let deadline = tokio::time::Instant::now() + tokio::time::Duration::from_secs(10);
        loop {
            if manager.run_state(id).await == RunState::RetryBackoff {
                break;
            }
            assert!(
                tokio::time::Instant::now() < deadline,
                "download did not reach RetryBackoff within 10s"
            );
            tokio::time::sleep(tokio::time::Duration::from_millis(50)).await;
        }

        assert_eq!(manager.pause_all().await, 1);

        assert_eq!(manager.run_state(id).await, RunState::Idle);
        let task = manager.get_by_id(id).await.expect("task still exists");
        assert_eq!(task.status, DownloadStatus::Paused);
    }

    /// `stop_folder_tasks` counts a task whose claim is currently held by an
    /// in-progress start (its beforeRequest script still running) as in
    /// flight, and pauses the download that start ends up spawning -- the
    /// task's persisted status is still `Pending` at the moment the batch
    /// pause takes its snapshot.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn stop_folder_tasks_pauses_claim_held_task() {
        let manager = DownloadManager::new();
        let mut task = DownloadTask::new("http://127.0.0.1:1/x".to_string(), "/tmp".into());
        task.folder_id = "folder1".to_string();
        let id = task.id;
        manager.add_download(task).await;

        let (script_tx, script_rx) = mpsc::channel::<ScriptRequest>();
        // Never join this thread: nothing else drains the channel after the
        // test's assertions run, and the retry loop's own clone would keep
        // it open, so joining would hang.
        let _responder = std::thread::spawn(move || {
            while let Ok(req) = script_rx.recv() {
                if let ScriptRequest::BeforeRequest { ctx, response, .. } = req {
                    std::thread::sleep(std::time::Duration::from_millis(300));
                    let _ = response.send((ctx, Ok(())));
                }
            }
        });

        let config = Arc::new(tokio::sync::RwLock::new(Config::default()));
        let manager_start = manager.clone();
        let start_handle = tokio::spawn(async move {
            manager_start.start_download(id, Some(script_tx), config).await
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

        let stopped = manager.stop_folder_tasks("folder1").await;
        assert_eq!(stopped, 1, "the claim-held task must be counted as in flight");

        let start_outcome = start_handle.await.unwrap().expect("start must not error");
        assert_eq!(start_outcome, StartOutcome::Started);

        let task = manager.get_by_id(id).await.expect("task still queued");
        assert_eq!(task.status, DownloadStatus::Paused);
        assert_eq!(manager.get_active_count().await, 0);
    }

    /// A batch pause only touches in-flight tasks: a `Pending` task and a
    /// task already `Paused` with no live handle are left alone.
    #[tokio::test]
    async fn stop_all_tasks_skips_idle_tasks() {
        let manager = DownloadManager::new();

        let pending = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        let pending_id = pending.id;
        manager.add_download(pending).await;

        let mut paused = DownloadTask::new("https://example.com/b.bin".to_string(), "/tmp".into());
        paused.status = DownloadStatus::Paused;
        let paused_id = paused.id;
        manager.add_download(paused).await;

        assert_eq!(manager.stop_all_tasks().await, 0);

        assert_eq!(
            manager.get_by_id(pending_id).await.expect("task exists").status,
            DownloadStatus::Pending
        );
        assert_eq!(
            manager.get_by_id(paused_id).await.expect("task exists").status,
            DownloadStatus::Paused
        );
    }

    /// `pause_if_in_flight` re-checks after taking the claim: a task that
    /// left the in-flight set between the batch's snapshot and this call
    /// (e.g. a backoff task whose retries were just exhausted, leaving it
    /// `Error` with no live handle) must not have its terminal status
    /// overwritten with `Paused`, and must not be counted as paused.
    #[tokio::test]
    async fn pause_if_in_flight_skips_task_no_longer_in_flight() {
        let manager = DownloadManager::new();

        let mut task = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        task.status = DownloadStatus::Error;
        let id = task.id;
        manager.add_download(task).await;

        match manager.pause_if_in_flight(id).await {
            Ok(false) => {}
            other => panic!("expected Ok(false) (not in flight), got {other:?}"),
        }

        assert_eq!(
            manager.get_by_id(id).await.expect("task exists").status,
            DownloadStatus::Error,
            "a stale candidate must not have its terminal status overwritten"
        );
    }

    /// `remove_many` must run its per-id claim waits concurrently: three busy
    /// ids at a 300ms budget must together take well under 3 * 300ms.
    #[tokio::test]
    async fn remove_many_busy_ids_waits_one_budget() {
        let mut manager = DownloadManager::new();
        manager.set_claim_wait_budget(std::time::Duration::from_millis(300));

        let mut ids = Vec::new();
        let mut claims = Vec::new();
        for i in 0..3 {
            let task = DownloadTask::new(format!("https://example.com/{i}.bin"), "/tmp".into());
            let id = task.id;
            manager.add_download(task).await;
            claims.push(StartClaim::try_acquire(&manager.start_claims, id).expect("claim must be free"));
            ids.push(id);
        }

        let start = tokio::time::Instant::now();
        let results = manager.remove_many(&ids).await;
        let elapsed = start.elapsed();

        assert_eq!(results.len(), 3);
        for result in results {
            match result {
                Err(TaskBusy(_)) => {}
                other => panic!("expected TaskBusy, got {other:?}"),
            }
        }
        assert!(
            elapsed < std::time::Duration::from_millis(800),
            "batch remove of busy ids took {elapsed:?}, expected one shared claim-wait budget"
        );

        drop(claims);
    }

    /// `pause_many_if_in_flight` skips a task that isn't in flight, reporting
    /// `Ok(false)` without changing its status.
    #[tokio::test]
    async fn pause_many_skips_not_in_flight() {
        let manager = DownloadManager::new();

        let task = DownloadTask::new("https://example.com/a.bin".to_string(), "/tmp".into());
        let id = task.id;
        manager.add_download(task).await;

        let results = manager.pause_many_if_in_flight(&[id]).await;
        match results.as_slice() {
            [Ok(false)] => {}
            other => panic!("expected [Ok(false)], got {other:?}"),
        }

        assert_eq!(
            manager.get_by_id(id).await.expect("task exists").status,
            DownloadStatus::Pending
        );
    }
}
