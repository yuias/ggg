use super::config::{Config, FolderConfig, ReferrerPolicy};
use crate::download::task::DownloadTask;
use std::collections::HashMap;
use std::path::PathBuf;

/// Resolved settings after applying inheritance: Queue > Folder > Application
#[derive(Debug, Clone)]
pub struct ResolvedSettings {
    pub save_path: PathBuf,
    pub user_agent: String,
    pub headers: HashMap<String, String>,
    pub max_concurrent: usize,
    pub scripts_enabled: bool,
    pub retry_count: u32,
    pub max_redirects: u32,
    pub referrer_policy: ReferrerPolicy,
}

impl ResolvedSettings {
    /// Resolve settings for a task by applying inheritance chain:
    /// Queue (task-specific) > Folder > Application
    pub fn resolve(config: &Config, folder_id: &str, task: &DownloadTask) -> Self {
        let folder_config = config.folders.get(folder_id);

        // Resolve save_path with auto-date directory logic
        let save_path = Self::resolve_save_path(config, folder_config, task);

        // Resolve user_agent: task > folder > app
        let user_agent = task
            .user_agent
            .clone()
            .or_else(|| folder_config.and_then(|f| f.user_agent.clone()))
            .unwrap_or_else(|| config.download.user_agent.clone());

        // Resolve headers: merge folder defaults with task overrides
        let mut headers = folder_config
            .map(|f| f.default_headers.clone())
            .unwrap_or_default();
        headers.extend(task.headers.clone());

        // Resolve max_concurrent: folder > app-level per-folder > app global
        let max_concurrent = folder_config
            .and_then(|f| f.max_concurrent)
            .or(config.download.max_concurrent_per_folder)
            .unwrap_or(config.download.max_concurrent);

        // Resolve scripts_enabled with validation
        let scripts_enabled = Self::resolve_scripts_enabled(config, folder_config);

        // Resolve referrer_policy: folder > app
        let referrer_policy = folder_config
            .and_then(|f| f.referrer_policy.clone())
            .unwrap_or_else(|| config.download.referrer_policy.clone());

        Self {
            save_path,
            user_agent,
            headers,
            max_concurrent,
            scripts_enabled,
            retry_count: config.download.retry_count,
            max_redirects: config.download.max_redirects,
            referrer_policy,
        }
    }

    fn resolve_save_path(
        config: &Config,
        folder_config: Option<&FolderConfig>,
        task: &DownloadTask,
    ) -> PathBuf {
        // Determine the expected base path from folder config or app default
        let base_path = folder_config
            .map(|f| f.save_path.clone())
            .unwrap_or_else(|| config.download.default_directory.clone());

        // If task has an explicit save_path override (different from both the
        // folder base path and the app default), honour it as-is.
        // This handles script overrides and manual path changes (queue level override).
        if task.save_path != base_path && task.save_path != config.download.default_directory {
            return task.save_path.clone();
        }

        // Apply auto-date directory if enabled for this folder
        if folder_config
            .map(|f| f.auto_date_directory)
            .unwrap_or(false)
        {
            let date_str = task.created_at.with_timezone(&chrono::Local).format("%Y%m%d").to_string();
            base_path.join(date_str)
        } else {
            base_path
        }
    }

    fn resolve_scripts_enabled(
        config: &Config,
        folder_config: Option<&FolderConfig>,
    ) -> bool {
        // Validation: folder cannot enable scripts if app disables them
        if !config.scripts.enabled {
            return false;
        }

        // If folder overrides, use that (but already validated above)
        folder_config
            .and_then(|f| f.scripts_enabled)
            .unwrap_or(config.scripts.enabled)
    }
}

/// Validation errors for folder configuration
#[derive(Debug)]
pub enum ValidationError {
    /// Folder tries to enable scripts when app-level is disabled
    ScriptsEnabledWhenDisabled(String),
    /// Concurrent download constraint violated: (per_folder * active_folders) > global
    ConcurrentDownloadConstraintViolation {
        max_concurrent: usize,
        max_concurrent_per_folder: usize,
        parallel_folder_count: usize,
        calculated: usize,
    },
    /// Folder max_concurrent exceeds application max_concurrent
    FolderMaxConcurrentExceedsApp {
        folder_id: String,
        folder_max: usize,
        app_max: usize,
    },
}

