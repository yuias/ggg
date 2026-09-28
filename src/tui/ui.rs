use super::app::TuiApp;
use super::format::{
    format_progress_bar, format_progress_with_bar, format_size, format_speed, truncate_filename,
};
use super::state::{DeleteTarget, DetailsPosition, FocusPane, FolderTreeItem, UiMode};
use super::theme;
use crate::download::task::{DownloadStatus, LogLevel};
use crate::download::http_errors::HttpErrorInfo;
use fluent::fluent_args;
use ratatui::{
    layout::{Alignment, Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Cell, Clear, List, ListItem, ListState, Paragraph, Row, Table, Tabs, Wrap},
    Frame,
};
use unicode_width::UnicodeWidthStr;

/// Main rendering function
pub fn render(app: &TuiApp, f: &mut Frame) {
    let size = f.area();

    // Create main layout based on UI mode
    let is_main_screen = matches!(
        app.state.ui_mode,
        UiMode::Normal | UiMode::AddDownload | UiMode::DownloadPreview |
        UiMode::Search | UiMode::ChangeFolder | UiMode::ChangeSavePath | UiMode::SwitchFolder |
        UiMode::ConfirmDelete | UiMode::ContextMenu | UiMode::Help
    ) || (matches!(app.state.ui_mode, UiMode::EditingField) && !app.state.is_editing_app_setting);

    // Main layout: content area + status bar
    let main_chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([
            Constraint::Min(0),    // Content area
            Constraint::Length(1), // Status bar
        ])
        .split(size);

    // Render main area (overlays handled separately)
    match app.state.ui_mode {
        UiMode::Settings | UiMode::FolderEdit => render_settings(app, f, main_chunks[0]),
        UiMode::EditingField if app.state.is_editing_app_setting => render_settings(app, f, main_chunks[0]),
        // A folder-delete confirmation was opened from Settings, so keep the
        // Settings screen underneath the overlay instead of flashing the
        // three-pane layout behind it.
        UiMode::ConfirmDelete if matches!(app.state.delete_target, DeleteTarget::Folder(_)) => {
            render_settings(app, f, main_chunks[0])
        }
        _ if is_main_screen => render_three_pane_layout(app, f, main_chunks[0]),
        _ => render_three_pane_layout(app, f, main_chunks[0]),
    }

    // Render status bar
    render_status_bar(app, f, main_chunks[1]);

    // Render input dialogs (overlays)
    match app.state.ui_mode {
        UiMode::Help => render_help(app, f, size),
        UiMode::AddDownload => render_add_download_dialog(app, f, size),
        UiMode::EditingField => render_input_dialog(app, f, size),
        UiMode::DownloadPreview => render_download_preview_dialog(app, f, size),
        UiMode::Search => {}, // Search is inline in status bar
        UiMode::ChangeFolder => render_change_folder_for_item_dialog(app, f, size),
        UiMode::ChangeSavePath => render_change_save_path_dialog(app, f, size),
        UiMode::SwitchFolder => render_switch_folder_dialog(app, f, size),
        UiMode::ConfirmDelete => render_confirm_delete_dialog(app, f, size),
        UiMode::ContextMenu => render_context_menu(app, f, size),
        UiMode::FolderContextMenu => render_folder_context_menu(app, f, size),
        _ => {}
    }
}

/// Render the 3-pane layout (folder tree | download list / details)
fn render_three_pane_layout(app: &TuiApp, f: &mut Frame, area: Rect) {
    // Horizontal split: folder tree (left) | right side
    let horizontal_chunks = Layout::default()
        .direction(Direction::Horizontal)
        .constraints([
            Constraint::Length(22), // Folder tree (fixed width)
            Constraint::Min(0),     // Right side (download list + details)
        ])
        .split(area);

    // Store folder tree region
    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.folder_tree = Some(horizontal_chunks[0]);
    }

    // Render folder tree
    render_folder_tree(app, f, horizontal_chunks[0]);

    // Right side layout depends on details_position
    match app.state.details_position {
        DetailsPosition::Bottom => {
            // Vertical split: download list (top) | details (bottom)
            let right_chunks = Layout::default()
                .direction(Direction::Vertical)
                .constraints([
                    Constraint::Percentage(60), // Download list
                    Constraint::Percentage(40), // Details panel
                ])
                .split(horizontal_chunks[1]);

            // Store pane regions
            {
                let mut regions = app.state.click_regions.borrow_mut();
                regions.download_list = Some(right_chunks[0]);
                regions.details_panel = Some(right_chunks[1]);
            }

            render_download_list(app, f, right_chunks[0]);
            render_details_panel(app, f, right_chunks[1]);
        }
        DetailsPosition::Right => {
            // Horizontal split: download list (left) | details (right)
            let right_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([
                    Constraint::Percentage(70), // Download list
                    Constraint::Percentage(30), // Details panel
                ])
                .split(horizontal_chunks[1]);

            // Store pane regions
            {
                let mut regions = app.state.click_regions.borrow_mut();
                regions.download_list = Some(right_chunks[0]);
                regions.details_panel = Some(right_chunks[1]);
            }

            render_download_list(app, f, right_chunks[0]);
            render_details_panel(app, f, right_chunks[1]);
        }
        DetailsPosition::Hidden => {
            // Store pane regions
            {
                let mut regions = app.state.click_regions.borrow_mut();
                regions.download_list = Some(horizontal_chunks[1]);
                regions.details_panel = None;
            }

            // Full area for download list
            render_download_list(app, f, horizontal_chunks[1]);
        }
    }
}

/// Render the folder tree pane
fn render_folder_tree(app: &TuiApp, f: &mut Frame, area: Rect) {
    let t = |key: &str| app.state.t(key);
    let is_focused = app.state.focus_pane == FocusPane::FolderTree;

    // Build list items from tree_items
    let completed_label = t("tree-completed-node");
    // Pre-compute folder display names for tree items
    let folder_tree_names: Vec<String> = app.state.tree_items.iter().map(|item| {
        match item {
            FolderTreeItem::Folder(id) => app.state.folder_display_name(id),
            FolderTreeItem::CompletedNode => completed_label.clone(),
        }
    }).collect();

    let items: Vec<ListItem> = app.state.tree_items.iter().enumerate().map(|(i, item)| {
        let (icon, name) = match item {
            FolderTreeItem::Folder(_) => ("📁", folder_tree_names[i].as_str()),
            FolderTreeItem::CompletedNode => ("📋", folder_tree_names[i].as_str()),
        };

        let style = if i == app.state.tree_selected_index {
            Style::default()
                .fg(theme::ACCENT_SELECTED)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(theme::TEXT_NORMAL)
        };

        ListItem::new(format!(" {} {}", icon, name)).style(style)
    }).collect();

    let border_style = if is_focused {
        Style::default().fg(theme::ACCENT_SELECTED)
    } else {
        Style::default().fg(theme::BORDER_INACTIVE)
    };

    let list = List::new(items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(t("pane-folders"))
        )
        .highlight_style(
            Style::default()
                .bg(theme::BG_SELECTION)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    // Create a ListState for rendering
    let mut list_state = ListState::default();
    list_state.select(Some(app.state.tree_selected_index));

    f.render_stateful_widget(list, area, &mut list_state);

    // Track clickable regions for folder items
    // Inner area accounts for border (1 cell on each side)
    let inner_area = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let mut folder_items = Vec::new();
    for (idx, _) in app.state.tree_items.iter().enumerate() {
        if idx < inner_area.height as usize {
            let item_rect = Rect {
                x: inner_area.x,
                y: inner_area.y + idx as u16,
                width: inner_area.width,
                height: 1,
            };
            folder_items.push((idx, item_rect));
        }
    }

    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.folder_items = folder_items;
    }
}

