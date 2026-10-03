use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Hook event types that scripts can listen to
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum HookEvent {
    /// Before HTTP request is sent - can modify URL, headers, user-agent
    BeforeRequest,
    /// After receiving response headers - can inspect status, headers
    HeadersReceived,
    /// When authentication is required - can provide credentials
    AuthRequired,
    /// After download completes successfully - can rename/move file
    Completed,
    /// When error occurs - can handle errors and implement retry logic
    ErrorOccurred,
    /// Download progress updates
    Progress,
}

impl HookEvent {
    /// Parse event name from string (JavaScript event names)
    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "beforeRequest" | "onBeforeRequest" => Some(Self::BeforeRequest),
            "headersReceived" | "onHeadersReceived" => Some(Self::HeadersReceived),
            "authRequired" | "onAuthRequired" => Some(Self::AuthRequired),
            "completed" | "complete" | "onCompleted" => Some(Self::Completed),
            "error" | "errorOccurred" | "onErrorOccurred" => Some(Self::ErrorOccurred),
            "progress" | "onProgress" => Some(Self::Progress),
            _ => None,
        }
    }

    /// Get the canonical event name for JavaScript
    pub fn name(&self) -> &'static str {
        match self {
            Self::BeforeRequest => "beforeRequest",
            Self::HeadersReceived => "headersReceived",
            Self::AuthRequired => "authRequired",
            Self::Completed => "completed",
            Self::ErrorOccurred => "error",
            Self::Progress => "progress",
        }
    }

    /// Check if this event requires synchronous execution
    pub fn is_sync(&self) -> bool {
        matches!(
            self,
            Self::BeforeRequest | Self::HeadersReceived | Self::AuthRequired | Self::Completed
        )
    }
}

/// Trait for event context objects that can be passed to JavaScript
pub trait EventContext: Serialize + for<'de> Deserialize<'de> {
    /// Get the event type this context is for
    fn event_type() -> HookEvent;

    /// Convert to JSON value for passing to JavaScript
    fn to_json(&self) -> Result<serde_json::Value, serde_json::Error> {
        serde_json::to_value(self)
    }

    /// Create from JSON value (after JavaScript modification)
    fn from_json(value: serde_json::Value) -> Result<Self, serde_json::Error>
    where
        Self: Sized,
    {
        serde_json::from_value(value)
    }
}

/// Context for beforeRequest hook
/// JavaScript can modify: url, headers, user_agent
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct BeforeRequestContext {
    /// Download URL (modifiable)
    pub url: String,
    /// HTTP headers (modifiable)
    #[serde(default)]
    pub headers: HashMap<String, String>,
    /// User agent string (modifiable)
    pub user_agent: Option<String>,
    /// Download ID (read-only)
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_id: Option<String>,
}

impl EventContext for BeforeRequestContext {
    fn event_type() -> HookEvent {
        HookEvent::BeforeRequest
    }
}

/// Context for headersReceived hook
/// All fields are read-only
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HeadersReceivedContext {
    /// Original request URL
    pub url: String,
    /// HTTP status code
    pub status: u16,
    /// Response headers
    pub headers: HashMap<String, String>,
    /// Content length if known
    pub content_length: Option<u64>,
    /// ETag if present
    pub etag: Option<String>,
    /// Last-Modified if present
    pub last_modified: Option<String>,
    /// Content-Type if present
    pub content_type: Option<String>,
}

impl EventContext for HeadersReceivedContext {
    fn event_type() -> HookEvent {
        HookEvent::HeadersReceived
    }
}

/// Context for authRequired hook
/// JavaScript can provide: username, password
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AuthRequiredContext {
    /// URL requiring authentication
    pub url: String,
    /// Authentication realm
    pub realm: Option<String>,
    /// Username (modifiable)
    pub username: Option<String>,
    /// Password (modifiable)
    pub password: Option<String>,
}

impl EventContext for AuthRequiredContext {
    fn event_type() -> HookEvent {
        HookEvent::AuthRequired
    }
}

