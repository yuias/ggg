use super::error;
use super::output;
use super::remote;
use super::daemon_ops;
use super::{BridgeAction, Commands, ConfigAction, DebugAction, ScriptAction, FolderAction, ExportAction, ImportAction, TestAction};
use super::bridge;
use crate::app::config::{Config, FolderConfig};
use crate::app::state::AppState;
use crate::download::manager::{DownloadManager, EditHeadersError, RunState, StartOutcome};
use crate::download::task::{DownloadTask, DownloadStatus};
use crate::download::completion_log::CompletedEntry;
use crate::download::enqueue::{
    build_task, enqueue, resolve_folder_id, resolve_target_folder, EnqueueOptions, EnqueueOutcome,
};
use crate::ipc::control::protocol::{AddedTask, SkippedUrl, WaitOutcome};
use crate::script::events::{BeforeRequestContext, HookEvent};
use anyhow::Result;
use std::path::PathBuf;
use std::collections::HashMap;
use uuid::Uuid;

/// Handle a CLI command and return exit code
pub async fn handle_command(
    command: Commands,
    state: AppState,
    manager: DownloadManager,
) -> i32 {
    let result = match command {
        Commands::Add { url, folder, referrer, header, immediate, quiet, json } => {
            handle_add(url, folder, referrer, header, immediate, quiet, json, &state, &manager).await
        }
        Commands::List { json, status, folder } => handle_list(&state, &manager, json, status, folder).await,
        Commands::Start { id, wait } => handle_start(id, &state, &manager, wait).await,
        Commands::Pause { id } => handle_pause(id, &manager).await,
        Commands::Remove { id } => handle_remove(id, &manager).await,
        Commands::Status { id, json, show_secrets } => handle_status(id, &manager, json, show_secrets).await,
        Commands::Config { action } => handle_config(action, &state).await,
        Commands::Logs { follow, level, lines } => handle_logs(follow, level, lines).await,
        Commands::History { today, folder, json } => handle_history(today, folder, json).await,
        Commands::Stats { folder, json } => handle_stats(&manager, folder, json).await,
        Commands::Debug { action } => handle_debug(action, &state, &manager).await,
        Commands::Script { action } => handle_script(action, &state).await,
        Commands::Folder { action } => handle_folder(action, &state).await,
        Commands::StartAll { folder } => handle_start_all(&state, &manager, folder).await,
        Commands::PauseAll { folder } => handle_pause_all(&state, &manager, folder).await,
        Commands::Clear { status, folder } => handle_clear(&manager, status, folder).await,
        Commands::BatchAdd { file, folder } => handle_batch_add(&state, &manager, file, folder).await,
        Commands::Priority { id, set } => handle_priority(&manager, id, set).await,
        Commands::Modify { id, referrer, clear_referrer, header, remove_header } => {
            handle_modify(&manager, id, referrer, clear_referrer, header, remove_header).await
        }
        Commands::Move { id, to_top, to_bottom, before, folder } => {
            handle_move(&manager, &state, id, to_top, to_bottom, before, folder).await
        }
        Commands::Export { action } => handle_export(action, &state, &manager).await,
        Commands::Import { action } => handle_import(action, &state, &manager).await,
        Commands::Test { action } => handle_test(action, &state, &manager).await,
        Commands::Bridge { action } => match action {
            BridgeAction::Install { extension_ids, bridge_path } => {
                bridge::handle_install(extension_ids, bridge_path).await
            }
            BridgeAction::Uninstall => bridge::handle_uninstall().await,
            BridgeAction::Status => bridge::handle_status().await,
        },
        // `route` handles every `Daemon`/`Wait` request when a daemon is
        // reachable; these arms are only reached by the probe-then-write
        // race `route`'s doc comment accepts, or when no daemon is running
        // at all -- `wait` has no standalone meaning since nothing else
        // would be running the download to wait on.
        Commands::Daemon { .. } => handle_daemon_standalone(),
        Commands::Wait { .. } => handle_daemon_standalone(),
    };

    match result {
        Ok(code) => code,
        Err(e) => {
            eprintln!("Error: {}", e);
            error::ERROR
        }
    }
}

/// Add a new download. Shares `resolve_target_folder`/`build_task`/`enqueue`
/// with the daemon's `Add` handler, so folder `save_path` and
/// `default_headers` apply here too, not just when routed through a daemon.
async fn handle_add(
    url: String,
    folder: Option<String>,
    referrer: Option<super::RedactedArg>,
    header: Vec<super::HeaderArg>,
    immediate: bool,
    quiet: bool,
    json: bool,
    state: &AppState,
    manager: &DownloadManager,
) -> Result<i32> {
    // The standalone process exits right after this command, so a download
    // started here would be killed immediately; only a daemon can honor
    // `--immediate`.
    if immediate {
        eprintln!("--immediate requires a running daemon");
        return Ok(error::INVALID_INPUT);
    }

    let mut headers = Vec::with_capacity(header.len());
    for h in &header {
        match super::remote::parse_header_arg(h) {
            Ok(pair) => headers.push(pair),
            Err(e) => {
                eprintln!("Error: {e}");
                return Ok(error::INVALID_INPUT);
            }
        }
    }

    let config = state.config.read().await.clone();
    let folder_id = match resolve_target_folder(&config, folder.as_deref()) {
        Ok(id) => id,
        Err(_) => {
            eprintln!("Error: unknown folder");
            return Ok(error::INVALID_INPUT);
        }
    };

    let opts = EnqueueOptions {
        referrer: referrer.map(|r| r.0),
        headers,
    };

    // Expand range patterns like `[1-10]` into individual URLs so the CLI
    // matches the TUI's add behavior.
    let mut added = Vec::new();
    let mut skipped = Vec::new();
    for u in crate::util::url_expansion::expand_url(&url) {
        // Reject empty / non-http(s)/ftp URLs up front instead of queueing
        // garbage that only fails later.
        if !is_valid_download_url(&u) {
            skipped.push(SkippedUrl {
                url: u,
                reason: "invalid_url".to_string(),
            });
            continue;
        }

        let task = build_task(&u, &folder_id, &config, &opts);
        match enqueue(manager, state, task, false, false).await {
            EnqueueOutcome::Added {
                id,
                started,
                start_error,
            } => added.push(AddedTask {
                id,
                url: u,
                folder_id: folder_id.clone(),
                started,
                start_error,
            }),
            EnqueueOutcome::Duplicate => {
                skipped.push(SkippedUrl {
                    url: u,
                    reason: "duplicate".to_string(),
                })
            }
        }
    }

    if !added.is_empty() {
        manager.save_queue_to_folders().await?;
    }

    Ok(super::remote::render_added(&added, &skipped, quiet, json))
}

/// Standalone fallback for `daemon ping|status|stop`: these commands are
/// only meaningful against a running daemon, so there is nothing to do here
/// beyond reporting that none is reachable.
fn handle_daemon_standalone() -> Result<i32> {
    eprintln!("no ggg daemon running");
    Ok(error::DAEMON_UNREACHABLE)
}

/// Parse a task ID string, returning a uniform error on malformed input.
pub(crate) fn parse_task_id(id_str: &str) -> Result<Uuid> {
    Uuid::parse_str(id_str).map_err(|_| anyhow::anyhow!("Invalid UUID format"))
}

/// Validate a download URL: must parse and use a scheme the HTTP client
/// supports. Mirrors the TUI's `is_valid_download_url`.
pub(crate) fn is_valid_download_url(text: &str) -> bool {
    match url::Url::parse(text) {
        Ok(parsed) => matches!(parsed.scheme(), "http" | "https" | "ftp" | "ftps"),
        Err(_) => false,
    }
}

/// List all downloads, applying the same status/folder filters and header
/// redaction whether or not a daemon is routing this request (see
/// `remote::render_tasks`), so `list --json` looks the same either way.
async fn handle_list(
    state: &AppState,
    manager: &DownloadManager,
    json: bool,
    status: Option<String>,
    folder: Option<String>,
) -> Result<i32> {
    // Matches the routed path's `InvalidInput` -> exit 3 for the same two
    // failures (`remote::send_and_render`'s `List` arm), instead of `?`
    // propagating them to the generic exit 1 every other `Err` gets.
    let statuses = match super::remote::parse_status_arg(status.as_deref()) {
        Ok(s) => s,
        Err(e) => {
            eprintln!("Error: {e}");
            return Ok(error::INVALID_INPUT);
        }
    };
    let resolved_folder = match &folder {
        Some(f) => {
            let config = state.config.read().await.clone();
            match resolve_folder_id(&config, f) {
                Some(id) => Some(id),
                None => {
                    eprintln!("Error: unknown folder");
                    return Ok(error::INVALID_INPUT);
                }
            }
        }
        None => None,
    };

    let tasks = manager.get_all_downloads().await;
    let tasks = super::daemon_ops::filter_tasks(tasks, &statuses, resolved_folder.as_deref());
    super::remote::render_tasks(tasks, json);

    Ok(error::SUCCESS)
}

