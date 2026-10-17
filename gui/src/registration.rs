//! Decides whether a request to add URLs opens the Add Downloads dialog or
//! registers the URLs at once. Pure so the whole matrix can be tested.

// The IPC source is constructed by an input path added after the drop target.
#![allow(dead_code)]

use ggg::{
    download::enqueue::EnqueueOptions,
    ui::queue_ops::AddRequest,
    util::url_input::{expand_for_registration, parse_url_text},
};

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

/// What to do with the URLs found in a registration request.
pub enum Resolved {
    /// Open the Add Downloads dialog with `prefill` (one URL per line).
    Dialog { prefill: String, expand_checked: bool },
    /// Register at once. `truncated` says the list was cut at the per-request limit.
    Add { request: AddRequest, truncated: bool },
    /// Direct registration found nothing to add.
    NoUrl,
}

/// Applies [`plan`] to the URLs in `text`. Pure: the caller handles windows,
/// status text and dispatch. Invalid ranges stay in the list as written, as
/// in the TUI.
pub fn resolve(source: Source, text: &str, expand_setting: bool, folder_id: &str) -> Resolved {
    resolve_urls(source, parse_url_text(text), expand_setting, folder_id)
}

/// [`resolve`] for URLs that are already separated, such as the ones read from
/// dropped `.url` files.
pub fn resolve_urls(
    source: Source,
    urls: Vec<String>,
    expand_setting: bool,
    folder_id: &str,
) -> Resolved {
    match plan(source, expand_setting) {
        Plan::Dialog { expand_checked } => Resolved::Dialog {
            prefill: urls.join("
"),
            expand_checked,
        },
        Plan::Direct { .. } if urls.is_empty() => Resolved::NoUrl,
        Plan::Direct { expand } => {
            let outcome = expand_for_registration(&urls, expand);
            Resolved::Add {
                request: AddRequest {
                    urls: outcome.urls,
                    folder_id: folder_id.to_string(),
                    options: EnqueueOptions::default(),
                    immediate: false,
                    save_as: None,
                },
                truncated: outcome.truncated,
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ui::add_dialog::AddDialogModel;

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

    const CLIPBOARD: &str = "https://example.com/a[1-2].bin
https://example.com/b.bin";

    fn added(resolved: Resolved) -> (AddRequest, bool) {
        match resolved {
            Resolved::Add { request, truncated } => (request, truncated),
            _ => panic!("expected a direct add"),
        }
    }

    #[test]
    fn ctrl_v_prefills_the_dialog_with_both_lines() {
        let source = Source::PasteKey { shift: false };
        let Resolved::Dialog { prefill, expand_checked } = resolve(source, CLIPBOARD, true, "f1") else {
            panic!("expected the dialog");
        };
        assert_eq!(prefill, CLIPBOARD);
        assert!(expand_checked);
        let model = AddDialogModel::new(prefill, expand_checked, "f1".to_string());
        assert_eq!(model.preview().count, 3);
    }

    #[test]
    fn dialog_text_without_urls_opens_an_empty_dialog() {
        let source = Source::MenuPaste { shift: false };
        let Resolved::Dialog { prefill, .. } = resolve(source, "just words", true, "f1") else {
            panic!("expected the dialog");
        };
        assert_eq!(prefill, "");
    }

    #[test]
    fn direct_add_expands_ranges_when_the_setting_is_on() {
        let (request, truncated) = added(resolve(Source::PasteKey { shift: true }, CLIPBOARD, true, "f1"));
        assert_eq!(request.urls.len(), 3);
        assert_eq!(request.folder_id, "f1");
        assert!(!request.immediate);
        assert!(request.save_as.is_none());
        assert!(!truncated);
    }

    #[test]
    fn direct_add_keeps_lines_when_the_setting_is_off() {
        let (request, _) = added(resolve(Source::PasteKey { shift: true }, CLIPBOARD, false, "f1"));
        assert_eq!(
            request.urls,
            ["https://example.com/a[1-2].bin", "https://example.com/b.bin"]
        );
    }

    #[test]
    fn shift_click_on_the_menu_adds_directly() {
        let (request, _) = added(resolve(Source::MenuPaste { shift: true }, CLIPBOARD, true, "f2"));
        assert_eq!(request.urls.len(), 3);
        assert_eq!(request.folder_id, "f2");
    }

    #[test]
    fn direct_add_without_urls_reports_none_found() {
        let resolved = resolve(Source::PasteKey { shift: true }, "nothing here
", true, "f1");
        assert!(matches!(resolved, Resolved::NoUrl));
        let resolved = resolve(Source::Ipc, "", true, "f1");
        assert!(matches!(resolved, Resolved::NoUrl));
    }

    #[test]
    fn invalid_range_is_added_as_written() {
        let text = "https://example.com/a[5-1].bin";
        let (request, _) = added(resolve(Source::Ipc, text, true, "f1"));
        assert_eq!(request.urls, [text]);
    }

    #[test]
    fn direct_add_reports_truncation() {
        let text = "https://example.com/a[1-999].bin
https://example.com/b[1-999].bin";
        let (request, truncated) = added(resolve(Source::Ipc, text, true, "f1"));
        assert_eq!(request.urls.len(), ggg::util::url_input::MAX_URLS_PER_REGISTRATION);
        assert!(truncated);
    }
}
