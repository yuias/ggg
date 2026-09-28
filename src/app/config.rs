use crate::app::keybindings::KeybindingsConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::PathBuf;
use uuid::Uuid;

/// Policy for computing the Referrer header on HTTP requests.
///
/// Simple variants serialize as plain strings (`"none"`, `"same_as_url"`, etc.)
/// while `Custom` serializes as `{ type = "custom", value = "..." }`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum ReferrerPolicy {
    /// Simple string variants
    Simple(ReferrerPolicyKind),
    /// Custom referrer value: `{ type = "custom", value = "..." }`
    Custom {
        #[serde(rename = "type")]
        kind: CustomTag,
        value: String,
    },
}

/// Tag used to identify the custom variant in TOML
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CustomTag {
    Custom,
}

/// Simple referrer policy variants (serialized as lowercase strings)
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReferrerPolicyKind {
    /// No Referrer header
    None,
    /// Use the full download URL as Referrer
    SameAsUrl,
    /// Use the URL path (e.g. `https://foo/bar/` from `https://foo/bar/file.jpg`)
    UrlPath,
    /// Use the URL origin (e.g. `https://foo/` from `https://foo/bar/file.jpg`)
    UrlOrigin,
}

impl Default for ReferrerPolicy {
    fn default() -> Self {
        Self::Simple(ReferrerPolicyKind::None)
    }
}

impl ReferrerPolicy {
    /// Convenience constructors
    pub fn none() -> Self {
        Self::Simple(ReferrerPolicyKind::None)
    }
    pub fn same_as_url() -> Self {
        Self::Simple(ReferrerPolicyKind::SameAsUrl)
    }
    pub fn url_path() -> Self {
        Self::Simple(ReferrerPolicyKind::UrlPath)
    }
    pub fn url_origin() -> Self {
        Self::Simple(ReferrerPolicyKind::UrlOrigin)
    }
    pub fn custom(value: impl Into<String>) -> Self {
        Self::Custom {
            kind: CustomTag::Custom,
            value: value.into(),
        }
    }

    /// Compute the actual Referrer header value for a given download URL.
    /// Returns `None` for the `None` policy or if the URL is invalid.
    pub fn compute(&self, url: &str) -> Option<String> {
        match self {
            Self::Simple(ReferrerPolicyKind::None) => Option::None,
            Self::Simple(ReferrerPolicyKind::SameAsUrl) => Some(url.to_string()),
            Self::Simple(ReferrerPolicyKind::UrlPath) => {
                let parsed = url::Url::parse(url).ok()?;
                let path = parsed.path();
                // Strip the last path segment (filename)
                let parent = match path.rfind('/') {
                    Some(pos) => &path[..=pos],
                    Option::None => "/",
                };
                Some(format!("{}://{}{}", parsed.scheme(), parsed.authority(), parent))
            }
            Self::Simple(ReferrerPolicyKind::UrlOrigin) => {
                let parsed = url::Url::parse(url).ok()?;
                Some(parsed.origin().ascii_serialization() + "/")
            }
            Self::Custom { value, .. } => {
                if value.is_empty() {
                    Option::None
                } else {
                    Some(value.clone())
                }
            }
        }
    }

    /// Get the next policy in the cycle (for TUI editing).
    /// Cycle: None → SameAsUrl → UrlPath → UrlOrigin → None
    /// Custom is not included in the cycle; it is set separately.
    pub fn cycle_next(&self) -> Self {
        match self {
            Self::Simple(ReferrerPolicyKind::None) => Self::same_as_url(),
            Self::Simple(ReferrerPolicyKind::SameAsUrl) => Self::url_path(),
            Self::Simple(ReferrerPolicyKind::UrlPath) => Self::url_origin(),
            Self::Simple(ReferrerPolicyKind::UrlOrigin) | Self::Custom { .. } => Self::none(),
        }
    }

    /// Get a display label key for i18n
    pub fn display_key(&self) -> &str {
        match self {
            Self::Simple(ReferrerPolicyKind::None) => "settings-referrer-none",
            Self::Simple(ReferrerPolicyKind::SameAsUrl) => "settings-referrer-same-as-url",
            Self::Simple(ReferrerPolicyKind::UrlPath) => "settings-referrer-url-path",
            Self::Simple(ReferrerPolicyKind::UrlOrigin) => "settings-referrer-url-origin",
            Self::Custom { .. } => "settings-referrer-custom",
        }
    }
}

/// UI-only configuration consumed exclusively by the interactive front-end.
///
/// Grouped apart from the download/runtime settings so a non-TUI front-end (or
/// the download core extracted as its own crate) can ignore it wholesale. It is
/// `#[serde(flatten)]`-ed into the parent config, so on disk the keybindings
/// still live under the top-level `[keybindings]` table — this is purely a
/// code-level grouping with no change to the config file format.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UiConfig {
    #[serde(default)]
    pub keybindings: KeybindingsConfig,
}

/// Application-level configuration (saved to config/settings.toml)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApplicationConfig {
    pub general: GeneralConfig,
    pub download: DownloadConfig,
    pub network: NetworkConfig,
    pub scripts: ScriptConfig,
    #[serde(flatten)]
    pub ui: UiConfig,
}