/// Render download list table
fn render_download_list(app: &TuiApp, f: &mut Frame, area: Rect) {
    let t = |key: &str| app.state.t(key);
    let is_focused = app.state.focus_pane == FocusPane::DownloadList;
    let is_viewing_history = app.state.is_viewing_completed_node();

    let filtered = app.state.filtered_downloads();
    let count = filtered.len();

    // Create table header with inverted colors for better visibility
    let header = Row::new(vec![
        Cell::from(t("column-sel")),
        Cell::from(t("column-status")),
        Cell::from(t("column-filename")),
        Cell::from(t("column-size")),
        Cell::from(t("column-progress")),
        Cell::from(t("column-speed")),
        Cell::from(t("column-eta")),
    ])
    .style(
        Style::default()
            .fg(theme::FG_ON_TABLE_HEADER)
            .bg(theme::BG_TABLE_HEADER)
            .add_modifier(Modifier::BOLD),
    )
    .height(1);

    // Create table rows
    // Note: ratatui's Table handles viewport rendering internally,
    // so we create all rows but the widget only renders visible ones
    let rows: Vec<Row> = filtered
        .iter()
        .map(|task| {
            let status_icon = status_icon(app, &task.status);
            // Use red for failed items in history view
            let status_color = if is_viewing_history && task.status == DownloadStatus::Error {
                Color::Red
            } else {
                status_color(&task.status)
            };

            // Selection indicator
            let sel_indicator = if app.state.is_download_selected(task.id) {
                "[✓]"
            } else {
                "[ ]"
            };
            let sel_color = if app.state.is_download_selected(task.id) {
                Color::Green
            } else {
                Color::DarkGray
            };

            let total_size = task.size.unwrap_or(0);
            let progress_text = format_progress_with_bar(task.downloaded, task.size);

            // Calculate speed display
            let speed_text = task.speed()
                .map(|s| format_speed(s))
                .unwrap_or_else(|| "-".to_string());
            
            // Calculate ETA display
            let eta_text = task.eta_display()
                .unwrap_or_else(|| "-".to_string());

            Row::new(vec![
                Cell::from(sel_indicator).style(Style::default().fg(sel_color)),
                Cell::from(status_icon).style(Style::default().fg(status_color)),
                Cell::from(truncate_filename(&task.filename, 50)),
                Cell::from(format_size(total_size)),
                Cell::from(progress_text),
                Cell::from(speed_text),
                Cell::from(eta_text),
            ])
        })
        .collect();

    // Create table widget
    let widths = [
        Constraint::Length(5),   // Selection column
        Constraint::Length(15),  // Status (wider for emoji)
        Constraint::Min(20),     // Filename
        Constraint::Length(10),  // Size
        Constraint::Length(16),  // Progress (with bar)
        Constraint::Length(10),  // Speed
        Constraint::Length(10),  // ETA
    ];

    // Build title based on context
    let selection_count = app.state.selected_downloads.len();
    let base_title = if is_viewing_history {
        t("pane-history")
    } else {
        t("pane-downloads")
    };

    let title = if selection_count > 0 {
        if app.state.search_query.is_empty() {
            format!("{} ({} items, {} selected)", base_title, count, selection_count)
        } else {
            format!("{} ({} items, {} selected, filtered: \"{}\")", base_title, count, selection_count, app.state.search_query)
        }
    } else if app.state.search_query.is_empty() {
        format!("{} ({} items)", base_title, count)
    } else {
        format!("{} ({} items, filtered: \"{}\")", base_title, count, app.state.search_query)
    };

    let border_style = if is_focused {
        Style::default().fg(theme::ACCENT_SELECTED)
    } else {
        Style::default().fg(theme::BORDER_INACTIVE)
    };

    let table = Table::new(rows, widths)
        .header(header)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title)
                .style(Style::default().bg(theme::BG_PRIMARY))
        )
        .row_highlight_style(
            Style::default()
                .bg(theme::BG_SELECTION)
                .add_modifier(Modifier::BOLD),
        )
        .highlight_symbol("▶ ");

    f.render_stateful_widget(table, area, &mut *app.state.table_state_mut());

    // Track clickable regions for download rows
    // Inner area: border (1) + we need to account for header row (1)
    let inner_area = Rect {
        x: area.x + 1,
        y: area.y + 2, // +1 for border, +1 for header
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(3), // -2 for borders, -1 for header
    };

    let scroll_offset = app.state.table_state().offset();
    let mut download_rows = Vec::new();

    for visible_idx in 0..inner_area.height as usize {
        let data_idx = scroll_offset + visible_idx;
        if data_idx < count {
            let row_rect = Rect {
                x: inner_area.x,
                y: inner_area.y + visible_idx as u16,
                width: inner_area.width,
                height: 1,
            };
            download_rows.push((data_idx, row_rect));
        }
    }

    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.download_rows = download_rows;
    }
}

/// Render details panel for selected download
fn render_details_panel(app: &TuiApp, f: &mut Frame, area: Rect) {
    let t = |key: &str| app.state.t(key);
    let is_focused = app.state.focus_pane == FocusPane::DetailsPanel;

    let border_style = if is_focused {
        Style::default().fg(theme::ACCENT_SELECTED)
    } else {
        Style::default().fg(theme::BORDER_INACTIVE)
    };

    if let Some(task) = app.state.get_selected_download() {
        // Split panel vertically: info (top) and logs (bottom)
        let chunks = Layout::default()
            .direction(Direction::Vertical)
            .constraints([
                Constraint::Percentage(50),  // Info section
                Constraint::Percentage(50),  // Log section
            ])
            .split(area);

        render_task_info(app, task, f, chunks[0], border_style);
        render_task_logs(app, task, f, chunks[1], border_style);
    } else {
        let paragraph = Paragraph::new(t("message-no-download-selected"))
            .block(
                Block::default()
                    .borders(Borders::ALL)
                    .border_style(border_style)
                    .title(t("pane-details"))
            )
            .wrap(Wrap { trim: true });
        f.render_widget(paragraph, area);
    }
}

/// Render task basic info section
fn render_task_info(app: &TuiApp, task: &crate::download::task::DownloadTask, f: &mut Frame, area: Rect, border_style: Style) {
    let total_size = task.size.unwrap_or(0);
    let progress = if total_size > 0 {
        (task.downloaded as f64 / total_size as f64) * 100.0
    } else {
        0.0
    };

    let mut details = vec![
        Line::from(vec![
            Span::styled(
                format!("{} ", app.state.t("details-label-status")),
                Style::default().add_modifier(Modifier::BOLD)
            ),
            Span::styled(
                status_icon(app, &task.status),
                Style::default().fg(status_color(&task.status)).add_modifier(Modifier::BOLD),
            ),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("{} ", app.state.t("details-label-url")),
                Style::default().add_modifier(Modifier::BOLD)
            ),
        ]),
        Line::from(Span::raw(&task.url)),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("{} ", app.state.t("details-label-save-path")),
                Style::default().add_modifier(Modifier::BOLD)
            ),
        ]),
        Line::from(Span::raw(task.save_path.to_string_lossy().to_string())),
        Line::from(""),
        Line::from(vec![
            Span::styled(
                format!("{} ", app.state.t("details-label-size")),
                Style::default().add_modifier(Modifier::BOLD)
            ),
            Span::raw(format_size(total_size)),
        ]),
        Line::from(vec![
            Span::styled(
                format!("{} ", app.state.t("details-label-downloaded")),
                Style::default().add_modifier(Modifier::BOLD)
            ),
            Span::raw(format_size(task.downloaded)),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("Progress: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(format!("{:.1}%", progress)),
        ]),
        Line::from(Span::raw(format_progress_bar(task.downloaded, task.size, 30))),
    ];

    // Add error message if present - enhanced display with visual prominence
    if let Some(ref error) = task.error_message {
        details.push(Line::from(""));
        details.push(Line::from(Span::styled(
            "═══════════════════════════════",
            Style::default().fg(Color::Red),
        )));

        // Parse error info from status code
        let error_info = if let Some(status) = task.last_status_code {
            HttpErrorInfo::from_status(status)
        } else {
            // Treat as network error if no status code
            HttpErrorInfo::network_error(error)
        };

        // Show error with category icon
        details.push(Line::from(vec![
            Span::styled(
                format!("{} ERROR: ", error_info.category_icon()),
                Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)
            ),
            Span::styled(
                error,
                Style::default().fg(Color::Red)
            ),
        ]));

        // Show suggestion
        details.push(Line::from(""));
        details.push(Line::from(vec![
            Span::styled("💡 ", Style::default().fg(Color::Yellow)),
            Span::styled(
                error_info.suggestion.clone(),
                Style::default().fg(Color::Yellow).add_modifier(Modifier::ITALIC)
            ),
        ]));

        // Show retry information
        if error_info.is_retryable {
            let retry_msg = if task.retry_count > 0 {
                format!("Retry #{} will attempt automatically.", task.retry_count + 1)
            } else {
                "Press 'r' to retry manually.".to_string()
            };
            details.push(Line::from(Span::styled(
                retry_msg,
                Style::default().fg(Color::Cyan).add_modifier(Modifier::ITALIC)
            )));
        }

        details.push(Line::from(Span::styled(
            "═══════════════════════════════",
            Style::default().fg(Color::Red),
        )));
        details.push(Line::from(""));
        details.push(Line::from(Span::styled(
            "Check logs below for full details.",
            Style::default().fg(Color::Gray).add_modifier(Modifier::ITALIC)
        )));
    }

    let paragraph = Paragraph::new(details)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(app.state.t("pane-details"))
        )
        .wrap(Wrap { trim: true });

    f.render_widget(paragraph, area);
}

