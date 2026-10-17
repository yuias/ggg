//! Control protocol for the headless daemon.
//!
//! Separate from `ggg-ipc` (the link pipe protocol): the types
//! here carry ggg domain types (`DownloadTask`, `DownloadStatus`), and the
//! `ggg` binary is both the only client and the only server.
pub mod protocol;

#[cfg(windows)]
pub mod server;
#[cfg(windows)]
pub mod client;
#[cfg(windows)]
pub mod owner;
