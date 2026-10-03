//! Keyboard shortcut customization support
//!
//! Allows users to customize keyboard shortcuts through configuration.

use crossterm::event::{KeyCode, KeyModifiers};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Actions that can be bound to keyboard shortcuts
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum KeyAction {
    // Navigation
    MoveUp,
    MoveDown,
    MoveToTop,
    MoveToBottom,
    PageUp,
    PageDown,
    FocusNextPane,
    FocusPrevPane,
    FocusLeft,
    FocusRight,

    // Selection
    SelectItem,
    ToggleSelection,
    SelectAll,
    DeselectAll,

    // Actions
    AddDownload,
    DeleteDownload,
    ToggleDownload,
    RetryDownload,
    ResumeAll,
    PauseAll,
    OpenContextMenu,
    EditItem,

    // View
    ToggleDetails,
    CycleDetailsPosition,
    OpenSearch,
    OpenHelp,
    OpenSettings,
    SwitchFolder,

    // System
    Quit,
    Undo,
    Refresh,

    // Variant names are the config keys on disk; never rename or remove one.
    Back,
    ConfirmYes,
    ConfirmNo,
    ReloadConfig,
}

impl KeyAction {
    /// Returns all available actions
    pub fn all() -> Vec<KeyAction> {
        vec![
            KeyAction::MoveUp,
            KeyAction::MoveDown,
            KeyAction::MoveToTop,
            KeyAction::MoveToBottom,
            KeyAction::PageUp,
            KeyAction::PageDown,
            KeyAction::FocusNextPane,
            KeyAction::FocusPrevPane,
            KeyAction::FocusLeft,
            KeyAction::FocusRight,
            KeyAction::SelectItem,
            KeyAction::ToggleSelection,
            KeyAction::SelectAll,
            KeyAction::DeselectAll,
            KeyAction::AddDownload,
            KeyAction::DeleteDownload,
            KeyAction::ToggleDownload,
            KeyAction::RetryDownload,
            KeyAction::ResumeAll,
            KeyAction::PauseAll,
            KeyAction::OpenContextMenu,
            KeyAction::EditItem,
            KeyAction::ToggleDetails,
            KeyAction::CycleDetailsPosition,
            KeyAction::OpenSearch,
            KeyAction::OpenHelp,
            KeyAction::OpenSettings,
            KeyAction::SwitchFolder,
            KeyAction::Quit,
            KeyAction::Undo,
            KeyAction::Refresh,
            KeyAction::Back,
            KeyAction::ConfirmYes,
            KeyAction::ConfirmNo,
            KeyAction::ReloadConfig,
        ]
    }

    /// Contexts in which this action is resolved by [`KeybindingResolver::resolve_in`].
    ///
    /// Exhaustive on purpose: adding a variant forces a decision about where it applies
    /// rather than silently falling into a wildcard arm.
    pub fn contexts(self) -> &'static [KeyContext] {
        use KeyContext::*;
        match self {
            KeyAction::MoveUp | KeyAction::MoveDown => {
                &[Normal, Settings, Menu, Dialog, Help]
            }
            KeyAction::MoveToTop
            | KeyAction::MoveToBottom
            | KeyAction::PageUp
            | KeyAction::PageDown => &[Normal, Settings, Dialog, Help],
            KeyAction::FocusNextPane
            | KeyAction::FocusPrevPane
            | KeyAction::FocusLeft
            | KeyAction::FocusRight => &[Normal, Settings],
            KeyAction::SelectItem => &[Normal, Settings, Menu, Dialog],
            KeyAction::ToggleSelection => &[Normal, Settings],
            KeyAction::AddDownload => &[Normal, Settings],
            KeyAction::DeleteDownload => &[Normal, Settings, Menu],
            KeyAction::ToggleDownload | KeyAction::RetryDownload | KeyAction::EditItem => {
                &[Normal, Menu]
            }
            KeyAction::OpenHelp => &[Normal, Help],
            KeyAction::OpenSettings => &[Normal, Settings],
            KeyAction::Back => &[Settings, Menu, Dialog, Help],
            KeyAction::ConfirmYes | KeyAction::ConfirmNo => &[Dialog],
            KeyAction::ReloadConfig => &[Settings],
            KeyAction::SelectAll
            | KeyAction::DeselectAll
            | KeyAction::ResumeAll
            | KeyAction::PauseAll
            | KeyAction::OpenContextMenu
            | KeyAction::ToggleDetails
            | KeyAction::CycleDetailsPosition
            | KeyAction::OpenSearch
            | KeyAction::SwitchFolder
            | KeyAction::Quit
            | KeyAction::Undo
            | KeyAction::Refresh => &[Normal],
        }
    }
}