/// Render task logs section
fn render_task_logs(_app: &TuiApp, task: &crate::download::task::DownloadTask, f: &mut Frame, area: Rect, border_style: Style) {
    let mut log_lines = Vec::new();

    if task.logs.is_empty() {
        log_lines.push(Line::from(Span::styled(
            "No log entries yet",
            Style::default().fg(Color::Gray),
        )));
    } else {
        // Show last N log entries (most recent at bottom)
        let max_logs = (area.height.saturating_sub(2)) as usize; // Account for borders
        let start_idx = task.logs.len().saturating_sub(max_logs);

        for log in &task.logs[start_idx..] {
            let timestamp_str = log.timestamp.with_timezone(&chrono::Local).format("%H:%M:%S").to_string();

            let (level_str, level_color) = match log.level {
                LogLevel::Info => ("INFO ", Color::White),
                LogLevel::Warn => ("WARN ", Color::Yellow),
                LogLevel::Error => ("ERROR", Color::Red),
            };

            log_lines.push(Line::from(vec![
                Span::styled(
                    format!("[{}] ", timestamp_str),
                    Style::default().fg(Color::DarkGray),
                ),
                Span::styled(
                    format!("{} ", level_str),
                    Style::default().fg(level_color).add_modifier(Modifier::BOLD),
                ),
                Span::raw(&log.message),
            ]));
        }
    }

    let log_count = task.logs.len();
    let title = if log_count > 0 {
        format!("Logs ({} entries)", log_count)
    } else {
        "Logs".to_string()
    };

    let paragraph = Paragraph::new(log_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(border_style)
                .title(title)
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

/// Render status bar with keybindings (Quick Actions Bar)
fn render_status_bar(app: &TuiApp, f: &mut Frame, area: Rect) {
    let t = |key: &str| app.state.t(key);
    let t_args = |key: &str, args: Option<&fluent_bundle::FluentArgs>| app.state.t_with_args(key, args);

    let (left_content, right_content) = match app.state.ui_mode {
        UiMode::Normal => {
            // Quick actions for main screen
            let undo_hint = if !app.state.delete_history.is_empty() {
                let args = fluent_args! {
                    "count" => app.state.delete_history.len(),
                };
                format!(" | {}", t_args("status-normal-undo", Some(&args)))
            } else {
                String::new()
            };

            let status_hint = app
                .state
                .status_message
                .as_ref()
                .map(|msg| format!(" | {msg}"))
                .unwrap_or_default();

            let current_folder_name = app.state.current_folder_name();
            let args = fluent_args! {
                "folder" => current_folder_name.as_str(),
            };
            // Action hints depend on which pane is focused.
            let actions_key = match app.state.focus_pane {
                FocusPane::FolderTree => "status-normal-actions-folder",
                FocusPane::DownloadList => "status-normal-actions-download",
                FocusPane::DetailsPanel => "status-normal-actions-details",
            };
            let left = format!(
                "{} | {}{}{} | {}",
                t_args("status-normal-folder", Some(&args)),
                t(actions_key),
                undo_hint,
                status_hint,
                t("status-normal-right")
            );
            // Version displayed on the right for main screen
            let version_args = fluent_args! {
                "version" => env!("CARGO_PKG_VERSION"),
            };
            let right = t_args("app-version", Some(&version_args));
            (left, right)
        }
        // For other screens, show hints on left, nothing on right
        UiMode::AddDownload => {
            (t("status-hint-cancel"), String::new())
        }
        UiMode::EditingField => {
            (t("status-hint-cancel"), String::new())
        }
        UiMode::DownloadPreview => {
            (t("status-hint-confirm-cancel"), String::new())
        }
        UiMode::Search => {
            (t("status-hint-finish"), String::new())
        }
        UiMode::ChangeFolder => {
            (t("status-hint-navigate"), String::new())
        }
        UiMode::ChangeSavePath => {
            (t("status-hint-confirm-cancel"), String::new())
        }
        UiMode::SwitchFolder => {
            (t("status-hint-navigate"), String::new())
        }
        UiMode::Help => {
            (t("status-hint-close"), String::new())
        }
        UiMode::Settings => {
            // The restart notice set right after editing a field is shown
            // here too, not only in Normal mode, since the user is usually
            // still on the settings screen when it matters.
            let status_hint = app
                .state
                .status_message
                .as_ref()
                .map(|msg| format!(" | {msg}"))
                .unwrap_or_default();
            (format!("{}{}", t("status-hint-settings"), status_hint), String::new())
        }
        UiMode::FolderEdit => {
            (t("status-hint-folder-edit"), String::new())
        }
        UiMode::ConfirmDelete => {
            (t("status-hint-confirm-yn"), String::new())
        }
        UiMode::ContextMenu => {
            (t("status-hint-menu"), String::new())
        }
        UiMode::FolderContextMenu => {
            (t("status-hint-menu"), String::new())
        }
    };

    // Create a single line without border.
    // Use display width (not char count) so wide glyphs like the 📥 emoji
    // reserve their true 2-column footprint; otherwise the line overflows and
    // ratatui clips the right-aligned version text (e.g. "v0.4.0" -> "v0.4").
    let padding_width = area.width.saturating_sub(
        (UnicodeWidthStr::width(left_content.as_str())
            + UnicodeWidthStr::width(right_content.as_str())
            + 2) as u16,
    );

    let status_line = Line::from(vec![
        Span::styled(left_content, Style::default().fg(Color::Cyan)),
        Span::raw(" ".repeat(padding_width as usize)),
        Span::styled(right_content, Style::default().fg(Color::Yellow)),
    ]);

    let paragraph = Paragraph::new(status_line);
    f.render_widget(paragraph, area);
}

/// Render help screen overlay as centered popup
fn render_help(app: &TuiApp, f: &mut Frame, area: Rect) {
    let t = |key: &str| app.state.t(key);

    // Define popup dimensions
    let dialog_width = 80;
    let dialog_height = 40;

    // Calculate centered position
    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width.min(area.width),
        height: dialog_height.min(area.height),
    };

    let mut help_text = vec![
        Line::from(Span::styled(
            t("help-title"),
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD),
        )),
        Line::from(""),
        Line::from(Span::styled(t("help-section-primary"), Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("  {}", t("help-key-space"))),
        Line::from(format!("  {}", t("help-key-enter"))),
        Line::from(format!("  {}", t("help-key-a"))),
        Line::from(format!("  {}", t("help-key-d"))),
        Line::from(format!("  {}", t("help-key-ctrl-z"))),
        Line::from(format!("  {}", t("help-key-m"))),
        Line::from(format!("  {}", t("help-key-e"))),
        Line::from(format!("  {}", t("help-key-r"))),
        Line::from(format!("  {}", t("help-key-shift-s"))),
        Line::from(format!("  {}", t("help-key-shift-p"))),
        Line::from(""),
        Line::from(Span::styled(t("help-section-multi"), Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("  {}", t("help-key-v"))),
        Line::from(format!("  {}", t("help-key-v-shift"))),
        Line::from(format!("  {}", t("help-key-esc-clear"))),
        Line::from(format!("  {}", t("help-key-multi-action"))),
        Line::from(""),
        Line::from(Span::styled(t("help-section-navigation"), Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("  {}", t("help-key-jk"))),
        Line::from(format!("  {}", t("help-key-g"))),
        Line::from(format!("  {}", t("help-key-g-shift"))),
        Line::from(format!("  {}", t("help-key-ctrl-d"))),
        Line::from(format!("  {}", t("help-key-ctrl-u"))),
        Line::from(""),
        Line::from(Span::styled(t("help-section-panes"), Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("  {}", t("help-key-prev-pane"))),
        Line::from(format!("  {}", t("help-key-next-pane"))),
        Line::from(""),
        Line::from(Span::styled(t("help-section-search"), Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("  {}", t("help-key-slash"))),
        Line::from(format!("  {}", t("help-key-esc-search"))),
        Line::from(""),
        Line::from(Span::styled(t("help-section-ui"), Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("  {}", t("help-key-question"))),
        Line::from(format!("  {}", t("help-key-x"))),
        Line::from(format!("  {}", t("help-key-i"))),
        Line::from(format!("  {}", t("help-key-r-shift"))),
        Line::from(""),
        Line::from(Span::styled(t("help-section-settings"), Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("  {}", t("help-key-reload-config"))),
        Line::from(""),
        Line::from(Span::styled(t("help-section-system"), Style::default().add_modifier(Modifier::BOLD))),
        Line::from(format!("  {}", t("help-key-quit"))),
        Line::from(""),
    ];

    // Show IPC pipe name on Windows
    #[cfg(windows)]
    if let Some(ref pipe_name) = app.state.ipc_pipe_name {
        help_text.push(Line::from(Span::styled(
            "IPC",
            Style::default().add_modifier(Modifier::BOLD),
        )));
        help_text.push(Line::from(format!("  Pipe: {}", pipe_name)));
        help_text.push(Line::from(""));
    }

    help_text.push(Line::from(Span::styled(
        t("help-footer"),
        Style::default().fg(Color::Green),
    )));

    let paragraph = Paragraph::new(help_text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.state.t("dialog-help"))
                .style(Style::default().bg(theme::BG_PRIMARY)),
        )
        .wrap(Wrap { trim: false });

    // Clear the background area and render the popup
    f.render_widget(Clear, dialog_area);
    f.render_widget(paragraph, dialog_area);
}

/// Render settings screen with tabs (Application / Folder)
fn render_settings(app: &TuiApp, f: &mut Frame, area: Rect) {
    use crate::tui::state::SettingsSection;

    // Split into tabs and content
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([Constraint::Length(3), Constraint::Min(0)])
        .split(area);

    // Render tabs
    render_settings_tabs(app, f, chunks[0]);

    // Render section content
    match app.state.settings_section {
        SettingsSection::General | SettingsSection::Downloads | SettingsSection::Scripts => {
            render_application_settings(app, f, chunks[1])
        }
        SettingsSection::Folders => {
            // Split into left (folder list) and right (details/editor) panels
            let folder_chunks = Layout::default()
                .direction(Direction::Horizontal)
                .constraints([Constraint::Percentage(30), Constraint::Percentage(70)])
                .split(chunks[1]);

            render_folder_list(app, f, folder_chunks[0]);
            render_folder_details(app, f, folder_chunks[1]);
        }
    }
}

/// Render settings section tabs
fn render_settings_tabs(app: &TuiApp, f: &mut Frame, area: Rect) {
    use crate::tui::state::SettingsSection;

    let titles: Vec<String> = SettingsSection::ALL
        .iter()
        .map(|section| app.state.t(section.label_key()))
        .collect();
    let selected_index = SettingsSection::ALL
        .iter()
        .position(|s| *s == app.state.settings_section)
        .unwrap_or(0);

    let tabs = Tabs::new(titles.clone())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(theme::BORDER_INACTIVE))
                .title(app.state.t("dialog-settings")),
        )
        .select(selected_index)
        .style(Style::default().fg(theme::TEXT_MUTED))
        .highlight_style(
            Style::default()
                .fg(theme::ACCENT_SELECTED)
                .add_modifier(Modifier::BOLD),
        )
        .divider(" │ ");

    f.render_widget(tabs, area);

    // Track clickable regions for tabs
    // Inner area: border (1) on each side
    let inner_x = area.x + 1;
    let inner_y = area.y + 1;

    // Calculate tab positions (tabs are separated by " │ " which is 3 cells
    // wide); use display width, not byte length, so CJK labels line up.
    let mut tab_rects = Vec::new();
    let mut current_x = inner_x;

    for (idx, title) in titles.iter().enumerate() {
        // Tab width: display width + 2 for padding
        let tab_width = UnicodeWidthStr::width(title.as_str()) as u16 + 2;
        let tab_rect = Rect {
            x: current_x,
            y: inner_y,
            width: tab_width,
            height: 1,
        };
        tab_rects.push((idx, tab_rect));

        // Move to next tab position (tab width + separator " │ " = 3)
        current_x += tab_width + 3;
    }

    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.settings_tabs = tab_rects;
    }
}

/// Render application settings
fn render_application_settings(app: &TuiApp, f: &mut Frame, area: Rect) {
    use crate::tui::state::{ApplicationSettingsField, SettingsSection};

    let config = app.state.app_state.config.try_read();
    let mut lines = Vec::new();

    // Modern color palette
    let section_header_color = theme::ACCENT_SECTION;
    let selected_color = theme::ACCENT_SELECTED;
    let description_color = theme::TEXT_DESCRIPTION;
    let border_color = theme::BORDER_INACTIVE;
    let success_color = theme::STATUS_SUCCESS;
    let error_color = theme::STATUS_ERROR;
    let muted_color = theme::TEXT_MUTED;

    let current_section = app.state.settings_section;

    if let Ok(config) = config {
        lines.push(Line::from(Span::styled(
            app.state.t(current_section.label_key()),
            Style::default()
                .fg(section_header_color)
                .add_modifier(Modifier::BOLD),
        )));
        lines.push(Line::from(""));

        let fields = ApplicationSettingsField::in_section(current_section);
        for (idx, field) in fields.iter().enumerate() {
            let is_selected = idx == app.state.app_settings_field_index;
            let prefix = if is_selected { "▸ " } else { "  " };
            let style = if is_selected {
                Style::default()
                    .fg(selected_color)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT_NORMAL)
            };

            let value = match field {
                ApplicationSettingsField::MaxConcurrent => {
                    config.download.max_concurrent.to_string()
                }
                ApplicationSettingsField::MaxConcurrentPerFolder => config
                    .download
                    .max_concurrent_per_folder
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| app.state.t("settings-value-not-set")),
                ApplicationSettingsField::MaxActiveFolders => config
                    .download
                    .parallel_folder_count
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| app.state.t("settings-value-not-set")),
                ApplicationSettingsField::MaxRedirects => {
                    config.download.max_redirects.to_string()
                }
                ApplicationSettingsField::RetryCount => {
                    config.download.retry_count.to_string()
                }
                ApplicationSettingsField::UserAgent => {
                    config.download.user_agent.clone()
                }
                ApplicationSettingsField::ReferrerPolicy => {
                    use crate::app::config::ReferrerPolicy;
                    match &config.download.referrer_policy {
                        ReferrerPolicy::Custom { value, .. } => {
                            format!("{}: {}", app.state.t("settings-referrer-custom"), value)
                        }
                        other => app.state.t(other.display_key()),
                    }
                }
                ApplicationSettingsField::ScriptsEnabled => {
                    if config.scripts.enabled {
                        app.state.t("settings-value-enabled")
                    } else {
                        app.state.t("settings-value-disabled")
                    }
                }
                ApplicationSettingsField::SkipDownloadPreview => {
                    if config.general.skip_download_preview { 
                        app.state.t("settings-value-enabled") 
                    } else { 
                        app.state.t("settings-value-disabled") 
                    }
                }
                ApplicationSettingsField::Language => {
                    config.general.language.clone()
                }
                ApplicationSettingsField::AutoLaunchDnd => {
                    if config.general.auto_launch_dnd {
                        app.state.t("settings-value-enabled")
                    } else {
                        app.state.t("settings-value-disabled")
                    }
                }
            };

            let restart_marker = if field.requires_restart() {
                format!(" {}", app.state.t("settings-restart-marker"))
            } else {
                String::new()
            };
            lines.push(Line::from(Span::styled(
                format!(
                    "{}{}{}: {}",
                    prefix,
                    app.state.t(field.label_key()),
                    restart_marker,
                    value
                ),
                style,
            )));

            // Show description for selected field
            if is_selected {
                lines.push(Line::from(Span::styled(
                    format!("   {}", app.state.t(field.description_key())),
                    Style::default().fg(description_color).add_modifier(Modifier::ITALIC),
                )));
            }
        }

        // Add constraint info (Downloads tab only: it explains the
        // MaxConcurrentPerFolder x MaxActiveFolders <= MaxConcurrent rule)
        if current_section == SettingsSection::Downloads {
            lines.push(Line::from(""));
            lines.push(Line::from(""));
            lines.push(Line::from(Span::styled(
                "Constraint:",
                Style::default().fg(section_header_color),
            )));

            let max_concurrent = config.download.max_concurrent;
            let max_per_folder = config
                .download
                .max_concurrent_per_folder
                .unwrap_or(max_concurrent);
            let active_folders = config.download.parallel_folder_count.unwrap_or(1);
            // Saturating: these come from unbounded user-entered config values.
            let calculated = max_per_folder.saturating_mul(active_folders);
            let constraint_met = calculated <= max_concurrent;

            let constraint_style = if constraint_met {
                Style::default().fg(success_color)
            } else {
                Style::default().fg(error_color)
            };

            lines.push(Line::from(Span::styled(
                format!(
                    "({} × {}) = {} {} {}",
                    max_per_folder,
                    active_folders,
                    calculated,
                    if constraint_met { "≤" } else { ">" },
                    max_concurrent
                ),
                constraint_style,
            )));

            if !constraint_met {
                lines.push(Line::from(Span::styled(
                    "⚠ Constraint violated! Values will be adjusted on save.",
                    Style::default().fg(error_color),
                )));
            }
        }

        // Script file rows (Scripts tab only): flattened right after
        // ScriptsEnabled, continuing the same row selection index instead of
        // a collapsible sub-list.
        if current_section == SettingsSection::Scripts {
            lines.push(Line::from(""));
            lines.push(Line::from(""));

            let script_files_config = config.scripts.script_files.clone();

            // List all script files (cached; refreshed on tick by the app loop so
            // rendering doesn't block on read_dir every frame).
            let script_files = app.state.cached_script_files.clone();
            let field_count = fields.len();

            let header_args = fluent_args! { "count" => script_files.len() };
            lines.push(Line::from(Span::styled(
                app.state.t_with_args("settings-scripts-header", Some(&header_args)),
                Style::default().fg(section_header_color).add_modifier(Modifier::BOLD),
            )));
            lines.push(Line::from(""));

            if script_files.is_empty() {
                lines.push(Line::from(Span::styled(
                    format!("  {}", app.state.t("settings-scripts-none")),
                    Style::default().fg(muted_color),
                )));
            } else {
                for (idx, filename) in script_files.iter().enumerate() {
                    let is_selected = field_count + idx == app.state.app_settings_field_index;
                    let is_enabled = script_files_config.get(filename).copied().unwrap_or(true);

                    let prefix = if is_selected { "▸ " } else { "  " };
                    let status = if is_enabled { "✓" } else { "✗" };

                    let style = if is_selected {
                        Style::default().fg(selected_color).add_modifier(Modifier::BOLD)
                    } else if is_enabled {
                        Style::default().fg(success_color)
                    } else {
                        Style::default().fg(error_color)
                    };

                    lines.push(Line::from(vec![
                        Span::styled(prefix, style),
                        Span::styled(format!("{} ", status), style),
                        Span::styled(filename.clone(), style),
                    ]));
                }

                lines.push(Line::from(""));
                lines.push(Line::from(Span::styled(
                    app.state.t("help-script-toggle"),
                    Style::default().fg(muted_color),
                )));
            }
        }
    } else {
        // Config briefly locked (e.g. mid-edit): show a stable placeholder
        // instead of a blank panel.
        lines.push(Line::from(Span::styled(
            "Loading configuration…",
            Style::default().fg(muted_color),
        )));
    }

    // Add help text
    lines.push(Line::from(""));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        app.state.t("help-edit-field"),
        Style::default().fg(success_color),
    )));
    lines.push(Line::from(Span::styled(
        app.state.t("settings-hint-switch-tab"),
        Style::default().fg(section_header_color),
    )));
    lines.push(Line::from(Span::styled(
        app.state.t("settings-hint-close"),
        Style::default().fg(muted_color),
    )));

    // App version + build revision
    lines.push(Line::from(""));
    lines.push(Line::from(""));
    lines.push(Line::from(Span::styled(
        format!("v{} ({})", env!("CARGO_PKG_VERSION"), env!("GIT_HASH")),
        Style::default().fg(muted_color),
    )));

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(app.state.t("dialog-details"))
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);
}

