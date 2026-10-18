//! Editing state of the Settings window.
//!
//! The window edits `draft`; `original` is what the running core has applied.
//! Apply sends only the difference as per-field commands, so a failed run keeps
//! the draft and the next run resends just what is still different.

use ggg::{
    app::config::Config,
    ui::{commands::Command, settings_diff::settings_commands, settings_fields::ApplicationSettingsField},
};

use crate::gui_config::GuiConfig;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SettingsPage {
    #[default]
    General,
    Downloads,
    Connection,
    Scripts,
    Folders,
}

impl SettingsPage {
    pub fn all() -> [SettingsPage; 5] {
        [Self::General, Self::Downloads, Self::Connection, Self::Scripts, Self::Folders]
    }
}

pub struct SettingsModel {
    /// Config the core runs with, as last read back.
    pub original: Config,
    pub draft: Config,
    pub gui_original: GuiConfig,
    pub gui_draft: GuiConfig,
    pub page: SettingsPage,
    pub selected_folder: Option<String>,
    /// Index (into the applied command list) of the failing command and its
    /// localized message.
    pub error: Option<(usize, String)>,
    /// The command behind `error`, so the view can mark the failing field.
    pub failed_command: Option<Command>,
    /// Set once an applied change touched a field that only takes effect
    /// after a restart; stays set while the window is open.
    pub restart_needed: bool,
    /// Commands handed out by `begin_apply`, kept to resolve `error`'s index.
    in_flight: Vec<Command>,
}

impl SettingsModel {
    pub fn new(config: &Config, gui: &GuiConfig) -> Self {
        let mut model = Self {
            original: config.clone(),
            draft: config.clone(),
            gui_original: gui.clone(),
            gui_draft: gui.clone(),
            page: SettingsPage::General,
            selected_folder: None,
            error: None,
            failed_command: None,
            restart_needed: false,
            in_flight: Vec::new(),
        };
        model.selected_folder = model.fallback_folder();
        model
    }

    /// Opens on the Folders page with `folder_id` selected, if it exists.
    pub fn open_folder(&mut self, folder_id: &str) {
        self.page = SettingsPage::Folders;
        if self.draft.folders.contains_key(folder_id) {
            self.selected_folder = Some(folder_id.to_string());
        }
    }

    /// Whether Apply has anything to do. Only edited fields count: default
    /// headers and folders added or removed through the draft are not part of
    /// the comparison.
    pub fn is_dirty(&self) -> bool {
        !self.commands().is_empty() || self.gui_changed()
    }

    pub fn gui_changed(&self) -> bool {
        self.gui_draft.expand_ranges_on_direct_add != self.gui_original.expand_ranges_on_direct_add
    }

    /// Records the GUI-only setting as saved (the caller writes `gui.toml`).
    pub fn mark_gui_applied(&mut self) {
        self.gui_original.expand_ranges_on_direct_add = self.gui_draft.expand_ranges_on_direct_add;
    }

    pub fn commands(&self) -> Vec<Command> {
        settings_commands(&self.original, &self.draft)
    }

    /// Commands to send for Apply. The result of running them goes to
    /// `on_apply_result`.
    pub fn begin_apply(&mut self) -> Vec<Command> {
        self.error = None;
        self.failed_command = None;
        self.in_flight = self.commands();
        self.in_flight.clone()
    }

    /// Fields that differ between `original` and `draft` and need a restart.
    // The pages mark every restart field instead; kept for the model tests.
    #[allow(dead_code)]
    pub fn changed_restart_fields(&self) -> Vec<ApplicationSettingsField> {
        restart_fields_differing(&self.original, &self.draft)
    }

    /// Takes the config read back after a run of commands. `draft` is kept so
    /// only the remaining difference is sent next time. Returns true when
    /// every command succeeded.
    pub fn on_apply_result(&mut self, error: Option<(usize, String)>, config: &Config) -> bool {
        if !restart_fields_differing(&self.original, config).is_empty() {
            self.restart_needed = true;
        }
        self.failed_command = error.as_ref().and_then(|(index, _)| self.in_flight.get(*index).cloned());
        self.in_flight.clear();
        self.original = config.clone();
        self.sync_folders(config);
        let ok = error.is_none();
        self.error = error;
        ok
    }