/// Screen or overlay a key press is resolved against. Each non-text-input mode picks the
/// context matching its role so one physical key can mean different things (or nothing) in
/// different places without the actions themselves being context-specific.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum KeyContext {
    /// Three-pane main screen.
    Normal,
    /// Settings tabs and the folder editor (non-text state).
    Settings,
    /// ContextMenu, FolderContextMenu.
    Menu,
    /// ChangeFolder, SwitchFolder, ConfirmDelete, DownloadPreview.
    Dialog,
    /// Help screen.
    Help,
}

impl KeyContext {
    pub const ALL: [KeyContext; 5] = [
        KeyContext::Normal,
        KeyContext::Settings,
        KeyContext::Menu,
        KeyContext::Dialog,
        KeyContext::Help,
    ];
}

/// A key binding specification (serializable format)
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum KeyBindingSpec {
    /// Single key (e.g., "j", "Enter", "Space")
    Single(String),
    /// Multiple keys (e.g., ["j", "Down"])
    Multiple(Vec<String>),
}

impl Default for KeyBindingSpec {
    fn default() -> Self {
        KeyBindingSpec::Single(String::new())
    }
}

/// Parsed key combination
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct KeyCombo {
    pub code: KeyCode,
    pub modifiers: KeyModifiers,
}

impl KeyCombo {
    pub fn new(code: KeyCode, modifiers: KeyModifiers) -> Self {
        Self { code, modifiers }
    }

    /// Parse a key string like "Ctrl+j", "Shift+Enter", "Space", "a"
    pub fn parse(s: &str) -> Option<KeyCombo> {
        let s = s.trim();
        let mut modifiers = KeyModifiers::empty();
        let mut key_part = s;

        // Parse modifiers
        let parts: Vec<&str> = s.split('+').collect();
        if parts.len() > 1 {
            for part in &parts[..parts.len() - 1] {
                match part.to_lowercase().as_str() {
                    "ctrl" | "control" => modifiers |= KeyModifiers::CONTROL,
                    "alt" => modifiers |= KeyModifiers::ALT,
                    "shift" => modifiers |= KeyModifiers::SHIFT,
                    _ => return None, // Unknown modifier
                }
            }
            key_part = parts.last()?;
        }

        // Parse key code
        let code = parse_key_code(key_part)?;

        // Same canonical form as normalize_key_event, limited to Tab so that
        // other shifted keys (e.g. "Shift+A") keep their parsing.
        let (code, modifiers) = match code {
            KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => {
                (KeyCode::BackTab, modifiers - KeyModifiers::SHIFT)
            }
            KeyCode::BackTab => (code, modifiers - KeyModifiers::SHIFT),
            _ => (code, modifiers),
        };

        Some(KeyCombo::new(code, modifiers))
    }

    /// Check if this combo matches the given key event
    pub fn matches(&self, code: KeyCode, modifiers: KeyModifiers) -> bool {
        self.code == code && self.modifiers == modifiers
    }
}

