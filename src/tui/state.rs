use crate::app::state::AppState;
use crate::download::manager::DownloadManager;
use crate::download::task::DownloadTask;
use crate::util::i18n::LocalizationManager;
use ratatui::layout::Rect;
use ratatui::widgets::TableState;
use std::cell::RefCell;
use std::sync::Arc;
use uuid::Uuid;

/// UI mode determines what the TUI is currently doing
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiMode {
    /// Normal navigation and commands
    Normal,
    /// Adding a new download (input dialog)
    AddDownload,
    /// Generic input field editing
    EditingField,
    /// Preview download before adding
    DownloadPreview,
    /// Searching/filtering downloads
    Search,
    /// Changing application folder for selected download (folder picker)
    ChangeFolder,
    /// Changing save path for selected download (free text input)
    ChangeSavePath,
    /// Switching current folder for new downloads
    SwitchFolder,
    /// Help screen overlay
    Help,
    /// Settings screen
    Settings,
    /// Editing folder settings
    FolderEdit,
    /// Confirm delete dialog
    ConfirmDelete,
    /// Context menu (popup actions)
    ContextMenu,
    /// Folder context menu (popup actions for folder tree)
    FolderContextMenu,
}

impl UiMode {
    /// Returns true if this mode accepts free-form text input.
    /// Mouse capture is disabled in these modes so that terminal-native
    /// paste (e.g. right-click paste in Windows Terminal) works.
    pub fn is_text_input(&self) -> bool {
        matches!(
            self,
            UiMode::AddDownload | UiMode::EditingField | UiMode::Search | UiMode::FolderEdit | UiMode::ChangeSavePath
        )
    }
}

/// Active pane in the 3-pane layout
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FocusPane {
    /// Folder tree on the left
    #[default]
    FolderTree,
    /// Download list in the center
    DownloadList,
    /// Details panel at the bottom/right
    DetailsPanel,
}

/// Item type in the folder tree
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FolderTreeItem {
    /// Regular folder (folder_id)
    Folder(String),
    /// Special "Completed" node showing history
    CompletedNode,
}

/// Position of the details panel
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DetailsPosition {
    /// Below the download list
    #[default]
    Bottom,
    /// Right of the download list (current behavior)
    Right,
    /// Hidden (no details panel)
    Hidden,
}

/// What the confirm-delete dialog will act on.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub enum DeleteTarget {
    /// The selected download(s) in Normal mode (today's behavior).
    #[default]
    Downloads,
    /// A folder in Settings, identified by its UUID key.
    Folder(String),
}

/// Settings screen sections
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SettingsSection {
    General,
    Downloads,
    Scripts,
    Folders,
}

impl SettingsSection {
    pub const ALL: [SettingsSection; 4] = [
        Self::General,
        Self::Downloads,
        Self::Scripts,
        Self::Folders,
    ];

    /// Next tab, wrapping from the last back to the first.
    pub fn next(self) -> Self {
        let idx = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(idx + 1) % Self::ALL.len()]
    }

    /// Previous tab, wrapping from the first back to the last.
    pub fn prev(self) -> Self {
        let idx = Self::ALL.iter().position(|s| *s == self).unwrap_or(0);
        Self::ALL[(idx + Self::ALL.len() - 1) % Self::ALL.len()]
    }

    /// Translation key for the tab label
    pub fn label_key(self) -> &'static str {
        match self {
            Self::General => "settings-tab-general",
            Self::Downloads => "settings-tab-downloads",
            Self::Scripts => "settings-tab-scripts",
            Self::Folders => "settings-tab-folders",
        }
    }
}

/// Application-level settings fields.
///
/// Order is display order within each `SettingsSection`, grouped by section
/// (General, then Downloads, then Scripts) so `in_section` can filter `all()`
/// without re-sorting.
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
        ]
    }

    /// The tab this field is shown on.
    pub fn section(&self) -> SettingsSection {
        match self {
            Self::Language | Self::SkipDownloadPreview | Self::AutoLaunchDnd => {
                SettingsSection::General
            }
            Self::UserAgent
            | Self::ReferrerPolicy
            | Self::MaxRedirects
            | Self::RetryCount
            | Self::MaxConcurrent
            | Self::MaxConcurrentPerFolder
            | Self::MaxActiveFolders => SettingsSection::Downloads,
            Self::ScriptsEnabled => SettingsSection::Scripts,
        }
    }

    /// Fields shown on `section`, in display order.
    pub fn in_section(section: SettingsSection) -> Vec<Self> {
        Self::all().into_iter().filter(|f| f.section() == section).collect()
    }

    /// Number of selectable rows on `section`: its fixed fields plus, on the
    /// Scripts tab, one row per script file (the script list is flattened
    /// into the field list rather than shown as a collapsible sub-list).
    pub fn row_count(section: SettingsSection, script_file_count: usize) -> usize {
        let field_count = Self::in_section(section).len();
        if section == SettingsSection::Scripts {
            field_count + script_file_count
        } else {
            field_count
        }
    }

    /// Whether changing this field only takes effect after a restart.
    /// `MaxConcurrent` applies live via `DownloadManager::set_max_concurrent`,
    /// so it is not in this list; the others are read only at startup (see
    /// `main.rs`, where the manager and its per-folder queues are built once).
    /// `ScriptsEnabled` is included because the script executor thread is
    /// only spawned when scripts are enabled at startup (`AppState::new_with_scripts`);
    /// turning it on later leaves `script_sender` `None` for the rest of the
    /// session, so scripts silently never run until a restart.
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

    /// Number of selectable rows in the folder editor: the fixed fields plus
    /// one row per script file (folder override), flattened into the same
    /// list rather than shown as a collapsible sub-list.
    pub fn row_count(script_file_count: usize) -> usize {
        Self::all().len() + script_file_count
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

/// Context menu actions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ContextMenuAction {
    StartPause,
    Retry,
    Delete,
    ChangeFolder,
    ChangeSavePath,
    CopyUrl,
    OpenFolder,
    Cancel,
}

