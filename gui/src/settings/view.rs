//! Settings window: pages drawn into the window's own OS window, and the
//! state that is not part of the editing model (pending actions, notices).

use crate::{
    bridge::actions::{Action, ActionKind, ActionResult, status_text},
    i18n::Tr,
    settings::model::{SettingsModel, SettingsPage},
};
use egui::{
    Align, Button, ComboBox, DragValue, Grid, Id, Key, Layout, Modifiers, RichText, ScrollArea,
    TextEdit, Ui,
};
use fluent_bundle::FluentArgs;
use ggg::{
    app::config::{Config, FolderConfig, ReferrerPolicy, ReferrerPolicyKind},
    ui::{
        commands::Command,
        settings_fields::{ApplicationSettingsField, SettingsField},
    },
    util::{redact::display_header_value, script_files::scan_script_files},
};
use std::{collections::HashMap, fmt::Debug, hash::Hash, path::PathBuf};

/// Upper bound of the counters; the core has no limit of its own, this only
/// keeps a drag from reaching absurd values.
const COUNT_MAX: usize = 9999;
const PAGE_LIST_WIDTH: f32 = 140.0;
/// Rows of the folder list before it scrolls.
const FOLDER_LIST_HEIGHT: f32 = 110.0;
const CONFIRM_WIDTH: f32 = 380.0;

pub struct SettingsWindow {
    pub model: SettingsModel,
    /// Script files found in the scripts directory; scanned when the window
    /// opens and after a reload, not on every frame.
    script_files: Vec<String>,
    /// The action whose result is still outstanding. While it is set, nothing
    /// else is sent, so a double click cannot send the same difference twice.
    pending: Option<ActionKind>,
    /// OK was clicked: close once the pending apply succeeded.
    close_on_success: bool,
    /// Folder the user is asked to confirm removing.
    confirm_remove: Option<String>,
    /// Result of the last immediate action (folder add/remove, reloads).
    notice: Option<Notice>,
}

struct Notice {
    text: String,
    error: bool,
}

/// What the app has to do after a frame.
#[derive(Default)]
pub struct SettingsOutcome {
    pub actions: Vec<Action>,
    /// Close the window, discarding what was not applied.
    pub close: bool,
    /// New value of the GUI-only setting that Apply wrote; the app saves it.
    pub expand_ranges: Option<bool>,
}

impl SettingsWindow {
    pub fn new(model: SettingsModel, script_files: Vec<String>) -> Self {
        Self {
            model,
            script_files,
            pending: None,
            close_on_success: false,
            confirm_remove: None,
            notice: None,
        }
    }

    pub fn busy(&self) -> bool {
        self.pending.is_some()
    }

    /// Takes the result of an action started by this window. Returns true
    /// when the window should close (OK after a successful apply).
    pub fn on_action_result(&mut self, tr: &Tr, result: &ActionResult) -> bool {
        match result {
            ActionResult::Settings { error, config, .. } => {
                if self.pending == Some(ActionKind::ApplySettings) {
                    self.pending = None;
                }
                let error = error.as_ref().map(|(index, e)| (*index, tr.command_error(e)));
                let close = std::mem::take(&mut self.close_on_success);
                if self.model.on_apply_result(error, config) {
                    self.notice = None;
                    return close;
                }
                self.show_failure();
            }
            ActionResult::Command { action, result: outcome, config } => {
                if self.pending == Some(*action) {
                    self.pending = None;
                }
                let error = outcome.is_err();
                if !error {
                    match action {
                        ActionKind::CreateFolder => self.model.on_folder_created(config),
                        ActionKind::DeleteFolder => self.model.on_folder_deleted(config),
                        ActionKind::ReloadConfig => {
                            self.model.on_config_reloaded(config);
                            self.rescan_scripts();
                        }
                        ActionKind::ReloadScripts => self.rescan_scripts(),
                        _ => return false,
                    }
                }
                if matches!(
                    action,
                    ActionKind::CreateFolder
                        | ActionKind::DeleteFolder
                        | ActionKind::ReloadConfig
                        | ActionKind::ReloadScripts
                ) {
                    self.notice = Some(Notice { text: status_text(tr, result), error });
                }
            }
            _ => {}
        }
        false
    }

    fn rescan_scripts(&mut self) {
        self.script_files = scan_script_files(&self.model.original.scripts.directory);
    }

    /// Shows the page and folder of the field that was refused, so the error
    /// is not hidden behind another page.
    fn show_failure(&mut self) {
        let Some(command) = &self.model.failed_command else {
            return;
        };
        let Some(target) = target_of(command) else {
            return;
        };
        let (page, folder) = target.location();
        let folder = folder.map(str::to_string);
        self.model.page = page;
        if let Some(folder) = folder {
            self.model.open_folder(&folder);
        }
    }

    /// Starts Apply (and the close that OK adds when it succeeds).
    fn start_apply(&mut self, close_after: bool, out: &mut SettingsOutcome) {
        if self.model.gui_changed() {
            out.expand_ranges = Some(self.model.gui_draft.expand_ranges_on_direct_add);
            self.model.mark_gui_applied();
        }
        let commands = self.model.begin_apply();
        if commands.is_empty() {
            out.close |= close_after;
            return;
        }
        self.pending = Some(ActionKind::ApplySettings);
        self.close_on_success = close_after;
        out.actions.push(Action::ApplySettings { commands });
    }

    fn start(&mut self, action: Action, out: &mut SettingsOutcome) {
        self.pending = Some(action.kind());
        self.notice = None;
        out.actions.push(action);
    }
}

/// The setting a command edits, to mark it when the command failed.
enum Target<'a> {
    App(ApplicationSettingsField),
    ScriptFile(&'a str),
    Folder(&'a str, SettingsField),
    FolderScriptFile(&'a str, &'a str),
}

impl Target<'_> {
    fn location(&self) -> (SettingsPage, Option<&str>) {
        use ApplicationSettingsField as F;
        match self {
            Target::App(F::Language | F::SkipDownloadPreview | F::AutoLaunchDnd) => {
                (SettingsPage::General, None)
            }
            Target::App(
                F::DefaultDirectory
                | F::MaxConcurrent
                | F::MaxConcurrentPerFolder
                | F::MaxActiveFolders
                | F::RetryCount,
            ) => (SettingsPage::Downloads, None),
            Target::App(F::UserAgent | F::ReferrerPolicy | F::MaxRedirects) => {
                (SettingsPage::Connection, None)
            }
            Target::App(F::ScriptsEnabled) | Target::ScriptFile(_) => (SettingsPage::Scripts, None),
            Target::Folder(folder, _) | Target::FolderScriptFile(folder, _) => {
                (SettingsPage::Folders, Some(folder))
            }
        }
    }
}

