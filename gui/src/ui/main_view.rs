//! Main window layout: folder tree, download table, details pane and status bar.

use super::{
    details::{self, DetailsTab},
    queue_table::{self, TableKind},
};
use crate::{bridge::snapshot::Snapshot, gui_config::GuiConfig, i18n::Tr};
use egui::{Button, CursorIcon, Frame, Panel, Sense, Stroke, Ui};
use fluent_bundle::FluentArgs;
use ggg::{
    download::task::{DownloadStatus, DownloadTask},
    ui::queue_view::{QueueView, view_rows},
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
    focus: Option<Uuid>,
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

        let rows = match &self.view {
            Some(view) => view_rows(
                &snapshot.folder_downloads,
                &snapshot.history,
                view,
                &self.search,
            ),
            None => Vec::new(),
        };
        if self
            .focus
            .is_some_and(|id| !rows.iter().any(|task| task.id == id))
        {
            self.focus = None;
        }
        let summary = status_summary(snapshot, rows.len(), usize::from(self.focus.is_some()), tr);

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
                    .focus
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
            egui::CentralPanel::default().show(ui, |ui| self.table(ui, tr, &rows));
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
            self.focus = None;
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
        if ui.add(button).clicked() && !selected {
            self.view = Some(node);
            self.focus = None;
        }
    }

    fn table(&mut self, ui: &mut Ui, tr: &Tr, rows: &[&DownloadTask]) {
        let kind = match self.view {
            Some(QueueView::History) => TableKind::History,
            _ => TableKind::Queue,
        };
        if kind == TableKind::History {
            ui.add(
                egui::TextEdit::singleline(&mut self.search)
                    .id_salt("history_search")
                    .hint_text(tr.t("gui-history-search-hint"))
                    .desired_width(f32::INFINITY),
            );
            ui.add_space(ui.spacing().item_spacing.y);
        }
        if let Some(id) = queue_table::show(ui, tr, kind, rows, self.focus) {
            self.focus = Some(id);
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
    use ggg::app::config::Config;
    use std::{collections::HashMap, path::PathBuf, sync::Arc};

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