impl ContextMenuAction {
    /// Get all available menu items
    pub fn all() -> Vec<Self> {
        vec![
            Self::StartPause,
            Self::Retry,
            Self::Delete,
            Self::ChangeFolder,
            Self::ChangeSavePath,
            Self::CopyUrl,
            Self::OpenFolder,
            Self::Cancel,
        ]
    }

    /// Get translation key for label
    pub fn label_key(&self) -> &str {
        match self {
            Self::StartPause => "context-menu-start-pause",
            Self::Retry => "context-menu-retry",
            Self::Delete => "context-menu-delete",
            Self::ChangeFolder => "context-menu-change-folder",
            Self::ChangeSavePath => "context-menu-change-save-path",
            Self::CopyUrl => "context-menu-copy-url",
            Self::OpenFolder => "context-menu-open-folder",
            Self::Cancel => "context-menu-cancel",
        }
    }

    /// Keybinding hint shown next to the label. Resolver-backed items show the
    /// first configured key for their `KeyAction` so a remap is reflected here
    /// too; the local accelerators (`f`, `c`, `o`) have no global action and
    /// stay literal.
    pub fn key_hint(&self, resolver: &crate::app::keybindings::KeybindingResolver) -> String {
        use crate::app::keybindings::KeyAction;

        match self {
            Self::StartPause => first_key_or(resolver, KeyAction::ToggleDownload, "s"),
            Self::Retry => first_key_or(resolver, KeyAction::RetryDownload, "r"),
            Self::Delete => first_key_or(resolver, KeyAction::DeleteDownload, "d"),
            Self::ChangeFolder => "f".to_string(),
            Self::ChangeSavePath => first_key_or(resolver, KeyAction::EditItem, "e"),
            Self::CopyUrl => "c".to_string(),
            Self::OpenFolder => "o".to_string(),
            Self::Cancel => first_key_or(resolver, KeyAction::Back, "Esc"),
        }
    }
}

/// The first configured key string for `action`, or `fallback` if unbound.
fn first_key_or(
    resolver: &crate::app::keybindings::KeybindingResolver,
    action: crate::app::keybindings::KeyAction,
    fallback: &str,
) -> String {
    resolver
        .keys_for(action)
        .first()
        .cloned()
        .unwrap_or_else(|| fallback.to_string())
}


/// Folder-specific context menu actions
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FolderContextMenuAction {
    StartAll,
    StopAll,
    DeleteAll,
    ClearHistory, // Only for Completed node
    Cancel,
}

impl FolderContextMenuAction {
    /// Get all menu items for regular folders
    pub fn all_for_folder() -> Vec<Self> {
        vec![Self::StartAll, Self::StopAll, Self::DeleteAll, Self::Cancel]
    }

    /// Get all menu items for Completed node
    pub fn all_for_completed() -> Vec<Self> {
        vec![Self::ClearHistory, Self::Cancel]
    }

    /// Get translation key for label
    pub fn label_key(&self) -> &str {
        match self {
            Self::StartAll => "folder-menu-start-all",
            Self::StopAll => "folder-menu-stop-all",
            Self::DeleteAll => "folder-menu-delete-all",
            Self::ClearHistory => "folder-menu-clear-history",
            Self::Cancel => "context-menu-cancel",
        }
    }

    /// Keybinding hint shown next to the label. See `ContextMenuAction::key_hint`.
    pub fn key_hint(&self, resolver: &crate::app::keybindings::KeybindingResolver) -> String {
        use crate::app::keybindings::KeyAction;

        match self {
            Self::StartAll => first_key_or(resolver, KeyAction::ToggleDownload, "s"),
            Self::StopAll => "p".to_string(),
            Self::DeleteAll => first_key_or(resolver, KeyAction::DeleteDownload, "d"),
            Self::ClearHistory => "c".to_string(),
            Self::Cancel => first_key_or(resolver, KeyAction::Back, "Esc"),
        }
    }
}

/// Clickable regions for hit detection (updated each render)
#[derive(Debug, Clone, Default)]
pub struct ClickableRegions {
    pub folder_tree: Option<Rect>,
    pub download_list: Option<Rect>,
    pub details_panel: Option<Rect>,
    pub folder_items: Vec<(usize, Rect)>, // (index, rect) pairs for folder items
    pub download_rows: Vec<(usize, Rect)>, // (index, rect) pairs for download rows
    pub context_menu: Option<Rect>,
    pub context_menu_items: Vec<Rect>,
    pub dialog_buttons: Vec<(String, Rect)>,
    // Folder picker dialog regions (Change folder dialog)
    pub folder_picker_items: Vec<(usize, Rect)>, // (index, rect) pairs for folder rows
    // Settings screen regions
    pub settings_tabs: Vec<(usize, Rect)>, // (tab_index, rect) pairs for settings tabs
    pub settings_folder_items: Vec<(usize, Rect)>, // (index, rect) pairs for folder list in settings
}

/// TUI application state
pub struct TuiState {
    /// Reference to app state (config, etc.)
    pub app_state: AppState,

    /// Internationalization manager
    pub i18n: Arc<LocalizationManager>,

    /// Per-folder download tasks (folder_id -> tasks)
    pub folder_downloads: std::collections::HashMap<String, Vec<DownloadTask>>,

    /// Folder display names cache (folder_id UUID -> display name)
    /// Updated every tick from config
    pub folder_names: std::collections::HashMap<String, String>,

    /// Download history items (completed, failed, deleted)
    pub history_items: Vec<DownloadTask>,

    /// Selected index in the download list
    pub selected_index: usize,

    /// Currently focused pane in 3-pane layout
    pub focus_pane: FocusPane,

    /// Items in the folder tree
    pub tree_items: Vec<FolderTreeItem>,

    /// Selected index in the folder tree
    pub tree_selected_index: usize,

    /// Details panel position (Bottom/Right/Hidden)
    pub details_position: DetailsPosition,

    /// Search query (only used for history/completed node)
    pub search_query: String,

    /// Current UI mode
    pub ui_mode: UiMode,

    /// Show details panel
    pub show_details: bool,

    /// Input buffer for dialogs
    pub input_buffer: String,