/// Start a download
async fn handle_start(
    id_str: String,
    state: &AppState,
    manager: &DownloadManager,
    wait: bool,
) -> Result<i32> {
    let id = parse_task_id(&id_str)?;

    // Check if download exists
    let task = manager.get_by_id(id).await
        .ok_or_else(|| anyhow::anyhow!("Download not found"))?;

    // Start download with script support
    let outcome = manager.start_download(id, state.script_sender.clone(), state.config.clone()).await?;
    manager.save_queue_to_folders().await?;

    match outcome {
        StartOutcome::Started => println!("Started download: {}", task.filename),
        StartOutcome::AlreadyRunning => println!("Download already running: {}", task.filename),
        StartOutcome::Busy => println!("Download is busy (being started or modified): {}", task.filename),
    }

    if wait {
        let (outcome, task) =
            wait_until_final(id, manager, tokio::time::Duration::from_millis(500)).await;
        // The failure path (max retries exceeded) never saves the terminal
        // `Error` status on its own; every other terminal path already went
        // through a queue write, so this is a harmless extra save for them.
        // A failed save must not mask the outcome's exit code.
        if let Err(e) = manager.save_queue_to_folders().await {
            tracing::error!("Failed to save queue after wait: {e}");
        }
        let scripts_timeout = state.config.read().await.scripts.timeout;
        state
            .flush_scripts(
                tokio::time::Duration::from_secs(scripts_timeout)
                    .saturating_add(tokio::time::Duration::from_secs(5)),
            )
            .await;
        remote::render_waited(outcome, task, false);
        return Ok(remote::exit_code_for_wait(outcome));
    }

    Ok(error::SUCCESS)
}

/// Polls until `id` reaches a state `daemon_ops::wait_state` treats as
/// final -- the same rule the daemon's own `Wait` uses -- printing progress
/// and one notice per backoff episode along the way. A task in retry
/// backoff (live handle, persisted status `Paused`) is not terminal, so
/// this keeps waiting through it instead of reporting a stale pause.
async fn wait_until_final(
    id: Uuid,
    manager: &DownloadManager,
    poll: tokio::time::Duration,
) -> (WaitOutcome, Option<DownloadTask>) {
    use std::io::{self, Write};

    println!("Monitoring download progress...");
    let mut was_backoff = false;

    loop {
        if let Some(task) = manager.get_by_id(id).await {
            if let Some(total) = task.size {
                let progress = (task.downloaded as f64 / total as f64 * 100.0) as u8;
                let downloaded_str = output::format_bytes(task.downloaded);
                let total_str = output::format_bytes(total);
                print!("\r[{:3}%] {} / {}   ", progress, downloaded_str, total_str);
            } else {
                let downloaded_str = output::format_bytes(task.downloaded);
                print!("\rDownloaded: {}   ", downloaded_str);
            }
            let _ = io::stdout().flush();

            let in_backoff = manager.run_state(id).await == RunState::RetryBackoff;
            if in_backoff && !was_backoff {
                println!("\nRetrying (attempt {})...", task.retry_count + 1);
            }
            was_backoff = in_backoff;
        }

        if let Some((outcome, task)) = daemon_ops::wait_state(id, manager).await {
            println!();
            return (outcome, task);
        }

        tokio::time::sleep(poll).await;
    }
}

/// Pause a download
async fn handle_pause(
    id_str: String,
    manager: &DownloadManager,
) -> Result<i32> {
    let id = parse_task_id(&id_str)?;

    // Check if download exists
    let task = manager.get_by_id(id).await
        .ok_or_else(|| anyhow::anyhow!("Download not found"))?;

    manager.pause_download(id).await?;
    manager.save_queue_to_folders().await?;

    println!("Paused download: {}", task.filename);

    Ok(error::SUCCESS)
}

/// Remove a download
async fn handle_remove(
    id_str: String,
    manager: &DownloadManager,
) -> Result<i32> {
    let id = parse_task_id(&id_str)?;

    let task = match manager.remove_download(id).await {
        Ok(Some(task)) => task,
        Ok(None) => return Err(anyhow::anyhow!("Download not found")),
        Err(e) => {
            println!("{e}");
            return Ok(error::INVALID_INPUT);
        }
    };

    manager.save_queue_to_folders().await?;

    println!("Removed download: {}", task.filename);

    Ok(error::SUCCESS)
}

/// Show download status
async fn handle_status(id_str: String, manager: &DownloadManager, json: bool, show_secrets: bool) -> Result<i32> {
    let id = parse_task_id(&id_str)?;

    let task = manager.get_by_id(id).await
        .ok_or_else(|| anyhow::anyhow!("Download not found"))?;

    super::remote::render_task(task, false, json, show_secrets);

    Ok(error::SUCCESS)
}

/// Handle configuration commands
async fn handle_config(action: ConfigAction, state: &AppState) -> Result<i32> {
    match action {
        ConfigAction::Get { key } => {
            let config = state.config.read().await;
            let value = get_config_value(&config, &key)?;
            println!("{}", value);
            Ok(error::SUCCESS)
        }
        ConfigAction::Set { key, value } => {
            let mut config = state.config.write().await;
            set_config_value(&mut config, &key, &value)?;
            config.save()?;
            println!("Configuration updated: {} = {}", key, value);
            Ok(error::SUCCESS)
        }
        ConfigAction::Show { json } => {
            let config = state.config.read().await;
            if json {
                println!("{}", serde_json::to_string_pretty(&*config)?);
            } else {
                println!("{}", toml::to_string_pretty(&*config)?);
            }
            Ok(error::SUCCESS)
        }
    }
}

/// Get configuration value by dot notation key
pub(crate) fn get_config_value(config: &Config, key: &str) -> Result<String> {
    let parts: Vec<&str> = key.split('.').collect();

    match parts.as_slice() {
        ["general", "language"] => Ok(config.general.language.clone()),
        ["general", "theme"] => Ok(config.general.theme.clone()),
        ["general", "minimize_to_tray"] => Ok(config.general.minimize_to_tray.to_string()),
        ["general", "start_minimized"] => Ok(config.general.start_minimized.to_string()),
        ["download", "default_directory"] => Ok(config.download.default_directory.display().to_string()),
        ["download", "max_concurrent"] => Ok(config.download.max_concurrent.to_string()),
        ["download", "retry_count"] => Ok(config.download.retry_count.to_string()),
        ["download", "retry_delay"] => Ok(config.download.retry_delay.to_string()),
        ["download", "user_agent"] => Ok(config.download.user_agent.clone()),
        ["download", "bandwidth_limit"] => Ok(config.download.bandwidth_limit.to_string()),
        ["network", "proxy_enabled"] => Ok(config.network.proxy_enabled.to_string()),
        ["network", "proxy_type"] => Ok(config.network.proxy_type.clone()),
        ["network", "proxy_host"] => Ok(config.network.proxy_host.clone()),
        ["network", "proxy_port"] => Ok(config.network.proxy_port.to_string()),
        ["scripts", "enabled"] => Ok(config.scripts.enabled.to_string()),
        ["scripts", "directory"] => Ok(config.scripts.directory.display().to_string()),
        ["scripts", "timeout"] => Ok(config.scripts.timeout.to_string()),
        _ => Err(anyhow::anyhow!("Unknown configuration key: {}", key)),
    }
}

/// Set configuration value by dot notation key
pub(crate) fn set_config_value(config: &mut Config, key: &str, value: &str) -> Result<()> {
    let parts: Vec<&str> = key.split('.').collect();

    match parts.as_slice() {
        ["general", "language"] => config.general.language = value.to_string(),
        ["general", "theme"] => config.general.theme = value.to_string(),
        ["general", "minimize_to_tray"] => config.general.minimize_to_tray = value.parse()?,
        ["general", "start_minimized"] => config.general.start_minimized = value.parse()?,
        ["download", "default_directory"] => config.download.default_directory = PathBuf::from(value),
        ["download", "max_concurrent"] => config.download.max_concurrent = value.parse()?,
        ["download", "retry_count"] => config.download.retry_count = value.parse()?,
        ["download", "retry_delay"] => config.download.retry_delay = value.parse()?,
        ["download", "user_agent"] => config.download.user_agent = value.to_string(),
        ["download", "bandwidth_limit"] => config.download.bandwidth_limit = value.parse()?,
        ["network", "proxy_enabled"] => config.network.proxy_enabled = value.parse()?,
        ["network", "proxy_type"] => config.network.proxy_type = value.to_string(),
        ["network", "proxy_host"] => config.network.proxy_host = value.to_string(),
        ["network", "proxy_port"] => config.network.proxy_port = value.parse()?,
        ["scripts", "enabled"] => config.scripts.enabled = value.parse()?,
        ["scripts", "directory"] => config.scripts.directory = PathBuf::from(value),
        ["scripts", "timeout"] => config.scripts.timeout = value.parse()?,
        _ => return Err(anyhow::anyhow!("Unknown configuration key: {}", key)),
    }

    Ok(())
}

