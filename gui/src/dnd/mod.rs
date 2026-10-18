//! Links, text and `.url` files dropped onto the main window. The OLE side
//! lives in `data_object` (Windows only); everything that decides what a drop
//! means is pure and tested here.

use ggg::util::url_input::{
    MAX_URLS_PER_REGISTRATION, decode_text_file, parse_internet_shortcut, parse_url_text,
};
use std::{
    collections::HashSet,
    fs::File,
    io::Read as _,
    path::{Path, PathBuf},
};

#[cfg(windows)]
mod data_object;

/// Larger `.url` files are not read; a real shortcut is a few hundred bytes.
pub const MAX_URL_FILE_BYTES: u64 = 64 * 1024;

// Values of the OLE `MK_*` key flags and `DROPEFFECT_*` effects, kept here so
// the decisions below compile and test on every platform. A test on Windows
// compares them with the system constants.
const MK_SHIFT: u32 = 0x0004;
const MK_CONTROL: u32 = 0x0008;
const EFFECT_NONE: u32 = 0;
const EFFECT_COPY: u32 = 1;
const EFFECT_LINK: u32 = 4;

/// What a drop carried, read while the OLE data object was still valid.
// Only the Windows drop target sends it.
#[cfg_attr(not(windows), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DropPayload {
    pub urls: Vec<String>,
    pub ctrl: bool,
    pub shift: bool,
}

/// `.url` files among `files` (path, size in bytes), in input order. Other
/// extensions and files over [`MAX_URL_FILE_BYTES`] are skipped.
// The non-Windows build has no drop target to call it.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn url_file_candidates(files: &[(PathBuf, u64)]) -> Vec<PathBuf> {
    files
        .iter()
        .filter(|(path, size)| *size <= MAX_URL_FILE_BYTES && has_url_extension(path))
        .map(|(path, _)| path.clone())
        .collect()
}

/// Whether `path` ends in `.url`, in any letter case.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn has_url_extension(path: &Path) -> bool {
    path.extension()
        .is_some_and(|ext| ext.eq_ignore_ascii_case("url"))
}

/// URLs inside the Internet Shortcut files at `paths`, in order and without
/// duplicates. Files that cannot be read or hold no URL are skipped.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn urls_from_url_files(paths: &[PathBuf]) -> Vec<String> {
    let mut seen = HashSet::new();
    let mut urls = Vec::new();
    for path in paths {
        if urls.len() >= MAX_URLS_PER_REGISTRATION {
            break;
        }
        let Some(url) = read_shortcut(path) else {
            continue;
        };
        if seen.insert(url.clone()) {
            urls.push(url);
        }
    }
    urls
}

fn read_shortcut(path: &Path) -> Option<String> {
    let mut bytes = Vec::new();
    // The limit guards a file that grew after its size was checked.
    File::open(path)
        .and_then(|file| file.take(MAX_URL_FILE_BYTES).read_to_end(&mut bytes))
        .map_err(|e| tracing::debug!("Cannot read dropped file {}: {e}", path.display()))
        .ok()?;
    parse_internet_shortcut(&decode_text_file(&bytes))
}

/// (Ctrl, Shift) from the `grfKeyState` of an OLE drag call.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn modifiers(key_state: u32) -> (bool, bool) {
    (key_state & MK_CONTROL != 0, key_state & MK_SHIFT != 0)
}

/// UTF-16 text of a clipboard format up to the first NUL, or all of `units`
/// when there is none. Sources are not always NUL-terminated within the block.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn wide_until_nul(units: &[u16]) -> String {
    let end = units.iter().position(|&u| u == 0).unwrap_or(units.len());
    String::from_utf16_lossy(&units[..end])
}

/// The first single URL in a `UniformResourceLocatorW` string.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn single_url(text: &str) -> Vec<String> {
    parse_url_text(text).into_iter().take(1).collect()
}

/// Runs `readers` in order and returns what the first one yields, so a later
/// (more expensive or less precise) format is not read once an earlier one
/// produced URLs.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn first_urls(readers: &mut [&mut dyn FnMut() -> Vec<String>]) -> Vec<String> {
    for reader in readers {
        let urls = reader();
        if !urls.is_empty() {
            return urls;
        }
    }
    Vec::new()
}

/// Effect to answer a drag with, from the effects the source allows. Copy is
/// preferred and move never chosen: the source would delete what it dragged.
#[cfg_attr(not(windows), allow(dead_code))]
pub fn choose_effect(allowed: u32, accepted: bool) -> u32 {
    if !accepted {
        EFFECT_NONE
    } else if allowed & EFFECT_COPY != 0 {
        EFFECT_COPY
    } else if allowed & EFFECT_LINK != 0 {
        EFFECT_LINK
    } else {
        EFFECT_NONE
    }
}

#[cfg(windows)]
pub use data_object::{DropRegistration, init_ole, register_drop_target};

/// Keeps OLE initialised on this thread while it lives.
#[cfg(not(windows))]
pub struct OleGuard;

#[cfg(not(windows))]
pub fn init_ole() -> Option<OleGuard> {
    None
}

