//! In-window dialogs (`egui::Modal`) of the main window: confirmations,
//! move to folder, item editor, shortcut help and About.

use crate::{
    bridge::{actions::Action, snapshot::Snapshot},
    commands::{GuiCommand, find_task},
    i18n::Tr,
};
use egui::{Button, Id, Key, Modifiers, RichText, ScrollArea, TextEdit, Ui};
use fluent_bundle::FluentArgs;
use ggg::ui::item_edit::ItemEditorState;
use uuid::Uuid;

/// Wide enough for a path, narrow enough for a small window.
const DIALOG_WIDTH: f32 = 420.0;
const LIST_MAX_HEIGHT: f32 = 240.0;
const HEADERS_MAX_HEIGHT: f32 = 90.0;

pub enum Modal {
    ConfirmDelete {
        targets: Vec<Uuid>,
    },
    ConfirmDeleteFolder {
        folder_id: String,
        name: String,
        count: usize,
    },
    ConfirmClearHistory {
        count: usize,
    },
    MoveToFolder {
        targets: Vec<Uuid>,
        current: String,
        choice: String,
    },
    EditItem(Box<EditModal>),
    Shortcuts,
    About,
}

pub struct EditModal {
    state: ItemEditorState,
    /// Message key of the last failed validation.
    error: Option<&'static str>,
    /// The referrer field takes focus once, when the dialog opens.
    focused: bool,
}

/// What the dialog wants after this frame.
enum Outcome {
    Keep,
    Close,
    Run(Action),
}

impl Modal {
    /// Dialog for `cmd`, or `None` when the command opens none or has nothing
    /// to act on. `targets` are in display order; `focus` is the focused row.
    pub fn for_command(
        cmd: GuiCommand,
        targets: &[Uuid],
        focus: Option<Uuid>,
        tree_folder: Option<&str>,
        snapshot: &Snapshot,
    ) -> Option<Self> {
        // The row the dialog describes: the focused one when it is a target.
        let lead = focus
            .filter(|id| targets.contains(id))
            .or_else(|| targets.first().copied())
            .and_then(|id| find_task(snapshot, id));
        match cmd {
            GuiCommand::Delete if !targets.is_empty() => Some(Modal::ConfirmDelete {
                targets: targets.to_vec(),
            }),
            GuiCommand::DeleteFolderTasks => {
                let folder_id = tree_folder?;
                let name = snapshot
                    .folders
                    .iter()
                    .find(|(id, _)| id == folder_id)
                    .map_or_else(|| folder_id.to_string(), |(_, name)| name.clone());
                Some(Modal::ConfirmDeleteFolder {
                    folder_id: folder_id.to_string(),
                    name,
                    count: snapshot.folder_downloads.get(folder_id).map_or(0, Vec::len),
                })
            }
            GuiCommand::ClearHistory => Some(Modal::ConfirmClearHistory {
                count: snapshot.history.len(),
            }),
            GuiCommand::MoveToFolder => {
                let current = lead?.folder_id.clone();
                Some(Modal::MoveToFolder {
                    targets: targets.to_vec(),
                    choice: current.clone(),
                    current,
                })
            }
            GuiCommand::EditItem => Some(Modal::EditItem(Box::new(EditModal {
                state: ItemEditorState::from_task(lead?, targets.to_vec()),
                error: None,
                focused: false,
            }))),
            GuiCommand::ShowShortcuts => Some(Modal::Shortcuts),
            GuiCommand::About => Some(Modal::About),
            _ => None,
        }
    }
}

