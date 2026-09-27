//! Shared logic for turning a URL into a queued (and maybe started) task.
//! Used by the TUI, the standalone CLI, and the daemon so folder rules
//! (duplicate prevention, auto-start, header/referrer defaults) are applied
//! the same way everywhere.

use super::manager::DownloadManager;
use super::task::DownloadTask;
use crate::app::config::Config;
use crate::app::state::AppState;
use uuid::Uuid;

/// Header/referrer overrides applied on top of a folder's defaults.
#[derive(Debug, Clone, Default)]
pub struct EnqueueOptions {
    pub referrer: Option<String>,
    pub headers: Vec<(String, String)>,
}

/// Result of queuing a single task.
#[derive(Debug, Clone)]
pub enum EnqueueOutcome {
    Added {
        id: Uuid,
        started: bool,
        start_error: Option<String>,
    },
    Duplicate,
}

/// Resolve a folder identifier: accepts either the UUID key or the display
/// name.
pub fn resolve_folder_id(config: &Config, id: &str) -> Option<String> {
    // Try direct key match first (UUID)
    if config.folders.contains_key(id) {
        return Some(id.to_string());
    }
    // Fallback: search by display name
    config.find_folder_id_by_name(id)
}

/// Resolve the folder a new task should land in. `None` picks the folder
/// named "default", falling back to the first folder in sorted order, and
/// finally to the literal string "default" (matching `DownloadTask::new`'s
/// own default when no folders exist at all).
pub fn resolve_target_folder(config: &Config, folder: Option<&str>) -> anyhow::Result<String> {
    match folder {
        Some(id) => {
            resolve_folder_id(config, id).ok_or_else(|| anyhow::anyhow!("unknown folder"))
        }
        None => {
            if let Some(id) = config.find_folder_id_by_name("default") {
                Ok(id)
            } else if let Some((id, _)) = config.sorted_folder_entries().into_iter().next() {
                Ok(id)
            } else {
                Ok("default".to_string())
            }
        }
    }
}

/// Build a task for `folder_id`, applying the folder's defaults first and
/// then the caller's header/referrer overrides on top.
pub fn build_task(
    url: &str,
    folder_id: &str,
    config: &Config,
    opts: &EnqueueOptions,
) -> DownloadTask {
    let mut task = DownloadTask::new_with_folder(url.to_string(), folder_id.to_string(), config);
    for (name, value) in &opts.headers {
        task.set_header(name, value);
    }
    if let Some(referrer) = &opts.referrer {
        task.set_header("Referer", referrer);
    }
    task
}

