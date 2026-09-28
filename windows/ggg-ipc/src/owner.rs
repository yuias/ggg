//! Owner-SID trust check for a Named Pipe handle.
//!
//! A pipe name under a well-known namespace (`\\.\pipe\...`) is predictable,
//! so another local user could create it first and squat: answer requests,
//! or read what a client sends it. The default DACL on a pipe already denies
//! other users write access to *our* pipe, so the remaining question is
//! whether the pipe we opened is in fact ours. The owner of a kernel object
//! can only be set to a SID in the creator's own token, or to any SID with
//! `SeRestorePrivilege`, so a non-admin squatter cannot make a pipe that
//! appears owned by the victim. Checking the pipe's own owner SID therefore
//! establishes trust without depending on the server's PID (which handle
//! inheritance and PID reuse can make misleading) and without opening the
//! server process's token (which is denied across an elevation boundary).
//!
//! The pure decision (`decide_owner`) and its verdict/messages are compiled
//! on every target so callers and tests do not need a `cfg(windows)` guard.
//! The Win32 calls that gather the SIDs are `cfg(windows)`.

/// S-1-5-32-544 (BUILTIN\Administrators) in binary form.
pub const ADMINISTRATORS_SID: [u8; 16] =
    [1, 2, 0, 0, 0, 0, 0, 5, 0x20, 0, 0, 0, 0x20, 0x02, 0, 0];

/// Result of comparing a pipe's owner SID against the caller's own SIDs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OwnerVerdict {
    /// Owner equals the caller's TokenUser SID or the caller's TokenOwner SID.
    Trusted,
    /// Owner is BUILTIN\Administrators and did not match the caller:
    /// the server runs elevated and the caller does not.
    ElevatedServer,
    /// Any other owner, or no owner at all.
    OtherOwner,
}

/// Pure decision: is `pipe_owner` one of the caller's own SIDs?
///
/// `own_user` is the caller's TokenUser SID; `own_default_owner` is the
/// caller's TokenOwner SID (the default owner an elevated token would stamp
/// on objects it creates, which is BUILTIN\Administrators for an elevated
/// admin token).
pub fn decide_owner(
    pipe_owner: Option<&[u8]>,
    own_user: &[u8],
    own_default_owner: &[u8],
) -> OwnerVerdict {
    match pipe_owner {
        Some(owner) if owner == own_user || owner == own_default_owner => OwnerVerdict::Trusted,
        Some(owner) if owner == ADMINISTRATORS_SID => OwnerVerdict::ElevatedServer,
        _ => OwnerVerdict::OtherOwner,
    }
}

/// Exact message for an `OtherOwner` verdict.
pub const OTHER_OWNER_MSG: &str = "pipe owned by another user";
/// Exact message for an `ElevatedServer` verdict.
pub const ELEVATED_SERVER_MSG: &str =
    "pipe is owned by Administrators: the server is running elevated and this process is not; run both elevated or both not elevated";

/// Errors from [`verify_pipe_owner`].
#[derive(Debug)]
pub enum OwnerCheckError {
    /// The owner check ran to completion but did not trust the pipe.
    Untrusted(OwnerVerdict),
    /// A Win32 call failed; the string names the call and the OS error.
    Failed(String),
}

impl OwnerCheckError {
    /// Message safe to show a user; the Win32 detail in `Failed` is for logs.
    pub fn user_message(&self) -> String {
        match self {
            OwnerCheckError::Untrusted(OwnerVerdict::ElevatedServer) => {
                ELEVATED_SERVER_MSG.to_string()
            }
            OwnerCheckError::Untrusted(_) => OTHER_OWNER_MSG.to_string(),
            OwnerCheckError::Failed(detail) => format!("could not verify the pipe owner: {detail}"),
        }
    }
}

#[cfg(windows)]
mod win32 {
    use super::{decide_owner, OwnerCheckError, OwnerVerdict};

