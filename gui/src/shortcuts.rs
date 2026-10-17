//! Keyboard shortcuts. One table drives the matching, the menu labels and the
//! shortcut help, so the three cannot drift apart.

use crate::commands::{CommandContext, GuiCommand, is_enabled};
use egui::{Event, Key, Modifiers};

/// A key chord. `ctrl` means Ctrl on Windows and Linux.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Shortcut {
    pub key: Key,
    pub ctrl: bool,
    pub shift: bool,
    pub alt: bool,
}

impl Shortcut {
    const fn key(key: Key) -> Self {
        Self { key, ctrl: false, shift: false, alt: false }
    }

    const fn ctrl(key: Key) -> Self {
        Self { ctrl: true, ..Self::key(key) }
    }

    const fn ctrl_shift(key: Key) -> Self {
        Self { ctrl: true, shift: true, ..Self::key(key) }
    }

    fn modifiers(self) -> Modifiers {
        Modifiers {
            alt: self.alt,
            ctrl: self.ctrl,
            shift: self.shift,
            ..Modifiers::NONE
        }
    }

    #[cfg(test)]
    fn modifier_count(self) -> usize {
        usize::from(self.ctrl) + usize::from(self.shift) + usize::from(self.alt)
    }

    /// Text as shown after a tab in a menu item, e.g. `Ctrl+Shift+V`.
    pub fn text(self) -> String {
        let mut text = String::new();
        for (held, name) in [(self.ctrl, "Ctrl+"), (self.alt, "Alt+"), (self.shift, "Shift+")] {
            if held {
                text.push_str(name);
            }
        }
        match self.key {
            Key::Escape => text.push_str("Esc"),
            key => text.push_str(key.symbol_or_name()),
        }
        text
    }
}

/// How the chord reaches the program.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Trigger {
    /// An ordinary key event.
    Key,
    /// egui-winit turns Ctrl+V, Shift+Insert and the Paste key into
    /// `Event::Paste` and sends no key event for them.
    Paste,
    /// Same for Ctrl+C, Ctrl+Insert and the Copy key.
    Copy,
    /// Handled by Windows before egui sees it; listed for the labels only.
    System,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Scope {
    Anywhere,
    /// Only while the download table has the keyboard.
    Table,
}

struct Binding {
    command: GuiCommand,
    shortcut: Shortcut,
    trigger: Trigger,
    scope: Scope,
}

const fn bind(command: GuiCommand, shortcut: Shortcut, trigger: Trigger, scope: Scope) -> Binding {
    Binding { command, shortcut, trigger, scope }
}

/// Most modifiers first: egui ignores extra Shift and Alt when matching, so
/// Ctrl+Shift+V has to be tried before any plain Ctrl+V.
const BINDINGS: &[Binding] = &[
    bind(GuiCommand::PasteUrlsDirect, Shortcut::ctrl_shift(Key::V), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::NewDownload, Shortcut::ctrl(Key::N), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::PasteUrls, Shortcut::ctrl(Key::V), Trigger::Paste, Scope::Anywhere),
    bind(GuiCommand::CopyUrls, Shortcut::ctrl(Key::C), Trigger::Copy, Scope::Anywhere),
    bind(GuiCommand::SelectAll, Shortcut::ctrl(Key::A), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::Undo, Shortcut::ctrl(Key::Z), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::Retry, Shortcut::ctrl(Key::R), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::MoveToFolder, Shortcut::ctrl(Key::M), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::ToggleDetails, Shortcut::ctrl(Key::I), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::Find, Shortcut::ctrl(Key::F), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::OpenSettings, Shortcut::ctrl(Key::Comma), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::ShowShortcuts, Shortcut::key(Key::F1), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::EditItem, Shortcut::key(Key::F2), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::Refresh, Shortcut::key(Key::F5), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::Delete, Shortcut::key(Key::Delete), Trigger::Key, Scope::Anywhere),
    bind(GuiCommand::StartPause, Shortcut::key(Key::Space), Trigger::Key, Scope::Table),
    bind(GuiCommand::SelectNone, Shortcut::key(Key::Escape), Trigger::Key, Scope::Table),
    bind(
        GuiCommand::Exit,
        Shortcut { alt: true, ..Shortcut::key(Key::F4) },
        Trigger::System,
        Scope::Anywhere,
    ),
];

/// Shortcut of `command`, if it has one.
pub fn shortcut_of(command: GuiCommand) -> Option<Shortcut> {
    BINDINGS
        .iter()
        .find(|binding| binding.command == command)
        .map(|binding| binding.shortcut)
}

