//! Localization for [`CommandError`] values returned by `ui::commands`.
//!
//! Keeping this mapping in the TUI (the display layer) is what lets the command
//! layer stay free of any i18n dependency: `CommandError` carries
//! machine-readable data, and this is where each variant becomes a translated,
//! user-facing string in the application's current locale.

use super::state::TuiState;
use crate::ui::commands::CommandError;
use fluent::fluent_args;

/// Translate a [`CommandError`] into a localized, user-facing message.
pub(crate) fn localize_command_error(state: &TuiState, err: &CommandError) -> String {
    match err {
        CommandError::InvalidUuid => state.t("cmd-error-invalid-uuid"),
        CommandError::InvalidConfig => state.t("cmd-error-invalid-config"),
        CommandError::ScriptsDisabled => state.t("cmd-error-scripts-disabled"),
        CommandError::ReloadActiveDownloads => state.t("cmd-error-reload-active-downloads"),
        CommandError::StartDownload { error } => state.t_with_args(
            "cmd-error-start-download",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::PauseDownload { error } => state.t_with_args(
            "cmd-error-pause-download",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::RemoveDownload { error } => state.t_with_args(
            "cmd-error-remove-download",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::ChangeFolder { error } => state.t_with_args(
            "cmd-error-change-folder",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::ValidationFailed { error } => state.t_with_args(
            "cmd-error-validation-failed",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::SaveConfig { error } => state.t_with_args(
            "cmd-error-save-config",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::FolderNotFound { folder } => state.t_with_args(
            "cmd-error-folder-not-found",
            Some(&fluent_args!["folder" => folder.clone()]),
        ),
        CommandError::ReloadScripts { error } => state.t_with_args(
            "cmd-error-reload-scripts",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::ScriptCommunication { error } => state.t_with_args(
            "cmd-error-script-communication",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::BlockingTask { error } => state.t_with_args(
            "cmd-error-blocking-task",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::ReloadConfig { error } => state.t_with_args(
            "cmd-error-reload-config",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        // Internal/developer-facing; no translation catalog entry.
        CommandError::Serialization { error } => error.clone(),
    }
}
