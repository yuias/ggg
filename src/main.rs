use anyhow::Result;
use clap::Parser;
use ggg::{
    cli::{self, Cli},
    tui::run_tui,
};

#[tokio::main]
async fn main() -> Result<()> {
    // Parse CLI arguments first to get verbose flag
    let cli = Cli::parse();

    // Applied before the log appender is created, so `--config X` writes logs
    // under X, where `logs` looks for them.
    if let Some(ref config_dir) = cli.config {
        ggg::util::paths::set_config_dir_override(Some(config_dir.clone()));
    }

    let log_guard = ggg::app::bootstrap::init_logging(cli.verbose);

    tracing::info!("Starting GGG...");
    if cli.verbose {
        tracing::info!("Verbose logging enabled (TRACE level)");
    }
    tracing::trace!("CLI arguments: {:?}", cli);

    if let Some(ref config_dir) = cli.config {
        tracing::info!("Using config directory override: {:?}", config_dir);
    }

    // Computed once: routing needs it for every subcommand, and a headless
    // start below needs it again for the single-instance bind.
    let pipe_name = ggg::ipc::control::protocol::default_control_pipe_name();

    // Route before anything else touches the config or the queue files, so a
    // routed command's daemon round trip never pays for a load it doesn't
    // need, and a refused command never gets a chance to write standalone.
    if let Some(ref command) = cli.command {
        match ggg::cli::remote::route(command, &pipe_name).await {
            ggg::cli::remote::RouteDecision::Handled(code) => {
                drop(log_guard);
                std::process::exit(code);
            }
            ggg::cli::remote::RouteDecision::Standalone => {}
        }
    }

    // Bind the single-instance control pipe before loading the queue, so a
    // second `--headless` for the same config directory fails before it can
    // read or write any queue files.
    #[cfg(windows)]
    let control_binding: Option<ggg::cli::daemon_ops::ControlBinding> =
        if cli.command.is_none() && cli.headless {
            match ggg::ipc::control::server::bind(&pipe_name).await {
                Ok(bound) => Some(bound),
                Err(ggg::ipc::control::server::BindError::AlreadyRunning { pid }) => {
                    let pid_str = pid.map(|p| p.to_string()).unwrap_or_else(|| "unknown".to_string());
                    eprintln!("ggg daemon already running (pid {pid_str}) for this config directory");
                    drop(log_guard);
                    std::process::exit(ggg::cli::error::DAEMON_RUNNING);
                }
                Err(ggg::ipc::control::server::BindError::InvalidName(reason)) => {
                    eprintln!("invalid daemon pipe name {pipe_name:?}: {reason} (check GGG_DAEMON_PIPE)");
                    drop(log_guard);
                    std::process::exit(ggg::cli::error::ERROR);
                }
                Err(ggg::ipc::control::server::BindError::Io(e)) => {
                    eprintln!("failed to bind daemon control pipe: {e}");
                    drop(log_guard);
                    std::process::exit(ggg::cli::error::ERROR);
                }
            }
        } else {
            None
        };
    #[cfg(not(windows))]
    let control_binding: Option<ggg::cli::daemon_ops::ControlBinding> = None;

    let ggg::app::bootstrap::Core {
        state,
        manager: download_manager,
        recovered,
    } = ggg::app::bootstrap::init_core().await?;

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
                cli::daemon::run_daemon(state, download_manager, recovered, control_binding).await?;
            } else {
                // TUI mode (default)
                run_tui(state, download_manager).await?;
            }
        }
    }

    Ok(())
}
