//! Commands that menus, shortcuts and context menus trigger, and when each
//! one is available.

use crate::bridge::snapshot::Snapshot;
use ggg::download::task::DownloadTask;
use uuid::Uuid;

// The menu bar and the shortcut table construct the variants that no context
// menu does.
#[allow(dead_code)]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum GuiCommand {
    NewDownload,
    PasteUrls,
    PasteUrlsDirect,
    OpenSettings,
    Exit,
    Undo,
    CopyUrls,
    SelectAll,
    SelectNone,
    Delete,
    Find,
    StartPause,
    Retry,
    EditItem,
    MoveToFolder,
    OpenContainingFolder,
    ResumeAll,
    PauseAll,
    StartFolder,
    StopFolder,
    DeleteFolderTasks,
    ClearHistory,
    FolderSettings,
    ToggleDetails,
    Refresh,
    ShowShortcuts,
    About,
}

/// What `is_enabled` needs to know about the window.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct CommandContext {
    /// The Completed node is shown; its rows are no longer in the queue.
    pub view_is_history: bool,
    pub has_targets: bool,
    pub has_error_target: bool,
    pub undo_depth: usize,
    /// An OS-modal window or an in-window modal is open.
    pub modal_open: bool,
    /// Folder of the tree node a folder command would act on.
    pub tree_folder: Option<String>,
}

/// Single source for menu items, context-menu entries and shortcuts, so a
/// command that arrives late (a queued menu or key event) is still refused
/// while a modal is open.
pub fn is_enabled(cmd: GuiCommand, ctx: &CommandContext) -> bool {
    use GuiCommand::*;
    if ctx.modal_open {
        return false;
    }
    match cmd {
        NewDownload | PasteUrls | PasteUrlsDirect | OpenSettings | Exit | SelectAll
        | SelectNone | Find | ResumeAll | PauseAll | ToggleDetails | Refresh | ShowShortcuts
        | About => true,
        Undo => ctx.undo_depth > 0,
        // Finished downloads are not in the queue, so these two still work there.
        CopyUrls | OpenContainingFolder => ctx.has_targets,
        Delete | StartPause | EditItem | MoveToFolder => {
            ctx.has_targets && !ctx.view_is_history
        }
        Retry => ctx.has_error_target,
        StartFolder | StopFolder | DeleteFolderTasks | FolderSettings => ctx.tree_folder.is_some(),
        ClearHistory => ctx.view_is_history,
    }
}

/// Task by id in the queue or in the history.
pub fn find_task(snapshot: &Snapshot, id: Uuid) -> Option<&DownloadTask> {
    snapshot
        .folder_downloads
        .values()
        .flatten()
        .chain(&snapshot.history)
        .find(|task| task.id == id)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> CommandContext {
        CommandContext {
            has_targets: true,
            has_error_target: true,
            undo_depth: 1,
            tree_folder: Some("f1".to_string()),
            ..CommandContext::default()
        }
    }

    const ALL: &[GuiCommand] = &[
        GuiCommand::NewDownload,
        GuiCommand::PasteUrls,
        GuiCommand::PasteUrlsDirect,
        GuiCommand::OpenSettings,
        GuiCommand::Exit,
        GuiCommand::Undo,
        GuiCommand::CopyUrls,
        GuiCommand::SelectAll,
        GuiCommand::SelectNone,
        GuiCommand::Delete,
        GuiCommand::Find,
        GuiCommand::StartPause,
        GuiCommand::Retry,
        GuiCommand::EditItem,
        GuiCommand::MoveToFolder,
        GuiCommand::OpenContainingFolder,
        GuiCommand::ResumeAll,
        GuiCommand::PauseAll,
        GuiCommand::StartFolder,
        GuiCommand::StopFolder,
        GuiCommand::DeleteFolderTasks,
        GuiCommand::ClearHistory,
        GuiCommand::FolderSettings,
        GuiCommand::ToggleDetails,
        GuiCommand::Refresh,
        GuiCommand::ShowShortcuts,
        GuiCommand::About,
    ];

    #[test]
    fn retry_enabled_only_with_error_target() {
        let mut ctx = ready();
        assert!(is_enabled(GuiCommand::Retry, &ctx));
        ctx.has_error_target = false;
        assert!(!is_enabled(GuiCommand::Retry, &ctx));
    }

    #[test]
    fn undo_disabled_when_stack_empty() {
        let mut ctx = ready();
        assert!(is_enabled(GuiCommand::Undo, &ctx));
        ctx.undo_depth = 0;
        assert!(!is_enabled(GuiCommand::Undo, &ctx));
    }

    #[test]
    fn everything_disabled_while_modal_open() {
        let mut ctx = ready();
        ctx.view_is_history = true;
        ctx.modal_open = true;
        for cmd in ALL {
            assert!(!is_enabled(*cmd, &ctx), "{cmd:?} must be disabled");
        }
        // Control: the same context without the modal enables most of them.
        ctx.modal_open = false;
        assert!(ALL.iter().any(|cmd| is_enabled(*cmd, &ctx)));
    }

    #[test]
    fn queue_only_commands_are_disabled_on_the_completed_node() {
        let mut ctx = ready();
        ctx.view_is_history = true;
        assert!(!is_enabled(GuiCommand::Delete, &ctx));
        assert!(!is_enabled(GuiCommand::StartPause, &ctx));
        assert!(is_enabled(GuiCommand::CopyUrls, &ctx));
        assert!(is_enabled(GuiCommand::ClearHistory, &ctx));
        ctx.view_is_history = false;
        assert!(!is_enabled(GuiCommand::ClearHistory, &ctx));
    }

    #[test]
    fn folder_commands_need_a_tree_folder() {
        let mut ctx = ready();
        assert!(is_enabled(GuiCommand::StartFolder, &ctx));
        ctx.tree_folder = None;
        assert!(!is_enabled(GuiCommand::StartFolder, &ctx));
        assert!(!is_enabled(GuiCommand::DeleteFolderTasks, &ctx));
    }
}
