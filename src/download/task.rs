use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use chrono::{DateTime, Utc};
use uuid::Uuid;

/// Log entry for download events
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: DateTime<Utc>,
    pub level: LogLevel,
    pub message: String,
}

/// Log level for entries
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LogLevel {
    Info,
    Warn,
    Error,
}

impl LogEntry {
    pub fn info(message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            level: LogLevel::Info,
            message: message.into(),
        }
    }

    pub fn warn(message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            level: LogLevel::Warn,
            message: message.into(),
        }
    }

    pub fn error(message: impl Into<String>) -> Self {
        Self {
            timestamp: Utc::now(),
            level: LogLevel::Error,
            message: message.into(),
        }
    }
}

/// Represents a single download task
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadTask {
    pub id: Uuid,
    pub url: String,
    pub filename: String,
    pub save_path: PathBuf,
    pub folder_id: String,
    pub size: Option<u64>,
    pub downloaded: u64,
    pub status: DownloadStatus,
    pub priority: i32,
    pub created_at: DateTime<Utc>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub headers: std::collections::HashMap<String, String>,
    pub user_agent: Option<String>,
    pub resume_supported: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub error_message: Option<String>,
    pub logs: Vec<LogEntry>,
    pub retry_count: u32,
    pub last_status_code: Option<u16>,
    /// True once some attempt resolved and deduped `filename` for this
    /// task's current `folder_id`/`save_path` -- i.e. `filename` is verified
    /// to identify this task's own on-disk file, not one borrowed from an
    /// unrelated file that happens to share the name a server/redirect would
    /// otherwise re-derive. A retry may only treat itself as resuming when
    /// this is true; `#[serde(default)]` makes an older queue.toml (missing
    /// the field) default to `false`, the safe choice.
    #[serde(default)]
    pub filename_settled: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DownloadStatus {
    Pending,
    Downloading,
    Paused,
    Completed,
    Error,
    Deleted,
}

/// Derive a filename from a URL's last path segment, falling back to
/// "download" when the URL ends in `/` (empty segment) or has no path.
fn filename_from_url(url: &str) -> String {
    let name = url
        .split('/')
        .last()
        .unwrap_or("")
        .split('?')
        .next()
        .unwrap_or("");
    if name.is_empty() {
        "download".to_string()
    } else {
        name.to_string()
    }
}

impl DownloadTask {
    pub fn new(url: String, save_path: PathBuf) -> Self {
        let filename = filename_from_url(&url);

        let mut task = Self {
            id: Uuid::new_v4(),
            url,
            filename,
            save_path,
            folder_id: "default".to_string(),
            size: None,
            downloaded: 0,
            status: DownloadStatus::Pending,
            priority: 0,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            headers: std::collections::HashMap::new(),
            user_agent: None,
            resume_supported: false,
            etag: None,
            last_modified: None,
            error_message: None,
            logs: Vec::new(),
            retry_count: 0,
            last_status_code: None,
            filename_settled: false,
        };
        task.logs.push(LogEntry::info("Download task created"));
        task
    }

    /// Create a new task with folder settings applied
    pub fn new_with_folder(
        url: String,
        folder_id: String,
        config: &crate::app::config::Config,
    ) -> Self {
        let folder_config = config.folders.get(&folder_id);

        // Determine save_path from folder or app default
        let save_path = folder_config
            .map(|f| f.save_path.clone())
            .unwrap_or_else(|| config.download.default_directory.clone());

        // Apply folder defaults for headers
        let headers = folder_config
            .map(|f| f.default_headers.clone())
            .unwrap_or_default();

        // Apply folder default user agent
        let user_agent = folder_config.and_then(|f| f.user_agent.clone());

        let filename = filename_from_url(&url);

        let mut task = Self {
            id: Uuid::new_v4(),
            url,
            filename,
            save_path,
            folder_id: folder_id.clone(),
            size: None,
            downloaded: 0,
            status: DownloadStatus::Pending,
            priority: 0,
            created_at: Utc::now(),
            started_at: None,
            completed_at: None,
            headers,
            user_agent,
            resume_supported: false,
            etag: None,
            last_modified: None,
            error_message: None,
            logs: Vec::new(),
            retry_count: 0,
            last_status_code: None,
            filename_settled: false,
        };
        task.logs.push(LogEntry::info(format!("Download task created in folder '{}'", folder_id)));
        task
    }

    /// Add an info log entry
    pub fn log_info(&mut self, message: String) {
        self.logs.push(LogEntry::info(message));
    }

    /// Add a warning log entry
    pub fn log_warn(&mut self, message: String) {
        self.logs.push(LogEntry::warn(message));
    }

    /// Add an error log entry
    pub fn log_error(&mut self, message: String) {
        self.logs.push(LogEntry::error(message));
    }

    /// Calculate current download speed in bytes per second
    /// Returns None if the task is not actively downloading
    pub fn speed(&self) -> Option<f64> {
        if self.status != DownloadStatus::Downloading {
            return None;
        }

        let started = self.started_at?;
        let elapsed = Utc::now().signed_duration_since(started);
        let elapsed_secs = elapsed.num_milliseconds() as f64 / 1000.0;

        if elapsed_secs > 0.0 && self.downloaded > 0 {
            Some(self.downloaded as f64 / elapsed_secs)
        } else {
            None
        }
    }

    /// Calculate estimated time remaining in seconds
    /// Returns None if speed is zero, size is unknown, or already completed
    pub fn eta_seconds(&self) -> Option<u64> {
        if self.status != DownloadStatus::Downloading {
            return None;
        }
        
        let total_size = self.size?;
        let remaining = total_size.saturating_sub(self.downloaded);
        
        if remaining == 0 {
            return Some(0);
        }
        
        let speed = self.speed()?;
        if speed > 0.0 {
            Some((remaining as f64 / speed) as u64)
        } else {
            None
        }
    }

    /// Format ETA as human-readable string (e.g., "2h 15m", "45s")
    pub fn eta_display(&self) -> Option<String> {
        let seconds = self.eta_seconds()?;
        Some(format_duration(seconds))
    }

    /// Replace any existing key equal ignoring ASCII case, then insert `name`
    /// as given (so a later lookup sees the caller's exact casing).
    pub fn set_header(&mut self, name: &str, value: &str) {
        self.remove_header(name);
        self.headers.insert(name.to_string(), value.to_string());
    }

    /// Remove a header matching `name` case-insensitively. Returns whether a
    /// header was removed.
    pub fn remove_header(&mut self, name: &str) -> bool {
        let existing_key = self
            .headers
            .keys()
            .find(|k| k.eq_ignore_ascii_case(name))
            .cloned();
        match existing_key {
            Some(key) => {
                self.headers.remove(&key);
                true
            }
            None => false,
        }
    }
}

/// Format duration in seconds to human-readable string
pub fn format_duration(seconds: u64) -> String {
    if seconds < 60 {
        format!("{}s", seconds)
    } else if seconds < 3600 {
        let mins = seconds / 60;
        let secs = seconds % 60;
        if secs > 0 {
            format!("{}m {}s", mins, secs)
        } else {
            format!("{}m", mins)
        }
    } else {
        let hours = seconds / 3600;
        let mins = (seconds % 3600) / 60;
        if mins > 0 {
            format!("{}h {}m", hours, mins)
        } else {
            format!("{}h", hours)
        }
    }
}

#[cfg(test)]
mod filename_tests {
    use super::filename_from_url;

    #[test]
    fn derives_last_path_segment() {
        assert_eq!(filename_from_url("https://host/path/file.zip"), "file.zip");
        assert_eq!(filename_from_url("https://host/file.zip?a=1"), "file.zip");
    }

    #[test]
    fn falls_back_when_empty() {
        // URL ending in '/' yields an empty segment -> fallback.
        assert_eq!(filename_from_url("https://host/path/"), "download");
        assert_eq!(filename_from_url("https://host/"), "download");
    }
}

#[cfg(test)]
mod header_tests {
    use super::{DownloadTask, PathBuf};

    #[test]
    fn set_header_replaces_case_insensitive() {
        let mut task = DownloadTask::new("https://host/f".to_string(), PathBuf::from("."));
        task.headers.insert("referer".to_string(), "https://old/".to_string());
        task.set_header("Referer", "https://new/");

        assert_eq!(task.headers.get("Referer").map(String::as_str), Some("https://new/"));
        assert!(!task.headers.contains_key("referer"));
        assert_eq!(task.headers.len(), 1);
    }

    #[test]
    fn remove_header_case_insensitive() {
        let mut task = DownloadTask::new("https://host/f".to_string(), PathBuf::from("."));
        task.headers.insert("X-Test".to_string(), "1".to_string());

        assert!(task.remove_header("x-test"));
        assert!(task.headers.is_empty());
        assert!(!task.remove_header("x-test"));
    }
}
