use crate::AppState;
use crate::app::config::ReferrerPolicy;
use crate::download::{manager::DownloadManager, task::DownloadTask};
use serde::{Deserialize, Serialize};

/// Commands that can be invoked from the TUI
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "camelCase")]
pub enum Command {
    AddDownload { urls: Vec<String> },
    StartDownload { id: String },
    PauseDownload { id: String },
    GetDownloads,
    RemoveDownload { id: String },
    ChangeFolder { id: String, folder_id: String },
    GetConfig,
    UpdateConfig { config: serde_json::Value },

    // Application-level settings
    UpdateMaxConcurrent { value: usize },
    UpdateMaxConcurrentPerFolder { value: Option<usize> },
    UpdateMaxActiveFolders { value: Option<usize> },
    UpdateMaxRedirects { value: u32 },
    UpdateRetryCount { value: u32 },
    UpdateScriptsEnabled { value: bool },
    UpdateSkipDownloadPreview { value: bool },
    UpdateAutoLaunchDnd { value: bool },
    UpdateLanguage { value: String },
    UpdateUserAgent { value: String },
    UpdateReferrerPolicy { policy: ReferrerPolicy },

    // Folder-level settings
    UpdateFolderMaxConcurrent { folder_id: String, value: Option<usize> },
    UpdateFolderUserAgent { folder_id: String, value: Option<String> },
    UpdateFolderReferrerPolicy { folder_id: String, policy: Option<ReferrerPolicy> },

    // Script settings
    ToggleScriptFile { filename: String },
    ToggleFolderScriptFile { folder_id: String, filename: String },
    ReloadScripts,

    // Config management
    ReloadConfig,
}

/// UI-agnostic error returned by [`handle_command`].
///
/// Variants carry machine-readable data rather than pre-translated strings, so
/// the command layer stays free of any i18n / presentation dependency. The
/// display layer is responsible for turning a `CommandError` into a localized,
/// user-facing message (the TUI does this in `tui::localize_command_error`).
/// The [`std::fmt::Display`] impl provides a plain-English fallback suitable for
/// logging or for consumers without a translation catalog.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum CommandError {
    /// The supplied download id was not a valid UUID.
    InvalidUuid,
    /// The supplied configuration payload could not be deserialized.
    InvalidConfig,
    /// A scripting operation was requested while scripting is disabled.
    ScriptsDisabled,
    /// A config reload was requested while downloads are still active.
    ReloadActiveDownloads,
    StartDownload { error: String },
    PauseDownload { error: String },
    RemoveDownload { error: String },
    ChangeFolder { error: String },
    ValidationFailed { error: String },
    SaveConfig { error: String },
    FolderNotFound { folder: String },
    ReloadScripts { error: String },
    ScriptCommunication { error: String },
    BlockingTask { error: String },
    ReloadConfig { error: String },
    /// Internal serialization failure (developer-facing, not localized).
    Serialization { error: String },
}

impl std::fmt::Display for CommandError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CommandError::InvalidUuid => write!(f, "Invalid download ID"),
            CommandError::InvalidConfig => write!(f, "Invalid configuration"),
            CommandError::ScriptsDisabled => write!(f, "Scripting is disabled"),
            CommandError::ReloadActiveDownloads => {
                write!(f, "Cannot reload configuration while downloads are active")
            }
            CommandError::StartDownload { error } => write!(f, "Failed to start download: {error}"),
            CommandError::PauseDownload { error } => write!(f, "Failed to pause download: {error}"),
            CommandError::RemoveDownload { error } => write!(f, "Failed to remove download: {error}"),
            CommandError::ChangeFolder { error } => write!(f, "Failed to change folder: {error}"),
            CommandError::ValidationFailed { error } => write!(f, "Validation failed: {error}"),
            CommandError::SaveConfig { error } => write!(f, "Failed to save configuration: {error}"),
            CommandError::FolderNotFound { folder } => write!(f, "Folder not found: {folder}"),
            CommandError::ReloadScripts { error } => write!(f, "Failed to reload scripts: {error}"),
            CommandError::ScriptCommunication { error } => {
                write!(f, "Script communication error: {error}")
            }
            CommandError::BlockingTask { error } => write!(f, "Background task error: {error}"),
            CommandError::ReloadConfig { error } => {
                write!(f, "Failed to reload configuration: {error}")
            }
            CommandError::Serialization { error } => write!(f, "{error}"),
        }
    }
}

