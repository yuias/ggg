//! Server-owner check for the daemon control pipe.
//!
//! The pipe name is derived from the config directory and is therefore
//! predictable. Another local user could create it first, keep the real
//! daemon from binding, answer `Pong`, and receive `add` payloads (including
//! `--header` values such as `Cookie` or `Authorization`). The pipe's default
//! DACL already denies other users write access to *our* pipe, so squatting
//! (someone else's process serving under our predictable name) is the only
//! attack that matters, and this check defends against exactly that: the
//! client verifies the connected pipe's server process runs as the same user
//! before sending anything.
use std::os::windows::io::AsRawHandle;

use tokio::net::windows::named_pipe::NamedPipeClient;
use windows::core::HRESULT;
use windows::Win32::Foundation::{CloseHandle, ERROR_INSUFFICIENT_BUFFER, HANDLE};
use windows::Win32::Security::{
    GetLengthSid, GetTokenInformation, IsValidSid, TokenUser, TOKEN_QUERY, TOKEN_USER,
};
use windows::Win32::System::Pipes::GetNamedPipeServerProcessId;
use windows::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_QUERY_LIMITED_INFORMATION,
};

/// Exact message for `DaemonProbe::Unreachable` and for `client::send` errors
/// on a mismatch.
pub const OWNER_MISMATCH_MSG: &str = "pipe owned by another user";

/// Closes a raw handle on drop, so every early-return path in this module
/// still releases it. Never wrap `GetCurrentProcess()`'s pseudo-handle in
/// this: it must not be closed.
struct HandleGuard(HANDLE);

impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_invalid() {
            // SAFETY: `self.0` is a handle this module opened (OpenProcess or
            // OpenProcessToken) and has not closed yet; closing it once, here,
            // is the intended cleanup for every return path.
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
}

/// Ok only if the pipe's server process token user SID equals the current
/// process token user SID. `Err(detail)` on mismatch or on any failed call;
/// callers log the detail at warn and surface [`OWNER_MISMATCH_MSG`] to the
/// user.
pub fn verify_server_owner(client: &NamedPipeClient) -> Result<(), String> {
    let pipe_handle = HANDLE(client.as_raw_handle());

    let mut server_pid: u32 = 0;
    // SAFETY: `pipe_handle` wraps a live pipe handle owned by `client`, which
    // outlives this call; `server_pid` is a valid out-param. We never close
    // this handle, it belongs to `client`.
    unsafe { GetNamedPipeServerProcessId(pipe_handle, &mut server_pid) }
        .map_err(|e| format!("GetNamedPipeServerProcessId: {e}"))?;
    if server_pid == 0 {
        return Err("GetNamedPipeServerProcessId: returned pid 0".to_string());
    }

    // SAFETY: PROCESS_QUERY_LIMITED_INFORMATION is a read-only access right,
    // `false` means the handle is not inheritable, and `server_pid` came from
    // the call above.
    let server_process = unsafe {
        OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, server_pid)
    }
    .map_err(|e| format!("OpenProcess(pid {server_pid}): {e}"))?;
    let _server_process_guard = HandleGuard(server_process);

    let server_sid = token_user_sid(server_process);
    // SAFETY: GetCurrentProcess returns a pseudo-handle that is always valid
    // and must never be closed.
    let own_process = unsafe { GetCurrentProcess() };
    let own_sid = token_user_sid(own_process);

    owner_decision(server_sid, own_sid)
}

