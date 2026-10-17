//! Start-up shared by every binary that hosts the download core.

use super::{config::Config, state::AppState};
use crate::download::manager::DownloadManager;
use anyhow::Result;
use std::path::PathBuf;
use tracing_subscriber::prelude::*;
use uuid::Uuid;

/// Everything a front end needs to drive the core.
pub struct Core {
    pub state: AppState,
    pub manager: DownloadManager,
    /// Tasks that were `Downloading` when the previous process stopped.
    pub recovered: Vec<Uuid>,
}

/// Installs the JSON file logger and a panic hook that logs panics.
///
/// The returned guard flushes buffered log lines when dropped, so the caller
/// must keep it alive (and drop it before `std::process::exit`).
pub fn init_logging(verbose: bool) -> tracing_appender::non_blocking::WorkerGuard {
    // Get logs directory (creates if needed)
    let logs_dir = crate::util::paths::get_logs_dir().unwrap_or_else(|_| PathBuf::from("."));
    std::fs::create_dir_all(&logs_dir).ok();

    // Set up daily rotating file appender (YYYYMMDD.jsonl format)
    let file_appender = tracing_appender::rolling::daily(&logs_dir, "app.jsonl");
    let (non_blocking, log_guard) = tracing_appender::non_blocking(file_appender);

    // Set log level based on verbose flag
    let log_level = if verbose {
        tracing::Level::TRACE
    } else {
        tracing::Level::INFO
    };

    // Initialize logging with JSON format for structured logs
    tracing_subscriber::registry()
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .with_writer(non_blocking)
                .with_ansi(false)
                .with_filter(tracing_subscriber::filter::LevelFilter::from_level(
                    log_level,
                )),
        )
        .init();

    // Panics otherwise only reach stderr, which the TUI's alternate screen hides.
    let default_panic_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        let thread = std::thread::current();
        tracing::error!(
            thread = thread.name().unwrap_or("<unnamed>"),
            backtrace = %std::backtrace::Backtrace::force_capture(),
            "Panic: {}",
            info
        );
        default_panic_hook(info);
    }));

    log_guard
}

/// Loads the configuration and queue files and builds the application state
/// and download manager.
pub async fn init_core() -> Result<Core> {
    // Load configuration
    tracing::trace!("Loading configuration from file...");
    let config = Config::load().unwrap_or_default();
    // `{:?}` on `Config` would include every folder's `default_headers`,
    // which can carry secrets (a `Cookie`/`Authorization` value); log a
    // summary instead.
    tracing::info!(
        "Config loaded: {} folder(s), max_concurrent={}, retry_count={}",
        config.folders.len(),
        config.download.max_concurrent,
        config.download.retry_count,
    );
    tracing::trace!("Configuration details: max_concurrent={}, retry_count={}",
        config.download.max_concurrent,
        config.download.retry_count);

    // Initialize application state with scripts
    let state = AppState::new_with_scripts(config.clone()).await?;

    // Initialize download manager with folder slot configuration
    let max_concurrent = config.download.max_concurrent;
    let max_concurrent_per_folder = config.download.max_concurrent_per_folder.unwrap_or(max_concurrent);
    let parallel_folder_count = config.download.parallel_folder_count.unwrap_or(1);

    let mut manager = DownloadManager::with_config(
        max_concurrent,
        max_concurrent_per_folder,
        parallel_folder_count,
        config.download.retry_count,
        config.download.retry_delay,
    );
    manager.set_claim_wait_budget(std::time::Duration::from_millis(config.download.claim_wait_ms));

    // Load queue from folder-based files
    if let Err(e) = manager.load_queue_from_folders().await {
        tracing::warn!("Failed to load queue from folder files: {}", e);
    } else {
        tracing::info!("Queue loaded from folder files");
    }

    // Warn about legacy queue.json
    if PathBuf::from("queue.json").exists() {
        tracing::warn!("Legacy queue.json detected. New queues are stored in config/{{folder_id}}/queue.toml");
    }

    // No download handle can exist yet, so a persisted `Downloading` task can
    // only mean the previous process was interrupted. Recover it in every
    // mode, before any front end runs, so the TUI, GUI and standalone CLI
    // see it as startable too.
    let recovered = manager.recover_interrupted().await;

    Ok(Core {
        state,
        manager,
        recovered,
    })
}
