//! Details pane: task information and its log.

use super::queue_table::{progress_fraction, status_text};
use crate::i18n::Tr;
use chrono::Local;
use egui::{Label, RichText, ScrollArea};
use ggg::{
    download::{
        http_errors::HttpErrorInfo,
        task::{DownloadTask, LogEntry, LogLevel},
    },
    ui::item_edit::{find_header, masked_headers},
    util::format::format_size,
};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum DetailsTab {
    #[default]
    Info,
    Log,
}

pub fn show(ui: &mut egui::Ui, tr: &Tr, task: Option<&DownloadTask>, tab: &mut DetailsTab) {
    ui.horizontal(|ui| {
        ui.selectable_value(tab, DetailsTab::Info, tr.t("gui-details-tab-info"));
        ui.selectable_value(tab, DetailsTab::Log, tr.t("gui-details-tab-log"));
    });
    ui.separator();

    let Some(task) = task else {
        ui.weak(tr.t("message-no-download-selected"));
        return;
    };
    match tab {
        DetailsTab::Info => info(ui, tr, task),
        DetailsTab::Log => log(ui, tr, task),
    }
}

fn info(ui: &mut egui::Ui, tr: &Tr, task: &DownloadTask) {
    ScrollArea::vertical()
        .id_salt("details_info_scroll")
        .auto_shrink([false, false])
        .show(ui, |ui| {
            egui::Grid::new("details_info_grid")
                .num_columns(2)
                .show(ui, |ui| {
                    for (label, value) in info_rows(tr, task) {
                        ui.label(RichText::new(label).strong());
                        ui.add(Label::new(value).wrap());
                        ui.end_row();
                    }
                });

            if let Some(error) = &task.error_message {
                ui.add_space(ui.spacing().item_spacing.y);
                ui.horizontal_wrapped(|ui| {
                    let color = ui.visuals().error_fg_color;
                    ui.label(RichText::new(tr.t("gui-details-label-error")).strong().color(color));
                    ui.label(RichText::new(error).color(color));
                });
                ui.label(error_suggestion(task, error));
            }
        });
}

/// `(label, value)` pairs for the Info grid, in display order.
fn info_rows(tr: &Tr, task: &DownloadTask) -> Vec<(String, String)> {
    let mut rows = vec![
        (
            tr.t("details-label-status"),
            status_text(tr, task.status),
        ),
        (tr.t("details-label-url"), task.url.clone()),
        (
            tr.t("details-label-save-path"),
            task.save_path.to_string_lossy().into_owned(),
        ),
        (
            tr.t("details-label-referrer"),
            find_header(task, "Referer")
                .map(str::to_string)
                .unwrap_or_else(|| tr.t("details-referrer-policy")),
        ),
    ];

    let headers = masked_headers(task);
    if !headers.is_empty() {
        let text = headers
            .iter()
            .map(|(name, value)| format!("{name}: {value}"))
            .collect::<Vec<_>>()
            .join("\n");
        rows.push((tr.t("details-label-headers"), text));
    }

    rows.push((
        tr.t("details-label-size"),
        task.size.map_or_else(String::new, format_size),
    ));
    rows.push((
        tr.t("details-label-downloaded"),
        format_size(task.downloaded),
    ));
    if let Some(fraction) = progress_fraction(task) {
        rows.push((
            tr.t("gui-details-label-progress"),
            format!("{:.1}%", fraction * 100.0),
        ));
    }
    rows
}

fn error_suggestion(task: &DownloadTask, error: &str) -> String {
    match task.last_status_code {
        Some(status) => HttpErrorInfo::from_status(status),
        None => HttpErrorInfo::network_error(error),
    }
    .suggestion
}

fn log(ui: &mut egui::Ui, tr: &Tr, task: &DownloadTask) {
    if task.logs.is_empty() {
        ui.weak(tr.t("message-no-logs"));
        return;
    }
    ScrollArea::vertical()
        .id_salt("details_log_scroll")
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            for entry in &task.logs {
                let mut text = RichText::new(log_line(entry)).monospace();
                match entry.level {
                    LogLevel::Info => {}
                    LogLevel::Warn => text = text.color(ui.visuals().warn_fg_color),
                    LogLevel::Error => text = text.color(ui.visuals().error_fg_color),
                }
                ui.add(Label::new(text).wrap());
            }
        });
}

/// `[HH:MM:SS] LEVEL message` in local time.
fn log_line(entry: &LogEntry) -> String {
    let level = match entry.level {
        LogLevel::Info => "INFO ",
        LogLevel::Warn => "WARN ",
        LogLevel::Error => "ERROR",
    };
    let time = entry.timestamp.with_timezone(&Local).format("%H:%M:%S");
    format!("[{time}] {level} {}", entry.message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn task() -> DownloadTask {
        DownloadTask::new("https://host/f.bin".to_string(), PathBuf::from("dl"))
    }

    #[test]
    fn log_line_has_time_level_and_message() {
        let line = log_line(&LogEntry::warn("slow"));
        assert!(line.starts_with('['), "{line}");
        assert!(line.ends_with("] WARN  slow") || line.ends_with("WARN  slow"), "{line}");
        assert_eq!(line.find(']'), Some(9), "{line}");
    }

    #[test]
    fn info_rows_fall_back_to_referrer_policy_and_mask_headers() {
        let tr = Tr::load("en-US");
        let mut t = task();
        let rows = info_rows(&tr, &t);
        assert_eq!(rows[3].1, tr.t("details-referrer-policy"));
        assert!(!rows.iter().any(|(l, _)| *l == tr.t("details-label-headers")));

        t.set_header("Referer", "https://ref/");
        t.set_header("Authorization", "Bearer secret-token");
        let rows = info_rows(&tr, &t);
        assert_eq!(rows[3].1, "https://ref/");
        let headers = rows
            .iter()
            .find(|(l, _)| *l == tr.t("details-label-headers"))
            .expect("headers row");
        assert!(headers.1.starts_with("Authorization:"));
        assert!(!headers.1.contains("secret-token"));
    }
}
