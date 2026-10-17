#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bridge;
mod event;
mod gui_config;
mod i18n;
mod window;

use anyhow::{Context as _, Result};
use clap::Parser;
use event::UserEvent;
use std::{path::PathBuf, time::Duration};
use winit::event_loop::EventLoop;

#[derive(Parser, Debug)]
#[command(name = "ggg-gui", version, about = "Native GUI front end for ggg")]
struct Args {
    /// Use this directory for configuration, queue files and logs
    #[arg(long, value_name = "DIR")]
    config: Option<PathBuf>,

    /// Log at TRACE level
    #[arg(short, long)]
    verbose: bool,
}

fn main() -> Result<()> {
    let args = Args::parse();

    // Applied before the log appender is created so logs land under the override.
    if let Some(dir) = args.config {
        ggg::util::paths::set_config_dir_override(Some(dir));
    }
    let _log_guard = ggg::app::bootstrap::init_logging(args.verbose);
    tracing::info!("Starting ggg-gui");

    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .thread_name("ggg-core")
        .build()
        .context("build tokio runtime")?;

    // Core start-up needs the runtime context; blocking is fine before the loop starts.
    let core = rt.block_on(ggg::app::bootstrap::init_core())?;
    let manager = core.manager.clone();
    let language = rt.block_on(core.state.config.read()).general.language.clone();
    let tr = i18n::Tr::load(&language);

    // After init_core so the config directory is already resolved and created.
    let gui_config_path = match gui_config::GuiConfig::default_path() {
        Ok(path) => Some(path),
        Err(e) => {
            tracing::warn!("GUI state will not be persisted: {e:#}");
            None
        }
    };
    let gui_config = gui_config_path
        .as_deref()
        .map(gui_config::GuiConfig::load_from)
        .unwrap_or_default();

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("create event loop")?;
    let mut app = app::GuiApp::new(
        rt.handle().clone(),
        core,
        tr,
        gui_config,
        event_loop.create_proxy(),
    );
    let run_result = event_loop.run_app(&mut app);

    // The loop is done, so blocking here cannot stall the UI.
    match rt.block_on(manager.save_queue_to_folders()) {
        Ok(()) => tracing::info!("GUI shutdown: queue saved"),
        Err(e) => tracing::error!("GUI shutdown: failed to save queue: {e:#}"),
    }

    if let Some(path) = &gui_config_path {
        match app.gui_config().save_to(path) {
            Ok(()) => tracing::info!("GUI shutdown: GUI state saved"),
            Err(e) => tracing::error!("GUI shutdown: failed to save GUI state: {e:#}"),
        }
    }

    drop(app);
    rt.shutdown_timeout(Duration::from_secs(3));

    run_result.context("event loop")
}