/// Resolve the most recent application log file. The daily rolling appender
/// writes `app.jsonl.YYYY-MM-DD` into the logs directory; the date suffix sorts
/// chronologically, so the lexicographically greatest name is the newest.
fn current_log_file() -> Result<Option<PathBuf>> {
    let logs_dir = crate::util::paths::get_logs_dir()?;
    if !logs_dir.is_dir() {
        return Ok(None);
    }
    let mut candidates: Vec<PathBuf> = std::fs::read_dir(&logs_dir)?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with("app.jsonl"))
                .unwrap_or(false)
        })
        .collect();
    candidates.sort();
    Ok(candidates.pop())
}

/// Display application logs
async fn handle_logs(
    follow: bool,
    level: Option<String>,
    lines: Option<usize>,
) -> Result<i32> {
    let log_file = match current_log_file()? {
        Some(f) => f,
        None => {
            let dir = crate::util::paths::get_logs_dir()?;
            return Err(anyhow::anyhow!("Log file not found in {}", dir.display()));
        }
    };

    let lines_to_show = lines.unwrap_or(50);

    if follow {
        // Follow mode - tail -f
        println!("Following log file (Ctrl+C to stop)...");
        follow_log_file(&log_file, level).await?;
    } else {
        // Show last N lines
        let content = std::fs::read_to_string(&log_file)?;
        let mut log_lines: Vec<&str> = content.lines().collect();

        // Filter by log level if specified
        if let Some(ref level_filter) = level {
            let level_upper = level_filter.to_uppercase();
            log_lines.retain(|line| line.contains(&level_upper));
        }

        // Take last N lines
        let start_idx = log_lines.len().saturating_sub(lines_to_show);
        for line in &log_lines[start_idx..] {
            println!("{}", line);
        }

        println!("\n({} lines shown)", log_lines.len() - start_idx);
    }

    Ok(error::SUCCESS)
}

/// Follow log file (tail -f mode)
async fn follow_log_file(log_file: &PathBuf, level: Option<String>) -> Result<()> {
    use std::io::{BufRead, BufReader, Seek, SeekFrom};
    use std::fs::File;

    let mut current_path = log_file.clone();
    let mut file = File::open(&current_path)?;
    file.seek(SeekFrom::End(0))?;

    let mut reader = BufReader::new(file);
    let mut line = String::new();

    loop {
        match reader.read_line(&mut line) {
            Ok(bytes_read) => {
                if bytes_read > 0 {
                    // Filter by log level if specified
                    if let Some(ref level_filter) = level {
                        let level_upper = level_filter.to_uppercase();
                        if line.contains(&level_upper) {
                            print!("{}", line);
                        }
                    } else {
                        print!("{}", line);
                    }
                    line.clear();
                } else {
                    // No new data. Detect daily rotation: if a newer log file
                    // exists, switch to it and tail from its start so lines
                    // written to the new file are not missed.
                    if let Ok(Some(latest)) = current_log_file() {
                        if latest != current_path {
                            current_path = latest;
                            reader = BufReader::new(File::open(&current_path)?);
                            continue;
                        }
                    }
                    tokio::time::sleep(tokio::time::Duration::from_millis(100)).await;
                }
            }
            Err(e) => return Err(e.into()),
        }
    }
}

/// Show download completion history
async fn handle_history(
    today: bool,
    folder: Option<String>,
    json: bool,
) -> Result<i32> {
    let logs_dir = crate::util::paths::get_logs_dir()?;

    if !logs_dir.exists() {
        println!("No completion history found");
        return Ok(error::SUCCESS);
    }

    // Collect all log files
    let mut log_files = Vec::new();
    if today {
        // Only today's log (UTC, matching how completion logs are named)
        let today_str = chrono::Utc::now().format("%Y%m%d").to_string();
        let today_file = logs_dir.join(format!("{}.jsonl", today_str));
        if today_file.exists() {
            log_files.push(today_file);
        }
    } else {
        // All log files
        for entry in std::fs::read_dir(&logs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                log_files.push(path);
            }
        }
        log_files.sort();
    }

    if log_files.is_empty() {
        println!("No completion history found");
        return Ok(error::SUCCESS);
    }

    // Read and parse all entries
    let mut entries = Vec::new();
    for log_file in log_files {
        let content = std::fs::read_to_string(&log_file)?;
        for line in content.lines() {
            if line.trim().is_empty() {
                continue;
            }
            match serde_json::from_str::<CompletedEntry>(line) {
                Ok(entry) => {
                    // Filter by folder if specified
                    if let Some(ref folder_filter) = folder {
                        if entry.folder_id == *folder_filter {
                            entries.push(entry);
                        }
                    } else {
                        entries.push(entry);
                    }
                }
                Err(e) => {
                    tracing::warn!("Failed to parse completion entry: {}", e);
                }
            }
        }
    }

    if entries.is_empty() {
        println!("No completion history found");
        return Ok(error::SUCCESS);
    }

    // Output results
    if json {
        println!("{}", serde_json::to_string_pretty(&entries)?);
    } else {
        println!("Completion History ({} entries)\n", entries.len());
        for entry in entries {
            let status_symbol = if entry.status == "completed" { "✓" } else { "✗" };
            let duration = entry.duration_secs
                .map(|d| format!("{:.1}s", d))
                .unwrap_or_else(|| "N/A".to_string());

            println!("{} {} [{}] {}",
                status_symbol,
                entry.filename,
                entry.folder_id,
                duration
            );

            if let Some(ref err) = entry.error_message {
                println!("  Error: {}", err);
            }
        }
    }

    Ok(error::SUCCESS)
}

/// Show download statistics
async fn handle_stats(
    manager: &DownloadManager,
    folder: Option<String>,
    json: bool,
) -> Result<i32> {
    let tasks = manager.get_all_downloads().await;
    let logs_dir = crate::util::paths::get_logs_dir()?;

    // Calculate queue statistics
    let mut queue_stats = std::collections::HashMap::new();
    queue_stats.insert("total", tasks.len());
    queue_stats.insert("pending", tasks.iter().filter(|t| matches!(t.status, DownloadStatus::Pending)).count());
    queue_stats.insert("downloading", tasks.iter().filter(|t| matches!(t.status, DownloadStatus::Downloading)).count());
    queue_stats.insert("paused", tasks.iter().filter(|t| matches!(t.status, DownloadStatus::Paused)).count());
    queue_stats.insert("completed", tasks.iter().filter(|t| matches!(t.status, DownloadStatus::Completed)).count());
    queue_stats.insert("error", tasks.iter().filter(|t| matches!(t.status, DownloadStatus::Error)).count());

    // Calculate total bytes (queue only)
    let total_bytes: u64 = tasks.iter().filter_map(|t| t.size).sum();
    let downloaded_bytes: u64 = tasks.iter().map(|t| t.downloaded).sum();

    // Read completion history for all-time stats
    let mut completed_count = 0;
    let mut error_count = 0;
    let mut total_duration_secs = 0.0;

    if logs_dir.exists() {
        for entry in std::fs::read_dir(&logs_dir)? {
            let entry = entry?;
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("jsonl") {
                let content = std::fs::read_to_string(&path)?;
                for line in content.lines() {
                    if line.trim().is_empty() {
                        continue;
                    }
                    if let Ok(entry) = serde_json::from_str::<CompletedEntry>(line) {
                        // Filter by folder if specified
                        if let Some(ref folder_filter) = folder {
                            if entry.folder_id != *folder_filter {
                                continue;
                            }
                        }

                        if entry.status == "completed" {
                            completed_count += 1;
                            if let Some(duration) = entry.duration_secs {
                                total_duration_secs += duration;
                            }
                        } else {
                            error_count += 1;
                        }
                    }
                }
            }
        }
    }

    if json {
        let stats = serde_json::json!({
            "queue": queue_stats,
            "bytes": {
                "total": total_bytes,
                "downloaded": downloaded_bytes,
            },
            "history": {
                "completed": completed_count,
                "errors": error_count,
                "avg_duration_secs": if completed_count > 0 {
                    total_duration_secs / completed_count as f64
                } else {
                    0.0
                },
            },
        });
        println!("{}", serde_json::to_string_pretty(&stats)?);
    } else {
        println!("Download Statistics\n");
        println!("Queue:");
        println!("  Total: {}", queue_stats["total"]);
        println!("  Pending: {}", queue_stats["pending"]);
        println!("  Downloading: {}", queue_stats["downloading"]);
        println!("  Paused: {}", queue_stats["paused"]);
        println!("  Completed: {}", queue_stats["completed"]);
        println!("  Error: {}", queue_stats["error"]);
        println!("\nBytes:");
        println!("  Total: {}", output::format_bytes(total_bytes));
        println!("  Downloaded: {}", output::format_bytes(downloaded_bytes));
        println!("\nHistory (all-time):");
        println!("  Completed: {}", completed_count);
        println!("  Errors: {}", error_count);
        if completed_count > 0 {
            let avg_duration = total_duration_secs / completed_count as f64;
            println!("  Avg Duration: {:.1}s", avg_duration);
        }
    }

    Ok(error::SUCCESS)
}

