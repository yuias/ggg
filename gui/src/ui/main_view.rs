//! Main window layout: folder tree, download table, details pane and status bar.

use super::{
    context_menu,
    details::{self, DetailsTab},
    modals::{self, Modal},
    queue_table::{self, TableKind, TableOutput, TableParams},
    selection::{Mods, Selection},
    sort,
};
use crate::{
    bridge::{actions::Action, snapshot::Snapshot},
    commands::{CommandContext, GuiCommand, is_enabled},
    gui_config::GuiConfig,
    i18n::Tr,
    shortcuts,
};
use egui::{Button, CursorIcon, Frame, Key, Panel, Sense, Stroke, Ui};
use fluent_bundle::FluentArgs;
use ggg::{
    download::task::{DownloadStatus, DownloadTask},
    ui::{
        queue_ops::open_in_file_manager,
        queue_view::{QueueView, view_rows},
    },
    util::format::format_speed,
};
use uuid::Uuid;

const TREE_MIN_WIDTH: f32 = 120.0;
/// The tree never takes more than this share of the window.
const TREE_MAX_FRACTION: f32 = 0.5;
const DETAILS_MIN_HEIGHT: f32 = 80.0;
/// Room that stays for the table when the details pane is dragged up.
const TABLE_MIN_HEIGHT: f32 = 120.0;
const SPLITTER_THICKNESS: f32 = 6.0;

/// What the view cannot know by itself.
#[derive(Clone, Copy, Debug, Default)]
pub struct ViewEnv {
    /// Delete batches the app can restore.
    pub undo_depth: usize,
    /// An OS-modal window (add dialog, settings) is open.
    pub os_modal_open: bool,
}

/// What the app has to run after a frame.
#[derive(Default)]
pub struct ViewOutput {
    /// Commands that need the core or the window; the view already handled
    /// the others.
    pub commands: Vec<GuiCommand>,
    /// Confirmed dialog results.
    pub actions: Vec<Action>,
}

/// View state that is not persisted.
#[derive(Default)]
pub struct MainView {
    /// `None` until the first snapshot names the default folder.
    view: Option<QueueView>,
    /// Last regular folder; still the add target while Completed is shown.
    target_folder: Option<String>,
    selection: Selection,
    /// Whether the table (not the tree or the search box) gets navigation keys.
    table_active: bool,
    /// Rows per PgUp/PgDn step, measured by the last drawn table.
    page_rows: usize,
    /// Row to scroll into view after a keyboard move.
    scroll_to: Option<Uuid>,
    search: String,
    tab: DetailsTab,
    modal: Option<Modal>,
    env: ViewEnv,
    /// Commands requested during the frame, routed once it is drawn.
    pending: Vec<GuiCommand>,
    /// Row ids in display order, as of the last frame.
    ids: Vec<Uuid>,
    has_targets: bool,
    has_error_target: bool,
    /// Move the keyboard focus to the history search box.
    focus_search: bool,
}

impl MainView {
    /// Folder that new downloads go to.
    // Read by the add flows.
    #[allow(dead_code)]
    pub fn target_folder(&self) -> Option<&str> {
        self.target_folder.as_deref()
    }

    /// Rows an action applies to: the selection, or the focused row.
    pub fn targets(&self) -> Vec<Uuid> {
        self.selection.targets(&self.ids)
    }

    /// The focused row, which start/pause uses to decide between the two.
    pub fn focus(&self) -> Option<Uuid> {
        self.selection.focus()
    }

    /// Folder of the selected tree node; `None` on the Completed node.
    pub fn tree_folder(&self) -> Option<&str> {
        match &self.view {
            Some(QueueView::Folder(id)) => Some(id),
            _ => None,
        }
    }

    pub fn command_context(&self) -> CommandContext {
        CommandContext {
            view_is_history: self.view == Some(QueueView::History),
            has_targets: self.has_targets,
            has_error_target: self.has_error_target,
            undo_depth: self.env.undo_depth,
            modal_open: self.modal.is_some() || self.env.os_modal_open,
            tree_folder: self.tree_folder().map(str::to_string),
        }
    }

    /// Runs `cmd` if it only concerns the view (selection, search, details
    /// pane, dialogs). Returns the command back when the app has to run it,
    /// and `None` when it was handled or is not enabled.
    pub fn route(
        &mut self,
        cmd: GuiCommand,
        snapshot: &Snapshot,
        config: &mut GuiConfig,
    ) -> Option<GuiCommand> {
        if !is_enabled(cmd, &self.command_context()) {
            return None;
        }
        match cmd {
            GuiCommand::SelectAll => self.selection.select_all(&self.ids),
            GuiCommand::SelectNone => {
                self.selection.clear();
                self.search.clear();
            }
            GuiCommand::Find => {
                self.view = Some(QueueView::History);
                self.selection.clear();
                self.focus_search = true;
            }
            GuiCommand::ToggleDetails => config.details_visible = !config.details_visible,
            GuiCommand::Delete
            | GuiCommand::MoveToFolder
            | GuiCommand::EditItem
            | GuiCommand::DeleteFolderTasks
            | GuiCommand::ClearHistory
            | GuiCommand::ShowShortcuts
            | GuiCommand::About => {
                self.modal = Modal::for_command(
                    cmd,
                    &self.targets(),
                    self.selection.focus(),
                    self.tree_folder(),
                    snapshot,
                );
            }
            other => return Some(other),
        }
        None
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        snapshot: &Snapshot,
        tr: &Tr,
        config: &mut GuiConfig,
        status: Option<&str>,
        env: ViewEnv,
    ) -> ViewOutput {
        self.env = env;
        self.normalize(snapshot);

        let mut rows = match &self.view {
            Some(view) => view_rows(
                &snapshot.folder_downloads,
                &snapshot.history,
                view,
                &self.search,
            ),
            None => Vec::new(),
        };
        // Sorting is display-only. Selection and keyboard navigation use the
        // displayed order, so the sort comes first.
        let kind = self.table_kind();
        if let Some(state) = config
            .sort
            .as_ref()
            .filter(|state| queue_table::has_column(kind, &state.column))
        {
            sort::sort_rows(&mut rows, &state.column, state.ascending);
        }
        let ids: Vec<Uuid> = rows.iter().map(|task| task.id).collect();
        self.selection.retain(&ids);
        self.refresh_target_state(kind, &rows, &ids);
        self.handle_keys(ui, kind, &ids, &rows, config);
        let pressed = shortcuts::poll(ui.ctx(), &self.command_context(), self.table_active);
        self.pending.extend(pressed);
        let summary = status_summary(snapshot, rows.len(), self.selection.len(), tr);

        Panel::bottom("status_bar").show(ui, |ui| {
            ui.horizontal(|ui| {
                if let Some(status) = status {
                    ui.label(status);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(summary);
                });
            });
        });