    /// Input dialog title (for EditingField mode)
    pub input_title: String,

    /// Input dialog prompt/label (for EditingField mode)
    pub input_prompt: String,

    /// Current folder ID for new downloads
    pub current_folder_id: String,

    /// Folder picker: selected folder index
    pub folder_picker_index: usize,

    /// Settings screen: selected folder ID
    pub selected_folder_id: Option<String>,

    /// What the confirm-delete dialog (`UiMode::ConfirmDelete`) will act on.
    pub delete_target: DeleteTarget,

    /// Settings screen: currently editing field
    pub settings_edit_field: Option<SettingsField>,

    /// Settings screen: folder list selection index
    pub settings_folder_index: usize,

    /// Settings screen: field selection index (for editing)
    pub settings_field_index: usize,

    /// Settings screen: active tab
    pub settings_section: SettingsSection,

    /// Settings screen: application settings field index
    pub app_settings_field_index: usize,

    /// Settings screen: currently editing application setting
    pub is_editing_app_setting: bool,

    /// Validation/error message to display (None = no error)
    pub validation_error: Option<String>,

    /// Transient status message shown in the Normal-mode status bar (e.g. a
    /// busy-count notice after a delete/pause hits a `TaskBusy` claim).
    /// `None` = nothing to show.
    pub status_message: Option<String>,

    /// Rendering optimization: flag to indicate if UI needs redraw
    pub needs_redraw: bool,

    /// Multi-selection: set of selected download IDs
    pub selected_downloads: std::collections::HashSet<uuid::Uuid>,

    /// Context menu: selected menu item index
    pub context_menu_index: usize,

    /// Undo stack of deleted downloads. Each entry is one delete *operation*
    /// (a batch), so a multi-selection delete is undone as a single unit.
    pub delete_history: Vec<Vec<DownloadTask>>,

    /// Download preview: information fetched from server
    pub preview_info: Option<crate::download::http_client::DownloadInfo>,

    /// Table state for ratatui widget (RefCell for interior mutability)
    table_state: RefCell<TableState>,

    /// Clickable regions for mouse hit detection (updated each render)
    pub click_regions: RefCell<ClickableRegions>,

    /// Folder context menu: selected menu item index
    pub folder_context_menu_index: usize,

    /// Cached sorted list of `.js` script filenames, refreshed on tick while the
    /// settings screen is open. Avoids a blocking `read_dir` on every frame and
    /// every settings keypress.
    pub cached_script_files: Vec<String>,

    /// Cached `(folder_id, display_name)` entries, refreshed every tick from
    /// config. Folder-picker dialogs render from this owned snapshot so they
    /// never vanish on a transient `config.try_read()` failure.
    pub cached_folder_entries: Vec<(String, String)>,

    /// Keyboard shortcut resolver
    pub keybinding_resolver: crate::app::keybindings::KeybindingResolver,

    /// IPC Named Pipe name (Windows only, set when pipe server starts)
    #[cfg(windows)]
    pub ipc_pipe_name: Option<String>,
}

/// Loads `language`, falling back to English and then to a placeholder
/// manager, so a bad locale never keeps the TUI from starting.
fn load_translations(language: &str) -> Arc<LocalizationManager> {
    Arc::new(LocalizationManager::new(language).unwrap_or_else(|e| {
        tracing::error!("Failed to load translations for '{}': {}", language, e);
        tracing::info!("Falling back to English");
        LocalizationManager::new("en").unwrap_or_else(|e2| {
            tracing::error!("Failed to load fallback English locale: {}", e2);
            LocalizationManager::fallback()
        })
    }))
}

/// Row count moved per PageUp/PageDown, kept equal to the download list's page size.
/// Shared by the folder tree and every other row-indexed list (settings rows,
/// the Folders list, the folder editor fields) so paging feels consistent.
const TREE_PAGE_SIZE: usize = 10;

/// Index after jumping to the top of a `count`-row list.
fn tree_top_index(_count: usize) -> usize {
    0
}

/// Index after jumping to the bottom of a `count`-row list.
fn tree_bottom_index(count: usize) -> usize {
    count.saturating_sub(1)
}

/// Index after paging up by `page` rows from `current`.
fn tree_page_up_index(current: usize, page: usize) -> usize {
    current.saturating_sub(page)
}

/// Index after paging down by `page` rows from `current`, clamped to `count` rows.
fn tree_page_down_index(current: usize, count: usize, page: usize) -> usize {
    if count == 0 {
        0
    } else {
        (current + page).min(count - 1)
    }
}

impl TuiState {
    pub fn new(
        app_state: AppState,
        keybindings: &crate::app::keybindings::KeybindingsConfig,
        language: &str,
    ) -> Self {
        let mut table_state = TableState::default();
        table_state.select(Some(0));

        let i18n = load_translations(language);

        // Create keybinding resolver from config
        let keybinding_resolver =
            crate::app::keybindings::KeybindingResolver::from_config(keybindings);

        Self {
            app_state,
            i18n,
            folder_downloads: std::collections::HashMap::new(),
            folder_names: std::collections::HashMap::new(),
            history_items: Vec::new(),
            selected_index: 0,
            focus_pane: FocusPane::DownloadList,
            tree_items: vec![FolderTreeItem::Folder("default".to_string()), FolderTreeItem::CompletedNode],
            tree_selected_index: 0,
            details_position: DetailsPosition::Bottom,
            search_query: String::new(),
            ui_mode: UiMode::Normal,
            show_details: true,
            input_buffer: String::new(),
            input_title: String::new(),
            input_prompt: String::new(),
            current_folder_id: "default".to_string(),
            folder_picker_index: 0,
            selected_folder_id: None,
            delete_target: DeleteTarget::default(),
            settings_edit_field: None,
            settings_folder_index: 0,
            settings_field_index: 0,
            settings_section: SettingsSection::General,
            app_settings_field_index: 0,
            is_editing_app_setting: false,
            validation_error: None,
            status_message: None,
            needs_redraw: true,  // Initial render needed
            selected_downloads: std::collections::HashSet::new(),
            context_menu_index: 0,
            delete_history: Vec::new(),
            preview_info: None,
            table_state: RefCell::new(table_state),
            click_regions: RefCell::new(ClickableRegions::default()),
            folder_context_menu_index: 0,
            cached_script_files: Vec::new(),
            cached_folder_entries: Vec::new(),
            keybinding_resolver,
            #[cfg(windows)]
            ipc_pipe_name: None,
        }
    }

