#![cfg(windows)]
#![windows_subsystem = "windows"]

mod drop_target;
mod ipc_client;
mod window;

use std::sync::{Arc, Mutex};

/// Shared application state between window, drop target, and IPC client.
#[derive(Debug, Clone)]
pub struct AppState {
    /// Connection status with the TUI application
    pub connected: bool,
    /// Last URL sent via IPC
    pub last_url: Option<String>,
    /// Status message for the UI
    pub status_message: String,
    /// Named Pipe name to connect to
    pub pipe_name: String,
}

impl AppState {
    fn new(pipe_name: String) -> Self {
        Self {
            connected: false,
            last_url: None,
            status_message: "⌛️ Connecting...".to_string(),
            pipe_name,
        }
    }
}

/// Thread-safe shared state handle
pub type SharedState = Arc<Mutex<AppState>>;

fn main() {
    // Create Named Mutex for single-instance detection.
    // The mutex is held for the lifetime of the process and
    // automatically released by the OS on exit.
    let _instance_mutex = unsafe {
        use windows::Win32::Foundation::{GetLastError, ERROR_ALREADY_EXISTS};
        let mutex = windows::Win32::System::Threading::CreateMutexW(
            None,
            true,
            windows::core::w!("Global\\ggg-dnd-running"),
        );
        // ERROR_ALREADY_EXISTS means another ggg-dnd is already running. The
        // mutex exists primarily so ggg can detect ggg-dnd, so we intentionally
        // proceed rather than exit — but record it for diagnostics.
        if GetLastError() == ERROR_ALREADY_EXISTS {
            eprintln!("ggg-dnd: another instance appears to be running");
        }
        mutex
    };

    // Determine pipe name from command-line args or use default
    let pipe_name = std::env::args()
        .nth(1)
        .unwrap_or_else(ggg_ipc::default_pipe_name);

    let state = Arc::new(Mutex::new(AppState::new(pipe_name.clone())));

    // Start IPC connection monitor in a background thread
    let ipc_state = state.clone();
    std::thread::spawn(move || {
        ipc_client::connection_monitor(ipc_state);
    });

    // Run the Win32 GUI (blocks until window is closed)
    if let Err(e) = window::run(state) {
        eprintln!("Fatal error: {}", e);
        std::process::exit(1);
    }
}
