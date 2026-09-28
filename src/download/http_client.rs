use anyhow::{anyhow, Result};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue, CONTENT_LENGTH, ETAG, IF_RANGE, LAST_MODIFIED, RANGE, REFERER, USER_AGENT};
use std::path::Path;
use tokio::fs::File;
use tokio::io::{AsyncWriteExt, BufWriter};
use futures_util::StreamExt;

use super::http_errors::HttpErrorInfo;

/// Progress callback for download operations
pub type ProgressCallback = Box<dyn Fn(u64, Option<u64>) + Send + Sync>;

/// Information about a download response
#[derive(Debug, Clone)]
pub struct DownloadInfo {
    pub size: Option<u64>,
    pub resume_supported: bool,
    pub etag: Option<String>,
    pub last_modified: Option<String>,
    pub filename: Option<String>,
    pub status: u16,
    pub headers: std::collections::HashMap<String, String>,
    pub content_type: Option<String>,
    pub auth_required: bool,
    pub auth_realm: Option<String>,
    /// The final URL after following redirects (if any)
    pub final_url: Option<String>,
    /// Total bytes present in the file after the transfer (resume offset plus
    /// bytes written this session). Zero for metadata-only (`get_info`) calls.
    pub bytes_written: u64,
}

/// Parsed HTTP response headers
#[derive(Debug, Clone)]
struct ParsedHeaders {
    size: Option<u64>,
    resume_supported: bool,
    etag: Option<String>,
    last_modified: Option<String>,
    filename: Option<String>,
    content_type: Option<String>,
    all_headers: std::collections::HashMap<String, String>,
}

/// Extract filename from Content-Disposition header value.
/// Prioritizes `filename*` (RFC 5987/6266) over `filename`.
fn parse_content_disposition_filename(value: &str) -> Option<String> {
    // Try filename* first (RFC 5987: encoding'language'value)
    if let Some(star_pos) = value.find("filename*=") {
        let rest = &value[star_pos + "filename*=".len()..];
        // Strip optional quotes around the entire ext-value (non-standard but seen in the wild)
        let rest = rest.trim_start_matches('"');
        // Format: charset'language'percent-encoded-value
        if let Some(tick_pos) = rest.find("''") {
            let encoded = &rest[tick_pos + 2..];
            // Take until the next `;` or `"` or end
            let encoded = encoded
                .split(|c: char| c == ';' || c == '"')
                .next()
                .unwrap_or(encoded)
                .trim();
            let decoded = percent_encoding::percent_decode_str(encoded)
                .decode_utf8()
                .ok()?;
            if !decoded.is_empty() {
                return Some(decoded.into_owned());
            }
        }
    }

    // Fallback to filename= (ASCII)
    if let Some(pos) = value.find("filename=") {
        let rest = &value[pos + "filename=".len()..];
        let name = if rest.starts_with('"') {
            // Quoted-string: read until the closing quote, honoring `\"`
            // (and `\\`) backslash escapes so an escaped quote inside the value
            // doesn't terminate it early.
            let mut out = String::new();
            let mut chars = rest[1..].chars();
            while let Some(c) = chars.next() {
                match c {
                    '\\' => {
                        if let Some(escaped) = chars.next() {
                            out.push(escaped);
                        }
                    }
                    '"' => break,
                    _ => out.push(c),
                }
            }
            out
        } else {
            // Unquoted: take until `;` or end
            rest.split(';').next().unwrap_or("").trim().to_string()
        };
        if !name.is_empty() {
            return Some(name);
        }
    }

    None
}