        let tree_max = (ui.available_width() * TREE_MAX_FRACTION).max(TREE_MIN_WIDTH);
        let tree_width = config.tree_width.clamp(TREE_MIN_WIDTH, tree_max);
        Panel::left("tree_panel")
            .resizable(false)
            .exact_size(tree_width)
            .show_separator_line(false)
            .frame(Frame::central_panel(ui.style()))
            .show(ui, |ui| self.tree(ui, snapshot, tr));
        let drag = splitter(ui, SplitterAxis::Vertical);
        if drag != 0.0 {
            config.tree_width = (tree_width + drag).clamp(TREE_MIN_WIDTH, tree_max);
        }

        // The details pane sits under the table only, so the tree keeps the
        // full height like the folder pane of a file manager.
        Frame::NONE.show(ui, |ui| {
            if config.details_visible {
                let details_max =
                    (ui.available_height() - TABLE_MIN_HEIGHT).max(DETAILS_MIN_HEIGHT);
                let details_height = config.details_height.clamp(DETAILS_MIN_HEIGHT, details_max);
                let focused = self
                    .selection
                    .focus()
                    .and_then(|id| rows.iter().find(|task| task.id == id).copied());
                Panel::bottom("details_panel")
                    .resizable(false)
                    .exact_size(details_height)
                    .show_separator_line(false)
                    .frame(Frame::central_panel(ui.style()))
                    .show(ui, |ui| details::show(ui, tr, focused, &mut self.tab));
                // Declared after the details panel so it sits above it.
                let drag = splitter(ui, SplitterAxis::Horizontal);
                if drag != 0.0 {
                    config.details_height =
                        (details_height - drag).clamp(DETAILS_MIN_HEIGHT, details_max);
                }
            }
            egui::CentralPanel::default()
                .show(ui, |ui| self.table(ui, tr, kind, &rows, &ids, config));
        });

