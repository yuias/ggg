//! Download table for a folder (queue) or the Completed node (history).

use super::selection::{Mods, Selection};
use crate::{
    gui_config::{ColumnWidth, SortState},
    i18n::Tr,
};
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

/// Narrowest the Filename column may get. It takes whatever width the other
/// columns leave, so the table fits the pane instead of cutting off the last
/// column.
const NAME_MIN_WIDTH: f32 = 120.0;

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

struct ColumnDef {
    /// Key shared with `GuiConfig` (widths, sort) and `sort::SortColumn`.
    id: &'static str,
    title_key: &'static str,
    width: f32,
    min_width: f32,
}

const fn column(
    id: &'static str,
    title_key: &'static str,
    width: f32,
    min_width: f32,
) -> ColumnDef {
    ColumnDef { id, title_key, width, min_width }
}

// The first column is always Filename: it absorbs the free width, so its own
// `width` is unused.
const QUEUE_COLUMNS: &[ColumnDef] = &[
    column("name", "column-filename", 0.0, NAME_MIN_WIDTH),
    column("status", "column-status", 90.0, 60.0),
    column("size", "column-size", 75.0, 60.0),
    column("progress", "column-progress", 100.0, 70.0),
    column("speed", "column-speed", 75.0, 60.0),
    column("eta", "column-eta", 60.0, 50.0),
    column("added", "gui-column-added", 125.0, 100.0),
];

const HISTORY_COLUMNS: &[ColumnDef] = &[
    column("name", "column-filename", 0.0, NAME_MIN_WIDTH),
    column("status", "column-status", 90.0, 60.0),
    column("size", "column-size", 75.0, 60.0),
    column("completed", "gui-column-completed", 125.0, 100.0),
    column("url", "gui-column-url", 220.0, 80.0),
];

fn column_defs(kind: TableKind) -> &'static [ColumnDef] {
    match kind {
        TableKind::Queue => QUEUE_COLUMNS,
        TableKind::History => HISTORY_COLUMNS,
    }
}

/// Whether the table of `kind` has `column`. A saved sort on a column the
/// other table lacks must not reorder this one.
pub fn has_column(kind: TableKind, column: &str) -> bool {
    column_defs(kind).iter().any(|def| def.id == column)
}

pub struct TableParams<'a> {
    pub kind: TableKind,
    /// Rows in display order (already sorted and filtered).
    pub rows: &'a [&'a DownloadTask],
    pub selection: &'a Selection,
    pub sort: Option<&'a SortState>,
    /// Row to bring into view this frame, after a keyboard move.
    pub scroll_to: Option<Uuid>,
}

/// What the user did in the table this frame; the caller applies it.
#[derive(Default)]
pub struct TableOutput {
    pub clicked: Option<(Uuid, Mods)>,
    pub secondary_clicked: Option<Uuid>,
    /// Double-click on a row.
    pub activated: Option<Uuid>,
    pub header_clicked: Option<&'static str>,
    /// Rows that fit the visible area, the step of PgUp/PgDn.
    pub page_rows: usize,
}