fn target_of(command: &Command) -> Option<Target<'_>> {
    use ApplicationSettingsField as F;
    use Command as C;
    use SettingsField as S;
    Some(match command {
        C::UpdateLanguage { .. } => Target::App(F::Language),
        C::UpdateSkipDownloadPreview { .. } => Target::App(F::SkipDownloadPreview),
        C::UpdateAutoLaunchDnd { .. } => Target::App(F::AutoLaunchDnd),
        C::UpdateDefaultDirectory { .. } => Target::App(F::DefaultDirectory),
        C::UpdateMaxConcurrent { .. } => Target::App(F::MaxConcurrent),
        C::UpdateMaxConcurrentPerFolder { .. } => Target::App(F::MaxConcurrentPerFolder),
        C::UpdateMaxActiveFolders { .. } => Target::App(F::MaxActiveFolders),
        C::UpdateRetryCount { .. } => Target::App(F::RetryCount),
        C::UpdateUserAgent { .. } => Target::App(F::UserAgent),
        C::UpdateReferrerPolicy { .. } => Target::App(F::ReferrerPolicy),
        C::UpdateMaxRedirects { .. } => Target::App(F::MaxRedirects),
        C::UpdateScriptsEnabled { .. } => Target::App(F::ScriptsEnabled),
        C::SetScriptFileEnabled { filename, .. } => Target::ScriptFile(filename),
        C::UpdateFolderName { folder_id, .. } => Target::Folder(folder_id, S::FolderName),
        C::UpdateFolderSavePath { folder_id, .. } => Target::Folder(folder_id, S::FolderSavePath),
        C::UpdateFolderAutoDateDirectory { folder_id, .. } => {
            Target::Folder(folder_id, S::FolderAutoDate)
        }
        C::UpdateFolderAutoStartDownloads { folder_id, .. } => {
            Target::Folder(folder_id, S::FolderAutoStart)
        }
        C::UpdateFolderPreventDuplicateUrl { folder_id, .. } => {
            Target::Folder(folder_id, S::FolderPreventDuplicateUrl)
        }
        C::UpdateFolderScriptsEnabled { folder_id, .. } => Target::Folder(folder_id, S::FolderScripts),
        C::UpdateFolderMaxConcurrent { folder_id, .. } => {
            Target::Folder(folder_id, S::FolderMaxConcurrent)
        }
        C::UpdateFolderUserAgent { folder_id, .. } => Target::Folder(folder_id, S::FolderUserAgent),
        C::UpdateFolderReferrerPolicy { folder_id, .. } => {
            Target::Folder(folder_id, S::FolderReferrerPolicy)
        }
        C::SetFolderScriptFileOverride { folder_id, filename, .. } => {
            Target::FolderScriptFile(folder_id, filename)
        }
        _ => return None,
    })
}

/// The refused command with its message, copied out of the model so the pages
/// can borrow the draft mutably while they mark the failing field.
struct Failure {
    command: Option<Command>,
    message: String,
}

impl Failure {
    fn of(model: &SettingsModel) -> Self {
        match (&model.failed_command, &model.error) {
            (Some(command), Some((_, message))) => {
                Self { command: Some(command.clone()), message: message.clone() }
            }
            _ => Self { command: None, message: String::new() },
        }
    }

    fn target(&self) -> Option<Target<'_>> {
        self.command.as_ref().and_then(target_of)
    }

    fn app(&self, field: ApplicationSettingsField) -> Option<&str> {
        match self.target() {
            Some(Target::App(f)) if f == field => Some(&self.message),
            _ => None,
        }
    }

    fn script_file(&self, name: &str) -> Option<&str> {
        match self.target() {
            Some(Target::ScriptFile(f)) if f == name => Some(&self.message),
            _ => None,
        }
    }

    fn folder(&self, folder: &str, field: SettingsField) -> Option<&str> {
        match self.target() {
            Some(Target::Folder(id, f)) if id == folder && f == field => Some(&self.message),
            _ => None,
        }
    }

    fn folder_script_file(&self, folder: &str, name: &str) -> Option<&str> {
        match self.target() {
            Some(Target::FolderScriptFile(id, file)) if id == folder && file == name => {
                Some(&self.message)
            }
            _ => None,
        }
    }
}

fn page_title(tr: &Tr, page: SettingsPage) -> String {
    tr.t(match page {
        SettingsPage::General => "settings-tab-general",
        SettingsPage::Downloads => "settings-tab-downloads",
        SettingsPage::Connection => "gui-settings-page-connection",
        SettingsPage::Scripts => "settings-tab-scripts",
        SettingsPage::Folders => "settings-tab-folders",
    })
}

pub fn show(ui: &mut Ui, tr: &Tr, win: &mut SettingsWindow) -> SettingsOutcome {
    let mut out = SettingsOutcome::default();

    // Taken before the widgets so that Esc closes the window even from a
    // text field; the removal confirmation handles its own Esc.
    let mut cancel = win.confirm_remove.is_none()
        && ui.ctx().input_mut(|input| input.consume_key(Modifiers::NONE, Key::Escape));
    let (mut ok, mut apply) = (false, false);

    let idle = !win.busy();
    let dirty = win.model.is_dirty();
    egui::Panel::bottom("settings_buttons").show(ui, |ui| {
        banners(ui, tr, win);
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            apply = ui
                .add_enabled(dirty && idle, Button::new(tr.t("gui-button-apply")))
                .clicked();
            cancel |= ui.button(tr.t("gui-button-cancel")).clicked();
            ok = ui.add_enabled(idle, Button::new(tr.t("gui-button-ok"))).clicked();
        });
    });

    egui::Panel::left("settings_pages")
        .resizable(false)
        .exact_size(PAGE_LIST_WIDTH)
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                for page in SettingsPage::all() {
                    ui.selectable_value(&mut win.model.page, page, page_title(tr, page));
                }
            });
        });

    egui::CentralPanel::default().show(ui, |ui| {
        ScrollArea::vertical()
            .id_salt("settings_page")
            .max_height(ui.available_height())
            .auto_shrink([false, false])
            .show(ui, |ui| page(ui, tr, win, &mut out));
    });

    confirm_remove(ui.ctx(), tr, win, &mut out);

    if cancel {
        out.close = true;
        out.actions.clear();
        return out;
    }
    if (ok || apply) && !win.busy() {
        win.start_apply(ok, &mut out);
    }
    out
}

fn banners(ui: &mut Ui, tr: &Tr, win: &SettingsWindow) {
    let error_color = ui.visuals().error_fg_color;
    if let Some((_, message)) = &win.model.error {
        let mut args = FluentArgs::new();
        args.set("error", message.as_str());
        let text = tr.t_args("gui-status-settings-failed", &args);
        ui.add(egui::Label::new(RichText::new(text).color(error_color)).wrap());
    }
    if let Some(notice) = &win.notice {
        let mut text = RichText::new(&notice.text);
        if notice.error {
            text = text.color(error_color);
        }
        ui.add(egui::Label::new(text).wrap());
    }
    if win.model.restart_needed {
        ui.add(egui::Label::new(tr.t("gui-settings-restart-banner")).wrap());
    }
}

fn page(ui: &mut Ui, tr: &Tr, win: &mut SettingsWindow, out: &mut SettingsOutcome) {
    let failure = Failure::of(&win.model);
    match win.model.page {
        SettingsPage::General => general_page(ui, tr, win, &failure, out),
        SettingsPage::Downloads => downloads_page(ui, tr, &mut win.model.draft, &failure),
        SettingsPage::Connection => connection_page(ui, tr, &mut win.model.draft, &failure),
        SettingsPage::Scripts => scripts_page(ui, tr, win, &failure, out),
        SettingsPage::Folders => folders_page(ui, tr, win, &failure, out),
    }
}

// Rows

/// One row of a page: the label (with its description as a tooltip and
/// short tags such as "(restart required)") and the control. `add` receives
/// the label's id so a control without text can name itself after it.
fn row(
    ui: &mut Ui,
    label: &str,
    tooltip: &str,
    tags: &[String],
    error: Option<&str>,
    add: impl FnOnce(&mut Ui, Id),
) {
    let mut label_id = Id::NULL;
    ui.horizontal(|ui| {
        let response = ui.label(label);
        label_id = response.id;
        if !tooltip.is_empty() {
            response.on_hover_text(tooltip);
        }
        for tag in tags {
            ui.label(RichText::new(tag).weak());
        }
    });
    ui.vertical(|ui| {
        add(ui, label_id);
        if let Some(error) = error {
            let color = ui.visuals().error_fg_color;
            ui.add(egui::Label::new(RichText::new(error).color(color)).wrap());
        }
    });
    ui.end_row();
}

