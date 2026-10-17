//! Add Downloads dialog: the model with its validation, and the view drawn
//! into the dialog's own OS window.

use crate::{bridge::snapshot::Snapshot, i18n::Tr};
use egui::{Key, Modifiers, RichText, ScrollArea, TextEdit, TextStyle, Ui};
use fluent_bundle::FluentArgs;
use ggg::{
    cli::remote::parse_header_arg,
    download::enqueue::EnqueueOptions,
    ui::{
        item_edit::{is_valid_filename, is_valid_referrer},
        queue_ops::{AddRequest, SaveAs},
    },
    util::url_input::{MAX_URLS_PER_REGISTRATION, expand_for_registration, parse_url_text},
};
use std::path::Path;

/// Visible rows of the URL and header fields before they scroll.
const URL_ROWS: usize = 6;
const HEADER_ROWS: usize = 3;
/// Vertical room a text field adds around its rows.
const FIELD_PADDING: f32 = 8.0;

pub struct AddDialogModel {
    pub urls_text: String,
    pub expand: bool,
    pub folder_id: String,
    pub referrer: String,
    /// One `Name: value` per line.
    pub headers_text: String,
    /// Empty means the folder's location and the server's file name. Kept
    /// while the field is disabled so unchecking a range box brings it back.
    pub save_as_text: String,
    pub start_now: bool,
    /// Localized message of the last failed OK.
    pub error: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddPreview {
    pub count: usize,
    pub invalid_ranges: Vec<String>,
    pub truncated: bool,
}

impl AddPreview {
    /// One name cannot serve several downloads, so "Save as" needs exactly one.
    fn allows_save_as(&self) -> bool {
        self.count == 1
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AddDialogError {
    NoUrls,
    InvalidRange(String),
    InvalidReferrer,
    /// 1-based line of the headers field.
    InvalidHeader { line: usize },
    SaveAsEmptyName,
    SaveAsInvalidName,
    SaveAsRelativeDir,
    SaveAsInvalidDir,
}

impl AddDialogError {
    pub fn message(&self, tr: &Tr) -> String {
        let mut args = FluentArgs::new();
        let key = match self {
            Self::NoUrls => "gui-add-error-no-urls",
            Self::InvalidRange(url) => {
                args.set("url", url.as_str());
                "gui-add-invalid-range"
            }
            Self::InvalidReferrer => "gui-add-error-referrer",
            Self::InvalidHeader { line } => {
                args.set("line", *line);
                "gui-add-error-header"
            }
            Self::SaveAsEmptyName => "gui-add-save-as-empty-name",
            Self::SaveAsInvalidName => "gui-add-save-as-invalid-name",
            Self::SaveAsRelativeDir => "gui-add-save-as-relative-dir",
            Self::SaveAsInvalidDir => "gui-add-save-as-invalid-dir",
        };
        tr.t_args(key, &args)
    }
}

impl AddDialogModel {
    pub fn new(urls_text: String, expand: bool, folder_id: String) -> Self {
        Self {
            urls_text,
            expand,
            folder_id,
            referrer: String::new(),
            headers_text: String::new(),
            save_as_text: String::new(),
            start_now: false,
            error: None,
        }
    }

    /// URLs the dialog would add, after range expansion with the current
    /// `expand` value.
    fn expanded(&self) -> ggg::util::url_input::ExpandOutcome {
        expand_for_registration(&parse_url_text(&self.urls_text), self.expand)
    }

    pub fn preview(&self) -> AddPreview {
        let outcome = self.expanded();
        AddPreview {
            count: outcome.urls.len(),
            invalid_ranges: outcome.invalid_ranges,
            truncated: outcome.truncated,
        }
    }

    /// True only when exactly one URL will be added.
    pub fn save_as_enabled(&self) -> bool {
        self.preview().allows_save_as()
    }

    pub fn validate(&self) -> Result<AddRequest, AddDialogError> {
        let outcome = self.expanded();
        if outcome.urls.is_empty() {
            return Err(AddDialogError::NoUrls);
        }
        // A bad range would otherwise be queued as a literal URL with
        // brackets, which is never what the user meant here.
        if let Some(url) = outcome.invalid_ranges.first() {
            return Err(AddDialogError::InvalidRange(url.clone()));
        }

        let referrer = self.referrer.trim();
        if !is_valid_referrer(referrer) {
            return Err(AddDialogError::InvalidReferrer);
        }

        let mut headers = Vec::new();
        for (index, line) in self.headers_text.lines().enumerate() {
            if line.trim().is_empty() {
                continue;
            }
            let header = parse_header_arg(line)
                .map_err(|_| AddDialogError::InvalidHeader { line: index + 1 })?;
            headers.push(header);
        }

        let save_as = if self.save_as_enabled() {
            parse_save_as(&self.save_as_text)?
        } else {
            None
        };

        Ok(AddRequest {
            urls: outcome.urls,
            folder_id: self.folder_id.clone(),
            options: EnqueueOptions {
                referrer: (!referrer.is_empty()).then(|| referrer.to_string()),
                headers,
            },
            immediate: self.start_now,
            save_as,
        })
    }
}

/// Splits the "Save as" text into an optional directory and a file name.
/// Existence of the directory is not checked; the core creates it at start.
pub fn parse_save_as(text: &str) -> Result<Option<SaveAs>, AddDialogError> {
    let text = text.trim();
    if text.is_empty() {
        return Ok(None);
    }
    let (dir, name) = match text.rfind(['\\', '/']) {
        None => (None, text),
        Some(at) => (Some(&text[..=at]), &text[at + 1..]),
    };
    if name.is_empty() {
        return Err(AddDialogError::SaveAsEmptyName);
    }
    // The name must reach the task unchanged by `sanitize_filename`.
    if !is_valid_filename(name) {
        return Err(AddDialogError::SaveAsInvalidName);
    }
    let dir = match dir {
        None => None,
        Some(dir) => {
            if !Path::new(dir).is_absolute() {
                return Err(AddDialogError::SaveAsRelativeDir);
            }
            if dir
                .chars()
                .any(|c| c.is_control() || matches!(c, '*' | '?' | '"' | '<' | '>' | '|'))
            {
                return Err(AddDialogError::SaveAsInvalidDir);
            }
            Some(dir.into())
        }
    };
    Ok(Some(SaveAs { dir, filename: name.to_string() }))
}

/// Folder a new dialog starts on: the one being viewed, else the default.
pub fn initial_folder(target: Option<&str>, snapshot: &Snapshot) -> String {
    target
        .filter(|id| snapshot.folders.iter().any(|(folder, _)| folder == id))
        .map_or_else(|| snapshot.default_folder_id.clone(), str::to_string)
}

/// The model plus view state that is not part of the input.
pub struct AddDialog {
    pub model: AddDialogModel,
    /// The URL field takes the keyboard focus once, when the dialog opens.
    focused: bool,
}

impl AddDialog {
    pub fn new(model: AddDialogModel) -> Self {
        Self { model, focused: false }
    }
}

/// How a dialog frame ended.
pub enum AddOutcome {
    Keep,
    Submit(AddRequest),
    Cancel,
}

/// Draws the dialog. `folders` are `(id, display name)` in display order.
pub fn show(
    ui: &mut Ui,
    tr: &Tr,
    dialog: &mut AddDialog,
    folders: &[(String, String)],
) -> AddOutcome {
    // Taken before the widgets are drawn so a multiline field cannot use
    // Ctrl+Enter as a line break, and Esc cannot only leave a field.
    let (esc, ctrl_enter) = ui.ctx().input_mut(|input| {
        (
            input.consume_key(Modifiers::NONE, Key::Escape),
            input.consume_key(Modifiers::COMMAND, Key::Enter),
        )
    });
    let mut cancel = esc;
    let mut submit = ctrl_enter;

    egui::Panel::bottom("add_dialog_buttons").show(ui, |ui| {
        if let Some(error) = &dialog.model.error {
            ui.colored_label(ui.visuals().error_fg_color, error);
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.button(tr.t("gui-button-cancel")).clicked() {
                cancel = true;
            }
            if ui.button(tr.t("gui-button-ok")).clicked() {
                submit = true;
            }
        });
    });
    egui::CentralPanel::default().show(ui, |ui| {
        ScrollArea::vertical()
            .id_salt("add_dialog_form")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                submit |= form(ui, tr, dialog, folders);
            });
    });