    /// Update downloads from manager
    pub async fn update_downloads(&mut self, manager: &DownloadManager) {
        // Get all downloads and group by folder_id
        let all_downloads = manager.get_all_downloads().await;
        self.folder_downloads.clear();
        for task in all_downloads {
            self.folder_downloads
                .entry(task.folder_id.clone())
                .or_default()
                .push(task);
        }
        
        self.history_items = manager.get_history().await;

        // Also update tree items and folder name cache based on current config
        let config = self.app_state.config.read().await;
        // Update folder names cache
        self.folder_names.clear();
        for (id, fc) in &config.folders {
            let name = if fc.name.is_empty() { id.clone() } else { fc.name.clone() };
            self.folder_names.insert(id.clone(), name);
        }
        let entries = config.sorted_folder_entries();
        drop(config);

        // Snapshot for folder-picker dialogs (rendered without taking the lock).
        self.cached_folder_entries = entries.clone();

        self.tree_items = entries
            .into_iter()
            .map(|(id, _name)| FolderTreeItem::Folder(id))
            .chain(std::iter::once(FolderTreeItem::CompletedNode))
            .collect();
    }

    /// Get the currently selected tree item
    pub fn selected_tree_item(&self) -> Option<&FolderTreeItem> {
        self.tree_items.get(self.tree_selected_index)
    }

    /// Check if currently viewing the Completed node
    pub fn is_viewing_completed_node(&self) -> bool {
        matches!(self.selected_tree_item(), Some(FolderTreeItem::CompletedNode))
    }

    /// Get the selected folder ID (None if viewing Completed node)
    pub fn selected_folder_id_from_tree(&self) -> Option<&str> {
        match self.selected_tree_item() {
            Some(FolderTreeItem::Folder(id)) => Some(id.as_str()),
            _ => None,
        }
    }

    /// Get downloads for the currently selected folder/node
    /// 
    /// - For folder nodes: returns tasks from that folder directly (no filtering)
    /// - For completed node: returns history items with optional search filter
    pub fn current_downloads(&self) -> Vec<&DownloadTask> {
        if self.is_viewing_completed_node() {
            // History view with search; lowercase the query once, not per item.
            if self.search_query.is_empty() {
                return self.history_items.iter().collect();
            }
            let query = self.search_query.to_lowercase();
            self.history_items
                .iter()
                .filter(|task| task.filename.to_lowercase().contains(&query))
                .collect()
        } else {
            // Direct folder access - no filtering needed
            match self.selected_folder_id_from_tree() {
                Some(folder_id) => {
                    self.folder_downloads
                        .get(folder_id)
                        .map(|tasks| tasks.iter().collect())
                        .unwrap_or_default()
                }
                None => Vec::new(),
            }
        }
    }

    /// Backwards compatibility alias for filtered_downloads
    /// TODO: Remove after full migration
    pub fn filtered_downloads(&self) -> Vec<&DownloadTask> {
        self.current_downloads()
    }

    /// Get total count of downloads across all folders
    pub fn total_download_count(&self) -> usize {
        self.folder_downloads.values().map(|v| v.len()).sum()
    }

    /// Move selection down
    pub fn move_selection_down(&mut self) {
        let filtered_count = self.filtered_downloads().len();
        if filtered_count > 0 {
            self.selected_index = (self.selected_index + 1).min(filtered_count - 1);
            self.table_state.borrow_mut().select(Some(self.selected_index));
        }
    }

    /// Move selection up
    pub fn move_selection_up(&mut self) {
        if self.selected_index > 0 {
            self.selected_index -= 1;
            self.table_state.borrow_mut().select(Some(self.selected_index));
        }
    }

    /// Move to top
    pub fn move_to_top(&mut self) {
        self.selected_index = 0;
        self.table_state.borrow_mut().select(Some(0));
    }

    /// Move to bottom
    pub fn move_to_bottom(&mut self) {
        let filtered_count = self.filtered_downloads().len();
        if filtered_count > 0 {
            self.selected_index = filtered_count - 1;
            self.table_state.borrow_mut().select(Some(self.selected_index));
        }
    }

    /// Get selected download
    pub fn get_selected_download(&self) -> Option<&DownloadTask> {
        let filtered = self.filtered_downloads();
        filtered.get(self.selected_index).copied()
    }

    /// Cycle focus to the next pane
    pub fn focus_next_pane(&mut self) {
        self.focus_pane = match self.focus_pane {
            FocusPane::FolderTree => FocusPane::DownloadList,
            FocusPane::DownloadList => {
                if self.details_position != DetailsPosition::Hidden {
                    FocusPane::DetailsPanel
                } else {
                    FocusPane::FolderTree
                }
            }
            FocusPane::DetailsPanel => FocusPane::FolderTree,
        };
    }

    /// Cycle focus to the previous pane
    pub fn focus_prev_pane(&mut self) {
        self.focus_pane = match self.focus_pane {
            FocusPane::FolderTree => {
                if self.details_position != DetailsPosition::Hidden {
                    FocusPane::DetailsPanel
                } else {
                    FocusPane::DownloadList
                }
            }
            FocusPane::DownloadList => FocusPane::FolderTree,
            FocusPane::DetailsPanel => FocusPane::DownloadList,
        };
    }

    /// Focus a specific pane
    pub fn set_focus(&mut self, pane: FocusPane) {
        self.focus_pane = pane;
    }

    /// Move tree selection down (visual only, use sync_current_folder_from_tree to confirm)
    pub fn move_tree_selection_down(&mut self) {
        let count = self.tree_items.len();
        if count > 0 {
            self.tree_selected_index = (self.tree_selected_index + 1).min(count - 1);
            // Reset download list selection when changing folder view
            self.selected_index = 0;
            self.table_state.borrow_mut().select(Some(0));
        }
    }

