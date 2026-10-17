//! IPC protocol messages exchanged between ggg (TUI) and its IPC clients.
//!
//! Wire format: each message is a single JSON line terminated by `\n`.
use serde::{Deserialize, Serialize};

/// Default Named Pipe name
pub const DEFAULT_PIPE_NAME: &str = r"\\.\pipe\ggg";

/// Prefix for fallback pipe names (appended with the decimal pid)
pub const PIPE_NAME_PREFIX: &str = r"\\.\pipe\ggg-";

/// Set by tests and scripts to move the shared pipe off the real name.
pub const PIPE_ENV: &str = "GGG_PIPE";

const PIPE_DIR_PREFIX: &str = r"\\.\pipe\";
const MAX_PIPE_NAME_UTF16: usize = 256;

/// `GGG_PIPE` when it is a usable pipe name, else `DEFAULT_PIPE_NAME`.
pub fn default_pipe_name() -> String {
    pipe_name_from(std::env::var(PIPE_ENV).ok().as_deref())
}

/// Pure part of [`default_pipe_name`]: `Some(v)` is used only if it starts
/// with `\\.\pipe\` (ASCII case-insensitive), has a non-empty rest without
/// `\`, and is at most 256 UTF-16 units; anything else yields
/// `DEFAULT_PIPE_NAME`. The checks keep a stray value from pointing the
/// clients outside the pipe namespace.
pub fn pipe_name_from(env_value: Option<&str>) -> String {
    let Some(value) = env_value else {
        return DEFAULT_PIPE_NAME.to_string();
    };
    let prefix_len = PIPE_DIR_PREFIX.len();
    let has_prefix = value.len() >= prefix_len
        && value.as_bytes()[..prefix_len].eq_ignore_ascii_case(PIPE_DIR_PREFIX.as_bytes());
    // The prefix is ASCII, so a match means `prefix_len` is a char boundary.
    if !has_prefix {
        return DEFAULT_PIPE_NAME.to_string();
    }
    let rest = &value[prefix_len..];
    if rest.is_empty() || rest.contains('\\') || value.encode_utf16().count() > MAX_PIPE_NAME_UTF16
    {
        return DEFAULT_PIPE_NAME.to_string();
    }
    value.to_string()
}

/// Request sent from a client (ggg-dnd, ggg-bridge) to ggg.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcRequest {
    /// Add a URL to the current folder's download queue
    #[serde(rename = "add_url")]
    AddUrl { url: String },

    /// Connection health check
    #[serde(rename = "ping")]
    Ping,

    /// Bring the receiving GUI to the foreground and add `urls`
    #[serde(rename = "activate")]
    Activate {
        #[serde(default)]
        urls: Vec<String>,
    },
}

/// Response sent from ggg back to a client.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum IpcResponse {
    /// URL was accepted and queued
    #[serde(rename = "ok")]
    Ok { message: String },

    /// Request was rejected or an error occurred
    #[serde(rename = "error")]
    Error { message: String },

    /// Pong reply to a ping request
    #[serde(rename = "pong")]
    Pong,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn activate_round_trip_wire_format() {
        let req = IpcRequest::Activate {
            urls: vec!["http://127.0.0.1:1/a".to_string(), "http://127.0.0.1:1/b".to_string()],
        };
        let json = serde_json::to_string(&req).unwrap();
        assert_eq!(
            json,
            r#"{"type":"activate","urls":["http://127.0.0.1:1/a","http://127.0.0.1:1/b"]}"#
        );
        match serde_json::from_str::<IpcRequest>(&json).unwrap() {
            IpcRequest::Activate { urls } => assert_eq!(urls.len(), 2),
            other => panic!("expected Activate, got {other:?}"),
        }
    }

    #[test]
    fn activate_without_urls_parses_with_empty_list() {
        match serde_json::from_str::<IpcRequest>(r#"{"type":"activate"}"#).unwrap() {
            IpcRequest::Activate { urls } => assert!(urls.is_empty()),
            other => panic!("expected Activate, got {other:?}"),
        }
    }

    #[test]
    fn pipe_name_from_none_yields_default() {
        assert_eq!(pipe_name_from(None), DEFAULT_PIPE_NAME);
    }

    #[test]
    fn pipe_name_from_valid_override_is_used() {
        let name = r"\\.\pipe\ggg-test-1234";
        assert_eq!(pipe_name_from(Some(name)), name);
        let upper = r"\\.\PIPE\ggg-test";
        assert_eq!(pipe_name_from(Some(upper)), upper);
    }

    #[test]
    fn pipe_name_from_wrong_prefix_yields_default() {
        for v in [r"\\.\foo\x", r"\\server\pipe\x", "ggg", "", r"C:\pipe\x"] {
            assert_eq!(pipe_name_from(Some(v)), DEFAULT_PIPE_NAME, "{v:?}");
        }
        // Multibyte text shorter in chars than the prefix must not panic.
        assert_eq!(pipe_name_from(Some("あいうえお")), DEFAULT_PIPE_NAME);
    }

    #[test]
    fn pipe_name_from_empty_rest_yields_default() {
        assert_eq!(pipe_name_from(Some(r"\\.\pipe\")), DEFAULT_PIPE_NAME);
    }

    #[test]
    fn pipe_name_from_rest_with_backslash_yields_default() {
        assert_eq!(pipe_name_from(Some(r"\\.\pipe\a\b")), DEFAULT_PIPE_NAME);
    }

    #[test]
    fn pipe_name_from_overlong_name_yields_default() {
        let at_limit = format!(
            "{}{}",
            PIPE_DIR_PREFIX,
            "a".repeat(MAX_PIPE_NAME_UTF16 - PIPE_DIR_PREFIX.len())
        );
        assert_eq!(pipe_name_from(Some(&at_limit)), at_limit);
        let over = format!("{at_limit}a");
        assert_eq!(pipe_name_from(Some(&over)), DEFAULT_PIPE_NAME);
    }
}