    /// After `CreateFolder`: the new folder appears in both configs and is
    /// selected; edits to other folders stay.
    pub fn on_folder_created(&mut self, config: &Config) {
        let new_id = config
            .sorted_folder_entries()
            .into_iter()
            .map(|(id, _)| id)
            .find(|id| !self.original.folders.contains_key(id));
        self.original = config.clone();
        self.sync_folders(config);
        if new_id.is_some() {
            self.selected_folder = new_id;
        }
    }

    /// After `DeleteFolder`: the folder disappears from both configs; edits to
    /// other folders stay.
    pub fn on_folder_deleted(&mut self, config: &Config) {
        self.original = config.clone();
        self.sync_folders(config);
    }

    /// After `ReloadConfig`: the disk state replaces every edit.
    pub fn on_config_reloaded(&mut self, config: &Config) {
        if !restart_fields_differing(&self.original, config).is_empty() {
            self.restart_needed = true;
        }
        self.original = config.clone();
        self.draft = config.clone();
        self.error = None;
        self.failed_command = None;
        self.in_flight.clear();
        self.ensure_selection();
    }

    /// Makes the draft's folder set match `config` without touching edits to
    /// folders that exist in both.
    fn sync_folders(&mut self, config: &Config) {
        self.draft.folders.retain(|id, _| config.folders.contains_key(id));
        for (id, folder) in &config.folders {
            self.draft.folders.entry(id.clone()).or_insert_with(|| folder.clone());
        }
        self.ensure_selection();
    }

    fn ensure_selection(&mut self) {
        let valid = self.selected_folder.as_ref().is_some_and(|id| self.draft.folders.contains_key(id));
        if !valid {
            self.selected_folder = self.fallback_folder();
        }
    }

    /// The default folder when there is one, else the first by display name.
    fn fallback_folder(&self) -> Option<String> {
        self.draft
            .folders
            .keys()
            .find(|id| self.draft.is_default_folder(id))
            .cloned()
            .or_else(|| self.draft.sorted_folder_entries().into_iter().next().map(|(id, _)| id))
    }
}

fn restart_fields_differing(a: &Config, b: &Config) -> Vec<ApplicationSettingsField> {
    ApplicationSettingsField::all()
        .into_iter()
        .filter(|field| field.requires_restart() && field_differs(*field, a, b))
        .collect()
}

fn field_differs(field: ApplicationSettingsField, a: &Config, b: &Config) -> bool {
    use ApplicationSettingsField as F;
    match field {
        F::Language => a.general.language != b.general.language,
        F::SkipDownloadPreview => a.general.skip_download_preview != b.general.skip_download_preview,
        F::AutoLaunchDnd => a.general.auto_launch_dnd != b.general.auto_launch_dnd,
        F::UserAgent => a.download.user_agent != b.download.user_agent,
        F::ReferrerPolicy => a.download.referrer_policy != b.download.referrer_policy,
        F::MaxRedirects => a.download.max_redirects != b.download.max_redirects,
        F::RetryCount => a.download.retry_count != b.download.retry_count,
        F::MaxConcurrent => a.download.max_concurrent != b.download.max_concurrent,
        F::MaxConcurrentPerFolder => a.download.max_concurrent_per_folder != b.download.max_concurrent_per_folder,
        F::MaxActiveFolders => a.download.parallel_folder_count != b.download.parallel_folder_count,
        F::ScriptsEnabled => a.scripts.enabled != b.scripts.enabled,
        F::DefaultDirectory => a.download.default_directory != b.download.default_directory,
    }
}

// The pages use drag values, so these text parsers are only exercised by tests.
#[allow(dead_code)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberError {
    Empty,
    /// Not a non-negative integer, or too large for the field.
    Invalid,
    /// A limit of 0 would stall the queue; the commands' validation expects
    /// a positive value (`validate_folder_config`).
    Zero,
}

