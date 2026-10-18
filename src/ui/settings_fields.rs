//! Settings field metadata shared by every front end.
//!
//! Only data that does not depend on a particular UI lives here: the field
//! lists, their translation keys and whether a change needs a restart. How the
//! fields are grouped into tabs or rows is up to each front end.

/// Application-level settings fields.
///
/// Order is display order, grouped General, then Downloads, then Scripts, so a
/// front end can filter `all()` by section without re-sorting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ApplicationSettingsField {
    Language,
    SkipDownloadPreview,
    AutoLaunchDnd,
    UserAgent,
    ReferrerPolicy,
    MaxRedirects,
    RetryCount,
    MaxConcurrent,
    MaxConcurrentPerFolder,
    MaxActiveFolders,
    ScriptsEnabled,
    DefaultDirectory,
}

impl ApplicationSettingsField {
    pub fn all() -> Vec<Self> {
        vec![
            Self::Language,
            Self::SkipDownloadPreview,
            Self::AutoLaunchDnd,
            Self::UserAgent,
            Self::ReferrerPolicy,
            Self::MaxRedirects,
            Self::RetryCount,
            Self::MaxConcurrent,
            Self::MaxConcurrentPerFolder,
            Self::MaxActiveFolders,
            Self::ScriptsEnabled,
            Self::DefaultDirectory,
        ]
    }

    /// Whether changing this field only takes effect after a restart.
    /// `MaxConcurrent` applies live via `DownloadManager::set_max_concurrent`,
    /// so it is not in this list; the others are read only at startup (the
    /// manager and its per-folder queues are built once).
    /// `ScriptsEnabled` is included because the script executor thread is
    /// only spawned when scripts are enabled at startup (`AppState::new_with_scripts`);
    /// turning it on later leaves `script_sender` `None` for the rest of the
    /// session, so scripts silently never run until a restart.
    /// `DefaultDirectory` is not in this list either: it is read from the
    /// shared config on every download/folder creation (`DownloadTask::new_with_folder`,
    /// `download::manager`), never cached at startup, so an edit applies to
    /// the next download without a restart.
    pub fn requires_restart(&self) -> bool {
        matches!(
            self,
            Self::Language
                | Self::AutoLaunchDnd
                | Self::RetryCount
                | Self::MaxConcurrentPerFolder
                | Self::MaxActiveFolders
                | Self::ScriptsEnabled
        )
    }

    /// Get translation key for label
    pub fn label_key(&self) -> &str {
        match self {
            Self::MaxConcurrent => "settings-app-max-concurrent",
            Self::MaxConcurrentPerFolder => "settings-app-max-concurrent-per-folder",
            Self::MaxActiveFolders => "settings-app-max-active-folders",
            Self::MaxRedirects => "settings-app-max-redirects",
            Self::RetryCount => "settings-app-retry-count",
            Self::UserAgent => "settings-app-user-agent",
            Self::ReferrerPolicy => "settings-app-referrer-policy",
            Self::ScriptsEnabled => "settings-app-scripts-enabled",
            Self::SkipDownloadPreview => "settings-app-skip-download-preview",
            Self::Language => "settings-app-language",
            Self::AutoLaunchDnd => "settings-app-auto-launch-dnd",
            Self::DefaultDirectory => "settings-app-default-directory",
        }
    }

    /// Get translation key for description
    pub fn description_key(&self) -> &str {
        match self {
            Self::MaxConcurrent => "settings-app-max-concurrent-desc",
            Self::MaxConcurrentPerFolder => "settings-app-max-concurrent-per-folder-desc",
            Self::MaxActiveFolders => "settings-app-max-active-folders-desc",
            Self::MaxRedirects => "settings-app-max-redirects-desc",
            Self::RetryCount => "settings-app-retry-count-desc",
            Self::UserAgent => "settings-app-user-agent-desc",
            Self::ReferrerPolicy => "settings-app-referrer-policy-desc",
            Self::ScriptsEnabled => "settings-app-scripts-enabled-desc",
            Self::SkipDownloadPreview => "settings-app-skip-download-preview-desc",
            Self::Language => "settings-app-language-desc",
            Self::AutoLaunchDnd => "settings-app-auto-launch-dnd-desc",
            Self::DefaultDirectory => "settings-app-default-directory-desc",
        }
    }
}