/// Render folder list (left panel)
fn render_folder_list(app: &TuiApp, f: &mut Frame, area: Rect) {
    // Modern color palette
    let selected_color = theme::ACCENT_SELECTED;
    let border_color = theme::BORDER_INACTIVE;
    let success_color = theme::STATUS_SUCCESS;
    let error_color = theme::STATUS_ERROR;
    let muted_color = theme::TEXT_MUTED;

    let mut folder_items = Vec::new();

    // Render from the per-tick folder snapshot (no config lock at render).
    let folder_entries = &app.state.cached_folder_entries;
    let folder_count = folder_entries.len();

    {
        for (idx, (_folder_id, display_name)) in folder_entries.iter().enumerate() {
            let is_selected = idx == app.state.settings_folder_index;
            let style = if is_selected {
                Style::default()
                    .fg(selected_color)
                    .add_modifier(Modifier::BOLD)
            } else {
                Style::default().fg(theme::TEXT_NORMAL)
            };

            let prefix = if is_selected {
                "▸ "
            } else {
                "  "
            };

            folder_items.push(Line::from(Span::styled(
                format!("{}{}", prefix, display_name),
                style,
            )));
        }
    }

    if folder_items.is_empty() {
        folder_items.push(Line::from(Span::styled(
            "No folders",
            Style::default().fg(muted_color),
        )));
    }

    // Add help text at bottom
    folder_items.push(Line::from(""));
    folder_items.push(Line::from(""));
    folder_items.push(Line::from(Span::styled(
        "n: new folder",
        Style::default().fg(success_color),
    )));
    folder_items.push(Line::from(Span::styled(
        "r: rename",
        Style::default().fg(theme::ACCENT_PURPLE),
    )));
    folder_items.push(Line::from(Span::styled(
        "d: delete",
        Style::default().fg(error_color),
    )));
    folder_items.push(Line::from(Span::styled(
        "x/Esc: close",
        Style::default().fg(muted_color),
    )));

    let paragraph = Paragraph::new(folder_items)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(app.state.t("dialog-folders"))
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);

    // Track clickable regions for folder items
    let inner_area = Rect {
        x: area.x + 1,
        y: area.y + 1,
        width: area.width.saturating_sub(2),
        height: area.height.saturating_sub(2),
    };

    let mut folder_item_rects = Vec::new();
    for idx in 0..folder_count {
        if idx < inner_area.height as usize {
            let item_rect = Rect {
                x: inner_area.x,
                y: inner_area.y + idx as u16,
                width: inner_area.width,
                height: 1,
            };
            folder_item_rects.push((idx, item_rect));
        }
    }

    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.settings_folder_items = folder_item_rects;
    }
}