/// Parse a key code string
fn parse_key_code(s: &str) -> Option<KeyCode> {
    let s = s.trim();

    // Check for special keys first
    match s.to_lowercase().as_str() {
        "enter" | "return" => return Some(KeyCode::Enter),
        "space" => return Some(KeyCode::Char(' ')),
        "tab" => return Some(KeyCode::Tab),
        "backtab" => return Some(KeyCode::BackTab),
        "backspace" => return Some(KeyCode::Backspace),
        "esc" | "escape" => return Some(KeyCode::Esc),
        "up" => return Some(KeyCode::Up),
        "down" => return Some(KeyCode::Down),
        "left" => return Some(KeyCode::Left),
        "right" => return Some(KeyCode::Right),
        "home" => return Some(KeyCode::Home),
        "end" => return Some(KeyCode::End),
        "pageup" => return Some(KeyCode::PageUp),
        "pagedown" => return Some(KeyCode::PageDown),
        "delete" | "del" => return Some(KeyCode::Delete),
        "insert" | "ins" => return Some(KeyCode::Insert),
        "f1" => return Some(KeyCode::F(1)),
        "f2" => return Some(KeyCode::F(2)),
        "f3" => return Some(KeyCode::F(3)),
        "f4" => return Some(KeyCode::F(4)),
        "f5" => return Some(KeyCode::F(5)),
        "f6" => return Some(KeyCode::F(6)),
        "f7" => return Some(KeyCode::F(7)),
        "f8" => return Some(KeyCode::F(8)),
        "f9" => return Some(KeyCode::F(9)),
        "f10" => return Some(KeyCode::F(10)),
        "f11" => return Some(KeyCode::F(11)),
        "f12" => return Some(KeyCode::F(12)),
        _ => {}
    }

    // Single character
    let mut chars = s.chars();
    let first = chars.next()?;
    if chars.next().is_none() {
        return Some(KeyCode::Char(first));
    }

    None
}

/// Keybindings configuration
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct KeybindingsConfig {
    #[serde(flatten, deserialize_with = "deserialize_known_bindings")]
    pub bindings: HashMap<KeyAction, KeyBindingSpec>,
}

/// Drops entries whose action name this build does not know (renamed,
/// removed, or from a newer build) instead of failing the whole settings
/// file; the resolver fills missing actions with defaults.
fn deserialize_known_bindings<'de, D>(
    deserializer: D,
) -> Result<HashMap<KeyAction, KeyBindingSpec>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::IntoDeserializer;
    let raw: HashMap<String, KeyBindingSpec> = HashMap::deserialize(deserializer)?;
    let mut bindings = HashMap::new();
    for (name, spec) in raw {
        let action: Result<KeyAction, serde::de::value::Error> =
            KeyAction::deserialize(name.as_str().into_deserializer());
        match action {
            Ok(action) => {
                bindings.insert(action, spec);
            }
            Err(_) => tracing::warn!("Ignoring unknown keybinding action '{}'", name),
        }
    }
    Ok(bindings)
}