/// Context for completed hook
/// JavaScript can modify: new_filename, move_to_path
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CompletedContext {
    /// Original URL
    pub url: String,
    /// Current filename
    pub filename: String,
    /// Current save path (directory)
    pub save_path: String,
    /// New filename if renaming (modifiable)
    pub new_filename: Option<String>,
    /// Target directory if moving (modifiable). Created if missing; a relative
    /// path is resolved against `save_path`.
    pub move_to_path: Option<String>,
    /// Download size in bytes
    pub size: u64,
    /// Download duration in seconds
    pub duration: Option<f64>,
}

impl EventContext for CompletedContext {
    fn event_type() -> HookEvent {
        HookEvent::Completed
    }
}

/// Context for error hook
/// All fields are read-only
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ErrorContext {
    /// Original URL
    pub url: String,
    /// Filename if known
    pub filename: Option<String>,
    /// Error message
    pub error: String,
    /// Retry count
    pub retry_count: u32,
    /// HTTP status code if applicable
    pub status_code: Option<u16>,
}

impl EventContext for ErrorContext {
    fn event_type() -> HookEvent {
        HookEvent::ErrorOccurred
    }
}

/// Context for progress hook
/// All fields are read-only
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProgressContext {
    /// Original URL
    pub url: String,
    /// Filename
    pub filename: String,
    /// Downloaded bytes
    pub downloaded: u64,
    /// Total bytes (if known)
    pub total: Option<u64>,
    /// Download speed in bytes/sec
    pub speed: Option<f64>,
    /// Progress percentage (0-100); None when the total is unknown or zero
    pub percentage: Option<f32>,
}

impl ProgressContext {
    /// Percentage 0-100, or None when the total is unknown or zero.
    pub fn compute_percentage(downloaded: u64, total: Option<u64>) -> Option<f32> {
        match total {
            Some(t) if t > 0 => Some(((downloaded as f64 / t as f64) * 100.0).min(100.0) as f32),
            _ => None,
        }
    }
}