/// A count that must be at least 1: concurrency limits.
#[allow(dead_code)]
pub fn parse_positive(text: &str) -> Result<usize, NumberError> {
    match parse_count::<usize>(text)? {
        0 => Err(NumberError::Zero),
        n => Ok(n),
    }
}

/// An optional limit: empty text means "no limit" (`None`).
#[allow(dead_code)]
pub fn parse_optional_positive(text: &str) -> Result<Option<usize>, NumberError> {
    match parse_positive(text) {
        Ok(n) => Ok(Some(n)),
        Err(NumberError::Empty) => Ok(None),
        Err(e) => Err(e),
    }
}

/// A count where 0 is meaningful: retries and redirects.
#[allow(dead_code)]
pub fn parse_zero_ok(text: &str) -> Result<u32, NumberError> {
    parse_count(text)
}

#[allow(dead_code)]
fn parse_count<T: std::str::FromStr>(text: &str) -> Result<T, NumberError> {
    let text = text.trim();
    if text.is_empty() {
        return Err(NumberError::Empty);
    }
    // Digits only: `+5` parses as an integer but is not what the user is
    // expected to type.
    if !text.bytes().all(|b| b.is_ascii_digit()) {
        return Err(NumberError::Invalid);
    }
    text.parse().map_err(|_| NumberError::Invalid)
}

#[cfg(test)]
mod settings_model {
    use super::*;
    use ggg::app::config::FolderConfig;

    fn folder(name: &str) -> FolderConfig {
        FolderConfig { name: name.to_string(), ..FolderConfig::default() }
    }

    fn config_with(ids: &[&str]) -> Config {
        let mut config = Config::default();
        config.folders.clear();
        for id in ids {
            config.folders.insert(id.to_string(), folder(id));
        }
        config
    }

    fn model(config: &Config) -> SettingsModel {
        SettingsModel::new(config, &GuiConfig::default())
    }

    #[test]
    fn fresh_model_is_clean() {
        let m = model(&config_with(&["a", "b"]));
        assert!(!m.is_dirty());
        assert!(m.commands().is_empty());
        assert!(m.changed_restart_fields().is_empty());
        assert!(m.error.is_none());
        assert!(!m.restart_needed);
    }

    #[test]
    fn edit_then_revert_is_clean() {
        let mut m = model(&config_with(&["a"]));
        let before = m.draft.download.max_concurrent;
        m.draft.download.max_concurrent += 1;
        m.draft.folders.get_mut("a").unwrap().name = "renamed".to_string();
        m.gui_draft.expand_ranges_on_direct_add = !m.gui_original.expand_ranges_on_direct_add;
        assert!(m.is_dirty());

        m.draft.download.max_concurrent = before;
        m.draft.folders.get_mut("a").unwrap().name = "a".to_string();
        assert!(m.is_dirty(), "the GUI-only edit still counts");
        m.gui_draft.expand_ranges_on_direct_add = m.gui_original.expand_ranges_on_direct_add;
        assert!(!m.is_dirty());
    }

    #[test]
    fn restart_field_change_is_reported() {
        let mut m = model(&config_with(&["a"]));
        m.draft.download.max_concurrent += 1;
        assert!(m.changed_restart_fields().is_empty(), "applies live");

        m.draft.general.language = "ja".to_string();
        m.draft.download.max_concurrent_per_folder = Some(1);
        assert_eq!(
            m.changed_restart_fields(),
            vec![ApplicationSettingsField::Language, ApplicationSettingsField::MaxConcurrentPerFolder]
        );
    }

    #[test]
    fn apply_error_rebases_original_and_keeps_draft() {
        let mut m = model(&config_with(&["a"]));
        let base = m.original.download.max_concurrent;
        m.draft.download.max_concurrent = base + 1;
        m.draft.download.user_agent = "agent".to_string();

        let sent = m.begin_apply();
        assert_eq!(sent.len(), 2);

        // The core applied the first command and rejected the second.
        let mut read_back = m.original.clone();
        read_back.download.max_concurrent = base + 1;
        let ok = m.on_apply_result(Some((1, "rejected".to_string())), &read_back);

        assert!(!ok);
        assert_eq!(m.original.download.max_concurrent, base + 1);
        assert_eq!(m.draft.download.user_agent, "agent");
        assert_eq!(m.error, Some((1, "rejected".to_string())));
        assert_eq!(m.failed_command, Some(sent[1].clone()));
        // Only the remaining difference is sent next time.
        assert_eq!(m.commands(), vec![sent[1].clone()]);
        assert!(m.is_dirty());
        assert!(!m.restart_needed);
    }