/// Token user SID bytes of `process`.
fn token_user_sid(process: HANDLE) -> Result<Vec<u8>, String> {
    let mut token = HANDLE(std::ptr::null_mut());
    // SAFETY: `process` is a valid process handle (either a pseudo-handle or
    // one this module opened); `token` is a valid out-param.
    unsafe { OpenProcessToken(process, TOKEN_QUERY, &mut token) }
        .map_err(|e| format!("OpenProcessToken: {e}"))?;
    let _token_guard = HandleGuard(token);

    let mut len: u32 = 0;
    // SAFETY: passing `None` and length 0 is the documented way to query the
    // required buffer size; a well-behaved call fails with
    // `ERROR_INSUFFICIENT_BUFFER` and still writes the required size into
    // `len`. Any other outcome (success, or a different error) means `len`
    // cannot be trusted, so it is rejected here rather than used below.
    match unsafe { GetTokenInformation(token, TokenUser, None, 0, &mut len) } {
        Ok(()) => return Err("GetTokenInformation(size query): unexpectedly succeeded".to_string()),
        Err(e) if e.code() == HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0) => {}
        Err(e) => return Err(format!("GetTokenInformation(size query): {e}")),
    }
    if len == 0 {
        return Err("GetTokenInformation(size query): returned zero length".to_string());
    }

    // TOKEN_USER must be pointer-aligned; a Vec<u64> sized to cover `len`
    // bytes guarantees that regardless of how many bytes the SID itself needs.
    let word_count = (len as usize).div_ceil(8);
    let mut buf: Vec<u64> = vec![0u64; word_count];
    let ptr = buf.as_mut_ptr().cast::<core::ffi::c_void>();

    // SAFETY: `ptr` points into `buf`, which is at least `len` bytes long and
    // aligned for `TOKEN_USER`; `len` is the size `GetTokenInformation` itself
    // reported as required above.
    unsafe { GetTokenInformation(token, TokenUser, Some(ptr), len, &mut len) }
        .map_err(|e| format!("GetTokenInformation: {e}"))?;

    // SAFETY: the call above succeeded, so `buf` now holds a valid `TOKEN_USER`
    // at its start, and `buf` outlives this reference.
    let token_user = unsafe { &*ptr.cast::<TOKEN_USER>() };
    let sid = token_user.User.Sid;

    // TOKEN_USER.User.Sid points into `buf`. Confirm that before any API
    // dereferences it: first the fixed SID header, which IsValidSid and
    // GetLengthSid read, then the full reported length.
    const SID_HEADER_LEN: usize = 8;
    let buf_start = buf.as_ptr() as usize;
    let buf_end = buf_start + len as usize;
    let sid_start = sid.0 as usize;
    if sid_start < buf_start || sid_start.saturating_add(SID_HEADER_LEN) > buf_end {
        return Err("token user SID header lies outside the token information buffer".to_string());
    }

    // SAFETY: the SID header lies within `buf` (checked above), and
    // `IsValidSid` reads only the header before validating the sub-authority
    // count against it.
    if !unsafe { IsValidSid(sid) }.as_bool() {
        return Err("IsValidSid: SID is not well-formed".to_string());
    }

    // SAFETY: `sid` is a well-formed SID whose header lies within `buf`;
    // GetLengthSid computes the length from that header.
    let sid_len = unsafe { GetLengthSid(sid) };
    if sid_len == 0 {
        return Err("GetLengthSid: returned zero length".to_string());
    }

    let sid_end = sid_start
        .checked_add(sid_len as usize)
        .ok_or_else(|| "sid pointer + GetLengthSid overflowed".to_string())?;
    if sid_start < buf_start || sid_end > buf_end {
        return Err("token user SID lies outside the token information buffer".to_string());
    }

    // SAFETY: `sid` is valid for `sid_len` bytes, per GetLengthSid's own
    // report, checked above to be well-formed (IsValidSid) and to lie
    // entirely within `buf`'s byte range, and `buf` (which backs it) is
    // still alive here.
    let bytes = unsafe { std::slice::from_raw_parts(sid.0.cast::<u8>(), sid_len as usize) };
    Ok(bytes.to_vec())
}

/// Pure decision, unit-tested: `Ok` iff both are `Ok` and byte-equal.
/// `EqualSid` is deliberately not used: in `windows` 0.62 its "not equal"
/// result also comes back as `Err`, indistinguishable from a real failure.
fn owner_decision(server: Result<Vec<u8>, String>, own: Result<Vec<u8>, String>) -> Result<(), String> {
    match (server, own) {
        (Ok(server_sid), Ok(own_sid)) if server_sid == own_sid => Ok(()),
        (Ok(_), Ok(_)) => Err("server user SID differs".to_string()),
        (Err(e), Ok(_)) => Err(format!("server SID lookup failed: {e}")),
        (Ok(_), Err(e)) => Err(format!("own SID lookup failed: {e}")),
        (Err(se), Err(oe)) => Err(format!("server SID lookup failed: {se}; own SID lookup failed: {oe}")),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn owner_decision_equal_is_ok() {
        let sid = vec![1u8, 2, 3];
        assert_eq!(owner_decision(Ok(sid.clone()), Ok(sid)), Ok(()));
    }

    #[test]
    fn owner_decision_different_is_err() {
        assert!(owner_decision(Ok(vec![1, 2, 3]), Ok(vec![1, 2, 4])).is_err());
    }

    #[test]
    fn owner_decision_either_side_error_is_err() {
        assert!(owner_decision(Err("boom".to_string()), Ok(vec![1])).is_err());
        assert!(owner_decision(Ok(vec![1]), Err("boom".to_string())).is_err());
        assert!(owner_decision(Err("a".to_string()), Err("b".to_string())).is_err());
    }

    #[test]
    fn token_user_sid_of_current_process_is_stable() {
        // SAFETY: GetCurrentProcess is a pseudo-handle, always valid, never closed.
        let process = unsafe { GetCurrentProcess() };
        let first = token_user_sid(process).expect("first token_user_sid call");
        let second = token_user_sid(process).expect("second token_user_sid call");
        assert!(!first.is_empty());
        assert_eq!(first, second);
    }
}