    /// Move tree selection up (visual only, use sync_current_folder_from_tree to confirm)
    pub fn move_tree_selection_up(&mut self) {
        if self.tree_selected_index > 0 {
            self.tree_selected_index -= 1;
            // Reset download list selection when changing folder view
            self.selected_index = 0;
            self.table_state.borrow_mut().select(Some(0));
        }
    }

    /// Update the tree cursor and, only if it actually moved, reset the
    /// download list selection to match: `current_downloads` follows the
    /// tree cursor, so landing on a different row means a different list.
    fn set_tree_index(&mut self, new_index: usize) {
        if new_index != self.tree_selected_index {
            self.tree_selected_index = new_index;
            self.selected_index = 0;
            self.table_state.borrow_mut().select(Some(0));
        }
    }

    /// Move tree selection to the top row (visual only, use
    /// sync_current_folder_from_tree to confirm)
    pub fn move_tree_to_top(&mut self) {
        self.set_tree_index(tree_top_index(self.tree_items.len()));
    }

    /// Move tree selection to the bottom row (visual only, use
    /// sync_current_folder_from_tree to confirm)
    pub fn move_tree_to_bottom(&mut self) {
        self.set_tree_index(tree_bottom_index(self.tree_items.len()));
    }

    /// Page the tree selection up (visual only, use
    /// sync_current_folder_from_tree to confirm)
    pub fn tree_page_up(&mut self) {
        self.set_tree_index(tree_page_up_index(self.tree_selected_index, TREE_PAGE_SIZE));
    }

    /// Page the tree selection down (visual only, use
    /// sync_current_folder_from_tree to confirm)
    pub fn tree_page_down(&mut self) {
        self.set_tree_index(tree_page_down_index(
            self.tree_selected_index,
            self.tree_items.len(),
            TREE_PAGE_SIZE,
        ));
    }

    /// Sync current_folder_id with tree selection (if a folder is selected)
    pub fn sync_current_folder_from_tree(&mut self) {
        if let Some(FolderTreeItem::Folder(folder_id)) = self.selected_tree_item() {
            self.current_folder_id = folder_id.clone();
        }
    }

    /// Toggle details panel position (Bottom -> Right -> Hidden -> Bottom)
    pub fn toggle_details_position(&mut self) {
        self.details_position = match self.details_position {
            DetailsPosition::Bottom => DetailsPosition::Right,
            DetailsPosition::Right => DetailsPosition::Hidden,
            DetailsPosition::Hidden => DetailsPosition::Bottom,
        };
        // If details panel is hidden and it was focused, move focus
        if self.details_position == DetailsPosition::Hidden && self.focus_pane == FocusPane::DetailsPanel {
            self.focus_pane = FocusPane::DownloadList;
        }
    }

    /// Set search query
    pub fn set_search_query(&mut self, query: String) {
        self.search_query = query;
        self.selected_index = 0;
        self.table_state.borrow_mut().select(Some(0));
    }

    /// Clear search
    pub fn clear_search(&mut self) {
        self.search_query.clear();
    }