/// Draws the table. Column widths are read from and written back to `widths`
/// so they persist; columns without an entry use their default.
pub fn show(
    ui: &mut egui::Ui,
    tr: &Tr,
    params: &TableParams<'_>,
    widths: &mut Vec<ColumnWidth>,
) -> TableOutput {
    let TableParams { kind, rows, selection, sort, scroll_to } = *params;
    let defs = column_defs(kind);
    let row_height = ui.spacing().interact_size.y;
    let ctx = ui.ctx().clone();
    let mut out = TableOutput::default();

    let current: Vec<f32> = defs
        .iter()
        .map(|def| {
            widths
                .iter()
                .find(|w| w.id == def.id)
                .map_or(def.width, |w| w.width)
                .max(def.min_width)
        })
        .collect();

    // A resizable column may only grow until Filename is down to its minimum.
    // egui_extras clamps every column to its range each frame, so a narrower
    // window also pulls the columns back instead of pushing the last one out
    // of view.
    let usable = ui.available_width() - ui.spacing().scroll.allocated_width();
    let gaps = ui.spacing().item_spacing.x * defs.len() as f32;
    let fixed_total: f32 = current.iter().skip(1).sum();
    // The ids egui_extras gives the column resize handles; it exposes no other
    // way to tell a user drag from a width the layout changed.
    let state_id = ui.scope_id().with(egui::IdSalt::new(kind.id()));
    let resize_ids: Vec<egui::Id> = (0..defs.len())
        .map(|i| state_id.with("resize_column").with(i))
        .collect();

    // Only a resize by the user is worth keeping. Widths that were pulled back
    // because the window got narrow must not overwrite the saved ones, or the
    // columns would stay small once the window grows again. The width a drag
    // ends on shows up a frame or two later, so keep recording for a few frames.
    let user_resizing = resize_ids.iter().skip(1).any(|id| {
        ctx.read_response(*id)
            .is_some_and(|r| r.dragged() || r.drag_stopped() || r.double_clicked())
    });
    let record_id = state_id.with("record_widths");
    let frames_left: u8 = if user_resizing {
        3
    } else {
        ctx.data(|d| d.get_temp::<u8>(record_id)).unwrap_or(0).saturating_sub(1)
    };
    ctx.data_mut(|d| d.insert_temp(record_id, frames_left));

    let maxes: Vec<f32> = defs
        .iter()
        .zip(&current)
        .map(|(def, width)| (usable - NAME_MIN_WIDTH - (fixed_total - width) - gaps).max(def.min_width))
        .collect();

    // egui_extras never widens a column by itself, so after the window grew
    // again the columns would stay at the narrow size. Drop its state then and
    // let the saved widths apply once more. Only when the usable width grew, so
    // a width that cannot be reached does not reset every frame.
    let layout_id = state_id.with("layout");
    let (previous_usable, previous): (f32, Vec<f32>) = ctx
        .data(|d| d.get_temp(layout_id))
        .unwrap_or((usable, Vec::new()));
    let squeezed = previous.len() == defs.len()
        && (1..defs.len()).any(|i| previous[i] + 0.5 < current[i].min(maxes[i]));
    let relayout = frames_left == 0 && usable > previous_usable + 0.5 && squeezed;

    let mut table = TableBuilder::new(ui)
        .id_salt(kind.id())
        .striped(true)
        .resizable(true)
        .sense(Sense::click())
        .cell_layout(Layout::left_to_right(Align::Center))
        // Not resizable: egui_extras only lets a non-resizable column follow
        // the free space.
        .column(
            Column::remainder()
                .at_least(NAME_MIN_WIDTH)
                .clip(true)
                .resizable(false),
        );
    for ((def, width), max) in defs.iter().zip(&current).zip(&maxes).skip(1) {
        table = table.column(
            Column::initial(width.min(*max))
                .range(def.min_width..=*max)
                .clip(true),
        );
    }
    if relayout {
        table.reset();
    }
    if let Some(id) = scroll_to
        && let Some(index) = rows.iter().position(|task| task.id == id)
    {
        table = table.scroll_to_row(index, None);
    }

    let mut observed: Vec<f32> = Vec::new();
    table
        .header(row_height, |mut header| {
            for def in defs {
                let (_, response) = header.col(|ui| {
                    ui.add(
                        Label::new(egui::RichText::new(tr.t(def.title_key)).strong())
                            .selectable(false),
                    );
                    if let Some(state) = sort.filter(|s| s.column == def.id) {
                        sort_arrow(ui, state.ascending);
                    }
                });
                if response
                    .on_hover_cursor(egui::CursorIcon::PointingHand)
                    .clicked()
                {
                    out.header_clicked = Some(def.id);
                }
            }
        })
        .body(|mut body| {
            observed = body.widths().to_vec();
            let body_ui = body.ui_mut();
            let step = row_height + body_ui.spacing().item_spacing.y;
            out.page_rows = ((body_ui.clip_rect().height() / step).floor() as usize).max(1);

            body.rows(row_height, rows.len(), |mut row| {
                let task = rows[row.index()];
                let selected = selection.is_selected(task.id);
                row.set_selected(selected);
                // Marks a keyboard focus that is not part of the selection.
                row.set_overline(!selected && selection.focus() == Some(task.id));

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

                // A double click also reports the click of its second press;
                // handling both would toggle a Ctrl+double-click back off.
                let response = row.response();
                if response.double_clicked() {
                    out.activated = Some(task.id);
                } else if response.clicked() {
                    let m = ctx.input(|i| i.modifiers);
                    out.clicked = Some((task.id, Mods { ctrl: m.command, shift: m.shift }));
                } else if response.secondary_clicked() {
                    out.secondary_clicked = Some(task.id);
                }
            });
        });

    ctx.data_mut(|d| d.insert_temp(layout_id, (usable, observed.clone())));
    if frames_left > 0 {
        ctx.request_repaint();
        // Filename (index 0) follows the pane, so only the other widths are kept.
        for (def, width) in defs.iter().zip(&observed).skip(1) {
            match widths.iter_mut().find(|w| w.id == def.id) {
                Some(entry) if (entry.width - width).abs() < 0.5 => {}
                Some(entry) => entry.width = *width,
                None => widths.push(ColumnWidth { id: def.id.to_string(), width: *width }),
            }
        }
    }

    out
}

/// Small triangle after the header title. Drawn as a shape because the bundled
/// fonts may lack the arrow glyphs.
fn sort_arrow(ui: &mut egui::Ui, ascending: bool) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(8.0, 6.0), Sense::hover());
    let (base, tip) = if ascending {
        (rect.bottom(), rect.top())
    } else {
        (rect.top(), rect.bottom())
    };
    ui.painter().add(egui::Shape::convex_polygon(
        vec![
            egui::pos2(rect.left(), base),
            egui::pos2(rect.right(), base),
            egui::pos2(rect.center().x, tip),
        ],
        ui.visuals().text_color(),
        egui::Stroke::NONE,
    ));
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

    #[test]
    fn column_ids_are_sortable_and_start_with_the_filename() {
        for kind in [TableKind::Queue, TableKind::History] {
            let defs = column_defs(kind);
            assert_eq!(defs[0].id, "name");
            for def in defs {
                assert!(
                    super::super::sort::SortColumn::from_id(def.id).is_some(),
                    "{} cannot be sorted",
                    def.id
                );
            }
        }
        assert!(has_column(TableKind::Queue, "speed"));
        assert!(!has_column(TableKind::History, "speed"));
    }
}
