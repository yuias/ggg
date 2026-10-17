//! Download table for a folder (queue) or the Completed node (history).

use crate::i18n::Tr;
use chrono::{DateTime, Local, Utc};
use egui::{Align, Label, Layout, ProgressBar, Sense};
use egui_extras::{Column, TableBuilder};
use ggg::{
    download::task::{DownloadStatus, DownloadTask},
    util::format::{format_size, format_speed},
};
use uuid::Uuid;

/// Shown in cells that have no value yet (unknown size, no speed).
const EMPTY_CELL: &str = "\u{2014}";

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TableKind {
    Queue,
    History,
}

impl TableKind {
    /// Table ids differ because each column set keeps its own widths.
    fn id(self) -> &'static str {
        match self {
            TableKind::Queue => "queue_table",
            TableKind::History => "history_table",
        }
    }
}

/// Shows `rows` and returns the row the user clicked, if any.
pub fn show(
    ui: &mut egui::Ui,
    tr: &Tr,
    kind: TableKind,
    rows: &[&DownloadTask],
    focus: Option<Uuid>,
) -> Option<Uuid> {
    let row_height = ui.spacing().interact_size.y;
    let mut clicked = None;

    let mut table = TableBuilder::new(ui)
        .id_salt(kind.id())
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        .column(Column::initial(240.0).at_least(120.0).clip(true));
    table = match kind {
        TableKind::Queue => table
            .column(Column::initial(100.0).at_least(70.0).clip(true))
            .column(Column::initial(80.0).at_least(60.0).clip(true))
            .column(Column::initial(120.0).at_least(80.0).clip(true))
            .column(Column::initial(80.0).at_least(60.0).clip(true))
            .column(Column::initial(70.0).at_least(50.0).clip(true))
            .column(Column::remainder().at_least(100.0).clip(true)),
        TableKind::History => table
            .column(Column::initial(100.0).at_least(70.0).clip(true))
            .column(Column::initial(80.0).at_least(60.0).clip(true))
            .column(Column::initial(130.0).at_least(100.0).clip(true))
            .column(Column::remainder().at_least(120.0).clip(true)),
    };

    let headers = match kind {
        TableKind::Queue => vec![
            tr.t("column-filename"),
            tr.t("column-status"),
            tr.t("column-size"),
            tr.t("column-progress"),
            tr.t("column-speed"),
            tr.t("column-eta"),
            tr.t("gui-column-added"),
        ],
        TableKind::History => vec![
            tr.t("column-filename"),
            tr.t("column-status"),
            tr.t("column-size"),
            tr.t("gui-column-completed"),
            tr.t("gui-column-url"),
        ],
    };

    table
        .header(row_height, |mut header| {
            for title in &headers {
                header.col(|ui| {
                    ui.add(Label::new(egui::RichText::new(title).strong()).selectable(false));
                });
            }
        })
        .body(|body| {
            body.rows(row_height, rows.len(), |mut row| {
                let task = rows[row.index()];
                row.set_selected(focus == Some(task.id));

                row.col(|ui| {
                    ui.add(Label::new(&task.filename).truncate().selectable(false))
                        .on_hover_text(format!("{}\n{}", task.filename, task.url));
                });
                row.col(|ui| {
                    let text = status_text(tr, task.status);
                    let text = if task.status == DownloadStatus::Error {
                        egui::RichText::new(text).color(ui.visuals().error_fg_color)
                    } else {
                        egui::RichText::new(text)
                    };
                    ui.add(Label::new(text).truncate().selectable(false));
                });
                row.col(|ui| cell_text(ui, &size_text(task)));
                match kind {
                    TableKind::Queue => {
                        row.col(|ui| match progress_fraction(task) {
                            Some(fraction) => {
                                ui.add(
                                    ProgressBar::new(fraction)
                                        .text(format!("{:.0}%", fraction * 100.0))
                                        .desired_width(ui.available_width()),
                                );
                            }
                            None => cell_text(ui, EMPTY_CELL),
                        });
                        row.col(|ui| {
                            let speed = task.speed().map(format_speed);
                            cell_text(ui, speed.as_deref().unwrap_or(EMPTY_CELL));
                        });
                        row.col(|ui| {
                            let eta = task.eta_display();
                            cell_text(ui, eta.as_deref().unwrap_or(EMPTY_CELL));
                        });
                        row.col(|ui| cell_text(ui, &local_time(Some(task.created_at))));
                    }
                    TableKind::History => {
                        row.col(|ui| cell_text(ui, &local_time(task.completed_at)));
                        row.col(|ui| {
                            ui.add(Label::new(&task.url).truncate().selectable(false))
                                .on_hover_text(&task.url);
                        });
                    }
                }

                if row.response().clicked() {
                    clicked = Some(task.id);
                }
            });
        });

    clicked
}

fn cell_text(ui: &mut egui::Ui, text: &str) {
    ui.add(Label::new(text).truncate().selectable(false));
}

/// Localized status without the leading emoji the shared strings carry; the
/// text alone reads cleaner in a table and renders with any font.
pub fn status_text(tr: &Tr, status: DownloadStatus) -> String {
    let key = match status {
        DownloadStatus::Pending => "status-pending",
        DownloadStatus::Downloading => "status-downloading",
        DownloadStatus::Paused => "status-paused",
        DownloadStatus::Completed => "status-completed",
        DownloadStatus::Error => "status-error",
        DownloadStatus::Deleted => "status-deleted",
    };
    strip_leading_symbols(&tr.t(key)).to_string()
}

fn strip_leading_symbols(text: &str) -> &str {
    text.trim_start_matches(|c: char| !c.is_alphanumeric())
}

/// `None` when the total is unknown and the task is not finished.
pub fn progress_fraction(task: &DownloadTask) -> Option<f32> {
    match task.size {
        Some(size) if size > 0 => {
            Some((task.downloaded as f64 / size as f64).clamp(0.0, 1.0) as f32)
        }
        _ if task.status == DownloadStatus::Completed => Some(1.0),
        _ => None,
    }
}

fn size_text(task: &DownloadTask) -> String {
    task.size.map_or_else(|| EMPTY_CELL.to_string(), format_size)
}

/// Local time to the minute; empty when the moment is unknown.
fn local_time(time: Option<DateTime<Utc>>) -> String {
    time.map(|t| t.with_timezone(&Local).format("%Y-%m-%d %H:%M").to_string())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn task() -> DownloadTask {
        DownloadTask::new("https://host/f.bin".to_string(), PathBuf::from("."))
    }

    #[test]
    fn progress_fraction_clamps_and_handles_unknown_size() {
        let mut t = task();
        assert_eq!(progress_fraction(&t), None);

        t.size = Some(200);
        t.downloaded = 50;
        assert_eq!(progress_fraction(&t), Some(0.25));

        t.downloaded = 500;
        assert_eq!(progress_fraction(&t), Some(1.0));

        t.size = None;
        t.status = DownloadStatus::Completed;
        assert_eq!(progress_fraction(&t), Some(1.0));
    }

    #[test]
    fn leading_symbols_are_stripped_from_status_labels() {
        assert_eq!(strip_leading_symbols("\u{23f8}\u{fe0f}  Paused"), "Paused");
        assert_eq!(strip_leading_symbols("Error"), "Error");
        assert_eq!(strip_leading_symbols("\u{2705} \u{5b8c}\u{4e86}"), "\u{5b8c}\u{4e86}");
    }
}
