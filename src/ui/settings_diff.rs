//! Turns an edited settings draft into the per-field [`Command`]s that apply it.
//!
//! Each `Update*` command carries its own validation and live side effects, so
//! a front end sends only the fields that differ instead of replacing the whole
//! config. The order is fixed so a failed run leaves a predictable prefix
//! applied and the caller can map the failing index back to a field.

use std::collections::{BTreeSet, HashMap};

use crate::app::config::{Config, FolderConfig};
use crate::ui::commands::Command;
use crate::ui::settings_fields::SettingsField;

/// Commands that turn `original` into `draft`, in application order.
///
/// Folders present in only one of the two configs are ignored: adding and
/// removing folders is done immediately through `CreateFolder` /
/// `DeleteFolder`, not through the draft. Default headers have no command and
/// are never compared.
pub fn settings_commands(original: &Config, draft: &Config) -> Vec<Command> {
    let mut out = Vec::new();

    // General
    if original.general.language != draft.general.language {
        out.push(Command::UpdateLanguage { value: draft.general.language.clone() });
    }
    if original.general.skip_download_preview != draft.general.skip_download_preview {
        out.push(Command::UpdateSkipDownloadPreview { value: draft.general.skip_download_preview });
    }
    if original.general.auto_launch_dnd != draft.general.auto_launch_dnd {
        out.push(Command::UpdateAutoLaunchDnd { value: draft.general.auto_launch_dnd });
    }

    // Downloads
    let (od, dd) = (&original.download, &draft.download);
    if od.default_directory != dd.default_directory {
        out.push(Command::UpdateDefaultDirectory { value: dd.default_directory.clone() });
    }
    if od.max_concurrent != dd.max_concurrent {
        out.push(Command::UpdateMaxConcurrent { value: dd.max_concurrent });
    }
    if od.max_concurrent_per_folder != dd.max_concurrent_per_folder {
        out.push(Command::UpdateMaxConcurrentPerFolder { value: dd.max_concurrent_per_folder });
    }
    if od.parallel_folder_count != dd.parallel_folder_count {
        out.push(Command::UpdateMaxActiveFolders { value: dd.parallel_folder_count });
    }
    if od.retry_count != dd.retry_count {
        out.push(Command::UpdateRetryCount { value: dd.retry_count });
    }

    // Connection
    if od.user_agent != dd.user_agent {
        out.push(Command::UpdateUserAgent { value: dd.user_agent.clone() });
    }
    if od.referrer_policy != dd.referrer_policy {
        out.push(Command::UpdateReferrerPolicy { policy: dd.referrer_policy.clone() });
    }
    if od.max_redirects != dd.max_redirects {
        out.push(Command::UpdateMaxRedirects { value: dd.max_redirects });
    }

    // Scripts
    if original.scripts.enabled != draft.scripts.enabled {
        out.push(Command::UpdateScriptsEnabled { value: draft.scripts.enabled });
    }
    for filename in sorted_union(&original.scripts.script_files, &draft.scripts.script_files) {
        // A missing entry means enabled, so compare effective values: an
        // explicit `true` and an absent key are the same setting.
        let before = original.scripts.script_files.get(&filename).copied().unwrap_or(true);
        let after = draft.scripts.script_files.get(&filename).copied().unwrap_or(true);
        if before != after {
            out.push(Command::SetScriptFileEnabled { filename, enabled: after });
        }
    }

    // Folders
    let mut folder_ids: Vec<&String> = draft.folders.keys().filter(|id| original.folders.contains_key(*id)).collect();
    folder_ids.sort();
    for id in folder_ids {
        folder_commands(id, &original.folders[id], &draft.folders[id], &mut out);
    }

    out
}