/// Registration of the main window as a drop target; revoked on drop.
#[cfg(not(windows))]
pub struct DropRegistration;

#[cfg(not(windows))]
pub fn register_drop_target(
    _hwnd: isize,
    _sink: impl Fn(DropPayload) + 'static,
) -> Option<DropRegistration> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64) -> (PathBuf, u64) {
        (PathBuf::from(name), size)
    }

    #[test]
    fn only_url_files_are_candidates() {
        let files = [
            file("a.URL", 10),
            file("notes.txt", 10),
            file("b.url", 10),
            file("url", 10),
            file("c.url.txt", 10),
            file("d.Url", 10),
        ];
        assert_eq!(
            url_file_candidates(&files),
            [PathBuf::from("a.URL"), PathBuf::from("b.url"), PathBuf::from("d.Url")]
        );
    }

    #[test]
    fn oversized_url_file_is_skipped() {
        let files = [
            file("small.url", MAX_URL_FILE_BYTES),
            file("big.url", MAX_URL_FILE_BYTES + 1),
        ];
        assert_eq!(url_file_candidates(&files), [PathBuf::from("small.url")]);
    }

    #[test]
    fn url_files_are_read_in_order_without_duplicates() {
        let dir = tempfile::tempdir().unwrap();
        let write = |name: &str, bytes: &[u8]| {
            let path = dir.path().join(name);
            std::fs::write(&path, bytes).unwrap();
            path
        };
        let a = write("a.url", b"[InternetShortcut]\r\nURL=https://example.com/a\r\n");
        let wide: Vec<u8> = [0xFEFF_u16]
            .into_iter()
            .chain("[InternetShortcut]\r\nURL=https://example.com/b\r\n".encode_utf16())
            .flat_map(u16::to_le_bytes)
            .collect();
        let b = write("b.url", &wide);
        let again = write("c.url", b"[InternetShortcut]\nURL=https://example.com/a\n");
        let broken = write("d.url", b"not a shortcut");
        let missing = dir.path().join("missing.url");
        assert_eq!(
            urls_from_url_files(&[a, b, again, broken, missing]),
            ["https://example.com/a", "https://example.com/b"]
        );
    }

    #[test]
    fn modifiers_come_from_key_state_bits() {
        assert_eq!(modifiers(0), (false, false));
        assert_eq!(modifiers(MK_CONTROL), (true, false));
        assert_eq!(modifiers(MK_SHIFT), (false, true));
        // Mouse buttons (MK_LBUTTON = 1, MK_ALT = 0x20) are ignored.
        assert_eq!(modifiers(MK_CONTROL | MK_SHIFT | 0x21), (true, true));
        assert_eq!(modifiers(0x21), (false, false));
    }

    #[test]
    fn wide_text_stops_at_the_first_nul() {
        let units: Vec<u16> = "https://example.com/a\0junk".encode_utf16().collect();
        assert_eq!(wide_until_nul(&units), "https://example.com/a");
        let units: Vec<u16> = "no terminator".encode_utf16().collect();
        assert_eq!(wide_until_nul(&units), "no terminator");
        assert_eq!(wide_until_nul(&[]), "");
    }

    #[test]
    fn url_format_yields_one_url() {
        assert_eq!(
            single_url("https://example.com/a\nhttps://example.com/b"),
            ["https://example.com/a"]
        );
        assert!(single_url("javascript:void(0)").is_empty());
    }

    #[test]
    fn first_format_with_urls_wins() {
        let mut calls = Vec::new();
        let mut url = || {
            calls.push("url");
            Vec::new()
        };
        let mut text = || vec!["https://example.com/text".to_string()];
        let mut files = || panic!("files must not be read once text yielded URLs");
        let urls = first_urls(&mut [&mut url, &mut text, &mut files]);
        assert_eq!(urls, ["https://example.com/text"]);
        assert_eq!(calls, ["url"]);
        assert!(first_urls(&mut []).is_empty());
    }

    #[test]
    fn copy_is_preferred_and_move_never_chosen() {
        assert_eq!(choose_effect(EFFECT_COPY | 2 | EFFECT_LINK, true), EFFECT_COPY);
        assert_eq!(choose_effect(EFFECT_LINK | 2, true), EFFECT_LINK);
        assert_eq!(choose_effect(2, true), EFFECT_NONE);
        assert_eq!(choose_effect(EFFECT_COPY, false), EFFECT_NONE);
    }

    #[cfg(windows)]
    #[test]
    fn constants_match_the_system_values() {
        use windows::Win32::System::{Ole, SystemServices};
        assert_eq!(MK_SHIFT, SystemServices::MK_SHIFT.0);
        assert_eq!(MK_CONTROL, SystemServices::MK_CONTROL.0);
        assert_eq!(EFFECT_NONE, Ole::DROPEFFECT_NONE.0);
        assert_eq!(EFFECT_COPY, Ole::DROPEFFECT_COPY.0);
        assert_eq!(EFFECT_LINK, Ole::DROPEFFECT_LINK.0);
    }
}
