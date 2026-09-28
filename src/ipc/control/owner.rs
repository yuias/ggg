//! Server-owner check for the daemon control pipe.
//!
//! The pipe name is derived from the config directory and is therefore
//! predictable. Another local user could create it first, keep the real
//! daemon from binding, answer `Pong`, and receive `add` payloads (including
//! `--header` values such as `Cookie` or `Authorization`). The pipe's default
//! DACL already denies other users write access to *our* pipe, so squatting
//! (someone else's process serving under our predictable name) is the only
//! attack that matters, and this check defends against exactly that: the
//! client verifies the connected pipe's owner SID is one of its own SIDs
//! before sending anything. The owner SID lives on the pipe object itself,
//! set at creation time by the kernel to the creator's token, so it needs no
//! access to the server process (which handle inheritance and PID reuse can
//! make misleading, and which is denied entirely across an elevation
//! boundary). See `ggg_ipc::owner` for the FFI and the trust rule.
use std::os::windows::io::AsRawHandle;

use tokio::net::windows::named_pipe::NamedPipeClient;

/// Exact message for `DaemonProbe::Unreachable` and for `client::send` errors
/// when the pipe is owned by another user (as opposed to an elevation
/// mismatch, which gets its own message).
pub const OWNER_MISMATCH_MSG: &str = ggg_ipc::owner::OTHER_OWNER_MSG;

/// Ok only if the pipe's owner SID is one of the current process's own SIDs
/// (see `ggg_ipc::owner::decide_owner`). `Err(message)` is safe to show the
/// user; callers also log the underlying detail at warn.
pub fn check_server_owner(client: &NamedPipeClient) -> Result<(), String> {
    ggg_ipc::owner::verify_pipe_owner(client.as_raw_handle()).map_err(|e| {
        tracing::warn!("control pipe owner check failed: {e:?}");
        e.user_message()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_mismatch_msg_text_unchanged() {
        assert_eq!(OWNER_MISMATCH_MSG, "pipe owned by another user");
    }
}
