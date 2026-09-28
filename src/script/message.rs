/// Message passing infrastructure for script execution across thread boundaries
///
/// Since `deno_core::JsRuntime` (V8) is !Send, we cannot share ScriptManager
/// across threads. Instead, we run a dedicated script executor thread and
/// communicate via channels.

use super::error::ScriptResult;
use super::events::*;
use std::sync::mpsc;

/// Request to execute a script hook
///
/// Sent from download tasks to the script executor thread
pub enum ScriptRequest {
    /// Execute beforeRequest hook
    ///
    /// Modifies context in-place, returns modified context
    BeforeRequest {
        ctx: BeforeRequestContext,
        effective_script_files: std::collections::HashMap<String, bool>,
        response: mpsc::Sender<(BeforeRequestContext, ScriptResult<()>)>,
    },

    /// Execute headersReceived hook
    ///
    /// Read-only inspection of server response
    HeadersReceived {
        ctx: HeadersReceivedContext,
        effective_script_files: std::collections::HashMap<String, bool>,
        response: mpsc::Sender<ScriptResult<()>>,
    },

    /// Execute completed hook
    ///
    /// Modifies context in-place for file operations, returns modified context
    Completed {
        ctx: CompletedContext,
        effective_script_files: std::collections::HashMap<String, bool>,
        response: mpsc::Sender<(CompletedContext, ScriptResult<()>)>,
    },

    /// Execute error hook (fire-and-forget)
    ///
    /// No response expected
    Error {
        ctx: ErrorContext,
        effective_script_files: std::collections::HashMap<String, bool>,
    },

    /// Execute progress hook (fire-and-forget)
    ///
    /// No response expected
    Progress {
        ctx: ProgressContext,
        effective_script_files: std::collections::HashMap<String, bool>,
    },

    /// Execute authRequired hook
    ///
    /// Modifies context in-place for authentication, returns modified context
    AuthRequired {
        ctx: AuthRequiredContext,
        effective_script_files: std::collections::HashMap<String, bool>,
        response: mpsc::Sender<(AuthRequiredContext, ScriptResult<()>)>,
    },

    /// Reload all scripts from disk
    ///
    /// Returns success/failure result
    Reload {
        response: mpsc::Sender<ScriptResult<()>>,
    },

    /// Barrier: the executor replies once every request queued before this
    /// one has been processed. Used to wait for fire-and-forget hooks (e.g.
    /// the error hook) without adding a response channel to each of them.
    Flush {
        response: mpsc::Sender<()>,
    },
}

impl ScriptRequest {
    /// The event kind, with no context: every `ctx` variant carries task
    /// headers (`BeforeRequestContext`, etc.), which may include a secret
    /// such as a `Cookie` or `Authorization` value, so logging must never
    /// format the context itself.
    pub(crate) fn kind(&self) -> &'static str {
        match self {
            Self::BeforeRequest { .. } => "BeforeRequest",
            Self::HeadersReceived { .. } => "HeadersReceived",
            Self::Completed { .. } => "Completed",
            Self::Error { .. } => "Error",
            Self::Progress { .. } => "Progress",
            Self::AuthRequired { .. } => "AuthRequired",
            Self::Reload { .. } => "Reload",
            Self::Flush { .. } => "Flush",
        }
    }
}

impl std::fmt::Debug for ScriptRequest {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "ScriptRequest::{}", self.kind())
    }
}
