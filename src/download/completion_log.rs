/// Completion logging functionality
///
/// Appends completed downloads to application-wide JSONL log files.
/// Log files are organized by date: {config_dir}/logs/YYYYMMDD.jsonl

use super::task::DownloadTask;
use anyhow::Result;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::io::Write;
use uuid::Uuid;

/// Entry in completion log (subset of DownloadTask fields)
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CompletedEntry {
    /// Unique task ID
    pub id: Uuid,
    /// Download URL
    pub url: String,
    /// Final filename
    pub filename: String,
    /// Folder ID
    pub folder_id: String,
    /// File size in bytes
    pub size: Option<u64>,
    /// Download start timestamp
    pub started_at: Option<DateTime<Utc>>,
    /// Download completion timestamp
    pub completed_at: Option<DateTime<Utc>>,
    /// Download duration in seconds
    pub duration_secs: Option<f64>,
    /// Final status ("completed" or "error")
    pub status: String,
    /// Error message if status is "error"
    pub error_message: Option<String>,
}

impl From<&DownloadTask> for CompletedEntry {
    fn from(task: &DownloadTask) -> Self {
        // Calculate duration if both timestamps are available
        let duration_secs = if let (Some(start), Some(end)) = (task.started_at, task.completed_at)
        {
            Some((end - start).num_milliseconds() as f64 / 1000.0)
        } else {
            None
        };

        Self {
            id: task.id,
            url: task.url.clone(),
            filename: task.filename.clone(),
            folder_id: task.folder_id.clone(),
            size: task.size,
            started_at: task.started_at,
            completed_at: task.completed_at,
            duration_secs,
            status: format!("{:?}", task.status).to_lowercase(),
            error_message: task.error_message.clone(),
        }
    }
}

/// Appends completed download to application-wide log
///
/// Creates log directory if it doesn't exist.
/// Appends to {config_dir}/logs/YYYYMMDD.jsonl (one line per completion).
///
/// # Arguments
///
/// * `task` - The completed download task
///
/// # Errors
///
/// Returns error if:
/// - Failed to create logs directory
/// - Failed to open/write to log file
/// - Failed to serialize task to JSON
pub async fn append_completion(task: &DownloadTask) -> Result<()> {
    let logs_dir = crate::util::paths::get_logs_dir()?;

    // Generate log filename based on the current UTC date, matching the UTC
    // timestamps stored in each entry (and the app log's UTC daily rotation).
    let today = chrono::Utc::now().format("%Y%m%d").to_string();
    let log_file = logs_dir.join(format!("{}.jsonl", today));

    // Convert task to CompletedEntry and serialize to single-line JSON
    let entry = CompletedEntry::from(task);
    let json_line = serde_json::to_string(&entry)?;

    // The filesystem work — create_dir_all, open, append, and especially the
    // fsync — blocks for potentially many milliseconds. Run it on the blocking
    // pool so it doesn't stall the async runtime (and every other download's
    // progress) on each completion.
    let log_file_for_msg = log_file.clone();
    tokio::task::spawn_blocking(move || -> Result<()> {
        std::fs::create_dir_all(&logs_dir)?;
        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&log_file)?;
        writeln!(file, "{}", json_line)?;
        file.sync_all()?; // Ensure written to disk
        Ok(())
    })
    .await??;

    tracing::debug!(
        "Appended completion log: {} to {}",
        task.filename,
        log_file_for_msg.display()
    );

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::task::DownloadStatus;
    use std::path::PathBuf;

    #[test]
    fn test_completed_entry_from_task() {
        let task = DownloadTask {
            id: Uuid::new_v4(),
            url: "https://example.com/file.zip".to_string(),
            filename: "file.zip".to_string(),
            save_path: PathBuf::from("/downloads"),
            folder_id: "default".to_string(),
            size: Some(1024000),
            downloaded: 1024000,
            status: DownloadStatus::Completed,
            priority: 0,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
            headers: std::collections::HashMap::new(),
            user_agent: None,
            resume_supported: false,
            etag: None,
            last_modified: None,
            error_message: None,
            logs: Vec::new(),
            retry_count: 0,
            last_status_code: Some(200),
            filename_settled: false,
        };

        let entry = CompletedEntry::from(&task);

        assert_eq!(entry.id, task.id);
        assert_eq!(entry.url, "https://example.com/file.zip");
        assert_eq!(entry.filename, "file.zip");
        assert_eq!(entry.folder_id, "default");
        assert_eq!(entry.size, Some(1024000));
        assert_eq!(entry.status, "completed");
        assert!(entry.duration_secs.is_some());
    }

    #[test]
    fn test_completed_entry_serialization() {
        let entry = CompletedEntry {
            id: Uuid::new_v4(),
            url: "https://example.com/file.zip".to_string(),
            filename: "file.zip".to_string(),
            folder_id: "default".to_string(),
            size: Some(1024000),
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
            duration_secs: Some(300.5),
            status: "completed".to_string(),
            error_message: None,
        };

        // Should serialize to JSON
        let json = serde_json::to_string(&entry).unwrap();
        assert!(json.contains("\"url\":\"https://example.com/file.zip\""));
        assert!(json.contains("\"filename\":\"file.zip\""));
        assert!(json.contains("\"folder_id\":\"default\""));

        // Should be single line (no newlines)
        assert!(!json.contains('\n'));
    }

    #[tokio::test]
    async fn test_append_completion_creates_directory() {
        // This test verifies that the function doesn't panic
        // Actual file creation is tested in integration tests
        let task = DownloadTask {
            id: Uuid::new_v4(),
            url: "https://example.com/test.zip".to_string(),
            filename: "test.zip".to_string(),
            save_path: PathBuf::from("/downloads"),
            folder_id: "default".to_string(),
            size: Some(1024),
            downloaded: 1024,
            status: DownloadStatus::Completed,
            priority: 0,
            created_at: Utc::now(),
            started_at: Some(Utc::now()),
            completed_at: Some(Utc::now()),
            headers: std::collections::HashMap::new(),
            user_agent: None,
            resume_supported: false,
            etag: None,
            last_modified: None,
            error_message: None,
            logs: Vec::new(),
            retry_count: 0,
            last_status_code: Some(200),
            filename_settled: false,
        };

        // Should not panic (may fail if permissions issue)
        let _ = append_completion(&task).await;
    }
}
