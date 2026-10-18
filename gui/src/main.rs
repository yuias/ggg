#![cfg_attr(all(windows, not(debug_assertions)), windows_subsystem = "windows")]

mod app;
mod bridge;
mod commands;
mod dnd;
mod event;
mod gui_config;
mod i18n;
mod instance;
mod menu;
mod platform;
mod registration;
mod render;
mod settings;
mod shortcuts;
mod ui;
mod window;

use anyhow::{Context as _, Result};
use clap::Parser;
use event::UserEvent;
use std::{path::PathBuf, process::ExitCode, time::Duration};
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

    /// Links to add; a running instance for the same configuration receives
    /// them instead of this process opening a second window
    #[arg(value_name = "URL")]
    urls: Vec<String>,
}

fn main() -> Result<ExitCode> {
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

    // Before the core loads: a second start must not touch the queue files the
    // running instance owns. Returning (not exiting) flushes the log guard.
    let ipc = match instance::acquire(&rt, &args.urls) {
        instance::Start::Primary(ipc) => ipc,
        instance::Start::Done(code) => return Ok(code),
    };

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

    // Drag and drop needs OLE in a single-threaded apartment on this thread.
    // Declared before the loop so it is released after every window is gone.
    let _ole = dnd::init_ole();

    let event_loop = EventLoop::<UserEvent>::with_user_event()
        .build()
        .context("create event loop")?;
    let mut app = app::GuiApp::new(
        rt.handle().clone(),
        core,
        tr,
        gui_config,
        event_loop.create_proxy(),
        ipc,
    )
    .with_gui_config_path(gui_config_path.clone());
    if !args.urls.is_empty() {
        // Delivered once the loop runs, like a link from the pipe.
        let _ = event_loop
            .create_proxy()
            .send_event(UserEvent::IpcText(args.urls.join("\n")));
    }
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

    run_result.context("event loop")?;
    Ok(ExitCode::SUCCESS)
}