fn app_row(
    ui: &mut Ui,
    tr: &Tr,
    failure: &Failure,
    field: ApplicationSettingsField,
    add: impl FnOnce(&mut Ui, Id),
) {
    let mut tooltip = tr.t(field.description_key());
    let mut tags = Vec::new();
    // The GUI never shows a preview and never launches ggg-dnd; both
    // settings exist because the TUI reads the same file.
    let tui_note = match field {
        ApplicationSettingsField::SkipDownloadPreview => Some("gui-settings-tui-only-preview"),
        ApplicationSettingsField::AutoLaunchDnd => Some("gui-settings-tui-only-dnd"),
        _ => None,
    };
    if let Some(note) = tui_note {
        tags.push(tr.t("gui-settings-tui-only-suffix"));
        tooltip.push('\n');
        tooltip.push_str(&tr.t(note));
    }
    if field.requires_restart() {
        tags.push(tr.t("settings-restart-marker"));
    }
    row(ui, &tr.t(field.label_key()), &tooltip, &tags, failure.app(field), add);
}

fn folder_row(
    ui: &mut Ui,
    tr: &Tr,
    failure: &Failure,
    folder_id: &str,
    field: SettingsField,
    add: impl FnOnce(&mut Ui, Id),
) {
    row(
        ui,
        &tr.t(field.label_key()),
        &tr.t(field.description_key()),
        &[],
        failure.folder(folder_id, field),
        add,
    );
}

// Controls

fn text(ui: &mut Ui, value: &mut String, label: Id) {
    ui.add(TextEdit::singleline(value).desired_width(ui.available_width()))
        .labelled_by(label);
}

fn path_text(ui: &mut Ui, value: &mut PathBuf, label: Id) {
    // The conversion is lossy for names that are not valid Unicode, so the
    // path is only replaced when the user edited it.
    let mut edited = value.to_string_lossy().into_owned();
    let response = ui
        .add(TextEdit::singleline(&mut edited).desired_width(ui.available_width()))
        .labelled_by(label);
    if response.changed() {
        *value = PathBuf::from(edited);
    }
}

fn checkbox(ui: &mut Ui, value: &mut bool, label: Id) {
    ui.checkbox(value, "").labelled_by(label);
}

fn count<T: egui::emath::Numeric>(
    ui: &mut Ui,
    value: &mut T,
    range: std::ops::RangeInclusive<T>,
    label: Id,
) {
    // An existing value outside the range is shown as it is: clamping it
    // would change the draft without the user doing anything.
    ui.add(DragValue::new(value).range(range).clamp_existing_to_range(false))
        .labelled_by(label);
}

/// A checkbox that turns an optional limit on, and the number it carries.
fn optional_count(
    ui: &mut Ui,
    label: Id,
    check_text: &str,
    value: &mut Option<usize>,
    initial: usize,
) {
    ui.horizontal(|ui| {
        let mut on = value.is_some();
        if ui.checkbox(&mut on, check_text).changed() {
            *value = on.then_some(initial);
        }
        let mut shown = value.unwrap_or(initial);
        let changed = ui
            .add_enabled(
                on,
                DragValue::new(&mut shown).range(1..=COUNT_MAX).clamp_existing_to_range(false),
            )
            .labelled_by(label)
            .changed();
        if changed && on {
            *value = Some(shown);
        }
    });
}

fn override_text(ui: &mut Ui, label: Id, check_text: &str, value: &mut Option<String>, initial: &str) {
    ui.horizontal(|ui| {
        let mut on = value.is_some();
        if ui.checkbox(&mut on, check_text).changed() {
            *value = on.then(|| initial.to_string());
        }
        let mut shown = value.clone().unwrap_or_else(|| initial.to_string());
        let changed = ui
            .add_enabled(on, TextEdit::singleline(&mut shown).desired_width(ui.available_width()))
            .labelled_by(label)
            .changed();
        if changed && on {
            *value = Some(shown);
        }
    });
}

#[derive(Clone, Copy, PartialEq)]
enum PolicyKind {
    Inherit,
    Simple(ReferrerPolicyKind),
    Custom,
}

fn policy_kind_label(tr: &Tr, kind: PolicyKind) -> String {
    tr.t(match kind {
        PolicyKind::Inherit => "settings-value-inherit",
        PolicyKind::Simple(ReferrerPolicyKind::None) => "settings-referrer-none",
        PolicyKind::Simple(ReferrerPolicyKind::SameAsUrl) => "settings-referrer-same-as-url",
        PolicyKind::Simple(ReferrerPolicyKind::UrlPath) => "settings-referrer-url-path",
        PolicyKind::Simple(ReferrerPolicyKind::UrlOrigin) => "settings-referrer-url-origin",
        PolicyKind::Custom => "settings-referrer-custom",
    })
}

/// Combo for a referrer policy, plus the value field for a custom one.
/// `None` means "inherit" and is offered only when `allow_inherit` is set.
fn referrer_editor(
    ui: &mut Ui,
    tr: &Tr,
    salt: impl Hash + Debug,
    label: Id,
    policy: &mut Option<ReferrerPolicy>,
    allow_inherit: bool,
) {
    let current = match policy {
        None => PolicyKind::Inherit,
        Some(ReferrerPolicy::Simple(kind)) => PolicyKind::Simple(*kind),
        Some(ReferrerPolicy::Custom { .. }) => PolicyKind::Custom,
    };
    let mut chosen = current;
    ui.horizontal(|ui| {
        ComboBox::from_id_salt(salt)
            .selected_text(policy_kind_label(tr, current))
            .show_ui(ui, |ui| {
                let inherit = allow_inherit.then_some(PolicyKind::Inherit);
                let kinds = inherit.into_iter().chain([
                    PolicyKind::Simple(ReferrerPolicyKind::None),
                    PolicyKind::Simple(ReferrerPolicyKind::SameAsUrl),
                    PolicyKind::Simple(ReferrerPolicyKind::UrlPath),
                    PolicyKind::Simple(ReferrerPolicyKind::UrlOrigin),
                    PolicyKind::Custom,
                ]);
                for kind in kinds {
                    ui.selectable_value(&mut chosen, kind, policy_kind_label(tr, kind));
                }
            })
            .response
            .labelled_by(label);
        if let Some(ReferrerPolicy::Custom { value, .. }) = policy {
            ui.add(
                TextEdit::singleline(value)
                    .hint_text(tr.t("gui-settings-referrer-custom-hint"))
                    .desired_width(ui.available_width()),
            )
            .labelled_by(label);
        }
    });
    if chosen != current {
        *policy = match chosen {
            PolicyKind::Inherit => None,
            PolicyKind::Simple(kind) => Some(ReferrerPolicy::Simple(kind)),
            PolicyKind::Custom => Some(ReferrerPolicy::custom(String::new())),
        };
    }
}

/// Inherit / enabled / disabled for an optional flag. `inherit_text` says
/// what inheriting means right now.
fn tri_state(
    ui: &mut Ui,
    tr: &Tr,
    salt: impl Hash + Debug,
    label: Id,
    value: &mut Option<bool>,
    inherit_text: &str,
) {
    let text = |value: Option<bool>| match value {
        None => inherit_text.to_string(),
        Some(true) => tr.t("settings-value-enabled"),
        Some(false) => tr.t("settings-value-disabled"),
    };
    ComboBox::from_id_salt(salt)
        .selected_text(text(*value))
        .show_ui(ui, |ui| {
            for choice in [None, Some(true), Some(false)] {
                ui.selectable_value(value, choice, text(choice));
            }
        })
        .response
        .labelled_by(label);
}

fn inherit_with(tr: &Tr, enabled: bool) -> String {
    let mut args = FluentArgs::new();
    args.set(
        "value",
        tr.t(if enabled { "settings-value-enabled" } else { "settings-value-disabled" }),
    );
    tr.t_args("gui-settings-inherit-with", &args)
}

// Pages

fn grid(id: impl Hash + Debug) -> Grid {
    Grid::new(("settings_grid", id)).num_columns(2)
}

