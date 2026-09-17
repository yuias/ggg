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
use std::sync::{mpsc, Arc};
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

#[derive(Clone)]
pub struct DownloadManager {
    /// Per-folder download queues
    folder_queues: Arc<RwLock<HashMap<String, FolderQueue>>>,

    http_client: Arc<HttpClient>,
    active_downloads: Arc<RwLock<HashMap<Uuid, JoinHandle<()>>>>,

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

    pub async fn remove_download(&self, id: Uuid) -> Option<DownloadTask> {
        // Cancel active download if running. Aborting drops the spawned task
        // without running its cleanup, so the folder slot is released below.
        if let Some(handle) = self.active_downloads.write().await.remove(&id) {
            handle.abort();
        }

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

    pub async fn start_download(
        &self,
        id: Uuid,
        script_sender: Option<mpsc::Sender<ScriptRequest>>,
        config: Arc<tokio::sync::RwLock<crate::app::config::Config>>,
    ) -> Result<()> {
        let mut task = self.get_by_id(id).await
            .ok_or_else(|| anyhow::anyhow!("Task not found"))?;

        if task.status == DownloadStatus::Downloading {
            return Ok(()); // Already downloading
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

        let handle = tokio::spawn(async move {
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
                            queue.update(current_task.clone()).await;

                            // Wait before retry with exponential backoff
                            tokio::time::sleep(tokio::time::Duration::from_secs(backoff_delay)).await;

                            // Prepare for retry
                            current_task.status = DownloadStatus::Downloading;
                            current_task.error_message = None;
                            queue.update(current_task.clone()).await;
                        } else {
                            // Max retries exceeded, mark as error
                            current_task.status = DownloadStatus::Error;
                            current_task.log_error(format!("Max retries ({}) exceeded", max_retries));
                            queue.update(current_task.clone()).await;
                            manager_for_cleanup.emit(DownloadEvent::Failed {
                                task_id: id,
                                error: current_task.error_message.clone().unwrap_or_default(),
                            });

                            // Record failure for circuit breaker
                            if let Some(domain) = super::circuit_breaker::extract_domain(&task_url) {
                                circuit_breaker.record_failure(&domain);
                            }

                            // Hook Point 4: error - Error handling (fire-and-forget)
                            if let Some(ref sender) = script_sender_for_error {
                                // Compute effective script_files
                                let effective_script_files = Self::compute_effective_script_files(&config, &current_task.folder_id).await;

                                let ctx = crate::script::events::ErrorContext {
                                    url: current_task.url.clone(),
                                    filename: Some(current_task.filename.clone()),
                                    error: current_task.error_message.as_deref().unwrap_or("Unknown error").to_string(),
                                    retry_count: current_task.retry_count,
                                    status_code: current_task.last_status_code,
                                };

                                // Fire-and-forget (no need to wait for response)
                                let sender_clone = (*sender).clone();
                                tokio::task::spawn_blocking(move || {
                                    if let Err(e) = sender_clone.send(ScriptRequest::Error {
                                        ctx,
                                        effective_script_files,
                                    }) {
                                        tracing::error!("Failed to send error hook: {}", e);
                                    }
                                });
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
        });

        self.active_downloads.write().await.insert(id, handle);
        self.emit(DownloadEvent::Started { task_id: id });

        Ok(())
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

    pub async fn pause_download(&self, id: Uuid) -> Result<()> {
        // Abort the download task. Aborting drops the spawned future, so its
        // own cleanup never runs — this function releases bookkeeping itself.
        if let Some(handle) = self.active_downloads.write().await.remove(&id) {
            handle.abort();
        }

        // Update status and counts
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

        Ok(())
    }

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
        // pause below would interrupt its download for no reason.
        if current.folder_id == new_folder_id {
            return Ok(());
        }

        // An in-flight download holds a clone of its source folder's queue, so
        // moving it would send every later count update to the wrong queue and
        // leave it stuck `Downloading`. Pause first: that aborts the spawned
        // future and settles the source queue's counts before the move.
        if current.status == DownloadStatus::Downloading {
            self.pause_download(id).await?;
        }

        // Find and remove from old folder queue
        let task = {
            let queues = self.folder_queues.read().await;
            let mut found_task = None;
            for queue in queues.values() {
                if let Some(t) = queue.remove(id).await {
                    found_task = Some(t);
                    break;
                }
            }
            found_task
        };

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
            if self.start_download(id, script_sender.clone(), config.clone()).await.is_ok() {
                resumed += 1;
            }
        }
        resumed
    }

    /// Pause all currently downloading tasks
    /// Returns the number of downloads paused
    pub async fn pause_all(&self) -> usize {
        let downloads = self.get_all_downloads().await;
        let active: Vec<Uuid> = downloads
            .iter()
            .filter(|t| t.status == DownloadStatus::Downloading)
            .map(|t| t.id)
            .collect();
        
        let mut paused = 0;
        for id in active {
            if self.pause_download(id).await.is_ok() {
                paused += 1;
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

    pub async fn get_active_count(&self) -> usize {
        self.active_downloads.read().await.len()
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

    /// Check if there are any active downloads
    pub async fn has_active_downloads(&self) -> bool {
        !self.active_downloads.read().await.is_empty()
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
            if self.start_download(task.id, script_sender.clone(), config.clone()).await.is_ok() {
                started += 1;
            }
        }

        started
    }

    /// Stop (pause) all downloading tasks in a specific folder
    /// Returns the number of tasks stopped
    pub async fn stop_folder_tasks(&self, folder_id: &str) -> usize {
        let queue = match self.get_folder_queue(folder_id).await {
            Some(q) => q,
            None => return 0,
        };

        let all_tasks = queue.get_all().await;
        let downloading: Vec<Uuid> = all_tasks
            .iter()
            .filter(|t| t.status == DownloadStatus::Downloading)
            .map(|t| t.id)
            .collect();

        let mut stopped = 0;
        for id in downloading {
            if self.pause_download(id).await.is_ok() {
                stopped += 1;
            }
        }

        stopped
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
            if self.start_download(id, script_sender.clone(), config.clone()).await.is_ok() {
                started += 1;
            }
        }

        started
    }

    /// Stop all downloading tasks across all folders
    /// Returns the number of tasks stopped
    pub async fn stop_all_tasks(&self) -> usize {
        let downloads = self.get_all_downloads().await;
        let downloading: Vec<Uuid> = downloads
            .iter()
            .filter(|t| t.status == DownloadStatus::Downloading)
            .map(|t| t.id)
            .collect();

        let mut stopped = 0;
        for id in downloading {
            if self.pause_download(id).await.is_ok() {
                stopped += 1;
            }
        }

        stopped
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

        manager.remove_download(id).await;
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

        // Should return None for non-existent task
        assert!(result.is_none());
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
}