impl std::error::Error for CommandError {}

/// Response to a command
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CommandResponse {
    Success { data: serde_json::Value },
    Error(CommandError),
}

impl CommandResponse {
    /// Convenience constructor for the common `{"status": "ok"}` success.
    fn ok() -> Self {
        CommandResponse::Success {
            data: serde_json::json!({"status": "ok"}),
        }
    }
}

pub async fn handle_command(
    command: Command,
    state: AppState,
    download_manager: DownloadManager,
) -> CommandResponse {
    match command {
        Command::AddDownload { urls } => {
            let config = state.config.read().await;
            for url in urls {
                let task = DownloadTask::new(url, config.download.default_directory.clone());
                download_manager.add_download(task).await;
            }
            CommandResponse::ok()
        }
        Command::StartDownload { id } => {
            if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
                match download_manager.start_download(uuid, state.script_sender.clone(), state.config.clone()).await {
                    Ok(_) => CommandResponse::ok(),
                    Err(e) => CommandResponse::Error(CommandError::StartDownload {
                        error: e.to_string(),
                    }),
                }
            } else {
                CommandResponse::Error(CommandError::InvalidUuid)
            }
        }
        Command::PauseDownload { id } => {
            if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
                match download_manager.pause_download(uuid).await {
                    Ok(_) => CommandResponse::ok(),
                    Err(e) => CommandResponse::Error(CommandError::PauseDownload {
                        error: e.to_string(),
                    }),
                }
            } else {
                CommandResponse::Error(CommandError::InvalidUuid)
            }
        }
        Command::GetDownloads => {
            let downloads = download_manager.get_all_downloads().await;
            match serde_json::to_value(&downloads) {
                Ok(data) => CommandResponse::Success { data },
                Err(e) => CommandResponse::Error(CommandError::Serialization {
                    error: format!("Failed to serialize downloads: {}", e),
                }),
            }
        }
        Command::RemoveDownload { id } => {
            if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
                match download_manager.remove_download(uuid).await {
                    Ok(_) => CommandResponse::ok(),
                    Err(e) => CommandResponse::Error(CommandError::RemoveDownload {
                        error: e.to_string(),
                    }),
                }
            } else {
                CommandResponse::Error(CommandError::InvalidUuid)
            }
        }
        Command::ChangeFolder { id, folder_id } => {
            if let Ok(uuid) = uuid::Uuid::parse_str(&id) {
                match download_manager.change_folder(uuid, folder_id, Some(&state.config)).await {
                    Ok(_) => CommandResponse::ok(),
                    Err(e) => CommandResponse::Error(CommandError::ChangeFolder {
                        error: e.to_string(),
                    }),
                }
            } else {
                CommandResponse::Error(CommandError::InvalidUuid)
            }
        }
        Command::GetConfig => {
            let config = state.config.read().await;
            match serde_json::to_value(&*config) {
                Ok(data) => CommandResponse::Success { data },
                Err(e) => CommandResponse::Error(CommandError::Serialization {
                    error: format!("Failed to serialize config: {}", e),
                }),
            }
        }
        Command::UpdateConfig { config } => {
            let mut state_config = state.config.write().await;
            if let Ok(new_config) = serde_json::from_value(config) {
                // Validate before applying
                if let Err(errors) = crate::app::settings::validate_folder_config(&new_config) {
                    let error_str = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ");
                    return CommandResponse::Error(CommandError::ValidationFailed { error: error_str });
                }

                *state_config = new_config;
                // Save to disk
                if let Err(e) = state_config.save() {
                    return CommandResponse::Error(CommandError::SaveConfig {
                        error: e.to_string(),
                    });
                }

                CommandResponse::ok()
            } else {
                CommandResponse::Error(CommandError::InvalidConfig)
            }
        }

        Command::UpdateMaxConcurrent { value } => {
            let mut config = state.config.write().await;
            config.download.max_concurrent = value;

            // Validate constraints
            if let Err(errors) = crate::app::settings::validate_folder_config(&config) {
                let error_str = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ");
                return CommandResponse::Error(CommandError::ValidationFailed { error: error_str });
            }

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }

        Command::UpdateMaxConcurrentPerFolder { value } => {
            let mut config = state.config.write().await;
            config.download.max_concurrent_per_folder = value;

            // Validate constraints
            if let Err(errors) = crate::app::settings::validate_folder_config(&config) {
                let error_str = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ");
                return CommandResponse::Error(CommandError::ValidationFailed { error: error_str });
            }

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }

        Command::UpdateMaxActiveFolders { value } => {
            let mut config = state.config.write().await;
            config.download.parallel_folder_count = value;

            // Validate constraints
            if let Err(errors) = crate::app::settings::validate_folder_config(&config) {
                let error_str = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ");
                return CommandResponse::Error(CommandError::ValidationFailed { error: error_str });
            }

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }

        Command::UpdateMaxRedirects { value } => {
            let mut config = state.config.write().await;
            config.download.max_redirects = value;

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }

        Command::UpdateRetryCount { value } => {
            let mut config = state.config.write().await;
            config.download.retry_count = value;

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }

        Command::UpdateScriptsEnabled { value } => {
            let mut config = state.config.write().await;
            config.scripts.enabled = value;

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }
        Command::UpdateSkipDownloadPreview { value } => {
            let mut config = state.config.write().await;
            config.general.skip_download_preview = value;

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }
        Command::UpdateAutoLaunchDnd { value } => {
            let mut config = state.config.write().await;
            config.general.auto_launch_dnd = value;

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }
        Command::UpdateLanguage { value } => {
            let mut config = state.config.write().await;
            config.general.language = value.clone();

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }

        Command::UpdateUserAgent { value } => {
            let mut config = state.config.write().await;
            config.download.user_agent = value.clone();

            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "value": value}),
            }
        }

        Command::UpdateReferrerPolicy { policy } => {
            let mut config = state.config.write().await;
            config.download.referrer_policy = policy.clone();

            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::ok()
        }

        Command::UpdateFolderMaxConcurrent { folder_id, value } => {
            let mut config = state.config.write().await;

            // Get or create folder config
            let folder_config = config
                .folders
                .entry(folder_id.clone())
                .or_insert_with(crate::app::config::FolderConfig::default);

            folder_config.max_concurrent = value;

            // Validate constraints
            if let Err(errors) = crate::app::settings::validate_folder_config(&config) {
                let error_str = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ");
                return CommandResponse::Error(CommandError::ValidationFailed { error: error_str });
            }

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "folder_id": folder_id, "value": value}),
            }
        }

        Command::UpdateFolderUserAgent { folder_id, value } => {
            let mut config = state.config.write().await;

            let folder_config = config
                .folders
                .entry(folder_id.clone())
                .or_insert_with(crate::app::config::FolderConfig::default);

            folder_config.user_agent = value.clone();

            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "folder_id": folder_id}),
            }
        }

        Command::UpdateFolderReferrerPolicy { folder_id, policy } => {
            let mut config = state.config.write().await;

            let folder_config = config
                .folders
                .entry(folder_id.clone())
                .or_insert_with(crate::app::config::FolderConfig::default);

            folder_config.referrer_policy = policy;

            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({"status": "ok", "folder_id": folder_id}),
            }
        }

        Command::ToggleScriptFile { filename } => {
            let mut config = state.config.write().await;

            // Toggle the enabled status (default is true if not in map)
            let current_status = config.scripts.script_files.get(&filename).copied().unwrap_or(true);
            let new_status = !current_status;
            config.scripts.script_files.insert(filename.clone(), new_status);

            // Save to disk
            if let Err(e) = config.save() {
                return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
            }

            CommandResponse::Success {
                data: serde_json::json!({
                    "status": "ok",
                    "filename": filename,
                    "enabled": new_status
                }),
            }
        }

        Command::ToggleFolderScriptFile { folder_id, filename } => {
            let mut config = state.config.write().await;

            // Get application-level status first (before mutable borrow)
            let app_status = config.scripts.script_files.get(&filename).copied().unwrap_or(true);

            // Get or create folder config
            if let Some(folder_config) = config.folders.get_mut(&folder_id) {
                // Get or create the script_files map for this folder
                let script_files = folder_config.script_files.get_or_insert_with(std::collections::HashMap::new);

                // Get current effective status (inherit from Application if not overridden)
                let current_status = script_files.get(&filename).copied().unwrap_or(app_status);

                // Toggle: enabled -> disabled, disabled -> remove override (inherit)
                if script_files.contains_key(&filename) {
                    // Already overridden - remove the override to inherit from Application
                    script_files.remove(&filename);
                } else {
                    // Not overridden - set opposite of current inherited value
                    script_files.insert(filename.clone(), !current_status);
                }

                // If script_files becomes empty, set to None to inherit all
                if script_files.is_empty() {
                    folder_config.script_files = None;
                }

                // Save to disk
                if let Err(e) = config.save() {
                    return CommandResponse::Error(CommandError::SaveConfig { error: e.to_string() });
                }

                CommandResponse::Success {
                    data: serde_json::json!({
                        "status": "ok",
                        "folder_id": folder_id,
                        "filename": filename,
                    }),
                }
            } else {
                CommandResponse::Error(CommandError::FolderNotFound { folder: folder_id })
            }
        }

        Command::ReloadScripts => {
            // Send reload message to script executor
            if let Some(ref script_sender) = state.script_sender {
                let (response_tx, response_rx) = std::sync::mpsc::channel();
                let sender_clone = script_sender.clone();

                // Send request and receive response in blocking task
                match tokio::task::spawn_blocking(move || {
                    if let Err(e) = sender_clone.send(crate::script::message::ScriptRequest::Reload {
                        response: response_tx,
                    }) {
                        return Err(format!("{:?}", e));
                    }

                    response_rx.recv()
                        .map_err(|e| format!("{:?}", e))
                }).await
                {
                    Ok(Ok(Ok(_))) => CommandResponse::ok(),
                    Ok(Ok(Err(e))) => CommandResponse::Error(CommandError::ReloadScripts {
                        error: e.to_string(),
                    }),
                    Ok(Err(e)) => CommandResponse::Error(CommandError::ScriptCommunication {
                        error: e.clone(),
                    }),
                    Err(e) => CommandResponse::Error(CommandError::BlockingTask {
                        error: e.to_string(),
                    }),
                }
            } else {
                CommandResponse::Error(CommandError::ScriptsDisabled)
            }
        }

        Command::ReloadConfig => {
            // Check if any downloads are active
            let has_active = download_manager.has_active_downloads().await;
            if has_active {
                return CommandResponse::Error(CommandError::ReloadActiveDownloads);
            }

            // Reload configuration from disk
            match crate::app::config::Config::load() {
                Ok(new_config) => {
                    // Update application state
                    let mut config = state.config.write().await;
                    *config = new_config;

                    CommandResponse::ok()
                }
                Err(e) => CommandResponse::Error(CommandError::ReloadConfig {
                    error: e.to_string(),
                }),
            }
        }
    }
}