/// Draws the open dialog, if any. Returns the action to run once the user
/// confirmed; the dialog is gone from `modal` after that or after a cancel.
pub fn show(
    ctx: &egui::Context,
    tr: &Tr,
    modal: &mut Option<Modal>,
    snapshot: &Snapshot,
) -> Option<Action> {
    let open = modal.as_mut()?;
    let outcome = match open {
        Modal::ConfirmDelete { targets } => {
            let message = if targets.len() == 1 {
                tr.t("confirm-delete-single")
            } else {
                tr.t_args("confirm-delete-multiple", &count_args(targets.len()))
            };
            let targets = targets.clone();
            confirm(ctx, tr, "confirm_delete_modal", &message, || {
                Action::Delete { targets }
            })
        }
        Modal::ConfirmDeleteFolder { folder_id, name, count } => {
            let mut args = count_args(*count);
            args.set("name", name.as_str());
            let message = tr.t_args("gui-confirm-delete-folder", &args);
            let folder_id = folder_id.clone();
            confirm(ctx, tr, "confirm_delete_folder_modal", &message, || {
                Action::DeleteFolderTasks(folder_id)
            })
        }
        Modal::ConfirmClearHistory { count } => {
            let message = tr.t_args("gui-confirm-clear-history", &count_args(*count));
            confirm(ctx, tr, "confirm_clear_history_modal", &message, || {
                Action::ClearHistory
            })
        }
        Modal::MoveToFolder { targets, current, choice } => {
            move_dialog(ctx, tr, snapshot, targets, current, choice)
        }
        Modal::EditItem(edit) => edit_dialog(ctx, tr, edit),
        Modal::Shortcuts => info_dialog(ctx, tr, "shortcuts_modal", "gui-shortcuts-title", |ui| {
            ui.label(tr.t("gui-shortcuts-placeholder"));
        }),
        Modal::About => info_dialog(ctx, tr, "about_modal", "gui-about-title", |ui| {
            ui.label(RichText::new(tr.t("gui-app-title")).strong());
            let mut args = FluentArgs::new();
            args.set("version", env!("CARGO_PKG_VERSION"));
            ui.label(tr.t_args("gui-about-version", &args));
            ui.label(tr.t("gui-about-description"));
        }),
    };
    match outcome {
        Outcome::Keep => None,
        Outcome::Close => {
            *modal = None;
            None
        }
        Outcome::Run(action) => {
            *modal = None;
            Some(action)
        }
    }
}

fn count_args(count: usize) -> FluentArgs<'static> {
    let mut args = FluentArgs::new();
    args.set("count", count);
    args
}

/// Runs `content` in a modal and turns Esc and a click on the backdrop into a
/// cancel.
fn dialog(
    ctx: &egui::Context,
    id: &'static str,
    title: String,
    content: impl FnOnce(&mut Ui) -> Outcome,
) -> Outcome {
    let response = egui::Modal::new(Id::unique(id)).show(ctx, |ui| {
        ui.set_width(DIALOG_WIDTH);
        ui.label(RichText::new(title).strong());
        ui.separator();
        content(ui)
    });
    let cancelled = response.should_close();
    match response.inner {
        Outcome::Keep if cancelled => Outcome::Close,
        other => other,
    }
}

/// Enter is the default answer of a dialog without a text field.
fn enter_pressed(ctx: &egui::Context) -> bool {
    ctx.input_mut(|input| input.consume_key(Modifiers::NONE, Key::Enter))
}

fn confirm(
    ctx: &egui::Context,
    tr: &Tr,
    id: &'static str,
    message: &str,
    action: impl FnOnce() -> Action,
) -> Outcome {
    let enter = enter_pressed(ctx);
    dialog(ctx, id, tr.t("gui-confirm-title"), |ui| {
        ui.add(egui::Label::new(message).wrap());
        ui.add_space(ui.spacing().item_spacing.y);
        let mut outcome = Outcome::Keep;
        ui.horizontal(|ui| {
            if ui.button(tr.t("gui-button-delete")).clicked() || enter {
                outcome = Outcome::Run(action());
            } else if ui.button(tr.t("gui-button-cancel")).clicked() {
                outcome = Outcome::Close;
            }
        });
        outcome
    })
}

