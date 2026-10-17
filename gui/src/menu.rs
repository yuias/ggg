//! Native menu bar. The layout is plain data so it can be checked without a
//! window; the Win32 menu itself exists only on Windows.

use crate::{
    commands::GuiCommand,
    i18n::Tr,
};

/// One entry of a drop-down menu.
#[derive(Clone, Copy)]
enum Item {
    /// Label message id and the shortcut text shown right-aligned after a tab.
    /// Shortcuts are handled elsewhere; the text only documents them.
    Command(GuiCommand, &'static str, &'static str),
    /// A command with a check mark that follows the details-pane setting.
    Details(&'static str, &'static str),
    Separator,
}

const LAYOUT: &[(&str, &[Item])] = &[
    (
        "gui-menu-file",
        &[
            Item::Command(GuiCommand::NewDownload, "gui-menu-new-download", "Ctrl+N"),
            Item::Command(GuiCommand::PasteUrlsDirect, "gui-menu-paste-and-add", "Ctrl+Shift+V"),
            Item::Separator,
            Item::Command(GuiCommand::Exit, "gui-menu-exit", "Alt+F4"),
        ],
    ),
    (
        "gui-menu-edit",
        &[
            Item::Command(GuiCommand::Undo, "gui-menu-undo", "Ctrl+Z"),
            Item::Separator,
            Item::Command(GuiCommand::CopyUrls, "gui-menu-copy-urls", "Ctrl+C"),
            Item::Command(GuiCommand::PasteUrls, "gui-menu-paste-urls", "Ctrl+V"),
            Item::Command(GuiCommand::Delete, "gui-menu-delete", "Delete"),
            Item::Separator,
            Item::Command(GuiCommand::SelectAll, "gui-menu-select-all", "Ctrl+A"),
            Item::Command(GuiCommand::SelectNone, "gui-menu-select-none", "Esc"),
            Item::Separator,
            Item::Command(GuiCommand::Find, "gui-menu-find", "Ctrl+F"),
        ],
    ),
    (
        "gui-menu-download",
        &[
            Item::Command(GuiCommand::StartPause, "gui-menu-start-pause", "Space"),
            Item::Command(GuiCommand::Retry, "gui-menu-retry", "Ctrl+R"),
            Item::Separator,
            Item::Command(GuiCommand::EditItem, "gui-menu-edit-item", "F2"),
            Item::Command(GuiCommand::MoveToFolder, "gui-menu-move-to-folder", "Ctrl+M"),
            Item::Command(
                GuiCommand::OpenContainingFolder,
                "gui-menu-open-containing-folder",
                "",
            ),
            Item::Separator,
            Item::Command(GuiCommand::ResumeAll, "gui-menu-start-all", ""),
            Item::Command(GuiCommand::PauseAll, "gui-menu-pause-all", ""),
        ],
    ),
    (
        "gui-menu-view",
        &[
            Item::Details("gui-menu-details-pane", "Ctrl+I"),
            Item::Command(GuiCommand::Refresh, "gui-menu-refresh", "F5"),
        ],
    ),
    (
        "gui-menu-tools",
        &[Item::Command(GuiCommand::OpenSettings, "gui-menu-settings", "Ctrl+,")],
    ),
    (
        "gui-menu-help",
        &[
            Item::Command(GuiCommand::ShowShortcuts, "gui-menu-shortcuts", "F1"),
            Item::Separator,
            Item::Command(GuiCommand::About, "gui-menu-about", ""),
        ],
    ),
];

/// Label with the shortcut text after a tab, which Windows aligns in a column.
/// The tab is added here because Fluent does not expand `\t` escapes.
#[cfg_attr(not(windows), allow(dead_code))]
fn label(tr: &Tr, key: &str, shortcut: &str) -> String {
    let text = tr.t(key);
    if shortcut.is_empty() {
        text
    } else {
        format!("{text}\t{shortcut}")
    }
}

#[cfg(windows)]
pub use win::MenuBar;

#[cfg(not(windows))]
pub struct MenuBar;

#[cfg(not(windows))]
impl MenuBar {
    pub fn new(_tr: &Tr) -> Option<Self> {
        None
    }

    pub fn sync(&mut self, _ctx: &crate::commands::CommandContext, _details_visible: bool) {}

    pub fn invalidate(&mut self) {}

    pub fn command(&self, _id: &str) -> Option<GuiCommand> {
        None
    }

    pub fn attach(&self, _hwnd: isize, _dark: bool) {}

    pub fn set_dark(&self, _hwnd: isize, _dark: bool) {}
}

#[cfg(windows)]
mod win {
    use super::{Item, LAYOUT, label};
    use crate::{
        commands::{CommandContext, GuiCommand, is_enabled},
        i18n::Tr,
    };
    use muda::{CheckMenuItem, IsMenuItem, Menu, MenuItem, MenuTheme, PredefinedMenuItem, Submenu};

    /// Id of the details-pane item; commands use their `Debug` name.
    const DETAILS_ID: &str = "ToggleDetails";

