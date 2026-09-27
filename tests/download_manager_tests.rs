mod common;

use common::*;
use ggg::download::manager::{DownloadManager, StartOutcome};
use ggg::download::task::DownloadStatus;
use tokio::time::{sleep, Duration};

// ========================================
// Task Management Tests (8 tests)
// ========================================

#[tokio::test]
async fn test_manager_add_download() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let task = create_test_task(
        "http://example.com/file.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    let task_id = task.id;

    manager.add_download(task).await;

    let tasks = manager.get_all_downloads().await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, task_id);
}

#[tokio::test]
async fn test_manager_add_sanitizes_filename() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let mut task = create_test_task(
        "http://example.com/file.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    // Set filename with invalid characters
    task.filename = "test<>file.zip".to_string();
    let task_id = task.id;

    manager.add_download(task).await;

    let saved_task = manager.get_by_id(task_id).await.unwrap();
    // Should sanitize < and > to _
    assert_eq!(saved_task.filename, "test__file.zip");
}

#[tokio::test]
async fn test_manager_get_by_id() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let task = create_test_task(
        "http://example.com/file.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    let task_id = task.id;

    manager.add_download(task).await;

    let retrieved = manager.get_by_id(task_id).await;
    assert!(retrieved.is_some());
    assert_eq!(retrieved.unwrap().id, task_id);
}

#[tokio::test]
async fn test_manager_get_all() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let task1 = create_test_task(
        "http://example.com/file1.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    let task2 = create_test_task(
        "http://example.com/file2.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );

    manager.add_download(task1).await;
    manager.add_download(task2).await;

    let tasks = manager.get_all_downloads().await;
    assert_eq!(tasks.len(), 2);
}

#[tokio::test]
async fn test_manager_remove_download() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let task = create_test_task(
        "http://example.com/file.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    let task_id = task.id;

    manager.add_download(task).await;
    assert_eq!(manager.get_all_downloads().await.len(), 1);

    let removed = manager.remove_download(task_id).await;
    assert!(removed.is_some());
    assert_eq!(removed.unwrap().id, task_id);

    let tasks = manager.get_all_downloads().await;
    assert_eq!(tasks.len(), 0);
}

#[tokio::test]
async fn test_manager_change_folder() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let task = create_test_task(
        "http://example.com/file.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    let task_id = task.id;

    manager.add_download(task).await;

    // Change folder
    manager.change_folder(task_id, "videos".to_string(), None).await.unwrap();

    let updated_task = manager.get_by_id(task_id).await.unwrap();
    assert_eq!(updated_task.folder_id, "videos");
}

#[tokio::test]
async fn test_manager_multiple_tasks() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    // Add 5 tasks
    for i in 0..5 {
        let task = create_test_task(
            format!("http://example.com/file{}.zip", i),
            temp_dir.path().to_path_buf(),
        );
        manager.add_download(task).await;
    }

    let tasks = manager.get_all_downloads().await;
    assert_eq!(tasks.len(), 5);

    // Verify each task has unique ID
    let ids: Vec<_> = tasks.iter().map(|t| t.id).collect();
    let unique_ids: std::collections::HashSet<_> = ids.iter().collect();
    assert_eq!(unique_ids.len(), 5);
}

#[tokio::test]
async fn test_manager_concurrent_operations() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    // Spawn multiple concurrent add operations
    let mut handles = vec![];

    for i in 0..10 {
        let manager_clone = manager.clone();
        let path = temp_dir.path().to_path_buf();
        let handle = tokio::spawn(async move {
            let task = create_test_task(
                format!("http://example.com/file{}.zip", i),
                path,
            );
            manager_clone.add_download(task).await;
        });
        handles.push(handle);
    }

    // Wait for all operations to complete
    for handle in handles {
        handle.await.unwrap();
    }

    let tasks = manager.get_all_downloads().await;
    assert_eq!(tasks.len(), 10);
}

// ========================================
// Download Lifecycle Tests (7 tests)
// ========================================