    type Handle = *mut core::ffi::c_void;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn OpenProcessToken(process: Handle, access: u32, token: *mut Handle) -> i32;
        fn GetTokenInformation(
            token: Handle,
            class: i32,
            info: *mut core::ffi::c_void,
            len: u32,
            ret: *mut u32,
        ) -> i32;
        fn GetKernelObjectSecurity(
            h: Handle,
            info: u32,
            sd: *mut core::ffi::c_void,
            len: u32,
            needed: *mut u32,
        ) -> i32;
        fn GetSecurityDescriptorOwner(
            sd: *mut core::ffi::c_void,
            owner: *mut *mut core::ffi::c_void,
            defaulted: *mut i32,
        ) -> i32;
        fn IsValidSid(sid: *mut core::ffi::c_void) -> i32;
        fn GetLengthSid(sid: *mut core::ffi::c_void) -> u32;
    }
    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GetCurrentProcess() -> Handle;
        fn CloseHandle(h: Handle) -> i32;
        fn GetLastError() -> u32;
    }

    const TOKEN_QUERY: u32 = 0x0008;
    const TOKEN_USER_CLASS: i32 = 1; // TokenUser
    const TOKEN_OWNER_CLASS: i32 = 4; // TokenOwner
    const OWNER_SECURITY_INFORMATION: u32 = 0x1;
    const ERROR_INSUFFICIENT_BUFFER: u32 = 122;
    /// Fixed part of a SID that `IsValidSid`/`GetLengthSid` read before
    /// trusting the sub-authority count, used to bounds-check a SID pointer
    /// before those calls dereference it.
    const SID_HEADER_LEN: usize = 8;

    /// Closes a token handle on drop, so every early-return path releases it.
    /// Never wrap `GetCurrentProcess()`'s pseudo-handle in this: it must not
    /// be closed.
    struct HandleGuard(Handle);

    impl Drop for HandleGuard {
        fn drop(&mut self) {
            if !self.0.is_null() {
                // SAFETY: `self.0` is a handle this module opened
                // (OpenProcessToken) and has not closed yet; closing it once,
                // here, is the intended cleanup for every return path.
                unsafe {
                    CloseHandle(self.0);
                }
            }
        }
    }

    /// Ok only if the pipe's owner SID is one of the caller's own SIDs.
    /// `pipe` must be a live handle to the pipe instance; it is never closed
    /// here, it belongs to the caller.
    pub fn verify_pipe_owner(pipe: std::os::windows::io::RawHandle) -> Result<(), OwnerCheckError> {
        let pipe_handle = pipe as Handle;

        // SAFETY: GetCurrentProcess returns a pseudo-handle that is always
        // valid and must never be closed.
        let current_process = unsafe { GetCurrentProcess() };
        let mut token: Handle = core::ptr::null_mut();
        // SAFETY: `current_process` is a valid pseudo-handle; `token` is a
        // valid out-param.
        let ok = unsafe { OpenProcessToken(current_process, TOKEN_QUERY, &mut token) };
        if ok == 0 {
            // SAFETY: GetLastError reads the code set by the call above on
            // this thread.
            let err = unsafe { GetLastError() };
            return Err(OwnerCheckError::Failed(format!(
                "OpenProcessToken: os error {err}"
            )));
        }
        let _token_guard = HandleGuard(token);

        let own_user =
            token_sid(token, TOKEN_USER_CLASS).map_err(OwnerCheckError::Failed)?;
        let own_default_owner =
            token_sid(token, TOKEN_OWNER_CLASS).map_err(OwnerCheckError::Failed)?;
        let pipe_owner = pipe_security_owner(pipe_handle).map_err(OwnerCheckError::Failed)?;

        match decide_owner(pipe_owner.as_deref(), &own_user, &own_default_owner) {
            OwnerVerdict::Trusted => Ok(()),
            verdict => Err(OwnerCheckError::Untrusted(verdict)),
        }
    }

    /// SID bytes of a `TOKEN_USER`/`TOKEN_OWNER`-class token information.
    /// Both structs start with a SID pointer field, so one helper covers
    /// both classes.
    fn token_sid(token: Handle, class: i32) -> Result<Vec<u8>, String> {
        let mut len: u32 = 0;
        // SAFETY: a null buffer and length 0 is the documented way to query
        // the required size; a well-behaved call fails with
        // ERROR_INSUFFICIENT_BUFFER and still writes the required size into
        // `len`. Any other outcome means `len` cannot be trusted, so it is
        // rejected below rather than used.
        let ok = unsafe { GetTokenInformation(token, class, core::ptr::null_mut(), 0, &mut len) };
        if ok != 0 {
            return Err("GetTokenInformation(size query): unexpectedly succeeded".to_string());
        }
        // SAFETY: GetLastError reads the code set by the call above.
        let err = unsafe { GetLastError() };
        if err != ERROR_INSUFFICIENT_BUFFER {
            return Err(format!("GetTokenInformation(size query): os error {err}"));
        }
        if len == 0 {
            return Err("GetTokenInformation(size query): returned zero length".to_string());
        }

        // The struct must be pointer-aligned; a Vec<u64> sized to cover `len`
        // bytes guarantees that regardless of how many bytes the SID itself
        // needs.
        let word_count = (len as usize).div_ceil(8);
        let mut buf: Vec<u64> = vec![0u64; word_count];
        let ptr = buf.as_mut_ptr().cast::<core::ffi::c_void>();
        let mut out_len = len;

        // SAFETY: `ptr` points into `buf`, which is at least `len` bytes long
        // and aligned; `len` is the size the call above reported as required.
        let ok = unsafe { GetTokenInformation(token, class, ptr, len, &mut out_len) };
        if ok == 0 {
            // SAFETY: GetLastError reads the code set by the call above.
            let err = unsafe { GetLastError() };
            return Err(format!("GetTokenInformation: os error {err}"));
        }

        // SAFETY: the call above succeeded, so `buf` now holds a valid
        // TOKEN_USER/TOKEN_OWNER at its start; both begin with a SID pointer
        // field, which is what this reads.
        let sid_ptr = unsafe { *ptr.cast::<*mut core::ffi::c_void>() };
        extract_sid(sid_ptr, buf.as_ptr() as usize, len as usize, "token")
    }

    /// Owner SID bytes of the security descriptor for kernel object `h`, or
    /// `None` if it has no owner.
    fn pipe_security_owner(h: Handle) -> Result<Option<Vec<u8>>, String> {
        let mut needed: u32 = 0;
        // SAFETY: a null buffer and length 0 is the documented way to query
        // the required size for the security descriptor.
        let ok = unsafe {
            GetKernelObjectSecurity(h, OWNER_SECURITY_INFORMATION, core::ptr::null_mut(), 0, &mut needed)
        };
        if ok != 0 {
            return Err("GetKernelObjectSecurity(size query): unexpectedly succeeded".to_string());
        }
        // SAFETY: GetLastError reads the code set by the call above.
        let err = unsafe { GetLastError() };
        if err != ERROR_INSUFFICIENT_BUFFER {
            return Err(format!("GetKernelObjectSecurity(size query): os error {err}"));
        }
        if needed == 0 {
            return Err("GetKernelObjectSecurity(size query): returned zero length".to_string());
        }

        let word_count = (needed as usize).div_ceil(8);
        let mut buf: Vec<u64> = vec![0u64; word_count];
        let ptr = buf.as_mut_ptr().cast::<core::ffi::c_void>();
        let mut out_len = needed;

        // SAFETY: `ptr` points into `buf`, sized to `needed` bytes as
        // reported above.
        let ok = unsafe {
            GetKernelObjectSecurity(h, OWNER_SECURITY_INFORMATION, ptr, needed, &mut out_len)
        };
        if ok == 0 {
            // SAFETY: GetLastError reads the code set by the call above.
            let err = unsafe { GetLastError() };
            return Err(format!("GetKernelObjectSecurity: os error {err}"));
        }

        let mut owner: *mut core::ffi::c_void = core::ptr::null_mut();
        let mut defaulted: i32 = 0;
        // SAFETY: `ptr` holds a security descriptor GetKernelObjectSecurity
        // just filled in above; `owner`/`defaulted` are valid out-params.
        let ok = unsafe { GetSecurityDescriptorOwner(ptr, &mut owner, &mut defaulted) };
        if ok == 0 {
            // SAFETY: GetLastError reads the code set by the call above.
            let err = unsafe { GetLastError() };
            return Err(format!("GetSecurityDescriptorOwner: os error {err}"));
        }
        if owner.is_null() {
            return Ok(None);
        }

        extract_sid(owner, buf.as_ptr() as usize, needed as usize, "security descriptor")
            .map(Some)
    }

    /// Validates that `sid` lies inside `[buf_start, buf_start + buf_len)`,
    /// then copies its bytes out. `what` names the buffer for error text.
    fn extract_sid(
        sid: *mut core::ffi::c_void,
        buf_start: usize,
        buf_len: usize,
        what: &str,
    ) -> Result<Vec<u8>, String> {
        let buf_end = buf_start + buf_len;
        let sid_start = sid as usize;
        if sid_start < buf_start || sid_start.saturating_add(SID_HEADER_LEN) > buf_end {
            return Err(format!("SID header lies outside the {what} buffer"));
        }

        // SAFETY: the SID header lies within the buffer (checked above), and
        // IsValidSid reads only the header before validating the
        // sub-authority count against it.
        if unsafe { IsValidSid(sid) } == 0 {
            return Err("IsValidSid: SID is not well-formed".to_string());
        }

        // SAFETY: `sid` is a well-formed SID whose header lies within the
        // buffer; GetLengthSid computes the length from that header.
        let sid_len = unsafe { GetLengthSid(sid) };
        if sid_len == 0 {
            return Err("GetLengthSid: returned zero length".to_string());
        }

        let sid_end = sid_start
            .checked_add(sid_len as usize)
            .ok_or_else(|| "sid pointer + GetLengthSid overflowed".to_string())?;
        if sid_start < buf_start || sid_end > buf_end {
            return Err(format!("SID lies outside the {what} buffer"));
        }

        // SAFETY: `sid` is valid for `sid_len` bytes per GetLengthSid's own
        // report, checked above to be well-formed (IsValidSid) and to lie
        // entirely within the buffer's byte range, and the buffer backing it
        // is still alive at the call site (the local `buf` it was built
        // from has not been dropped yet).
        let bytes = unsafe { std::slice::from_raw_parts(sid.cast::<u8>(), sid_len as usize) };
        Ok(bytes.to_vec())
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn administrators_sid_constant_is_valid() {
            let ptr = super::super::ADMINISTRATORS_SID.as_ptr() as *mut core::ffi::c_void;
            // SAFETY: ADMINISTRATORS_SID is a well-formed 16-byte SID literal,
            // alive for the duration of these calls.
            assert_ne!(unsafe { IsValidSid(ptr) }, 0);
            // SAFETY: same as above.
            assert_eq!(unsafe { GetLengthSid(ptr) }, 16);
        }

        #[test]
        fn own_token_sids_are_stable() {
            // SAFETY: GetCurrentProcess is a pseudo-handle, always valid.
            let current_process = unsafe { GetCurrentProcess() };
            let mut token: Handle = core::ptr::null_mut();
            // SAFETY: `current_process` is a valid pseudo-handle; `token` is
            // a valid out-param.
            let ok = unsafe { OpenProcessToken(current_process, TOKEN_QUERY, &mut token) };
            assert_ne!(ok, 0, "OpenProcessToken failed");
            let _guard = HandleGuard(token);

            let user_first = token_sid(token, TOKEN_USER_CLASS).expect("token user 1");
            let user_second = token_sid(token, TOKEN_USER_CLASS).expect("token user 2");
            assert!(!user_first.is_empty());
            assert_eq!(user_first, user_second);

            let owner_first = token_sid(token, TOKEN_OWNER_CLASS).expect("token owner 1");
            let owner_second = token_sid(token, TOKEN_OWNER_CLASS).expect("token owner 2");
            assert!(!owner_first.is_empty());
            assert_eq!(owner_first, owner_second);
        }

        /// Exercises the security-descriptor path (`GetKernelObjectSecurity`
        /// / `GetSecurityDescriptorOwner`) end to end, which the pipe-owner
        /// acceptance tests otherwise leave uncovered at this layer: the
        /// current process object's owner is always its own creating
        /// token's TokenOwner SID, so this is `Trusted` whether or not the
        /// test runs elevated.
        #[test]
        fn own_process_handle_is_trusted() {
            // SAFETY: GetCurrentProcess is a pseudo-handle, always valid,
            // and `verify_pipe_owner` never closes the handle it is given.
            let current_process = unsafe { GetCurrentProcess() };
            let result = verify_pipe_owner(current_process as std::os::windows::io::RawHandle);
            assert!(result.is_ok(), "expected own process handle to be trusted: {result:?}");
        }
    }
}

