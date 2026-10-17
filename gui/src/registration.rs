//! Decides whether a request to add URLs opens the Add Downloads dialog or
//! registers the URLs at once. Pure so the whole matrix can be tested.

// The paste, menu, drop and IPC paths that call this are added after it.
#![allow(dead_code)]

/// Where a registration request came from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Source {
    /// File > New Download: always an empty dialog.
    NewDownload,
    /// Ctrl+V (`shift: false`) or Ctrl+Shift+V (`shift: true`).
    PasteKey { shift: bool },
    /// Edit > Paste URLs; `shift` is whether Shift was held on the click.
    MenuPaste { shift: bool },
    Drop { ctrl: bool, shift: bool },
    Ipc,
}

#[derive(Debug, PartialEq, Eq)]
pub enum Plan {
    /// Open the dialog; `expand_checked` is the initial state of its expand box.
    Dialog { expand_checked: bool },
    /// Register without a dialog; `expand` says whether to expand ranges.
    Direct { expand: bool },
}

/// `expand_setting` is the user's range-expansion preference. Shift always
/// means "skip the dialog", even on a Ctrl+drop.
pub fn plan(source: Source, expand_setting: bool) -> Plan {
    let dialog = match source {
        Source::NewDownload => true,
        Source::PasteKey { shift } | Source::MenuPaste { shift } => !shift,
        Source::Drop { ctrl, shift } => ctrl && !shift,
        Source::Ipc => false,
    };
    if dialog {
        Plan::Dialog { expand_checked: expand_setting }
    } else {
        Plan::Direct { expand: expand_setting }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dialog(setting: bool) -> Plan {
        Plan::Dialog { expand_checked: setting }
    }

    fn direct(setting: bool) -> Plan {
        Plan::Direct { expand: setting }
    }

    #[test]
    fn new_download_dialog_is_empty() {
        assert_eq!(plan(Source::NewDownload, true), dialog(true));
        assert_eq!(plan(Source::NewDownload, false), dialog(false));
    }

    #[test]
    fn ctrl_v_shows_dialog() {
        assert_eq!(plan(Source::PasteKey { shift: false }, true), dialog(true));
        assert_eq!(plan(Source::MenuPaste { shift: false }, true), dialog(true));
    }

    #[test]
    fn ctrl_shift_v_is_direct() {
        assert_eq!(plan(Source::PasteKey { shift: true }, true), direct(true));
    }

    #[test]
    fn menu_shift_click_is_direct() {
        assert_eq!(plan(Source::MenuPaste { shift: true }, true), direct(true));
    }

    #[test]
    fn plain_drop_is_direct() {
        let source = Source::Drop { ctrl: false, shift: false };
        assert_eq!(plan(source, true), direct(true));
        // Shift without Ctrl is still a direct drop.
        let source = Source::Drop { ctrl: false, shift: true };
        assert_eq!(plan(source, true), direct(true));
    }

    #[test]
    fn ctrl_drop_shows_dialog() {
        let source = Source::Drop { ctrl: true, shift: false };
        assert_eq!(plan(source, true), dialog(true));
    }

    #[test]
    fn ctrl_shift_drop_is_direct() {
        let source = Source::Drop { ctrl: true, shift: true };
        assert_eq!(plan(source, true), direct(true));
    }

    #[test]
    fn ipc_is_direct() {
        assert_eq!(plan(Source::Ipc, true), direct(true));
    }

    #[test]
    fn expand_follows_setting_both_ways() {
        for setting in [true, false] {
            assert_eq!(plan(Source::NewDownload, setting), dialog(setting));
            assert_eq!(plan(Source::Ipc, setting), direct(setting));
            let source = Source::Drop { ctrl: true, shift: false };
            assert_eq!(plan(source, setting), dialog(setting));
            let source = Source::MenuPaste { shift: true };
            assert_eq!(plan(source, setting), direct(setting));
        }
    }
}