        let mut output = ViewOutput::default();
        if let Some(action) = modals::show(ui.ctx(), tr, &mut self.modal, snapshot) {
            output.actions.push(action);
        }
        let pending = std::mem::take(&mut self.pending);
        if !pending.is_empty() || !output.actions.is_empty() {
            // Opening or closing a dialog changes the next frame, which no
            // input event is going to ask for.
            ui.ctx().request_repaint();
        }
        for cmd in pending {
            if let Some(core) = self.route(cmd, snapshot, config) {
                output.commands.push(core);
            }
        }
        output
    }

    /// Caches what the command gates need, so commands that arrive between
    /// frames (menu, shortcuts) can be checked without the rows.
    fn refresh_target_state(&mut self, kind: TableKind, rows: &[&DownloadTask], ids: &[Uuid]) {
        self.ids = ids.to_vec();
        let targets = self.selection.targets(ids);
        self.has_targets = !targets.is_empty();
        self.has_error_target = kind == TableKind::Queue
            && rows
                .iter()
                .any(|task| task.status == DownloadStatus::Error && targets.contains(&task.id));
    }

    /// Keeps the selected node valid when folders appear or disappear.
    fn normalize(&mut self, snapshot: &Snapshot) {
        let known = |id: &str| snapshot.folders.iter().any(|(f, _)| f == id);
        let default_valid = known(&snapshot.default_folder_id);

        let view_valid = match &self.view {
            Some(QueueView::Folder(id)) => known(id),
            Some(QueueView::History) => true,
            None => false,
        };
        if !view_valid && default_valid {
            self.view = Some(QueueView::Folder(snapshot.default_folder_id.clone()));
            self.selection.clear();
        }
        match &self.view {
            Some(QueueView::Folder(id)) => self.target_folder = Some(id.clone()),
            Some(QueueView::History) => {
                // The remembered folder may be gone; fall back to the default.
                if !self.target_folder.as_deref().is_some_and(known) && default_valid {
                    self.target_folder = Some(snapshot.default_folder_id.clone());
                }
            }
            None => {}
        }
    }

    fn tree(&mut self, ui: &mut Ui, snapshot: &Snapshot, tr: &Tr) {
        let base = self.command_context();
        egui::ScrollArea::vertical()
            .id_salt("tree_scroll")
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.with_layout(
                    egui::Layout::top_down_justified(egui::Align::LEFT),
                    |ui| {
                        for (id, name) in &snapshot.folders {
                            let count = snapshot.folder_downloads.get(id).map_or(0, Vec::len);
                            let node = QueueView::Folder(id.clone());
                            let text = tr.t_args(
                                "gui-tree-folder",
                                &count_args(Some(name.as_str()), count),
                            );
                            self.tree_node(ui, tr, &base, node, text);
                        }
                        let text = tr.t_args(
                            "gui-tree-completed",
                            &count_args(None, snapshot.history.len()),
                        );
                        self.tree_node(ui, tr, &base, QueueView::History, text);
                    },
                );
            });
    }

    fn tree_node(
        &mut self,
        ui: &mut Ui,
        tr: &Tr,
        base: &CommandContext,
        node: QueueView,
        text: String,
    ) {
        let selected = self.view.as_ref() == Some(&node);
        // The trailing grow atom left-aligns the text inside the full-width button.
        let button = Button::selectable(selected, (text, egui::Atom::grow()));
        let response = ui.add(button);
        // A right click selects the node too, so the folder commands of its
        // menu act on the node that was clicked.
        if response.clicked() || response.secondary_clicked() {
            self.table_active = false;
            if !selected {
                self.view = Some(node.clone());
                self.selection.clear();
            }
        }
        let mut menu = base.clone();
        match &node {
            QueueView::Folder(id) => {
                menu.view_is_history = false;
                menu.tree_folder = Some(id.clone());
            }
            QueueView::History => {
                menu.view_is_history = true;
                menu.tree_folder = None;
            }
        }
        response.context_menu(|ui| context_menu::tree_menu(ui, tr, &menu, &mut self.pending));
    }

    fn table_kind(&self) -> TableKind {
        match self.view {
            Some(QueueView::History) => TableKind::History,
            _ => TableKind::Queue,
        }
    }

    fn table(
        &mut self,
        ui: &mut Ui,
        tr: &Tr,
        kind: TableKind,
        rows: &[&DownloadTask],
        ids: &[Uuid],
        config: &mut GuiConfig,
    ) {
        if kind == TableKind::History {
            let search = ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .id_salt("history_search")
                    .hint_text(tr.t("gui-history-search-hint"))
                    .desired_width(f32::INFINITY),
            );
            if std::mem::take(&mut self.focus_search) {
                search.request_focus();
            }
            if search.gained_focus() {
                self.table_active = false;
            }
            ui.add_space(ui.spacing().item_spacing.y);
        }
        let base = self.command_context();
        let mut requested = Vec::new();
        if kind == TableKind::Queue {
            // Declared before the table so its rows stay on top and only the
            // empty area below them reaches this menu.
            let background = ui.interact(
                ui.available_rect_before_wrap(),
                ui.scope_id().with("queue_background"),
                Sense::click(),
            );
            background.context_menu(|ui| {
                context_menu::background_menu(ui, tr, &base, &mut requested);
            });
        }
        let params = TableParams {
            kind,
            rows,
            selection: &self.selection,
            sort: config
                .sort
                .as_ref()
                .filter(|state| queue_table::has_column(kind, &state.column)),
            scroll_to: self.scroll_to.take(),
        };
        let widths = match kind {
            TableKind::Queue => &mut config.queue_columns,
            TableKind::History => &mut config.history_columns,
        };
        // The selection a right click makes is applied after the table is
        // drawn, so the menu works out for itself which rows it acts on.
        let selection = &self.selection;
        let mut row_menu = |ui: &mut Ui, task: &DownloadTask| {
            let mut ctx = base.clone();
            ctx.has_targets = true;
            ctx.has_error_target = if selection.is_selected(task.id) {
                rows.iter().any(|row| {
                    row.status == DownloadStatus::Error && selection.is_selected(row.id)
                })
            } else {
                task.status == DownloadStatus::Error
            };
            context_menu::row_menu(ui, tr, &ctx, &mut requested);
        };
        let output = queue_table::show(ui, tr, &params, widths, &mut row_menu);
        self.pending.append(&mut requested);
        self.apply_table_output(output, kind, rows, ids, config);
    }

    fn apply_table_output(
        &mut self,
        output: TableOutput,
        kind: TableKind,
        rows: &[&DownloadTask],
        ids: &[Uuid],
        config: &mut GuiConfig,
    ) {
        self.page_rows = output.page_rows;
        if let Some(column) = output.header_clicked {
            let current = config
                .sort
                .as_ref()
                .filter(|state| queue_table::has_column(kind, &state.column));
            config.sort = sort::next_sort(current, column);
        }
        if let Some((id, mods)) = output.clicked {
            self.selection.click(id, ids, mods);
            self.table_active = true;
        }
        if let Some(id) = output.secondary_clicked {
            self.selection.secondary_click(id);
            self.table_active = true;
        }
        if let Some(id) = output.activated {
            self.selection.click(id, ids, Mods::default());
            self.table_active = true;
            self.activate(id, kind, rows, config);
        }
    }

    /// Double-click and Enter: a queue row reveals its details, a finished
    /// download opens its folder.
    fn activate(
        &mut self,
        id: Uuid,
        kind: TableKind,
        rows: &[&DownloadTask],
        config: &mut GuiConfig,
    ) {
        match kind {
            TableKind::Queue => config.details_visible = true,
            TableKind::History => {
                if let Some(task) = rows.iter().find(|task| task.id == id) {
                    open_in_file_manager(&task.save_path);
                }
            }
        }
    }

    /// Navigation keys for the table. Handled events are removed from the input
    /// so nothing else reacts to them.
    fn handle_keys(
        &mut self,
        ui: &Ui,
        kind: TableKind,
        ids: &[Uuid],
        rows: &[&DownloadTask],
        config: &mut GuiConfig,
    ) {
        // A focused text field or button owns the keys (egui moves focus with
        // the arrows), so only act while nothing else has focus.
        // A dialog owns Enter and Esc while it is open.
        if !self.table_active
            || self.modal.is_some()
            || self.env.os_modal_open
            || ui.ctx().egui_wants_keyboard_input()
        {
            return;
        }
        let pressed = ui.input_mut(|input| {
            let mut pressed = Vec::new();
            input.events.retain(|event| {
                let egui::Event::Key { key, pressed: true, modifiers, .. } = event else {
                    return true;
                };
                let handled = !modifiers.alt
                    && match key {
                        Key::ArrowUp | Key::ArrowDown | Key::PageUp | Key::PageDown => true,
                        Key::Home | Key::End | Key::Enter => true,
                        Key::Space => modifiers.command,
                        _ => false,
                    };
                if handled {
                    pressed.push((*key, *modifiers));
                }
                !handled
            });
            pressed
        });

        let page = self.page_rows.max(1) as isize;
        for (key, modifiers) in pressed {
            let mods = Mods { ctrl: modifiers.command, shift: modifiers.shift };
            match key {
                Key::ArrowUp => self.selection.move_focus(-1, ids, mods),
                Key::ArrowDown => self.selection.move_focus(1, ids, mods),
                Key::PageUp => self.selection.move_focus(-page, ids, mods),
                Key::PageDown => self.selection.move_focus(page, ids, mods),
                Key::Home => self.selection.focus_edge(false, ids, mods),
                Key::End => self.selection.focus_edge(true, ids, mods),
                Key::Space => self.selection.toggle_focused(),
                Key::Enter => {
                    if let Some(id) = self.selection.focus() {
                        self.activate(id, kind, rows, config);
                    }
                }
                _ => {}
            }
            self.scroll_to = self.selection.focus();
        }
    }
}

fn count_args<'a>(name: Option<&'a str>, count: usize) -> FluentArgs<'a> {
    let mut args = FluentArgs::new();
    if let Some(name) = name {
        args.set("name", name);
    }
    args.set("count", count);
    args
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum SplitterAxis {
    /// A vertical bar between side-by-side panes; drags change x.
    Vertical,
    /// A horizontal bar between stacked panes; drags change y.
    Horizontal,
}

/// Draws a splitter panel and returns the drag distance along its axis in
/// points. The caller owns the size, so it persists in `GuiConfig` rather than
/// in egui panel memory.
fn splitter(ui: &mut Ui, axis: SplitterAxis) -> f32 {
    let (panel, id) = match axis {
        SplitterAxis::Vertical => (Panel::left("tree_splitter"), "tree_splitter_grip"),
        SplitterAxis::Horizontal => (Panel::bottom("details_splitter"), "details_splitter_grip"),
    };
    panel
        .resizable(false)
        .exact_size(SPLITTER_THICKNESS)
        .show_separator_line(false)
        .frame(Frame::NONE)
        .show(ui, |ui| {
            let rect = ui.available_rect_before_wrap();
            let response = ui.interact(rect, ui.scope_id().with(id), Sense::drag());
            if response.hovered() || response.dragged() {
                ui.ctx().set_cursor_icon(match axis {
                    SplitterAxis::Vertical => CursorIcon::ResizeHorizontal,
                    SplitterAxis::Horizontal => CursorIcon::ResizeVertical,
                });
            }
            let widgets = &ui.visuals().widgets;
            let stroke: Stroke = if response.dragged() {
                widgets.active.bg_stroke
            } else if response.hovered() {
                widgets.hovered.bg_stroke
            } else {
                widgets.noninteractive.bg_stroke
            };
            match axis {
                SplitterAxis::Vertical => {
                    ui.painter().vline(rect.center().x, rect.y_range(), stroke);
                }
                SplitterAxis::Horizontal => {
                    ui.painter().hline(rect.x_range(), rect.center().y, stroke);
                }
            }
            match axis {
                SplitterAxis::Vertical => response.drag_delta().x,
                SplitterAxis::Horizontal => response.drag_delta().y,
            }
        })
        .inner
}