fn general_page(
    ui: &mut Ui,
    tr: &Tr,
    win: &mut SettingsWindow,
    failure: &Failure,
    out: &mut SettingsOutcome,
) {
    use ApplicationSettingsField as F;
    let busy = win.busy();
    let model = &mut win.model;
    grid("general").show(ui, |ui| {
        app_row(ui, tr, failure, F::Language, |ui, label| {
            // "en-US" and "en" both mean English; only a choice changes it.
            let is_ja = model.draft.general.language.starts_with("ja");
            let mut chosen_ja = is_ja;
            let name = |ja: bool| tr.t(if ja { "gui-settings-language-ja" } else { "gui-settings-language-en" });
            ComboBox::from_id_salt("settings_language")
                .selected_text(name(is_ja))
                .show_ui(ui, |ui| {
                    for ja in [false, true] {
                        ui.selectable_value(&mut chosen_ja, ja, name(ja));
                    }
                })
                .response
                .labelled_by(label);
            if chosen_ja != is_ja {
                model.draft.general.language = if chosen_ja { "ja" } else { "en" }.to_string();
            }
        });
        app_row(ui, tr, failure, F::SkipDownloadPreview, |ui, label| {
            checkbox(ui, &mut model.draft.general.skip_download_preview, label);
        });
        app_row(ui, tr, failure, F::AutoLaunchDnd, |ui, label| {
            checkbox(ui, &mut model.draft.general.auto_launch_dnd, label);
        });
        row(
            ui,
            &tr.t("gui-settings-expand-ranges"),
            &tr.t("gui-settings-expand-ranges-desc"),
            &[],
            None,
            |ui, label| checkbox(ui, &mut model.gui_draft.expand_ranges_on_direct_add, label),
        );
    });
    ui.add_space(ui.spacing().item_spacing.y);
    let reload = ui
        .add_enabled(!busy, Button::new(tr.t("gui-settings-reload-config")))
        .on_hover_text(tr.t("gui-settings-reload-config-desc"));
    if reload.clicked() {
        win.start(Action::ReloadConfig, out);
    }
}

fn downloads_page(ui: &mut Ui, tr: &Tr, draft: &mut Config, failure: &Failure) {
    use ApplicationSettingsField as F;
    let download = &mut draft.download;
    grid("downloads").show(ui, |ui| {
        app_row(ui, tr, failure, F::DefaultDirectory, |ui, label| {
            path_text(ui, &mut download.default_directory, label);
        });
        app_row(ui, tr, failure, F::MaxConcurrent, |ui, label| {
            count(ui, &mut download.max_concurrent, 1..=COUNT_MAX, label);
        });
        app_row(ui, tr, failure, F::MaxConcurrentPerFolder, |ui, label| {
            optional_count(ui, label, &tr.t("gui-settings-limit"), &mut download.max_concurrent_per_folder, 1);
        });
        app_row(ui, tr, failure, F::MaxActiveFolders, |ui, label| {
            optional_count(ui, label, &tr.t("gui-settings-limit"), &mut download.parallel_folder_count, 1);
        });
        app_row(ui, tr, failure, F::RetryCount, |ui, label| {
            count(ui, &mut download.retry_count, 0..=1000, label);
        });
    });
}

fn connection_page(ui: &mut Ui, tr: &Tr, draft: &mut Config, failure: &Failure) {
    use ApplicationSettingsField as F;
    let download = &mut draft.download;
    grid("connection").show(ui, |ui| {
        app_row(ui, tr, failure, F::UserAgent, |ui, label| {
            text(ui, &mut download.user_agent, label);
        });
        app_row(ui, tr, failure, F::ReferrerPolicy, |ui, label| {
            // The application policy is never "inherit".
            let mut policy = Some(download.referrer_policy.clone());
            referrer_editor(ui, tr, "settings_referrer_policy", label, &mut policy, false);
            if let Some(policy) = policy {
                download.referrer_policy = policy;
            }
        });
        app_row(ui, tr, failure, F::MaxRedirects, |ui, label| {
            count(ui, &mut download.max_redirects, 0..=1000, label);
        });
    });
}

fn scripts_page(
    ui: &mut Ui,
    tr: &Tr,
    win: &mut SettingsWindow,
    failure: &Failure,
    out: &mut SettingsOutcome,
) {
    use ApplicationSettingsField as F;
    let busy = win.busy();
    let SettingsWindow { model, script_files, .. } = &mut *win;
    let scripts = &mut model.draft.scripts;
    grid("scripts").show(ui, |ui| {
        app_row(ui, tr, failure, F::ScriptsEnabled, |ui, label| {
            checkbox(ui, &mut scripts.enabled, label);
        });
    });
    ui.add_space(ui.spacing().item_spacing.y);
    let mut args = FluentArgs::new();
    args.set("count", script_files.len());
    ui.label(RichText::new(tr.t_args("settings-scripts-header", &args)).strong());
    if script_files.is_empty() {
        ui.label(RichText::new(tr.t("settings-scripts-none")).weak());
    } else {
        grid("script_files").show(ui, |ui| {
            for name in script_files.iter() {
                row(ui, name, "", &[], failure.script_file(name), |ui, label| {
                    // A missing entry means enabled.
                    let mut enabled = scripts.script_files.get(name).copied().unwrap_or(true);
                    if ui.checkbox(&mut enabled, "").labelled_by(label).changed() {
                        scripts.script_files.insert(name.clone(), enabled);
                    }
                });
            }
        });
    }
    ui.add_space(ui.spacing().item_spacing.y);
    let reload = ui
        .add_enabled(!busy, Button::new(tr.t("gui-settings-reload-scripts")))
        .on_hover_text(tr.t("gui-settings-reload-scripts-desc"));
    if reload.clicked() {
        win.start(Action::ReloadScripts, out);
    }
}

fn folders_page(
    ui: &mut Ui,
    tr: &Tr,
    win: &mut SettingsWindow,
    failure: &Failure,
    out: &mut SettingsOutcome,
) {
    let busy = win.busy();
    let model = &win.model;
    // Listed in the order of the applied names, so a name being typed does
    // not move its row.
    let entries: Vec<(String, String)> = model
        .original
        .sorted_folder_entries()
        .into_iter()
        .map(|(id, original_name)| {
            let name = model
                .draft
                .folders
                .get(&id)
                .map(|f| f.name.clone())
                .filter(|name| !name.is_empty())
                .unwrap_or(original_name);
            (id, name)
        })
        .collect();
    let selected = model.selected_folder.clone();
    let removable = selected
        .as_deref()
        .is_some_and(|id| model.original.folders.contains_key(id) && !model.original.is_default_folder(id));

    if entries.is_empty() {
        ui.label(RichText::new(tr.t("settings-no-folders")).weak());
    }
    let mut chosen = selected.clone();
    ScrollArea::vertical()
        .id_salt("settings_folder_list")
        .max_height(FOLDER_LIST_HEIGHT)
        .auto_shrink([false, true])
        .show(ui, |ui| {
            ui.with_layout(Layout::top_down_justified(Align::LEFT), |ui| {
                for (id, name) in &entries {
                    let is_selected = chosen.as_deref() == Some(id.as_str());
                    if ui.selectable_label(is_selected, name).clicked() {
                        chosen = Some(id.clone());
                    }
                }
            });
        });
    win.model.selected_folder = chosen;

    ui.horizontal(|ui| {
        if ui.add_enabled(!busy, Button::new(tr.t("gui-settings-folder-add"))).clicked() {
            win.start(Action::CreateFolder, out);
        }
        let remove = ui.add_enabled(!busy && removable, Button::new(tr.t("gui-settings-folder-remove")));
        if remove.clicked() {
            win.confirm_remove = selected.clone();
        }
        ui.label(RichText::new(tr.t("gui-settings-folder-immediate")).weak());
    });
    ui.separator();

    let Some(folder_id) = win.model.selected_folder.clone() else {
        return;
    };
    let is_default = win.model.original.is_default_folder(&folder_id);
    let SettingsWindow { model, script_files, .. } = &mut *win;
    let Config { folders, download, scripts, .. } = &mut model.draft;
    let Some(folder) = folders.get_mut(&folder_id) else {
        return;
    };
    ui.push_id(("settings_folder", &folder_id), |ui| {
        folder_fields(
            ui,
            tr,
            failure,
            &folder_id,
            folder,
            is_default,
            &download.user_agent,
            scripts,
            script_files,
        );
    });
}