/// Handle debug commands
async fn handle_debug(
    action: DebugAction,
    state: &AppState,
    manager: &DownloadManager,
) -> Result<i32> {
    match action {
        DebugAction::ManagerState { json } => handle_debug_manager_state(manager, json).await,
        DebugAction::FolderSlots { json } => handle_debug_folder_slots(manager, json).await,
        DebugAction::Task { id, json } => handle_debug_task(id, manager, json).await,
        DebugAction::ValidateConfig => handle_debug_validate_config(state).await,
        DebugAction::CheckQueue { json } => handle_debug_check_queue(manager, json).await,
    }
}

/// Show download manager internal state
async fn handle_debug_manager_state(manager: &DownloadManager, json: bool) -> Result<i32> {
    let tasks = manager.get_all_downloads().await;
    let active_count = manager.get_active_count().await;

    if json {
        let state = serde_json::json!({
            "total_tasks": tasks.len(),
            "active_downloads": active_count,
            "task_ids": tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
        });
        println!("{}", serde_json::to_string_pretty(&state)?);
    } else {
        println!("Download Manager State\n");
        println!("Total Tasks: {}", tasks.len());
        println!("Active Downloads: {}", active_count);
        println!("\nTask IDs:");
        for task in tasks {
            println!("  {} - {} ({:?})", task.id, task.filename, task.status);
        }
    }

    Ok(error::SUCCESS)
}

/// Show active folder and slot states
async fn handle_debug_folder_slots(manager: &DownloadManager, json: bool) -> Result<i32> {
    let tasks = manager.get_all_downloads().await;

    // Group tasks by folder
    let mut folder_tasks: std::collections::HashMap<String, Vec<&DownloadTask>> = std::collections::HashMap::new();
    for task in &tasks {
        folder_tasks.entry(task.folder_id.clone())
            .or_insert_with(Vec::new)
            .push(task);
    }

    if json {
        let mut folder_info = serde_json::Map::new();
        for (folder_id, tasks) in folder_tasks {
            let active = tasks.iter().filter(|t| matches!(t.status, DownloadStatus::Downloading)).count();
            folder_info.insert(folder_id, serde_json::json!({
                "total": tasks.len(),
                "active": active,
                "task_ids": tasks.iter().map(|t| t.id).collect::<Vec<_>>(),
            }));
        }
        println!("{}", serde_json::to_string_pretty(&folder_info)?);
    } else {
        println!("Folder Slot States\n");
        for (folder_id, tasks) in folder_tasks {
            let active = tasks.iter().filter(|t| matches!(t.status, DownloadStatus::Downloading)).count();
            println!("Folder: {} (Total: {}, Active: {})", folder_id, tasks.len(), active);
            for task in tasks {
                println!("  {} - {} ({:?})", task.id, task.filename, task.status);
            }
            println!();
        }
    }

    Ok(error::SUCCESS)
}