impl Default for KeybindingsConfig {
    fn default() -> Self {
        let mut bindings = HashMap::new();

        // Navigation
        bindings.insert(
            KeyAction::MoveUp,
            KeyBindingSpec::Multiple(vec!["k".into(), "Up".into()]),
        );
        bindings.insert(
            KeyAction::MoveDown,
            KeyBindingSpec::Multiple(vec!["j".into(), "Down".into()]),
        );
        bindings.insert(
            KeyAction::MoveToTop,
            KeyBindingSpec::Multiple(vec!["g".into(), "Home".into()]),
        );
        bindings.insert(
            KeyAction::MoveToBottom,
            KeyBindingSpec::Multiple(vec!["G".into(), "End".into()]),
        );
        bindings.insert(KeyAction::PageUp, KeyBindingSpec::Single("Ctrl+u".into()));
        bindings.insert(KeyAction::PageDown, KeyBindingSpec::Single("Ctrl+d".into()));
        bindings.insert(KeyAction::FocusNextPane, KeyBindingSpec::Single("Tab".into()));
        bindings.insert(
            KeyAction::FocusPrevPane,
            KeyBindingSpec::Single("BackTab".into()),
        );
        bindings.insert(
            KeyAction::FocusLeft,
            KeyBindingSpec::Multiple(vec!["h".into(), "Left".into()]),
        );
        bindings.insert(
            KeyAction::FocusRight,
            KeyBindingSpec::Multiple(vec!["l".into(), "Right".into()]),
        );

        // Selection
        bindings.insert(KeyAction::SelectItem, KeyBindingSpec::Single("Enter".into()));
        bindings.insert(
            KeyAction::ToggleSelection,
            KeyBindingSpec::Single("Space".into()),
        );
        bindings.insert(KeyAction::SelectAll, KeyBindingSpec::Single("V".into()));
        bindings.insert(
            KeyAction::DeselectAll,
            KeyBindingSpec::Single("Escape".into()),
        );

        // Actions
        bindings.insert(KeyAction::AddDownload, KeyBindingSpec::Single("a".into()));
        bindings.insert(KeyAction::DeleteDownload, KeyBindingSpec::Single("d".into()));
        bindings.insert(KeyAction::ToggleDownload, KeyBindingSpec::Single("s".into()));
        bindings.insert(KeyAction::RetryDownload, KeyBindingSpec::Single("r".into()));
        bindings.insert(KeyAction::ResumeAll, KeyBindingSpec::Single("S".into()));
        bindings.insert(KeyAction::PauseAll, KeyBindingSpec::Single("P".into()));
        bindings.insert(KeyAction::OpenContextMenu, KeyBindingSpec::Single("m".into()));
        bindings.insert(KeyAction::EditItem, KeyBindingSpec::Single("e".into()));

        // View
        bindings.insert(KeyAction::ToggleDetails, KeyBindingSpec::Single("i".into()));
        bindings.insert(KeyAction::CycleDetailsPosition, KeyBindingSpec::Single("D".into()));
        bindings.insert(KeyAction::OpenSearch, KeyBindingSpec::Single("/".into()));
        bindings.insert(KeyAction::OpenHelp, KeyBindingSpec::Single("?".into()));
        bindings.insert(KeyAction::OpenSettings, KeyBindingSpec::Single("x".into()));
        bindings.insert(KeyAction::SwitchFolder, KeyBindingSpec::Single("F".into()));

        // System
        bindings.insert(
            KeyAction::Quit,
            KeyBindingSpec::Multiple(vec!["q".into(), "Ctrl+c".into()]),
        );
        bindings.insert(KeyAction::Undo, KeyBindingSpec::Single("Ctrl+z".into()));
        bindings.insert(KeyAction::Refresh, KeyBindingSpec::Single("R".into()));

        // Cross-context actions (Settings/Menu/Dialog/Help)
        bindings.insert(
            KeyAction::Back,
            KeyBindingSpec::Multiple(vec!["Escape".into(), "q".into()]),
        );
        bindings.insert(KeyAction::ConfirmYes, KeyBindingSpec::Single("y".into()));
        bindings.insert(KeyAction::ConfirmNo, KeyBindingSpec::Single("n".into()));
        bindings.insert(KeyAction::ReloadConfig, KeyBindingSpec::Single("R".into()));

        Self { bindings }
    }
}

/// Runtime keybinding resolver
#[derive(Debug, Clone)]
pub struct KeybindingResolver {
    /// Maps (context, key combination) to the action resolved in that context.
    action_map: HashMap<(KeyContext, KeyCombo), KeyAction>,
    /// Configured key strings per action, kept verbatim (not reverse-formatted from
    /// `KeyCombo`) for display in help text and status-bar hints.
    display: HashMap<KeyAction, Vec<String>>,
}

