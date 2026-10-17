//! Main window layout: folder tree, download table, details pane and status bar.

use super::{
    details::{self, DetailsTab},
    queue_table::{self, TableKind, TableOutput, TableParams},
    selection::{Mods, Selection},
    sort,
};
use crate::{bridge::snapshot::Snapshot, gui_config::GuiConfig, i18n::Tr};
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
}

impl MainView {
    /// Folder that new downloads go to.
    // Read by the add flows.
    #[allow(dead_code)]
    pub fn target_folder(&self) -> Option<&str> {
        self.target_folder.as_deref()
    }

    pub fn show(
        &mut self,
        ui: &mut Ui,
        snapshot: &Snapshot,
        tr: &Tr,
        config: &mut GuiConfig,
        status: Option<&str>,
    ) {
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
        self.handle_keys(ui, kind, &ids, &rows, config);
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
                            self.tree_node(ui, node, text);
                        }
                        let text = tr.t_args(
                            "gui-tree-completed",
                            &count_args(None, snapshot.history.len()),
                        );
                        self.tree_node(ui, QueueView::History, text);
                    },
                );
            });
    }

    fn tree_node(&mut self, ui: &mut Ui, node: QueueView, text: String) {
        let selected = self.view.as_ref() == Some(&node);
        // The trailing grow atom left-aligns the text inside the full-width button.
        let button = Button::selectable(selected, (text, egui::Atom::grow()));
        if ui.add(button).clicked() {
            self.table_active = false;
            if !selected {
                self.view = Some(node);
                self.selection.clear();
            }
        }
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
            if search.gained_focus() {
                self.table_active = false;
            }
            ui.add_space(ui.spacing().item_spacing.y);
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
        let output = queue_table::show(ui, tr, &params, widths);
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
        if !self.table_active || ui.ctx().egui_wants_keyboard_input() {
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
    use std::{cell::RefCell, collections::HashMap, path::PathBuf, rc::Rc, sync::Arc};

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
            .build_ui(move |ui| view.show(ui, &snapshot, &tr, &mut config, None))
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
            .build_ui(move |ui| view.show(ui, &snapshot, &tr, &mut config.borrow_mut(), None))
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
}