/// Show detailed task information
async fn handle_debug_task(id_str: String, manager: &DownloadManager, json: bool) -> Result<i32> {
    let id = parse_task_id(&id_str)?;

    let task = manager.get_by_id(id).await
        .ok_or_else(|| anyhow::anyhow!("Task not found"))?;

    if json {
        let mut task = task.clone();
        for (name, value) in task.headers.iter_mut() {
            if crate::util::redact::is_sensitive_header(name) {
                *value = crate::util::redact::REDACTED.to_string();
            }
        }
        println!("{}", serde_json::to_string_pretty(&task)?);
    } else {
        println!("Task Details\n");
        println!("ID: {}", task.id);
        println!("URL: {}", task.url);
        println!("Filename: {}", task.filename);
        println!("Folder: {}", task.folder_id);
        println!("Save Path: {}", task.save_path.display());
        println!("Status: {:?}", task.status);
        println!("Size: {}", task.size.map(|s| output::format_bytes(s)).unwrap_or_else(|| "Unknown".to_string()));
        println!("Downloaded: {}", output::format_bytes(task.downloaded));
        println!("Priority: {}", task.priority);
        println!("Resume Supported: {}", task.resume_supported);
        println!("Retry Count: {}", task.retry_count);
        println!("\nTimestamps:");
        println!("  Created: {}", task.created_at.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S"));
        if let Some(started) = task.started_at {
            println!("  Started: {}", started.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S"));
        }
        if let Some(completed) = task.completed_at {
            println!("  Completed: {}", completed.with_timezone(&chrono::Local).format("%Y-%m-%d %H:%M:%S"));
        }
        if let Some(etag) = &task.etag {
            println!("\nETag: {}", etag);
        }
        if let Some(last_modified) = &task.last_modified {
            println!("Last-Modified: {}", last_modified);
        }
        if let Some(error) = &task.error_message {
            println!("\nError: {}", error);
        }
        if !task.headers.is_empty() {
            println!("\nHeaders:");
            for (key, value) in &task.headers {
                println!("  {}: {}", key, crate::util::redact::display_header_value(key, value));
            }
        }
    }

    Ok(error::SUCCESS)
}

/// Validate configuration
async fn handle_debug_validate_config(state: &AppState) -> Result<i32> {
    let config = state.config.read().await;

    println!("Validating Configuration...\n");

    let mut issues = Vec::new();

    // Check default directory
    if !config.download.default_directory.exists() {
        issues.push(format!("Default directory does not exist: {}", config.download.default_directory.display()));
    }

    // Check max_concurrent
    if config.download.max_concurrent == 0 {
        issues.push("max_concurrent cannot be 0".to_string());
    }

    // Check scripts directory
    if config.scripts.enabled && !config.scripts.directory.exists() {
        issues.push(format!("Scripts directory does not exist: {}", config.scripts.directory.display()));
    }

    // Check folder configurations
    for (folder_id, folder_config) in &config.folders {
        if !folder_config.save_path.exists() {
            issues.push(format!("Folder '{}' save path does not exist: {}", folder_id, folder_config.save_path.display()));
        }
    }

    if issues.is_empty() {
        println!("✓ Configuration is valid");
        Ok(error::SUCCESS)
    } else {
        println!("✗ Configuration has {} issue(s):\n", issues.len());
        for (i, issue) in issues.iter().enumerate() {
            println!("{}. {}", i + 1, issue);
        }
        Ok(error::ERROR)
    }
}

/// Check queue integrity
async fn handle_debug_check_queue(manager: &DownloadManager, json: bool) -> Result<i32> {
    let tasks = manager.get_all_downloads().await;

    let mut issues = Vec::new();

    // Check for duplicate IDs
    let mut seen_ids = std::collections::HashSet::new();
    for task in &tasks {
        if !seen_ids.insert(task.id) {
            issues.push(format!("Duplicate task ID: {}", task.id));
        }
    }

    // Check for tasks with invalid save paths
    for task in &tasks {
        if !task.save_path.exists() {
            let parent = task.save_path.parent();
            if parent.map(|p| !p.exists()).unwrap_or(true) {
                issues.push(format!("Task {} has invalid save path: {}", task.id, task.save_path.display()));
            }
        }
    }

    // Check for tasks with empty URLs
    for task in &tasks {
        if task.url.is_empty() {
            issues.push(format!("Task {} has empty URL", task.id));
        }
    }

    if json {
        let result = serde_json::json!({
            "total_tasks": tasks.len(),
            "issues": issues,
            "is_valid": issues.is_empty(),
        });
        println!("{}", serde_json::to_string_pretty(&result)?);
    } else {
        println!("Queue Integrity Check\n");
        println!("Total Tasks: {}", tasks.len());

        if issues.is_empty() {
            println!("\n✓ Queue is valid");
        } else {
            println!("\n✗ Found {} issue(s):\n", issues.len());
            for (i, issue) in issues.iter().enumerate() {
                println!("{}. {}", i + 1, issue);
            }
        }
    }

    if issues.is_empty() {
        Ok(error::SUCCESS)
    } else {
        Ok(error::ERROR)
    }
}

/// Handle script management commands
async fn handle_script(action: ScriptAction, state: &AppState) -> Result<i32> {
    match action {
        ScriptAction::List { enabled_only, json } => handle_script_list(state, enabled_only, json).await,
        ScriptAction::Enable { name } => handle_script_enable(state, name).await,
        ScriptAction::Disable { name } => handle_script_disable(state, name).await,
        ScriptAction::Test { name, event, url } => handle_script_test(state, name, event, url).await,
        ScriptAction::Reload => handle_script_reload(state).await,
    }
}

/// List all scripts
async fn handle_script_list(state: &AppState, enabled_only: bool, json: bool) -> Result<i32> {
    let config = state.config.read().await;

    if !config.scripts.enabled {
        println!("Scripts are globally disabled");
        return Ok(error::SUCCESS);
    }

    let scripts_dir = &config.scripts.directory;

    if !scripts_dir.exists() {
        return Err(anyhow::anyhow!("Scripts directory does not exist: {}", scripts_dir.display()));
    }

    // List all .js files in scripts directory
    let mut scripts = Vec::new();
    for entry in std::fs::read_dir(scripts_dir)? {
        let entry = entry?;
        let path = entry.path();

        if path.extension().and_then(|s| s.to_str()) == Some("js") {
            let filename = path.file_name()
                .and_then(|f| f.to_str())
                .unwrap_or("")
                .to_string();

            // Check if script is enabled
            let is_enabled = config.scripts.script_files
                .get(&filename)
                .copied()
                .unwrap_or(true); // Default: enabled

            if enabled_only && !is_enabled {
                continue;
            }

            scripts.push((filename, is_enabled));
        }
    }

    scripts.sort_by(|a, b| a.0.cmp(&b.0));

    if json {
        let script_list: Vec<serde_json::Value> = scripts
            .iter()
            .map(|(name, enabled)| {
                serde_json::json!({
                    "name": name,
                    "enabled": enabled,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&script_list)?);
    } else {
        println!("Scripts ({} total)\n", scripts.len());
        for (name, enabled) in scripts {
            let status = if enabled { "✓ enabled " } else { "✗ disabled" };
            println!("{} {}", status, name);
        }
    }

    Ok(error::SUCCESS)
}

/// Enable a script
async fn handle_script_enable(state: &AppState, name: String) -> Result<i32> {
    let mut config = state.config.write().await;

    // Verify script exists
    let script_path = config.scripts.directory.join(&name);
    if !script_path.exists() {
        return Err(anyhow::anyhow!("Script not found: {}", name));
    }

    // Set enabled status
    config.scripts.script_files.insert(name.clone(), true);
    config.save()?;

    println!("Enabled script: {}", name);
    println!("Saved. Takes effect the next time ggg starts.");

    Ok(error::SUCCESS)
}

/// Disable a script
async fn handle_script_disable(state: &AppState, name: String) -> Result<i32> {
    let mut config = state.config.write().await;

    // Verify script exists
    let script_path = config.scripts.directory.join(&name);
    if !script_path.exists() {
        return Err(anyhow::anyhow!("Script not found: {}", name));
    }

    // Set disabled status
    config.scripts.script_files.insert(name.clone(), false);
    config.save()?;

    println!("Disabled script: {}", name);
    println!("Saved. Takes effect the next time ggg starts.");

    Ok(error::SUCCESS)
}

/// Test a script (dry run)
async fn handle_script_test(
    state: &AppState,
    name: String,
    event: String,
    url: String,
) -> Result<i32> {
    let config = state.config.read().await;

    // Verify script exists
    let script_path = config.scripts.directory.join(&name);
    if !script_path.exists() {
        return Err(anyhow::anyhow!("Script not found: {}", name));
    }

    // Parse event
    let hook_event = match event.as_str() {
        "beforeRequest" | "before_request" => HookEvent::BeforeRequest,
        "headersReceived" | "headers_received" => HookEvent::HeadersReceived,
        "completed" => HookEvent::Completed,
        "errorOccurred" | "error_occurred" | "error" => HookEvent::ErrorOccurred,
        "progress" => HookEvent::Progress,
        _ => return Err(anyhow::anyhow!("Invalid event: {}. Valid events: beforeRequest, headersReceived, completed, errorOccurred, progress", event)),
    };

    println!("Testing script: {}", name);
    println!("Event: {:?}", hook_event);
    println!("URL: {}\n", url);

    // Create test engine
    let timeout = std::time::Duration::from_secs(config.scripts.timeout);
    let mut engine = crate::script::engine::ScriptEngine::new(timeout)?;

    // Load the script
    engine.load_script(&script_path)?;

    // Create test context (for now only support beforeRequest)
    match hook_event {
        HookEvent::BeforeRequest => {
            let mut ctx = BeforeRequestContext {
                url: url.clone(),
                headers: HashMap::new(),
                user_agent: None,
                download_id: None,
            };

            let effective_scripts = HashMap::new();
            let result = engine.execute_handlers(hook_event, &mut ctx, &effective_scripts)?;

            println!("Execution result: {}", if result { "Continue" } else { "Stop" });
            println!("\nModified context:");
            println!("  URL: {}", ctx.url);
            if let Some(ref ua) = ctx.user_agent {
                println!("  User-Agent: {}", ua);
            }
            if !ctx.headers.is_empty() {
                println!("  Headers:");
                for (key, value) in &ctx.headers {
                    println!("    {}: {}", key, crate::util::redact::display_header_value(key, value));
                }
            }
        }
        _ => {
            println!("Note: Only 'beforeRequest' event is currently supported for testing");
        }
    }

    Ok(error::SUCCESS)
}

/// Reload all scripts
///
/// Only reached when no daemon is running: `script reload` is routed through
/// the daemon (see `cli::remote::classify`) whenever one is, so this is the
/// "there is nothing to reload" case, not the "not implemented" case.
async fn handle_script_reload(_state: &AppState) -> Result<i32> {
    println!("No ggg daemon is running; there is nothing to reload.");
    println!("Start one with 'ggg --headless' to enable live script reload.");

    Ok(error::SUCCESS)
}

/// Handle folder management commands
async fn handle_folder(action: FolderAction, state: &AppState) -> Result<i32> {
    match action {
        FolderAction::List { json } => handle_folder_list(state, json).await,
        FolderAction::Create { id, path, auto_start } => handle_folder_create(state, id, path, auto_start).await,
        FolderAction::Show { id, json } => handle_folder_show(state, id, json).await,
        FolderAction::Config { id, set } => handle_folder_config(state, id, set).await,
        FolderAction::Delete { id } => handle_folder_delete(state, id).await,
    }
}

/// List all folders
async fn handle_folder_list(state: &AppState, json: bool) -> Result<i32> {
    let config = state.config.read().await;

    if config.folders.is_empty() {
        println!("No folders configured");
        return Ok(error::SUCCESS);
    }

    if json {
        let folders: Vec<serde_json::Value> = config.folders
            .iter()
            .map(|(id, folder)| {
                serde_json::json!({
                    "id": id,
                    "name": folder.name,
                    "save_path": folder.save_path.display().to_string(),
                    "auto_date_directory": folder.auto_date_directory,
                    "auto_start_downloads": folder.auto_start_downloads,
                    "scripts_enabled": folder.scripts_enabled,
                    "max_concurrent": folder.max_concurrent,
                })
            })
            .collect();
        println!("{}", serde_json::to_string_pretty(&folders)?);
    } else {
        println!("Folders ({} total)\n", config.folders.len());
        for (id, folder) in &config.folders {
            println!("Name: {}", folder.name);
            println!("  ID: {}", id);
            println!("  Path: {}", folder.save_path.display());
            println!("  Auto-Date Directory: {}", folder.auto_date_directory);
            println!("  Auto-Start: {}", folder.auto_start_downloads);
            if let Some(enabled) = folder.scripts_enabled {
                println!("  Scripts: {}", if enabled { "enabled" } else { "disabled" });
            }
            if let Some(max_concurrent) = folder.max_concurrent {
                println!("  Max Concurrent: {}", max_concurrent);
            }
            println!();
        }
    }

    Ok(error::SUCCESS)
}

/// Create a new folder
async fn handle_folder_create(
    state: &AppState,
    id: String,
    path: String,
    auto_start: bool,
) -> Result<i32> {
    let mut config = state.config.write().await;

    // Check if folder with same display name already exists
    if config.folders.values().any(|f| f.name == id) {
        return Err(anyhow::anyhow!("Folder '{}' already exists", id));
    }

    // Create folder config with UUID key
    let folder_id = Config::generate_folder_id();
    let folder_config = FolderConfig {
        name: id.clone(),
        save_path: PathBuf::from(&path),
        auto_date_directory: false,
        auto_start_downloads: auto_start,
        scripts_enabled: None,
        script_files: None,
        max_concurrent: None,
        user_agent: None,
        referrer_policy: None,
        default_headers: HashMap::new(),
        prevent_duplicate_url: false,
    };

    // Create directory if it doesn't exist
    std::fs::create_dir_all(&folder_config.save_path)?;

    config.folders.insert(folder_id, folder_config);
    config.save()?;

    println!("Created folder: {}", id);
    println!("  Path: {}", path);
    println!("  Auto-Start: {}", auto_start);

    Ok(error::SUCCESS)
}

/// Show folder settings
async fn handle_folder_show(state: &AppState, id: String, json: bool) -> Result<i32> {
    let config = state.config.read().await;

    let folder_id = resolve_folder_id(&config, &id)
        .ok_or_else(|| anyhow::anyhow!("Folder '{}' not found", id))?;
    let folder = config.folders.get(&folder_id).unwrap();

    if json {
        let folder_info = serde_json::json!({
            "id": id,
            "save_path": folder.save_path.display().to_string(),
            "auto_date_directory": folder.auto_date_directory,
            "auto_start_downloads": folder.auto_start_downloads,
            "scripts_enabled": folder.scripts_enabled,
            "max_concurrent": folder.max_concurrent,
            "user_agent": folder.user_agent,
            "default_headers": folder.default_headers,
            "script_files": folder.script_files,
        });
        println!("{}", serde_json::to_string_pretty(&folder_info)?);
    } else {
        println!("Folder: {}\n", id);
        println!("Save Path: {}", folder.save_path.display());
        println!("Auto-Date Directory: {}", folder.auto_date_directory);
        println!("Auto-Start Downloads: {}", folder.auto_start_downloads);

        if let Some(enabled) = folder.scripts_enabled {
            println!("Scripts Enabled: {}", enabled);
        } else {
            println!("Scripts Enabled: (inherit from application)");
        }

        if let Some(max_concurrent) = folder.max_concurrent {
            println!("Max Concurrent: {}", max_concurrent);
        } else {
            println!("Max Concurrent: (inherit from application)");
        }

        if let Some(ref ua) = folder.user_agent {
            println!("User-Agent: {}", ua);
        }

        if !folder.default_headers.is_empty() {
            println!("\nDefault Headers:");
            for (key, value) in &folder.default_headers {
                println!("  {}: {}", key, value);
            }
        }

        if let Some(ref script_files) = folder.script_files {
            if !script_files.is_empty() {
                println!("\nScript Files:");
                for (name, enabled) in script_files {
                    let status = if *enabled { "enabled" } else { "disabled" };
                    println!("  {} - {}", name, status);
                }
            }
        }
    }

    Ok(error::SUCCESS)
}

/// Update folder configuration
async fn handle_folder_config(state: &AppState, id: String, set: String) -> Result<i32> {
    let mut config = state.config.write().await;

    let folder_id = resolve_folder_id(&config, &id)
        .ok_or_else(|| anyhow::anyhow!("Folder '{}' not found", id))?;
    let folder = config.folders.get_mut(&folder_id).unwrap();

    // Parse key=value, splitting only on the first '=' so values may contain
    // '=' (e.g. a user-agent or header value).
    let (key, value) = set
        .split_once('=')
        .ok_or_else(|| anyhow::anyhow!("Invalid format. Expected: key=value"))?;
    let key = key.trim();
    let value = value.trim();

    // Update configuration
    match key {
        "auto_date_directory" => {
            folder.auto_date_directory = value.parse()?;
            println!("Updated auto_date_directory to {}", value);
        }
        "auto_start_downloads" => {
            folder.auto_start_downloads = value.parse()?;
            println!("Updated auto_start_downloads to {}", value);
        }
        "max_concurrent" => {
            folder.max_concurrent = Some(value.parse()?);
            println!("Updated max_concurrent to {}", value);
        }
        "scripts_enabled" => {
            folder.scripts_enabled = Some(value.parse()?);
            println!("Updated scripts_enabled to {}", value);
        }
        "user_agent" => {
            folder.user_agent = Some(value.to_string());
            println!("Updated user_agent to {}", value);
        }
        _ => return Err(anyhow::anyhow!("Unknown configuration key: {}. Valid keys: auto_date_directory, auto_start_downloads, max_concurrent, scripts_enabled, user_agent", key)),
    }

    config.save()?;

    Ok(error::SUCCESS)
}

/// Delete a folder
async fn handle_folder_delete(state: &AppState, id: String) -> Result<i32> {
    let mut config = state.config.write().await;

    let folder_id = resolve_folder_id(&config, &id)
        .ok_or_else(|| anyhow::anyhow!("Folder '{}' not found", id))?;
    let display_name = config.folder_name(&folder_id);

    config.folders.remove(&folder_id);
    config.save()?;

    println!("Deleted folder: {}", display_name);
    println!("Note: Files in the folder's save path were not deleted");

    Ok(error::SUCCESS)
}

// ========================================
// Batch Operations
// ========================================

/// Resolves `start-all`/`pause-all`'s optional `--folder` (a UUID key or a
/// display name, like `add`/`list`) to a folder key. `None` stays `None`
/// (every folder); an unresolved name is the caller's job to reject.
async fn resolve_optional_folder_standalone(
    state: &AppState,
    folder: &Option<String>,
) -> Option<Option<String>> {
    match folder {
        Some(f) => {
            let config = state.config.read().await.clone();
            resolve_folder_id(&config, f).map(Some)
        }
        None => Some(None),
    }
}

/// Start all downloads
async fn handle_start_all(
    state: &AppState,
    manager: &DownloadManager,
    folder: Option<String>,
) -> Result<i32> {
    // Resolved the same way `list --folder`/`add --folder` are, so a display
    // name matches its tasks instead of matching nothing (a raw string
    // compare against `task.folder_id` only ever matches the UUID key).
    let Some(folder_id) = resolve_optional_folder_standalone(state, &folder).await else {
        eprintln!("Error: unknown folder");
        return Ok(error::INVALID_INPUT);
    };

    let tasks = manager.get_all_downloads().await;

    let mut started_count = 0;
    for task in tasks {
        if let Some(ref f) = folder_id {
            if task.folder_id != *f {
                continue;
            }
        }

        // Only start pending or paused tasks
        if matches!(task.status, DownloadStatus::Pending | DownloadStatus::Paused) {
            match manager.start_download(task.id, state.script_sender.clone(), state.config.clone()).await {
                Ok(StartOutcome::Started) => started_count += 1,
                Ok(_) => {}
                Err(e) => tracing::warn!("Failed to start {}: {}", task.filename, e),
            }
        }
    }

    manager.save_queue_to_folders().await?;

    println!("Started {} download(s)", started_count);
    Ok(error::SUCCESS)
}

/// Pause all downloads
async fn handle_pause_all(
    state: &AppState,
    manager: &DownloadManager,
    folder: Option<String>,
) -> Result<i32> {
    let Some(folder_id) = resolve_optional_folder_standalone(state, &folder).await else {
        eprintln!("Error: unknown folder");
        return Ok(error::INVALID_INPUT);
    };

    let paused_count = match folder_id {
        Some(ref f) => manager.stop_folder_tasks(f).await,
        None => manager.pause_all().await,
    };

    manager.save_queue_to_folders().await?;

    println!("Paused {} download(s)", paused_count);
    Ok(error::SUCCESS)
}

/// Clear downloads by status
async fn handle_clear(
    manager: &DownloadManager,
    status_str: String,
    folder: Option<String>,
) -> Result<i32> {
    // Parse status list (comma-separated)
    let statuses: Vec<&str> = status_str
        .split(',')
        .map(|s| s.trim())
        .filter(|s| !s.is_empty())
        .collect();

    // Validate up front: an unknown/misspelled status would otherwise match
    // nothing and silently report "Removed 0".
    const VALID_STATUSES: &[&str] = &["completed", "error", "paused", "pending"];
    if statuses.is_empty() {
        return Err(anyhow::anyhow!("No status specified. Valid: {}", VALID_STATUSES.join(", ")));
    }
    if let Some(unknown) = statuses.iter().find(|s| !VALID_STATUSES.contains(s)) {
        return Err(anyhow::anyhow!(
            "Unknown status '{}'. Valid: {}",
            unknown,
            VALID_STATUSES.join(", ")
        ));
    }

    let tasks = manager.get_all_downloads().await;
    let mut removed_count = 0;

    for task in tasks {
        // Filter by folder if specified
        if let Some(ref folder_filter) = folder {
            if task.folder_id != *folder_filter {
                continue;
            }
        }

        // Check if task status matches any of the specified statuses
        let should_remove = statuses.iter().any(|status| {
            match *status {
                "completed" => matches!(task.status, DownloadStatus::Completed),
                "error" => matches!(task.status, DownloadStatus::Error),
                "paused" => matches!(task.status, DownloadStatus::Paused),
                "pending" => matches!(task.status, DownloadStatus::Pending),
                _ => false,
            }
        });

        if should_remove {
            match manager.remove_download(task.id).await {
                Ok(Some(_)) => removed_count += 1,
                Ok(None) => {}
                Err(e) => tracing::warn!("Failed to remove {}: {}", task.filename, e),
            }
        }
    }

    manager.save_queue_to_folders().await?;

    println!("Removed {} download(s)", removed_count);
    Ok(error::SUCCESS)
}

/// Enqueue path for `batch-add`, mirroring `handle_add`'s loop so folder
/// rules and duplicate handling match: resolves the target folder once, then
/// for each input line expands `[1-10]`-style range patterns, validates,
/// builds, and enqueues every resulting URL through the same
/// `build_task`/`enqueue` calls the daemon's `Add` handler uses. `Err(())`
/// means an unknown folder.
async fn enqueue_urls(
    state: &AppState,
    manager: &DownloadManager,
    urls: &[String],
    folder: Option<&str>,
) -> Result<(Vec<AddedTask>, Vec<SkippedUrl>), ()> {
    let config = state.config.read().await.clone();
    let folder_id = resolve_target_folder(&config, folder).map_err(|_| ())?;
    let opts = EnqueueOptions::default();

    let mut added = Vec::new();
    let mut skipped = Vec::new();
    for line in urls {
        for u in crate::util::url_expansion::expand_url(line) {
            if !is_valid_download_url(&u) {
                skipped.push(SkippedUrl {
                    url: u,
                    reason: "invalid_url".to_string(),
                });
                continue;
            }

            let task = build_task(&u, &folder_id, &config, &opts);
            match enqueue(manager, state, task, false, false).await {
                EnqueueOutcome::Added {
                    id,
                    started,
                    start_error,
                } => added.push(AddedTask {
                    id,
                    url: u,
                    folder_id: folder_id.clone(),
                    started,
                    start_error,
                }),
                EnqueueOutcome::Duplicate => skipped.push(SkippedUrl {
                    url: u,
                    reason: "duplicate".to_string(),
                }),
            }
        }
    }

    Ok((added, skipped))
}

/// Batch add downloads from file
async fn handle_batch_add(
    state: &AppState,
    manager: &DownloadManager,
    file: String,
    folder: Option<String>,
) -> Result<i32> {
    let file_path = PathBuf::from(&file);

    if !file_path.exists() {
        return Err(anyhow::anyhow!("File not found: {}", file));
    }

    let content = std::fs::read_to_string(&file_path)?;
    let urls: Vec<String> = content.lines()
        .map(|line| line.trim().to_string())
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();

    if urls.is_empty() {
        println!("No URLs found in file");
        return Ok(error::SUCCESS);
    }

    let (added, skipped) = match enqueue_urls(state, manager, &urls, folder.as_deref()).await {
        Ok(result) => result,
        Err(()) => {
            eprintln!("Error: unknown folder");
            return Ok(error::INVALID_INPUT);
        }
    };

    if !added.is_empty() {
        manager.save_queue_to_folders().await?;
    }

    println!("Added {} download(s) from {}", added.len(), file);
    // Same wording as `render_added`, so `add` and `batch-add` warn identically.
    for skip in &skipped {
        if skip.reason == "duplicate" {
            eprintln!("Skipped duplicate URL: {}", skip.url);
        } else {
            eprintln!("Skipping invalid URL: {}", skip.url);
        }
    }
    // Still a success: a batch file with only stale lines is not an error
    // for scripted use, but it is worth a visible note.
    if added.is_empty() {
        eprintln!("Warning: no downloads were added from {}", file);
    }

    Ok(error::SUCCESS)
}

/// Change a queued download's referrer/headers. Works the same standalone
/// as when routed through the daemon (`remote::send_and_render`'s `Modify`
/// arm): both apply the edit through `DownloadManager::edit_task_headers`
/// and the shared `daemon_ops::apply_header_edit`, so the task ends up in
/// the same state either way.
async fn handle_modify(
    manager: &DownloadManager,
    id_str: String,
    referrer: Option<super::RedactedArg>,
    clear_referrer: bool,
    header: Vec<super::HeaderArg>,
    remove_header: Vec<String>,
) -> Result<i32> {
    let id = parse_task_id(&id_str)?;

    let mut set_headers = Vec::with_capacity(header.len());
    for h in &header {
        match super::remote::parse_header_arg(h) {
            Ok(pair) => set_headers.push(pair),
            Err(e) => {
                eprintln!("Error: {e}");
                return Ok(error::INVALID_INPUT);
            }
        }
    }

    let edit = crate::ipc::control::protocol::HeaderEdit {
        referrer: referrer.map(|r| r.0),
        clear_referrer,
        set_headers,
        remove_headers: remove_header,
    };

    match manager
        .edit_task_headers(id, move |task| super::daemon_ops::apply_header_edit(task, &edit))
        .await
    {
        Ok(task) => {
            manager.save_queue_to_folders().await?;
            super::remote::render_modified(task);
            Ok(error::SUCCESS)
        }
        Err(EditHeadersError::NotFound) => {
            eprintln!("Error: Download not found");
            Ok(error::NOT_FOUND)
        }
        Err(e @ EditHeadersError::Busy(..)) => {
            eprintln!("Error: {e}");
            Ok(error::INVALID_INPUT)
        }
    }
}

// ========================================
// Priority and Queue Operations
// ========================================

/// Set download priority
async fn handle_priority(
    manager: &DownloadManager,
    id_str: String,
    priority: u8,
) -> Result<i32> {
    let id = parse_task_id(&id_str)?;

    manager.set_priority(id, priority).await?;
    manager.save_queue_to_folders().await?;

    println!("Set priority to {} for download {}", priority, id);
    Ok(error::SUCCESS)
}

/// Move download in queue or to another folder
async fn handle_move(
    manager: &DownloadManager,
    state: &AppState,
    id_str: String,
    to_top: bool,
    to_bottom: bool,
    before: Option<String>,
    folder: Option<String>,
) -> Result<i32> {
    let id = parse_task_id(&id_str)?;

    // Mutual exclusivity is enforced by the clap `move_op` group; here we only
    // need to require that at least one operation was given.
    if !to_top && !to_bottom && before.is_none() && folder.is_none() {
        return Err(anyhow::anyhow!("Must specify one of: --to-top, --to-bottom, --before, --folder"));
    }

    if to_top {
        manager.move_to_top(id).await?;
        println!("Moved download {} to top of queue", id);
    } else if to_bottom {
        manager.move_to_bottom(id).await?;
        println!("Moved download {} to bottom of queue", id);
    } else if let Some(before_id_str) = before {
        let before_id = Uuid::parse_str(&before_id_str)
            .map_err(|_| anyhow::anyhow!("Invalid before UUID format"))?;
        manager.move_before(id, before_id).await?;
        println!("Moved download {} before {}", id, before_id);
    } else if let Some(folder_arg) = folder {
        // Accept a display name as well as the UUID key, as the other folder
        // subcommands do. The read guard is dropped before the move, which
        // takes the config lock itself.
        let folder_id = {
            let config = state.config.read().await;
            resolve_folder_id(&config, &folder_arg)
                .ok_or_else(|| anyhow::anyhow!("Folder '{}' not found", folder_arg))?
        };
        manager.change_folder(id, folder_id.clone(), Some(&state.config)).await?;
        println!("Moved download {} to folder '{}'", id, folder_id);
    }

    manager.save_queue_to_folders().await?;
    Ok(error::SUCCESS)
}

// ========================================
// Export/Import
// ========================================

/// Handle export commands
async fn handle_export(
    action: ExportAction,
    _state: &AppState,
    manager: &DownloadManager,
) -> Result<i32> {
    match action {
        ExportAction::Queue { output } => handle_export_queue(manager, output).await,
        ExportAction::Config { output } => handle_export_config(_state, output).await,
    }
}

/// Export queue to file
async fn handle_export_queue(
    manager: &DownloadManager,
    output: String,
) -> Result<i32> {
    let output_path = PathBuf::from(&output);

    let tasks = manager.get_all_downloads().await;
    let json = serde_json::to_string_pretty(&tasks)?;
    std::fs::write(&output_path, json)?;

    println!("Exported {} task(s) to {}", tasks.len(), output);
    Ok(error::SUCCESS)
}

/// Export configuration to file
async fn handle_export_config(
    state: &AppState,
    output: String,
) -> Result<i32> {
    let output_path = PathBuf::from(&output);

    let config = state.config.read().await;
    let toml = toml::to_string_pretty(&*config)?;
    std::fs::write(&output_path, toml)?;

    println!("Exported configuration to {}", output);
    Ok(error::SUCCESS)
}

/// Handle import commands
async fn handle_import(
    action: ImportAction,
    state: &AppState,
    manager: &DownloadManager,
) -> Result<i32> {
    match action {
        ImportAction::Queue { input } => handle_import_queue(manager, input).await,
        ImportAction::Config { input } => handle_import_config(state, input).await,
    }
}

/// Import queue from file
async fn handle_import_queue(
    manager: &DownloadManager,
    input: String,
) -> Result<i32> {
    let input_path = PathBuf::from(&input);

    if !input_path.exists() {
        return Err(anyhow::anyhow!("File not found: {}", input));
    }

    let content = std::fs::read_to_string(&input_path)?;
    let tasks: Vec<DownloadTask> = serde_json::from_str(&content)?;

    for task in &tasks {
        manager.add_download(task.clone()).await;
    }

    manager.save_queue_to_folders().await?;

    println!("Imported {} task(s) from {}", tasks.len(), input);
    Ok(error::SUCCESS)
}

/// Import configuration from file
async fn handle_import_config(
    state: &AppState,
    input: String,
) -> Result<i32> {
    let input_path = PathBuf::from(&input);

    if !input_path.exists() {
        return Err(anyhow::anyhow!("File not found: {}", input));
    }

    let content = std::fs::read_to_string(&input_path)?;
    let new_config: Config = toml::from_str(&content)?;

    let mut config = state.config.write().await;
    *config = new_config;
    config.save()?;

    println!("Imported configuration from {}", input);
    println!("Note: Application restart may be required for some settings to take effect");
    Ok(error::SUCCESS)
}

// ========================================
// Test Utilities
// ========================================

/// Handle test utility commands
async fn handle_test(
    action: TestAction,
    state: &AppState,
    manager: &DownloadManager,
) -> Result<i32> {
    match action {
        TestAction::GenerateTasks { count, folder } => {
            handle_test_generate_tasks(state, manager, count, folder).await
        }
        TestAction::ResetQueue => handle_test_reset_queue(manager).await,
        TestAction::ResetConfig => handle_test_reset_config(state).await,
    }
}

/// Generate test download tasks
async fn handle_test_generate_tasks(
    state: &AppState,
    manager: &DownloadManager,
    count: usize,
    folder: Option<String>,
) -> Result<i32> {
    let config = state.config.read().await;
    let save_path = config.download.default_directory.clone();
    drop(config);

    println!("Generating {} test task(s)...", count);

    for i in 0..count {
        let url = format!("http://example.com/test_file_{}.zip", i);
        let mut task = DownloadTask::new(url, save_path.clone());
        task.filename = format!("test_file_{}.zip", i);

        if let Some(ref folder_id) = folder {
            task.folder_id = folder_id.clone();
        }

        manager.add_download(task).await;
    }

    manager.save_queue_to_folders().await?;

    println!("Generated {} test task(s)", count);
    Ok(error::SUCCESS)
}

/// Reset queue (delete all downloads)
async fn handle_test_reset_queue(manager: &DownloadManager) -> Result<i32> {
    let tasks = manager.get_all_downloads().await;
    let count = tasks.len();

    for task in tasks {
        if let Err(e) = manager.remove_download(task.id).await {
            tracing::warn!("Failed to remove {}: {}", task.filename, e);
        }
    }

    manager.save_queue_to_folders().await?;

    println!("Removed all {} task(s) from queue", count);
    Ok(error::SUCCESS)
}

/// Reset configuration to defaults
async fn handle_test_reset_config(state: &AppState) -> Result<i32> {
    let mut config = state.config.write().await;
    *config = Config::default();
    config.save()?;

    println!("Reset configuration to defaults");
    println!("Note: Application restart may be required");
    Ok(error::SUCCESS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn task_with_status(status: DownloadStatus) -> DownloadTask {
        let mut task = DownloadTask::new(
            "https://example.com/a".to_string(),
            std::path::PathBuf::from("a"),
        );
        task.status = status;
        task
    }

    /// A single-folder config named `name`, so `resolve_target_folder(None)`
    /// picks it without needing a folder named "default".
    fn config_with_folder(name: &str, prevent_duplicate: bool) -> (Config, String) {
        let mut config = Config::default();
        let mut folder = config
            .folders
            .get("default")
            .cloned()
            .unwrap_or_default();
        folder.name = name.to_string();
        folder.prevent_duplicate_url = prevent_duplicate;
        config.folders.clear();
        let folder_id = name.to_string();
        config.folders.insert(folder_id.clone(), folder);
        (config, folder_id)
    }

    #[tokio::test]
    async fn batch_enqueue_applies_folder_save_path_and_default_headers() {
        let (mut config, folder_id) = config_with_folder("downloads", false);
        {
            let folder = config.folders.get_mut(&folder_id).unwrap();
            folder.save_path = std::path::PathBuf::from("/batch-add-save-path");
            folder.default_headers.insert("X-Batch".to_string(), "1".to_string());
        }
        let state = AppState::new(config, "en");
        let manager = DownloadManager::new();

        let urls = vec!["https://example.com/f.zip".to_string()];
        let (added, skipped) = enqueue_urls(&state, &manager, &urls, Some(folder_id.as_str()))
            .await
            .expect("known folder");

        assert!(skipped.is_empty());
        assert_eq!(added.len(), 1);
        let stored = manager.get_by_id(added[0].id).await.expect("task stored");
        assert_eq!(stored.save_path, std::path::PathBuf::from("/batch-add-save-path"));
        assert_eq!(stored.headers.get("X-Batch").map(String::as_str), Some("1"));
    }

    #[tokio::test]
    async fn batch_enqueue_skips_invalid_and_duplicate() {
        let (config, folder_id) = config_with_folder("downloads", true);
        let state = AppState::new(config, "en");
        let manager = DownloadManager::new();

        let urls = vec![
            "not a url".to_string(),
            "https://example.com/f.zip".to_string(),
            "https://example.com/f.zip".to_string(),
        ];
        let (added, skipped) = enqueue_urls(&state, &manager, &urls, Some(folder_id.as_str()))
            .await
            .expect("known folder");

        assert_eq!(added.len(), 1);
        assert_eq!(skipped.len(), 2);
        assert_eq!(skipped[0].reason, "invalid_url");
        assert_eq!(skipped[1].reason, "duplicate");
    }

    #[tokio::test]
    async fn batch_enqueue_unknown_folder_is_err() {
        let (config, _) = config_with_folder("downloads", false);
        let state = AppState::new(config, "en");
        let manager = DownloadManager::new();

        let urls = vec!["https://example.com/f.zip".to_string()];
        let result = enqueue_urls(&state, &manager, &urls, Some("no-such-folder")).await;

        assert!(result.is_err());
    }

    /// A task in retry backoff (live handle, persisted status `Paused`) must
    /// not be reported as a real pause: the daemon's own `wait_state` treats
    /// it as "keep waiting", so `wait_until_final` must poll past it rather
    /// than returning `Paused` early.
    #[tokio::test]
    async fn standalone_wait_backoff_is_not_terminal() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Paused);
        let id = task.id;
        manager.add_download(task).await;

        let handle = tokio::spawn(std::future::pending::<()>());
        manager.insert_active_handle_for_test(id, handle).await;
        manager.mark_backoff_for_test(id, true).await;

        let result = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_until_final(id, &manager, std::time::Duration::from_millis(50)),
        )
        .await;
        assert!(result.is_err(), "a backoff task must not be reported as terminal");

        manager.abort_active_handle_for_test(id).await;
    }

    #[tokio::test]
    async fn standalone_wait_completed_via_history() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Downloading);
        let id = task.id;
        manager.add_to_history(task).await;

        let (outcome, task) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_until_final(id, &manager, std::time::Duration::from_millis(50)),
        )
        .await
        .expect("a history entry must resolve immediately");
        assert_eq!(outcome, WaitOutcome::Completed);
        assert_eq!(task.map(|t| t.id), Some(id));
    }

    #[tokio::test]
    async fn standalone_wait_manual_pause_is_paused() {
        let manager = DownloadManager::new();
        let task = task_with_status(DownloadStatus::Paused);
        let id = task.id;
        manager.add_download(task).await;

        let (outcome, task) = tokio::time::timeout(
            std::time::Duration::from_secs(1),
            wait_until_final(id, &manager, std::time::Duration::from_millis(50)),
        )
        .await
        .expect("a manually paused task with no live handle must resolve immediately");
        assert_eq!(outcome, WaitOutcome::Paused);
        assert_eq!(task.map(|t| t.status), Some(DownloadStatus::Paused));
        assert_eq!(remote::exit_code_for_wait(WaitOutcome::Paused), error::TASK_PAUSED);
    }
}