    if cancel {
        return AddOutcome::Cancel;
    }
    if !submit {
        return AddOutcome::Keep;
    }
    match dialog.model.validate() {
        Ok(request) => AddOutcome::Submit(request),
        Err(error) => {
            dialog.model.error = Some(error.message(tr));
            // The message sits in a panel that was already drawn this frame.
            ui.ctx().request_repaint();
            AddOutcome::Keep
        }
    }
}

/// The fields. Returns true when Enter was pressed in a single-line field.
fn form(ui: &mut Ui, tr: &Tr, dialog: &mut AddDialog, folders: &[(String, String)]) -> bool {
    let AddDialog { model, focused } = dialog;
    let mut submit = false;
    let mut edited = false;
    let preview = model.preview();

    let label = ui.label(tr.t("gui-add-urls"));
    let response = multiline(ui, "add_dialog_urls", &mut model.urls_text, URL_ROWS, label.id);
    if !*focused {
        response.request_focus();
        *focused = true;
    }
    edited |= response.changed();

    edited |= ui.checkbox(&mut model.expand, tr.t("gui-add-expand")).changed();
    // The preview is recomputed after the checkbox so the line never lags a frame.
    let preview = if edited { model.preview() } else { preview };
    let mut args = FluentArgs::new();
    args.set("count", preview.count);
    ui.label(tr.t_args("gui-add-preview", &args));
    let warn = ui.visuals().warn_fg_color;
    if let Some(url) = preview.invalid_ranges.first() {
        let mut args = FluentArgs::new();
        args.set("url", url.as_str());
        ui.add(egui::Label::new(RichText::new(tr.t_args("gui-add-invalid-range", &args)).color(warn)).wrap());
    }
    if preview.truncated {
        let mut args = FluentArgs::new();
        args.set("max", MAX_URLS_PER_REGISTRATION);
        ui.add(egui::Label::new(RichText::new(tr.t_args("gui-add-truncated", &args)).color(warn)).wrap());
    }

    let label = ui.label(tr.t("gui-add-folder"));
    let selected = folders
        .iter()
        .find(|(id, _)| *id == model.folder_id)
        .map_or_else(|| model.folder_id.clone(), |(_, name)| name.clone());
    egui::ComboBox::from_id_salt("add_dialog_folder")
        .selected_text(selected)
        .width(ui.available_width())
        .show_ui(ui, |ui| {
            for (id, name) in folders {
                ui.selectable_value(&mut model.folder_id, id.clone(), name);
            }
        })
        .response
        .labelled_by(label.id);

    let label = ui.label(tr.t("gui-add-referrer"));
    let response = ui
        .add(
            TextEdit::singleline(&mut model.referrer)
                .id_salt("add_dialog_referrer")
                .hint_text(tr.t("gui-add-referrer-hint"))
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label.id);
    submit |= entered(ui, &response);
    edited |= response.changed();

    let label = ui.label(tr.t("gui-add-headers"));
    let response = multiline(ui, "add_dialog_headers", &mut model.headers_text, HEADER_ROWS, label.id);
    edited |= response.changed();

    let save_as_enabled = preview.allows_save_as();
    let label = ui.label(tr.t("gui-add-save-as"));
    let mut response = ui
        .add_enabled(
            save_as_enabled,
            TextEdit::singleline(&mut model.save_as_text)
                .id_salt("add_dialog_save_as")
                .hint_text(tr.t("gui-add-save-as-hint"))
                .desired_width(f32::INFINITY),
        )
        .labelled_by(label.id);
    if !save_as_enabled {
        let mut args = FluentArgs::new();
        args.set("count", preview.count);
        response = response.on_disabled_hover_text(tr.t_args("gui-add-save-as-disabled", &args));
    }
    submit |= entered(ui, &response);
    edited |= response.changed();

    edited |= ui.checkbox(&mut model.start_now, tr.t("gui-add-start-now")).changed();

    if edited && model.error.take().is_some() {
        // The message is in a panel that was already drawn this frame.
        ui.ctx().request_repaint();
    }
    submit
}

