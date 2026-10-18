//! Turning user-supplied input (clipboard text, dropped text, `.url` files)
//! into download URLs. Pure functions with no I/O so every front end shares
//! the same rules.

use std::collections::HashSet;

use crate::util::url_expansion::expand_url;

/// Upper bound on URLs one registration may produce (after expansion).
pub const MAX_URLS_PER_REGISTRATION: usize = 1000;

/// Check if text is a valid URL with a scheme that can be downloaded.
/// Accepts the schemes the HTTP client (reqwest) can handle: http, https, ftp, ftps.
pub fn is_valid_download_url(text: &str) -> bool {
    match url::Url::parse(text) {
        Ok(parsed) => matches!(parsed.scheme(), "http" | "https" | "ftp" | "ftps"),
        Err(_) => false,
    }
}

/// Text (clipboard, dropped text, IPC payload) -> URLs, in order, deduplicated.
///
/// The original text of each URL is kept; it is not re-serialized through
/// `url::Url`, which would percent-encode range patterns such as `[1-10]`.
pub fn parse_url_text(text: &str) -> Vec<String> {
    const WRAPPERS: &[char] = &['<', '>', '"', '\'', '(', ')'];

    let mut seen: HashSet<String> = HashSet::new();
    let mut out: Vec<String> = Vec::new();

    let mut push = |candidate: &str, out: &mut Vec<String>| {
        if out.len() < MAX_URLS_PER_REGISTRATION && seen.insert(candidate.to_string()) {
            out.push(candidate.to_string());
        }
    };

    for raw_line in text.split('\n') {
        if out.len() >= MAX_URLS_PER_REGISTRATION {
            break;
        }
        let line = raw_line.strip_suffix('\r').unwrap_or(raw_line).trim();
        if line.is_empty() {
            continue;
        }
        // `url::Url` accepts raw spaces in the path, so "u1 u2" would parse as a
        // single URL; a line is taken whole only when it has no whitespace.
        if !line.contains(char::is_whitespace) && is_valid_download_url(line) {
            push(line, &mut out);
            continue;
        }
        // Prose or several URLs on one line: inspect each token on its own.
        for token in line.split_whitespace() {
            let token = token.trim_matches(WRAPPERS);
            if is_valid_download_url(token) {
                push(token, &mut out);
            }
        }
    }

    out
}

/// Contents of a Windows Internet Shortcut (.url) file -> its URL.
pub fn parse_internet_shortcut(content: &str) -> Option<String> {
    let mut in_section = false;

    for line in content.lines() {
        let line = line.trim();
        if let Some(inner) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            in_section = inner.trim().eq_ignore_ascii_case("InternetShortcut");
            continue;
        }
        if !in_section {
            continue;
        }
        if let Some((key, value)) = line.split_once('=')
            && key.trim().eq_ignore_ascii_case("URL")
        {
            // Only the first URL key counts, as in the Windows shell.
            let value = value.trim();
            return is_valid_download_url(value).then(|| value.to_string());
        }
    }

    None
}

/// Bytes of a small text file -> String: UTF-16LE when it starts with FF FE,
/// UTF-8 (lossy, BOM stripped) otherwise.
pub fn decode_text_file(bytes: &[u8]) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xFF, 0xFE]) {
        // A dangling odd byte cannot form a code unit and is dropped.
        let units: Vec<u16> = rest
            .chunks_exact(2)
            .map(|pair| u16::from_le_bytes([pair[0], pair[1]]))
            .collect();
        return String::from_utf16_lossy(&units);
    }
    let bytes = bytes.strip_prefix(&[0xEF, 0xBB, 0xBF]).unwrap_or(bytes);
    String::from_utf8_lossy(bytes).into_owned()
}

/// Result of [`expand_for_registration`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ExpandOutcome {
    pub urls: Vec<String>,
    /// Inputs whose range pattern was invalid or too large; kept verbatim in `urls`.
    pub invalid_ranges: Vec<String>,
    /// The result was cut at [`MAX_URLS_PER_REGISTRATION`].
    pub truncated: bool,
}