    /// Get table state reference (for rendering)
    pub fn table_state(&self) -> std::cell::Ref<'_, TableState> {
        self.table_state.borrow()
    }

    /// Get mutable table state reference
    pub fn table_state_mut(&self) -> std::cell::RefMut<'_, TableState> {
        self.table_state.borrow_mut()
    }

    /// Adjust selection after delete
    pub fn adjust_selection_after_delete(&mut self) {
        let filtered_count = self.filtered_downloads().len();
        if filtered_count == 0 {
            self.selected_index = 0;
        } else if self.selected_index >= filtered_count {
            self.selected_index = filtered_count - 1;
        }
        self.table_state.borrow_mut().select(Some(self.selected_index));
    }

    /// Move folder selection down in settings screen
    pub fn move_folder_selection_down(&mut self, folder_count: usize) {
        if folder_count > 0 {
            self.settings_folder_index = (self.settings_folder_index + 1).min(folder_count - 1);
        }
    }

    /// Move folder selection up in settings screen
    pub fn move_folder_selection_up(&mut self) {
        if self.settings_folder_index > 0 {
            self.settings_folder_index -= 1;
        }
    }

    /// Move folder selection to the first row.
    pub fn move_folder_selection_to_top(&mut self) {
        self.settings_folder_index = tree_top_index(0);
    }

    /// Move folder selection to the last row.
    pub fn move_folder_selection_to_bottom(&mut self, folder_count: usize) {
        self.settings_folder_index = tree_bottom_index(folder_count);
    }

    /// Page the folder selection up.
    pub fn folder_selection_page_up(&mut self) {
        self.settings_folder_index = tree_page_up_index(self.settings_folder_index, TREE_PAGE_SIZE);
    }

    /// Page the folder selection down, clamped to `folder_count` rows.
    pub fn folder_selection_page_down(&mut self, folder_count: usize) {
        self.settings_folder_index =
            tree_page_down_index(self.settings_folder_index, folder_count, TREE_PAGE_SIZE);
    }

    /// Move the folder-picker dialog selection down, wrapping at the end
    /// (ChangeFolder / SwitchFolder).
    pub fn move_folder_picker_down(&mut self, folder_count: usize) {
        if folder_count > 0 {
            self.folder_picker_index = (self.folder_picker_index + 1) % folder_count;
        }
    }

    /// Move the folder-picker dialog selection up, wrapping at the start.
    pub fn move_folder_picker_up(&mut self, folder_count: usize) {
        if folder_count > 0 {
            self.folder_picker_index = if self.folder_picker_index == 0 {
                folder_count - 1
            } else {
                self.folder_picker_index - 1
            };
        }
    }

    /// Move the folder-picker dialog selection to the first row.
    pub fn move_folder_picker_to_top(&mut self) {
        self.folder_picker_index = tree_top_index(0);
    }

    /// Move the folder-picker dialog selection to the last row.
    pub fn move_folder_picker_to_bottom(&mut self, folder_count: usize) {
        self.folder_picker_index = tree_bottom_index(folder_count);
    }

    /// Page the folder-picker dialog selection up.
    pub fn folder_picker_page_up(&mut self) {
        self.folder_picker_index = tree_page_up_index(self.folder_picker_index, TREE_PAGE_SIZE);
    }

    /// Page the folder-picker dialog selection down, clamped to `folder_count` rows.
    pub fn folder_picker_page_down(&mut self, folder_count: usize) {
        self.folder_picker_index =
            tree_page_down_index(self.folder_picker_index, folder_count, TREE_PAGE_SIZE);
    }

    /// Reset settings screen state
    pub fn reset_settings_state(&mut self) {
        self.selected_folder_id = None;
        self.settings_edit_field = None;
        self.settings_folder_index = 0;
        self.settings_field_index = 0;
        self.settings_section = SettingsSection::General;
        self.app_settings_field_index = 0;
        self.is_editing_app_setting = false;
        self.input_buffer.clear();
    }

    /// The application settings field currently selected, if any (the field
    /// list depends on the active tab, so this replaces indexing
    /// `ApplicationSettingsField::all()` directly). `None` both when the
    /// index is out of range and when it points at a flattened script row on
    /// the Scripts tab; use `current_app_script_index` for the latter.
    pub fn current_app_field(&self) -> Option<ApplicationSettingsField> {
        ApplicationSettingsField::in_section(self.settings_section)
            .get(self.app_settings_field_index)
            .copied()
    }

    /// Number of selectable rows on the current application settings tab:
    /// the fixed fields plus, on the Scripts tab, one row per cached script
    /// file.
    pub fn app_settings_row_count(&self) -> usize {
        ApplicationSettingsField::row_count(self.settings_section, self.cached_script_files.len())
    }

    /// Move the application settings row selection down, wrapping at the end.
    pub fn move_app_settings_down(&mut self) {
        let row_count = self.app_settings_row_count();
        if row_count > 0 {
            self.app_settings_field_index = (self.app_settings_field_index + 1) % row_count;
        }
    }

    /// Move the application settings row selection up, wrapping at the start.
    pub fn move_app_settings_up(&mut self) {
        let row_count = self.app_settings_row_count();
        if row_count > 0 {
            self.app_settings_field_index = if self.app_settings_field_index == 0 {
                row_count - 1
            } else {
                self.app_settings_field_index - 1
            };
        }
    }

    /// Move the application settings row selection to the first row.
    pub fn move_app_settings_to_top(&mut self) {
        self.app_settings_field_index = tree_top_index(0);
    }

    /// Move the application settings row selection to the last row.
    pub fn move_app_settings_to_bottom(&mut self) {
        self.app_settings_field_index = tree_bottom_index(self.app_settings_row_count());
    }

    /// Page the application settings row selection up.
    pub fn app_settings_page_up(&mut self) {
        self.app_settings_field_index =
            tree_page_up_index(self.app_settings_field_index, TREE_PAGE_SIZE);
    }

    /// Page the application settings row selection down.
    pub fn app_settings_page_down(&mut self) {
        self.app_settings_field_index = tree_page_down_index(
            self.app_settings_field_index,
            self.app_settings_row_count(),
            TREE_PAGE_SIZE,
        );
    }

    /// Index into `cached_script_files` for the currently selected row, if
    /// `app_settings_field_index` points past the fixed fields on the
    /// Scripts tab.
    pub fn current_app_script_index(&self) -> Option<usize> {
        if self.settings_section != SettingsSection::Scripts {
            return None;
        }
        let field_count = ApplicationSettingsField::in_section(self.settings_section).len();
        self.app_settings_field_index
            .checked_sub(field_count)
            .filter(|i| *i < self.cached_script_files.len())
    }

    /// Number of selectable rows in the folder editor: the fixed
    /// `SettingsField`s plus one row per cached script file (folder
    /// override).
    pub fn folder_settings_row_count(&self) -> usize {
        SettingsField::row_count(self.cached_script_files.len())
    }

    /// Index into `cached_script_files` for the currently selected row, if
    /// `settings_field_index` points past the fixed `SettingsField` rows.
    pub fn current_folder_script_index(&self) -> Option<usize> {
        let field_count = SettingsField::all().len();
        self.settings_field_index
            .checked_sub(field_count)
            .filter(|i| *i < self.cached_script_files.len())
    }

    /// Move field selection down in folder edit mode
    pub fn move_field_selection_down(&mut self, field_count: usize) {
        if field_count > 0 {
            self.settings_field_index = (self.settings_field_index + 1).min(field_count - 1);
        }
    }

    /// Move field selection up in folder edit mode
    pub fn move_field_selection_up(&mut self) {
        if self.settings_field_index > 0 {
            self.settings_field_index -= 1;
        }
    }

    /// Move field selection to the first row in folder edit mode.
    pub fn move_field_selection_to_top(&mut self) {
        self.settings_field_index = tree_top_index(0);
    }

    /// Move field selection to the last row in folder edit mode.
    pub fn move_field_selection_to_bottom(&mut self, field_count: usize) {
        self.settings_field_index = tree_bottom_index(field_count);
    }

    /// Page field selection up in folder edit mode.
    pub fn field_selection_page_up(&mut self) {
        self.settings_field_index = tree_page_up_index(self.settings_field_index, TREE_PAGE_SIZE);
    }

    /// Page field selection down in folder edit mode, clamped to `field_count` rows.
    pub fn field_selection_page_down(&mut self, field_count: usize) {
        self.settings_field_index =
            tree_page_down_index(self.settings_field_index, field_count, TREE_PAGE_SIZE);
    }

    /// Toggle selection for current download
    pub fn toggle_selection(&mut self) {
        // Get the ID first to avoid borrow issues
        let task_id = self.get_selected_download().map(|t| t.id);
        if let Some(id) = task_id {
            if self.selected_downloads.contains(&id) {
                self.selected_downloads.remove(&id);
            } else {
                self.selected_downloads.insert(id);
            }
        }
    }

    /// Check if a download is selected
    pub fn is_download_selected(&self, id: uuid::Uuid) -> bool {
        self.selected_downloads.contains(&id)
    }

    /// Clear all selections
    pub fn clear_selections(&mut self) {
        self.selected_downloads.clear();
    }

    /// Get all selected download IDs
    pub fn get_selected_download_ids(&self) -> Vec<uuid::Uuid> {
        self.selected_downloads.iter().copied().collect()
    }

    /// Determine target download IDs based on selection state.
    /// If the current (cursor) item is in the selected set, return all selected IDs.
    /// Otherwise, return only the current item's ID.
    pub fn get_target_download_ids(&self) -> Vec<Uuid> {
        if let Some(current) = self.get_selected_download() {
            if self.selected_downloads.contains(&current.id) {
                return self.selected_downloads.iter().copied().collect();
            }
            return vec![current.id];
        }
        vec![]
    }

    /// Select all visible downloads
    pub fn select_all(&mut self) {
        // Collect IDs first to avoid borrow issues
        let ids: Vec<uuid::Uuid> = self.filtered_downloads().iter().map(|t| t.id).collect();
        for id in ids {
            self.selected_downloads.insert(id);
        }
    }

    /// Move context menu selection down
    pub fn context_menu_move_down(&mut self) {
        let menu_items = ContextMenuAction::all();
        if !menu_items.is_empty() {
            self.context_menu_index = (self.context_menu_index + 1).min(menu_items.len() - 1);
        }
    }

    /// Move context menu selection up
    pub fn context_menu_move_up(&mut self) {
        if self.context_menu_index > 0 {
            self.context_menu_index -= 1;
        }
    }

    /// Get selected context menu action
    pub fn get_selected_menu_action(&self) -> Option<ContextMenuAction> {
        let menu_items = ContextMenuAction::all();
        menu_items.get(self.context_menu_index).copied()
    }

    /// Reset context menu state
    pub fn reset_context_menu(&mut self) {
        self.context_menu_index = 0;
    }

    /// Move folder context menu selection down
    pub fn folder_context_menu_move_down(&mut self, is_completed_node: bool) {
        let menu_items = if is_completed_node {
            FolderContextMenuAction::all_for_completed()
        } else {
            FolderContextMenuAction::all_for_folder()
        };
        if !menu_items.is_empty() {
            self.folder_context_menu_index =
                (self.folder_context_menu_index + 1).min(menu_items.len() - 1);
        }
    }

    /// Move folder context menu selection up
    pub fn folder_context_menu_move_up(&mut self) {
        if self.folder_context_menu_index > 0 {
            self.folder_context_menu_index -= 1;
        }
    }

    /// Get selected folder context menu action
    pub fn get_selected_folder_menu_action(
        &self,
        is_completed_node: bool,
    ) -> Option<FolderContextMenuAction> {
        let menu_items = if is_completed_node {
            FolderContextMenuAction::all_for_completed()
        } else {
            FolderContextMenuAction::all_for_folder()
        };
        menu_items.get(self.folder_context_menu_index).copied()
    }

    /// Reset folder context menu state
    pub fn reset_folder_context_menu(&mut self) {
        self.folder_context_menu_index = 0;
    }

    /// Get translated string by key
    ///
    /// # Arguments
    /// * `key` - Translation key (e.g., "help-title")
    ///
    /// # Returns
    /// * Translated string for current locale
    pub fn t(&self, key: &str) -> String {
        self.i18n.get(key)
    }

    /// Get translated string with arguments
    ///
    /// # Arguments
    /// * `key` - Translation key
    /// * `args` - Arguments for parameterized translations
    ///
    /// # Returns
    /// * Translated string with substituted arguments
    pub fn t_with_args(&self, key: &str, args: Option<&fluent_bundle::FluentArgs>) -> String {
        self.i18n.get_with_args(key, args)
    }

    /// Look up folder display name from UUID.
    /// Returns the display name from the cache, or the raw ID as fallback.
    pub fn folder_display_name(&self, folder_id: &str) -> String {
        self.folder_names
            .get(folder_id)
            .cloned()
            .unwrap_or_else(|| folder_id.to_string())
    }

    /// Get the display name of the current folder
    pub fn current_folder_name(&self) -> String {
        self.folder_display_name(&self.current_folder_id)
    }

    /// Mark UI as needing redraw (dirty flag)
    pub fn mark_dirty(&mut self) {
        self.needs_redraw = true;
    }

    /// Check if UI needs redraw
    pub fn needs_redraw(&self) -> bool {
        self.needs_redraw
    }

    /// Clear dirty flag after rendering
    pub fn clear_dirty(&mut self) {
        self.needs_redraw = false;
    }

    /// Reads the confirm-delete target and resets it to `Downloads`, so a
    /// stale folder target never lingers into the next Normal-mode delete.
    /// Called on both the confirm and the cancel path.
    pub fn take_delete_target(&mut self) -> DeleteTarget {
        std::mem::take(&mut self.delete_target)
    }
}