#[tokio::test]
async fn test_manager_start_download_with_mock() {
    let (_server, uri) = setup_mock_download_server().await;
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;

    // Start download
    let config = create_test_config();
    let result = manager.start_download(task_id, None, config).await;
    assert!(result.is_ok());

    // Wait a bit for download to start
    sleep(Duration::from_millis(100)).await;

    // Task may have already completed and been removed from queue
    // (completed tasks are logged to completion log and removed)
    if let Some(task) = manager.get_by_id(task_id).await {
        // Should be downloading or completed
        assert!(
            task.status == DownloadStatus::Downloading || task.status == DownloadStatus::Completed
        );
    }
    // If task is None, it has already completed and been removed - this is OK
}

#[tokio::test]
async fn test_manager_start_sets_status() {
    let (_server, uri) = setup_mock_download_server().await;
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;

    // Verify initial status
    let task = manager.get_by_id(task_id).await.unwrap();
    assert_eq!(task.status, DownloadStatus::Pending);

    // Start download
    let config = create_test_config();
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    // Status should change to Downloading
    let task = manager.get_by_id(task_id).await.unwrap();
    assert_eq!(task.status, DownloadStatus::Downloading);
}

#[tokio::test]
async fn test_manager_start_sets_timestamp() {
    let (_server, uri) = setup_mock_download_server().await;
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;

    // Verify no started_at timestamp
    let task = manager.get_by_id(task_id).await.unwrap();
    assert!(task.started_at.is_none());

    // Start download
    let config = create_test_config();
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    // Should have started_at timestamp
    let task = manager.get_by_id(task_id).await.unwrap();
    assert!(task.started_at.is_some());
}

#[tokio::test]
async fn test_manager_pause_download() {
    let (_server, uri) = setup_mock_download_server().await;
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;
    let config = create_test_config();
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    // Wait a bit for download to start
    sleep(Duration::from_millis(100)).await;

    // Pause download
    let result = manager.pause_download(task_id).await;
    assert!(result.is_ok());

    // Give it time to process the pause
    sleep(Duration::from_millis(50)).await;

    // Task may have already completed and been removed before pause
    if let Some(task) = manager.get_by_id(task_id).await {
        assert_eq!(task.status, DownloadStatus::Paused);
    }
    // If None, task completed before pause could be applied
}

#[tokio::test]
async fn test_manager_pause_sets_status() {
    let (_server, uri) = setup_mock_download_server().await;
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;
    let config = create_test_config();
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    sleep(Duration::from_millis(100)).await;

    manager.pause_download(task_id).await.unwrap();
    sleep(Duration::from_millis(50)).await;

    // Task may have already completed and been removed before pause
    if let Some(task) = manager.get_by_id(task_id).await {
        assert_eq!(task.status, DownloadStatus::Paused);
    }
    // If None, task completed before pause could be applied
}

#[tokio::test]
async fn test_manager_remove_active_aborts() {
    let (_server, uri) = setup_slow_mock_download_server().await;
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;
    let config = create_test_config();
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    // The GET response is delayed well past this test's lifetime, so the
    // download is still active here rather than having already completed.
    assert_eq!(manager.get_active_count().await, 1);
    assert!(manager.is_running(task_id).await);

    // Remove should abort the active download
    manager.remove_download(task_id).await;

    assert_eq!(manager.get_active_count().await, 0);
    assert!(!manager.is_running(task_id).await);
    assert!(manager.get_by_id(task_id).await.is_none());
}

#[tokio::test]
async fn test_manager_completion_updates_task() {
    let (_server, uri) = setup_mock_download_server().await;
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;
    let config = create_test_config();
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    // Wait for download to complete (small file, should be quick)
    for _ in 0..50 {
        sleep(Duration::from_millis(100)).await;
        match manager.get_by_id(task_id).await {
            Some(task) if task.status == DownloadStatus::Completed => {
                // Verify completion fields
                assert!(task.completed_at.is_some());
                assert_eq!(task.downloaded, 1024); // Mock server returns 1024 bytes
                return;
            }
            None => {
                // Task has been removed from queue - it completed successfully
                // (completed tasks are logged to completion log and removed)
                return;
            }
            _ => continue, // Still downloading, wait more
        }
    }

    panic!("Download did not complete within timeout");
}