fn folder_commands(id: &str, original: &FolderConfig, draft: &FolderConfig, out: &mut Vec<Command>) {
    let folder_id = || id.to_string();
    for field in SettingsField::all() {
        match field {
            SettingsField::FolderName => {
                if original.name != draft.name {
                    out.push(Command::UpdateFolderName { folder_id: folder_id(), value: draft.name.clone() });
                }
            }
            SettingsField::FolderSavePath => {
                if original.save_path != draft.save_path {
                    out.push(Command::UpdateFolderSavePath { folder_id: folder_id(), value: draft.save_path.clone() });
                }
            }
            SettingsField::FolderAutoDate => {
                if original.auto_date_directory != draft.auto_date_directory {
                    out.push(Command::UpdateFolderAutoDateDirectory {
                        folder_id: folder_id(),
                        value: draft.auto_date_directory,
                    });
                }
            }
            SettingsField::FolderAutoStart => {
                if original.auto_start_downloads != draft.auto_start_downloads {
                    out.push(Command::UpdateFolderAutoStartDownloads {
                        folder_id: folder_id(),
                        value: draft.auto_start_downloads,
                    });
                }
            }
            SettingsField::FolderPreventDuplicateUrl => {
                if original.prevent_duplicate_url != draft.prevent_duplicate_url {
                    out.push(Command::UpdateFolderPreventDuplicateUrl {
                        folder_id: folder_id(),
                        value: draft.prevent_duplicate_url,
                    });
                }
            }
            SettingsField::FolderScripts => {
                if original.scripts_enabled != draft.scripts_enabled {
                    out.push(Command::UpdateFolderScriptsEnabled {
                        folder_id: folder_id(),
                        value: draft.scripts_enabled,
                    });
                }
                // Per-file overrides belong to the same Scripts row, so they
                // follow it directly.
                let empty = HashMap::new();
                let before = original.script_files.as_ref().unwrap_or(&empty);
                let after = draft.script_files.as_ref().unwrap_or(&empty);
                for filename in sorted_union(before, after) {
                    let (b, a) = (before.get(&filename).copied(), after.get(&filename).copied());
                    if b != a {
                        out.push(Command::SetFolderScriptFileOverride {
                            folder_id: folder_id(),
                            filename,
                            value: a,
                        });
                    }
                }
            }
            SettingsField::FolderMaxConcurrent => {
                if original.max_concurrent != draft.max_concurrent {
                    out.push(Command::UpdateFolderMaxConcurrent {
                        folder_id: folder_id(),
                        value: draft.max_concurrent,
                    });
                }
            }
            SettingsField::FolderUserAgent => {
                if original.user_agent != draft.user_agent {
                    out.push(Command::UpdateFolderUserAgent {
                        folder_id: folder_id(),
                        value: draft.user_agent.clone(),
                    });
                }
            }
            SettingsField::FolderReferrerPolicy => {
                if original.referrer_policy != draft.referrer_policy {
                    out.push(Command::UpdateFolderReferrerPolicy {
                        folder_id: folder_id(),
                        policy: draft.referrer_policy.clone(),
                    });
                }
            }
            // Read-only in the settings UI; no command edits it.
            SettingsField::FolderHeaders => {}
        }
    }
}