impl std::fmt::Display for ValidationError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ValidationError::ScriptsEnabledWhenDisabled(folder_id) => {
                write!(
                    f,
                    "Folder '{}' cannot enable scripts when application-level scripts are disabled",
                    folder_id
                )
            }
            ValidationError::ConcurrentDownloadConstraintViolation {
                max_concurrent,
                max_concurrent_per_folder,
                parallel_folder_count,
                calculated,
            } => {
                write!(
                    f,
                    "Concurrent download constraint violated: (Max Per Folder: {} × Max Active Folders: {}) = {} > Max Concurrent Downloads: {}",
                    max_concurrent_per_folder,
                    parallel_folder_count,
                    calculated,
                    max_concurrent
                )
            }
            ValidationError::FolderMaxConcurrentExceedsApp {
                folder_id,
                folder_max,
                app_max,
            } => {
                write!(
                    f,
                    "Folder '{}' max concurrent downloads ({}) exceeds application limit ({})",
                    folder_id, folder_max, app_max
                )
            }
        }
    }
}

impl std::error::Error for ValidationError {}

/// Validate folder configuration
pub fn validate_folder_config(config: &Config) -> Result<(), Vec<ValidationError>> {
    let mut errors = Vec::new();

    // Validate concurrent download constraints at application level
    let max_concurrent = config.download.max_concurrent;
    if let (Some(max_per_folder), Some(parallel_count)) = (
        config.download.max_concurrent_per_folder,
        config.download.parallel_folder_count,
    ) {
        // Saturating: these are unbounded user-entered config values.
        let calculated = max_per_folder.saturating_mul(parallel_count);
        if calculated > max_concurrent {
            errors.push(ValidationError::ConcurrentDownloadConstraintViolation {
                max_concurrent,
                max_concurrent_per_folder: max_per_folder,
                parallel_folder_count: parallel_count,
                calculated,
            });
        }
    }

    // Validate each folder
    for (folder_id, folder_config) in &config.folders {
        // Check script validation rule
        if folder_config.scripts_enabled == Some(true) && !config.scripts.enabled {
            errors.push(ValidationError::ScriptsEnabledWhenDisabled(
                folder_id.clone(),
            ));
        }

        // Check folder max_concurrent doesn't exceed app max_concurrent
        if let Some(folder_max) = folder_config.max_concurrent {
            if folder_max > max_concurrent {
                errors.push(ValidationError::FolderMaxConcurrentExceedsApp {
                    folder_id: folder_id.clone(),
                    folder_max,
                    app_max: max_concurrent,
                });
            }
        }
    }

    if errors.is_empty() {
        Ok(())
    } else {
        Err(errors)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::{Config, DownloadConfig, FolderConfig, GeneralConfig, NetworkConfig, ScriptConfig};
    use chrono::Utc;
    use std::collections::HashMap;
    use std::path::PathBuf;
    use uuid::Uuid;

    fn create_test_config() -> Config {
        Config {
            general: GeneralConfig {
                language: "en".to_string(),
                theme: "dark".to_string(),
                minimize_to_tray: false,
                start_minimized: false,
                skip_download_preview: true,
                auto_launch_dnd: false,
            },
            download: DownloadConfig {
                default_directory: PathBuf::from("C:\\Downloads"),
                max_concurrent: 3,
                retry_count: 5,
                retry_delay: 3,
                user_agent: "TestAgent/1.0".to_string(),
                bandwidth_limit: 0,
                max_concurrent_per_folder: Some(2),
                parallel_folder_count: Some(2),
                max_redirects: 10,
                referrer_policy: ReferrerPolicy::default(),
                claim_wait_ms: 2000,
            },
            network: NetworkConfig {
                proxy_enabled: false,
                proxy_type: "http".to_string(),
                proxy_host: String::new(),
                proxy_port: 8080,
                proxy_auth: false,
                proxy_user: String::new(),
                proxy_pass: String::new(),
            },
            scripts: ScriptConfig {
                enabled: true,
                directory: PathBuf::from("./scripts"),
                timeout: 30,
                script_files: HashMap::new(),
            },
            ui: crate::app::config::UiConfig::default(),
            folders: HashMap::new(),
        }
    }

    fn create_test_task(url: String, save_path: PathBuf, folder_id: String) -> DownloadTask {
        DownloadTask {
            id: Uuid::new_v4(),
            url,
            filename: "test.zip".to_string(),
            save_path,
            folder_id,
            size: None,
            downloaded: 0,
            status: crate::download::task::DownloadStatus::Pending,
            priority: 0,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            headers: HashMap::new(),
            user_agent: None,
            resume_supported: false,
            etag: None,
            last_modified: None,
            error_message: None,
            logs: Vec::new(),
            last_status_code: None,
            retry_count: 0,
            filename_settled: false,
        }
    }

    #[test]
    fn test_settings_resolution_queue_override() {
        // Test: task.user_agent overrides folder and app
        let mut config = create_test_config();

        // Add folder with custom user agent
        config.folders.insert(
            "test_folder".to_string(),
            FolderConfig {
                name: String::new(),
                save_path: PathBuf::from("C:\\TestFolder"),
                auto_date_directory: false,
                auto_start_downloads: false,
                scripts_enabled: None,
                script_files: None,
                max_concurrent: None,
                user_agent: Some("FolderAgent/1.0".to_string()),
                referrer_policy: None,
                default_headers: HashMap::new(),
                prevent_duplicate_url: false,
            },
        );

        // Create task with its own user agent (queue-level override)
        let mut task = create_test_task(
            "https://example.com/file.zip".to_string(),
            PathBuf::from("C:\\TestFolder"),
            "test_folder".to_string(),
        );
        task.user_agent = Some("TaskAgent/1.0".to_string());

        let resolved = ResolvedSettings::resolve(&config, "test_folder", &task);

        // Queue-level user_agent should win
        assert_eq!(resolved.user_agent, "TaskAgent/1.0");
    }

    #[test]
    fn test_settings_resolution_folder_override() {
        // Test: folder.user_agent overrides app
        let mut config = create_test_config();

        config.folders.insert(
            "test_folder".to_string(),
            FolderConfig {
                name: String::new(),
                save_path: PathBuf::from("C:\\TestFolder"),
                auto_date_directory: false,
                auto_start_downloads: false,
                scripts_enabled: None,
                script_files: None,
                max_concurrent: None,
                user_agent: Some("FolderAgent/1.0".to_string()),
                referrer_policy: None,
                default_headers: HashMap::new(),
                prevent_duplicate_url: false,
            },
        );

        let task = create_test_task(
            "https://example.com/file.zip".to_string(),
            PathBuf::from("C:\\TestFolder"),
            "test_folder".to_string(),
        );

        let resolved = ResolvedSettings::resolve(&config, "test_folder", &task);

        // Folder-level user_agent should override app-level
        assert_eq!(resolved.user_agent, "FolderAgent/1.0");
    }

    #[test]
    fn test_settings_resolution_app_fallback() {
        // Test: uses app default when no overrides
        let config = create_test_config();

        let task = create_test_task(
            "https://example.com/file.zip".to_string(),
            PathBuf::from("C:\\Downloads"),
            "nonexistent_folder".to_string(),
        );

        let resolved = ResolvedSettings::resolve(&config, "nonexistent_folder", &task);

        // Should fall back to app-level user_agent
        assert_eq!(resolved.user_agent, "TestAgent/1.0");
        assert_eq!(resolved.retry_count, 5);
        assert_eq!(resolved.max_redirects, 10);
    }

    #[test]
    fn test_auto_date_directory() {
        // Test: YYYYMMDD appended to folder save_path
        let mut config = create_test_config();

        config.folders.insert(
            "test_folder".to_string(),
            FolderConfig {
                name: String::new(),
                save_path: PathBuf::from("C:\\TestFolder"),
                auto_date_directory: true,
                auto_start_downloads: false,
                scripts_enabled: None,
                script_files: None,
                max_concurrent: None,
                user_agent: None,
                referrer_policy: None,
                default_headers: HashMap::new(),
                prevent_duplicate_url: false,
            },
        );

        let task = create_test_task(
            "https://example.com/file.zip".to_string(),
            config.download.default_directory.clone(), // Not explicitly set, will use folder default
            "test_folder".to_string(),
        );

        let resolved = ResolvedSettings::resolve(&config, "test_folder", &task);

        // Should have date directory appended
        let date_str = task.created_at.with_timezone(&chrono::Local).format("%Y%m%d").to_string();
        let expected_path = PathBuf::from("C:\\TestFolder").join(date_str);
        assert_eq!(resolved.save_path, expected_path);
    }

    #[test]
    fn test_validation_scripts_disabled() {
        // Test: folder cannot enable scripts when app disables
        let mut config = create_test_config();
        config.scripts.enabled = false; // Disable scripts at app level
        // Fix concurrent downloads validation to avoid interfering with this test
        config.download.max_concurrent_per_folder = None;
        config.download.parallel_folder_count = None;

        config.folders.insert(
            "bad_folder".to_string(),
            FolderConfig {
                name: String::new(),
                save_path: PathBuf::from("C:\\BadFolder"),
                auto_date_directory: false,
                auto_start_downloads: false,
                scripts_enabled: Some(true), // Try to enable at folder level
                script_files: None,
                max_concurrent: None,
                user_agent: None,
                referrer_policy: None,
                default_headers: HashMap::new(),
                prevent_duplicate_url: false,
            },
        );

        let result = validate_folder_config(&config);
        assert!(result.is_err());

        if let Err(errors) = result {
            assert_eq!(errors.len(), 1);
            assert!(matches!(errors[0], ValidationError::ScriptsEnabledWhenDisabled(_)));
        }
    }

    #[test]
    fn test_validation_scripts_allowed_when_app_enabled() {
        // Test: folder can enable/disable scripts when app has them enabled
        let mut config = create_test_config();
        config.scripts.enabled = true;
        // Fix concurrent downloads validation to avoid interfering with this test
        config.download.max_concurrent_per_folder = None;
        config.download.parallel_folder_count = None;

        config.folders.insert(
            "folder_scripts_on".to_string(),
            FolderConfig {
                name: String::new(),
                save_path: PathBuf::from("C:\\Folder1"),
                auto_date_directory: false,
                auto_start_downloads: false,
                scripts_enabled: Some(true),
                script_files: None,
                max_concurrent: None,
                user_agent: None,
                referrer_policy: None,
                default_headers: HashMap::new(),
                prevent_duplicate_url: false,
            },
        );

        config.folders.insert(
            "folder_scripts_off".to_string(),
            FolderConfig {
                name: String::new(),
                save_path: PathBuf::from("C:\\Folder2"),
                auto_date_directory: false,
                auto_start_downloads: false,
                scripts_enabled: Some(false),
                script_files: None,
                max_concurrent: None,
                user_agent: None,
                referrer_policy: None,
                default_headers: HashMap::new(),
                prevent_duplicate_url: false,
            },
        );

        let result = validate_folder_config(&config);
        assert!(result.is_ok());
    }

    #[test]
    fn test_headers_merging() {
        // Test: task headers merged with folder defaults
        let mut config = create_test_config();

        let mut folder_headers = HashMap::new();
        folder_headers.insert("referer".to_string(), "https://folder.example.com".to_string());
        folder_headers.insert("x-custom".to_string(), "folder-value".to_string());

        config.folders.insert(
            "test_folder".to_string(),
            FolderConfig {
                name: String::new(),
                save_path: PathBuf::from("C:\\TestFolder"),
                auto_date_directory: false,
                auto_start_downloads: false,
                scripts_enabled: None,
                script_files: None,
                max_concurrent: None,
                user_agent: None,
                referrer_policy: None,
                default_headers: folder_headers,
                prevent_duplicate_url: false,
            },
        );

        let mut task = create_test_task(
            "https://example.com/file.zip".to_string(),
            PathBuf::from("C:\\TestFolder"),
            "test_folder".to_string(),
        );

        // Task overrides referer
        task.headers.insert("referer".to_string(), "https://task.example.com".to_string());

        let resolved = ResolvedSettings::resolve(&config, "test_folder", &task);

        // Task header should override folder header
        assert_eq!(resolved.headers.get("referer"), Some(&"https://task.example.com".to_string()));
        // Folder header should be inherited
        assert_eq!(resolved.headers.get("x-custom"), Some(&"folder-value".to_string()));
    }

    #[test]
    fn test_max_concurrent_resolution() {
        // Test: max_concurrent resolves folder > app-per-folder > app-global
        let mut config = create_test_config();
        config.download.max_concurrent = 10;
        config.download.max_concurrent_per_folder = Some(5);

        // Folder with explicit max_concurrent
        config.folders.insert(
            "folder_with_max".to_string(),
            FolderConfig {
                name: String::new(),
                save_path: PathBuf::from("C:\\Folder1"),
                auto_date_directory: false,
                auto_start_downloads: false,
                scripts_enabled: None,
                script_files: None,
                max_concurrent: Some(2),
                user_agent: None,
                referrer_policy: None,
                default_headers: HashMap::new(),
                prevent_duplicate_url: false,
            },
        );

        // Folder without explicit max_concurrent
        config.folders.insert(
            "folder_without_max".to_string(),
            FolderConfig {
                name: String::new(),
                save_path: PathBuf::from("C:\\Folder2"),
                auto_date_directory: false,
                auto_start_downloads: false,
                scripts_enabled: None,
                script_files: None,
                max_concurrent: None,
                user_agent: None,
                referrer_policy: None,
                default_headers: HashMap::new(),
                prevent_duplicate_url: false,
            },
        );

        let task1 = create_test_task(
            "https://example.com/file1.zip".to_string(),
            PathBuf::from("C:\\Folder1"),
            "folder_with_max".to_string(),
        );

        let task2 = create_test_task(
            "https://example.com/file2.zip".to_string(),
            PathBuf::from("C:\\Folder2"),
            "folder_without_max".to_string(),
        );

        let resolved1 = ResolvedSettings::resolve(&config, "folder_with_max", &task1);
        let resolved2 = ResolvedSettings::resolve(&config, "folder_without_max", &task2);

        // Folder-level should override
        assert_eq!(resolved1.max_concurrent, 2);
        // Should use app-level per-folder
        assert_eq!(resolved2.max_concurrent, 5);
    }
}
