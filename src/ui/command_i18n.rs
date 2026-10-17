//! Localization for [`CommandError`] values returned by `ui::commands`.
//!
//! Keeping this mapping outside the command layer is what lets that layer stay
//! free of any i18n dependency: `CommandError` carries machine-readable data,
//! and this is where each variant becomes a translated, user-facing string in
//! the application's current locale. It takes only a `LocalizationManager` so
//! every front end can share it.

use crate::ui::commands::CommandError;
use crate::util::i18n::LocalizationManager;
use fluent::fluent_args;

/// Translate a [`CommandError`] into a localized, user-facing message.
pub fn localize_command_error(i18n: &LocalizationManager, err: &CommandError) -> String {
    match err {
        CommandError::InvalidUuid => i18n.get("cmd-error-invalid-uuid"),
        CommandError::InvalidConfig => i18n.get("cmd-error-invalid-config"),
        CommandError::ScriptsDisabled => i18n.get("cmd-error-scripts-disabled"),
        CommandError::ReloadActiveDownloads => i18n.get("cmd-error-reload-active-downloads"),
        CommandError::EmptyFolderName => i18n.get("cmd-error-empty-folder-name"),
        CommandError::EmptyDefaultDirectory => i18n.get("cmd-error-empty-default-directory"),
        CommandError::DuplicateFolderName { name } => i18n.get_with_args(
            "cmd-error-duplicate-folder-name",
            Some(&fluent_args!["name" => name.clone()]),
        ),
        CommandError::DefaultFolderRename => i18n.get("cmd-error-default-folder-rename"),
        CommandError::DefaultFolderDelete => i18n.get("cmd-error-default-folder-delete"),
        CommandError::StartDownload { error } => i18n.get_with_args(
            "cmd-error-start-download",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::PauseDownload { error } => i18n.get_with_args(
            "cmd-error-pause-download",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::RemoveDownload { error } => i18n.get_with_args(
            "cmd-error-remove-download",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::ChangeFolder { error } => i18n.get_with_args(
            "cmd-error-change-folder",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::ValidationFailed { error } => i18n.get_with_args(
            "cmd-error-validation-failed",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::SaveConfig { error } => i18n.get_with_args(
            "cmd-error-save-config",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::FolderNotFound { folder } => i18n.get_with_args(
            "cmd-error-folder-not-found",
            Some(&fluent_args!["folder" => folder.clone()]),
        ),
        CommandError::ReloadScripts { error } => i18n.get_with_args(
            "cmd-error-reload-scripts",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::ScriptCommunication { error } => i18n.get_with_args(
            "cmd-error-script-communication",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::BlockingTask { error } => i18n.get_with_args(
            "cmd-error-blocking-task",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        CommandError::ReloadConfig { error } => i18n.get_with_args(
            "cmd-error-reload-config",
            Some(&fluent_args!["error" => error.clone()]),
        ),
        // Internal/developer-facing; no translation catalog entry.
        CommandError::Serialization { error } => error.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn en() -> LocalizationManager {
        LocalizationManager::new("en").expect("en locale loads")
    }

    #[test]
    fn duplicate_folder_name_interpolates_name() {
        let msg = localize_command_error(
            &en(),
            &CommandError::DuplicateFolderName {
                name: "Movies".to_string(),
            },
        );
        assert!(msg.contains("Movies"), "unexpected message: {msg}");
    }

    #[test]
    fn serialization_error_is_passed_through() {
        let msg = localize_command_error(
            &en(),
            &CommandError::Serialization {
                error: "bad json".to_string(),
            },
        );
        assert_eq!(msg, "bad json");
    }
}
