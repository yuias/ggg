//! Wire types for the daemon control pipe.
//!
//! This module is cross-platform and pure (no `#[cfg(windows)]`): the types
//! and the pipe-name derivation are useful on every target, even though the
//! actual named-pipe transport (`server`/`client`) is Windows-only.

use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::download::task::{DownloadStatus, DownloadTask};

pub const CONTROL_PROTOCOL_VERSION: u32 = 1;
pub const CONTROL_PIPE_PREFIX: &str = r"\\.\pipe\ggg-daemon-";
/// Env override so tests and scripts can pin the pipe name.
pub const CONTROL_PIPE_ENV: &str = "GGG_DAEMON_PIPE";

/// FNV-1a, 64-bit variant (offset basis `0xcbf29ce484222325`, prime
/// `0x100000001b3`).
fn fnv1a64(data: &str) -> u64 {
    let mut hash: u64 = 0xcbf29ce484222325;
    for byte in data.as_bytes() {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn hashed_pipe_name(normalized: &str) -> String {
    format!("{CONTROL_PIPE_PREFIX}{:016x}", fnv1a64(normalized))
}

/// `CONTROL_PIPE_PREFIX` + 16 lowercase hex chars of FNV-1a 64 over the
/// config dir string (made absolute with `std::path::absolute`, lowercased,
/// trailing `\`/`/` trimmed). The env var, if set and non-empty, wins.
pub fn control_pipe_name(config_dir: &std::path::Path) -> String {
    if let Ok(name) = std::env::var(CONTROL_PIPE_ENV) {
        if !name.is_empty() {
            return name;
        }
    }
    let absolute = std::path::absolute(config_dir).unwrap_or_else(|_| config_dir.to_path_buf());
    let normalized = absolute.to_string_lossy().to_lowercase();
    let normalized = normalized.trim_end_matches(['\\', '/']);
    hashed_pipe_name(normalized)
}

/// Resolves the dir via `crate::util::paths::find_config_directory()`; on
/// error hashes the literal "default".
pub fn default_control_pipe_name() -> String {
    match crate::util::paths::find_config_directory() {
        Ok(dir) => control_pipe_name(&dir),
        Err(_) => {
            if let Ok(name) = std::env::var(CONTROL_PIPE_ENV) {
                if !name.is_empty() {
                    return name;
                }
            }
            hashed_pipe_name("default")
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlRequest {
    Ping,
    Status,
    Shutdown,
    Add {
        url: String,
        folder: Option<String>,
        referrer: Option<String>,
        headers: Vec<(String, String)>,
        immediate: bool,
    },
    List {
        statuses: Vec<DownloadStatus>,
        folder: Option<String>,
    },
    Get {
        id: Uuid,
    },
    Start {
        id: Uuid,
    },
    Pause {
        id: Uuid,
    },
    Remove {
        id: Uuid,
    },
    StartAll {
        folder: Option<String>,
    },
    PauseAll {
        folder: Option<String>,
    },
    Modify {
        id: Uuid,
        edit: HeaderEdit,
    },
    Wait {
        id: Uuid,
        timeout_secs: Option<u64>,
    },
    ReloadScripts,
    SetScriptEnabled {
        name: String,
        enabled: bool,
    },
    Move {
        id: Uuid,
        op: MoveOp,
    },
    Priority {
        id: Uuid,
        priority: u8,
    },
    ConfigGet {
        key: String,
    },
    ConfigSet {
        key: String,
        value: String,
    },
}

impl ControlRequest {
    /// True if the request can change queue state; the daemon saves the
    /// queue (`save_queue_to_folders`) after answering such requests.
    pub fn is_mutating(&self) -> bool {
        matches!(
            self,
            ControlRequest::Add { .. }
                | ControlRequest::Start { .. }
                | ControlRequest::Pause { .. }
                | ControlRequest::Remove { .. }
                | ControlRequest::StartAll { .. }
                | ControlRequest::PauseAll { .. }
                | ControlRequest::Modify { .. }
                | ControlRequest::Move { .. }
                | ControlRequest::Priority { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
pub struct HeaderEdit {
    pub referrer: Option<String>, // set/replace Referer
    pub clear_referrer: bool,
    pub set_headers: Vec<(String, String)>,
    pub remove_headers: Vec<String>, // case-insensitive
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "snake_case")]
pub enum MoveOp {
    ToTop,
    ToBottom,
    Before { id: Uuid },
    Folder { folder: String },
}

// No PartialEq: DownloadTask does not derive it.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlResponse {
    Pong {
        protocol: u32,
        pid: u32,
        version: String,
    },
    Status {
        status: DaemonStatus,
    },
    Ok {
        message: String,
    },
    Added {
        added: Vec<AddedTask>,
        skipped: Vec<SkippedUrl>,
    },
    Tasks {
        tasks: Vec<DownloadTask>,
    },
    Task {
        task: DownloadTask,
        in_history: bool,
    },
    Waited {
        outcome: WaitOutcome,
        task: Option<DownloadTask>,
    },
    Value {
        value: String,
    },
    Error {
        code: ControlErrorCode,
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AddedTask {
    pub id: Uuid,
    pub url: String,
    pub folder_id: String,
    pub started: bool,
    pub start_error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkippedUrl {
    pub url: String,
    pub reason: String, // "invalid_url" | "duplicate"
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DaemonStatus {
    pub pid: u32,
    pub version: String,
    pub protocol: u32,
    pub uptime_secs: u64,
    pub pipe_name: String,
    pub config_dir: String,
    pub scripts_active: bool,
    pub pending: usize,
    pub downloading: usize,
    pub paused: usize,
    pub error: usize,
    pub history: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WaitOutcome {
    Completed,
    Failed,
    Paused,
    Removed,
    Timeout,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlErrorCode {
    NotFound,
    InvalidInput,
    Unsupported,
    Internal,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serial_test::serial;

    #[test]
    fn ping_wire_format() {
        let json = serde_json::to_string(&ControlRequest::Ping).unwrap();
        assert_eq!(json, r#"{"type":"ping"}"#);
        let back: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(back, ControlRequest::Ping);
    }

    #[test]
    fn add_request_round_trip() {
        let req = ControlRequest::Add {
            url: "https://example.com/file".to_string(),
            folder: Some("default".to_string()),
            referrer: Some("https://example.org/".to_string()),
            headers: vec![("X-Test".to_string(), "1".to_string())],
            immediate: true,
        };
        let json = serde_json::to_string(&req).unwrap();
        let back: ControlRequest = serde_json::from_str(&json).unwrap();
        assert_eq!(req, back);
    }

    fn all_request_variants() -> Vec<ControlRequest> {
        vec![
            ControlRequest::Ping,
            ControlRequest::Status,
            ControlRequest::Shutdown,
            ControlRequest::Add {
                url: "https://example.com/a".to_string(),
                folder: None,
                referrer: None,
                headers: vec![],
                immediate: false,
            },
            ControlRequest::List {
                statuses: vec![DownloadStatus::Pending, DownloadStatus::Error],
                folder: Some("default".to_string()),
            },
            ControlRequest::Get { id: Uuid::nil() },
            ControlRequest::Start { id: Uuid::nil() },
            ControlRequest::Pause { id: Uuid::nil() },
            ControlRequest::Remove { id: Uuid::nil() },
            ControlRequest::StartAll { folder: None },
            ControlRequest::PauseAll {
                folder: Some("default".to_string()),
            },
            ControlRequest::Modify {
                id: Uuid::nil(),
                edit: HeaderEdit {
                    referrer: Some("https://example.org/".to_string()),
                    clear_referrer: false,
                    set_headers: vec![("X-Test".to_string(), "1".to_string())],
                    remove_headers: vec!["X-Old".to_string()],
                },
            },
            ControlRequest::Wait {
                id: Uuid::nil(),
                timeout_secs: Some(30),
            },
            ControlRequest::ReloadScripts,
            ControlRequest::SetScriptEnabled {
                name: "my-script".to_string(),
                enabled: true,
            },
            ControlRequest::Move {
                id: Uuid::nil(),
                op: MoveOp::ToTop,
            },
            ControlRequest::Move {
                id: Uuid::nil(),
                op: MoveOp::Before { id: Uuid::nil() },
            },
            ControlRequest::Move {
                id: Uuid::nil(),
                op: MoveOp::Folder {
                    folder: "default".to_string(),
                },
            },
            ControlRequest::Priority {
                id: Uuid::nil(),
                priority: 5,
            },
            ControlRequest::ConfigGet {
                key: "some.key".to_string(),
            },
            ControlRequest::ConfigSet {
                key: "some.key".to_string(),
                value: "some value".to_string(),
            },
        ]
    }

    #[test]
    fn every_request_variant_round_trips() {
        for req in all_request_variants() {
            let json = serde_json::to_string(&req).unwrap();
            let back: ControlRequest = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("failed to round-trip {json}: {e}"));
            assert_eq!(req, back, "mismatch for {json}");
        }
    }

    fn sample_task() -> DownloadTask {
        DownloadTask::new(
            "https://example.com/file".to_string(),
            std::path::PathBuf::from("/tmp/file"),
        )
    }

    fn all_response_variants() -> Vec<ControlResponse> {
        vec![
            ControlResponse::Pong {
                protocol: CONTROL_PROTOCOL_VERSION,
                pid: 1234,
                version: "0.4.3".to_string(),
            },
            ControlResponse::Status {
                status: DaemonStatus {
                    pid: 1234,
                    version: "0.4.3".to_string(),
                    protocol: CONTROL_PROTOCOL_VERSION,
                    uptime_secs: 60,
                    pipe_name: r"\\.\pipe\ggg-daemon-abc".to_string(),
                    config_dir: "C:/config".to_string(),
                    scripts_active: true,
                    pending: 1,
                    downloading: 2,
                    paused: 3,
                    error: 4,
                    history: 5,
                },
            },
            ControlResponse::Ok {
                message: "done".to_string(),
            },
            ControlResponse::Added {
                added: vec![AddedTask {
                    id: Uuid::nil(),
                    url: "https://example.com/file".to_string(),
                    folder_id: "default".to_string(),
                    started: true,
                    start_error: None,
                }],
                skipped: vec![SkippedUrl {
                    url: "not a url".to_string(),
                    reason: "invalid_url".to_string(),
                }],
            },
            ControlResponse::Tasks {
                tasks: vec![sample_task()],
            },
            ControlResponse::Task {
                task: sample_task(),
                in_history: true,
            },
            ControlResponse::Waited {
                outcome: WaitOutcome::Completed,
                task: Some(sample_task()),
            },
            ControlResponse::Waited {
                outcome: WaitOutcome::Timeout,
                task: None,
            },
            ControlResponse::Value {
                value: "some value".to_string(),
            },
            ControlResponse::Error {
                code: ControlErrorCode::NotFound,
                message: "no such task".to_string(),
            },
        ]
    }

    #[test]
    fn every_response_variant_round_trips() {
        // ControlResponse carries DownloadTask, which has no PartialEq, so
        // compare re-serialized JSON instead of the values themselves.
        for resp in all_response_variants() {
            let json = serde_json::to_string(&resp).unwrap();
            let back: ControlResponse = serde_json::from_str(&json)
                .unwrap_or_else(|e| panic!("failed to round-trip {json}: {e}"));
            let json2 = serde_json::to_string(&back).unwrap();
            assert_eq!(json, json2);
        }
    }

    #[test]
    fn unknown_type_is_error() {
        let result: Result<ControlRequest, _> = serde_json::from_str(r#"{"type":"bogus"}"#);
        assert!(result.is_err());
    }

    #[test]
    #[serial(control_pipe_env)]
    fn pipe_name_is_stable_and_prefixed() {
        unsafe { std::env::remove_var(CONTROL_PIPE_ENV) };
        let dir = std::path::Path::new("./some/config/dir");
        let a = control_pipe_name(dir);
        let b = control_pipe_name(dir);
        assert_eq!(a, b);
        assert!(a.starts_with(CONTROL_PIPE_PREFIX));
        let hex = &a[CONTROL_PIPE_PREFIX.len()..];
        assert_eq!(hex.len(), 16);
        assert!(hex.chars().all(|c| c.is_ascii_hexdigit() && !c.is_ascii_uppercase()));
    }

    #[test]
    #[serial(control_pipe_env)]
    fn pipe_name_case_and_trailing_separator_insensitive() {
        unsafe { std::env::remove_var(CONTROL_PIPE_ENV) };
        let lower = control_pipe_name(std::path::Path::new("./some/config/dir"));
        let upper = control_pipe_name(std::path::Path::new("./SOME/CONFIG/DIR"));
        let trailing = control_pipe_name(std::path::Path::new("./some/config/dir/"));
        assert_eq!(lower, upper);
        assert_eq!(lower, trailing);
    }

    #[test]
    #[serial(control_pipe_env)]
    fn pipe_name_differs_per_dir() {
        unsafe { std::env::remove_var(CONTROL_PIPE_ENV) };
        let a = control_pipe_name(std::path::Path::new("./some/config/dir-a"));
        let b = control_pipe_name(std::path::Path::new("./some/config/dir-b"));
        assert_ne!(a, b);
    }

    #[test]
    #[serial(control_pipe_env)]
    fn env_override_wins() {
        unsafe { std::env::set_var(CONTROL_PIPE_ENV, r"\\.\pipe\ggg-daemon-pinned") };
        let name = control_pipe_name(std::path::Path::new("./some/config/dir"));
        assert_eq!(name, r"\\.\pipe\ggg-daemon-pinned");
        unsafe { std::env::remove_var(CONTROL_PIPE_ENV) };
    }

    #[test]
    fn is_mutating_classification() {
        assert!(ControlRequest::Add {
            url: String::new(),
            folder: None,
            referrer: None,
            headers: vec![],
            immediate: false,
        }
        .is_mutating());
        assert!(ControlRequest::Start { id: Uuid::nil() }.is_mutating());
        assert!(ControlRequest::Pause { id: Uuid::nil() }.is_mutating());
        assert!(ControlRequest::Remove { id: Uuid::nil() }.is_mutating());
        assert!(ControlRequest::StartAll { folder: None }.is_mutating());
        assert!(ControlRequest::PauseAll { folder: None }.is_mutating());
        assert!(
            ControlRequest::Modify {
                id: Uuid::nil(),
                edit: HeaderEdit::default(),
            }
            .is_mutating()
        );
        assert!(
            ControlRequest::Move {
                id: Uuid::nil(),
                op: MoveOp::ToTop,
            }
            .is_mutating()
        );
        assert!(
            ControlRequest::Priority {
                id: Uuid::nil(),
                priority: 0,
            }
            .is_mutating()
        );

        assert!(!ControlRequest::Ping.is_mutating());
        assert!(!ControlRequest::Status.is_mutating());
        assert!(!ControlRequest::Shutdown.is_mutating());
        assert!(!ControlRequest::List {
            statuses: vec![],
            folder: None,
        }
        .is_mutating());
        assert!(!ControlRequest::Get { id: Uuid::nil() }.is_mutating());
        assert!(!ControlRequest::Wait {
            id: Uuid::nil(),
            timeout_secs: None,
        }
        .is_mutating());
        assert!(!ControlRequest::ReloadScripts.is_mutating());
        assert!(!ControlRequest::SetScriptEnabled {
            name: String::new(),
            enabled: true,
        }
        .is_mutating());
        assert!(!ControlRequest::ConfigGet { key: String::new() }.is_mutating());
        assert!(!ControlRequest::ConfigSet {
            key: String::new(),
            value: String::new(),
        }
        .is_mutating());
    }
}
