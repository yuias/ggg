pub mod app;
pub mod cli;
pub mod download;
pub mod file;
pub mod ipc;
pub mod script; // Phase 3 - in progress
pub mod tui;
pub mod ui;
pub mod util;

pub use app::{config::Config, state::AppState};

/// Product version, shown by front ends that have their own crate version.
pub const VERSION: &str = env!("CARGO_PKG_VERSION");