    /// Chosen explicitly: muda's automatic mode needs the process to opt in to
    /// dark mode through undocumented uxtheme calls, which winit does not make.
    fn theme(dark: bool) -> MenuTheme {
        if dark { MenuTheme::Dark } else { MenuTheme::Light }
    }

    pub struct MenuBar {
        menu: Menu,
        items: Vec<(GuiCommand, MenuItem)>,
        details: CheckMenuItem,
        /// State last pushed to the items, to skip redundant Win32 calls.
        applied: Option<(CommandContext, bool)>,
    }

    impl MenuBar {
        pub fn new(tr: &Tr) -> Option<Self> {
            match Self::build(tr) {
                Ok(bar) => Some(bar),
                Err(e) => {
                    tracing::error!("Failed to build the menu bar: {e}");
                    None
                }
            }
        }

        fn build(tr: &Tr) -> muda::Result<Self> {
            let menu = Menu::new();
            let mut items = Vec::new();
            let mut details = None;
            for (title, entries) in LAYOUT {
                let submenu = Submenu::new(tr.t(title), true);
                for entry in *entries {
                    match *entry {
                        Item::Command(cmd, key, shortcut) => {
                            let item = MenuItem::with_id(
                                format!("{cmd:?}"),
                                label(tr, key, shortcut),
                                false,
                                None,
                            );
                            submenu.append(&item)?;
                            items.push((cmd, item));
                        }
                        Item::Details(key, shortcut) => {
                            let item = CheckMenuItem::with_id(
                                DETAILS_ID,
                                label(tr, key, shortcut),
                                false,
                                false,
                                None,
                            );
                            submenu.append(&item as &dyn IsMenuItem)?;
                            details = Some(item);
                        }
                        Item::Separator => submenu.append(&PredefinedMenuItem::separator())?,
                    }
                }
                menu.append(&submenu)?;
            }
            Ok(Self {
                menu,
                items,
                details: details.expect("the layout has a details item"),
                applied: None,
            })
        }

        /// Puts the menu bar on the window. The caller resizes the surface
        /// afterwards because the client area shrinks.
        pub fn attach(&self, hwnd: isize, dark: bool) {
            // SAFETY: `hwnd` belongs to the live main window, which outlives
            // the menu (both are owned by the app and dropped together).
            if let Err(e) = unsafe { self.menu.init_for_hwnd_with_theme(hwnd, theme(dark)) } {
                tracing::error!("Failed to attach the menu bar: {e}");
            }
        }

        /// Follows a system theme change; the bar is redrawn by muda.
        pub fn set_dark(&self, hwnd: isize, dark: bool) {
            // SAFETY: as in `attach`.
            if let Err(e) = unsafe { self.menu.set_theme_for_hwnd(hwnd, theme(dark)) } {
                tracing::error!("Failed to switch the menu bar theme: {e}");
            }
        }

        pub fn command(&self, id: &str) -> Option<GuiCommand> {
            if id == DETAILS_ID {
                return Some(GuiCommand::ToggleDetails);
            }
            self.items
                .iter()
                .find(|(cmd, _)| format!("{cmd:?}") == id)
                .map(|(cmd, _)| *cmd)
        }

        /// Forces the next `sync` to rewrite every item. A click toggles a
        /// check mark by itself, which must not outlive a refused command.
        pub fn invalidate(&mut self) {
            self.applied = None;
        }

        /// Enables, disables and checks items to match `ctx`.
        pub fn sync(&mut self, ctx: &CommandContext, details_visible: bool) {
            let state = (ctx.clone(), details_visible);
            if self.applied.as_ref() == Some(&state) {
                return;
            }
            for (cmd, item) in &self.items {
                item.set_enabled(is_enabled(*cmd, ctx));
            }
            self.details
                .set_enabled(is_enabled(GuiCommand::ToggleDetails, ctx));
            self.details.set_checked(details_visible);
            self.applied = Some(state);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_command_has_at_most_one_menu_entry() {
        let mut seen = Vec::new();
        for (_, items) in LAYOUT {
            for item in *items {
                if let Item::Command(cmd, ..) = item {
                    assert!(!seen.contains(cmd), "{cmd:?} appears twice");
                    seen.push(*cmd);
                }
            }
        }
        assert!(!seen.contains(&GuiCommand::ToggleDetails));
    }

    #[test]
    fn labels_come_from_the_catalog_with_a_mnemonic() {
        let tr = Tr::load("en");
        for (title, items) in LAYOUT {
            let keys = std::iter::once(*title).chain(items.iter().filter_map(|item| match item {
                Item::Command(_, key, _) | Item::Details(key, _) => Some(*key),
                Item::Separator => None,
            }));
            let mut mnemonics = Vec::new();
            for key in keys {
                let text = tr.t(key);
                assert!(!text.contains("missing"), "no message for {key}");
                let at = text.find('&').unwrap_or_else(|| panic!("{key} has no mnemonic"));
                let letter = text[at + 1..].chars().next().unwrap().to_ascii_lowercase();
                // The title's mnemonic lives in the bar, the others per menu.
                if key != *title {
                    assert!(!mnemonics.contains(&letter), "{key} reuses mnemonic {letter}");
                    mnemonics.push(letter);
                }
            }
        }
    }
}
