//! Pure formatting helpers for the TUI (human-readable sizes/speeds, filename
//! truncation, and progress bars). Extracted from `ui.rs` so the rendering
//! module stays focused on widget layout.

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

// Size/speed formatting lives in core so other front ends share it.
pub(crate) use crate::util::format::{format_size, format_speed};

/// Truncate filename with ellipsis if too long, preserving extension.
/// Uses unicode-width for accurate display width (handles Japanese/CJK correctly).
pub(crate) fn truncate_filename(filename: &str, max_width: usize) -> String {
    // Use display width (accounts for East Asian characters = 2 cells)
    let display_width = filename.width();

    if display_width <= max_width {
        return filename.to_string();
    }

    // Try to preserve extension
    if let Some(dot_pos) = filename.rfind('.') {
        let (name, ext) = filename.split_at(dot_pos);
        let ext_width = ext.width();

        // If extension is reasonable (< 10 width), keep it
        if ext_width < 10 && ext_width + 3 < max_width {
            // Calculate how much width we can use for the name part
            let target_name_width = max_width.saturating_sub(ext_width + 3); // 3 for "..."

            if target_name_width > 0 {
                // Truncate name by width, not character count
                let mut truncated_name = String::new();
                let mut current_width = 0;

                for ch in name.chars() {
                    let ch_width = ch.width().unwrap_or(1);
                    if current_width + ch_width > target_name_width {
                        break;
                    }
                    truncated_name.push(ch);
                    current_width += ch_width;
                }

                return format!("{}...{}", truncated_name, ext);
            }
        }
    }

    // Fallback: simple truncation with ellipsis at end
    let target_width = max_width.saturating_sub(3);
    let mut truncated = String::new();
    let mut current_width = 0;

    for ch in filename.chars() {
        let ch_width = ch.width().unwrap_or(1);
        if current_width + ch_width > target_width {
            break;
        }
        truncated.push(ch);
        current_width += ch_width;
    }

    format!("{}...", truncated)
}

/// Create a visual progress bar using Unicode block characters.
/// Optimized to reduce allocations by using `String::with_capacity`.
pub(crate) fn format_progress_bar(downloaded: u64, total: Option<u64>, width: usize) -> String {
    if let Some(total) = total {
        if total == 0 {
            return "░".repeat(width);
        }

        let progress = (downloaded as f64 / total as f64).min(1.0);
        let filled = (progress * width as f64) as usize;
        let remaining = width.saturating_sub(filled);

        // Pre-allocate with exact capacity to avoid reallocations
        let mut bar = String::with_capacity(width * 3); // 3 bytes per UTF-8 character
        for _ in 0..filled {
            bar.push('█');
        }
        for _ in 0..remaining {
            bar.push('░');
        }
        bar
    } else {
        // Unknown total - show indeterminate progress
        "▓".repeat(width)
    }
}

/// Format progress percentage with a visual indicator.
pub(crate) fn format_progress_with_bar(downloaded: u64, total: Option<u64>) -> String {
    if let Some(total) = total {
        if total == 0 {
            return "N/A".to_string();
        }
        let percentage = (downloaded * 100 / total).min(100);
        let bar = format_progress_bar(downloaded, Some(total), 10);
        format!("{:>3}% {}", percentage, bar)
    } else {
        "N/A  ░░░░░░░░░░".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_truncate_filename_preserves_extension() {
        let out = truncate_filename("a_very_long_file_name_indeed.zip", 16);
        assert!(out.ends_with(".zip"));
        assert!(out.width() <= 16);
    }

    #[test]
    fn test_truncate_filename_short_unchanged() {
        assert_eq!(truncate_filename("short.txt", 50), "short.txt");
    }

    #[test]
    fn test_format_progress_bar() {
        assert_eq!(format_progress_bar(5, Some(10), 10), "█████░░░░░");
        assert_eq!(format_progress_bar(0, Some(0), 4), "░░░░");
    }
}