fn move_dialog(
    ctx: &egui::Context,
    tr: &Tr,
    snapshot: &Snapshot,
    targets: &[Uuid],
    current: &str,
    choice: &mut String,
) -> Outcome {
    let enter = enter_pressed(ctx);
    dialog(ctx, "move_to_folder_modal", tr.t("gui-move-title"), |ui| {
        ScrollArea::vertical()
            .id_salt("move_folder_list")
            .max_height(LIST_MAX_HEIGHT)
            .auto_shrink([false, true])
            .show(ui, |ui| {
                ui.with_layout(
                    egui::Layout::top_down_justified(egui::Align::LEFT),
                    |ui| {
                        for (id, name) in &snapshot.folders {
                            let label = if id == current {
                                format!("{name} {}", tr.t("gui-move-current"))
                            } else {
                                name.clone()
                            };
                            let selected = id == choice;
                            if ui
                                .add(Button::selectable(selected, (label, egui::Atom::grow())))
                                .clicked()
                            {
                                *choice = id.clone();
                            }
                        }
                    },
                );
            });
        ui.add_space(ui.spacing().item_spacing.y);
        let mut outcome = Outcome::Keep;
        let can_move = choice.as_str() != current;
        ui.horizontal(|ui| {
            let move_clicked = ui
                .add_enabled(can_move, Button::new(tr.t("gui-button-move")))
                .clicked();
            if move_clicked || (enter && can_move) {
                outcome = Outcome::Run(Action::MoveToFolder {
                    targets: targets.to_vec(),
                    folder_id: choice.clone(),
                });
            } else if ui.button(tr.t("gui-button-cancel")).clicked() {
                outcome = Outcome::Close;
            }
        });
        outcome
    })
}

