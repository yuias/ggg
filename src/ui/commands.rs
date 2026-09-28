use crate::AppState;
use crate::app::config::{Config, ReferrerPolicy};
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
    UpdateFolderName { folder_id: String, value: String },
    UpdateFolderSavePath { folder_id: String, value: std::path::PathBuf },
    UpdateFolderMaxConcurrent { folder_id: String, value: Option<usize> },
    UpdateFolderUserAgent { folder_id: String, value: Option<String> },
    UpdateFolderReferrerPolicy { folder_id: String, policy: Option<ReferrerPolicy> },
    UpdateFolderAutoDateDirectory { folder_id: String, value: bool },
    UpdateFolderAutoStartDownloads { folder_id: String, value: bool },
    UpdateFolderPreventDuplicateUrl { folder_id: String, value: bool },
    UpdateFolderScriptsEnabled { folder_id: String, value: Option<bool> },
    CreateFolder,

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
    /// A folder name edit resolved to an empty (or whitespace-only) value.
    EmptyFolderName,
    /// A folder name edit collided with another folder's display name.
    DuplicateFolderName { name: String },
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
            CommandError::EmptyFolderName => write!(f, "Folder name cannot be empty"),
            CommandError::DuplicateFolderName { name } => {
                write!(f, "A folder named '{name}' already exists")
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

/// Applies `edit` to a clone of the shared config, validates and saves the
/// clone, and only swaps it into the shared state once both succeed. A
/// validation or save failure (including an error the closure itself raises,
/// e.g. an unknown folder) returns before the swap, so a rejected edit never
/// leaves memory and disk diverged. Mirrors `dispatch_config_set` in
/// `src/cli/daemon_ops.rs`. The write lock is held for the clone, edit,
/// validate and save sequence, but not across any other await.
async fn commit_config_edit(
    state: &AppState,
    edit: impl FnOnce(&mut Config) -> Result<(), CommandError>,
) -> Result<(), CommandError> {
    let mut guard = state.config.write().await;
    let mut clone = guard.clone();

    edit(&mut clone)?;

    if let Err(errors) = crate::app::settings::validate_folder_config(&clone) {
        let error_str = errors.iter().map(|e| e.to_string()).collect::<Vec<_>>().join(", ");
        return Err(CommandError::ValidationFailed { error: error_str });
    }

    if let Err(e) = clone.save() {
        return Err(CommandError::SaveConfig { error: e.to_string() });
    }

    *guard = clone;
    Ok(())
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
            let new_config: Config = match serde_json::from_value(config) {
                Ok(c) => c,
                Err(_) => return CommandResponse::Error(CommandError::InvalidConfig),
            };
            match commit_config_edit(&state, |cfg| {
                *cfg = new_config;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::ok(),
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateMaxConcurrent { value } => {
            match commit_config_edit(&state, |config| {
                config.download.max_concurrent = value;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateMaxConcurrentPerFolder { value } => {
            match commit_config_edit(&state, |config| {
                config.download.max_concurrent_per_folder = value;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateMaxActiveFolders { value } => {
            match commit_config_edit(&state, |config| {
                config.download.parallel_folder_count = value;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateMaxRedirects { value } => {
            match commit_config_edit(&state, |config| {
                config.download.max_redirects = value;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateRetryCount { value } => {
            match commit_config_edit(&state, |config| {
                config.download.retry_count = value;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateScriptsEnabled { value } => {
            match commit_config_edit(&state, |config| {
                config.scripts.enabled = value;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }
        Command::UpdateSkipDownloadPreview { value } => {
            match commit_config_edit(&state, |config| {
                config.general.skip_download_preview = value;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }
        Command::UpdateAutoLaunchDnd { value } => {
            match commit_config_edit(&state, |config| {
                config.general.auto_launch_dnd = value;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }
        Command::UpdateLanguage { value } => {
            match commit_config_edit(&state, |config| {
                config.general.language = value.clone();
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateUserAgent { value } => {
            match commit_config_edit(&state, |config| {
                config.download.user_agent = value.clone();
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateReferrerPolicy { policy } => {
            match commit_config_edit(&state, |config| {
                config.download.referrer_policy = policy;
                Ok(())
            })
            .await
            {
                Ok(()) => CommandResponse::ok(),
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateFolderName { folder_id, value } => {
            let new_name = value.trim().to_string();
            if new_name.is_empty() {
                return CommandResponse::Error(CommandError::EmptyFolderName);
            }

            let result = commit_config_edit(&state, |config| {
                if config
                    .folders
                    .iter()
                    .any(|(id, f)| id.as_str() != folder_id.as_str() && f.name == new_name)
                {
                    return Err(CommandError::DuplicateFolderName { name: new_name.clone() });
                }

                let folder_config = config
                    .folders
                    .entry(folder_id.clone())
                    .or_insert_with(crate::app::config::FolderConfig::default);
                folder_config.name = new_name.clone();
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": folder_id, "value": new_name}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateFolderSavePath { folder_id, value } => {
            let result = commit_config_edit(&state, |config| {
                let folder_config = config
                    .folders
                    .entry(folder_id.clone())
                    .or_insert_with(crate::app::config::FolderConfig::default);
                folder_config.save_path = value.clone();
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": folder_id}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateFolderMaxConcurrent { folder_id, value } => {
            let result = commit_config_edit(&state, |config| {
                let folder_config = config
                    .folders
                    .entry(folder_id.clone())
                    .or_insert_with(crate::app::config::FolderConfig::default);
                folder_config.max_concurrent = value;
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": folder_id, "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateFolderUserAgent { folder_id, value } => {
            let result = commit_config_edit(&state, |config| {
                let folder_config = config
                    .folders
                    .entry(folder_id.clone())
                    .or_insert_with(crate::app::config::FolderConfig::default);
                folder_config.user_agent = value.clone();
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": folder_id}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateFolderReferrerPolicy { folder_id, policy } => {
            let result = commit_config_edit(&state, |config| {
                let folder_config = config
                    .folders
                    .entry(folder_id.clone())
                    .or_insert_with(crate::app::config::FolderConfig::default);
                folder_config.referrer_policy = policy.clone();
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": folder_id}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateFolderAutoDateDirectory { folder_id, value } => {
            let result = commit_config_edit(&state, |config| {
                let folder_config = config
                    .folders
                    .entry(folder_id.clone())
                    .or_insert_with(crate::app::config::FolderConfig::default);
                folder_config.auto_date_directory = value;
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": folder_id, "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateFolderAutoStartDownloads { folder_id, value } => {
            let result = commit_config_edit(&state, |config| {
                let folder_config = config
                    .folders
                    .entry(folder_id.clone())
                    .or_insert_with(crate::app::config::FolderConfig::default);
                folder_config.auto_start_downloads = value;
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": folder_id, "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateFolderPreventDuplicateUrl { folder_id, value } => {
            let result = commit_config_edit(&state, |config| {
                let folder_config = config
                    .folders
                    .entry(folder_id.clone())
                    .or_insert_with(crate::app::config::FolderConfig::default);
                folder_config.prevent_duplicate_url = value;
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": folder_id, "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::UpdateFolderScriptsEnabled { folder_id, value } => {
            let result = commit_config_edit(&state, |config| {
                let folder_config = config
                    .folders
                    .entry(folder_id.clone())
                    .or_insert_with(crate::app::config::FolderConfig::default);
                folder_config.scripts_enabled = value;
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": folder_id, "value": value}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::CreateFolder => {
            let new_folder_id = Config::generate_folder_id();
            let result = commit_config_edit(&state, |config| {
                let mut counter = 1;
                let mut display_name = format!("folder_{}", counter);
                while config.folders.values().any(|f| f.name == display_name) {
                    counter += 1;
                    display_name = format!("folder_{}", counter);
                }

                let new_folder = crate::app::config::FolderConfig {
                    name: display_name,
                    save_path: config.download.default_directory.clone(),
                    ..crate::app::config::FolderConfig::default()
                };

                config.folders.insert(new_folder_id.clone(), new_folder);
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({"status": "ok", "folder_id": new_folder_id}),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::ToggleScriptFile { filename } => {
            // Toggle the enabled status (default is true if not in map)
            let mut new_status = true;
            let result = commit_config_edit(&state, |config| {
                let current_status = config.scripts.script_files.get(&filename).copied().unwrap_or(true);
                new_status = !current_status;
                config.scripts.script_files.insert(filename.clone(), new_status);
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({
                        "status": "ok",
                        "filename": filename,
                        "enabled": new_status
                    }),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::ToggleFolderScriptFile { folder_id, filename } => {
            let result = commit_config_edit(&state, |config| {
                // Get application-level status first (before the mutable borrow)
                let app_status = config.scripts.script_files.get(&filename).copied().unwrap_or(true);

                let folder_config = config
                    .folders
                    .get_mut(&folder_id)
                    .ok_or_else(|| CommandError::FolderNotFound { folder: folder_id.clone() })?;

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

                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({
                        "status": "ok",
                        "folder_id": folder_id,
                        "filename": filename,
                    }),
                },
                Err(e) => CommandResponse::Error(e),
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

            // Reads a config that is already on disk rather than deriving one
            // from an edit, so there is nothing to validate or save here;
            // `commit_config_edit` does not apply. A load failure returns
            // before the write lock is taken, so memory is untouched either way.
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::FolderConfig;
    use crate::download::manager::DownloadManager;

    fn state_with(config: Config) -> AppState {
        AppState::new(config)
    }

    #[tokio::test]
    async fn update_max_concurrent_rejected_keeps_old_value() {
        let mut config = Config::default();
        // 5 * 2 = 10, exactly at the initial limit, so this setup is valid
        // before the update under test.
        config.download.max_concurrent = 10;
        config.download.max_concurrent_per_folder = Some(5);
        config.download.parallel_folder_count = Some(2);
        let state = state_with(config);

        let response =
            handle_command(Command::UpdateMaxConcurrent { value: 3 }, state.clone(), DownloadManager::new()).await;

        match response {
            CommandResponse::Error(CommandError::ValidationFailed { .. }) => {}
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
        let config = state.config.read().await;
        assert_eq!(config.download.max_concurrent, 10);
    }

    #[tokio::test]
    async fn update_folder_max_concurrent_rejected_keeps_old_value() {
        let mut config = Config::default();
        config.download.max_concurrent = 3;
        config.folders.insert(
            "folder-1".to_string(),
            FolderConfig { max_concurrent: Some(2), ..FolderConfig::default() },
        );
        let state = state_with(config);

        let response = handle_command(
            Command::UpdateFolderMaxConcurrent { folder_id: "folder-1".to_string(), value: Some(5) },
            state.clone(),
            DownloadManager::new(),
        )
        .await;

        match response {
            CommandResponse::Error(CommandError::ValidationFailed { .. }) => {}
            other => panic!("expected ValidationFailed, got {other:?}"),
        }
        let config = state.config.read().await;
        assert_eq!(config.folders.get("folder-1").and_then(|f| f.max_concurrent), Some(2));
    }

    #[tokio::test]
    async fn update_folder_name_rejects_empty_name() {
        let mut config = Config::default();
        config.folders.insert(
            "folder-1".to_string(),
            FolderConfig { name: "Downloads".to_string(), ..FolderConfig::default() },
        );
        let state = state_with(config);

        let response = handle_command(
            Command::UpdateFolderName { folder_id: "folder-1".to_string(), value: "   ".to_string() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;

        match response {
            CommandResponse::Error(CommandError::EmptyFolderName) => {}
            other => panic!("expected EmptyFolderName, got {other:?}"),
        }
        let config = state.config.read().await;
        assert_eq!(config.folders.get("folder-1").map(|f| f.name.clone()), Some("Downloads".to_string()));
    }

    #[tokio::test]
    async fn update_folder_name_rejects_duplicate_name() {
        let mut config = Config::default();
        config.folders.insert(
            "folder-1".to_string(),
            FolderConfig { name: "Downloads".to_string(), ..FolderConfig::default() },
        );
        config.folders.insert(
            "folder-2".to_string(),
            FolderConfig { name: "Movies".to_string(), ..FolderConfig::default() },
        );
        let state = state_with(config);

        let response = handle_command(
            Command::UpdateFolderName { folder_id: "folder-2".to_string(), value: "Downloads".to_string() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;

        match response {
            CommandResponse::Error(CommandError::DuplicateFolderName { .. }) => {}
            other => panic!("expected DuplicateFolderName, got {other:?}"),
        }
        let config = state.config.read().await;
        assert_eq!(config.folders.get("folder-2").map(|f| f.name.clone()), Some("Movies".to_string()));
    }

    // Exercises the real `Config::save()`/load path, so it needs an isolated
    // config directory; `#[serial]` keeps it from racing other tests that
    // touch the same process-global override.
    #[tokio::test]
    #[serial_test::serial]
    async fn update_folder_auto_date_directory_persists_to_disk() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        // A non-UUID key would be treated as a legacy name and re-keyed on
        // load, so use a real UUID folder id like production code does.
        let folder_id = Config::generate_folder_id();
        let mut config = Config::default();
        config.folders.insert(folder_id.clone(), FolderConfig::default());
        let state = state_with(config);

        let response = handle_command(
            Command::UpdateFolderAutoDateDirectory { folder_id: folder_id.clone(), value: true },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(response, CommandResponse::Success { .. }), "response: {response:?}");

        // Reload from disk (as a fresh process would after a quit) to prove
        // the toggle was not only applied in memory.
        let reloaded = Config::load().expect("reloaded config");

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert_eq!(reloaded.folders.get(&folder_id).map(|f| f.auto_date_directory), Some(true));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn create_folder_persists_to_disk() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let state = state_with(Config::default());

        let response = handle_command(Command::CreateFolder, state.clone(), DownloadManager::new()).await;
        let folder_id = match response {
            CommandResponse::Success { data } => data
                .get("folder_id")
                .and_then(|v| v.as_str())
                .map(|s| s.to_string())
                .expect("response carries the new folder_id"),
            other => panic!("expected Success, got {other:?}"),
        };

        let reloaded = Config::load().expect("reloaded config");

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert!(reloaded.folders.contains_key(&folder_id));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn update_folder_save_path_persists_to_disk() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let folder_id = Config::generate_folder_id();
        let mut config = Config::default();
        config.folders.insert(folder_id.clone(), FolderConfig::default());
        let state = state_with(config);

        let new_path = std::path::PathBuf::from("custom_downloads");
        let response = handle_command(
            Command::UpdateFolderSavePath { folder_id: folder_id.clone(), value: new_path.clone() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(response, CommandResponse::Success { .. }), "response: {response:?}");

        let reloaded = Config::load().expect("reloaded config");

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert_eq!(reloaded.folders.get(&folder_id).map(|f| f.save_path.clone()), Some(new_path));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn update_folder_name_persists_to_disk() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let folder_id = Config::generate_folder_id();
        let mut config = Config::default();
        config.folders.insert(folder_id.clone(), FolderConfig::default());
        let state = state_with(config);

        let response = handle_command(
            Command::UpdateFolderName { folder_id: folder_id.clone(), value: "Renamed".to_string() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(response, CommandResponse::Success { .. }), "response: {response:?}");

        // The folder id (UUID key) stays the same across a rename; only the
        // display name changes.
        let reloaded = Config::load().expect("reloaded config");

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert_eq!(reloaded.folders.get(&folder_id).map(|f| f.name.clone()), Some("Renamed".to_string()));
    }
}