#[allow(clippy::too_many_arguments)]
fn folder_fields(
    ui: &mut Ui,
    tr: &Tr,
    failure: &Failure,
    folder_id: &str,
    folder: &mut FolderConfig,
    is_default: bool,
    app_user_agent: &str,
    app_scripts: &ggg::app::config::ScriptConfig,
    script_files: &[String],
) {
    use SettingsField as S;
    grid("folder").show(ui, |ui| {
        folder_row(ui, tr, failure, folder_id, S::FolderName, |ui, label| {
            // The default folder is recognised by its name, so it keeps it.
            ui.add_enabled_ui(!is_default, |ui| text(ui, &mut folder.name, label));
        });
        folder_row(ui, tr, failure, folder_id, S::FolderSavePath, |ui, label| {
            path_text(ui, &mut folder.save_path, label);
        });
        folder_row(ui, tr, failure, folder_id, S::FolderAutoDate, |ui, label| {
            checkbox(ui, &mut folder.auto_date_directory, label);
        });
        folder_row(ui, tr, failure, folder_id, S::FolderAutoStart, |ui, label| {
            checkbox(ui, &mut folder.auto_start_downloads, label);
        });
        folder_row(ui, tr, failure, folder_id, S::FolderPreventDuplicateUrl, |ui, label| {
            checkbox(ui, &mut folder.prevent_duplicate_url, label);
        });
        folder_row(ui, tr, failure, folder_id, S::FolderScripts, |ui, label| {
            let inherit = inherit_with(tr, app_scripts.enabled);
            tri_state(ui, tr, "settings_folder_scripts", label, &mut folder.scripts_enabled, &inherit);
        });
        for name in script_files {
            row(
                ui,
                name,
                "",
                &[],
                failure.folder_script_file(folder_id, name),
                |ui, label| {
                    let app_value = app_scripts.script_files.get(name).copied().unwrap_or(true);
                    let mut value = folder.script_files.as_ref().and_then(|m| m.get(name).copied());
                    let before = value;
                    tri_state(ui, tr, ("settings_folder_script", name), label, &mut value, &inherit_with(tr, app_value));
                    if value != before {
                        set_script_override(folder, name, value);
                    }
                },
            );
        }
        folder_row(ui, tr, failure, folder_id, S::FolderMaxConcurrent, |ui, label| {
            optional_count(ui, label, &tr.t("gui-settings-override"), &mut folder.max_concurrent, 1);
        });
        folder_row(ui, tr, failure, folder_id, S::FolderUserAgent, |ui, label| {
            override_text(ui, label, &tr.t("gui-settings-override"), &mut folder.user_agent, app_user_agent);
        });
        folder_row(ui, tr, failure, folder_id, S::FolderReferrerPolicy, |ui, label| {
            referrer_editor(ui, tr, "settings_folder_referrer", label, &mut folder.referrer_policy, true);
        });
        folder_row(ui, tr, failure, folder_id, S::FolderHeaders, |ui, _| {
            if folder.default_headers.is_empty() {
                ui.label(RichText::new(tr.t("settings-value-not-set")).weak());
            }
            let mut headers: Vec<_> = folder.default_headers.iter().collect();
            headers.sort();
            for (name, value) in headers {
                ui.label(format!("{name}: {}", display_header_value(name, value)));
            }
        });
    });
}

/// Sets or removes (`None`) a folder's override for one script file. An empty
/// override map is dropped so the folder reads as having none.
fn set_script_override(folder: &mut FolderConfig, name: &str, value: Option<bool>) {
    match value {
        Some(enabled) => {
            folder
                .script_files
                .get_or_insert_with(HashMap::new)
                .insert(name.to_string(), enabled);
        }
        None => {
            if let Some(files) = &mut folder.script_files {
                files.remove(name);
                if files.is_empty() {
                    folder.script_files = None;
                }
            }
        }
    }
}