/// A multiline field that scrolls after `rows` rows instead of growing.
fn multiline(
    ui: &mut Ui,
    id_salt: &'static str,
    text: &mut String,
    rows: usize,
    label: egui::Id,
) -> egui::Response {
    let max_height = ui.text_style_height(&TextStyle::Body) * rows as f32 + FIELD_PADDING;
    ScrollArea::vertical()
        .id_salt(id_salt)
        .max_height(max_height)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.add(
                TextEdit::multiline(text)
                    .id_salt((id_salt, "text"))
                    .desired_rows(rows)
                    .desired_width(f32::INFINITY),
            )
            .labelled_by(label)
        })
        .inner
}

/// Enter leaves a single-line field with the focus lost.
fn entered(ui: &Ui, response: &egui::Response) -> bool {
    response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter))
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
    };

    fn model(urls: &str, expand: bool) -> AddDialogModel {
        AddDialogModel::new(urls.to_string(), expand, "f1".to_string())
    }

    fn error(model: &AddDialogModel) -> Option<AddDialogError> {
        model.validate().err()
    }

    #[test]
    fn preview_counts_expanded_range() {
        let preview = model("https://example.com/a[1-3].jpg", true).preview();
        assert_eq!(preview.count, 3);
        assert!(preview.invalid_ranges.is_empty());
        assert!(!preview.truncated);
    }

    #[test]
    fn preview_without_expand_counts_lines() {
        let m = model("https://example.com/a[1-3].jpg\nhttps://example.com/b", false);
        assert_eq!(m.preview().count, 2);
    }

    #[test]
    fn invalid_range_blocks_ok() {
        let m = model("https://example.com/a[5-1].jpg", true);
        assert_eq!(m.preview().invalid_ranges.len(), 1);
        assert_eq!(
            error(&m),
            Some(AddDialogError::InvalidRange("https://example.com/a[5-1].jpg".to_string()))
        );
        // Without expansion the same text is an ordinary URL.
        assert!(model("https://example.com/a[5-1].jpg", false).validate().is_ok());
    }

    #[test]
    fn empty_text_has_no_urls() {
        assert_eq!(error(&model("  \nnot a url", true)), Some(AddDialogError::NoUrls));
    }

    #[test]
    fn invalid_referrer_rejected() {
        let mut m = model("https://example.com/a", false);
        m.referrer = "https://ref.example/\u{7}".to_string();
        assert_eq!(error(&m), Some(AddDialogError::InvalidReferrer));
    }

    #[test]
    fn bad_header_line_reports_line_number() {
        let mut m = model("https://example.com/a", false);
        m.headers_text = "X-A: 1\n\nno colon here".to_string();
        assert_eq!(error(&m), Some(AddDialogError::InvalidHeader { line: 3 }));
        m.headers_text = "X-A: bad\u{1}value".to_string();
        assert_eq!(error(&m), Some(AddDialogError::InvalidHeader { line: 1 }));
    }

    #[test]
    fn valid_input_builds_add_request() {
        let mut m = model("https://example.com/a[1-2].bin\nhttps://example.com/b", true);
        m.folder_id = "f2".to_string();
        m.referrer = "  https://ref.example/  ".to_string();
        m.headers_text = "X-A: 1\n\nX-B:  two \r\n".to_string();
        m.start_now = true;
        m.save_as_text = "ignored-for-three-urls.bin".to_string();

        let request = m.validate().ok().expect("valid input");
        assert_eq!(
            request.urls,
            vec![
                "https://example.com/a1.bin",
                "https://example.com/a2.bin",
                "https://example.com/b",
            ]
        );
        assert_eq!(request.folder_id, "f2");
        assert_eq!(request.options.referrer.as_deref(), Some("https://ref.example/"));
        assert_eq!(
            request.options.headers,
            vec![("X-A".to_string(), "1".to_string()), ("X-B".to_string(), "two".to_string())]
        );
        assert!(request.immediate);
        assert_eq!(request.save_as, None);
    }

    #[test]
    fn empty_referrer_and_headers_leave_options_empty() {
        let request = model("https://example.com/a", false).validate().ok().unwrap();
        assert_eq!(request.options.referrer, None);
        assert!(request.options.headers.is_empty());
        assert!(!request.immediate);
        assert_eq!(request.save_as, None);
    }

    #[test]
    fn save_as_enabled_only_for_one_url() {
        assert!(model("https://example.com/a", false).save_as_enabled());
        assert!(!model("https://example.com/a\nhttps://example.com/b", false).save_as_enabled());
        assert!(!model("https://example.com/a[1-3].jpg", true).save_as_enabled());
        assert!(model("https://example.com/a[1-3].jpg", false).save_as_enabled());
        assert!(!model("", false).save_as_enabled());
    }

    #[test]
    fn save_as_ignored_when_disabled() {
        let mut m = model("https://example.com/a\nhttps://example.com/b", false);
        m.save_as_text = "a?b".to_string();
        let request = m.validate().ok().expect("an invalid save-as is ignored");
        assert_eq!(request.urls.len(), 2);
        assert_eq!(request.save_as, None);
    }

    #[test]
    fn save_as_error_blocks_ok_for_one_url() {
        let mut m = model("https://example.com/a", false);
        m.save_as_text = "a?b".to_string();
        assert_eq!(error(&m), Some(AddDialogError::SaveAsInvalidName));
    }

    #[test]
    fn save_as_empty_text_is_none() {
        assert_eq!(parse_save_as(""), Ok(None));
        assert_eq!(parse_save_as("   "), Ok(None));
    }

    #[test]
    fn save_as_bare_name_uses_folder_dir() {
        let save_as = parse_save_as(" mine.bin ").unwrap().unwrap();
        assert_eq!(save_as, SaveAs { dir: None, filename: "mine.bin".to_string() });
    }

    /// An absolute directory with a trailing separator, built at run time so
    /// the tests do not depend on any particular drive or mount point.
    fn absolute_dir(name: &str) -> String {
        let mut dir = std::env::temp_dir().join(name).to_string_lossy().into_owned();
        dir.push(std::path::MAIN_SEPARATOR);
        dir
    }

    #[test]
    fn save_as_full_path_splits_dir_and_name() {
        let dir = absolute_dir("dl");
        let save_as = parse_save_as(&format!("{dir}mine.bin")).unwrap().unwrap();
        assert_eq!(save_as.dir.as_deref(), Some(Path::new(&dir)));
        assert_eq!(save_as.filename, "mine.bin");

        // Both separators split the text.
        let dir = absolute_dir("dl").replace('\\', "/");
        let save_as = parse_save_as(&format!("{dir}sub/mine.bin")).unwrap().unwrap();
        assert_eq!(save_as.dir.as_deref(), Some(Path::new(&format!("{dir}sub/"))));
    }

    #[cfg(windows)]
    #[test]
    fn save_as_accepts_a_unc_dir() {
        let save_as = parse_save_as("\\\\server\\share\\mine.bin").unwrap().unwrap();
        assert_eq!(save_as.filename, "mine.bin");
    }

    #[test]
    fn save_as_rejects_invalid_dir_chars() {
        let dir = absolute_dir("dl");
        assert_eq!(
            parse_save_as(&format!("{dir}d*l/mine.bin")),
            Err(AddDialogError::SaveAsInvalidDir)
        );
        assert_eq!(
            parse_save_as(&format!("{dir}d\u{7}l/mine.bin")),
            Err(AddDialogError::SaveAsInvalidDir)
        );
    }

    #[test]
    fn save_as_trailing_separator_is_empty_name() {
        let dir = absolute_dir("dl");
        assert_eq!(parse_save_as(&dir), Err(AddDialogError::SaveAsEmptyName));
        assert_eq!(parse_save_as("dl/"), Err(AddDialogError::SaveAsEmptyName));
    }

    #[test]
    fn save_as_rejects_invalid_name_chars() {
        for bad in ["a?b.bin", "CON", "name.", "a:b", "a<b", "..", &"x".repeat(300)] {
            assert_eq!(
                parse_save_as(bad),
                Err(AddDialogError::SaveAsInvalidName),
                "{bad:?} must be rejected"
            );
        }
    }

    #[test]
    fn save_as_rejects_relative_dir() {
        assert_eq!(parse_save_as("sub\\mine.bin"), Err(AddDialogError::SaveAsRelativeDir));
        assert_eq!(parse_save_as("sub/mine.bin"), Err(AddDialogError::SaveAsRelativeDir));
    }

    #[test]
    fn initial_folder_prefers_a_known_target() {
        let snapshot = Snapshot {
            folders: vec![
                ("f1".to_string(), "One".to_string()),
                ("f2".to_string(), "Two".to_string()),
            ],
            default_folder_id: "f1".to_string(),
            ..Snapshot::default()
        };
        assert_eq!(initial_folder(Some("f2"), &snapshot), "f2");
        assert_eq!(initial_folder(Some("gone"), &snapshot), "f1");
        assert_eq!(initial_folder(None, &snapshot), "f1");
    }

    // The view, driven through kittest.

    struct Rig {
        dialog: Rc<RefCell<AddDialog>>,
        submitted: Rc<RefCell<Vec<AddRequest>>>,
        cancelled: Rc<Cell<u32>>,
    }

    fn open_dialog(urls: &str, expand: bool) -> (Harness<'static, ()>, Rig) {
        let rig = Rig {
            dialog: Rc::new(RefCell::new(AddDialog::new(model(urls, expand)))),
            submitted: Rc::default(),
            cancelled: Rc::default(),
        };
        let folders = vec![
            ("f1".to_string(), "Downloads".to_string()),
            ("f2".to_string(), "Music".to_string()),
        ];
        let tr = Tr::load("en-US");
        let (dialog, submitted, cancelled) =
            (rig.dialog.clone(), rig.submitted.clone(), rig.cancelled.clone());
        let harness = Harness::builder()
            .with_size(egui::vec2(560.0, 440.0))
            .build_ui(move |ui| match show(ui, &tr, &mut dialog.borrow_mut(), &folders) {
                AddOutcome::Keep => {}
                AddOutcome::Submit(request) => submitted.borrow_mut().push(request),
                AddOutcome::Cancel => cancelled.set(cancelled.get() + 1),
            });
        (harness, rig)
    }

    // Multiline fields come first in `field`, then the single-line ones.
    const URLS: usize = 0;
    const HEADERS: usize = 1;
    const REFERRER: usize = 2;
    const SAVE_AS: usize = 3;

    fn field<'h>(
        harness: &'h Harness<'static, ()>,
        index: usize,
    ) -> egui_kittest::Node<'h> {
        harness
            .get_all_by_role(egui::accesskit::Role::MultilineTextInput)
            .chain(harness.get_all_by_role(egui::accesskit::Role::TextInput))
            .nth(index)
            .expect("text field")
    }

    #[test]
    fn urls_field_has_focus_on_open() {
        let (mut harness, _rig) = open_dialog("", false);
        harness.run();
        assert!(field(&harness, URLS).is_focused());
    }

    #[test]
    fn paste_inside_the_url_field_goes_into_the_field_and_opens_nothing() {
        let (mut harness, rig) = open_dialog("", false);
        harness.run();
        assert!(field(&harness, URLS).is_focused());
        harness.event(egui::Event::Paste("https://example.com/a[1-2].bin
https://example.com/b.bin".to_string()));
        harness.run();
        // A dialog frame is the only consumer: no second dialog, no submit.
        assert_eq!(
            rig.dialog.borrow().model.urls_text,
            "https://example.com/a[1-2].bin
https://example.com/b.bin"
        );
        assert!(rig.submitted.borrow().is_empty());
        assert_eq!(rig.cancelled.get(), 0);
    }

    #[test]
    fn ok_submits_the_request() {
        let (mut harness, rig) = open_dialog("https://example.com/a.bin", false);
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        let submitted = rig.submitted.borrow();
        assert_eq!(submitted.len(), 1);
        assert_eq!(submitted[0].urls, vec!["https://example.com/a.bin"]);
        assert_eq!(submitted[0].folder_id, "f1");
        assert_eq!(rig.cancelled.get(), 0);
    }

    #[test]
    fn ok_without_urls_shows_the_error_and_stays() {
        let (mut harness, rig) = open_dialog("", false);
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        harness.get_by_label("Enter at least one URL.");
        assert!(rig.submitted.borrow().is_empty());
        assert_eq!(rig.cancelled.get(), 0);
    }

    #[test]
    fn editing_clears_the_error() {
        let (mut harness, rig) = open_dialog("", false);
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        harness.get_by_label("Enter at least one URL.");
        field(&harness, URLS).click();
        harness.run();
        field(&harness, URLS).type_text("https://example.com/a");
        harness.run();
        assert!(harness.query_by_label("Enter at least one URL.").is_none());
        assert!(rig.dialog.borrow().model.error.is_none());
    }

    #[test]
    fn cancel_button_and_esc_cancel() {
        let (mut harness, rig) = open_dialog("https://example.com/a", false);
        harness.run();
        harness.get_by_label("Cancel").click();
        harness.run();
        assert_eq!(rig.cancelled.get(), 1);

        let (mut harness, rig) = open_dialog("https://example.com/a", false);
        harness.run();
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert_eq!(rig.cancelled.get(), 1);
        assert!(rig.submitted.borrow().is_empty());
    }

    #[test]
    fn ctrl_enter_submits_from_the_url_field() {
        let (mut harness, rig) = open_dialog("https://example.com/a", false);
        harness.run();
        harness.key_press_modifiers(Modifiers::COMMAND, Key::Enter);
        harness.run();
        assert_eq!(rig.submitted.borrow().len(), 1);
        // The multiline field must not have taken the key as a line break.
        assert_eq!(rig.dialog.borrow().model.urls_text, "https://example.com/a");
    }

    #[test]
    fn enter_in_the_referrer_field_submits() {
        let (mut harness, rig) = open_dialog("https://example.com/a", false);
        harness.run();
        field(&harness, REFERRER).click();
        harness.run();
        field(&harness, REFERRER).type_text("https://ref.example/");
        harness.run();
        harness.key_press(Key::Enter);
        harness.run();
        let submitted = rig.submitted.borrow();
        assert_eq!(submitted.len(), 1);
        assert_eq!(
            submitted[0].options.referrer.as_deref(),
            Some("https://ref.example/")
        );
    }

    #[test]
    fn enter_in_the_urls_field_adds_a_line() {
        let (mut harness, rig) = open_dialog("https://example.com/a", false);
        harness.run();
        harness.key_press(Key::Enter);
        harness.run();
        assert!(rig.submitted.borrow().is_empty());
        assert!(rig.dialog.borrow().model.urls_text.contains('\n'));
    }

    #[test]
    fn preview_line_follows_the_expand_checkbox() {
        let (mut harness, _rig) = open_dialog("https://example.com/a[1-3].jpg", true);
        harness.run();
        harness.get_by_label("3 URLs will be added");
        harness.get_by_label("Expand URL ranges such as [1-10]").click();
        harness.run();
        harness.get_by_label("1 URL will be added");
    }

    #[test]
    fn save_as_is_disabled_unless_one_url_will_be_added() {
        let (mut harness, _rig) = open_dialog("https://example.com/a[1-3].jpg", true);
        harness.run();
        let save_as = field(&harness, SAVE_AS);
        assert!(
            egui_kittest::kittest::NodeT::accesskit_node(&save_as).is_disabled(),
            "three URLs must grey out Save as"
        );
        harness.get_by_label("Expand URL ranges such as [1-10]").click();
        harness.run();
        let save_as = field(&harness, SAVE_AS);
        assert!(!egui_kittest::kittest::NodeT::accesskit_node(&save_as).is_disabled());
    }

    #[test]
    fn headers_field_reaches_the_request() {
        let (mut harness, rig) = open_dialog("https://example.com/a", false);
        harness.run();
        field(&harness, HEADERS).click();
        harness.run();
        field(&harness, HEADERS).type_text("X-Token: abc");
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        let submitted = rig.submitted.borrow();
        assert_eq!(submitted.len(), 1);
        assert_eq!(
            submitted[0].options.headers,
            vec![("X-Token".to_string(), "abc".to_string())]
        );
    }

    #[test]
    fn save_as_reaches_the_request_for_one_url() {
        let (mut harness, rig) = open_dialog("https://example.com/a", false);
        harness.run();
        field(&harness, SAVE_AS).click();
        harness.run();
        field(&harness, SAVE_AS).type_text("mine.bin");
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        let submitted = rig.submitted.borrow();
        assert_eq!(submitted.len(), 1);
        assert_eq!(
            submitted[0].save_as,
            Some(SaveAs { dir: None, filename: "mine.bin".to_string() })
        );
    }

    #[test]
    fn invalid_save_as_shows_its_message() {
        let (mut harness, rig) = open_dialog("https://example.com/a", false);
        rig.dialog.borrow_mut().model.save_as_text = "a?b".to_string();
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        harness.get_by_label_contains("Enter a valid file name");
        assert!(rig.submitted.borrow().is_empty());
    }

    #[test]
    fn invalid_range_shows_its_message() {
        let (mut harness, rig) = open_dialog("https://example.com/a[5-1].jpg", true);
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        assert!(rig.submitted.borrow().is_empty());
        assert!(rig.dialog.borrow().model.error.as_deref().is_some_and(|e| e.contains("a[5-1]")));
    }

    #[test]
    fn folder_combo_shows_and_changes_the_folder() {
        let (mut harness, rig) = open_dialog("https://example.com/a", false);
        harness.run();
        let combo = harness.get_by_role(egui::accesskit::Role::ComboBox);
        assert_eq!(egui_kittest::kittest::NodeT::accesskit_node(&combo).value().as_deref(), Some("Downloads"));
        combo.click();
        harness.run();
        harness.get_by_label("Music").click();
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        assert_eq!(rig.submitted.borrow()[0].folder_id, "f2");
    }
}
