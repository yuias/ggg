//! Context menus of the download rows, the table background and the folder tree.

use crate::{
    commands::{CommandContext, GuiCommand, is_enabled},
    i18n::Tr,
};
use egui::{Button, Ui};

/// Message id of the context-menu label for `cmd`.
fn label_key(cmd: GuiCommand) -> &'static str {
    match cmd {
        GuiCommand::StartPause => "gui-cmd-start-pause",
        GuiCommand::Retry => "gui-cmd-retry",
        GuiCommand::EditItem => "gui-cmd-edit-item",
        GuiCommand::MoveToFolder => "gui-cmd-move-to-folder",
        GuiCommand::CopyUrls => "gui-cmd-copy-urls",
        GuiCommand::OpenContainingFolder => "gui-cmd-open-containing-folder",
        GuiCommand::Delete => "gui-cmd-delete",
        GuiCommand::PasteUrls => "gui-cmd-paste-urls",
        GuiCommand::StartFolder => "gui-cmd-start-folder",
        GuiCommand::StopFolder => "gui-cmd-stop-folder",
        GuiCommand::DeleteFolderTasks => "gui-cmd-delete-folder-tasks",
        GuiCommand::ClearHistory => "gui-cmd-clear-history",
        GuiCommand::FolderSettings => "gui-cmd-folder-settings",
        other => unreachable!("{other:?} has no context-menu entry"),
    }
}

/// Adds one entry; a click requests `cmd` and closes the menu. The caller runs
/// the request after the frame, once the selection it applies to is settled.
fn entry(ui: &mut Ui, tr: &Tr, ctx: &CommandContext, cmd: GuiCommand, out: &mut Vec<GuiCommand>) {
    let label = tr.t(label_key(cmd));
    if ui.add_enabled(is_enabled(cmd, ctx), Button::new(label)).clicked() {
        out.push(cmd);
        ui.close();
    }
}

/// Menu of a download row. `ctx` describes the rows the menu applies to,
/// which for an unselected row is that row alone.
pub fn row_menu(ui: &mut Ui, tr: &Tr, ctx: &CommandContext, out: &mut Vec<GuiCommand>) {
    if !ctx.view_is_history {
        entry(ui, tr, ctx, GuiCommand::StartPause, out);
        entry(ui, tr, ctx, GuiCommand::Retry, out);
        ui.separator();
        entry(ui, tr, ctx, GuiCommand::EditItem, out);
        entry(ui, tr, ctx, GuiCommand::MoveToFolder, out);
        ui.separator();
    }
    entry(ui, tr, ctx, GuiCommand::CopyUrls, out);
    entry(ui, tr, ctx, GuiCommand::OpenContainingFolder, out);
    if !ctx.view_is_history {
        ui.separator();
        entry(ui, tr, ctx, GuiCommand::Delete, out);
    }
}

/// Menu of the empty part of the queue table.
pub fn background_menu(ui: &mut Ui, tr: &Tr, ctx: &CommandContext, out: &mut Vec<GuiCommand>) {
    entry(ui, tr, ctx, GuiCommand::PasteUrls, out);
}

/// Menu of a tree node. `ctx.tree_folder` is the folder of that node, or
/// `None` for the Completed node.
pub fn tree_menu(ui: &mut Ui, tr: &Tr, ctx: &CommandContext, out: &mut Vec<GuiCommand>) {
    if ctx.view_is_history {
        entry(ui, tr, ctx, GuiCommand::ClearHistory, out);
        return;
    }
    entry(ui, tr, ctx, GuiCommand::StartFolder, out);
    entry(ui, tr, ctx, GuiCommand::StopFolder, out);
    entry(ui, tr, ctx, GuiCommand::DeleteFolderTasks, out);
    ui.separator();
    entry(ui, tr, ctx, GuiCommand::FolderSettings, out);
}