/// Asks before a folder is removed; removing acts at once and Cancel of the
/// window does not undo it.
fn confirm_remove(ctx: &egui::Context, tr: &Tr, win: &mut SettingsWindow, out: &mut SettingsOutcome) {
    let Some(folder_id) = win.confirm_remove.clone() else {
        return;
    };
    if !win.model.draft.folders.contains_key(&folder_id) {
        win.confirm_remove = None;
        return;
    }
    let name = win.model.draft.folder_name(&folder_id);
    let mut args = FluentArgs::new();
    args.set("name", name.as_str());
    let message = tr.t_args("gui-settings-folder-remove-confirm", &args);

    let response = egui::Modal::new(Id::unique("settings_remove_folder_modal")).show(ctx, |ui| {
        ui.set_width(CONFIRM_WIDTH);
        ui.label(RichText::new(tr.t("gui-confirm-title")).strong());
        ui.separator();
        ui.add(egui::Label::new(message).wrap());
        ui.label(RichText::new(tr.t("confirm-delete-folder-config-note")).weak());
        ui.add_space(ui.spacing().item_spacing.y);
        ui.horizontal(|ui| {
            if ui.button(tr.t("gui-settings-folder-remove-button")).clicked() {
                return Some(true);
            }
            if ui.button(tr.t("gui-button-cancel")).clicked() {
                return Some(false);
            }
            None
        })
        .inner
    });
    let answer = response
        .inner
        .or_else(|| response.should_close().then_some(false));
    match answer {
        Some(true) => {
            win.confirm_remove = None;
            if !win.busy() {
                win.start(Action::DeleteFolder(folder_id), out);
            }
        }
        Some(false) => win.confirm_remove = None,
        None => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{bridge::actions::ActionKind, gui_config::GuiConfig};
    use egui_kittest::{Harness, kittest::Queryable};
    use ggg::ui::commands::CommandError;
    use std::{
        cell::{Cell, RefCell},
        rc::Rc,
        sync::Arc,
    };

    const DEFAULT_ID: &str = "f1";
    const MUSIC_ID: &str = "f2";

    fn tr() -> Tr {
        Tr::load("en-US")
    }

    fn folder(name: &str) -> FolderConfig {
        FolderConfig { name: name.to_string(), ..FolderConfig::default() }
    }

    fn sample_config() -> Config {
        let mut config = Config::default();
        config.folders.clear();
        config.folders.insert(DEFAULT_ID.to_string(), folder("default"));
        config.folders.insert(MUSIC_ID.to_string(), folder("Music"));
        // A directory that does not exist, so a rescan never reads real files.
        config.scripts.directory = std::env::temp_dir().join("settings-view-test-no-such-dir");
        config
    }

    /// The window under test, and what its frames asked for. Nothing is
    /// dispatched: the tests play the part of the core by feeding results back.
    struct Rig {
        window: Rc<RefCell<SettingsWindow>>,
        actions: Rc<RefCell<Vec<Action>>>,
        closed: Rc<Cell<u32>>,
        expand: Rc<Cell<Option<bool>>>,
    }

    impl Rig {
        fn edit(&self, edit: impl FnOnce(&mut SettingsModel)) {
            edit(&mut self.window.borrow_mut().model);
        }

        fn page(&self, page: SettingsPage) {
            self.edit(|model| model.page = page);
        }

        fn take_actions(&self) -> Vec<Action> {
            std::mem::take(&mut *self.actions.borrow_mut())
        }

        /// The commands of the one `ApplySettings` action taken.
        fn take_commands(&self) -> Vec<Command> {
            let mut actions = self.take_actions();
            assert_eq!(actions.len(), 1, "one action expected");
            match actions.remove(0) {
                Action::ApplySettings { commands } => commands,
                _ => panic!("expected ApplySettings"),
            }
        }

        fn deliver(&self, result: ActionResult) -> bool {
            self.window.borrow_mut().on_action_result(&tr(), &result)
        }

        /// The core accepted the commands: it now runs the draft.
        fn applied_ok(&self) -> bool {
            let config = Arc::new(self.window.borrow().model.draft.clone());
            self.deliver(ActionResult::Settings { applied: 1, error: None, config })
        }
    }

    fn rig_with(config: Config, script_files: &[&str]) -> (Harness<'static, ()>, Rig) {
        let model = SettingsModel::new(&config, &GuiConfig::default());
        let files = script_files.iter().map(|s| s.to_string()).collect();
        let rig = Rig {
            window: Rc::new(RefCell::new(SettingsWindow::new(model, files))),
            actions: Rc::default(),
            closed: Rc::default(),
            expand: Rc::default(),
        };
        let (window, actions, closed, expand) =
            (rig.window.clone(), rig.actions.clone(), rig.closed.clone(), rig.expand.clone());
        let tr = tr();
        let harness = Harness::builder()
            .with_size(egui::vec2(760.0, 540.0))
            .build_ui(move |ui| {
                let outcome = show(ui, &tr, &mut window.borrow_mut());
                actions.borrow_mut().extend(outcome.actions);
                if outcome.close {
                    closed.set(closed.get() + 1);
                }
                if outcome.expand_ranges.is_some() {
                    expand.set(outcome.expand_ranges);
                }
            });
        (harness, rig)
    }

    fn rig() -> (Harness<'static, ()>, Rig) {
        rig_with(sample_config(), &[])
    }

    fn disabled(node: &egui_kittest::Node<'_>) -> bool {
        egui_kittest::kittest::NodeT::accesskit_node(node).is_disabled()
    }

    fn count_labels(harness: &Harness<'_, ()>, label: &str) -> usize {
        harness.query_all_by_label(label).count()
    }

    #[test]
    fn settings_lists_every_application_field() {
        let (mut harness, rig) = rig();
        let tr = tr();
        let mut missing: Vec<ApplicationSettingsField> = ApplicationSettingsField::all();
        for page in SettingsPage::all() {
            rig.page(page);
            harness.run();
            missing.retain(|field| count_labels(&harness, &tr.t(field.label_key())) == 0);
        }
        assert!(missing.is_empty(), "labels not shown on any page: {missing:?}");
    }

    #[test]
    fn folders_page_lists_every_folder_field() {
        let (mut harness, rig) = rig();
        let tr = tr();
        rig.edit(|model| model.open_folder(MUSIC_ID));
        harness.run();
        for field in SettingsField::all() {
            let label = tr.t(field.label_key());
            assert!(count_labels(&harness, &label) > 0, "{label} is not shown");
        }
    }

    #[test]
    fn tui_only_rows_say_so_and_restart_rows_are_marked() {
        let (mut harness, _rig) = rig();
        harness.run();
        // Skip download preview and Auto launch ggg-dnd.
        assert_eq!(count_labels(&harness, "(TUI only)"), 2);
        // Language and Auto launch ggg-dnd.
        assert_eq!(count_labels(&harness, "(restart required)"), 2);
    }

    #[test]
    fn apply_disabled_until_edit() {
        let (mut harness, rig) = rig();
        harness.run();
        assert!(disabled(&harness.get_by_label("Apply")));
        assert!(!disabled(&harness.get_by_label("OK")));

        rig.edit(|model| model.draft.download.max_concurrent += 2);
        harness.run();
        assert!(!disabled(&harness.get_by_label("Apply")));

        rig.edit(|model| model.draft.download.max_concurrent -= 2);
        harness.run();
        assert!(disabled(&harness.get_by_label("Apply")));
    }

    #[test]
    fn apply_sends_the_difference_and_greys_out_after_success() {
        let (mut harness, rig) = rig();
        let before = rig.window.borrow().model.original.download.max_concurrent;
        rig.edit(|model| model.draft.download.max_concurrent = before + 2);
        harness.run();

        harness.get_by_label("Apply").click();
        harness.run();
        assert_eq!(rig.take_commands(), vec![Command::UpdateMaxConcurrent { value: before + 2 }]);
        // Nothing more can be sent until the result is back.
        assert!(disabled(&harness.get_by_label("Apply")));
        assert!(rig.window.borrow().busy());

        assert!(!rig.applied_ok(), "Apply keeps the window open");
        harness.run();
        assert!(disabled(&harness.get_by_label("Apply")));
        assert!(!rig.window.borrow().busy());
        assert_eq!(rig.closed.get(), 0);
    }

    #[test]
    fn rename_to_an_existing_name_shows_the_error_and_keeps_the_window() {
        let (mut harness, rig) = rig();
        rig.edit(|model| model.draft.folders.get_mut(MUSIC_ID).unwrap().name = "default".to_string());
        harness.run();

        harness.get_by_label("OK").click();
        harness.run();
        assert_eq!(rig.take_commands().len(), 1);

        let error = CommandError::DuplicateFolderName { name: "default".to_string() };
        let message = tr().command_error(&error);
        let config = Arc::new(rig.window.borrow().model.original.clone());
        let close = rig.deliver(ActionResult::Settings { applied: 0, error: Some((0, error)), config });
        harness.run();

        assert!(!close, "a refused OK must not close the window");
        assert_eq!(rig.closed.get(), 0);
        // On the Folders page, next to the field, and in the banner.
        assert_eq!(rig.window.borrow().model.page, SettingsPage::Folders);
        assert_eq!(rig.window.borrow().model.selected_folder.as_deref(), Some(MUSIC_ID));
        harness.get_by_label(&message);
        let mut args = FluentArgs::new();
        args.set("error", message.as_str());
        harness.get_by_label(&tr().t_args("gui-status-settings-failed", &args));
        // The draft is kept, so the user can fix the name and retry.
        assert!(!disabled(&harness.get_by_label("Apply")));
    }

    #[test]
    fn error_on_another_page_switches_to_it() {
        let (mut harness, rig) = rig();
        rig.page(SettingsPage::General);
        rig.edit(|model| model.draft.download.default_directory = PathBuf::new());
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        let _ = rig.take_commands();

        let config = Arc::new(rig.window.borrow().model.original.clone());
        rig.deliver(ActionResult::Settings {
            applied: 0,
            error: Some((0, CommandError::EmptyDefaultDirectory)),
            config,
        });
        harness.run();
        assert_eq!(rig.window.borrow().model.page, SettingsPage::Downloads);
        harness.get_by_label(&tr().command_error(&CommandError::EmptyDefaultDirectory));
    }

    #[test]
    fn cancel_discards_edits() {
        let (mut harness, rig) = rig();
        rig.edit(|model| model.draft.download.max_concurrent += 1);
        harness.run();
        harness.get_by_label("Cancel").click();
        harness.run();
        assert_eq!(rig.closed.get(), 1);
        assert!(rig.take_actions().is_empty());
        assert!(rig.expand.get().is_none());
    }

    #[test]
    fn escape_cancels() {
        let (mut harness, rig) = rig();
        harness.run();
        harness.key_press(egui::Key::Escape);
        harness.run();
        assert_eq!(rig.closed.get(), 1);
        assert!(rig.take_actions().is_empty());
    }

    #[test]
    fn ok_without_edits_just_closes() {
        let (mut harness, rig) = rig();
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        assert_eq!(rig.closed.get(), 1);
        assert!(rig.take_actions().is_empty());
    }

    #[test]
    fn ok_closes_only_after_the_apply_succeeded() {
        let (mut harness, rig) = rig();
        rig.edit(|model| model.draft.download.retry_count += 1);
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        assert_eq!(rig.take_commands().len(), 1);
        assert_eq!(rig.closed.get(), 0, "the window waits for the result");
        assert!(disabled(&harness.get_by_label("OK")));
        assert!(rig.applied_ok(), "success after OK closes the window");
    }

    #[test]
    fn language_change_reports_that_a_restart_is_needed() {
        let (mut harness, rig) = rig();
        // The bundled fonts have no Japanese glyphs; the app gets them from the system.
        harness.ctx.add_font_provider(Arc::new(egui_system_font_provider::SystemFontProvider::new()));
        harness.run();
        // The combo shows English; pick Japanese from it.
        harness.get_by_label("Language").click();
        harness.run();
        harness.get_by_label("日本語").click();
        harness.run();
        assert_eq!(rig.window.borrow().model.draft.general.language, "ja");

        harness.get_by_label("OK").click();
        harness.run();
        assert_eq!(rig.take_commands(), vec![Command::UpdateLanguage { value: "ja".to_string() }]);
        assert!(rig.applied_ok());
        harness.run();
        assert!(rig.window.borrow().model.restart_needed);
        harness.get_by_label(&tr().t("gui-settings-restart-banner"));
    }

    #[test]
    fn gui_only_setting_needs_no_command() {
        let (mut harness, rig) = rig();
        rig.edit(|model| model.gui_draft.expand_ranges_on_direct_add = false);
        harness.run();
        assert!(!disabled(&harness.get_by_label("Apply")));
        harness.get_by_label("Apply").click();
        harness.run();
        assert!(rig.take_actions().is_empty());
        assert_eq!(rig.expand.get(), Some(false));
        assert!(disabled(&harness.get_by_label("Apply")));
        assert!(!rig.window.borrow().busy());
    }

    #[test]
    fn adding_a_folder_acts_at_once_and_survives_cancel() {
        let (mut harness, rig) = rig();
        rig.page(SettingsPage::Folders);
        harness.run();
        harness.get_by_label("Add").click();
        harness.run();
        let actions = rig.take_actions();
        assert!(matches!(actions.as_slice(), [Action::CreateFolder]));
        assert!(disabled(&harness.get_by_label("Add")));

        let mut live = rig.window.borrow().model.original.clone();
        live.folders.insert("f3".to_string(), folder("folder_1"));
        rig.deliver(ActionResult::Command {
            action: ActionKind::CreateFolder,
            result: Ok(serde_json::json!({"folder_id": "f3"})),
            config: Arc::new(live),
        });
        harness.run();
        harness.get_by_label("folder_1");
        assert_eq!(rig.window.borrow().model.selected_folder.as_deref(), Some("f3"));
        // A new folder is not a pending edit.
        assert!(disabled(&harness.get_by_label("Apply")));

        harness.get_by_label("Cancel").click();
        harness.run();
        assert_eq!(rig.closed.get(), 1);
        assert!(rig.take_actions().is_empty(), "Cancel sends nothing, so the folder stays");
    }

    #[test]
    fn removing_a_folder_asks_first_and_the_default_cannot_be_removed() {
        let (mut harness, rig) = rig();
        rig.edit(|model| model.open_folder(DEFAULT_ID));
        harness.run();
        assert!(disabled(&harness.get_by_label("Remove...")));

        rig.edit(|model| model.open_folder(MUSIC_ID));
        harness.run();
        harness.get_by_label("Remove...").click();
        harness.run();
        assert!(rig.take_actions().is_empty(), "nothing happens before the answer");
        harness.get_by_label_contains("Remove the folder \"Music\"?");

        harness.get_by_label("Remove").click();
        harness.run();
        let actions = rig.take_actions();
        assert!(matches!(actions.as_slice(), [Action::DeleteFolder(id)] if id == MUSIC_ID));
    }

    #[test]
    fn declining_the_removal_sends_nothing() {
        let (mut harness, rig) = rig();
        rig.edit(|model| model.open_folder(MUSIC_ID));
        harness.run();
        harness.get_by_label("Remove...").click();
        harness.run();
        // The confirmation and the window both have a Cancel button.
        let cancel: Vec<_> = harness.get_all_by_label("Cancel").collect();
        assert_eq!(cancel.len(), 2);
        cancel.last().unwrap().click();
        harness.run();
        assert!(rig.take_actions().is_empty());
        assert_eq!(rig.closed.get(), 0);
        assert!(harness.query_by_label_contains("Remove the folder").is_none());
    }

    #[test]
    fn folder_script_override_off_is_sent_on_ok() {
        let (mut harness, rig) = rig_with(sample_config(), &["a.js"]);
        rig.edit(|model| model.open_folder(MUSIC_ID));
        harness.run();

        // The file's row: its combo is named after the file.
        harness.get_by_label("a.js").click();
        harness.run();
        harness.get_by_label("Disabled").click();
        harness.run();

        harness.get_by_label("OK").click();
        harness.run();
        assert_eq!(
            rig.take_commands(),
            vec![Command::SetFolderScriptFileOverride {
                folder_id: MUSIC_ID.to_string(),
                filename: "a.js".to_string(),
                value: Some(false),
            }]
        );
    }

    #[test]
    fn reload_settings_button_requests_the_action() {
        let (mut harness, rig) = rig();
        harness.run();
        harness.get_by_label("Reload settings from disk").click();
        harness.run();
        assert!(matches!(rig.take_actions().as_slice(), [Action::ReloadConfig]));
        assert!(disabled(&harness.get_by_label("Reload settings from disk")));
    }

    #[test]
    fn reload_scripts_button_requests_the_action_and_lists_new_files() {
        let dir = tempfile::TempDir::new().unwrap();
        let mut config = sample_config();
        config.scripts.directory = dir.path().to_path_buf();
        let (mut harness, rig) = rig_with(config.clone(), &[]);
        rig.page(SettingsPage::Scripts);
        harness.run();
        assert!(harness.query_by_label("a.js").is_none());

        harness.get_by_label("Reload scripts").click();
        harness.run();
        assert!(matches!(rig.take_actions().as_slice(), [Action::ReloadScripts]));

        std::fs::write(dir.path().join("a.js"), "").unwrap();
        rig.deliver(ActionResult::Command {
            action: ActionKind::ReloadScripts,
            result: Ok(serde_json::Value::Null),
            config: Arc::new(config),
        });
        harness.run();
        harness.get_by_label("a.js");
    }

    #[test]
    fn reloading_from_disk_resets_the_draft() {
        let (mut harness, rig) = rig();
        rig.edit(|model| model.draft.download.max_concurrent += 4);
        harness.run();
        assert!(!disabled(&harness.get_by_label("Apply")));

        let disk = rig.window.borrow().model.original.clone();
        rig.deliver(ActionResult::Command {
            action: ActionKind::ReloadConfig,
            result: Ok(serde_json::Value::Null),
            config: Arc::new(disk),
        });
        harness.run();
        assert!(disabled(&harness.get_by_label("Apply")));
    }

    #[test]
    fn refused_immediate_action_is_reported_in_the_window() {
        let (mut harness, rig) = rig();
        rig.deliver(ActionResult::Command {
            action: ActionKind::ReloadConfig,
            result: Err(CommandError::ReloadActiveDownloads),
            config: Arc::new(sample_config()),
        });
        harness.run();
        harness.get_by_label(&tr().command_error(&CommandError::ReloadActiveDownloads));
    }

    #[test]
    fn script_without_an_entry_counts_as_enabled() {
        let (mut harness, rig) = rig_with(sample_config(), &["a.js", "b.js"]);
        rig.edit(|model| {
            model.draft.scripts.script_files.insert("b.js".to_string(), false);
        });
        rig.page(SettingsPage::Scripts);
        harness.run();
        harness.get_by_label("a.js");
        harness.get_by_label("b.js");
        assert_eq!(
            rig.window.borrow().model.commands(),
            vec![Command::SetScriptFileEnabled { filename: "b.js".to_string(), enabled: false }]
        );
    }

    #[test]
    fn set_script_override_drops_an_empty_map() {
        let mut folder = folder("x");
        set_script_override(&mut folder, "a.js", Some(false));
        assert_eq!(folder.script_files.as_ref().unwrap()["a.js"], false);
        set_script_override(&mut folder, "a.js", None);
        assert!(folder.script_files.is_none());
    }

    #[test]
    fn failed_commands_point_at_their_page() {
        let location = |command: Command| {
            let target = target_of(&command).expect("a settings command");
            let (page, folder) = target.location();
            (page, folder.map(str::to_string))
        };
        assert_eq!(
            location(Command::UpdateLanguage { value: "ja".into() }),
            (SettingsPage::General, None)
        );
        assert_eq!(
            location(Command::UpdateMaxRedirects { value: 1 }),
            (SettingsPage::Connection, None)
        );
        assert_eq!(
            location(Command::SetScriptFileEnabled { filename: "a.js".into(), enabled: true }),
            (SettingsPage::Scripts, None)
        );
        assert_eq!(
            location(Command::UpdateFolderName { folder_id: "f2".into(), value: "x".into() }),
            (SettingsPage::Folders, Some("f2".to_string()))
        );
        assert!(target_of(&Command::CreateFolder).is_none());
    }

    /// Points the config directory at a temp dir for one test. `Config::save`
    /// deletes folder directories it does not know, so a test that reaches it
    /// without this would wipe the real configuration. Reset on drop, so a
    /// panic cannot leave the override behind.
    struct ConfigIsolation {
        dir: tempfile::TempDir,
    }

    impl ConfigIsolation {
        fn new() -> Self {
            let dir = tempfile::TempDir::new().unwrap();
            ggg::util::paths::set_config_dir_override(Some(dir.path().to_path_buf()));
            unsafe { std::env::set_var("GGG_TEST_MODE", "1") };
            Self { dir }
        }
    }

    impl Drop for ConfigIsolation {
        fn drop(&mut self) {
            ggg::util::paths::set_config_dir_override(None);
            unsafe { std::env::remove_var("GGG_TEST_MODE") };
        }
    }

    /// Runs `commands` the way an apply does and returns the first error.
    async fn run_commands(state: &ggg::AppState, commands: Vec<Command>) -> Option<CommandError> {
        use ggg::ui::commands::{CommandResponse, handle_command};
        for command in commands {
            let manager = ggg::download::manager::DownloadManager::new();
            if let CommandResponse::Error(e) = handle_command(command, state.clone(), manager).await {
                return Some(e);
            }
        }
        None
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn applied_edits_reach_the_settings_files() {
        let isolation = ConfigIsolation::new();
        let state = ggg::AppState::new(sample_config());
        let mut model = SettingsModel::new(&sample_config(), &GuiConfig::default());

        // Max concurrent 3 -> 5, and a per-folder script file switched off.
        model.draft.download.max_concurrent = 5;
        let music = model.draft.folders.get_mut(MUSIC_ID).unwrap();
        set_script_override(music, "a.js", Some(false));
        assert!(run_commands(&state, model.begin_apply()).await.is_none());

        let app = std::fs::read_to_string(isolation.dir.path().join("settings.toml")).unwrap();
        assert!(app.contains("max_concurrent = 5"), "{app}");
        let folder =
            std::fs::read_to_string(isolation.dir.path().join(MUSIC_ID).join("settings.toml")).unwrap();
        assert!(folder.contains("\"a.js\" = false"), "{folder}");
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn renaming_to_an_existing_name_is_refused_and_not_saved() {
        let isolation = ConfigIsolation::new();
        let state = ggg::AppState::new(sample_config());
        let mut model = SettingsModel::new(&sample_config(), &GuiConfig::default());

        model.draft.folders.get_mut(MUSIC_ID).unwrap().name = "default".to_string();
        let error = run_commands(&state, model.begin_apply()).await;

        assert!(matches!(error, Some(CommandError::DuplicateFolderName { .. })), "{error:?}");
        assert_eq!(state.config.read().await.folders[MUSIC_ID].name, "Music");
        let saved = isolation.dir.path().join(MUSIC_ID).join("settings.toml");
        if let Ok(text) = std::fs::read_to_string(saved) {
            assert!(!text.contains("name = \"default\""), "{text}");
        }
    }

    #[test]
    fn every_page_fits_the_minimum_width() {
        use egui_kittest::kittest::NodeT;
        let mut config = sample_config();
        let music = config.folders.get_mut(MUSIC_ID).unwrap();
        music.referrer_policy = Some(ReferrerPolicy::custom("https://example.com/a-fairly-long-referrer-value"));
        music.user_agent = Some("Mozilla/5.0 (a long user agent string for the width check)".to_string());
        music.default_headers.insert("Authorization".to_string(), "secret".to_string());
        let (mut harness, rig) = rig_with(config, &["first-script.js", "second-script.js"]);
        harness.set_size(egui::vec2(crate::window::SETTINGS_MIN_SIZE.0 as f32, crate::window::SETTINGS_MIN_SIZE.1 as f32));
        rig.edit(|model| model.open_folder(MUSIC_ID));
        for page in SettingsPage::all() {
            rig.page(page);
            harness.run();
            let width = harness.ctx.content_rect().right();
            assert_eq!(width, crate::window::SETTINGS_MIN_SIZE.0 as f32);
            let widest = harness
                .root()
                .children_recursive()
                // The text inside a field is clipped to the field.
                .filter(|node| node.accesskit_node().role() != egui::accesskit::Role::TextRun)
                .filter_map(|node| node.accesskit_node().bounding_box())
                .map(|rect| rect.x1 as f32 / harness.ctx.pixels_per_point())
                .fold(0.0_f32, f32::max);
            assert!(widest <= width + 0.5, "{page:?}: content reaches {widest}, window is {width}");
        }
    }

    #[test]
    fn restricted_headers_are_masked() {
        let mut config = sample_config();
        let music = config.folders.get_mut(MUSIC_ID).unwrap();
        music.default_headers.insert("Authorization".to_string(), "secret-token".to_string());
        music.default_headers.insert("Accept".to_string(), "text/html".to_string());
        let (mut harness, rig) = rig_with(config, &[]);
        rig.edit(|model| model.open_folder(MUSIC_ID));
        harness.run();
        assert_eq!(count_labels(&harness, "Accept: text/html"), 1);
        assert_eq!(count_labels(&harness, "Authorization: secret-token"), 0);
        assert_eq!(count_labels(&harness, "Authorization: <redacted>"), 1);
    }
    #[test]
    fn strings_exist_in_every_locale() {
        for language in ["en-US", "ja-JP"] {
            let tr = Tr::load(language);
            let mut args = FluentArgs::new();
            args.set("name", "x");
            args.set("value", "y");
            args.set("error", "z");
            for key in [
                "gui-settings-title",
                "gui-settings-page-connection",
                "gui-button-apply",
                "gui-status-settings-applied-restart",
                "gui-settings-restart-banner",
                "gui-settings-tui-only-suffix",
                "gui-settings-tui-only-preview",
                "gui-settings-tui-only-dnd",
                "gui-settings-language-en",
                "gui-settings-language-ja",
                "gui-settings-expand-ranges",
                "gui-settings-expand-ranges-desc",
                "gui-settings-reload-config",
                "gui-settings-reload-config-desc",
                "gui-settings-reload-scripts",
                "gui-settings-reload-scripts-desc",
                "gui-settings-limit",
                "gui-settings-override",
                "gui-settings-referrer-custom-hint",
                "gui-settings-inherit-with",
                "gui-settings-folder-add",
                "gui-settings-folder-remove",
                "gui-settings-folder-remove-button",
                "gui-settings-folder-immediate",
                "gui-settings-folder-remove-confirm",
                "settings-tab-general",
                "settings-restart-marker",
                "settings-scripts-none",
                "confirm-delete-folder-config-note",
            ] {
                let text = tr.t_args(key, &args);
                assert!(
                    !text.is_empty() && !text.contains("[missing"),
                    "{language}: {key} -> {text:?}"
                );
            }
        }
    }
}