/// Render folder details/editor (right panel)
fn render_folder_details(app: &TuiApp, f: &mut Frame, area: Rect) {
    use super::state::SettingsField;

    let config = app.state.app_state.config.try_read();
    let is_edit_mode = app.state.ui_mode == UiMode::FolderEdit;
    let field_index = app.state.settings_field_index;

    // Modern color palette
    let selected_color = theme::ACCENT_SELECTED;
    let section_header_color = theme::ACCENT_SECTION;
    let border_color = theme::BORDER_INACTIVE;
    let success_color = theme::STATUS_SUCCESS;
    let error_color = theme::STATUS_ERROR;
    let muted_color = theme::TEXT_MUTED;
    let text_color = theme::TEXT_NORMAL;

    let mut detail_lines = Vec::new();

    if let Ok(config) = config {
        // Get selected folder using sorted entries
        let folder_entries = config.sorted_folder_entries();

        let selected_folder = if app.state.settings_folder_index < folder_entries.len() {
            Some(folder_entries[app.state.settings_folder_index].clone())
        } else {
            None
        };

        if let Some((ref folder_id, ref display_name)) = selected_folder {
            if let Some(folder_config) = config.folders.get(folder_id) {
                detail_lines.push(Line::from(Span::styled(
                    format!("Folder: {}", display_name),
                    Style::default()
                        .fg(selected_color)
                        .add_modifier(Modifier::BOLD),
                )));
                detail_lines.push(Line::from(""));

                // Helper to create field line with selection indicator
                let make_field_line = |idx: usize, label: &str, value: String| {
                    let is_selected = is_edit_mode && field_index == idx;
                    let prefix = if is_selected { "▸ " } else { "  " };
                    let style = if is_selected {
                        Style::default().fg(selected_color).add_modifier(Modifier::BOLD)
                    } else {
                        Style::default().fg(text_color)
                    };
                    Line::from(Span::styled(format!("{}{}: {}", prefix, label, value), style))
                };

                // Field 0: Name
                detail_lines.push(make_field_line(
                    0,
                    &app.state.t("settings-folder-name"),
                    display_name.clone(),
                ));

                // Field 1: Save Path
                detail_lines.push(make_field_line(
                    1,
                    &app.state.t("settings-folder-save-path"),
                    folder_config.save_path.display().to_string(),
                ));

                // Field 2: Auto-Date Directory
                let auto_date_str = if folder_config.auto_date_directory {
                    app.state.t("settings-value-enabled")
                } else {
                    app.state.t("settings-value-disabled")
                };
                detail_lines.push(make_field_line(2, &app.state.t("settings-folder-auto-date"), auto_date_str));

                // Field 3: Auto-Start Downloads
                let auto_start_str = if folder_config.auto_start_downloads {
                    app.state.t("settings-value-enabled")
                } else {
                    app.state.t("settings-value-disabled")
                };
                detail_lines.push(make_field_line(3, &app.state.t("settings-folder-auto-start"), auto_start_str));

                // Field 4: Prevent Duplicate URL
                let prevent_dup_str = if folder_config.prevent_duplicate_url {
                    app.state.t("settings-value-enabled")
                } else {
                    app.state.t("settings-value-disabled")
                };
                detail_lines.push(make_field_line(4, &app.state.t("settings-folder-prevent-duplicate-url"), prevent_dup_str));

                // Field 5: Scripts
                let scripts_status = match folder_config.scripts_enabled {
                    Some(true) => app.state.t("settings-value-enabled-override"),
                    Some(false) => app.state.t("settings-value-disabled-override"),
                    None => app.state.t("settings-value-inherit"),
                };
                detail_lines.push(make_field_line(5, &app.state.t("settings-folder-scripts"), scripts_status));

                // Field 6: Max Concurrent
                let max_concurrent_str = folder_config
                    .max_concurrent
                    .map(|n| n.to_string())
                    .unwrap_or_else(|| app.state.t("settings-value-inherit"));
                detail_lines.push(make_field_line(6, &app.state.t("settings-folder-max-concurrent"), max_concurrent_str));

                // Field 7: User Agent
                let user_agent_str = folder_config
                    .user_agent
                    .clone()
                    .unwrap_or_else(|| app.state.t("settings-value-inherit"));
                detail_lines.push(make_field_line(7, &app.state.t("settings-folder-user-agent"), user_agent_str));

                // Field 8: Referrer Policy
                let referrer_policy_str = match &folder_config.referrer_policy {
                    Some(policy) => {
                        use crate::app::config::ReferrerPolicy;
                        match policy {
                            ReferrerPolicy::Custom { value, .. } => {
                                format!("{}: {}", app.state.t("settings-referrer-custom"), value)
                            }
                            other => app.state.t(other.display_key()),
                        }
                    }
                    None => app.state.t("settings-value-inherit"),
                };
                detail_lines.push(make_field_line(8, &app.state.t("settings-folder-referrer-policy"), referrer_policy_str));

                // Field 9: Headers
                let headers_str = if folder_config.default_headers.is_empty() {
                    app.state.t("settings-value-not-set")
                } else {
                    format!("{} headers", folder_config.default_headers.len())
                };
                detail_lines.push(make_field_line(9, &app.state.t("settings-folder-headers"), headers_str));

                // Show headers details if not empty
                if !folder_config.default_headers.is_empty() {
                    detail_lines.push(Line::from(""));
                    for (key, value) in &folder_config.default_headers {
                        detail_lines.push(Line::from(Span::styled(
                            format!("    {}: {}", key, value),
                            Style::default().fg(muted_color),
                        )));
                    }
                }

                // Script file rows (folder override): flattened right after
                // the fixed fields, continuing the same row selection index
                // instead of a collapsible sub-list.
                detail_lines.push(Line::from(""));
                detail_lines.push(Line::from(""));

                let app_script_files = config.scripts.script_files.clone();
                let folder_script_files = folder_config.script_files.as_ref();

                // List all script files (cached; refreshed on tick by the app
                // loop so rendering doesn't block on read_dir every frame).
                let script_files = app.state.cached_script_files.clone();
                let field_count = SettingsField::all().len();

                let header_args = fluent_args! { "count" => script_files.len() };
                detail_lines.push(Line::from(Span::styled(
                    app.state.t_with_args("settings-scripts-header", Some(&header_args)),
                    Style::default().fg(section_header_color).add_modifier(Modifier::BOLD),
                )));
                detail_lines.push(Line::from(""));

                if script_files.is_empty() {
                    detail_lines.push(Line::from(Span::styled(
                        format!("  {}", app.state.t("settings-scripts-none")),
                        Style::default().fg(muted_color),
                    )));
                } else {
                    for (idx, filename) in script_files.iter().enumerate() {
                        let is_selected = is_edit_mode && field_count + idx == field_index;

                        // Determine effective status (with inheritance)
                        let (status_char, status_text, style_color) = if let Some(folder_files) = folder_script_files {
                            if let Some(&enabled) = folder_files.get(filename) {
                                // Folder override
                                if enabled {
                                    ("✓", filename.clone(), success_color)
                                } else {
                                    ("✗", filename.clone(), error_color)
                                }
                            } else {
                                // Inherit from Application
                                let app_enabled = app_script_files.get(filename).copied().unwrap_or(true);
                                if app_enabled {
                                    ("○", format!("{} (inherit)", filename), muted_color)
                                } else {
                                    ("○", format!("{} (inherit)", filename), theme::TEXT_DIM)
                                }
                            }
                        } else {
                            // No folder override, all inherit
                            let app_enabled = app_script_files.get(filename).copied().unwrap_or(true);
                            if app_enabled {
                                ("○", format!("{} (inherit)", filename), muted_color)
                            } else {
                                ("○", format!("{} (inherit)", filename), theme::TEXT_DIM)
                            }
                        };

                        let prefix = if is_selected { "▸ " } else { "  " };

                        let style = if is_selected {
                            Style::default().fg(selected_color).add_modifier(Modifier::BOLD)
                        } else {
                            Style::default().fg(style_color)
                        };

                        detail_lines.push(Line::from(vec![
                            Span::styled(prefix, style),
                            Span::styled(format!("{} ", status_char), style),
                            Span::styled(status_text, style),
                        ]));
                    }

                    detail_lines.push(Line::from(""));
                    detail_lines.push(Line::from(Span::styled(
                        app.state.t("help-script-toggle"),
                        Style::default().fg(muted_color),
                    )));
                }

                detail_lines.push(Line::from(""));
            }
        } else {
            detail_lines.push(Line::from(Span::styled(
                "No folder selected",
                Style::default().fg(muted_color),
            )));
        }

        // Application settings summary at bottom
        detail_lines.push(Line::from(""));
        detail_lines.push(Line::from(Span::styled(
            "Application Settings:",
            Style::default()
                .fg(section_header_color)
                .add_modifier(Modifier::BOLD),
        )));
        detail_lines.push(Line::from(Span::styled(
            format!("  Max Concurrent: {}", config.download.max_concurrent),
            Style::default().fg(text_color),
        )));
        detail_lines.push(Line::from(Span::styled(
            format!("  Max Redirects: {}", config.download.max_redirects),
            Style::default().fg(text_color),
        )));
        detail_lines.push(Line::from(Span::styled(
            format!("  Retry Count: {}", config.download.retry_count),
            Style::default().fg(text_color),
        )));
        detail_lines.push(Line::from(Span::styled(
            format!("  Scripts: {}", if config.scripts.enabled { "Enabled" } else { "Disabled" }),
            Style::default().fg(text_color),
        )));
    } else {
        detail_lines.push(Line::from(Span::styled(
            "Unable to read configuration",
            Style::default().fg(error_color),
        )));
    }

    detail_lines.push(Line::from(""));

    // Show different help text based on mode
    if is_edit_mode {
        detail_lines.push(Line::from(Span::styled(
            app.state.t("help-folder-edit-navigate"),
            Style::default().fg(success_color),
        )));
        detail_lines.push(Line::from(Span::styled(
            "Toggle: auto-date, scripts | Input: save-path, max-concurrent, user-agent",
            Style::default().fg(muted_color),
        )));
    } else {
        detail_lines.push(Line::from(Span::styled(
            app.state.t("help-folder-list"),
            Style::default().fg(success_color),
        )));
    }

    let paragraph = Paragraph::new(detail_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .border_style(Style::default().fg(border_color))
                .title(app.state.t("dialog-folder-details"))
        )
        .wrap(Wrap { trim: false });

    f.render_widget(paragraph, area);

    // Show input dialog if editing a text field
    if let Some(field) = app.state.settings_edit_field {
        use super::state::SettingsField;
        match field {
            SettingsField::FolderName
            | SettingsField::FolderSavePath
            | SettingsField::FolderMaxConcurrent
            | SettingsField::FolderUserAgent => {
                render_field_edit_dialog(app, f, area, field);
            }
            _ => {}
        }
    }
}