/// Parse common HTTP response headers
fn parse_response_headers(headers: &HeaderMap) -> ParsedHeaders {
    let size = headers
        .get(CONTENT_LENGTH)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.parse::<u64>().ok());

    let resume_supported = headers
        .get("accept-ranges")
        .and_then(|v| v.to_str().ok())
        .map(|v| v == "bytes")
        .unwrap_or(false);

    let etag = headers
        .get(ETAG)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let last_modified = headers
        .get(LAST_MODIFIED)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    let filename = headers
        .get("content-disposition")
        .and_then(|v| v.to_str().ok())
        .and_then(|v| parse_content_disposition_filename(v));

    let content_type = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);

    // Selective header storage - only keep headers that are actually useful
    // This reduces storage and memory footprint significantly for large download queues
    const USEFUL_HEADERS: &[&str] = &[
        // Resume/Range support
        "accept-ranges",
        "content-range",
        // Caching/Validation
        "etag",
        "last-modified",
        "cache-control",
        "expires",
        // Content info
        "content-type",
        "content-length",
        "content-disposition",
        "content-encoding",
        // Server/Request tracking (useful for debugging)
        "server",
        "x-request-id",
        "x-served-by",
        // Rate limiting
        "retry-after",
        "x-ratelimit-remaining",
        "x-ratelimit-reset",
    ];

    let mut all_headers = std::collections::HashMap::new();
    for (key, value) in headers.iter() {
        let key_lower = key.as_str().to_lowercase();
        if USEFUL_HEADERS.contains(&key_lower.as_str()) {
            if let Ok(value_str) = value.to_str() {
                all_headers.insert(key_lower, value_str.to_string());
            }
        }
    }

    ParsedHeaders {
        size,
        resume_supported,
        etag,
        last_modified,
        filename,
        content_type,
        all_headers,
    }
}

pub struct HttpClient {
    client: reqwest::Client,
}

impl HttpClient {
    /// Create a new HTTP client with default settings
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36")
            .timeout(std::time::Duration::from_secs(300))        // 5 min total timeout
            .connect_timeout(std::time::Duration::from_secs(30)) // 30s connect timeout
            .pool_max_idle_per_host(10)                          // Allow more idle connections
            .build()?;