#[cfg(test)]
mod settings_tests {
    use super::*;

    #[test]
    fn in_section_covers_every_field_exactly_once() {
        let mut seen = std::collections::HashSet::new();
        for section in SettingsSection::ALL {
            for field in ApplicationSettingsField::in_section(section) {
                assert!(
                    seen.insert(field),
                    "{:?} appeared in more than one section",
                    field
                );
                assert_eq!(field.section(), section);
            }
        }
        assert_eq!(seen.len(), ApplicationSettingsField::all().len());
    }

    #[test]
    fn settings_section_next_and_prev_wrap() {
        assert_eq!(SettingsSection::General.next(), SettingsSection::Downloads);
        assert_eq!(SettingsSection::Downloads.next(), SettingsSection::Scripts);
        assert_eq!(SettingsSection::Scripts.next(), SettingsSection::Folders);
        assert_eq!(SettingsSection::Folders.next(), SettingsSection::General);

        assert_eq!(SettingsSection::General.prev(), SettingsSection::Folders);
        assert_eq!(SettingsSection::Folders.prev(), SettingsSection::Scripts);
        assert_eq!(SettingsSection::Scripts.prev(), SettingsSection::Downloads);
        assert_eq!(SettingsSection::Downloads.prev(), SettingsSection::General);
    }