/// Render input dialog for editing a field
fn render_field_edit_dialog(app: &TuiApp, f: &mut Frame, area: Rect, field: super::state::SettingsField) {
    let dialog_width = 60;
    let dialog_height = 5;

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    let args = fluent::fluent_args! {
        "label" => app.state.t(field.label_key()),
    };
    let title = app.state.t_with_args("dialog-edit-label", Some(&args));

    let input_widget = Paragraph::new(app.state.input_buffer.as_str())
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::default().fg(Color::Yellow)),
        )
        .style(Style::default().fg(Color::White));

    // Clear area and render dialog
    f.render_widget(Clear, dialog_area);
    f.render_widget(input_widget, dialog_area);
}

/// Render add download dialog (centered overlay)
fn render_add_download_dialog(app: &TuiApp, f: &mut Frame, area: Rect) {
    let dialog_width = 60;
    let dialog_height = 5;

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    let text = format!("{} {}", app.state.t("prompt-url"), app.state.input_buffer);
    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.state.t("dialog-add-download"))
                .style(Style::default().bg(theme::BG_PRIMARY)),
        );

    f.render_widget(paragraph, dialog_area);
}

/// Render generic input dialog with custom title and prompt (centered overlay)
fn render_input_dialog(app: &TuiApp, f: &mut Frame, area: Rect) {
    let has_error = app.state.validation_error.is_some();
    let dialog_width = 60;
    let dialog_height = if has_error { 8 } else { 5 };  // Expand for error

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    // Split into input area and optional error area
    let chunks = if has_error {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Length(5), Constraint::Length(3)])
            .split(dialog_area)
    } else {
        Layout::default()
            .direction(Direction::Vertical)
            .constraints([Constraint::Min(0)])
            .split(dialog_area)
    };

    // Render input field
    let text = format!("{} {}", app.state.input_prompt, app.state.input_buffer);
    let paragraph = Paragraph::new(text)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.state.input_title.clone())
                .style(Style::default().bg(theme::BG_PRIMARY)),
        );
    f.render_widget(Clear, chunks[0]);
    f.render_widget(paragraph, chunks[0]);

    // Render error message if present
    if let Some(ref error_msg) = app.state.validation_error {
        let error_para = Paragraph::new(error_msg.as_str())
            .block(
                Block::default()
                    .borders(Borders::TOP | Borders::LEFT | Borders::RIGHT)
                    .style(Style::default().bg(theme::BG_PRIMARY))
            )
            .style(Style::default().fg(Color::Red))
            .wrap(Wrap { trim: true });
        f.render_widget(Clear, chunks[1]);
        f.render_widget(error_para, chunks[1]);
    }
}