        Ok(Self { client })
    }

    /// Build the configured client, falling back to a default reqwest client
    /// (with a logged error) if the custom builder options fail, instead of
    /// panicking at application startup. Only a completely broken TLS/resolver
    /// environment — where no client can be built at all — still panics.
    pub fn new_or_fallback() -> Self {
        if let Ok(client) = Self::new() {
            return client;
        }
        match reqwest::Client::builder().build() {
            Ok(client) => {
                tracing::error!("Configured HTTP client failed to build; using a default client");
                Self { client }
            }
            Err(e) => panic!("Unable to initialize any HTTP client: {e}"),
        }
    }

    /// Create a new HTTP client with custom user agent
    pub fn with_user_agent(user_agent: &str) -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent)
            .timeout(std::time::Duration::from_secs(300))        // 5 min total timeout
            .connect_timeout(std::time::Duration::from_secs(30)) // 30s connect timeout
            .pool_max_idle_per_host(10)                          // Allow more idle connections
            .build()?;

        Ok(Self { client })
    }

    /// Get download information without downloading the file
    pub async fn get_info(&self, url: &str, headers: &HeaderMap) -> Result<DownloadInfo> {
        let response = self.client
            .head(url)
            .headers(headers.clone())
            .send()
            .await?;

        // Parse response headers
        let parsed = parse_response_headers(response.headers());
        let status = response.status().as_u16();
        let (auth_required, auth_realm) = Self::check_auth_required(status, response.headers());
        let final_url = Some(response.url().to_string());

        Ok(DownloadInfo {
            size: parsed.size,
            resume_supported: parsed.resume_supported,
            etag: parsed.etag,
            last_modified: parsed.last_modified,
            filename: parsed.filename,
            status,
            headers: parsed.all_headers,
            content_type: parsed.content_type,
            auth_required,
            auth_realm,
            final_url,
            bytes_written: 0,
        })
    }

    /// Download a file with streaming and progress callback
    pub async fn download_to_file<F>(
        &self,
        url: &str,
        path: &Path,
        headers: &HeaderMap,
        resume_from: Option<u64>,
        if_range: Option<&str>,
        progress_callback: Option<F>,
    ) -> Result<DownloadInfo>
    where
        F: Fn(u64, Option<u64>) + Send + Sync,
    {
        tracing::trace!("Starting download: url={}, path={:?}, resume_from={:?}", url, path, resume_from);

        let mut request = self.client.get(url).headers(headers.clone());

        // Add Range header for resume support. If-Range ties the range
        // request to the version we already have bytes from: the server
        // returns 206 for that exact version and 200 (full body) for any
        // other, so a resource that changed since the partial file was
        // written is caught by the 200-instead-of-206 handling below instead
        // of corrupting the file by appending mismatched ranges.
        let mut actual_resume_from = resume_from;
        if let Some(offset) = resume_from {
            tracing::trace!("Adding Range header for resume: bytes={}-", offset);
            request = request.header(RANGE, format!("bytes={}-", offset));
            if let Some(validator) = if_range {
                request = request.header(IF_RANGE, validator);
            }
        }

        tracing::trace!("Sending HTTP request to {}", url);
        let mut response = request.send().await?;
        tracing::trace!("Received response with status: {}", response.status());

        // Fallback: if server returns 416 (Range Not Satisfiable) during resume,
        // retry from scratch without Range header
        if response.status().as_u16() == 416 && resume_from.is_some() {
            tracing::warn!("Got 416 Range Not Satisfiable, retrying without Range header");
            actual_resume_from = None;
            let retry_request = self.client.get(url).headers(headers.clone());
            response = retry_request.send().await?;
            tracing::trace!("Retry response status: {}", response.status());
        }

        // If we sent a Range header but the server ignored it and replied 200
        // (full body) instead of 206 (partial), appending to the existing
        // partial file would corrupt it. Restart as a fresh download: the 200
        // response already carries the complete body, so just rewrite the file.
        if actual_resume_from.is_some() && response.status().as_u16() == 200 {
            tracing::warn!(
                "Server ignored Range header (returned 200 instead of 206); \
                 restarting download from scratch to avoid file corruption"
            );
            actual_resume_from = None;
        }

        // Check for auth requirement BEFORE generic error check
        let status = response.status().as_u16();
        let (auth_required, auth_realm) = Self::check_auth_required(status, response.headers());

        if auth_required {
            let error_info = HttpErrorInfo::from_status(status);
            return Err(anyhow!(
                "{}: realm={}",
                error_info.format(),
                auth_realm.unwrap_or_else(|| "unknown".to_string())
            ));
        }

        if !response.status().is_success() {
            let status = response.status().as_u16();
            let error_info = HttpErrorInfo::from_status(status);
            return Err(anyhow!("{}", error_info.format()));
        }

        // Get download info from response headers
        tracing::trace!("Parsing response headers for download info");
        let parsed = parse_response_headers(response.headers());

        tracing::trace!("Download info: size={:?}, resume_supported={}", parsed.size, parsed.resume_supported);

        // Extract values for use in download logic
        let size = parsed.size;
        let resume_supported = parsed.resume_supported;
        let etag = parsed.etag;
        let last_modified = parsed.last_modified;
        let filename = parsed.filename;
        let status = response.status().as_u16();
        let content_type = parsed.content_type;
        let response_headers = parsed.all_headers;
        let final_url = Some(response.url().to_string());

        // Open file for writing (append if resuming, fresh if fallback occurred)
        let file = if actual_resume_from.is_some() {
            tokio::fs::OpenOptions::new()
                .append(true)
                .create(true)
                .open(path)
                .await?
        } else {
            File::create(path).await?
        };

        // Wrap file in BufWriter for better I/O performance (64KB buffer)
        // Larger buffer reduces syscall overhead for high-speed downloads
        let mut file = BufWriter::with_capacity(64 * 1024, file);

        // Stream the response body to file
        let mut stream = response.bytes_stream();
        let mut downloaded = actual_resume_from.unwrap_or(0);
        let mut last_progress_update = std::time::Instant::now();
        let mut last_progress_bytes = downloaded;

        // Progress update thresholds
        const MIN_PROGRESS_BYTES: u64 = 1024 * 1024; // 1 MB
        const MIN_PROGRESS_INTERVAL: std::time::Duration = std::time::Duration::from_millis(500);

        // For a 206 resume, `size` (this response's own Content-Length) is
        // only the *remaining* bytes, not the file's full size. Report the
        // resume offset plus that length instead, so a consumer that persists
        // this value (e.g. `DownloadTask::size`) keeps the true total instead
        // of silently shrinking it to the partial length on every resume.
        let total_for_progress = match actual_resume_from {
            Some(offset) => size.map(|partial| offset + partial),
            None => size,
        };

        while let Some(chunk) = stream.next().await {
            let chunk = chunk?;
            file.write_all(&chunk).await?;
            downloaded += chunk.len() as u64;

            // Call progress callback (throttled by both time and data size to reduce overhead)
            if let Some(ref callback) = progress_callback {
                let now = std::time::Instant::now();
                let bytes_since_update = downloaded - last_progress_bytes;
                let time_since_update = now.duration_since(last_progress_update);

                if bytes_since_update >= MIN_PROGRESS_BYTES || time_since_update >= MIN_PROGRESS_INTERVAL {
                    callback(downloaded, total_for_progress);
                    last_progress_bytes = downloaded;
                    last_progress_update = now;
                }
            }
        }

        // Final progress update to ensure 100% is reported
        if let Some(ref callback) = progress_callback {
            callback(downloaded, total_for_progress);
        }

        file.flush().await?;

        // Guard against a silently truncated transfer. If the server advertised
        // a length and the connection closed early, the byte stream simply ends
        // without an error, which would otherwise be reported as a successful
        // download of a partial file. Returning an error lets the retry/resume
        // path recover.
        let expected_total = total_for_progress;
        if let Some(expected) = expected_total {
            if downloaded < expected {
                return Err(anyhow!(
                    "Incomplete download: received {} of {} bytes (connection closed early)",
                    downloaded,
                    expected
                ));
            }
        }

        Ok(DownloadInfo {
            size,
            resume_supported,
            etag,
            last_modified,
            filename,
            status,
            headers: response_headers,
            content_type,
            auth_required: false,  // Already checked above, would have returned early if true
            auth_realm: None,
            final_url,
            bytes_written: downloaded,
        })
    }

    /// Build custom headers from user-specified values
    pub fn build_headers(
        user_agent: Option<&str>,
        referer: Option<&str>,
        custom_headers: &std::collections::HashMap<String, String>,
    ) -> Result<HeaderMap> {
        let mut headers = HeaderMap::new();

        if let Some(ua) = user_agent {
            headers.insert(USER_AGENT, HeaderValue::from_str(ua)?);
        }

        if let Some(ref_url) = referer {
            headers.insert(REFERER, HeaderValue::from_str(ref_url)?);
        }

        for (key, value) in custom_headers {
            let header_name: HeaderName = key.parse()?;
            headers.insert(header_name, HeaderValue::from_str(value)?);
        }

        Ok(headers)
    }

    /// Check if response requires authentication
    /// Returns (requires_auth, realm)
    fn check_auth_required(
        status: u16,
        headers: &HeaderMap
    ) -> (bool, Option<String>) {
        // Detect 401 Unauthorized or 407 Proxy Authentication Required
        if status != 401 && status != 407 {
            return (false, None);
        }

        // Get appropriate auth header
        let auth_header_name = if status == 401 {
            "www-authenticate"
        } else {
            "proxy-authenticate"
        };

        let auth_header_value = headers
            .get(auth_header_name)
            .and_then(|v| v.to_str().ok());

        // Simple realm extraction: Basic realm="value" or Digest realm="value"
        let realm = auth_header_value.and_then(|header| {
            // Find realm= in header
            if let Some(start) = header.find("realm=") {
                let realm_part = &header[start + 6..];
                if realm_part.starts_with('"') {
                    // realm="quoted value"
                    realm_part.split('"').nth(1).map(|s| s.to_string())
                } else {
                    // realm=unquoted
                    realm_part.split_whitespace().next().map(|s| s.to_string())
                }
            } else {
                None
            }
        });

        (true, realm)
    }
}

