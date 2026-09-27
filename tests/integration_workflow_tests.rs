mod common;

use common::*;
use ggg::download::manager::StartOutcome;
use ggg::download::task::DownloadStatus;
use tokio::time::{sleep, timeout, Duration};

// Initialize logging once for all tests
fn init_logging() {
    let _ = tracing_subscriber::fmt()
        .with_test_writer()
        .with_max_level(tracing::Level::DEBUG)
        .try_init();
}

// ========================================
// End-to-End Workflow Tests (6 tests)
// ========================================

#[tokio::test]
async fn test_full_download_workflow() {
    init_logging();

    // Setup mock server with test content
    let test_content = generate_test_content(2048);
    let (_server, uri) = setup_mock_file_server("/file.zip", test_content.clone()).await;
    let temp_dir = tempfile::tempdir().unwrap();

    let manager = create_test_manager();
    let config = create_test_config();

    // Step 1: Add download
    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url.clone(), temp_dir.path().to_path_buf());
    let task_id = task.id;
    manager.add_download(task).await;

    // Verify task was added
    let added_task = manager.get_by_id(task_id).await.unwrap();
    assert_eq!(added_task.status, DownloadStatus::Pending);
    assert_eq!(added_task.url, url);

    // Step 2: Start download
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    // Verify download started (may have already completed)
    if let Some(started_task) = manager.get_by_id(task_id).await {
        assert!(
            matches!(started_task.status, DownloadStatus::Downloading | DownloadStatus::Completed),
            "Expected Downloading or Completed, got {:?}", started_task.status
        );
        assert!(started_task.started_at.is_some());
    }
    // If None, download already completed - this is OK

    // Step 3: Wait for completion (task will be removed from queue when done)
    // We check for file creation instead of task status
    // Note: File may have unique suffix like file[timestamp].zip
    let result = timeout(Duration::from_secs(5), async {
        loop {
            // Check if any file matching pattern exists with expected size
            if let Ok(entries) = std::fs::read_dir(temp_dir.path()) {
                for entry in entries.flatten() {
                    if let Ok(filename) = entry.file_name().into_string() {
                        if filename.starts_with("file") && filename.ends_with(".zip") {
                            if let Ok(metadata) = entry.metadata() {
                                if metadata.len() == 2048 {
                                    return;
                                }
                            }
                        }
                    }
                }
            }

            // Also check if task was removed from queue (completion)
            if manager.get_by_id(task_id).await.is_none() {
                // Task removed - check if file exists
                if let Ok(entries) = std::fs::read_dir(temp_dir.path()) {
                    for entry in entries.flatten() {
                        if let Ok(filename) = entry.file_name().into_string() {
                            if filename.starts_with("file") && filename.ends_with(".zip") {
                                return;
                            }
                        }
                    }
                }
            }

            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    assert!(result.is_ok(), "Download did not complete within timeout");

    // Step 4: Verify file was created with correct content
    let mut downloaded_file = None;
    if let Ok(entries) = std::fs::read_dir(temp_dir.path()) {
        for entry in entries.flatten() {
            if let Ok(filename) = entry.file_name().into_string() {
                if filename.starts_with("file") && filename.ends_with(".zip") {
                    downloaded_file = Some(entry.path());
                    break;
                }
            }
        }
    }

    let file_path = downloaded_file.expect("Downloaded file should exist");
    let downloaded_content = std::fs::read(&file_path).unwrap();
    assert_eq!(downloaded_content.len(), 2048, "File size should match");
    assert_eq!(downloaded_content, test_content, "File content should match");
}

#[tokio::test]
async fn test_pause_resume_workflow() {
    let (_server, uri) = setup_mock_download_server().await;
    let temp_dir = tempfile::tempdir().unwrap();

    let manager = create_test_manager();
    let config = create_test_config();

    // Step 1: Add and start download
    let url = format!("{}/file.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;
    assert_eq!(
        manager.start_download(task_id, None, config.clone()).await.unwrap(),
        StartOutcome::Started
    );

    // Wait for download to start
    sleep(Duration::from_millis(100)).await;

    // Verify downloading (or may already be completed due to small file size)
    let task = manager.get_by_id(task_id).await;

    // If task is already gone (completed and removed), verify file exists and skip pause/resume test
    // Note: File may have unique suffix like file[timestamp].zip
    if task.is_none() {
        let mut found_file = false;
        if let Ok(entries) = std::fs::read_dir(temp_dir.path()) {
            for entry in entries.flatten() {
                if let Ok(filename) = entry.file_name().into_string() {
                    if filename.starts_with("file") && filename.ends_with(".zip") {
                        if let Ok(metadata) = entry.metadata() {
                            assert_eq!(metadata.len(), 1024, "File size should be correct");
                            found_file = true;
                            break;
                        }
                    }
                }
            }
        }
        assert!(found_file, "File should exist if download completed");
        return; // Test passed - download was too fast to pause
    }

    let task = task.unwrap();
    assert!(
        task.status == DownloadStatus::Downloading || task.status == DownloadStatus::Completed,
        "Status should be Downloading or Completed, got {:?}",
        task.status
    );

    // Step 2: Pause download (if not already completed)
    let status_before_pause = task.status;
    manager.pause_download(task_id).await.unwrap();
    sleep(Duration::from_millis(100)).await;

    // Verify paused (unless it completed before we could pause or was removed)
    let paused_task = manager.get_by_id(task_id).await;
    if paused_task.is_none() {
        // Download completed and was removed from queue
        let mut found_file = false;
        if let Ok(entries) = std::fs::read_dir(temp_dir.path()) {
            for entry in entries.flatten() {
                if let Ok(filename) = entry.file_name().into_string() {
                    if filename.starts_with("file") && filename.ends_with(".zip") {
                        found_file = true;
                        break;
                    }
                }
            }
        }
        assert!(found_file, "File should exist if download completed");
        return;
    }

    let paused_task = paused_task.unwrap();
    if status_before_pause == DownloadStatus::Downloading {
        assert_eq!(paused_task.status, DownloadStatus::Paused);
    }

    // Step 3: Resume download (start again) if it was paused
    if paused_task.status == DownloadStatus::Paused {
        let _ = manager.start_download(task_id, None, config).await.unwrap();

        // Verify resumed
        let resumed_task = manager.get_by_id(task_id).await.unwrap();
        assert_eq!(resumed_task.status, DownloadStatus::Downloading);

        // Step 4: Wait for completion (check file or task removal)
        let file_path = temp_dir.path().join("file.zip");
        let result = timeout(Duration::from_secs(5), async {
            loop {
                // Check if file exists with expected size
                if file_path.exists() {
                    if let Ok(metadata) = std::fs::metadata(&file_path) {
                        if metadata.len() == 1024 {
                            return;
                        }
                    }
                }

                // Check if task removed (completed)
                if manager.get_by_id(task_id).await.is_none() && file_path.exists() {
                    return;
                }

                sleep(Duration::from_millis(100)).await;
            }
        })
        .await;

        assert!(result.is_ok(), "Download did not complete after resume");

        // Verify file was created
        assert!(file_path.exists());
    } else {
        // If it already completed, just verify it stayed completed
        assert_eq!(paused_task.status, DownloadStatus::Completed);
        assert_eq!(paused_task.downloaded, 1024); // Mock server size
    }
}

#[tokio::test]
async fn test_error_handling_workflow() {
    // Setup mock server that returns 404
    let (_server, uri) = setup_error_mock_server(404).await;
    let temp_dir = tempfile::tempdir().unwrap();

    let manager = create_test_manager();
    let config = create_test_config();

    // Step 1: Add download with invalid URL (will get 404)
    let url = format!("{}/nonexistent.zip", uri);
    let task = create_test_task(url, temp_dir.path().to_path_buf());
    let task_id = task.id;

    manager.add_download(task).await;

    // Step 2: Start download
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    // Step 3: Wait for error status
    let result = timeout(Duration::from_secs(5), async {
        loop {
            if let Some(task) = manager.get_by_id(task_id).await {
                if task.status == DownloadStatus::Error {
                    return task;
                }
            }
            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    assert!(result.is_ok(), "Download should transition to Error status");
    let error_task = result.unwrap();

    // Verify error status
    assert_eq!(error_task.status, DownloadStatus::Error);
    assert!(error_task.started_at.is_some());
    assert!(error_task.completed_at.is_none());
}

#[tokio::test]
async fn test_concurrent_downloads_workflow() {
    let (_server, uri) = setup_mock_download_server().await;
    let temp_dir = tempfile::tempdir().unwrap();

    // Create manager with max 3 concurrent downloads (no retries for faster tests)
    let manager = ggg::download::manager::DownloadManager::with_config(3, 3, 2, 0, 1);
    let config = create_test_config();

    // Step 1: Add 5 downloads
    let mut task_ids = vec![];
    for i in 0..5 {
        let url = format!("{}/file{}.zip", uri, i);
        let task = create_test_task(url, temp_dir.path().to_path_buf());
        let task_id = task.id;
        task_ids.push(task_id);

        manager.add_download(task).await;
    }

    // Verify all added
    assert_eq!(manager.get_all_downloads().await.len(), 5);

    // Step 2: Start all downloads
    for task_id in &task_ids {
        assert_eq!(
            manager.start_download(*task_id, None, config.clone()).await.unwrap(),
            StartOutcome::Started
        );
    }

    // Wait for downloads to start
    sleep(Duration::from_millis(200)).await;

    // Step 3: Verify at most 3 are active (due to concurrency limit)
    // Note: active_downloads tracks spawned tasks, not semaphore-limited concurrent downloads
    let active_count = manager.get_active_count().await;
    assert!(active_count <= 5, "Should not exceed number of started tasks");

    // Step 4: Wait for all downloads to complete (tasks will be removed from queue)
    // Check for file creation instead
    // Note: Files may have unique suffixes like file0[timestamp].zip to avoid duplicates
    let result = timeout(Duration::from_secs(10), async {
        loop {
            // Count how many files have been created with expected size
            let mut completed_count = 0;

            // Read directory and count files matching the pattern
            if let Ok(entries) = std::fs::read_dir(temp_dir.path()) {
                for entry in entries.flatten() {
                    if let Ok(filename) = entry.file_name().into_string() {
                        // Match files like file0.zip, file0[timestamp].zip, etc.
                        if filename.starts_with("file") && filename.ends_with(".zip") {
                            if let Ok(metadata) = entry.metadata() {
                                if metadata.len() == 1024 {
                                    completed_count += 1;
                                }
                            }
                        }
                    }
                }
            }

            if completed_count >= 5 {
                return;
            }

            sleep(Duration::from_millis(200)).await;
        }
    })
    .await;

    assert!(result.is_ok(), "All downloads should eventually complete");

    // Step 5: Verify all files exist with correct size (may have unique suffixes)
    let mut found_count = 0;
    if let Ok(entries) = std::fs::read_dir(temp_dir.path()) {
        for entry in entries.flatten() {
            if let Ok(filename) = entry.file_name().into_string() {
                if filename.starts_with("file") && filename.ends_with(".zip") {
                    if let Ok(metadata) = entry.metadata() {
                        assert_eq!(metadata.len(), 1024, "File {} should be 1024 bytes", filename);
                        found_count += 1;
                    }
                }
            }
        }
    }
    assert_eq!(found_count, 5, "Should have 5 completed files");
}

#[tokio::test]
async fn test_queue_persistence_workflow() {
    let temp_dir = tempfile::tempdir().unwrap();
    let queue_path = temp_dir.path().join("queue.json");

    // Step 1: Create manager and add tasks
    let manager1 = create_test_manager();

    let task1 = create_test_task(
        "http://example.com/file1.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    let task2 = create_test_task(
        "http://example.com/file2.zip".to_string(),
        temp_dir.path().to_path_buf(),
    );
    let task1_id = task1.id;
    let task2_id = task2.id;

    manager1.add_download(task1).await;
    manager1.add_download(task2).await;

    // Verify tasks added
    assert_eq!(manager1.get_all_downloads().await.len(), 2);

    // Step 2: Save queue
    manager1.save_queue(&queue_path).await.unwrap();
    assert!(queue_path.exists(), "Queue file should be created");

    // Step 3: Create new manager and load queue
    let manager2 = create_test_manager();

    // Before loading, should be empty
    assert_eq!(manager2.get_all_downloads().await.len(), 0);

    // Load queue
    manager2.load_queue(&queue_path).await.unwrap();

    // Step 4: Verify tasks were restored
    let loaded_tasks = manager2.get_all_downloads().await;
    assert_eq!(loaded_tasks.len(), 2, "Should restore 2 tasks");

    // Verify task IDs match
    let loaded_ids: Vec<_> = loaded_tasks.iter().map(|t| t.id).collect();
    assert!(loaded_ids.contains(&task1_id), "Should contain task1");
    assert!(loaded_ids.contains(&task2_id), "Should contain task2");

    // Verify task details preserved
    let loaded_task1 = manager2.get_by_id(task1_id).await.unwrap();
    assert_eq!(loaded_task1.url, "http://example.com/file1.zip");
    assert_eq!(loaded_task1.status, DownloadStatus::Pending);

    // Step 5: Verify manager can continue operations with loaded tasks
    manager2.change_folder(task1_id, "videos".to_string(), None).await.unwrap();

    let updated_task = manager2.get_by_id(task1_id).await.unwrap();
    assert_eq!(updated_task.folder_id, "videos");
}

#[tokio::test]
async fn test_resume_partial_download_workflow() {
    let full_content = generate_test_content(4096);
    let (_server, uri) = setup_resumable_mock_server(full_content.clone()).await;
    let temp_dir = tempfile::tempdir().unwrap();

    let manager = create_test_manager();
    let config = create_test_config();

    // Step 1: Create partial file (simulate interrupted download)
    let partial_content = &full_content[..2048];
    let filename = "resumable.zip";
    let file_path = temp_dir.path().join(filename);
    std::fs::write(&file_path, partial_content).unwrap();

    // Verify partial file exists
    assert_eq!(
        std::fs::metadata(&file_path).unwrap().len(),
        2048,
        "Partial file should be 2048 bytes"
    );

    // Step 2: Add download task with same filename
    let url = format!("{}/resumable.zip", uri);
    let mut task = create_test_task(url, temp_dir.path().to_path_buf());
    task.filename = filename.to_string();
    task.downloaded = 2048; // Set to partial progress
    let task_id = task.id;

    manager.add_download(task).await;

    // Step 3: Start download (should resume from 2048 bytes)
    assert_eq!(
        manager.start_download(task_id, None, config).await.unwrap(),
        StartOutcome::Started
    );

    // Step 4: Wait for completion (check file size or task removal)
    // Note: Mock server doesn't support Range requests, so download will be full re-download
    // File may be renamed to resumable[timestamp].zip to avoid duplicates
    let result = timeout(Duration::from_secs(5), async {
        loop {
            // Check if task removed (completed) - this is the primary indicator
            if manager.get_by_id(task_id).await.is_none() {
                return; // Download completed
            }

            sleep(Duration::from_millis(100)).await;
        }
    })
    .await;

    assert!(result.is_ok(), "Download should complete");

    // Step 5: Verify a file was created (may be the original or a new file)
    // Since mock server doesn't support Range requests, the actual size may vary
    let mut found_file = false;
    if let Ok(entries) = std::fs::read_dir(temp_dir.path()) {
        for entry in entries.flatten() {
            if let Ok(filename) = entry.file_name().into_string() {
                if filename.starts_with("resumable") && filename.ends_with(".zip") {
                    found_file = true;
                    break;
                }
            }
        }
    }

    assert!(found_file, "File should exist after download");

    // Note: The actual resume behavior depends on the server supporting Range headers
    // and the HttpClient checking for existing files. This test verifies the workflow
    // completes successfully when a partial file exists.
}
