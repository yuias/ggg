use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use wiremock::{Mock, MockServer, ResponseTemplate};
use wiremock::matchers::{method, path};

/// Setup a mock HTTP server for download testing
/// Returns (MockServer, base_url)
pub async fn setup_mock_download_server() -> (MockServer, String) {
    let server = MockServer::start().await;
    let uri = server.uri();

    // Mock HEAD request - returns file metadata
    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("Content-Length", "1024")
                .append_header("Accept-Ranges", "bytes")
                .append_header("ETag", "\"test-etag\"")
        )
        .mount(&server)
        .await;

    // Mock GET request - returns file content
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(vec![0u8; 1024])
                .append_header("Content-Length", "1024")
                .append_header("Accept-Ranges", "bytes")
        )
        .mount(&server)
        .await;

    (server, uri)
}

/// Setup a mock HTTP server whose GET response never completes within a
/// test's lifetime (HEAD stays fast), so a download started against it stays
/// active until the caller aborts it.
#[allow(dead_code)]
pub async fn setup_slow_mock_download_server() -> (MockServer, String) {
    let server = MockServer::start().await;
    let uri = server.uri();

    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("Content-Length", "1024")
                .append_header("Accept-Ranges", "bytes")
                .append_header("ETag", "\"test-etag\"")
        )
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(vec![0u8; 1024])
                .append_header("Content-Length", "1024")
                .append_header("Accept-Ranges", "bytes")
                .set_delay(Duration::from_secs(5))
        )
        .mount(&server)
        .await;

    (server, uri)
}

/// Setup a mock server for a specific file path with custom content
pub async fn setup_mock_file_server(file_path: &str, content: Vec<u8>) -> (MockServer, String) {
    let server = MockServer::start().await;
    let uri = server.uri();

    let content_length = content.len();

    // Mock HEAD request
    Mock::given(method("HEAD"))
        .and(path(file_path))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("Content-Length", content_length.to_string())
                .append_header("Accept-Ranges", "bytes")
        )
        .mount(&server)
        .await;

    // Mock GET request
    Mock::given(method("GET"))
        .and(path(file_path))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(content)
                .append_header("Content-Length", content_length.to_string())
        )
        .mount(&server)
        .await;

    (server, uri)
}

/// Setup a mock server that supports resumable downloads
#[allow(dead_code)]
pub async fn setup_resumable_mock_server(full_content: Vec<u8>) -> (MockServer, String) {
    let server = MockServer::start().await;
    let uri = server.uri();

    let content_length = full_content.len();

    // Mock HEAD request
    Mock::given(method("HEAD"))
        .respond_with(
            ResponseTemplate::new(200)
                .append_header("Content-Length", content_length.to_string())
                .append_header("Accept-Ranges", "bytes")
        )
        .mount(&server)
        .await;

    // Mock GET request for full download
    Mock::given(method("GET"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_bytes(full_content.clone())
                .append_header("Content-Length", content_length.to_string())
                .append_header("Accept-Ranges", "bytes")
        )
        .mount(&server)
        .await;

    (server, uri)
}

/// Setup a mock server that returns HTTP errors
#[allow(dead_code)]
pub async fn setup_error_mock_server(status_code: u16) -> (MockServer, String) {
    let server = MockServer::start().await;
    let uri = server.uri();

    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(status_code))
        .mount(&server)
        .await;

    Mock::given(method("HEAD"))
        .respond_with(ResponseTemplate::new(status_code))
        .mount(&server)
        .await;

    (server, uri)
}

/// Create a test download task
pub fn create_test_task(url: String, save_path: PathBuf) -> ggg::download::task::DownloadTask {
    ggg::download::task::DownloadTask::new(url, save_path)
}

/// Create a test download task with a specific filename
pub fn create_test_task_with_filename(
    url: String,
    save_path: PathBuf,
    filename: String,
) -> ggg::download::task::DownloadTask {
    let mut task = ggg::download::task::DownloadTask::new(url, save_path);
    task.filename = filename;
    task
}

/// Create a test configuration for download manager tests
#[allow(dead_code)]
pub fn create_test_config() -> Arc<RwLock<ggg::app::config::Config>> {
    Arc::new(RwLock::new(ggg::app::config::Config::default()))
}

/// Create a test download manager with minimal retry settings for faster testing
#[allow(dead_code)]
pub fn create_test_manager() -> ggg::download::manager::DownloadManager {
    // Use minimal retry settings for tests:
    // - max_concurrent: 3 (global limit)
    // - max_concurrent_per_folder: 3 (folder limit)
    // - parallel_folder_count: 2 (active folder limit)
    // - max_retries: 0 (no retries for faster test execution)
    // - retry_delay_secs: 1 (minimal delay if retries are needed)
    ggg::download::manager::DownloadManager::with_config(3, 3, 2, 0, 1)
}

/// Generate test file content of a specific size
pub fn generate_test_content(size: usize) -> Vec<u8> {
    (0..size).map(|i| (i % 256) as u8).collect()
}

/// Helper to wait for a task to reach a specific status
#[allow(dead_code)]
pub async fn wait_for_status(
    manager: &ggg::download::manager::DownloadManager,
    task_id: uuid::Uuid,
    expected_status: ggg::download::task::DownloadStatus,
    timeout_secs: u64,
) -> Result<(), String> {
    use tokio::time::{timeout, Duration};

    timeout(Duration::from_secs(timeout_secs), async {
        loop {
            if let Some(task) = manager.get_by_id(task_id).await {
                if task.status == expected_status {
                    return Ok(());
                }
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .map_err(|_| format!("Timeout waiting for status {:?}", expected_status))?
}

/// Helper to verify file contents match expected
#[allow(dead_code)]
pub fn verify_file_content(path: &std::path::Path, expected: &[u8]) -> Result<(), String> {
    let content = std::fs::read(path)
        .map_err(|e| format!("Failed to read file: {}", e))?;

    if content == expected {
        Ok(())
    } else {
        Err(format!(
            "File content mismatch: expected {} bytes, got {} bytes",
            expected.len(),
            content.len()
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_mock_server_setup() {
        let (_server, uri) = setup_mock_download_server().await;
        assert!(!uri.is_empty());
        assert!(uri.starts_with("http://"));
    }

    #[tokio::test]
    async fn test_mock_file_server() {
        let content = b"test content".to_vec();
        let (_server, uri) = setup_mock_file_server("/test.txt", content.clone()).await;

        // Verify server is running
        assert!(!uri.is_empty());
    }

    #[test]
    fn test_generate_content() {
        let content = generate_test_content(100);
        assert_eq!(content.len(), 100);
        assert_eq!(content[0], 0);
        assert_eq!(content[99], 99);
    }

    #[test]
    fn test_create_test_task() {
        let task = create_test_task(
            "http://example.com/file.zip".to_string(),
            PathBuf::from("/tmp"),
        );
        assert_eq!(task.url, "http://example.com/file.zip");
        assert_eq!(task.save_path, PathBuf::from("/tmp"));
    }

    #[test]
    fn test_create_test_task_with_filename() {
        let task = create_test_task_with_filename(
            "http://example.com/file.zip".to_string(),
            PathBuf::from("/tmp"),
            "custom.zip".to_string(),
        );
        assert_eq!(task.filename, "custom.zip");
    }
}