impl Default for HttpClient {
    fn default() -> Self {
        Self::new_or_fallback()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};
    use std::sync::{Arc, Mutex};

    #[tokio::test]
    async fn test_get_info_parses_content_length() {
        let mock_server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .and(path("/file.zip"))
            .respond_with(ResponseTemplate::new(200)
                .append_header("Content-Length", "1024")
                .append_header("Accept-Ranges", "bytes"))
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/file.zip", mock_server.uri());
        let info = client.get_info(&url, &Default::default()).await.unwrap();

        assert_eq!(info.size, Some(1024));
    }

    #[tokio::test]
    async fn test_get_info_detects_resume_support() {
        let mock_server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .and(path("/file.zip"))
            .respond_with(ResponseTemplate::new(200)
                .append_header("Content-Length", "2048")
                .append_header("Accept-Ranges", "bytes"))
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/file.zip", mock_server.uri());
        let info = client.get_info(&url, &Default::default()).await.unwrap();

        assert!(info.resume_supported);
    }

    #[tokio::test]
    async fn test_get_info_extracts_etag() {
        let mock_server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .and(path("/file.zip"))
            .respond_with(ResponseTemplate::new(200)
                .append_header("Content-Length", "512")
                .append_header("ETag", "\"abc123\""))
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/file.zip", mock_server.uri());
        let info = client.get_info(&url, &Default::default()).await.unwrap();

        assert_eq!(info.etag, Some("\"abc123\"".to_string()));
    }