fn edit_dialog(ctx: &egui::Context, tr: &Tr, edit: &mut EditModal) -> Outcome {
    let title = if edit.state.targets.len() > 1 {
        tr.t_args("dialog-item-edit-multi", &count_args(edit.state.targets.len()))
    } else {
        tr.t("dialog-item-edit")
    };
    dialog(ctx, "edit_item_modal", title, |ui| {
        let mut submit = false;
        egui::Grid::new("edit_item_grid")
            .num_columns(2)
            .show(ui, |ui| {
                let label = ui.label(tr.t("dialog-item-edit-save-path"));
                let response = ui.add(
                    TextEdit::singleline(&mut edit.state.save_path.value)
                        .id_salt("edit_item_save_path")
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
                submit |= entered(ui, &response);
                ui.end_row();

                // A multi-selection has no single name to show or edit.
                if edit.state.targets.len() == 1 {
                    let label = ui.label(tr.t("gui-edit-filename"));
                    if edit.state.filename_editable {
                        let response = ui.add(
                            TextEdit::singleline(&mut edit.state.filename.value)
                                .id_salt("edit_item_filename")
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(label.id);
                        submit |= entered(ui, &response);
                    } else {
                        // Shown from a copy so the stored value stays the
                        // current name.
                        let mut shown = edit.state.filename.value.clone();
                        ui.add_enabled(
                            false,
                            TextEdit::singleline(&mut shown)
                                .id_salt("edit_item_filename")
                                .desired_width(f32::INFINITY),
                        )
                        .labelled_by(label.id)
                        .on_disabled_hover_text(tr.t("gui-edit-filename-locked"));
                    }
                    ui.end_row();
                }

                let label = ui.label(tr.t("dialog-item-edit-referrer"));
                let response = ui.add(
                    TextEdit::singleline(&mut edit.state.referrer.value)
                        .id_salt("edit_item_referrer")
                        .hint_text(tr.t("dialog-item-edit-referrer-hint"))
                        .desired_width(f32::INFINITY),
                )
                .labelled_by(label.id);
                if !edit.focused {
                    response.request_focus();
                    edit.focused = true;
                }
                submit |= entered(ui, &response);
                ui.end_row();
            });

        if !edit.state.other_headers.is_empty() {
            ui.label(tr.t("dialog-item-edit-headers"));
            ScrollArea::vertical()
                .id_salt("edit_item_headers")
                .max_height(HEADERS_MAX_HEIGHT)
                .auto_shrink([false, true])
                .show(ui, |ui| {
                    for (name, value) in &edit.state.other_headers {
                        ui.add(egui::Label::new(format!("{name}: {value}")).wrap());
                    }
                });
        }
        if let Some(key) = edit.error {
            ui.colored_label(ui.visuals().error_fg_color, tr.t(key));
        }

        ui.add_space(ui.spacing().item_spacing.y);
        let mut outcome = Outcome::Keep;
        ui.horizontal(|ui| {
            if ui.button(tr.t("gui-button-ok")).clicked() {
                submit = true;
            }
            if ui.button(tr.t("gui-button-cancel")).clicked() {
                outcome = Outcome::Close;
            }
        });
        if submit {
            outcome = submit_edit(edit);
        }
        outcome
    })
}

/// Enter leaves a single-line field with the focus lost.
fn entered(ui: &Ui, response: &egui::Response) -> bool {
    response.lost_focus() && ui.input(|input| input.key_pressed(Key::Enter))
}

fn submit_edit(edit: &mut EditModal) -> Outcome {
    edit.error = None;
    if let Err(key) = edit.state.validate() {
        edit.error = Some(key);
        return Outcome::Keep;
    }
    let header_edit = edit.state.header_edit();
    let new_save_path = edit.state.new_save_path();
    let new_filename = edit.state.new_filename();
    if header_edit.is_none() && new_save_path.is_none() && new_filename.is_none() {
        return Outcome::Close;
    }
    Outcome::Run(Action::EditItems {
        targets: edit.state.targets.clone(),
        header_edit,
        new_save_path,
        new_filename,
    })
}

fn info_dialog(
    ctx: &egui::Context,
    tr: &Tr,
    id: &'static str,
    title_key: &str,
    body: impl FnOnce(&mut Ui),
) -> Outcome {
    let enter = enter_pressed(ctx);
    dialog(ctx, id, tr.t(title_key), |ui| {
        body(ui);
        ui.add_space(ui.spacing().item_spacing.y);
        if ui.button(tr.t("gui-button-close")).clicked() || enter {
            Outcome::Close
        } else {
            Outcome::Keep
        }
    })
}

#[cfg(test)]
mod edit_item {
    use super::*;
    use ggg::{app::config::Config, download::task::{DownloadStatus, DownloadTask}};
    use std::{collections::HashMap, path::PathBuf, sync::Arc};

    fn snapshot_with(task: DownloadTask) -> Snapshot {
        let mut folder_downloads = HashMap::new();
        folder_downloads.insert("f1".to_string(), vec![task]);
        Snapshot {
            revision: 1,
            config: Arc::new(Config::default()),
            folders: vec![("f1".to_string(), "Downloads".to_string())],
            default_folder_id: "f1".to_string(),
            folder_downloads,
            history: Vec::new(),
        }
    }

    fn open(task: DownloadTask) -> Box<EditModal> {
        let id = task.id;
        let snapshot = snapshot_with(task);
        match Modal::for_command(GuiCommand::EditItem, &[id], Some(id), None, &snapshot) {
            Some(Modal::EditItem(edit)) => edit,
            _ => panic!("expected the edit dialog"),
        }
    }

    fn task() -> DownloadTask {
        let mut task = DownloadTask::new("https://host/a.bin".to_string(), PathBuf::from("."));
        task.folder_id = "f1".to_string();
        task
    }

    #[test]
    fn filename_field_editable_for_unstarted_task() {
        let edit = open(task());
        assert!(edit.state.filename_editable);
        assert_eq!(edit.state.filename.value, "a.bin");
    }

    #[test]
    fn filename_field_locked_for_started_task() {
        for started in [
            |t: &mut DownloadTask| t.status = DownloadStatus::Downloading,
            |t: &mut DownloadTask| t.downloaded = 1,
            |t: &mut DownloadTask| t.filename_settled = true,
        ] {
            let mut task = task();
            started(&mut task);
            let mut edit = open(task);
            assert!(!edit.state.filename_editable);
            edit.state.filename.value = "other.bin".to_string();
            // A locked field never leaves the dialog, even if its value changed.
            match submit_edit(&mut edit) {
                Outcome::Close => {}
                _ => panic!("a locked name must not produce an action"),
            }
        }
    }
}
