//! Shared IPC types and Named Pipe client for ggg.
//!
//! This crate is consumed by:
//! - `ggg` (the TUI app, server side)
//! - `ggg-dnd` (Windows drag-and-drop helper, client side)
//! - `ggg-bridge` (Chrome Native Messaging host, client side)
//!
//! The protocol types are cross-platform; the pipe client is Windows-only.

pub mod owner;
pub mod protocol;

#[cfg(windows)]
pub mod client;

pub use protocol::{IpcRequest, IpcResponse, DEFAULT_PIPE_NAME, PIPE_NAME_PREFIX};

#[cfg(windows)]
pub use client::{ping, send_request, send_url, ClientError};