    #[tokio::test]
    async fn test_get_info_extracts_last_modified() {
        let mock_server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .and(path("/file.zip"))
            .respond_with(ResponseTemplate::new(200)
                .append_header("Content-Length", "256")
                .append_header("Last-Modified", "Wed, 21 Oct 2015 07:28:00 GMT"))
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/file.zip", mock_server.uri());
        let info = client.get_info(&url, &Default::default()).await.unwrap();

        assert_eq!(info.last_modified, Some("Wed, 21 Oct 2015 07:28:00 GMT".to_string()));
    }

    #[tokio::test]
    async fn test_download_creates_file() {
        let mock_server = MockServer::start().await;

        let test_data = b"Hello, World!";
        Mock::given(method("GET"))
            .and(path("/file.txt"))
            .respond_with(ResponseTemplate::new(200)
                .set_body_bytes(test_data.to_vec())
                .append_header("Content-Length", test_data.len().to_string()))
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/file.txt", mock_server.uri());

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("downloaded.txt");

        client.download_to_file(&url, &file_path, &Default::default(), None, None, None::<fn(u64, Option<u64>)>)
            .await
            .unwrap();

        assert!(file_path.exists());
        let content = std::fs::read(&file_path).unwrap();
        assert_eq!(content, test_data);
    }

    #[tokio::test]
    async fn test_download_progress_callback() {
        let mock_server = MockServer::start().await;

        let test_data = b"Test data for progress";
        Mock::given(method("GET"))
            .and(path("/file.txt"))
            .respond_with(ResponseTemplate::new(200)
                .set_body_bytes(test_data.to_vec())
                .append_header("Content-Length", test_data.len().to_string()))
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/file.txt", mock_server.uri());

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("progress.txt");

        let callback_count = Arc::new(Mutex::new(0));
        let callback_count_clone = callback_count.clone();

        client.download_to_file(
            &url,
            &file_path,
            &Default::default(),
            None,
            None,
            Some(move |downloaded, total| {
                *callback_count_clone.lock().unwrap() += 1;
                assert!(downloaded > 0);
                assert_eq!(total, Some(test_data.len() as u64));
            })
        )
        .await
        .unwrap();

        // Callback should have been called at least once
        assert!(*callback_count.lock().unwrap() > 0);
    }

    #[tokio::test]
    async fn test_download_resume_from_offset() {
        let mock_server = MockServer::start().await;

        let full_data = b"Complete file content";
        let resume_offset = 9u64; // Resume from byte 9

        Mock::given(method("GET"))
            .and(path("/file.txt"))
            .respond_with(ResponseTemplate::new(206) // Partial Content
                .set_body_bytes(full_data[resume_offset as usize..].to_vec())
                .append_header("Content-Length", (full_data.len() - resume_offset as usize).to_string())
                .append_header("Accept-Ranges", "bytes"))
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/file.txt", mock_server.uri());

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("resume.txt");

        // Create initial partial file
        std::fs::write(&file_path, &full_data[..resume_offset as usize]).unwrap();

        client.download_to_file(&url, &file_path, &Default::default(), Some(resume_offset), None, None::<fn(u64, Option<u64>)>)
            .await
            .unwrap();

        let content = std::fs::read(&file_path).unwrap();
        assert_eq!(content, full_data);
    }