impl EventContext for ProgressContext {
    fn event_type() -> HookEvent {
        HookEvent::Progress
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_hook_event_from_str() {
        assert_eq!(
            HookEvent::from_str("beforeRequest"),
            Some(HookEvent::BeforeRequest)
        );
        assert_eq!(
            HookEvent::from_str("onBeforeRequest"),
            Some(HookEvent::BeforeRequest)
        );
        assert_eq!(HookEvent::from_str("completed"), Some(HookEvent::Completed));
        assert_eq!(HookEvent::from_str("complete"), Some(HookEvent::Completed));
        assert_eq!(HookEvent::from_str("invalid"), None);
    }

    #[test]
    fn test_hook_event_name() {
        assert_eq!(HookEvent::BeforeRequest.name(), "beforeRequest");
        assert_eq!(HookEvent::Completed.name(), "completed");
    }

    #[test]
    fn test_hook_event_is_sync() {
        assert!(HookEvent::BeforeRequest.is_sync());
        assert!(HookEvent::Completed.is_sync());
        assert!(!HookEvent::ErrorOccurred.is_sync());
        assert!(!HookEvent::Progress.is_sync());
    }

    #[test]
    fn test_before_request_context_serialization() {
        let mut headers = HashMap::new();
        headers.insert("Referer".to_string(), "https://example.com".to_string());

        let ctx = BeforeRequestContext {
            url: "https://example.com/file.zip".to_string(),
            headers,
            user_agent: Some("GGG/1.0".to_string()),
            download_id: Some("test-id".to_string()),
        };

        // Serialize to JSON
        let json = ctx.to_json().unwrap();
        assert_eq!(json["url"], "https://example.com/file.zip");
        assert_eq!(json["userAgent"], "GGG/1.0");
        assert_eq!(json["headers"]["Referer"], "https://example.com");

        // Deserialize back
        let ctx2: BeforeRequestContext = BeforeRequestContext::from_json(json).unwrap();
        assert_eq!(ctx2.url, ctx.url);
        assert_eq!(ctx2.user_agent, ctx.user_agent);
    }

    #[test]
    fn test_headers_received_context_serialization() {
        let mut headers = HashMap::new();
        headers.insert("content-type".to_string(), "application/zip".to_string());

        let ctx = HeadersReceivedContext {
            url: "https://example.com/file.zip".to_string(),
            status: 200,
            headers,
            content_length: Some(1024),
            etag: Some("\"abc123\"".to_string()),
            last_modified: None,
            content_type: Some("application/zip".to_string()),
        };

        let json = ctx.to_json().unwrap();
        assert_eq!(json["status"], 200);
        assert_eq!(json["contentLength"], 1024);

        let ctx2: HeadersReceivedContext = HeadersReceivedContext::from_json(json).unwrap();
        assert_eq!(ctx2.status, ctx.status);
        assert_eq!(ctx2.content_length, ctx.content_length);
    }

    #[test]
    fn test_completed_context_serialization() {
        let ctx = CompletedContext {
            url: "https://example.com/file.zip".to_string(),
            filename: "file.zip".to_string(),
            save_path: "/downloads".to_string(),
            new_filename: Some("renamed.zip".to_string()),
            move_to_path: Some("/archive".to_string()),
            size: 1024,
            duration: Some(5.5),
        };

        let json = ctx.to_json().unwrap();
        assert_eq!(json["filename"], "file.zip");
        assert_eq!(json["newFilename"], "renamed.zip");
        assert_eq!(json["size"], 1024);

        let ctx2: CompletedContext = CompletedContext::from_json(json).unwrap();
        assert_eq!(ctx2.new_filename, Some("renamed.zip".to_string()));
        assert_eq!(ctx2.move_to_path, Some("/archive".to_string()));
    }

    #[test]
    fn test_error_context_serialization() {
        let ctx = ErrorContext {
            url: "https://example.com/file.zip".to_string(),
            filename: Some("file.zip".to_string()),
            error: "Connection timeout".to_string(),
            retry_count: 2,
            status_code: Some(504),
        };

        let json = ctx.to_json().unwrap();
        assert_eq!(json["error"], "Connection timeout");
        assert_eq!(json["retryCount"], 2);

        let ctx2: ErrorContext = ErrorContext::from_json(json).unwrap();
        assert_eq!(ctx2.error, "Connection timeout");
        assert_eq!(ctx2.retry_count, 2);
    }

    #[test]
    fn test_progress_context_serialization() {
        let ctx = ProgressContext {
            url: "https://example.com/file.zip".to_string(),
            filename: "file.zip".to_string(),
            downloaded: 512,
            total: Some(1024),
            speed: Some(1024.5),
            percentage: Some(50.0),
        };

        let json = ctx.to_json().unwrap();
        assert_eq!(json["downloaded"], 512);
        assert_eq!(json["total"], 1024);
        assert_eq!(json["percentage"], 50.0);

        let ctx2: ProgressContext = ProgressContext::from_json(json).unwrap();
        assert_eq!(ctx2.downloaded, 512);
        assert_eq!(ctx2.speed, Some(1024.5));
    }

    #[test]
    fn compute_percentage_handles_known_and_unknown_totals() {
        assert_eq!(ProgressContext::compute_percentage(50, Some(200)), Some(25.0));
        assert_eq!(ProgressContext::compute_percentage(0, Some(10)), Some(0.0));
        assert_eq!(ProgressContext::compute_percentage(10, None), None);
        assert_eq!(ProgressContext::compute_percentage(10, Some(0)), None);
        // Clamped when the server sends more than the advertised length.
        assert_eq!(ProgressContext::compute_percentage(300, Some(200)), Some(100.0));
    }

    #[test]
    fn test_context_round_trip() {
        // Test that all contexts can round-trip through JSON
        let mut headers = HashMap::new();
        headers.insert("test".to_string(), "value".to_string());

        let before_req = BeforeRequestContext {
            url: "https://test.com".to_string(),
            headers: headers.clone(),
            user_agent: Some("test".to_string()),
            download_id: None,
        };

        let json = serde_json::to_string(&before_req).unwrap();
        let restored: BeforeRequestContext = serde_json::from_str(&json).unwrap();
        assert_eq!(restored.url, before_req.url);
    }
}