/// Folder-level settings fields
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsField {
    FolderName,
    FolderSavePath,
    FolderAutoDate,
    FolderAutoStart,
    FolderPreventDuplicateUrl,
    FolderScripts,
    FolderMaxConcurrent,
    FolderUserAgent,
    FolderReferrerPolicy,
    FolderHeaders,
}

impl SettingsField {
    /// All folder settings fields, in display order. The order must match the
    /// index mapping used by the folder settings handlers.
    pub fn all() -> Vec<Self> {
        vec![
            Self::FolderName,
            Self::FolderSavePath,
            Self::FolderAutoDate,
            Self::FolderAutoStart,
            Self::FolderPreventDuplicateUrl,
            Self::FolderScripts,
            Self::FolderMaxConcurrent,
            Self::FolderUserAgent,
            Self::FolderReferrerPolicy,
            Self::FolderHeaders,
        ]
    }

    /// Map a 0-based display index to its field.
    pub fn from_index(index: usize) -> Option<Self> {
        Self::all().into_iter().nth(index)
    }

    /// Get translation key for label
    pub fn label_key(&self) -> &str {
        match self {
            Self::FolderName => "settings-folder-name",
            Self::FolderSavePath => "settings-folder-save-path",
            Self::FolderAutoDate => "settings-folder-auto-date",
            Self::FolderAutoStart => "settings-folder-auto-start",
            Self::FolderPreventDuplicateUrl => "settings-folder-prevent-duplicate-url",
            Self::FolderScripts => "settings-folder-scripts",
            Self::FolderMaxConcurrent => "settings-folder-max-concurrent",
            Self::FolderUserAgent => "settings-folder-user-agent",
            Self::FolderReferrerPolicy => "settings-folder-referrer-policy",
            Self::FolderHeaders => "settings-folder-headers",
        }
    }

    /// Get translation key for description
    pub fn description_key(&self) -> &str {
        match self {
            Self::FolderName => "settings-folder-name-desc",
            Self::FolderSavePath => "settings-folder-save-path-desc",
            Self::FolderAutoDate => "settings-folder-auto-date-desc",
            Self::FolderAutoStart => "settings-folder-auto-start-desc",
            Self::FolderPreventDuplicateUrl => "settings-folder-prevent-duplicate-url-desc",
            Self::FolderScripts => "settings-folder-scripts-desc",
            Self::FolderMaxConcurrent => "settings-folder-max-concurrent-desc",
            Self::FolderUserAgent => "settings-folder-user-agent-desc",
            Self::FolderReferrerPolicy => "settings-folder-referrer-policy-desc",
            Self::FolderHeaders => "settings-folder-headers-desc",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::i18n::LocalizationManager;

    #[test]
    fn restart_fields_match_documented_set() {
        let restart: Vec<_> = ApplicationSettingsField::all()
            .into_iter()
            .filter(|f| f.requires_restart())
            .collect();
        assert_eq!(
            restart,
            vec![
                ApplicationSettingsField::Language,
                ApplicationSettingsField::AutoLaunchDnd,
                ApplicationSettingsField::RetryCount,
                ApplicationSettingsField::MaxConcurrentPerFolder,
                ApplicationSettingsField::MaxActiveFolders,
                ApplicationSettingsField::ScriptsEnabled,
            ]
        );
    }

    #[test]
    fn every_field_has_label_and_description_keys_in_en_us() {
        let i18n = LocalizationManager::new("en").expect("en-US is embedded");
        let keys = ApplicationSettingsField::all()
            .into_iter()
            .flat_map(|f| [f.label_key().to_string(), f.description_key().to_string()])
            .chain(SettingsField::all().into_iter().flat_map(|f| {
                [f.label_key().to_string(), f.description_key().to_string()]
            }));
        for key in keys {
            let text = i18n.get(&key);
            assert!(!text.contains("[missing:"), "key {key} does not resolve in en-US");
        }
    }
}
