/// Scan a scripts directory for `.js` filenames, sorted. Returns an empty list
/// on any error. Used to populate `TuiState::cached_script_files`.
pub fn scan_script_files(dir: &std::path::Path) -> Vec<String> {
    match std::fs::read_dir(dir) {
        Ok(entries) => {
            let mut files: Vec<String> = entries
                .filter_map(|e| e.ok())
                .filter(|e| e.path().extension().and_then(|s| s.to_str()) == Some("js"))
                .filter_map(|e| e.file_name().to_str().map(|s| s.to_string()))
                .collect();
            files.sort();
            files
        }
        Err(_) => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scan_lists_only_js_sorted() {
        let dir = tempfile::TempDir::new().unwrap();
        for name in ["b.js", "a.js", "notes.txt", "c.json", "noext"] {
            std::fs::write(dir.path().join(name), "").unwrap();
        }
        // A subdirectory named like a script must not break the scan.
        std::fs::create_dir(dir.path().join("sub")).unwrap();

        assert_eq!(scan_script_files(dir.path()), vec!["a.js", "b.js"]);
    }

    #[test]
    fn scan_missing_dir_is_empty() {
        let dir = tempfile::TempDir::new().unwrap();
        assert!(scan_script_files(&dir.path().join("missing")).is_empty());
    }
}