impl KeybindingResolver {
    /// Create a new resolver from configuration.
    ///
    /// Missing actions are filled from `KeybindingsConfig::default()` so a partial
    /// user `[keybindings]` table still leaves every action bound to something.
    /// An action present in `config.bindings` replaces its whole default spec (all
    /// of that action's default keys are dropped, not merged key-by-key).
    pub fn from_config(config: &KeybindingsConfig) -> Self {
        let mut merged = KeybindingsConfig::default().bindings;
        merged.extend(config.bindings.iter().map(|(action, spec)| (*action, spec.clone())));

        let mut action_map: HashMap<(KeyContext, KeyCombo), KeyAction> = HashMap::new();
        let mut display: HashMap<KeyAction, Vec<String>> = HashMap::new();

        // Iterate in a stable order (by action name) so that when two actions
        // claim the same combo in the same context, the winner is deterministic
        // across runs rather than dependent on HashMap iteration order.
        let mut entries: Vec<(&KeyAction, &KeyBindingSpec)> = merged.iter().collect();
        entries.sort_by_key(|(action, _)| format!("{:?}", action));

        for (action, spec) in entries {
            let keys = match spec {
                KeyBindingSpec::Single(s) => vec![s.clone()],
                KeyBindingSpec::Multiple(v) => v.clone(),
            };

            display.insert(*action, keys.clone());

            for key_str in keys {
                if let Some(combo) = KeyCombo::parse(&key_str) {
                    for &ctx in action.contexts() {
                        if let Some(existing) = action_map.get(&(ctx, combo.clone())) {
                            if existing != action {
                                tracing::warn!(
                                    "Keybinding conflict in {:?} context: '{}' is bound to both {:?} and {:?}; keeping {:?}",
                                    ctx, key_str, existing, action, existing
                                );
                            }
                            continue; // first (stable) binding wins
                        }
                        action_map.insert((ctx, combo.clone()), *action);
                    }
                }
            }
        }

        Self { action_map, display }
    }

    /// Resolve a key event to an action in the `Normal` context.
    pub fn resolve(&self, code: KeyCode, modifiers: KeyModifiers) -> Option<KeyAction> {
        self.resolve_in(KeyContext::Normal, code, modifiers)
    }

    /// Resolve a key event to an action within a specific context.
    pub fn resolve_in(
        &self,
        ctx: KeyContext,
        code: KeyCode,
        modifiers: KeyModifiers,
    ) -> Option<KeyAction> {
        // Normalize shift modifier for uppercase characters
        let (code, modifiers) = normalize_key_event(code, modifiers);

        let combo = KeyCombo::new(code, modifiers);
        self.action_map.get(&(ctx, combo)).copied()
    }

    /// Configured key strings for an action, e.g. `["j", "Down"]`. Empty if unbound.
    pub fn keys_for(&self, action: KeyAction) -> &[String] {
        self.display
            .get(&action)
            .map(Vec::as_slice)
            .unwrap_or(&[])
    }

    /// Short, human-friendly form of the first configured key for `action`,
    /// for on-screen hints (status bar, menus, dialogs, help). `"-"` if the
    /// action is unbound, so a remap-to-nothing shows a placeholder rather
    /// than a stale literal.
    pub fn display_key(&self, action: KeyAction) -> String {
        self.keys_for(action)
            .first()
            .map(|k| format_key_for_display(k))
            .unwrap_or_else(|| "-".to_string())
    }
}

/// Short, human-friendly form of a configured key string, for on-screen
/// display only (help screen, status-bar and dialog hints). The resolver's
/// own key strings stay unabbreviated since they double as the parseable
/// config format; do not use this for parsing.
pub fn format_key_for_display(key: &str) -> String {
    match key {
        "Escape" => "Esc".to_string(),
        "BackTab" => "Shift+Tab".to_string(),
        other => other.to_string(),
    }
}

