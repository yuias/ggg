//! Control protocol for the headless daemon.
//!
//! Separate from `ggg-ipc` (the `ggg-dnd` / bridge pipe protocol): the types
//! here carry ggg domain types (`DownloadTask`, `DownloadStatus`), and the
//! `ggg` binary is both the only client and the only server.
pub mod protocol;

// server.rs (Windows) and client.rs are added once the pipe transport
// itself is implemented.