/// Add `task` to its folder's queue (honoring `prevent_duplicate_url`), then
/// start it when starting is allowed and either the folder auto-starts or the
/// caller asked for an immediate start. Never saves the queue; callers decide
/// when to persist.
pub async fn enqueue(
    manager: &DownloadManager,
    state: &AppState,
    task: DownloadTask,
    immediate: bool,
    allow_start: bool,
) -> EnqueueOutcome {
    let id = task.id;
    let folder_id = task.folder_id.clone();

    let (prevent_duplicate, auto_start) = {
        let config = state.config.read().await;
        let folder = config.folders.get(&folder_id);
        (
            folder.map(|f| f.prevent_duplicate_url).unwrap_or(false),
            folder.map(|f| f.auto_start_downloads).unwrap_or(false),
        )
    };

    if prevent_duplicate {
        if !manager.add_download_if_unique(task).await {
            tracing::info!("Skipped duplicate URL in folder '{}'", folder_id);
            return EnqueueOutcome::Duplicate;
        }
    } else {
        manager.add_download(task).await;
    }

    let mut started = false;
    let mut start_error = None;
    if allow_start && (auto_start || immediate) {
        match manager
            .start_download(id, state.script_sender.clone(), state.config.clone())
            .await
        {
            // `AlreadyRunning`/`Busy` for a task added a moment ago means the
            // auto-start sweep is already starting it, so this still counts
            // as started.
            Ok(_) => started = true,
            // A start failure must never abort an add; the task just stays queued.
            // Callers report `start_error` themselves, so log at debug here to
            // avoid double-logging the same failure.
            Err(e) => {
                tracing::debug!("Start failed in folder '{}': {}", folder_id, e);
                start_error = Some(e.to_string());
            }
        }
    }

    EnqueueOutcome::Added {
        id,
        started,
        start_error,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::Config;
    use crate::app::state::AppState;
    use crate::download::task::DownloadStatus;

    fn config_with_folder(name: &str, auto_start: bool, prevent_duplicate: bool) -> Config {
        let mut config = Config::default();
        let folder_id = name.to_string();
        let mut folder = config
            .folders
            .get("default")
            .cloned()
            .unwrap_or_default();
        folder.name = name.to_string();
        folder.auto_start_downloads = auto_start;
        folder.prevent_duplicate_url = prevent_duplicate;
        config.folders.clear();
        config.folders.insert(folder_id, folder);
        config
    }

    #[test]
    fn resolve_target_folder_by_name_and_key() {
        let mut config = Config::default();
        let mut folder = crate::app::config::FolderConfig::default();
        folder.name = "downloads".to_string();
        config.folders.insert("f-123".to_string(), folder);

        // By key.
        assert_eq!(
            resolve_target_folder(&config, Some("f-123")).unwrap(),
            "f-123"
        );
        // By display name, resolving to the same key.
        assert_eq!(
            resolve_target_folder(&config, Some("downloads")).unwrap(),
            "f-123"
        );
        // Unknown folder is an error.
        assert!(resolve_target_folder(&config, Some("missing")).is_err());
    }

    #[test]
    fn resolve_target_folder_none_falls_back() {
        let config = config_with_folder("downloads", false, false);
        // No folder named "default" exists, so the single folder wins.
        assert_eq!(resolve_target_folder(&config, None).unwrap(), "downloads");
    }

    #[test]
    fn build_task_applies_folder_defaults_then_overrides() {
        let mut config = config_with_folder("downloads", false, false);
        config
            .folders
            .get_mut("downloads")
            .unwrap()
            .default_headers
            .insert("X-Default".to_string(), "1".to_string());

        let opts = EnqueueOptions {
            referrer: Some("https://example.org/".to_string()),
            headers: vec![("X-Extra".to_string(), "2".to_string())],
        };
        let task = build_task("https://example.com/f.zip", "downloads", &config, &opts);

        assert_eq!(task.headers.get("X-Default").map(String::as_str), Some("1"));
        assert_eq!(task.headers.get("X-Extra").map(String::as_str), Some("2"));
        assert_eq!(task.headers.get("Referer").map(String::as_str), Some("https://example.org/"));
    }

    #[tokio::test]
    async fn enqueue_respects_prevent_duplicate() {
        let config = config_with_folder("downloads", false, true);
        let state = AppState::new(config.clone(), "en");
        let manager = crate::download::manager::DownloadManager::new();

        let opts = EnqueueOptions::default();
        let task1 = build_task("https://example.com/f.zip", "downloads", &config, &opts);
        let task2 = build_task("https://example.com/f.zip", "downloads", &config, &opts);

        let outcome1 = enqueue(&manager, &state, task1, false, true).await;
        assert!(matches!(outcome1, EnqueueOutcome::Added { .. }));

        let outcome2 = enqueue(&manager, &state, task2, false, true).await;
        assert!(matches!(outcome2, EnqueueOutcome::Duplicate));
    }

    #[tokio::test]
    async fn enqueue_does_not_start_when_not_allowed() {
        let config = config_with_folder("downloads", true, false);
        let state = AppState::new(config.clone(), "en");
        let manager = crate::download::manager::DownloadManager::new();

        let opts = EnqueueOptions::default();
        let task = build_task("https://example.com/f.zip", "downloads", &config, &opts);
        let id = task.id;

        let outcome = enqueue(&manager, &state, task, false, false).await;
        match outcome {
            EnqueueOutcome::Added { started, .. } => assert!(!started),
            EnqueueOutcome::Duplicate => panic!("unexpected duplicate"),
        }

        let stored = manager.get_by_id(id).await.expect("task present");
        assert_eq!(stored.status, DownloadStatus::Pending);
    }
}