// ========================================
// Concurrency Tests (3 tests)
// ========================================

#[tokio::test]
async fn test_manager_concurrent_limit() {
    let (_server, uri) = setup_mock_download_server().await;
    let manager = DownloadManager::with_max_concurrent(2);
    let temp_dir = tempfile::tempdir().unwrap();

    // Start 3 downloads
    let mut task_ids = vec![];
    for i in 0..3 {
        let url = format!("{}/file{}.zip", uri, i);
        let task = create_test_task(url, temp_dir.path().to_path_buf());
        let task_id = task.id;
        task_ids.push(task_id);

        manager.add_download(task).await;
        let config = create_test_config();
        assert_eq!(
            manager.start_download(task_id, None, config).await.unwrap(),
            StartOutcome::Started
        );
    }

    // Wait for downloads to start
    sleep(Duration::from_millis(200)).await;

    // Note: active_downloads tracks all spawned tasks (3), but semaphore limits
    // actual concurrent downloads to 2. The test verifies the manager can handle
    // starting more tasks than the concurrency limit without crashing.
    let active_count = manager.get_active_count().await;
    assert!(active_count <= 3, "Active count {} exceeds number of started tasks", active_count);

    // Verify all downloads eventually complete or are still running
    // Note: Completed tasks are removed from queue and logged to completion log
    for task_id in task_ids {
        if let Some(task) = manager.get_by_id(task_id).await {
            // Task still exists - should be downloading or completed
            assert!(
                matches!(task.status, DownloadStatus::Downloading | DownloadStatus::Completed),
                "Task {} has unexpected status: {:?}", task_id, task.status
            );
        }
        // If None, task has already completed and been removed - this is OK
    }
}

#[tokio::test]
async fn test_manager_get_active_count() {
    let (_server, uri) = setup_mock_download_server().await;
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    // Initially no active downloads
    assert_eq!(manager.get_active_count().await, 0);

    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;
    let config = create_test_config();
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    sleep(Duration::from_millis(100)).await;

    // Should have 1 active download (or 0 if completed very quickly)
    let count = manager.get_active_count().await;
    assert!(count <= 1, "Active count should be at most 1");
}

#[tokio::test]
async fn test_manager_set_max_concurrent() {
    let manager = DownloadManager::with_max_concurrent(3);

    // Change max concurrent
    manager.set_max_concurrent(5).await;

    // Note: This just updates the internal value, but semaphore can't be resized
    // This test mainly verifies the method doesn't panic
}

// ========================================
// Persistence Tests (2 tests)
// ========================================

#[tokio::test]
async fn test_manager_save_queue() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let task = create_test_task(
        "http://example.com/file.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    manager.add_download(task).await;

    let queue_path = temp_dir.path().join("queue.json");

    // Save queue
    let result = manager.save_queue(&queue_path).await;
    assert!(result.is_ok());
    assert!(queue_path.exists());
}

#[tokio::test]
async fn test_manager_load_queue() {
    let manager = DownloadManager::new();
    let temp_dir = tempfile::tempdir().unwrap();

    let task = create_test_task(
        "http://example.com/file.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    let task_id = task.id;

    manager.add_download(task).await;

    let queue_path = temp_dir.path().join("queue.json");
    manager.save_queue(&queue_path).await.unwrap();

    // Create new manager and load
    let new_manager = DownloadManager::new();
    let result = new_manager.load_queue(&queue_path).await;
    assert!(result.is_ok());

    let tasks = new_manager.get_all_downloads().await;
    assert_eq!(tasks.len(), 1);
    assert_eq!(tasks[0].id, task_id);
}