#[cfg(windows)]
pub use win32::verify_pipe_owner;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decide_owner_matches_token_user() {
        let user = vec![1, 2, 3];
        let default_owner = vec![9, 9, 9];
        assert_eq!(
            decide_owner(Some(&user), &user, &default_owner),
            OwnerVerdict::Trusted
        );
    }

    #[test]
    fn decide_owner_matches_token_owner() {
        let user = vec![1, 2, 3];
        let default_owner = vec![9, 9, 9];
        assert_eq!(
            decide_owner(Some(&default_owner), &user, &default_owner),
            OwnerVerdict::Trusted
        );
    }

    #[test]
    fn decide_owner_administrators_unmatched_is_elevated_server() {
        let user = vec![1, 2, 3];
        let default_owner = vec![9, 9, 9];
        assert_eq!(
            decide_owner(Some(&ADMINISTRATORS_SID), &user, &default_owner),
            OwnerVerdict::ElevatedServer
        );
    }

    #[test]
    fn decide_owner_administrators_matched_by_token_owner_is_trusted() {
        let user = vec![1, 2, 3];
        let default_owner = ADMINISTRATORS_SID.to_vec();
        assert_eq!(
            decide_owner(Some(&ADMINISTRATORS_SID), &user, &default_owner),
            OwnerVerdict::Trusted
        );
    }

    #[test]
    fn decide_owner_none_is_other() {
        let user = vec![1, 2, 3];
        let default_owner = vec![9, 9, 9];
        assert_eq!(decide_owner(None, &user, &default_owner), OwnerVerdict::OtherOwner);
    }

    #[test]
    fn decide_owner_other_sid_is_other() {
        let user = vec![1, 2, 3];
        let default_owner = vec![9, 9, 9];
        let other = vec![5, 5, 5];
        assert_eq!(
            decide_owner(Some(&other), &user, &default_owner),
            OwnerVerdict::OtherOwner
        );
    }
}
