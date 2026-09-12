use anyhow::Result;
use clap::Parser;
use ggg::{
    app::{config::Config, state::AppState},
    cli::{self, Cli},
    download::manager::DownloadManager,
    tui::run_tui,
};
use std::path::PathBuf;
use tracing_subscriber::prelude::*;

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments first to get verbose flag
    let cli = Cli::parse();

    // Get logs directory (creates if needed)
    let logs_dir = ggg::util::paths::get_logs_dir().unwrap_or_else(|_| PathBuf::from("."));
    std::fs::create_dir_all(&logs_dir).ok();

    // Set up daily rotating file appender (YYYYMMDD.jsonl format)
    let file_appender = tracing_appender::rolling::daily(&logs_dir, "app.jsonl");
    let (non_blocking, log_guard) = tracing_appender::non_blocking(file_appender);

    // Set log level based on verbose flag
    let log_level = if cli.verbose {
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

    tracing::info!("Starting GGG...");
    if cli.verbose {
        tracing::info!("Verbose logging enabled (TRACE level)");
    }
    tracing::trace!("CLI arguments: {:?}", cli);

    // Set config directory override if --config flag was used
    if let Some(ref config_dir) = cli.config {
        tracing::info!("Using config directory override: {:?}", config_dir);
        ggg::util::paths::set_config_dir_override(Some(config_dir.clone()));
    }

    // Load configuration
    tracing::trace!("Loading configuration from file...");
    let config = Config::load().unwrap_or_default();
    tracing::info!("Config loaded: {:?}", config);
    tracing::trace!("Configuration details: max_concurrent={}, retry_count={}",
        config.download.max_concurrent,
        config.download.retry_count);

    // Initialize application state with scripts
    let language = config.general.language.clone();
    let state = AppState::new_with_scripts(config.clone(), &language).await?;

    // Initialize download manager with folder slot configuration
    let max_concurrent = config.download.max_concurrent;
    let max_concurrent_per_folder = config.download.max_concurrent_per_folder.unwrap_or(max_concurrent);
    let parallel_folder_count = config.download.parallel_folder_count.unwrap_or(1);

    let download_manager = DownloadManager::with_config(
        max_concurrent,
        max_concurrent_per_folder,
        parallel_folder_count,
        config.download.retry_count,
        config.download.retry_delay,
    );

    // Load queue from folder-based files
    if let Err(e) = download_manager.load_queue_from_folders().await {
        tracing::warn!("Failed to load queue from folder files: {}", e);
    } else {
        tracing::info!("Queue loaded from folder files");
    }

    // Warn about legacy queue.json
    if PathBuf::from("queue.json").exists() {
        tracing::warn!("Legacy queue.json detected. New queues are stored in config/{{folder_id}}/queue.toml");
    }

    // Route based on CLI arguments
    match cli.command {
        Some(command) => {
            // CLI mode - handle command and exit
            let exit_code = cli::handler::handle_command(
                command,
                state,
                download_manager,
            ).await;

            // process::exit skips destructors; flush buffered log lines first.
            drop(log_guard);
            std::process::exit(exit_code);
        }
        None => {
            if cli.headless {
                // Headless daemon mode
                cli::daemon::run_daemon(download_manager).await?;
            } else {
                // TUI mode (default)
                run_tui(state, download_manager).await?;
            }
        }
    }

    Ok(())
}