/// Totals shown at the right end of the status bar.
#[derive(Debug, PartialEq)]
struct ActiveStats {
    count: usize,
    bytes_per_sec: f64,
}

fn active_stats(snapshot: &Snapshot) -> ActiveStats {
    let mut stats = ActiveStats {
        count: 0,
        bytes_per_sec: 0.0,
    };
    for task in snapshot.folder_downloads.values().flatten() {
        if task.status == DownloadStatus::Downloading {
            stats.count += 1;
            stats.bytes_per_sec += task.speed().unwrap_or(0.0);
        }
    }
    stats
}

/// Parts are left out when zero, so an idle window shows just the item count.
fn status_summary(snapshot: &Snapshot, shown: usize, selected: usize, tr: &Tr) -> String {
    let count_arg = |count: usize| {
        let mut args = FluentArgs::new();
        args.set("count", count);
        args
    };
    let mut parts = vec![tr.t_args("gui-statusbar-items", &count_arg(shown))];
    if selected > 0 {
        parts.push(tr.t_args("gui-statusbar-selected", &count_arg(selected)));
    }
    let active = active_stats(snapshot);
    if active.count > 0 {
        parts.push(tr.t_args("gui-statusbar-active", &count_arg(active.count)));
        parts.push(format_speed(active.bytes_per_sec));
    }
    parts.join(" \u{b7} ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui_kittest::{Harness, kittest::Queryable};
    use crate::gui_config::{ColumnWidth, SortState};
    use ggg::app::config::Config;
    use std::{
        cell::{Cell, RefCell},
        collections::HashMap,
        path::PathBuf,
        rc::Rc,
        sync::Arc,
    };

    fn task(name: &str, folder: &str) -> DownloadTask {
        let mut task = DownloadTask::new(format!("https://host/{name}"), PathBuf::from("."));
        task.filename = name.to_string();
        task.folder_id = folder.to_string();
        task
    }

    fn sample_snapshot() -> Snapshot {
        let mut folder_downloads = HashMap::new();
        folder_downloads.insert(
            "f1".to_string(),
            vec![task("alpha.iso", "f1"), task("beta.zip", "f1")],
        );
        folder_downloads.insert("f2".to_string(), vec![task("gamma.mp4", "f2")]);
        let mut done = task("old.bin", "f1");
        done.status = DownloadStatus::Completed;
        Snapshot {
            revision: 1,
            config: Arc::new(Config::default()),
            folders: vec![
                ("f1".to_string(), "Downloads".to_string()),
                ("f2".to_string(), "Videos".to_string()),
            ],
            default_folder_id: "f1".to_string(),
            folder_downloads,
            history: vec![done],
        }
    }

    fn harness(snapshot: Snapshot, config: GuiConfig) -> Harness<'static, ()> {
        let tr = Tr::load("en-US");
        let mut view = MainView::default();
        let mut config = config;
        Harness::builder()
            .with_size(egui::vec2(900.0, 600.0))
            .build_ui(move |ui| {
                view.show(ui, &snapshot, &tr, &mut config, None, ViewEnv::default());
            })
    }

    #[test]
    fn shows_folders_completed_node_and_rows() {
        // Three tasks in the default folder; the second folder is empty.
        let mut snapshot = sample_snapshot();
        snapshot.folder_downloads.remove("f2");
        snapshot
            .folder_downloads
            .get_mut("f1")
            .unwrap()
            .push(task("gamma.mp4", "f1"));
        let mut harness = harness(snapshot, GuiConfig::default());
        harness.run();

        harness.get_by_label("Downloads (3)");
        harness.get_by_label("Videos (0)");
        harness.get_by_label("Completed (1)");
        harness.get_by_label("alpha.iso");
        harness.get_by_label("beta.zip");
        harness.get_by_label("gamma.mp4");
    }

    #[test]
    fn default_folder_shows_only_its_own_rows() {
        let mut harness = harness(sample_snapshot(), GuiConfig::default());
        harness.run();

        harness.get_by_label("Downloads (2)");
        harness.get_by_label("Videos (1)");
        harness.get_by_label("alpha.iso");
        harness.get_by_label("beta.zip");
        assert!(harness.query_by_label("gamma.mp4").is_none());
    }

    #[test]
    fn clicking_a_row_fills_the_details_pane() {
        let mut harness = harness(sample_snapshot(), GuiConfig::default());
        harness.run();
        assert!(harness.query_by_label("Save Path:").is_none());

        harness.get_by_label("alpha.iso").click();
        harness.run();
        harness.get_by_label("Save Path:");
        harness.get_by_label("https://host/alpha.iso");
    }

    #[test]
    fn selecting_another_folder_switches_rows() {
        let mut harness = harness(sample_snapshot(), GuiConfig::default());
        harness.run();
        harness.get_by_label("Videos (1)").click();
        harness.run();
        harness.get_by_label("gamma.mp4");
        assert!(harness.query_by_label("alpha.iso").is_none());
    }

    #[test]
    fn completed_node_lists_history() {
        let mut harness = harness(sample_snapshot(), GuiConfig::default());
        harness.run();
        harness.get_by_label("Completed (1)").click();
        harness.run();
        harness.get_by_label("old.bin");
    }

    #[test]
    fn details_hidden_when_config_says_so() {
        let visible = {
            let mut harness = harness(sample_snapshot(), GuiConfig::default());
            harness.run();
            harness.query_by_label("Info").is_some()
        };
        assert!(visible, "details tabs are shown by default");

        let config = GuiConfig {
            details_visible: false,
            ..GuiConfig::default()
        };
        let mut harness = harness(sample_snapshot(), config);
        harness.run();
        assert!(harness.query_by_label("Info").is_none());
        assert!(harness.query_by_label("Log").is_none());
        harness.get_by_label("alpha.iso");
    }

    fn shared_harness(
        snapshot: Snapshot,
        config: Rc<RefCell<GuiConfig>>,
        size: egui::Vec2,
    ) -> Harness<'static, ()> {
        let tr = Tr::load("en-US");
        let mut view = MainView::default();
        Harness::builder()
            .with_size(size)
            .build_ui(move |ui| {
                view.show(ui, &snapshot, &tr, &mut config.borrow_mut(), None, ViewEnv::default());
            })
    }

    fn top_of(harness: &Harness<'_, ()>, label: &str) -> f32 {
        harness.get_by_label(label).rect().top()
    }

    #[test]
    fn ctrl_click_builds_a_multi_selection() {
        let mut snapshot = sample_snapshot();
        snapshot
            .folder_downloads
            .get_mut("f1")
            .unwrap()
            .push(task("delta.txt", "f1"));
        let mut harness = harness(snapshot, GuiConfig::default());
        harness.run();

        harness.get_by_label("alpha.iso").click();
        harness.run();
        harness.get_by_label_contains("1 selected");

        // The modifiers must still be held in the frame that sees the click;
        // `click_modifiers` releases them within that same frame. Like
        // egui-winit on Windows, set `command` together with `ctrl`.
        harness.event(egui::Event::ModifiersChanged(
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        ));
        harness.run();
        harness.get_by_label("delta.txt").click();
        harness.run();
        harness.get_by_label_contains("2 selected");

        harness.event(egui::Event::ModifiersChanged(egui::Modifiers::SHIFT));
        harness.run();
        harness.get_by_label("beta.zip").click();
        harness.run();
        // Shift replaces the selection with the range from the anchor (delta).
        harness.get_by_label_contains("2 selected");
    }

    #[test]
    fn arrow_keys_move_and_extend_the_selection() {
        let mut snapshot = sample_snapshot();
        snapshot
            .folder_downloads
            .get_mut("f1")
            .unwrap()
            .push(task("delta.txt", "f1"));
        let mut harness = harness(snapshot, GuiConfig::default());
        harness.run();

        harness.get_by_label("alpha.iso").click();
        harness.run();
        harness.key_press(egui::Key::ArrowDown);
        harness.run();
        harness.get_by_label_contains("1 selected");
        // Details follow the focused row.
        harness.get_by_label("https://host/beta.zip");

        harness.key_press_modifiers(egui::Modifiers::SHIFT, egui::Key::ArrowDown);
        harness.run();
        harness.get_by_label_contains("2 selected");
        harness.get_by_label("https://host/delta.txt");

        harness.key_press(egui::Key::Home);
        harness.run();
        harness.get_by_label("https://host/alpha.iso");
        harness.get_by_label_contains("1 selected");
    }

    #[test]
    fn header_click_cycles_sort_and_restores_queue_order() {
        let mut snapshot = sample_snapshot();
        // Queue order is the reverse of the alphabetical order.
        snapshot.folder_downloads.get_mut("f1").unwrap().reverse();
        let config = Rc::new(RefCell::new(GuiConfig::default()));
        let mut harness = shared_harness(snapshot, config.clone(), egui::vec2(900.0, 600.0));
        harness.run();
        assert!(top_of(&harness, "beta.zip") < top_of(&harness, "alpha.iso"));

        harness.get_by_label("Filename").click();
        harness.run();
        assert!(top_of(&harness, "alpha.iso") < top_of(&harness, "beta.zip"));
        assert_eq!(
            config.borrow().sort,
            Some(SortState { column: "name".into(), ascending: true })
        );

        harness.get_by_label("Filename").click();
        harness.run();
        assert!(top_of(&harness, "beta.zip") < top_of(&harness, "alpha.iso"));
        assert_eq!(config.borrow().sort.as_ref().map(|s| s.ascending), Some(false));

        // Third click: back to queue order, which here equals the reverse sort.
        harness.get_by_label("Filename").click();
        harness.run();
        assert_eq!(config.borrow().sort, None);
        assert!(top_of(&harness, "beta.zip") < top_of(&harness, "alpha.iso"));
    }

    #[test]
    fn columns_fit_the_pane_without_overwriting_saved_widths() {
        for width in [900.0, 1400.0] {
            let config = Rc::new(RefCell::new(GuiConfig::default()));
            let mut harness =
                shared_harness(sample_snapshot(), config.clone(), egui::vec2(width, 600.0));
            harness.run();

            // The last column must stay inside the window.
            let added = harness.get_by_label("Added").rect();
            assert!(added.left() < width && added.right() <= width, "Added at {added:?}");

            // Layout-driven widths are not the user's choice.
            assert!(config.borrow().queue_columns.is_empty());
        }
    }

    #[test]
    fn dragging_a_column_border_saves_the_width() {
        let config = Rc::new(RefCell::new(GuiConfig::default()));
        let mut harness = shared_harness(sample_snapshot(), config.clone(), egui::vec2(1400.0, 600.0));
        harness.run();

        // Border between Status and Size: one gap-half left of the Size cell.
        let size_left = harness.get_by_label("Size").rect().left();
        let y = harness.get_by_label("Size").rect().center().y;
        let border = egui::pos2(size_left - 4.0, y);
        harness.hover_at(border);
        harness.run();
        harness.event(egui::Event::PointerButton {
            pos: border,
            button: egui::PointerButton::Primary,
            pressed: true,
            modifiers: egui::Modifiers::NONE,
        });
        // The table keeps repainting while a drag lasts, so `run` would not settle.
        harness.run_steps(2);
        for dx in [10.0, 20.0, 30.0, 40.0] {
            harness.event(egui::Event::PointerMoved(border + egui::vec2(dx, 0.0)));
            harness.run_steps(2);
        }
        harness.event(egui::Event::PointerButton {
            pos: border + egui::vec2(40.0, 0.0),
            button: egui::PointerButton::Primary,
            pressed: false,
            modifiers: egui::Modifiers::NONE,
        });
        harness.run_steps(8);

        let status_width = config
            .borrow()
            .queue_columns
            .iter()
            .find(|c| c.id == "status")
            .map(|c| c.width)
            .expect("a dragged column is saved");
        // Filename absorbs the change, so the Status header moved, not Size.
        let status_left = harness.get_by_label("Status").rect().left();
        let expected = harness.get_by_label("Size").rect().left() - status_left - 8.0;
        assert!((status_width - expected).abs() < 2.0, "{status_width} vs {expected}");
        assert!(status_width > 110.0, "the drag did not widen the column: {status_width}");
    }

    #[test]
    fn columns_regain_their_widths_when_the_window_grows_again() {
        let config = Rc::new(RefCell::new(GuiConfig::default()));
        let mut harness = shared_harness(sample_snapshot(), config, egui::vec2(1400.0, 600.0));
        harness.run();
        let status_width = |h: &Harness<'_, ()>| {
            h.get_by_label("Size").rect().left() - h.get_by_label("Status").rect().left() - 8.0
        };
        let wide = status_width(&harness);

        harness.set_size(egui::vec2(900.0, 600.0));
        harness.run();
        harness.run();
        assert!(status_width(&harness) < wide - 10.0, "columns did not shrink");

        harness.set_size(egui::vec2(1400.0, 600.0));
        harness.run();
        harness.run();
        assert!((status_width(&harness) - wide).abs() < 2.0, "columns stayed narrow");
    }

    #[test]
    fn saved_column_widths_are_restored() {
        let config = Rc::new(RefCell::new(GuiConfig {
            queue_columns: vec![ColumnWidth { id: "status".into(), width: 160.0 }],
            ..GuiConfig::default()
        }));
        let mut harness = shared_harness(sample_snapshot(), config, egui::vec2(1400.0, 600.0));
        harness.run();

        let status = harness.get_by_label("Status").rect().left();
        let size = harness.get_by_label("Size").rect().left();
        // Column width plus the gap between columns.
        assert!(
            (size - status - 160.0 - 8.0).abs() < 2.0,
            "status column is {} wide",
            size - status
        );
    }

    #[test]
    fn status_summary_reports_active_downloads() {
        let tr = Tr::load("en-US");
        let mut snapshot = sample_snapshot();
        assert_eq!(status_summary(&snapshot, 2, 0, &tr), "2 items");
        assert_eq!(status_summary(&snapshot, 1, 1, &tr), "1 item \u{b7} 1 selected");

        let running = snapshot.folder_downloads.get_mut("f1").unwrap();
        running[0].status = DownloadStatus::Downloading;
        assert_eq!(active_stats(&snapshot).count, 1);
        assert!(status_summary(&snapshot, 2, 0, &tr).contains("1 active"));
    }

    #[test]
    fn target_folder_survives_switching_to_completed() {
        let snapshot = sample_snapshot();
        let mut view = MainView::default();
        view.normalize(&snapshot);
        assert_eq!(view.target_folder(), Some("f1"));

        view.view = Some(QueueView::Folder("f2".to_string()));
        view.normalize(&snapshot);
        view.view = Some(QueueView::History);
        view.normalize(&snapshot);
        assert_eq!(view.target_folder(), Some("f2"));
    }

    /// A main view with its outputs collected, as `GuiApp` would receive them.
    /// Nothing here reaches the core, so no config isolation is needed.
    struct Rig {
        view: Rc<RefCell<MainView>>,
        actions: Rc<RefCell<Vec<Action>>>,
        commands: Rc<RefCell<Vec<GuiCommand>>>,
        undo_depth: Rc<Cell<usize>>,
    }

    fn rig(snapshot: Snapshot) -> (Harness<'static, ()>, Rig) {
        let rig = Rig {
            view: Rc::new(RefCell::new(MainView::default())),
            actions: Rc::new(RefCell::new(Vec::new())),
            commands: Rc::new(RefCell::new(Vec::new())),
            undo_depth: Rc::new(Cell::new(0)),
        };
        let tr = Tr::load("en-US");
        let mut config = GuiConfig::default();
        let (view, actions, commands, undo_depth) = (
            rig.view.clone(),
            rig.actions.clone(),
            rig.commands.clone(),
            rig.undo_depth.clone(),
        );
        let harness = Harness::builder()
            .with_size(egui::vec2(900.0, 600.0))
            .build_ui(move |ui| {
                let env = ViewEnv { undo_depth: undo_depth.get(), os_modal_open: false };
                let out = view.borrow_mut().show(ui, &snapshot, &tr, &mut config, None, env);
                actions.borrow_mut().extend(out.actions);
                commands.borrow_mut().extend(out.commands);
            });
        (harness, rig)
    }

    fn id_of(snapshot: &Snapshot, folder: &str, index: usize) -> Uuid {
        snapshot.folder_downloads[folder][index].id
    }

    fn secondary_click_at(harness: &mut Harness<'_, ()>, pos: egui::Pos2) {
        harness.hover_at(pos);
        harness.run();
        for pressed in [true, false] {
            harness.event(egui::Event::PointerButton {
                pos,
                button: egui::PointerButton::Secondary,
                pressed,
                modifiers: egui::Modifiers::NONE,
            });
        }
        harness.run();
    }

    #[test]
    fn right_click_row_shows_delete_entry() {
        let (mut harness, _rig) = rig(sample_snapshot());
        harness.run();
        assert!(harness.query_by_label("Delete...").is_none());

        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Delete...");
        harness.get_by_label("Edit...");
        harness.get_by_label("Move to Folder...");
        harness.get_by_label("Copy URLs");
    }

    #[test]
    fn confirm_delete_cancel_keeps_rows() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Delete...").click();
        harness.run();
        harness.get_by_label("Delete this download?");

        harness.get_by_label("Cancel").click();
        harness.run();
        assert!(harness.query_by_label("Delete this download?").is_none());
        harness.get_by_label("alpha.iso");
        harness.get_by_label("beta.zip");
        assert!(rig.actions.borrow().is_empty(), "cancel must not delete");
    }

    #[test]
    fn confirm_delete_runs_the_action_for_every_selected_row() {
        let snapshot = sample_snapshot();
        let (alpha, beta) = (id_of(&snapshot, "f1", 0), id_of(&snapshot, "f1", 1));
        let (mut harness, rig) = rig(snapshot);
        harness.run();

        harness.get_by_label("alpha.iso").click();
        harness.run();
        harness.event(egui::Event::ModifiersChanged(
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
        ));
        harness.run();
        harness.get_by_label("beta.zip").click();
        harness.run();
        harness.event(egui::Event::ModifiersChanged(egui::Modifiers::NONE));
        harness.run();
        // A right click on a selected row keeps the whole selection.
        harness.get_by_label("beta.zip").click_secondary();
        harness.run();
        harness.get_by_label("Delete...").click();
        harness.run();
        harness.get_by_label("Delete 2 downloads?");

        harness.get_by_label("Delete").click();
        harness.run();
        {
            let actions = rig.actions.borrow();
            match actions.as_slice() {
                [Action::Delete { targets }] => assert_eq!(targets, &vec![alpha, beta]),
                _ => panic!("expected one delete action, got {} actions", actions.len()),
            }
        }
        assert!(harness.query_by_label("Delete 2 downloads?").is_none());
    }

    #[test]
    fn enter_confirms_the_delete_dialog() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Delete...").click();
        harness.run();

        harness.key_press(egui::Key::Enter);
        harness.run();
        assert!(matches!(rig.actions.borrow().as_slice(), [Action::Delete { .. }]));
    }

    #[test]
    fn move_dialog_sends_the_chosen_folder() {
        let snapshot = sample_snapshot();
        let alpha = id_of(&snapshot, "f1", 0);
        let (mut harness, rig) = rig(snapshot);
        harness.run();
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Move to Folder...").click();
        harness.run();
        harness.get_by_label_contains("(current)");

        harness.get_by_label("Videos").click();
        harness.run();
        harness.get_by_label("Move").click();
        harness.run();
        let actions = rig.actions.borrow();
        match actions.as_slice() {
            [Action::MoveToFolder { targets, folder_id }] => {
                assert_eq!(targets, &vec![alpha]);
                assert_eq!(folder_id, "f2");
            }
            _ => panic!("expected one move action, got {} actions", actions.len()),
        }
    }

    #[test]
    fn edit_dialog_sends_the_new_referrer() {
        let snapshot = sample_snapshot();
        let alpha = id_of(&snapshot, "f1", 0);
        let (mut harness, rig) = rig(snapshot);
        harness.run();
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Edit...").click();
        harness.run();
        harness.get_by_label("Edit item");

        // The referrer field has the focus when the dialog opens.
        let fields: Vec<_> = harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .collect();
        assert_eq!(fields.len(), 3);
        assert!(fields[2].is_focused());
        fields[2].type_text("https://ref.example/");
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();

        let actions = rig.actions.borrow();
        match actions.as_slice() {
            [Action::EditItems { targets, header_edit, new_save_path, new_filename }] => {
                assert_eq!(targets, &vec![alpha]);
                let referrer = header_edit.as_ref().and_then(|edit| edit.referrer.as_deref());
                assert_eq!(referrer, Some("https://ref.example/"));
                assert!(new_save_path.is_none());
                assert!(new_filename.is_none());
            }
            _ => panic!("expected one edit action, got {} actions", actions.len()),
        }
    }

    /// The second text field of the Edit item dialog is the file name.
    fn replace_name_field(harness: &mut Harness<'_, ()>, text: &str) {
        let field = harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .nth(1)
            .expect("file name field");
        field.click();
        harness.run();
        harness.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        harness.run();
        harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .nth(1)
            .expect("file name field")
            .type_text(text);
        harness.run();
    }

    #[test]
    fn edit_item_dialog_sends_the_new_filename() {
        let snapshot = sample_snapshot();
        let alpha = id_of(&snapshot, "f1", 0);
        let (mut harness, rig) = rig(snapshot);
        harness.run();
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Edit...").click();
        harness.run();

        replace_name_field(&mut harness, "renamed.bin");
        harness.get_by_label("OK").click();
        harness.run();

        let actions = rig.actions.borrow();
        match actions.as_slice() {
            [Action::EditItems { targets, header_edit, new_save_path, new_filename }] => {
                assert_eq!(targets, &vec![alpha]);
                assert!(header_edit.is_none() && new_save_path.is_none());
                assert_eq!(new_filename.as_deref(), Some("renamed.bin"));
            }
            _ => panic!("expected one edit action, got {} actions", actions.len()),
        }
    }

    #[test]
    fn edit_item_dialog_rejects_an_invalid_filename() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Edit...").click();
        harness.run();

        replace_name_field(&mut harness, "/x");
        harness.get_by_label("OK").click();
        harness.run();

        assert!(rig.actions.borrow().is_empty());
        harness.get_by_label_contains("Enter a valid file name");
        harness.get_by_label("Edit item");
    }

    #[test]
    fn edit_item_dialog_locks_the_filename_of_a_started_task() {
        let mut snapshot = sample_snapshot();
        let started = snapshot.folder_downloads.get_mut("f1").unwrap();
        started[0].status = DownloadStatus::Paused;
        started[0].downloaded = 5;
        let (mut harness, rig) = rig(snapshot);
        harness.run();
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Edit...").click();
        harness.run();

        let fields: Vec<_> = harness
            .get_all_by_role(egui::accesskit::Role::TextInput)
            .collect();
        assert_eq!(fields.len(), 3);
        assert!(egui_kittest::kittest::NodeT::accesskit_node(&fields[1]).is_disabled(), "the name field must be read-only");
        harness.get_by_label("OK").click();
        harness.run();
        assert!(rig.actions.borrow().is_empty());
    }

    #[test]
    fn edit_dialog_without_changes_sends_nothing() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Edit...").click();
        harness.run();
        harness.get_by_label("OK").click();
        harness.run();
        assert!(rig.actions.borrow().is_empty());
        assert!(harness.query_by_label("Edit item").is_none());
    }

    #[test]
    fn tree_menu_offers_folder_commands_and_asks_before_deleting() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        harness.get_by_label("Videos (1)").click_secondary();
        harness.run();
        harness.get_by_label("Start All").click();
        harness.run();
        assert_eq!(rig.commands.borrow().as_slice(), &[GuiCommand::StartFolder]);
        // The right click selected the node, so the command acts on it.
        harness.get_by_label("gamma.mp4");

        harness.get_by_label("Videos (1)").click_secondary();
        harness.run();
        harness.get_by_label("Delete All...").click();
        harness.run();
        harness.get_by_label_contains("Delete the 1 download in \"Videos\"?");
        harness.get_by_label("Cancel").click();
        harness.run();
        assert!(rig.actions.borrow().is_empty());
    }

    #[test]
    fn completed_node_menu_clears_history_after_confirmation() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        harness.get_by_label("Completed (1)").click_secondary();
        harness.run();
        harness.get_by_label("Clear History...").click();
        harness.run();
        harness.get_by_label_contains("Remove 1 entry from the history?");

        harness.get_by_label("Delete").click();
        harness.run();
        assert!(matches!(rig.actions.borrow().as_slice(), [Action::ClearHistory]));
    }

    #[test]
    fn history_row_menu_lists_only_commands_that_apply() {
        let (mut harness, _rig) = rig(sample_snapshot());
        harness.run();
        harness.get_by_label("Completed (1)").click();
        harness.run();
        harness.get_by_label("old.bin").click_secondary();
        harness.run();
        harness.get_by_label("Copy URLs");
        harness.get_by_label("Open Containing Folder");
        assert!(harness.query_by_label("Delete...").is_none());
    }

    #[test]
    fn right_click_on_the_empty_table_area_offers_paste() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        // Well below the two rows, inside the table pane.
        secondary_click_at(&mut harness, egui::pos2(500.0, 200.0));
        harness.get_by_label("Paste URLs...").click();
        harness.run();
        assert_eq!(rig.commands.borrow().as_slice(), &[GuiCommand::PasteUrls]);
    }

    #[test]
    fn commands_are_ignored_while_a_dialog_is_open() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Delete...").click();
        harness.run();

        rig.view.borrow_mut().pending.push(GuiCommand::SelectAll);
        rig.view.borrow_mut().pending.push(GuiCommand::Refresh);
        harness.run();
        assert!(rig.commands.borrow().is_empty());
        harness.get_by_label_contains("1 selected");
    }

    #[test]
    fn select_all_and_select_none_work_through_commands() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        rig.view.borrow_mut().pending.push(GuiCommand::SelectAll);
        harness.run();
        harness.get_by_label_contains("2 selected");

        rig.view.borrow_mut().pending.push(GuiCommand::SelectNone);
        harness.run();
        assert!(harness.query_by_label_contains("2 selected").is_none());
    }

    #[test]
    fn ctrl_z_requests_undo_only_when_a_batch_exists() {
        let (mut harness, rig) = rig(sample_snapshot());
        harness.run();
        harness.get_by_label("alpha.iso").click();
        harness.run();

        harness.key_press_modifiers(
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            egui::Key::Z,
        );
        harness.run();
        assert!(rig.commands.borrow().is_empty());

        rig.undo_depth.set(1);
        harness.key_press_modifiers(
            egui::Modifiers::CTRL | egui::Modifiers::COMMAND,
            egui::Key::Z,
        );
        harness.run();
        assert_eq!(rig.commands.borrow().as_slice(), &[GuiCommand::Undo]);
    }

    #[test]
    fn retry_entry_follows_the_error_state_of_the_clicked_row() {
        let mut snapshot = sample_snapshot();
        snapshot.folder_downloads.get_mut("f1").unwrap()[1].status = DownloadStatus::Error;
        let (mut harness, rig) = rig(snapshot);
        harness.run();

        // Not an error row: the entry is disabled, so the click does nothing.
        harness.get_by_label("alpha.iso").click_secondary();
        harness.run();
        harness.get_by_label("Retry").click();
        harness.run();
        assert!(rig.commands.borrow().is_empty());

        harness.get_by_label("beta.zip").click_secondary();
        harness.run();
        harness.get_by_label("Retry").click();
        harness.run();
        assert_eq!(rig.commands.borrow().as_slice(), &[GuiCommand::Retry]);
    }

    mod shortcuts {
        use super::*;
        use egui::{Event, Modifiers};

        const CTRL: Modifiers = Modifiers { ctrl: true, command: true, ..Modifiers::NONE };
        const CTRL_SHIFT: Modifiers =
            Modifiers { ctrl: true, shift: true, ..Modifiers::NONE };

        fn focus_history_search(harness: &mut Harness<'_, ()>) {
            harness.get_by_label("Completed (1)").click();
            harness.run();
            harness
                .get_by_role(egui::accesskit::Role::TextInput)
                .click();
            harness.run();
        }

        #[test]
        fn delete_key_opens_confirm_when_row_selected() {
            let (mut harness, _rig) = rig(sample_snapshot());
            harness.run();
            harness.get_by_label("alpha.iso").click();
            harness.run();
            harness.key_press(egui::Key::Delete);
            harness.run();
            harness.get_by_label("Delete this download?");
        }

        #[test]
        fn delete_key_does_nothing_without_a_selection() {
            let (mut harness, _rig) = rig(sample_snapshot());
            harness.run();
            harness.key_press(egui::Key::Delete);
            harness.run();
            assert!(harness.query_by_label("Delete this download?").is_none());
        }

        #[test]
        fn delete_key_ignored_while_text_field_focused() {
            let (mut harness, _rig) = rig(sample_snapshot());
            harness.run();
            focus_history_search(&mut harness);
            harness
                .get_by_role(egui::accesskit::Role::TextInput)
                .type_text("ab");
            harness.run();
            // Nothing in the history matches "ab", and Delete must edit the field.
            assert!(harness.query_by_label("old.bin").is_none());
            harness.key_press(egui::Key::Home);
            harness.key_press(egui::Key::Delete);
            harness.run();
            harness.get_by_label("old.bin");
            assert!(harness.query_by_label("Delete this download?").is_none());
        }

        #[test]
        fn ctrl_a_selects_all_rows() {
            let (mut harness, _rig) = rig(sample_snapshot());
            harness.run();
            harness.get_by_label("alpha.iso").click();
            harness.run();
            harness.get_by_label_contains("1 selected");
            harness.key_press_modifiers(CTRL, egui::Key::A);
            harness.run();
            harness.get_by_label_contains("2 selected");
        }

        #[test]
        fn paste_event_requests_paste_urls() {
            let (mut harness, rig) = rig(sample_snapshot());
            harness.run();
            harness.event(Event::Paste("https://example.com/a".to_string()));
            harness.run();
            assert_eq!(rig.commands.borrow().as_slice(), &[GuiCommand::PasteUrls]);
        }

        #[test]
        fn ctrl_shift_v_requests_direct_paste() {
            let (mut harness, rig) = rig(sample_snapshot());
            harness.run();
            harness.key_press_modifiers(CTRL_SHIFT, egui::Key::V);
            harness.run();
            assert_eq!(
                rig.commands.borrow().as_slice(),
                &[GuiCommand::PasteUrlsDirect]
            );
        }

        #[test]
        fn paste_is_ignored_while_the_search_field_has_focus() {
            let (mut harness, rig) = rig(sample_snapshot());
            harness.run();
            focus_history_search(&mut harness);
            harness.event(Event::Paste("old".to_string()));
            harness.key_press_modifiers(CTRL_SHIFT, egui::Key::V);
            harness.run();
            assert!(rig.commands.borrow().is_empty());
            // The text field took the pasted text.
            harness.get_by_label("old.bin");
        }

        #[test]
        fn f1_lists_the_shortcuts_and_esc_closes_the_list() {
            let (mut harness, _rig) = rig(sample_snapshot());
            harness.run();
            harness.key_press(egui::Key::F1);
            harness.run();
            harness.get_by_label("F1");
            harness.get_by_label("Ctrl+Shift+V");
            harness.get_by_label("Paste and Add");
            harness.get_by_label("Ctrl+V");

            harness.key_press(egui::Key::Escape);
            harness.run();
            assert!(harness.query_by_label("Ctrl+Shift+V").is_none());
        }

        #[test]
        fn esc_clears_the_selection_when_the_table_has_the_keys() {
            let (mut harness, _rig) = rig(sample_snapshot());
            harness.run();
            harness.get_by_label("alpha.iso").click();
            harness.run();
            harness.key_press(egui::Key::Escape);
            harness.run();
            assert!(harness.query_by_label_contains("1 selected").is_none());
        }
    }
}