/// Render download preview dialog (centered overlay)
fn render_download_preview_dialog(app: &TuiApp, f: &mut Frame, area: Rect) {
    let dialog_width = 80;
    let dialog_height = 18;

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    let mut lines = Vec::new();

    // URL
    lines.push(Line::from(vec![
        Span::styled(
            format!("{} ", app.state.t("prompt-url")),
            Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)
        ),
        Span::raw(&app.state.input_buffer),
    ]));
    lines.push(Line::from(""));

    // Show preview info if available
    if let Some(ref info) = app.state.preview_info {
        // Filename
        let filename = info.filename.clone().unwrap_or_else(|| {
            app.state.input_buffer
                .split('/')
                .last()
                .unwrap_or("download")
                .to_string()
        });
        lines.push(Line::from(vec![
            Span::styled(
                format!("{} ", app.state.t("details-label-filename")),
                Style::default().add_modifier(Modifier::BOLD).fg(Color::Green)
            ),
            Span::raw(filename),
        ]));

        // File size
        if let Some(size) = info.size {
            let size_str = format_size(size);
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", app.state.t("details-label-size-icon")),
                    Style::default().add_modifier(Modifier::BOLD).fg(Color::Yellow)
                ),
                Span::raw(size_str),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(
                    format!("{} ", app.state.t("details-label-size-icon")),
                    Style::default().add_modifier(Modifier::BOLD).fg(Color::Yellow)
                ),
                Span::styled("Unknown", Style::default().fg(Color::DarkGray)),
            ]));
        }

        // Resume support
        let resume_text = if info.resume_supported { "✓ Yes" } else { "✗ No" };
        let resume_color = if info.resume_supported { Color::Green } else { Color::Red };
        lines.push(Line::from(vec![
            Span::styled("🔄 Resume Support: ", Style::default().add_modifier(Modifier::BOLD).fg(Color::Magenta)),
            Span::styled(resume_text, Style::default().fg(resume_color)),
        ]));

        // Last modified
        if let Some(ref last_modified) = info.last_modified {
            lines.push(Line::from(vec![
                Span::styled("📅 Last Modified: ", Style::default().add_modifier(Modifier::BOLD).fg(Color::Blue)),
                Span::raw(last_modified),
            ]));
        }

        // ETag
        if let Some(ref etag) = info.etag {
            lines.push(Line::from(vec![
                Span::styled("🏷️  ETag: ", Style::default().add_modifier(Modifier::BOLD).fg(Color::Cyan)),
                Span::raw(etag),
            ]));
        }
    } else {
        // Show loading/error message
        lines.push(Line::from(vec![
            Span::styled("⚠️  ", Style::default().fg(Color::Yellow)),
            Span::styled("Failed to fetch download information", Style::default().fg(Color::Yellow)),
        ]));
        lines.push(Line::from(""));
        lines.push(Line::from(vec![
            Span::raw("You can still proceed with the download, but file information is not available."),
        ]));
    }

    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("Press ", Style::default().fg(Color::DarkGray)),
        Span::styled("Enter", Style::default().fg(Color::Green).add_modifier(Modifier::BOLD)),
        Span::styled(" to confirm or ", Style::default().fg(Color::DarkGray)),
        Span::styled("Esc", Style::default().fg(Color::Red).add_modifier(Modifier::BOLD)),
        Span::styled(" to cancel", Style::default().fg(Color::DarkGray)),
    ]));
    lines.push(Line::from(""));
    lines.push(Line::from(vec![
        Span::styled("    [ Confirm (Enter) ]", Style::default().fg(Color::Green)),
        Span::raw("         "),
        Span::styled("[ Cancel (Esc) ]    ", Style::default().fg(Color::Red)),
    ]));

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.state.t("dialog-download-preview"))
                .style(Style::default().bg(theme::BG_PRIMARY)),
        )
        .wrap(Wrap { trim: true });

    // Clear area and render dialog
    f.render_widget(Clear, dialog_area);
    f.render_widget(paragraph, dialog_area);

    // Track button regions for mouse clicks
    let button_y = dialog_area.y + dialog_height - 2;
    let inner_width = dialog_width - 2;
    let center_x = dialog_area.x + 1 + inner_width / 2;

    // Confirm button: left side
    let confirm_button = Rect {
        x: center_x.saturating_sub(20),
        y: button_y,
        width: 20,
        height: 1,
    };

    // Cancel button: right side
    let cancel_button = Rect {
        x: center_x + 5,
        y: button_y,
        width: 18,
        height: 1,
    };

    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.dialog_buttons = vec![
            ("confirm".to_string(), confirm_button),
            ("cancel".to_string(), cancel_button),
        ];
    }
}

/// Render change folder dialog (centered overlay)
/// Render folder picker dialog for changing an item's application folder
fn render_change_folder_for_item_dialog(app: &TuiApp, f: &mut Frame, area: Rect) {
    // Render from the per-tick snapshot so the dialog never vanishes on a
    // transient config lock contention.
    let folder_entries = app.state.cached_folder_entries.clone();

    let selected_index = app.state.folder_picker_index;

    // Current folder of the selected item
    let current_folder_id = app
        .state
        .get_selected_download()
        .map(|task| task.folder_id.clone())
        .unwrap_or_default();

    let max_folder_width = folder_entries
        .iter()
        .map(|(_id, name)| UnicodeWidthStr::width(name.as_str()))
        .max()
        .unwrap_or(20);

    let dialog_width = (max_folder_width as u16 + 8).max(40).min(60);
    let dialog_height = (folder_entries.len() as u16 + 4).max(8).min(20);

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    let mut folder_lines = Vec::new();
    let mut folder_picker_rects = Vec::new();
    // Inner area starts 1 cell in from each border.
    let inner_x = dialog_area.x + 1;
    let inner_y = dialog_area.y + 1;
    let inner_width = dialog_area.width.saturating_sub(2);
    for (idx, (folder_id, display_name)) in folder_entries.iter().enumerate() {
        let is_selected = idx == selected_index;
        let is_current = folder_id == &current_folder_id;

        let prefix = if is_selected { "▶ " } else { "  " };
        let suffix = if is_current { " (current)" } else { "" };

        let style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if is_current {
            Style::default().fg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };

        folder_lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(display_name.clone(), style),
            Span::styled(suffix, Style::default().fg(Color::DarkGray)),
        ]));

        let row_y = inner_y + idx as u16;
        // Only register rows that actually fit within the dialog viewport.
        if row_y < dialog_area.y + dialog_area.height.saturating_sub(1) {
            folder_picker_rects.push((
                idx,
                Rect {
                    x: inner_x,
                    y: row_y,
                    width: inner_width,
                    height: 1,
                },
            ));
        }
    }

    let paragraph = Paragraph::new(folder_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.state.t("dialog-change-folder"))
                .style(Style::default().bg(theme::BG_PRIMARY)),
        )
        .alignment(Alignment::Left);

    f.render_widget(Clear, dialog_area);
    f.render_widget(paragraph, dialog_area);

    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.folder_picker_items = folder_picker_rects;
    }
}

/// Render change save path dialog (free text input)
fn render_change_save_path_dialog(app: &TuiApp, f: &mut Frame, area: Rect) {
    let dialog_width = 80;
    let dialog_height = 7;

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    // Get current path from selected download
    let current_path = app
        .state
        .get_selected_download()
        .map(|task| task.save_path.to_string_lossy().to_string())
        .unwrap_or_else(|| "No download selected".to_string());

    let lines = vec![
        Line::from(vec![
            Span::styled("Current: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&current_path),
        ]),
        Line::from(""),
        Line::from(vec![
            Span::styled("New: ", Style::default().add_modifier(Modifier::BOLD)),
            Span::raw(&app.state.input_buffer),
        ]),
    ];

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.state.t("dialog-change-save-path"))
                .style(Style::default().bg(theme::BG_PRIMARY)),
        );

    f.render_widget(paragraph, dialog_area);
}

/// Render confirm delete dialog (centered overlay)
fn render_confirm_delete_dialog(app: &TuiApp, f: &mut Frame, area: Rect) {
    let folder_target = match &app.state.delete_target {
        DeleteTarget::Folder(folder_id) => Some(folder_id),
        DeleteTarget::Downloads => None,
    };

    // The folder branch adds one extra line (the config-directory note), so
    // its dialog is one row taller than the download branch's.
    let dialog_width = 60;
    let dialog_height = if folder_target.is_some() { 10 } else { 9 };

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    let mut lines = if let Some(folder_id) = folder_target {
        let folder_name = app
            .state
            .folder_names
            .get(folder_id)
            .cloned()
            .unwrap_or_else(|| folder_id.clone());

        vec![
            Line::from(Span::styled(
                "Are you sure you want to delete this folder?",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("Folder: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(folder_name),
            ]),
            Line::from(Span::styled(
                app.state.t("confirm-delete-folder-config-note"),
                Style::default().fg(Color::Red),
            )),
        ]
    } else {
        // Get filename of selected download
        let filename = app
            .state
            .get_selected_download()
            .map(|task| truncate_filename(&task.filename, 50))
            .unwrap_or_else(|| "Unknown".to_string());

        vec![
            Line::from(Span::styled(
                "Are you sure you want to delete this download?",
                Style::default().fg(Color::Yellow).add_modifier(Modifier::BOLD),
            )),
            Line::from(""),
            Line::from(vec![
                Span::styled("File: ", Style::default().add_modifier(Modifier::BOLD)),
                Span::raw(filename),
            ]),
        ]
    };

    lines.extend([
        Line::from(""),
        Line::from(Span::styled(
            "Press Enter or Y to confirm, N or Esc to cancel",
            Style::default().fg(Color::Cyan),
        )),
        Line::from(""),
        Line::from(vec![
            Span::styled("      [ Yes (Y) ]", Style::default().fg(Color::Green)),
            Span::raw("       "),
            Span::styled("[ No (N) ]      ", Style::default().fg(Color::Red)),
        ]),
    ]);

    let paragraph = Paragraph::new(lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.state.t("dialog-confirm-delete"))
                .style(Style::default().bg(theme::BG_PRIMARY)),
        )
        .alignment(Alignment::Center);

    // Clear area and render dialog
    f.render_widget(Clear, dialog_area);
    f.render_widget(paragraph, dialog_area);

    // Track button regions for mouse clicks
    // Buttons are on the last content line (dialog_area.y + dialog_height - 2)
    let button_y = dialog_area.y + dialog_height - 2;
    let inner_width = dialog_width - 2;
    let center_x = dialog_area.x + 1 + inner_width / 2;

    // Yes button: approximately 6 chars left of center, 11 chars wide
    let yes_button = Rect {
        x: center_x.saturating_sub(15),
        y: button_y,
        width: 13,
        height: 1,
    };

    // No button: approximately 6 chars right of center, 10 chars wide
    let no_button = Rect {
        x: center_x + 3,
        y: button_y,
        width: 12,
        height: 1,
    };

    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.dialog_buttons = vec![
            ("yes".to_string(), yes_button),
            ("no".to_string(), no_button),
        ];
    }
}