    #[test]
    fn folder_settings_field_index_zero_is_name() {
        assert_eq!(SettingsField::from_index(0), Some(SettingsField::FolderName));
    }

    #[test]
    fn folder_settings_field_all_has_no_duplicates() {
        let all = SettingsField::all();
        for i in 0..all.len() {
            for j in (i + 1)..all.len() {
                assert_ne!(all[i], all[j], "duplicate field in SettingsField::all()");
            }
        }
    }

    #[test]
    fn app_settings_row_count_flattens_scripts_on_the_scripts_tab() {
        let field_count = ApplicationSettingsField::in_section(SettingsSection::Scripts).len();
        assert_eq!(
            ApplicationSettingsField::row_count(SettingsSection::Scripts, 0),
            field_count
        );
        assert_eq!(
            ApplicationSettingsField::row_count(SettingsSection::Scripts, 5),
            field_count + 5
        );
    }

    #[test]
    fn app_settings_row_count_ignores_script_files_on_other_tabs() {
        let field_count = ApplicationSettingsField::in_section(SettingsSection::General).len();
        assert_eq!(
            ApplicationSettingsField::row_count(SettingsSection::General, 0),
            field_count
        );
        assert_eq!(
            ApplicationSettingsField::row_count(SettingsSection::General, 5),
            field_count
        );
    }

    #[test]
    fn folder_settings_row_count_flattens_script_overrides() {
        let field_count = SettingsField::all().len();
        assert_eq!(SettingsField::row_count(0), field_count);
        assert_eq!(SettingsField::row_count(4), field_count + 4);
    }
}

#[cfg(test)]
mod delete_target_tests {
    use super::*;
    use crate::app::config::Config;
    use crate::app::keybindings::KeybindingsConfig;

    fn test_state() -> TuiState {
        TuiState::new(AppState::new(Config::default()), &KeybindingsConfig::default(), "en")
    }

    #[test]
    fn delete_target_defaults_to_downloads() {
        let state = test_state();
        assert_eq!(state.delete_target, DeleteTarget::Downloads);
    }

    #[test]
    fn take_delete_target_resets_to_downloads_after_confirm() {
        let mut state = test_state();
        state.delete_target = DeleteTarget::Folder("folder-1".to_string());

        let taken = state.take_delete_target();

        assert_eq!(taken, DeleteTarget::Folder("folder-1".to_string()));
        assert_eq!(state.delete_target, DeleteTarget::Downloads);
    }

    #[test]
    fn take_delete_target_resets_to_downloads_after_cancel() {
        let mut state = test_state();
        state.delete_target = DeleteTarget::Folder("folder-2".to_string());

        // Cancel reads the target the same way confirm does; both paths must
        // leave the field reset for the next delete.
        let _ = state.take_delete_target();
        let taken_again = state.take_delete_target();

        assert_eq!(taken_again, DeleteTarget::Downloads);
        assert_eq!(state.delete_target, DeleteTarget::Downloads);
    }
}

#[cfg(test)]
mod tree_navigation_tests {
    use super::*;

    #[test]
    fn tree_top_index_is_always_zero() {
        assert_eq!(tree_top_index(0), 0);
        assert_eq!(tree_top_index(1), 0);
        assert_eq!(tree_top_index(5), 0);
    }

    #[test]
    fn tree_bottom_index_clamps_to_last_row() {
        assert_eq!(tree_bottom_index(0), 0);
        assert_eq!(tree_bottom_index(1), 0);
        assert_eq!(tree_bottom_index(5), 4);
    }

    #[test]
    fn tree_page_up_index_does_not_underflow() {
        assert_eq!(tree_page_up_index(0, 10), 0);
        assert_eq!(tree_page_up_index(3, 10), 0);
        assert_eq!(tree_page_up_index(15, 10), 5);
    }

    #[test]
    fn tree_page_down_index_clamps_to_last_row() {
        assert_eq!(tree_page_down_index(0, 0, 10), 0);
        assert_eq!(tree_page_down_index(0, 1, 10), 0);
        assert_eq!(tree_page_down_index(0, 15, 10), 10);
        assert_eq!(tree_page_down_index(8, 15, 10), 14);
    }
}

#[cfg(test)]
mod menu_accelerator_tests {
    use super::*;
    use crate::app::keybindings::{KeyAction, KeyContext, KeybindingResolver, KeybindingsConfig};
    use crossterm::event::{KeyCode, KeyModifiers};

    /// Local menu accelerators with no `KeyAction` of their own (see
    /// `handle_context_menu_mode` / `handle_folder_context_menu_mode`). None
    /// of these may collide with a default key resolved in `KeyContext::Menu`,
    /// or the accelerator would be unreachable.
    const LOCAL_ACCELERATORS: &[char] = &['f', 'c', 'o', 'p'];

    #[test]
    fn local_accelerators_do_not_collide_with_default_menu_keys() {
        let resolver = KeybindingResolver::from_config(&KeybindingsConfig::default());

        for &c in LOCAL_ACCELERATORS {
            let resolved = resolver.resolve_in(KeyContext::Menu, KeyCode::Char(c), KeyModifiers::empty());
            assert_eq!(
                resolved, None,
                "local accelerator '{}' collides with default Menu action {:?}",
                c, resolved
            );
        }
    }

    #[test]
    fn context_menu_key_hint_reflects_a_remap() {
        let mut bindings = std::collections::HashMap::new();
        bindings.insert(
            KeyAction::ToggleDownload,
            crate::app::keybindings::KeyBindingSpec::Single("t".into()),
        );
        let resolver = KeybindingResolver::from_config(&KeybindingsConfig { bindings });

        assert_eq!(ContextMenuAction::StartPause.key_hint(&resolver), "t");
        // Local accelerators are unaffected by a resolver remap.
        assert_eq!(ContextMenuAction::ChangeFolder.key_hint(&resolver), "f");
    }
}