    #[tokio::test]
    async fn test_download_resume_ignored_range_returns_200() {
        // Server ignores the Range header and returns 200 with the full body.
        // The existing partial file must be overwritten, not appended to.
        let mock_server = MockServer::start().await;

        let full_data = b"Complete file content";
        Mock::given(method("GET"))
            .and(path("/file.txt"))
            .respond_with(ResponseTemplate::new(200) // Full content, ignoring Range
                .set_body_bytes(full_data.to_vec())
                .append_header("Content-Length", full_data.len().to_string()))
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/file.txt", mock_server.uri());

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("resume200.txt");

        // Pre-existing partial file that must NOT be appended to
        std::fs::write(&file_path, b"Complete").unwrap();

        client.download_to_file(&url, &file_path, &Default::default(), Some(8), None, None::<fn(u64, Option<u64>)>)
            .await
            .unwrap();

        let content = std::fs::read(&file_path).unwrap();
        assert_eq!(content, full_data, "file should be rewritten with the full body, not appended");
    }

    #[tokio::test]
    async fn test_download_handles_http_error() {
        let mock_server = MockServer::start().await;

        Mock::given(method("GET"))
            .and(path("/missing.txt"))
            .respond_with(ResponseTemplate::new(404))
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/missing.txt", mock_server.uri());

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("error.txt");

        let result = client.download_to_file(&url, &file_path, &Default::default(), None, None, None::<fn(u64, Option<u64>)>)
            .await;

        assert!(result.is_err());
    }

