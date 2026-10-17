//! IPC re-exports and Windows Named Pipe server.
//!
//! Protocol types and the synchronous client live in the workspace crate
//! `ggg-ipc` so that all binaries (ggg, ggg-dnd, ggg-bridge) share the same
//! definitions. This module re-exports them under `crate::ipc::protocol::*`
//! for backwards compatibility with existing call sites.
pub mod protocol {
    pub use ggg_ipc::protocol::*;
}

#[cfg(windows)]
pub mod dnd_launch;
#[cfg(windows)]
pub mod pipe_server;

pub mod control;