    #[test]
    fn successful_apply_of_restart_field_sets_banner_flag() {
        let mut m = model(&config_with(&["a"]));
        m.draft.general.language = "ja".to_string();
        m.begin_apply();
        let mut read_back = m.original.clone();
        read_back.general.language = "ja".to_string();

        assert!(m.on_apply_result(None, &read_back));
        assert!(m.restart_needed);
        assert!(m.error.is_none());
        assert!(!m.is_dirty());
    }

    #[test]
    fn folder_delete_keeps_other_folder_edits() {
        let mut m = model(&config_with(&["a", "b", "c"]));
        m.draft.folders.get_mut("a").unwrap().name = "a2".to_string();
        m.draft.folders.get_mut("b").unwrap().auto_start_downloads = true;
        m.draft.folders.get_mut("c").unwrap().name = "c2".to_string();
        m.selected_folder = Some("c".to_string());

        let mut live = m.original.clone();
        live.folders.remove("c");
        m.on_folder_deleted(&live);

        assert!(!m.draft.folders.contains_key("c"));
        assert!(!m.original.folders.contains_key("c"));
        assert_eq!(m.draft.folders["a"].name, "a2");
        assert!(m.draft.folders["b"].auto_start_downloads);
        assert_eq!(m.commands().len(), 2);
        assert!(m.selected_folder.as_deref().is_some_and(|id| id != "c"));
    }

    #[test]
    fn folder_create_selects_new_folder_and_keeps_edits() {
        let mut m = model(&config_with(&["a"]));
        m.draft.folders.get_mut("a").unwrap().name = "a2".to_string();

        let mut live = m.original.clone();
        live.folders.insert("n".to_string(), folder("folder_1"));
        m.on_folder_created(&live);

        assert_eq!(m.selected_folder.as_deref(), Some("n"));
        assert_eq!(m.draft.folders["n"].name, "folder_1");
        assert_eq!(m.commands().len(), 1);
    }

    #[test]
    fn reload_resets_draft_and_flags_restart() {
        let mut m = model(&config_with(&["a"]));
        m.draft.download.max_concurrent += 5;
        let mut disk = m.original.clone();
        disk.scripts.enabled = !disk.scripts.enabled;

        m.on_config_reloaded(&disk);
        assert!(!m.is_dirty());
        assert!(m.restart_needed);
    }

    #[test]
    fn open_folder_selects_folders_page() {
        let mut m = model(&config_with(&["a", "b"]));
        m.open_folder("b");
        assert_eq!(m.page, SettingsPage::Folders);
        assert_eq!(m.selected_folder.as_deref(), Some("b"));
        m.open_folder("missing");
        assert_eq!(m.selected_folder.as_deref(), Some("b"));
    }

    #[test]
    fn numbers_reject_zero_where_a_limit_must_be_positive() {
        assert_eq!(parse_positive(" 4 "), Ok(4));
        assert_eq!(parse_positive("0"), Err(NumberError::Zero));
        assert_eq!(parse_positive(""), Err(NumberError::Empty));
        assert_eq!(parse_positive("-1"), Err(NumberError::Invalid));
        assert_eq!(parse_positive("+1"), Err(NumberError::Invalid));
        assert_eq!(parse_positive("1.5"), Err(NumberError::Invalid));
        assert_eq!(parse_optional_positive(""), Ok(None));
        assert_eq!(parse_optional_positive("3"), Ok(Some(3)));
        assert_eq!(parse_optional_positive("0"), Err(NumberError::Zero));
        assert_eq!(parse_zero_ok("0"), Ok(0));
        assert_eq!(parse_zero_ok("99999999999"), Err(NumberError::Invalid));
    }
}