    #[test]
    fn test_parse_response_headers_all_fields() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_LENGTH, "2048".parse().unwrap());
        headers.insert("accept-ranges", "bytes".parse().unwrap());
        headers.insert(ETAG, "\"xyz789\"".parse().unwrap());
        headers.insert(LAST_MODIFIED, "Thu, 22 Oct 2015 08:30:00 GMT".parse().unwrap());
        headers.insert("content-disposition", "attachment; filename=\"test.zip\"".parse().unwrap());
        headers.insert("content-type", "application/zip".parse().unwrap());
        headers.insert("x-custom-header", "custom-value".parse().unwrap()); // Not in USEFUL_HEADERS

        let parsed = parse_response_headers(&headers);

        assert_eq!(parsed.size, Some(2048));
        assert!(parsed.resume_supported);
        assert_eq!(parsed.etag, Some("\"xyz789\"".to_string()));
        assert_eq!(parsed.last_modified, Some("Thu, 22 Oct 2015 08:30:00 GMT".to_string()));
        assert_eq!(parsed.filename, Some("test.zip".to_string()));
        assert_eq!(parsed.content_type, Some("application/zip".to_string()));
        // Only 6 useful headers are stored (x-custom-header is filtered out)
        assert_eq!(parsed.all_headers.len(), 6);
        // Verify useful headers are stored (lowercase normalized)
        assert_eq!(parsed.all_headers.get("accept-ranges"), Some(&"bytes".to_string()));
        assert!(parsed.all_headers.get("x-custom-header").is_none());
    }

    #[test]
    fn test_parse_response_headers_minimal() {
        let headers = HeaderMap::new();
        let parsed = parse_response_headers(&headers);

        assert_eq!(parsed.size, None);
        assert!(!parsed.resume_supported);
        assert_eq!(parsed.etag, None);
        assert_eq!(parsed.last_modified, None);
        assert_eq!(parsed.filename, None);
        assert_eq!(parsed.content_type, None);
        assert_eq!(parsed.all_headers.len(), 0);
    }

    #[test]
    fn test_parse_response_headers_resume_not_supported() {
        let mut headers = HeaderMap::new();
        headers.insert("accept-ranges", "none".parse().unwrap());

        let parsed = parse_response_headers(&headers);

        assert!(!parsed.resume_supported);
    }

    #[test]
    fn test_parse_response_headers_content_disposition_parsing() {
        let mut headers = HeaderMap::new();
        headers.insert("content-disposition", "attachment; filename=\"document.pdf\"".parse().unwrap());

        let parsed = parse_response_headers(&headers);

        assert_eq!(parsed.filename, Some("document.pdf".to_string()));
    }

    #[test]
    fn test_content_disposition_filename_star_priority() {
        // filename* (RFC 5987) should take priority over filename
        let value = "inline; filename=\"___.jpg.webp\"; filename*=UTF-8''%E3%81%93%E3%81%AE%E3%81%8B.jpg.webp";
        assert_eq!(
            parse_content_disposition_filename(value),
            Some("このか.jpg.webp".to_string())
        );
    }

    #[test]
    fn test_content_disposition_filename_only() {
        let value = "attachment; filename=\"report.pdf\"";
        assert_eq!(
            parse_content_disposition_filename(value),
            Some("report.pdf".to_string())
        );
    }

    #[test]
    fn test_content_disposition_filename_star_only() {
        let value = "attachment; filename*=UTF-8''%E3%83%86%E3%82%B9%E3%83%88.txt";
        assert_eq!(
            parse_content_disposition_filename(value),
            Some("テスト.txt".to_string())
        );
    }

    #[test]
    fn test_content_disposition_escaped_quote() {
        // A backslash-escaped quote inside the quoted value must not end it early.
        let value = r#"attachment; filename="a\"b.txt""#;
        assert_eq!(
            parse_content_disposition_filename(value),
            Some(r#"a"b.txt"#.to_string())
        );
    }

    #[test]
    fn test_content_disposition_no_filename() {
        let value = "inline";
        assert_eq!(parse_content_disposition_filename(value), None);
    }

    #[test]
    fn test_parse_response_headers_invalid_content_length() {
        let mut headers = HeaderMap::new();
        headers.insert(CONTENT_LENGTH, "invalid".parse().unwrap());

        let parsed = parse_response_headers(&headers);

        assert_eq!(parsed.size, None);
    }

    #[tokio::test]
    async fn test_get_info_captures_final_url_on_redirect() {
        let mock_server = MockServer::start().await;

        // Redirect /original -> /redirected/image.jpg
        Mock::given(method("HEAD"))
            .and(path("/original"))
            .respond_with(
                ResponseTemplate::new(302)
                    .append_header("Location", &format!("{}/redirected/image.jpg", mock_server.uri())),
            )
            .mount(&mock_server)
            .await;

        Mock::given(method("HEAD"))
            .and(path("/redirected/image.jpg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Content-Length", "4096"),
            )
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/original", mock_server.uri());
        let info = client.get_info(&url, &Default::default()).await.unwrap();

        assert!(info.final_url.is_some());
        let final_url = info.final_url.unwrap();
        assert!(final_url.contains("/redirected/image.jpg"), "final_url should contain redirect destination: {}", final_url);
        assert_ne!(final_url, url, "final_url should differ from original url on redirect");
    }

    #[tokio::test]
    async fn test_get_info_final_url_same_without_redirect() {
        let mock_server = MockServer::start().await;

        Mock::given(method("HEAD"))
            .and(path("/direct/file.zip"))
            .respond_with(
                ResponseTemplate::new(200)
                    .append_header("Content-Length", "2048"),
            )
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/direct/file.zip", mock_server.uri());
        let info = client.get_info(&url, &Default::default()).await.unwrap();

        assert!(info.final_url.is_some());
        // Without redirect, final_url equals original url
        assert_eq!(info.final_url.unwrap(), url);
    }

    #[tokio::test]
    async fn test_download_captures_final_url_on_redirect() {
        let mock_server = MockServer::start().await;

        let test_data = b"redirect download content";

        // Redirect /download -> /actual/photo.jpg
        Mock::given(method("GET"))
            .and(path("/download"))
            .respond_with(
                ResponseTemplate::new(302)
                    .append_header("Location", &format!("{}/actual/photo.jpg", mock_server.uri())),
            )
            .mount(&mock_server)
            .await;

        Mock::given(method("GET"))
            .and(path("/actual/photo.jpg"))
            .respond_with(
                ResponseTemplate::new(200)
                    .set_body_bytes(test_data.to_vec())
                    .append_header("Content-Length", test_data.len().to_string()),
            )
            .mount(&mock_server)
            .await;

        let client = HttpClient::new().unwrap();
        let url = format!("{}/download", mock_server.uri());

        let temp_dir = tempfile::tempdir().unwrap();
        let file_path = temp_dir.path().join("out.bin");

        let info = client
            .download_to_file(&url, &file_path, &Default::default(), None, None, None::<fn(u64, Option<u64>)>)
            .await
            .unwrap();

        assert!(info.final_url.is_some());
        let final_url = info.final_url.unwrap();
        assert!(final_url.contains("/actual/photo.jpg"), "final_url should contain redirect destination: {}", final_url);
    }
}