/// Get status icon for download status
fn status_icon(app: &TuiApp, status: &DownloadStatus) -> String {
    match status {
        DownloadStatus::Pending => app.state.t("status-pending"),
        DownloadStatus::Downloading => app.state.t("status-downloading"),
        DownloadStatus::Paused => app.state.t("status-paused"),
        DownloadStatus::Completed => app.state.t("status-completed"),
        DownloadStatus::Error => app.state.t("status-error"),
        DownloadStatus::Deleted => app.state.t("status-deleted"),
    }
}

/// Get color for download status
fn status_color(status: &DownloadStatus) -> Color {
    match status {
        DownloadStatus::Pending => theme::DL_PENDING,
        DownloadStatus::Downloading => theme::DL_DOWNLOADING,
        DownloadStatus::Paused => theme::DL_PAUSED,
        DownloadStatus::Completed => theme::DL_COMPLETED,
        DownloadStatus::Error => theme::DL_ERROR,
        DownloadStatus::Deleted => theme::DL_DELETED,
    }
}

/// Render context menu (popup actions)
fn render_switch_folder_dialog(app: &TuiApp, f: &mut Frame, area: Rect) {
    // Render from the per-tick folder snapshot rather than taking the config
    // lock during render, so the dialog never vanishes on lock contention.
    let folder_entries = app.state.cached_folder_entries.clone();

    let selected_index = app.state.folder_picker_index;

    // Calculate dialog dimensions
    let max_folder_width = folder_entries
        .iter()
        .map(|(_id, name)| UnicodeWidthStr::width(name.as_str()))
        .max()
        .unwrap_or(20);

    let dialog_width = (max_folder_width as u16 + 8).max(40).min(60);
    let dialog_height = (folder_entries.len() as u16 + 4).max(8).min(20);

    let dialog_area = Rect {
        x: (area.width.saturating_sub(dialog_width)) / 2,
        y: (area.height.saturating_sub(dialog_height)) / 2,
        width: dialog_width,
        height: dialog_height,
    };

    // Create folder list lines
    let mut folder_lines = Vec::new();
    for (idx, (folder_id, display_name)) in folder_entries.iter().enumerate() {
        let is_selected = idx == selected_index;
        let is_current = folder_id == &app.state.current_folder_id;

        let prefix = if is_selected { "▶ " } else { "  " };
        let suffix = if is_current { " (current)" } else { "" };

        let style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else if is_current {
            Style::default()
                .fg(Color::Cyan)
        } else {
            Style::default().fg(Color::White)
        };

        folder_lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(display_name.clone(), style),
            Span::styled(suffix, Style::default().fg(Color::DarkGray)),
        ]));
    }

    let paragraph = Paragraph::new(folder_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.state.t("dialog-switch-folder"))
                .style(Style::default().bg(theme::BG_PRIMARY)),
        )
        .alignment(Alignment::Left);

    // Clear area and render dialog
    f.render_widget(Clear, dialog_area);
    f.render_widget(paragraph, dialog_area);
}

fn render_context_menu(app: &TuiApp, f: &mut Frame, area: Rect) {
    use super::state::ContextMenuAction;

    let menu_items = ContextMenuAction::all();
    let selected_index = app.state.context_menu_index;

    // Calculate menu dimensions
    let max_label_width = menu_items
        .iter()
        .map(|item| {
            app.state.t(item.label_key()).len()
                + item.key_hint(&app.state.keybinding_resolver).len()
                + 6 // +6 for spacing and brackets
        })
        .max()
        .unwrap_or(40);

    let menu_width = (max_label_width as u16 + 4).min(60); // +4 for borders and padding
    let menu_height = (menu_items.len() as u16 + 2).min(20); // +2 for borders

    let menu_area = Rect {
        x: (area.width.saturating_sub(menu_width)) / 2,
        y: (area.height.saturating_sub(menu_height)) / 2,
        width: menu_width,
        height: menu_height,
    };

    // Create menu lines
    let mut menu_lines = Vec::new();
    for (idx, action) in menu_items.iter().enumerate() {
        let is_selected = idx == selected_index;
        let prefix = if is_selected { "▶ " } else { "  " };

        let style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let key_hint_style = Style::default().fg(Color::DarkGray);

        menu_lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(app.state.t(action.label_key()), style),
            Span::raw("  "),
            Span::styled(
                format!("[{}]", action.key_hint(&app.state.keybinding_resolver)),
                key_hint_style,
            ),
        ]));
    }

    let paragraph = Paragraph::new(menu_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(app.state.t("dialog-actions"))
                .style(Style::default().bg(theme::BG_PRIMARY)),
        )
        .alignment(Alignment::Left);

    // Clear area and render menu
    f.render_widget(Clear, menu_area);
    f.render_widget(paragraph, menu_area);

    // Track clickable regions for menu items
    let inner_area = Rect {
        x: menu_area.x + 1,
        y: menu_area.y + 1,
        width: menu_area.width.saturating_sub(2),
        height: menu_area.height.saturating_sub(2),
    };

    let mut menu_item_rects = Vec::new();
    for (idx, _) in menu_items.iter().enumerate() {
        if idx < inner_area.height as usize {
            let item_rect = Rect {
                x: inner_area.x,
                y: inner_area.y + idx as u16,
                width: inner_area.width,
                height: 1,
            };
            menu_item_rects.push(item_rect);
        }
    }

    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.context_menu = Some(menu_area);
        regions.context_menu_items = menu_item_rects;
    }
}


/// Render folder context menu overlay
fn render_folder_context_menu(app: &TuiApp, f: &mut Frame, area: Rect) {
    use super::state::FolderContextMenuAction;

    let is_completed = app.state.is_viewing_completed_node();
    let menu_items = if is_completed {
        FolderContextMenuAction::all_for_completed()
    } else {
        FolderContextMenuAction::all_for_folder()
    };
    let selected_index = app.state.folder_context_menu_index;

    // Calculate menu dimensions
    let max_label_width = menu_items
        .iter()
        .map(|item| {
            app.state.t(item.label_key()).len()
                + item.key_hint(&app.state.keybinding_resolver).len()
                + 6
        })
        .max()
        .unwrap_or(40);

    let menu_width = (max_label_width as u16 + 4).min(60);
    let menu_height = (menu_items.len() as u16 + 2).min(20);

    let menu_area = Rect {
        x: (area.width.saturating_sub(menu_width)) / 2,
        y: (area.height.saturating_sub(menu_height)) / 2,
        width: menu_width,
        height: menu_height,
    };

    // Create menu lines
    let mut menu_lines = Vec::new();
    for (idx, action) in menu_items.iter().enumerate() {
        let is_selected = idx == selected_index;
        let prefix = if is_selected { "▶ " } else { "  " };

        let style = if is_selected {
            Style::default()
                .fg(Color::Yellow)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(Color::White)
        };

        let key_hint_style = Style::default().fg(Color::DarkGray);

        menu_lines.push(Line::from(vec![
            Span::styled(prefix, style),
            Span::styled(app.state.t(action.label_key()), style),
            Span::raw("  "),
            Span::styled(
                format!("[{}]", action.key_hint(&app.state.keybinding_resolver)),
                key_hint_style,
            ),
        ]));
    }

    // Get title based on context
    let title = if is_completed {
        app.state.t("dialog-history-actions")
    } else {
        app.state.t("dialog-folder-actions")
    };

    let paragraph = Paragraph::new(menu_lines)
        .block(
            Block::default()
                .borders(Borders::ALL)
                .title(title)
                .style(Style::default().bg(theme::BG_PRIMARY)),
        )
        .alignment(Alignment::Left);

    // Clear area and render menu
    f.render_widget(Clear, menu_area);
    f.render_widget(paragraph, menu_area);

    // Track clickable regions for menu items
    let inner_area = Rect {
        x: menu_area.x + 1,
        y: menu_area.y + 1,
        width: menu_area.width.saturating_sub(2),
        height: menu_area.height.saturating_sub(2),
    };

    let mut menu_item_rects = Vec::new();
    for (idx, _) in menu_items.iter().enumerate() {
        if idx < inner_area.height as usize {
            let item_rect = Rect {
                x: inner_area.x,
                y: inner_area.y + idx as u16,
                width: inner_area.width,
                height: 1,
            };
            menu_item_rects.push(item_rect);
        }
    }

    {
        let mut regions = app.state.click_regions.borrow_mut();
        regions.context_menu = Some(menu_area);
        regions.context_menu_items = menu_item_rects;
    }
}