/// Shortcut text of `command` for menus and the help dialog.
pub fn shortcut_text(command: GuiCommand) -> Option<String> {
    shortcut_of(command).map(Shortcut::text)
}

/// F-keys and Esc keep working while a text field has the keyboard; they
/// cannot be typed into it.
fn works_while_typing(key: Key) -> bool {
    key == Key::Escape
        || key
            .name()
            .strip_prefix('F')
            .is_some_and(|number| number.parse::<u8>().is_ok())
}

/// Removes the events of one kind and says whether there were any.
fn take_event(events: &mut Vec<Event>, is_kind: impl Fn(&Event) -> bool) -> bool {
    let before = events.len();
    events.retain(|event| !is_kind(event));
    events.len() != before
}

/// Commands whose shortcut was used this frame, in table order. The matching
/// events are consumed so nothing else reacts to them.
///
/// Paste is classified by what egui-winit emits: any `Event::Paste` is
/// `PasteUrls`, and only a real Ctrl+Shift+V key event is `PasteUrlsDirect`,
/// so one key press never produces both. Ctrl+V is deliberately not matched as
/// a key. The pasted text is dropped here; the command carries no payload.
pub fn poll(ctx: &egui::Context, command_ctx: &CommandContext, table_active: bool) -> Vec<GuiCommand> {
    // A dialog owns Enter and Esc, and an open popup closes itself on Esc
    // later in the frame; consuming the key first would break both.
    if command_ctx.modal_open || egui::Popup::is_any_open(ctx) {
        return Vec::new();
    }
    let typing = ctx.egui_wants_keyboard_input();
    let table_has_keys = table_active && !typing;
    let mut fired = Vec::new();
    ctx.input_mut(|input| {
        for binding in BINDINGS {
            let allowed = match binding.scope {
                Scope::Anywhere => !typing || works_while_typing(binding.shortcut.key),
                Scope::Table => table_has_keys,
            };
            if !allowed || !is_enabled(binding.command, command_ctx) {
                continue;
            }
            let hit = match binding.trigger {
                // Clipboard events belong to a focused text field.
                Trigger::Paste => !typing && take_event(&mut input.events, |e| matches!(e, Event::Paste(_))),
                Trigger::Copy => !typing && take_event(&mut input.events, |e| matches!(e, Event::Copy)),
                Trigger::Key => input.consume_key(binding.shortcut.modifiers(), binding.shortcut.key),
                Trigger::System => false,
            };
            if hit {
                tracing::info!("shortcut command {:?}", binding.command);
                fired.push(binding.command);
            }
        }
    });
    fired
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::Harness;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    #[test]
    fn no_duplicate_shortcuts() {
        for (i, a) in BINDINGS.iter().enumerate() {
            for b in &BINDINGS[i + 1..] {
                assert_ne!(a.shortcut, b.shortcut, "{:?} and {:?} share a shortcut", a.command, b.command);
                assert_ne!(a.command, b.command, "{:?} is bound twice", a.command);
            }
        }
    }

    #[test]
    fn most_specific_shortcuts_are_matched_first() {
        // Ctrl+Shift+V must come before any chord egui would also match for it.
        let counts: Vec<usize> = BINDINGS
            .iter()
            .filter(|b| b.trigger == Trigger::Key)
            .map(|b| b.shortcut.modifier_count())
            .collect();
        assert!(counts.windows(2).all(|pair| pair[0] >= pair[1]));
    }

    #[test]
    fn every_bound_command_has_a_menu_entry_for_the_help_list() {
        for binding in BINDINGS {
            assert!(
                crate::menu::commands().any(|(cmd, _)| cmd == binding.command),
                "{:?} is missing from the menu",
                binding.command
            );
        }
    }

    #[test]
    fn texts_match_what_menus_show() {
        assert_eq!(shortcut_text(GuiCommand::PasteUrls).as_deref(), Some("Ctrl+V"));
        assert_eq!(shortcut_text(GuiCommand::PasteUrlsDirect).as_deref(), Some("Ctrl+Shift+V"));
        assert_eq!(shortcut_text(GuiCommand::Exit).as_deref(), Some("Alt+F4"));
        assert_eq!(shortcut_text(GuiCommand::OpenSettings).as_deref(), Some("Ctrl+,"));
        assert_eq!(shortcut_text(GuiCommand::SelectNone).as_deref(), Some("Esc"));
        assert_eq!(shortcut_text(GuiCommand::Delete).as_deref(), Some("Delete"));
        assert_eq!(shortcut_text(GuiCommand::About), None);
    }

    /// A window with one text field and everything enabled. `typing` keeps the
    /// field focused.
    struct Rig {
        harness: Harness<'static, ()>,
        fired: Rc<RefCell<Vec<GuiCommand>>>,
    }

    fn rig(typing: bool) -> Rig {
        let fired = Rc::new(RefCell::new(Vec::new()));
        let sink = fired.clone();
        let focus = Rc::new(Cell::new(typing));
        let mut text = String::new();
        let command_ctx = CommandContext {
            has_targets: true,
            undo_depth: 1,
            ..CommandContext::default()
        };
        let mut harness = Harness::builder()
            .with_size(egui::vec2(300.0, 100.0))
            .build_ui(move |ui| {
                sink.borrow_mut().extend(poll(ui.ctx(), &command_ctx, true));
                let label = ui.label("Field");
                let field = ui.text_edit_singleline(&mut text).labelled_by(label.id);
                if focus.take() {
                    field.request_focus();
                }
            });
        harness.run();
        harness.run();
        Rig { harness, fired }
    }

    impl Rig {
        fn press(&mut self, modifiers: Modifiers, key: Key) {
            self.harness.key_press_modifiers(modifiers, key);
            self.harness.run();
        }

        fn fired(&self) -> Vec<GuiCommand> {
            self.fired.borrow().clone()
        }
    }

    const CTRL: Modifiers = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
    const CTRL_SHIFT: Modifiers = Modifiers { ctrl: true, command: false, shift: true, ..Modifiers::NONE };

    #[test]
    fn paste_event_means_paste_urls_and_ctrl_shift_v_means_direct() {
        let mut rig = rig(false);
        rig.harness.event(Event::Paste("https://example.com/a".to_string()));
        rig.harness.run();
        assert_eq!(rig.fired(), [GuiCommand::PasteUrls]);

        rig.press(CTRL_SHIFT, Key::V);
        assert_eq!(rig.fired(), [GuiCommand::PasteUrls, GuiCommand::PasteUrlsDirect]);
    }

    #[test]
    fn ctrl_v_key_event_alone_is_not_a_command() {
        // egui-winit never sends a key event for Ctrl+V, so a bare one must not
        // be mistaken for either paste.
        let mut rig = rig(false);
        rig.press(CTRL, Key::V);
        assert!(rig.fired().is_empty());
    }

    #[test]
    fn copy_event_means_copy_urls() {
        let mut rig = rig(false);
        rig.harness.event(Event::Copy);
        rig.harness.run();
        assert_eq!(rig.fired(), [GuiCommand::CopyUrls]);
    }

    #[test]
    fn paste_and_copy_belong_to_a_focused_text_field() {
        let mut rig = rig(true);
        rig.harness.event(Event::Paste("https://example.com/a".to_string()));
        rig.harness.event(Event::Copy);
        rig.harness.run();
        rig.press(CTRL_SHIFT, Key::V);
        assert!(rig.fired().is_empty(), "fired {:?}", rig.fired());
    }

    #[test]
    fn delete_key_ignored_while_text_field_focused() {
        let mut free = rig(false);
        free.press(Modifiers::NONE, Key::Delete);
        assert_eq!(free.fired(), [GuiCommand::Delete]);

        let mut typing = rig(true);
        typing.press(Modifiers::NONE, Key::Delete);
        typing.press(CTRL, Key::A);
        typing.press(CTRL, Key::Z);
        assert!(typing.fired().is_empty(), "fired {:?}", typing.fired());
    }

    #[test]
    fn function_keys_still_work_while_typing() {
        let mut typing = rig(true);
        typing.press(Modifiers::NONE, Key::F5);
        assert_eq!(typing.fired(), [GuiCommand::Refresh]);
    }

    #[test]
    fn disabled_commands_leave_their_keys_alone() {
        let fired = Rc::new(RefCell::new(Vec::new()));
        let sink = fired.clone();
        let mut harness = Harness::builder().build_ui(move |ui| {
            // No targets and nothing to undo.
            sink.borrow_mut().extend(poll(ui.ctx(), &CommandContext::default(), true));
        });
        harness.run();
        harness.key_press(Key::Delete);
        harness.key_press_modifiers(CTRL, Key::Z);
        harness.run();
        assert!(fired.borrow().is_empty());
    }
}