/// Complete configuration (Application settings + Folder settings)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Config {
    pub general: GeneralConfig,
    pub download: DownloadConfig,
    pub network: NetworkConfig,
    pub scripts: ScriptConfig,
    #[serde(flatten)]
    pub ui: UiConfig,
    #[serde(default)]
    pub folders: HashMap<String, FolderConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GeneralConfig {
    pub language: String,
    pub theme: String,
    pub minimize_to_tray: bool,
    pub start_minimized: bool,
    #[serde(default = "default_skip_download_preview")]
    pub skip_download_preview: bool,
    /// Auto-launch ggg-dnd GUI on startup (Windows only)
    #[serde(default)]
    pub auto_launch_dnd: bool,
}

fn default_skip_download_preview() -> bool {
    true
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadConfig {
    pub default_directory: PathBuf,
    pub max_concurrent: usize,
    pub retry_count: u32,
    pub retry_delay: u64,
    pub user_agent: String,
    pub bandwidth_limit: u64,
    #[serde(default)]
    pub max_concurrent_per_folder: Option<usize>,
    #[serde(default)]
    pub parallel_folder_count: Option<usize>,
    #[serde(default = "default_max_redirects")]
    pub max_redirects: u32,
    #[serde(default)]
    pub referrer_policy: ReferrerPolicy,
    /// How long `pause`/`remove`/`move` wait for a task's claim (usually a
    /// start running its beforeRequest script) before giving up. Hand-edited
    /// in settings.toml under `[download]`; not exposed in the settings UI.
    #[serde(default = "default_claim_wait_ms")]
    pub claim_wait_ms: u64,
}

fn default_max_redirects() -> u32 {
    5
}

/// Single source of truth for the claim-wait default, shared with
/// `download::manager::CLAIM_WAIT_BUDGET` so the two never drift apart.
pub const DEFAULT_CLAIM_WAIT_MS: u64 = 2000;

fn default_claim_wait_ms() -> u64 {
    DEFAULT_CLAIM_WAIT_MS
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NetworkConfig {
    pub proxy_enabled: bool,
    pub proxy_type: String,
    pub proxy_host: String,
    pub proxy_port: u16,
    pub proxy_auth: bool,
    pub proxy_user: String,
    pub proxy_pass: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScriptConfig {
    pub enabled: bool,
    pub directory: PathBuf,
    pub timeout: u64,
    /// Per-script file enable/disable settings
    /// Maps filename (without path) to enabled status
    #[serde(default)]
    pub script_files: HashMap<String, bool>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FolderConfig {
    /// Display name for the folder (user-visible)
    #[serde(default)]
    pub name: String,
    pub save_path: PathBuf,
    #[serde(default)]
    pub auto_date_directory: bool,
    #[serde(default)]
    pub auto_start_downloads: bool,
    #[serde(default)]
    pub scripts_enabled: Option<bool>,
    #[serde(default)]
    pub script_files: Option<HashMap<String, bool>>,
    #[serde(default)]
    pub max_concurrent: Option<usize>,
    #[serde(default)]
    pub user_agent: Option<String>,
    #[serde(default)]
    pub referrer_policy: Option<ReferrerPolicy>,
    #[serde(default)]
    pub default_headers: HashMap<String, String>,
    /// Prevent adding downloads with duplicate URLs already in the folder queue
    #[serde(default)]
    pub prevent_duplicate_url: bool,
}

impl Default for FolderConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            save_path: crate::util::paths::resolve_default_download_directory(),
            auto_date_directory: false,
            auto_start_downloads: false,
            scripts_enabled: None,
            script_files: None,
            max_concurrent: None,
            user_agent: None,
            referrer_policy: None,
            default_headers: HashMap::new(),
            prevent_duplicate_url: false,
        }
    }
}

impl FolderConfig {
    /// Create a new FolderConfig with a display name
    pub fn new_with_name(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            ..Self::default()
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self {
            general: GeneralConfig {
                language: "en".to_string(),
                theme: "classic".to_string(),
                minimize_to_tray: true,
                start_minimized: false,
                skip_download_preview: true,
                auto_launch_dnd: false,
            },
            download: DownloadConfig {
                default_directory: crate::util::paths::resolve_default_download_directory(),
                max_concurrent: 3,
                retry_count: 3,
                retry_delay: 5,
                user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string(),
                bandwidth_limit: 0,
                max_concurrent_per_folder: None,
                parallel_folder_count: None,
                max_redirects: 5,
                referrer_policy: ReferrerPolicy::default(),
                claim_wait_ms: default_claim_wait_ms(),
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
                directory: crate::util::paths::resolve_default_scripts_directory(),
                timeout: 30,
                script_files: HashMap::new(),
            },
            ui: UiConfig::default(),
            folders: HashMap::new(),
        }
    }
}

/// Name that identifies the protected default folder. `Config::load` (below)
/// recreates a folder with this name whenever none exists, so anything that
/// needs to know whether a folder *is* the default folder (rename/delete
/// guards) must check against this same value rather than inventing its own
/// rule.
pub const DEFAULT_FOLDER_NAME: &str = "default";

impl Config {
    /// Whether `folder_id` is the protected default folder, using the exact
    /// rule `load` uses to decide whether to recreate one (see step 4 below).
    pub fn is_default_folder(&self, folder_id: &str) -> bool {
        self.folders
            .get(folder_id)
            .is_some_and(|f| f.name == DEFAULT_FOLDER_NAME)
    }

    /// Look up folder display name by UUID key.
    /// Returns the folder's `name` field, or the key itself as fallback.
    pub fn folder_name(&self, folder_id: &str) -> String {
        self.folders
            .get(folder_id)
            .map(|f| {
                if f.name.is_empty() {
                    folder_id.to_string()
                } else {
                    f.name.clone()
                }
            })
            .unwrap_or_else(|| folder_id.to_string())
    }

    /// Find folder UUID key by display name.
    /// Returns None if no folder has the given name.
    pub fn find_folder_id_by_name(&self, name: &str) -> Option<String> {
        self.folders
            .iter()
            .find(|(_, f)| f.name == name)
            .map(|(k, _)| k.clone())
    }

    /// Get sorted list of (folder_id, display_name) pairs
    pub fn sorted_folder_entries(&self) -> Vec<(String, String)> {
        let mut entries: Vec<(String, String)> = self
            .folders
            .iter()
            .map(|(id, f)| {
                let display = if f.name.is_empty() {
                    id.clone()
                } else {
                    f.name.clone()
                };
                (id.clone(), display)
            })
            .collect();
        entries.sort_by(|a, b| a.1.cmp(&b.1));
        entries
    }

    /// Generate a new UUID-based folder key
    pub fn generate_folder_id() -> String {
        Uuid::new_v4().to_string()
    }

    /// Load configuration from multi-file structure
    pub fn load() -> anyhow::Result<Self> {
        // Step 1: Load application-level config
        let app_config = Self::load_application_config()?;

        // Step 2: Load all folder configs
        let folders = Self::load_all_folder_configs()?;

        // Step 3: Migrate legacy name-based keys to UUID keys
        let (mut folders, migrated) = Self::migrate_folder_keys(folders);

        // Step 4: Ensure a "default" folder exists (any folder named "default")
        let has_default = folders.values().any(|f| f.name == DEFAULT_FOLDER_NAME);
        if !has_default {
            tracing::info!("Creating default folder config");
            let default_id = Self::generate_folder_id();
            folders.insert(
                default_id,
                FolderConfig {
                    name: DEFAULT_FOLDER_NAME.to_string(),
                    save_path: app_config.download.default_directory.clone(),
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
        }

        // Step 5: Construct Config
        let config = Self {
            general: app_config.general,
            download: app_config.download,
            network: app_config.network,
            scripts: app_config.scripts,
            ui: app_config.ui,
            folders,
        };

        // Step 6: Validate
        if let Err(errors) = crate::app::settings::validate_folder_config(&config) {
            return Err(anyhow::anyhow!(
                "Invalid configuration: {}",
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        // Step 7: Auto-save after migration, and after creating a fresh
        // "default" folder, to persist the UUID key. Without this, a
        // freshly created default folder's key lives only in memory: the
        // next `load()` doesn't find it (no folder named "default" was ever
        // saved with a key), so it invents another UUID, and so on. Every
        // process start would then queue tasks under a different key, each
        // with no persisted settings.
        if migrated || !has_default {
            tracing::info!("Saving folder configs after migration or default-folder creation");
            if let Err(e) = config.save() {
                tracing::error!("Failed to save folder configs: {}", e);
            }
        }

        Ok(config)
    }

    /// Save configuration to multi-file structure
    pub fn save(&self) -> anyhow::Result<()> {
        // Step 1: Validate before saving
        if let Err(errors) = crate::app::settings::validate_folder_config(self) {
            return Err(anyhow::anyhow!(
                "Cannot save invalid config: {}",
                errors
                    .iter()
                    .map(|e| e.to_string())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }

        // Step 2: Save application-level config
        Self::save_application_config(self)?;

        // Step 3: Save each folder config
        for (folder_name, folder_config) in &self.folders {
            Self::save_folder_config(folder_name, folder_config)?;
        }

        // Step 4: Make save authoritative — remove on-disk folder directories
        // that are no longer in `self.folders`, so a removed folder is not
        // resurrected by `load_all_folder_configs` on the next launch.
        Self::remove_orphan_folder_dirs(&self.folders)?;

        Ok(())
    }

    /// Remove folder directories on disk whose key is not present in `folders`.
    fn remove_orphan_folder_dirs(
        folders: &HashMap<String, FolderConfig>,
    ) -> anyhow::Result<()> {
        let config_dir = crate::util::paths::find_config_directory()?;
        if !config_dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(&config_dir)? {
            let entry = entry?;
            let path = entry.path();
            // Only consider folder dirs (those holding a settings.toml).
            if !path.is_dir() || !path.join("settings.toml").exists() {
                continue;
            }
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if !folders.contains_key(name) {
                    match std::fs::remove_dir_all(&path) {
                        Ok(()) => tracing::info!("Removed orphaned folder directory: {}", name),
                        Err(e) => tracing::warn!(
                            "Failed to remove orphaned folder directory '{}': {}",
                            name,
                            e
                        ),
                    }
                }
            }
        }

        Ok(())
    }

    // --- Helper Methods ---

    fn load_application_config() -> anyhow::Result<ApplicationConfig> {
        use anyhow::Context;

        let config_path = crate::util::paths::get_app_config_path()?;

        if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .context(format!("Failed to read {:?}", config_path))?;
            let mut app_config: ApplicationConfig = toml::from_str(&content)
                .context(format!("Failed to parse {:?}", config_path))?;

            // Resolve relative scripts directory against config directory
            app_config.scripts.directory =
                crate::util::paths::resolve_relative_to_config(&app_config.scripts.directory);

            Ok(app_config)
        } else {
            tracing::info!("Application config not found, using defaults");
            Ok(ApplicationConfig {
                general: GeneralConfig {
                    language: "en".to_string(),
                    theme: "classic".to_string(),
                    minimize_to_tray: true,
                    start_minimized: false,
                    skip_download_preview: true,
                    auto_launch_dnd: false,
                },
                download: DownloadConfig {
                    default_directory: crate::util::paths::resolve_default_download_directory(),
                    max_concurrent: 3,
                    retry_count: 3,
                    retry_delay: 5,
                    user_agent: "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36".to_string(),
                    bandwidth_limit: 0,
                    max_concurrent_per_folder: None,
                    parallel_folder_count: None,
                    max_redirects: 5,
                    referrer_policy: ReferrerPolicy::default(),
                    claim_wait_ms: default_claim_wait_ms(),
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
                    directory: crate::util::paths::resolve_default_scripts_directory(),
                    timeout: 30,
                    script_files: HashMap::new(),
                },
                ui: UiConfig::default(),
            })
        }
    }

    fn save_application_config(&self) -> anyhow::Result<()> {
        use anyhow::Context;

        let config_path = crate::util::paths::get_app_config_path()?;

        // Ensure parent directory exists
        if let Some(parent) = config_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let app_config = ApplicationConfig {
            general: self.general.clone(),
            download: self.download.clone(),
            network: self.network.clone(),
            scripts: self.scripts.clone(),
            ui: self.ui.clone(),
        };

        let content = toml::to_string_pretty(&app_config)?;

        // Atomic write using temp file + rename
        let temp_path = config_path.with_extension("toml.tmp");
        std::fs::write(&temp_path, &content)
            .context("Failed to write temp config file")?;
        std::fs::rename(&temp_path, &config_path)
            .context("Failed to rename temp config file")?;

        tracing::info!("Saved application config to {:?}", config_path);
        Ok(())
    }

    fn load_all_folder_configs() -> anyhow::Result<HashMap<String, FolderConfig>> {
        let config_dir = crate::util::paths::find_config_directory()?;
        let mut folders = HashMap::new();

        // Scan for subdirectories in config/
        if !config_dir.exists() {
            return Ok(folders);
        }

        for entry in std::fs::read_dir(&config_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                // Only treat directories containing settings.toml as folders
                if !path.join("settings.toml").exists() {
                    continue;
                }

                let folder_name = path
                    .file_name()
                    .and_then(|n| n.to_str())
                    .ok_or_else(|| anyhow::anyhow!("Invalid folder name"))?
                    .to_string();

                match Self::load_folder_config(&folder_name) {
                    Ok(folder_config) => {
                        folders.insert(folder_name.clone(), folder_config);
                        tracing::debug!("Loaded folder config: {}", folder_name);
                    }
                    Err(e) => {
                        tracing::warn!("Failed to load folder '{}': {}", folder_name, e);
                        // Continue loading other folders
                    }
                }
            }
        }

        Ok(folders)
    }

    fn load_folder_config(folder_name: &str) -> anyhow::Result<FolderConfig> {
        use anyhow::Context;

        let folder_path = crate::util::paths::get_folder_config_path(folder_name)?;

        if folder_path.exists() {
            let content = std::fs::read_to_string(&folder_path)?;
            toml::from_str(&content)
                .context(format!("Failed to parse folder config: {}", folder_name))
        } else {
            // No config file - use defaults
            Ok(FolderConfig::default())
        }
    }

    fn save_folder_config(
        folder_name: &str,
        folder_config: &FolderConfig,
    ) -> anyhow::Result<()> {
        use anyhow::Context;

        let folder_path = crate::util::paths::get_folder_config_path(folder_name)?;

        // Ensure parent directory exists
        if let Some(parent) = folder_path.parent() {
            std::fs::create_dir_all(parent)?;
        }

        let content = toml::to_string_pretty(folder_config)?;

        // Atomic write
        let temp_path = folder_path.with_extension("toml.tmp");
        std::fs::write(&temp_path, &content)
            .context("Failed to write temp folder config")?;
        std::fs::rename(&temp_path, &folder_path)
            .context("Failed to rename temp folder config")?;

        tracing::debug!("Saved folder config: {}", folder_name);
        Ok(())
    }

    /// Migrate legacy folder configs: name-based keys → UUID keys.
    ///
    /// If a folder key is NOT a valid UUID, it is treated as a legacy name-based
    /// key. The folder is re-keyed with a new UUID, the old key becomes the
    /// `name` field, and the config directory is renamed on disk.
    ///
    /// Returns `(migrated_folders, did_migrate)`.
    fn migrate_folder_keys(
        folders: HashMap<String, FolderConfig>,
    ) -> (HashMap<String, FolderConfig>, bool) {
        let mut migrated = HashMap::new();
        let mut did_migrate = false;
        // Collect old→new mappings for directory renames
        let mut renames: Vec<(String, String)> = Vec::new();

        for (key, mut folder_config) in folders {
            if Uuid::parse_str(&key).is_ok() {
                // Already a UUID key — ensure name is populated
                if folder_config.name.is_empty() {
                    folder_config.name = key.clone();
                }
                migrated.insert(key, folder_config);
            } else {
                // Legacy name-based key — assign UUID
                let new_id = Self::generate_folder_id();
                tracing::info!(
                    "Migrating folder '{}' → UUID '{}'",
                    key,
                    new_id
                );
                folder_config.name = key.clone();
                renames.push((key, new_id.clone()));
                migrated.insert(new_id, folder_config);
                did_migrate = true;
            }
        }

        // Rename config directories on disk
        if did_migrate {
            if let Ok(config_dir) = crate::util::paths::find_config_directory() {
                for (old_name, new_id) in &renames {
                    let old_dir = config_dir.join(old_name);
                    let new_dir = config_dir.join(new_id);
                    if old_dir.exists() && old_dir.is_dir() {
                        if let Err(e) = std::fs::rename(&old_dir, &new_dir) {
                            tracing::error!(
                                "Failed to rename folder dir '{}' → '{}': {}",
                                old_dir.display(),
                                new_dir.display(),
                                e
                            );
                        } else {
                            tracing::info!(
                                "Renamed folder dir: {} → {}",
                                old_dir.display(),
                                new_dir.display()
                            );
                        }
                    }
                }
            }
        }

        (migrated, did_migrate)
    }

    /// Get the migration mapping from old name-based keys to new UUID keys.
    /// Used to update queue files after migration.
    pub fn get_migration_map(
        folders: &HashMap<String, FolderConfig>,
    ) -> HashMap<String, String> {
        // Returns name → uuid mapping for all folders
        let mut map = HashMap::new();
        for (id, config) in folders {
            if !config.name.is_empty() {
                map.insert(config.name.clone(), id.clone());
            }
        }
        map
    }

    #[cfg(test)]
    fn load_from(path: &std::path::Path) -> anyhow::Result<Self> {
        if path.exists() {
            let content = std::fs::read_to_string(path)?;
            Ok(toml::from_str(&content)?)
        } else {
            Ok(Self::default())
        }
    }

    #[cfg(test)]
    fn save_to(&self, path: &std::path::Path) -> anyhow::Result<()> {
        let content = toml::to_string_pretty(self)?;
        std::fs::write(path, content)?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pretty_assertions::assert_eq;
    use serial_test::serial;

    fn create_test_config_toml() -> &'static str {
        r#"
[general]
language = "ja"
theme = "classic"
minimize_to_tray = false
start_minimized = true
skip_download_preview = false

[download]
default_directory = "D:\\MyDownloads"
max_concurrent = 5
retry_count = 10
retry_delay = 3
user_agent = "CustomAgent/1.0"
bandwidth_limit = 1000000
max_concurrent_per_folder = 3
parallel_folder_count = 2
max_redirects = 10

[network]
proxy_enabled = true
proxy_type = "socks5"
proxy_host = "localhost"
proxy_port = 1080
proxy_auth = true
proxy_user = "testuser"
proxy_pass = "testpass"

[scripts]
enabled = false
directory = "./custom_scripts"
timeout = 60
"#
    }

    #[test]
    #[serial]
    fn test_config_default_values() {
        let config = Config::default();

        assert_eq!(config.general.language, "en");
        assert_eq!(config.general.theme, "classic");
        assert_eq!(config.general.minimize_to_tray, true);
        assert_eq!(config.general.start_minimized, false);

        assert_eq!(config.download.default_directory, crate::util::paths::resolve_default_download_directory());
        assert_eq!(config.download.max_concurrent, 3);
        assert_eq!(config.download.retry_count, 3);
        assert_eq!(config.download.retry_delay, 5);
        assert_eq!(config.download.bandwidth_limit, 0);

        assert_eq!(config.network.proxy_enabled, false);
        assert_eq!(config.network.proxy_type, "http");
        assert_eq!(config.network.proxy_port, 8080);

        assert_eq!(config.scripts.enabled, true);
        assert_eq!(config.scripts.directory, crate::util::paths::resolve_default_scripts_directory());
        assert_eq!(config.scripts.timeout, 30);
    }

    #[test]
    fn test_config_load_missing_file_uses_default() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("nonexistent.toml");

        // Load from non-existent path should return defaults
        let config = Config::load_from(&config_path).unwrap();

        // Should use defaults
        assert_eq!(config.general.language, "en");
        assert_eq!(config.download.max_concurrent, 3);
    }

    #[test]
    fn test_config_load_valid_toml() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        std::fs::write(&config_path, create_test_config_toml()).unwrap();

        let config = Config::load_from(&config_path).unwrap();

        assert_eq!(config.general.language, "ja");
        assert_eq!(config.general.theme, "classic");
        assert_eq!(config.general.minimize_to_tray, false);
        assert_eq!(config.general.start_minimized, true);
        assert_eq!(config.general.skip_download_preview, false);

        assert_eq!(config.download.default_directory, PathBuf::from("D:\\MyDownloads"));
        assert_eq!(config.download.max_concurrent, 5);
        assert_eq!(config.download.retry_count, 10);
        assert_eq!(config.download.retry_delay, 3);
        assert_eq!(config.download.max_redirects, 10);
        assert_eq!(config.download.max_concurrent_per_folder, Some(3));
        assert_eq!(config.download.parallel_folder_count, Some(2));

        assert_eq!(config.network.proxy_enabled, true);
        assert_eq!(config.network.proxy_type, "socks5");
        assert_eq!(config.network.proxy_host, "localhost");
        assert_eq!(config.network.proxy_port, 1080);
        assert_eq!(config.network.proxy_auth, true);
        assert_eq!(config.network.proxy_user, "testuser");
        assert_eq!(config.network.proxy_pass, "testpass");

        assert_eq!(config.scripts.enabled, false);
        assert_eq!(config.scripts.timeout, 60);
        assert_eq!(config.scripts.directory, PathBuf::from("./custom_scripts"));
    }

    #[test]
    fn test_config_load_invalid_toml_fails() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        // Write invalid TOML
        std::fs::write(&config_path, "this is not valid toml [[[").unwrap();

        let result = Config::load_from(&config_path);
        assert!(result.is_err());
    }

    #[test]
    fn test_config_save_creates_toml() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let config = Config::default();
        config.save_to(&config_path).unwrap();

        assert!(config_path.exists());
    }

    #[test]
    fn test_config_save_load_roundtrip() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let mut config = Config::default();
        config.general.language = "en".to_string();
        config.download.max_concurrent = 7;
        config.network.proxy_enabled = true;

        config.save_to(&config_path).unwrap();

        let loaded_config = Config::load_from(&config_path).unwrap();

        assert_eq!(loaded_config.general.language, "en");
        assert_eq!(loaded_config.download.max_concurrent, 7);
        assert_eq!(loaded_config.network.proxy_enabled, true);
    }

    #[test]
    fn test_config_all_sections_present() {
        let config = Config::default();

        // Verify all sections exist and have expected structure
        let _ = config.general.language;
        let _ = config.download.default_directory;
        let _ = config.network.proxy_enabled;
        let _ = config.scripts.enabled;
    }

    #[test]
    fn test_config_custom_values_preserved() {
        let temp_dir = tempfile::tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        let mut config = Config::default();
        config.general.theme = "modern".to_string();
        config.download.retry_count = 99;
        config.network.proxy_port = 9999;
        config.scripts.directory = PathBuf::from("/custom/path");

        config.save_to(&config_path).unwrap();

        let loaded = Config::load_from(&config_path).unwrap();

        assert_eq!(loaded.general.theme, "modern");
        assert_eq!(loaded.download.retry_count, 99);
        assert_eq!(loaded.network.proxy_port, 9999);
        assert_eq!(loaded.scripts.directory, PathBuf::from("/custom/path"));
    }

    #[test]
    fn test_application_config_serialization() {
        let app_config = ApplicationConfig {
            general: GeneralConfig {
                language: "en".to_string(),
                theme: "dark".to_string(),
                minimize_to_tray: false,
                start_minimized: true,
                skip_download_preview: true,
                auto_launch_dnd: false,
            },
            download: DownloadConfig {
                default_directory: PathBuf::from("C:\\Downloads"),
                max_concurrent: 5,
                retry_count: 3,
                retry_delay: 5,
                user_agent: "TestAgent".to_string(),
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
            ui: UiConfig::default(),
        };

        // Should serialize and deserialize correctly
        let serialized = toml::to_string_pretty(&app_config).unwrap();
        let deserialized: ApplicationConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.general.language, "en");
        assert_eq!(deserialized.download.max_concurrent, 5);
        assert_eq!(deserialized.download.max_redirects, 10);
    }

    #[test]
    fn test_keybindings_flatten_preserves_toml_layout() {
        use crate::app::keybindings::{KeyAction, KeyBindingSpec};

        // Grouping keybindings under UiConfig must not change the on-disk
        // layout: they stay flattened into the top-level [keybindings] table.
        let mut cfg = Config::default();
        cfg.ui
            .keybindings
            .bindings
            .insert(KeyAction::MoveUp, KeyBindingSpec::Single("x".into()));

        let app = ApplicationConfig {
            general: cfg.general.clone(),
            download: cfg.download.clone(),
            network: cfg.network.clone(),
            scripts: cfg.scripts.clone(),
            ui: cfg.ui.clone(),
        };

        let serialized = toml::to_string_pretty(&app).unwrap();
        assert!(serialized.contains("[keybindings]"), "keybindings table missing: {serialized}");
        assert!(!serialized.contains("[ui"), "ui section must not leak to disk: {serialized}");

        // An existing on-disk config (no [ui] wrapper) still deserializes.
        let back: ApplicationConfig = toml::from_str(&serialized).unwrap();
        assert_eq!(
            back.ui.keybindings.bindings.get(&KeyAction::MoveUp),
            Some(&KeyBindingSpec::Single("x".into()))
        );
    }

    #[test]
    fn test_folder_config_with_overrides() {
        let mut folder_config = FolderConfig::default();
        folder_config.max_concurrent = Some(5);
        folder_config.scripts_enabled = Some(false);
        folder_config.user_agent = Some("CustomAgent".to_string());

        let serialized = toml::to_string_pretty(&folder_config).unwrap();
        let deserialized: FolderConfig = toml::from_str(&serialized).unwrap();

        assert_eq!(deserialized.max_concurrent, Some(5));
        assert_eq!(deserialized.scripts_enabled, Some(false));
        assert_eq!(deserialized.user_agent, Some("CustomAgent".to_string()));
    }

    #[test]
    fn test_folder_config_inherits_when_none() {
        let folder_config = FolderConfig {
            name: "test".to_string(),
            save_path: PathBuf::from("C:\\Test"),
            auto_date_directory: true,
            auto_start_downloads: false,
            scripts_enabled: None, // Should inherit from app
            script_files: None,     // Should inherit from app
            max_concurrent: None,   // Should inherit from app
            user_agent: None,       // Should inherit from app
            referrer_policy: None,  // Should inherit from app
            default_headers: HashMap::new(),
            prevent_duplicate_url: false,
        };

        let serialized = toml::to_string_pretty(&folder_config).unwrap();
        let deserialized: FolderConfig = toml::from_str(&serialized).unwrap();

        // None values should remain None (inheritance handled at runtime)
        assert_eq!(deserialized.scripts_enabled, None);
        assert_eq!(deserialized.max_concurrent, None);
        assert_eq!(deserialized.user_agent, None);
    }

    #[test]
    fn test_default_folder_creation() {
        let config = Config::default();

        // Default config should have no folders initially
        assert_eq!(config.folders.len(), 0);
    }

    #[test]
    fn test_config_validation_integration() {
        use crate::app::settings::validate_folder_config;

        let mut config = Config::default();
        config.download.max_concurrent = 10;
        config.download.max_concurrent_per_folder = Some(5);
        config.download.parallel_folder_count = Some(2);

        // Valid configuration
        let result = validate_folder_config(&config);
        assert!(result.is_ok());

        // Invalid: per_folder * parallel > global
        config.download.max_concurrent_per_folder = Some(6);
        let result = validate_folder_config(&config);
        assert!(result.is_err());
    }

    #[test]
    #[serial]
    fn test_load_all_folder_configs_requires_settings_toml() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(&config_dir).unwrap();

        // Use config dir override instead of CWD change
        crate::util::paths::set_config_dir_override(Some(config_dir.clone()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        // Folders WITH settings.toml (should be recognized)
        std::fs::create_dir_all(config_dir.join("folder1")).unwrap();
        std::fs::write(
            config_dir.join("folder1").join("settings.toml"),
            r#"
save_path = "C:\\Folder1"
auto_date_directory = false
auto_start_downloads = false
"#,
        )
        .unwrap();

        std::fs::create_dir_all(config_dir.join("folder2")).unwrap();
        std::fs::write(
            config_dir.join("folder2").join("settings.toml"),
            r#"
save_path = "C:\\Folder2"
auto_date_directory = false
auto_start_downloads = false
"#,
        )
        .unwrap();

        // Directories WITHOUT settings.toml (should be ignored)
        std::fs::create_dir_all(config_dir.join(".logs")).unwrap();
        std::fs::create_dir_all(config_dir.join("scripts")).unwrap();
        std::fs::create_dir_all(config_dir.join("empty_dir")).unwrap();

        // Load folder configs
        let folders = Config::load_all_folder_configs().unwrap();

        // Clean up
        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        // Only directories with settings.toml should be loaded
        assert_eq!(folders.len(), 2);
        assert!(folders.contains_key("folder1"));
        assert!(folders.contains_key("folder2"));
        assert!(!folders.contains_key(".logs"));
        assert!(!folders.contains_key("scripts"));
        assert!(!folders.contains_key("empty_dir"));
    }

    #[test]
    #[serial]
    fn test_load_persists_default_folder_key_across_loads() {
        use tempfile::TempDir;

        let temp_dir = TempDir::new().unwrap();
        let config_dir = temp_dir.path().to_path_buf();
        std::fs::create_dir_all(&config_dir).unwrap();

        crate::util::paths::set_config_dir_override(Some(config_dir.clone()));
        unsafe { std::env::set_var("GGG_TEST_MODE", "1") };

        // First load: no folder exists yet, so `load()` invents a "default"
        // folder key. With the fix this gets saved immediately, not only
        // when `migrate_folder_keys` reports `migrated`.
        let first = Config::load().unwrap();
        let first_id = first
            .folders
            .iter()
            .find(|(_, f)| f.name == "default")
            .map(|(id, _)| id.clone())
            .expect("a default folder is created on first load");

        // Second load: the key must now be found on disk and reused, not
        // regenerated, or every process start would scatter tasks into a
        // fresh, settings-less folder directory.
        let second = Config::load().unwrap();
        let second_id = second
            .folders
            .iter()
            .find(|(_, f)| f.name == "default")
            .map(|(id, _)| id.clone())
            .expect("a default folder is present on second load");

        crate::util::paths::set_config_dir_override(None);
        unsafe { std::env::remove_var("GGG_TEST_MODE") };

        assert_eq!(first_id, second_id);
    }

    #[test]
    fn test_referrer_policy_serde_roundtrip() {
        #[derive(Serialize, Deserialize, Debug)]
        struct W {
            policy: ReferrerPolicy,
        }

        // All simple variants: deserialize from string
        for (input, expected) in [
            ("\"none\"", ReferrerPolicy::none()),
            ("\"same_as_url\"", ReferrerPolicy::same_as_url()),
            ("\"url_path\"", ReferrerPolicy::url_path()),
            ("\"url_origin\"", ReferrerPolicy::url_origin()),
        ] {
            let toml_str = format!("policy = {}\n", input);
            let w: W = toml::from_str(&toml_str).unwrap();
            assert_eq!(w.policy, expected, "Deserialize failed for input: {}", input);

            // Roundtrip: serialize and deserialize back
            let serialized = toml::to_string_pretty(&w).unwrap();
            let rt: W = toml::from_str(&serialized).unwrap();
            assert_eq!(rt.policy, expected, "Roundtrip failed for input: {}", input);
        }

        // Custom variant: deserialize from table
        let toml_str = "[policy]\ntype = \"custom\"\nvalue = \"https://example.com\"\n";
        let w: W = toml::from_str(toml_str).unwrap();
        assert_eq!(w.policy, ReferrerPolicy::custom("https://example.com"));

        // Custom roundtrip
        let serialized = toml::to_string_pretty(&w).unwrap();
        let rt: W = toml::from_str(&serialized).unwrap();
        assert_eq!(rt.policy, ReferrerPolicy::custom("https://example.com"));
    }

    #[test]
    fn test_referrer_policy_compute() {
        let url = "https://example.com/images/photo.jpg";

        // None → no referrer
        assert_eq!(ReferrerPolicy::none().compute(url), None);

        // SameAsUrl → full URL
        assert_eq!(
            ReferrerPolicy::same_as_url().compute(url),
            Some(url.to_string())
        );

        // UrlPath → parent path
        assert_eq!(
            ReferrerPolicy::url_path().compute(url),
            Some("https://example.com/images/".to_string())
        );

        // UrlOrigin → origin only
        assert_eq!(
            ReferrerPolicy::url_origin().compute(url),
            Some("https://example.com/".to_string())
        );

        // Custom → literal value
        assert_eq!(
            ReferrerPolicy::custom("https://custom.ref/page").compute(url),
            Some("https://custom.ref/page".to_string())
        );

        // Custom empty → None
        assert_eq!(ReferrerPolicy::custom("").compute(url), None);
    }

    #[test]
    fn test_referrer_policy_compute_edge_cases() {
        // URL with port
        assert_eq!(
            ReferrerPolicy::url_origin().compute("https://example.com:8443/path/file.txt"),
            Some("https://example.com:8443/".to_string())
        );

        // URL with no path segments
        assert_eq!(
            ReferrerPolicy::url_path().compute("https://example.com/file.txt"),
            Some("https://example.com/".to_string())
        );

        // Invalid URL
        assert_eq!(
            ReferrerPolicy::url_origin().compute("not-a-url"),
            None
        );
    }

    #[test]
    fn test_referrer_policy_cycle() {
        let p = ReferrerPolicy::none();
        let p = p.cycle_next();
        assert_eq!(p, ReferrerPolicy::same_as_url());
        let p = p.cycle_next();
        assert_eq!(p, ReferrerPolicy::url_path());
        let p = p.cycle_next();
        assert_eq!(p, ReferrerPolicy::url_origin());
        let p = p.cycle_next();
        assert_eq!(p, ReferrerPolicy::none());
    }

    #[test]
    fn test_referrer_policy_backward_compat() {
        // Existing config without referrer_policy should deserialize fine
        let toml_str = r#"
default_directory = "C:\\Downloads"
max_concurrent = 3
retry_count = 3
retry_delay = 5
user_agent = "Test/1.0"
bandwidth_limit = 0
max_redirects = 5
"#;
        let config: DownloadConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.referrer_policy, ReferrerPolicy::default());
    }

    #[test]
    fn claim_wait_ms_defaults_to_2000() {
        let toml_str = r#"
default_directory = "C:\\Downloads"
max_concurrent = 3
retry_count = 3
retry_delay = 5
user_agent = "Test/1.0"
bandwidth_limit = 0
max_redirects = 5
"#;
        let config: DownloadConfig = toml::from_str(toml_str).unwrap();
        assert_eq!(config.claim_wait_ms, 2000);

        let toml_with_value = format!("{toml_str}\nclaim_wait_ms = 500\n");
        let config: DownloadConfig = toml::from_str(&toml_with_value).unwrap();
        assert_eq!(config.claim_wait_ms, 500);
    }
}
