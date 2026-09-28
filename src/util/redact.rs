//! Masking rules for header values that a user should not see echoed back
//! on their own terminal (`debug task`, `script test`), for example when
//! sharing a screen or a copy-pasted log.

/// Placeholder printed instead of a sensitive header value.
pub const REDACTED: &str = "<redacted>";

/// Header names matched exactly (case-insensitive).
const SENSITIVE_EXACT: &[&str] = &[
    "authorization",       // Basic/Bearer credentials
    "proxy-authorization", // proxy credentials
    "cookie",              // session cookies
    "set-cookie",          // sets session cookies; can appear in script contexts
];

/// Substrings matched case-insensitively anywhere in the header name.
const SENSITIVE_SUBSTRINGS: &[&str] = &[
    "token",  // X-Auth-Token, X-CSRF-Token, X-Access-Token: bearer-style secrets
    "key",    // X-API-Key, Api-Key
    "secret", // X-Client-Secret and similar
    "auth",   // X-Auth-User, custom auth schemes (also covers the two exact auth names)
];

/// Case-insensitive. Known harmless over-matches: `Sec-WebSocket-Key`,
/// `WWW-Authenticate`. `Referer`/`Referrer` match none of these rules.
pub fn is_sensitive_header(name: &str) -> bool {
    let lower = name.to_ascii_lowercase();
    SENSITIVE_EXACT.contains(&lower.as_str())
        || SENSITIVE_SUBSTRINGS.iter().any(|s| lower.contains(s))
}

/// `REDACTED` for sensitive names, otherwise `value`.
pub fn display_header_value<'a>(name: &str, value: &'a str) -> &'a str {
    if is_sensitive_header(name) {
        REDACTED
    } else {
        value
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sensitive_exact_names_case_insensitive() {
        assert!(is_sensitive_header("AUTHORIZATION"));
        assert!(is_sensitive_header("cookie"));
        assert!(is_sensitive_header("Set-Cookie"));
        assert!(is_sensitive_header("Proxy-Authorization"));
    }

    #[test]
    fn sensitive_substrings() {
        assert!(is_sensitive_header("X-Auth-Token"));
        assert!(is_sensitive_header("x-api-key"));
        assert!(is_sensitive_header("Client-Secret"));
        assert!(is_sensitive_header("X-Auth-User"));
    }

    #[test]
    fn referer_and_common_headers_visible() {
        // "Referer" and "Referrer" must not be caught by any substring rule
        // (in particular not by "key" or "auth").
        assert!(!is_sensitive_header("Referer"));
        assert!(!is_sensitive_header("Referrer"));
        assert!(!is_sensitive_header("User-Agent"));
        assert!(!is_sensitive_header("Accept"));
        assert!(!is_sensitive_header("Range"));
        assert!(!is_sensitive_header("Keep-Alive"));
    }

    #[test]
    fn display_header_value_masks_only_sensitive() {
        assert_eq!(display_header_value("Cookie", "sid=abc"), REDACTED);
        assert_eq!(display_header_value("Referer", "https://example.com"), "https://example.com");
    }
}