/// expand == false: urls unchanged. expand == true: each url through
/// `url_expansion::expand_url`; a url whose range is invalid or too large
/// (expand_url returns empty) is kept verbatim and listed in invalid_ranges.
/// The result is cut at MAX_URLS_PER_REGISTRATION (truncated = true).
pub fn expand_for_registration(urls: &[String], expand: bool) -> ExpandOutcome {
    let mut outcome = ExpandOutcome::default();

    for url in urls {
        let expanded = if expand {
            let expanded = expand_url(url);
            if expanded.is_empty() {
                outcome.invalid_ranges.push(url.clone());
                vec![url.clone()]
            } else {
                expanded
            }
        } else {
            vec![url.clone()]
        };

        for item in expanded {
            if outcome.urls.len() >= MAX_URLS_PER_REGISTRATION {
                outcome.truncated = true;
                return outcome;
            }
            outcome.urls.push(item);
        }
    }

    outcome
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_is_valid_download_url_http() {
        assert!(is_valid_download_url("http://example.com/file.zip"));
        assert!(is_valid_download_url("https://example.com/file.zip"));
    }

    #[test]
    fn test_is_valid_download_url_ftp() {
        assert!(is_valid_download_url("ftp://example.com/file.zip"));
        assert!(is_valid_download_url("ftps://example.com/file.zip"));
    }

    #[test]
    fn test_is_valid_download_url_invalid_scheme() {
        assert!(!is_valid_download_url("javascript:alert('test')"));
        assert!(!is_valid_download_url("data:text/plain,hello"));
        assert!(!is_valid_download_url("file:///etc/passwd"));
        assert!(!is_valid_download_url("mailto:user@example.com"));
    }

    #[test]
    fn test_is_valid_download_url_malformed() {
        assert!(!is_valid_download_url("not a url"));
        assert!(!is_valid_download_url("htp://typo.com"));
        assert!(!is_valid_download_url("://missing-scheme.com"));
        assert!(!is_valid_download_url(""));
    }

    #[test]
    fn test_is_valid_download_url_with_query_and_fragment() {
        assert!(is_valid_download_url(
            "https://example.com/file.zip?download=true#section"
        ));
        assert!(is_valid_download_url(
            "http://example.com/path/to/file?param1=value1&param2=value2"
        ));
    }

    #[test]
    fn parse_multiline_with_crlf() {
        let text = "https://a.example/1.zip\r\n\r\n  http://b.example/2.zip  \r\nftp://c.example/3.zip";
        assert_eq!(
            parse_url_text(text),
            vec![
                "https://a.example/1.zip",
                "http://b.example/2.zip",
                "ftp://c.example/3.zip",
            ]
        );
    }

    #[test]
    fn parse_tokens_inside_prose() {
        let text = "get <https://a.example/1.zip> or (http://b.example/2.zip) then \"https://c.example/3.zip\" now";
        assert_eq!(
            parse_url_text(text),
            vec![
                "https://a.example/1.zip",
                "http://b.example/2.zip",
                "https://c.example/3.zip",
            ]
        );
        assert_eq!(
            parse_url_text("see https://a.example/x.zip please"),
            vec!["https://a.example/x.zip"]
        );
    }

    #[test]
    fn parse_dedups_keeps_order() {
        let text = "https://b.example/\nhttps://a.example/\nhttps://b.example/\nhttps://a.example/ https://c.example/";
        assert_eq!(
            parse_url_text(text),
            vec!["https://b.example/", "https://a.example/", "https://c.example/"]
        );
    }

    #[test]
    fn parse_keeps_range_pattern_verbatim() {
        assert_eq!(
            parse_url_text("https://example.com/img[1-10].jpg"),
            vec!["https://example.com/img[1-10].jpg"]
        );
    }

    #[test]
    fn parse_ignores_non_download_schemes() {
        let text = "file:///etc/passwd\nmailto:user@example.com\njavascript:alert(1)\nplain words";
        assert!(parse_url_text(text).is_empty());
        assert!(parse_url_text("").is_empty());
    }

    #[test]
    fn parse_stops_at_cap() {
        let text: String = (0..MAX_URLS_PER_REGISTRATION + 10)
            .map(|i| format!("https://example.com/{i}\n"))
            .collect();
        assert_eq!(parse_url_text(&text).len(), MAX_URLS_PER_REGISTRATION);
    }

    #[test]
    fn internet_shortcut_reads_url_key() {
        let content = "[InternetShortcut]\r\nIconIndex=0\r\n url = https://example.com/file.zip \r\n";
        assert_eq!(
            parse_internet_shortcut(content).as_deref(),
            Some("https://example.com/file.zip")
        );
        let lower = "[ internetshortcut ]\nurl=http://example.com/a\n";
        assert_eq!(
            parse_internet_shortcut(lower).as_deref(),
            Some("http://example.com/a")
        );
    }

    #[test]
    fn internet_shortcut_requires_section() {
        assert_eq!(parse_internet_shortcut("URL=https://example.com/a\n"), None);
        assert_eq!(
            parse_internet_shortcut("[Other]\nURL=https://example.com/a\n"),
            None
        );
        // Invalid scheme in the section yields nothing.
        assert_eq!(
            parse_internet_shortcut("[InternetShortcut]\nURL=file:///etc/passwd\n"),
            None
        );
    }

    #[test]
    fn decode_utf16le_with_bom() {
        let mut bytes = vec![0xFF, 0xFE];
        for unit in "[InternetShortcut]\nURL=https://example.com/日本".encode_utf16() {
            bytes.extend_from_slice(&unit.to_le_bytes());
        }
        assert_eq!(
            decode_text_file(&bytes),
            "[InternetShortcut]\nURL=https://example.com/日本"
        );
    }

    #[test]
    fn decode_utf8_strips_bom_and_is_lossy() {
        assert_eq!(decode_text_file(b"\xEF\xBB\xBFhello"), "hello");
        assert_eq!(decode_text_file(b"plain"), "plain");
        assert_eq!(decode_text_file(b"a\xFFb"), "a\u{FFFD}b");
    }

    #[test]
    fn expand_without_flag_returns_unchanged() {
        let urls = vec!["https://example.com/img[1-3].jpg".to_string()];
        let out = expand_for_registration(&urls, false);
        assert_eq!(out.urls, urls);
        assert!(out.invalid_ranges.is_empty());
        assert!(!out.truncated);
    }

    #[test]
    fn expand_expands_ranges() {
        let urls = vec![
            "https://example.com/img[08-10].jpg".to_string(),
            "https://example.com/plain.zip".to_string(),
        ];
        let out = expand_for_registration(&urls, true);
        assert_eq!(
            out.urls,
            vec![
                "https://example.com/img08.jpg",
                "https://example.com/img09.jpg",
                "https://example.com/img10.jpg",
                "https://example.com/plain.zip",
            ]
        );
        assert!(out.invalid_ranges.is_empty());
        assert!(!out.truncated);
    }

    #[test]
    fn expand_keeps_invalid_range_verbatim() {
        let reversed = "https://example.com/img[5-1].jpg".to_string();
        let too_big = "https://example.com/img[1-5000].jpg".to_string();
        let out = expand_for_registration(&[reversed.clone(), too_big.clone()], true);
        assert_eq!(out.urls, vec![reversed.clone(), too_big.clone()]);
        assert_eq!(out.invalid_ranges, vec![reversed, too_big]);
        assert!(!out.truncated);
    }

    #[test]
    fn expand_truncates_at_cap() {
        let urls = vec![
            "https://example.com/a[1-600].jpg".to_string(),
            "https://example.com/b[1-600].jpg".to_string(),
        ];
        let out = expand_for_registration(&urls, true);
        assert_eq!(out.urls.len(), MAX_URLS_PER_REGISTRATION);
        assert!(out.truncated);
        assert_eq!(out.urls[600], "https://example.com/b1.jpg");
    }

    #[test]
    fn expand_exactly_at_cap_is_not_truncated() {
        let urls = vec!["https://example.com/a[1-1000].jpg".to_string()];
        let out = expand_for_registration(&urls, true);
        assert_eq!(out.urls.len(), MAX_URLS_PER_REGISTRATION);
        assert!(!out.truncated);
    }
}