/// Strip SHIFT where the key code already encodes it. Terminals disagree on
/// whether SHIFT is reported for shifted characters ('?', '!', 'A'-'Z'), and
/// crossterm reports Shift+Tab as BackTab *with* SHIFT.
fn normalize_key_event(code: KeyCode, modifiers: KeyModifiers) -> (KeyCode, KeyModifiers) {
    match code {
        KeyCode::Char(c) if !c.is_ascii_lowercase() => {
            (code, modifiers - KeyModifiers::SHIFT)
        }
        KeyCode::BackTab => (code, modifiers - KeyModifiers::SHIFT),
        KeyCode::Tab if modifiers.contains(KeyModifiers::SHIFT) => {
            (KeyCode::BackTab, modifiers - KeyModifiers::SHIFT)
        }
        _ => (code, modifiers),
    }
}

impl Default for KeybindingResolver {
    fn default() -> Self {
        Self::from_config(&KeybindingsConfig::default())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unknown_action_names_are_ignored() {
        let config: KeybindingsConfig =
            toml::from_str("move_down = \"x\"
no_such_action = \"z\"
").unwrap();
        assert_eq!(config.bindings.len(), 1);
        assert_eq!(
            config.bindings.get(&KeyAction::MoveDown),
            Some(&KeyBindingSpec::Single("x".into()))
        );
    }

    #[test]
    fn backtab_with_shift_resolves_focus_prev_pane() {
        let resolver = KeybindingResolver::default();
        assert_eq!(
            resolver.resolve(KeyCode::BackTab, KeyModifiers::SHIFT),
            Some(KeyAction::FocusPrevPane)
        );
        assert_eq!(
            resolver.resolve(KeyCode::BackTab, KeyModifiers::empty()),
            Some(KeyAction::FocusPrevPane)
        );
    }

    #[test]
    fn shift_tab_event_resolves_like_backtab() {
        let resolver = KeybindingResolver::default();
        assert_eq!(
            resolver.resolve(KeyCode::Tab, KeyModifiers::SHIFT),
            resolver.resolve(KeyCode::BackTab, KeyModifiers::SHIFT)
        );
        assert_eq!(
            resolver.resolve(KeyCode::Tab, KeyModifiers::SHIFT),
            Some(KeyAction::FocusPrevPane)
        );
    }

    #[test]
    fn parse_shift_tab_aliases() {
        for s in ["Shift+Tab", "BackTab", "Shift+BackTab"] {
            let combo = KeyCombo::parse(s).unwrap();
            assert_eq!(combo.code, KeyCode::BackTab, "{s}");
            assert!(combo.modifiers.is_empty(), "{s}");
        }
        let combo = KeyCombo::parse("Ctrl+Shift+Tab").unwrap();
        assert_eq!(combo.code, KeyCode::BackTab);
        assert_eq!(combo.modifiers, KeyModifiers::CONTROL);
    }

    #[test]
    fn custom_shift_tab_binding_resolves() {
        let config: KeybindingsConfig =
            toml::from_str("focus_prev_pane = \"Shift+Tab\"\n").unwrap();
        let resolver = KeybindingResolver::from_config(&config);
        assert_eq!(
            resolver.resolve(KeyCode::BackTab, KeyModifiers::SHIFT),
            Some(KeyAction::FocusPrevPane)
        );
    }

    #[test]
    fn test_parse_single_char() {
        let combo = KeyCombo::parse("j").unwrap();
        assert_eq!(combo.code, KeyCode::Char('j'));
        assert!(combo.modifiers.is_empty());
    }

    #[test]
    fn test_parse_uppercase_char() {
        let combo = KeyCombo::parse("G").unwrap();
        assert_eq!(combo.code, KeyCode::Char('G'));
        assert!(combo.modifiers.is_empty());
    }

    #[test]
    fn test_parse_ctrl_key() {
        let combo = KeyCombo::parse("Ctrl+z").unwrap();
        assert_eq!(combo.code, KeyCode::Char('z'));
        assert!(combo.modifiers.contains(KeyModifiers::CONTROL));
    }

    #[test]
    fn test_parse_special_keys() {
        assert_eq!(KeyCombo::parse("Enter").unwrap().code, KeyCode::Enter);
        assert_eq!(KeyCombo::parse("Space").unwrap().code, KeyCode::Char(' '));
        assert_eq!(KeyCombo::parse("Tab").unwrap().code, KeyCode::Tab);
        assert_eq!(KeyCombo::parse("Escape").unwrap().code, KeyCode::Esc);
        assert_eq!(KeyCombo::parse("Up").unwrap().code, KeyCode::Up);
        assert_eq!(KeyCombo::parse("Down").unwrap().code, KeyCode::Down);
    }

    #[test]
    fn test_default_keybindings() {
        let resolver = KeybindingResolver::default();

        // Test basic navigation
        assert_eq!(
            resolver.resolve(KeyCode::Char('j'), KeyModifiers::empty()),
            Some(KeyAction::MoveDown)
        );
        assert_eq!(
            resolver.resolve(KeyCode::Down, KeyModifiers::empty()),
            Some(KeyAction::MoveDown)
        );
        assert_eq!(
            resolver.resolve(KeyCode::Char('k'), KeyModifiers::empty()),
            Some(KeyAction::MoveUp)
        );

        // Test actions
        assert_eq!(
            resolver.resolve(KeyCode::Char('a'), KeyModifiers::empty()),
            Some(KeyAction::AddDownload)
        );
        assert_eq!(
            resolver.resolve(KeyCode::Char('s'), KeyModifiers::empty()),
            Some(KeyAction::ToggleDownload)
        );
        assert_eq!(
            resolver.resolve(KeyCode::Char(' '), KeyModifiers::empty()),
            Some(KeyAction::ToggleSelection)
        );

        // Test ctrl combinations
        assert_eq!(
            resolver.resolve(KeyCode::Char('z'), KeyModifiers::CONTROL),
            Some(KeyAction::Undo)
        );
    }

    #[test]
    fn test_uppercase_normalization() {
        let resolver = KeybindingResolver::default();

        // G should work for MoveToBottom
        assert_eq!(
            resolver.resolve(KeyCode::Char('G'), KeyModifiers::empty()),
            Some(KeyAction::MoveToBottom)
        );

        // S should work for ResumeAll
        assert_eq!(
            resolver.resolve(KeyCode::Char('S'), KeyModifiers::empty()),
            Some(KeyAction::ResumeAll)
        );
    }

    #[test]
    fn test_resolve_in_context_same_key_different_action() {
        let resolver = KeybindingResolver::default();

        // 's' is ToggleDownload in both Normal and Menu, but unbound in Settings
        // (Settings has no local meaning for it in the design; it must not leak in).
        assert_eq!(
            resolver.resolve_in(KeyContext::Normal, KeyCode::Char('s'), KeyModifiers::empty()),
            Some(KeyAction::ToggleDownload)
        );
        assert_eq!(
            resolver.resolve_in(KeyContext::Menu, KeyCode::Char('s'), KeyModifiers::empty()),
            Some(KeyAction::ToggleDownload)
        );
        assert_eq!(
            resolver.resolve_in(KeyContext::Settings, KeyCode::Char('s'), KeyModifiers::empty()),
            None
        );
    }

    #[test]
    fn test_esc_resolves_to_deselect_in_normal_and_back_in_dialog() {
        let resolver = KeybindingResolver::default();

        assert_eq!(
            resolver.resolve_in(KeyContext::Normal, KeyCode::Esc, KeyModifiers::empty()),
            Some(KeyAction::DeselectAll)
        );
        assert_eq!(
            resolver.resolve_in(KeyContext::Dialog, KeyCode::Esc, KeyModifiers::empty()),
            Some(KeyAction::Back)
        );
    }

    #[test]
    fn test_partial_user_config_still_resolves_defaults() {
        let mut bindings = HashMap::new();
        bindings.insert(KeyAction::Quit, KeyBindingSpec::Single("q".into()));
        let config = KeybindingsConfig { bindings };
        let resolver = KeybindingResolver::from_config(&config);

        assert_eq!(
            resolver.resolve_in(KeyContext::Normal, KeyCode::Char('j'), KeyModifiers::empty()),
            Some(KeyAction::MoveDown)
        );
        assert_eq!(
            resolver.resolve_in(KeyContext::Dialog, KeyCode::Esc, KeyModifiers::empty()),
            Some(KeyAction::Back)
        );

        // The user's entry replaces the whole default spec for that action, so
        // "Ctrl+c" (the other default Quit key) is dropped, not merged in.
        assert_eq!(resolver.keys_for(KeyAction::Quit), &["q".to_string()]);
        assert_eq!(
            resolver.resolve_in(KeyContext::Normal, KeyCode::Char('c'), KeyModifiers::CONTROL),
            None
        );
    }

    #[test]
    fn test_keys_for_returns_configured_strings() {
        let resolver = KeybindingResolver::default();
        assert_eq!(
            resolver.keys_for(KeyAction::MoveDown),
            &["j".to_string(), "Down".to_string()]
        );
    }

    #[test]
    fn test_no_conflicts_within_any_context_in_defaults() {
        let config = KeybindingsConfig::default();

        for &ctx in KeyContext::ALL.iter() {
            let mut seen: HashMap<KeyCombo, KeyAction> = HashMap::new();
            for action in KeyAction::all() {
                let spec = config
                    .bindings
                    .get(&action)
                    .unwrap_or_else(|| panic!("{:?} has no default binding", action));
                let keys = match spec {
                    KeyBindingSpec::Single(s) => vec![s.clone()],
                    KeyBindingSpec::Multiple(v) => v.clone(),
                };
                if !action.contexts().contains(&ctx) {
                    continue;
                }
                for key_str in keys {
                    let combo = KeyCombo::parse(&key_str)
                        .unwrap_or_else(|| panic!("default key '{}' failed to parse", key_str));
                    if let Some(existing) = seen.insert(combo, action) {
                        assert_eq!(
                            existing, action,
                            "conflict in {:?} context: '{}' bound to both {:?} and {:?}",
                            ctx, key_str, existing, action
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn format_key_for_display_shortens_known_names() {
        assert_eq!(format_key_for_display("Escape"), "Esc");
        assert_eq!(format_key_for_display("BackTab"), "Shift+Tab");
        assert_eq!(format_key_for_display("j"), "j");
        assert_eq!(format_key_for_display("Ctrl+u"), "Ctrl+u");
    }

    #[test]
    fn display_key_uses_short_form_and_placeholder() {
        let resolver = KeybindingResolver::default();
        assert_eq!(resolver.display_key(KeyAction::Back), "Esc");
        assert_eq!(resolver.display_key(KeyAction::MoveDown), "j");

        let mut bindings = HashMap::new();
        bindings.insert(KeyAction::Quit, KeyBindingSpec::Multiple(vec![]));
        let resolver = KeybindingResolver::from_config(&KeybindingsConfig { bindings });
        assert_eq!(resolver.display_key(KeyAction::Quit), "-");
    }

    #[test]
    fn test_keybindings_config_serde_round_trip() {
        let original = KeybindingsConfig::default();
        let toml_str = toml::to_string(&original).expect("serialize");
        let restored: KeybindingsConfig = toml::from_str(&toml_str).expect("deserialize");

        let mut original_keys: Vec<_> = original.bindings.keys().copied().collect();
        let mut restored_keys: Vec<_> = restored.bindings.keys().copied().collect();
        original_keys.sort_by_key(|a| format!("{:?}", a));
        restored_keys.sort_by_key(|a| format!("{:?}", a));
        assert_eq!(original_keys, restored_keys);

        for action in original_keys {
            assert_eq!(
                original.bindings.get(&action),
                restored.bindings.get(&action)
            );
        }
    }
}