fn sorted_union(a: &HashMap<String, bool>, b: &HashMap<String, bool>) -> Vec<String> {
    a.keys().chain(b.keys()).cloned().collect::<BTreeSet<_>>().into_iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app::config::ReferrerPolicy;

    fn config_with_folders(ids: &[&str]) -> Config {
        let mut config = Config::default();
        for id in ids {
            config.folders.insert(id.to_string(), FolderConfig { name: id.to_string(), ..FolderConfig::default() });
        }
        config
    }

    #[test]
    fn identical_configs_yield_no_commands() {
        let config = config_with_folders(&["a", "b"]);
        assert!(settings_commands(&config, &config.clone()).is_empty());
    }

    #[test]
    fn changed_max_concurrent_yields_update_command() {
        let original = Config::default();
        let mut draft = original.clone();
        draft.download.max_concurrent = original.download.max_concurrent + 2;

        let commands = settings_commands(&original, &draft);
        assert_eq!(commands, vec![Command::UpdateMaxConcurrent { value: draft.download.max_concurrent }]);
    }

    #[test]
    fn folder_script_override_to_inherit_yields_none_value() {
        let mut original = config_with_folders(&["a"]);
        original.folders.get_mut("a").unwrap().script_files =
            Some(HashMap::from([("x.js".to_string(), false)]));
        let mut draft = original.clone();
        draft.folders.get_mut("a").unwrap().script_files = None;

        let commands = settings_commands(&original, &draft);
        assert_eq!(
            commands,
            vec![Command::SetFolderScriptFileOverride {
                folder_id: "a".to_string(),
                filename: "x.js".to_string(),
                value: None,
            }]
        );
    }

    #[test]
    fn folder_only_in_draft_is_ignored() {
        let original = config_with_folders(&["a"]);
        let mut draft = original.clone();
        draft.folders.insert("new".to_string(), FolderConfig { name: "new".to_string(), ..FolderConfig::default() });
        // A folder removed from the draft is ignored too.
        let mut draft_without = original.clone();
        draft_without.folders.clear();

        assert!(settings_commands(&original, &draft).is_empty());
        assert!(settings_commands(&original, &draft_without).is_empty());
    }

    #[test]
    fn order_is_stable() {
        let original = config_with_folders(&["a", "b"]);
        let mut draft = original.clone();

        // Folder b first in insertion terms, to show the output is sorted by id.
        {
            let b = draft.folders.get_mut("b").unwrap();
            b.name = "b2".to_string();
            b.max_concurrent = Some(1);
        }
        {
            let a = draft.folders.get_mut("a").unwrap();
            a.name = "a2".to_string();
            a.prevent_duplicate_url = true;
            a.scripts_enabled = Some(false);
            a.script_files = Some(HashMap::from([("z.js".to_string(), true), ("m.js".to_string(), false)]));
        }
        draft.scripts.script_files.insert("y.js".to_string(), false);
        draft.scripts.script_files.insert("c.js".to_string(), false);
        draft.scripts.enabled = !original.scripts.enabled;
        draft.download.max_redirects += 1;
        draft.download.referrer_policy = ReferrerPolicy::custom("x");
        draft.download.user_agent = "ua".to_string();
        draft.download.retry_count += 1;
        draft.download.parallel_folder_count = Some(2);
        draft.download.max_concurrent_per_folder = Some(1);
        draft.download.max_concurrent += 1;
        draft.download.default_directory = std::path::PathBuf::from("elsewhere");
        draft.general.auto_launch_dnd = !original.general.auto_launch_dnd;
        draft.general.skip_download_preview = !original.general.skip_download_preview;
        draft.general.language = "ja".to_string();

        let expected = vec![
            Command::UpdateLanguage { value: "ja".to_string() },
            Command::UpdateSkipDownloadPreview { value: draft.general.skip_download_preview },
            Command::UpdateAutoLaunchDnd { value: draft.general.auto_launch_dnd },
            Command::UpdateDefaultDirectory { value: std::path::PathBuf::from("elsewhere") },
            Command::UpdateMaxConcurrent { value: draft.download.max_concurrent },
            Command::UpdateMaxConcurrentPerFolder { value: Some(1) },
            Command::UpdateMaxActiveFolders { value: Some(2) },
            Command::UpdateRetryCount { value: draft.download.retry_count },
            Command::UpdateUserAgent { value: "ua".to_string() },
            Command::UpdateReferrerPolicy { policy: ReferrerPolicy::custom("x") },
            Command::UpdateMaxRedirects { value: draft.download.max_redirects },
            Command::UpdateScriptsEnabled { value: draft.scripts.enabled },
            Command::SetScriptFileEnabled { filename: "c.js".to_string(), enabled: false },
            Command::SetScriptFileEnabled { filename: "y.js".to_string(), enabled: false },
            Command::UpdateFolderName { folder_id: "a".to_string(), value: "a2".to_string() },
            Command::UpdateFolderPreventDuplicateUrl { folder_id: "a".to_string(), value: true },
            Command::UpdateFolderScriptsEnabled { folder_id: "a".to_string(), value: Some(false) },
            Command::SetFolderScriptFileOverride {
                folder_id: "a".to_string(),
                filename: "m.js".to_string(),
                value: Some(false),
            },
            Command::SetFolderScriptFileOverride {
                folder_id: "a".to_string(),
                filename: "z.js".to_string(),
                value: Some(true),
            },
            Command::UpdateFolderName { folder_id: "b".to_string(), value: "b2".to_string() },
            Command::UpdateFolderMaxConcurrent { folder_id: "b".to_string(), value: Some(1) },
        ];
        assert_eq!(settings_commands(&original, &draft), expected);
        // Same inputs, same output.
        assert_eq!(settings_commands(&original, &draft), expected);
    }
}
