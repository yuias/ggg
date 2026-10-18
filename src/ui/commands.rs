use crate::AppState;
use crate::app::config::{Config, DEFAULT_FOLDER_NAME, ReferrerPolicy};
use crate::download::{manager::DownloadManager, task::DownloadTask};
use serde::{Deserialize, Serialize};

/// Commands that can be invoked from the TUI
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
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
    UpdateDefaultDirectory { value: std::path::PathBuf },

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
    DeleteFolder { folder_id: String },

    // Script settings
    ToggleScriptFile { filename: String },
    ToggleFolderScriptFile { folder_id: String, filename: String },
    ReloadScripts,
    /// Absolute counterpart of `ToggleScriptFile`, for draft-based editors.
    SetScriptFileEnabled { filename: String, enabled: bool },
    /// Absolute counterpart of `ToggleFolderScriptFile`. `None` removes the
    /// folder override so the file inherits the application value.
    SetFolderScriptFileOverride { folder_id: String, filename: String, value: Option<bool> },

    // Config management
    ReloadConfig,
}

/// UI-agnostic error returned by [`handle_command`].
///
/// Variants carry machine-readable data rather than pre-translated strings, so
/// the command layer stays free of any i18n / presentation dependency. The
/// display layer is responsible for turning a `CommandError` into a localized,
/// user-facing message (`ui::command_i18n::localize_command_error` does this for every front end).
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
    /// A default download directory edit resolved to an empty (or
    /// whitespace-only) value.
    EmptyDefaultDirectory,
    /// A folder name edit collided with another folder's display name.
    DuplicateFolderName { name: String },
    /// An attempt to rename the protected default folder.
    DefaultFolderRename,
    /// An attempt to delete the protected default folder.
    DefaultFolderDelete,
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
            CommandError::EmptyDefaultDirectory => {
                write!(f, "Default download directory cannot be empty")
            }
            CommandError::DuplicateFolderName { name } => {
                write!(f, "A folder named '{name}' already exists")
            }
            CommandError::DefaultFolderRename => {
                write!(f, "Cannot rename the default folder")
            }
            CommandError::DefaultFolderDelete => {
                write!(f, "Cannot delete the default folder")
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
                // Find the previous default folder's id (if any) before it is
                // overwritten, so a replacement that renames or drops it away
                // (leaving no folder named `DEFAULT_FOLDER_NAME`, or leaving that
                // id under a different name) can be refused. Without this, a
                // full-config replace would bypass the same guard `UpdateFolderName`
                // enforces, and the next `Config::load` would silently recreate a
                // second default folder.
                let previous_default_id =
                    cfg.folders.iter().find(|(_, f)| f.name == DEFAULT_FOLDER_NAME).map(|(id, _)| id.clone());

                let still_has_default = new_config.folders.values().any(|f| f.name == DEFAULT_FOLDER_NAME);
                // Also refused if the previous default id was dropped entirely
                // (not just renamed in place): an absent id no longer has that
                // name either, and a replacement could otherwise drop the old
                // default id while adding an unrelated one named "default".
                let previous_id_lost = previous_default_id.as_ref().is_some_and(|id| {
                    !new_config.folders.get(id).is_some_and(|f| f.name == DEFAULT_FOLDER_NAME)
                });

                if !still_has_default || previous_id_lost {
                    return Err(CommandError::DefaultFolderRename);
                }

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
                Ok(()) => {
                    // Applies live: no restart needed, unlike the other
                    // download-limit fields (the manager rebuilds its
                    // per-folder queues only at startup).
                    download_manager.set_max_concurrent(value).await;
                    CommandResponse::Success {
                        data: serde_json::json!({"status": "ok", "value": value}),
                    }
                }
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

        Command::UpdateDefaultDirectory { value } => {
            if value.to_string_lossy().trim().is_empty() {
                return CommandResponse::Error(CommandError::EmptyDefaultDirectory);
            }

            match commit_config_edit(&state, |config| {
                config.download.default_directory = value.clone();
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

        Command::UpdateFolderName { folder_id, value } => {
            let new_name = value.trim().to_string();
            if new_name.is_empty() {
                return CommandResponse::Error(CommandError::EmptyFolderName);
            }

            let result = commit_config_edit(&state, |config| {
                // The default folder's name is what config load uses to find it
                // (`Config::is_default_folder`); renaming it away would make load
                // recreate a second folder named "default" on the next start. Setting
                // it to its current name (no-op) is fine and must not error, since
                // e.g. re-submitting an unchanged field should not be a rejection.
                if config.is_default_folder(&folder_id) && new_name != DEFAULT_FOLDER_NAME {
                    return Err(CommandError::DefaultFolderRename);
                }

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

        Command::DeleteFolder { folder_id } => {
            let result = commit_config_edit(&state, |config| {
                if config.is_default_folder(&folder_id) {
                    return Err(CommandError::DefaultFolderDelete);
                }
                if !config.folders.contains_key(&folder_id) {
                    return Err(CommandError::FolderNotFound { folder: folder_id.clone() });
                }
                config.folders.remove(&folder_id);
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

        Command::SetScriptFileEnabled { filename, enabled } => {
            let result = commit_config_edit(&state, |config| {
                config.scripts.script_files.insert(filename.clone(), enabled);
                Ok(())
            })
            .await;

            match result {
                Ok(()) => CommandResponse::Success {
                    data: serde_json::json!({
                        "status": "ok",
                        "filename": filename,
                        "enabled": enabled
                    }),
                },
                Err(e) => CommandResponse::Error(e),
            }
        }

        Command::SetFolderScriptFileOverride { folder_id, filename, value } => {
            let result = commit_config_edit(&state, |config| {
                let folder_config = config
                    .folders
                    .get_mut(&folder_id)
                    .ok_or_else(|| CommandError::FolderNotFound { folder: folder_id.clone() })?;

                match value {
                    Some(enabled) => {
                        folder_config
                            .script_files
                            .get_or_insert_with(std::collections::HashMap::new)
                            .insert(filename.clone(), enabled);
                    }
                    None => {
                        if let Some(script_files) = folder_config.script_files.as_mut() {
                            script_files.remove(&filename);
                            // An empty map means "inherit everything", the same
                            // state as no map.
                            if script_files.is_empty() {
                                folder_config.script_files = None;
                            }
                        }
                    }
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
                        "value": value
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
    #[serial_test::serial]
    async fn update_max_concurrent_applies_to_manager_live() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let config = Config::default();
        let state = state_with(config);
        let manager = DownloadManager::new();

        let response =
            handle_command(Command::UpdateMaxConcurrent { value: 7 }, state.clone(), manager.clone()).await;

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert!(matches!(response, CommandResponse::Success { .. }));
        assert_eq!(manager.get_max_concurrent().await, 7);
    }

    /// Redirects config persistence to a temp directory for the lifetime of the
    /// guard. `Config::save` deletes folder directories it does not know, so a
    /// test that saves must never reach the real config. Resetting in `Drop`
    /// also covers a failing assertion.
    struct ConfigIsolation {
        _dir: tempfile::TempDir,
    }

    impl ConfigIsolation {
        fn new() -> Self {
            let dir = tempfile::TempDir::new().unwrap();
            crate::util::paths::set_config_dir_override(Some(dir.path().to_path_buf()));
            unsafe { std::env::set_var("GGG_TEST_MODE", "1") };
            Self { _dir: dir }
        }
    }

    impl Drop for ConfigIsolation {
        fn drop(&mut self) {
            crate::util::paths::set_config_dir_override(None);
            unsafe { std::env::remove_var("GGG_TEST_MODE") };
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn set_script_file_enabled_persists() {
        let _isolation = ConfigIsolation::new();
        let state = state_with(Config::default());

        let off = handle_command(
            Command::SetScriptFileEnabled { filename: "a.js".to_string(), enabled: false },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(off, CommandResponse::Success { .. }), "got {off:?}");
        assert_eq!(state.config.read().await.scripts.script_files.get("a.js"), Some(&false));
        // Set absolutely, not toggled: repeating the command keeps the value.
        let again = handle_command(
            Command::SetScriptFileEnabled { filename: "a.js".to_string(), enabled: false },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(again, CommandResponse::Success { .. }), "got {again:?}");
        assert_eq!(state.config.read().await.scripts.script_files.get("a.js"), Some(&false));

        let on = handle_command(
            Command::SetScriptFileEnabled { filename: "a.js".to_string(), enabled: true },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(on, CommandResponse::Success { .. }), "got {on:?}");
        assert_eq!(state.config.read().await.scripts.script_files.get("a.js"), Some(&true));

        // The value reached disk, not just memory.
        let reloaded = Config::load().expect("config saved in the isolated directory loads");
        assert_eq!(reloaded.scripts.script_files.get("a.js"), Some(&true));
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn set_folder_script_override_none_removes_key() {
        let _isolation = ConfigIsolation::new();
        let mut config = Config::default();
        config.folders.insert(
            "folder-1".to_string(),
            FolderConfig { name: "Downloads".to_string(), ..FolderConfig::default() },
        );
        let state = state_with(config);

        let set = handle_command(
            Command::SetFolderScriptFileOverride {
                folder_id: "folder-1".to_string(),
                filename: "a.js".to_string(),
                value: Some(false),
            },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(set, CommandResponse::Success { .. }), "got {set:?}");
        assert_eq!(
            state.config.read().await.folders["folder-1"].script_files.as_ref().and_then(|m| m.get("a.js")),
            Some(&false)
        );

        let cleared = handle_command(
            Command::SetFolderScriptFileOverride {
                folder_id: "folder-1".to_string(),
                filename: "a.js".to_string(),
                value: None,
            },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(cleared, CommandResponse::Success { .. }), "got {cleared:?}");
        assert!(state.config.read().await.folders["folder-1"].script_files.is_none());

        let missing = handle_command(
            Command::SetFolderScriptFileOverride {
                folder_id: "nope".to_string(),
                filename: "a.js".to_string(),
                value: None,
            },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(missing, CommandResponse::Error(CommandError::FolderNotFound { .. })), "got {missing:?}");
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

    #[tokio::test]
    async fn update_folder_name_rejects_renaming_default_folder() {
        let mut config = Config::default();
        config.folders.insert(
            "folder-1".to_string(),
            FolderConfig { name: "default".to_string(), ..FolderConfig::default() },
        );
        let state = state_with(config);

        let response = handle_command(
            Command::UpdateFolderName { folder_id: "folder-1".to_string(), value: "Renamed".to_string() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;

        match response {
            CommandResponse::Error(CommandError::DefaultFolderRename) => {}
            other => panic!("expected DefaultFolderRename, got {other:?}"),
        }
        let config = state.config.read().await;
        assert_eq!(config.folders.get("folder-1").map(|f| f.name.clone()), Some("default".to_string()));
    }

    // Reaches `commit_config_edit`'s success path (a no-op rename still saves),
    // so it needs the same isolated-config-directory + `#[serial]` protocol as
    // the other `*_persists_to_disk` tests below: `Config::save` writes to
    // whatever `set_config_dir_override` currently points at, which is a
    // process-global, not per-test, so an unguarded save here would race any
    // concurrently running test that also touches the real/overridden config.
    #[tokio::test]
    #[serial_test::serial]
    async fn update_folder_name_allows_default_folder_unchanged_name_as_noop() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let folder_id = Config::generate_folder_id();
        let mut config = Config::default();
        config.folders.insert(folder_id.clone(), FolderConfig { name: "default".to_string(), ..FolderConfig::default() });
        let state = state_with(config);

        let response = handle_command(
            Command::UpdateFolderName { folder_id: folder_id.clone(), value: "default".to_string() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert!(matches!(response, CommandResponse::Success { .. }), "response: {response:?}");
        let config = state.config.read().await;
        assert_eq!(config.folders.get(&folder_id).map(|f| f.name.clone()), Some("default".to_string()));
    }

    #[tokio::test]
    async fn update_folder_name_rejects_renaming_to_default() {
        let mut config = Config::default();
        config.folders.insert(
            "folder-1".to_string(),
            FolderConfig { name: "default".to_string(), ..FolderConfig::default() },
        );
        config.folders.insert(
            "folder-2".to_string(),
            FolderConfig { name: "Movies".to_string(), ..FolderConfig::default() },
        );
        let state = state_with(config);

        let response = handle_command(
            Command::UpdateFolderName { folder_id: "folder-2".to_string(), value: "default".to_string() },
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

    #[tokio::test]
    async fn delete_folder_rejects_default_folder() {
        let mut config = Config::default();
        config.folders.insert(
            "folder-1".to_string(),
            FolderConfig { name: "default".to_string(), ..FolderConfig::default() },
        );
        let state = state_with(config);

        let response = handle_command(
            Command::DeleteFolder { folder_id: "folder-1".to_string() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;

        match response {
            CommandResponse::Error(CommandError::DefaultFolderDelete) => {}
            other => panic!("expected DefaultFolderDelete, got {other:?}"),
        }
        let config = state.config.read().await;
        assert!(config.folders.contains_key("folder-1"));
    }

    #[tokio::test]
    async fn delete_folder_rejects_missing_folder() {
        let state = state_with(Config::default());

        let response = handle_command(
            Command::DeleteFolder { folder_id: "does-not-exist".to_string() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;

        match response {
            CommandResponse::Error(CommandError::FolderNotFound { .. }) => {}
            other => panic!("expected FolderNotFound, got {other:?}"),
        }
    }

    // Exercises the real `Config::save()`/load path, so it needs an isolated
    // config directory; `#[serial]` keeps it from racing other tests that
    // touch the same process-global override.
    #[tokio::test]
    #[serial_test::serial]
    async fn delete_folder_persists_to_disk() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let folder_id = Config::generate_folder_id();
        let mut config = Config::default();
        config.folders.insert(folder_id.clone(), FolderConfig { name: "Movies".to_string(), ..FolderConfig::default() });
        let state = state_with(config);

        let response = handle_command(
            Command::DeleteFolder { folder_id: folder_id.clone() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(response, CommandResponse::Success { .. }), "response: {response:?}");

        let reloaded = Config::load().expect("reloaded config");

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert!(!reloaded.folders.contains_key(&folder_id));
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

    #[tokio::test]
    #[serial_test::serial]
    async fn update_default_directory_persists_to_disk() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let state = state_with(Config::default());

        let new_dir = std::path::PathBuf::from("custom_default_downloads");
        let response = handle_command(
            Command::UpdateDefaultDirectory { value: new_dir.clone() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;
        assert!(matches!(response, CommandResponse::Success { .. }), "response: {response:?}");

        let reloaded = Config::load().expect("reloaded config");

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert_eq!(reloaded.download.default_directory, new_dir);
    }

    #[tokio::test]
    async fn update_default_directory_rejects_empty() {
        let mut config = Config::default();
        config.download.default_directory = std::path::PathBuf::from("original");
        let state = state_with(config);

        let response = handle_command(
            Command::UpdateDefaultDirectory { value: std::path::PathBuf::new() },
            state.clone(),
            DownloadManager::new(),
        )
        .await;

        match response {
            CommandResponse::Error(CommandError::EmptyDefaultDirectory) => {}
            other => panic!("expected EmptyDefaultDirectory, got {other:?}"),
        }
        let config = state.config.read().await;
        assert_eq!(config.download.default_directory, std::path::PathBuf::from("original"));
    }

    #[tokio::test]
    async fn update_config_rejects_replacement_missing_default_folder() {
        let mut config = Config::default();
        config.folders.insert(
            "folder-1".to_string(),
            FolderConfig { name: "default".to_string(), ..FolderConfig::default() },
        );
        let state = state_with(config.clone());

        // Replacement config renames the only folder away from "default",
        // which would make the next `Config::load` recreate a second one.
        let mut replacement = config.clone();
        replacement.folders.get_mut("folder-1").unwrap().name = "Renamed".to_string();
        let replacement_value = serde_json::to_value(&replacement).unwrap();

        let response =
            handle_command(Command::UpdateConfig { config: replacement_value }, state.clone(), DownloadManager::new())
                .await;

        match response {
            CommandResponse::Error(CommandError::DefaultFolderRename) => {}
            other => panic!("expected DefaultFolderRename, got {other:?}"),
        }
        let config = state.config.read().await;
        assert_eq!(config.folders.get("folder-1").map(|f| f.name.clone()), Some("default".to_string()));
    }

    #[tokio::test]
    async fn update_config_rejects_replacement_dropping_previous_default_id() {
        let mut config = Config::default();
        config.folders.insert(
            "folder-1".to_string(),
            FolderConfig { name: "default".to_string(), ..FolderConfig::default() },
        );
        let state = state_with(config.clone());

        // Replacement drops the previous default folder's id entirely and adds
        // an unrelated folder that happens to be named "default"; `still_has_default`
        // alone would pass this, so the id-loss check must catch it too.
        let mut replacement = config.clone();
        replacement.folders.remove("folder-1");
        replacement.folders.insert(
            "folder-2".to_string(),
            FolderConfig { name: "default".to_string(), ..FolderConfig::default() },
        );
        let replacement_value = serde_json::to_value(&replacement).unwrap();

        let response =
            handle_command(Command::UpdateConfig { config: replacement_value }, state.clone(), DownloadManager::new())
                .await;

        match response {
            CommandResponse::Error(CommandError::DefaultFolderRename) => {}
            other => panic!("expected DefaultFolderRename, got {other:?}"),
        }
        let config = state.config.read().await;
        assert!(config.folders.contains_key("folder-1"));
        assert!(!config.folders.contains_key("folder-2"));
    }

    // Reaches `commit_config_edit`'s success path (the replacement is accepted
    // and saved), so needs the same isolated-config-directory + `#[serial]`
    // protocol as the `*_persists_to_disk` tests: an unguarded `Config::save`
    // here would race any concurrently running test touching the process-global
    // config-dir override.
    #[tokio::test]
    #[serial_test::serial]
    async fn update_config_accepts_replacement_keeping_default_folder() {
        let temp_dir = tempfile::TempDir::new().unwrap();
        crate::util::paths::set_config_dir_override(Some(temp_dir.path().to_path_buf()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        let mut config = Config::default();
        config.folders.insert(
            Config::generate_folder_id(),
            FolderConfig { name: "default".to_string(), ..FolderConfig::default() },
        );
        let state = state_with(config.clone());

        let mut replacement = config.clone();
        replacement.download.max_redirects += 1;
        let replacement_value = serde_json::to_value(&replacement).unwrap();

        let response =
            handle_command(Command::UpdateConfig { config: replacement_value }, state.clone(), DownloadManager::new())
                .await;

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert!(matches!(response, CommandResponse::Success { .. }), "response: {response:?}");
    }
}
