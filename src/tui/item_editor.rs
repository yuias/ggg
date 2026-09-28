//! Pure model for the item editor dialog (Save path + Referrer). Kept free of
//! `TuiApp` so it can be unit-tested without a TUI harness.

use std::path::PathBuf;
use uuid::Uuid;

use crate::download::task::DownloadTask;
use crate::ipc::control::protocol::HeaderEdit;

/// One editable text field; `original` is the prefill used to detect changes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TextField {
    pub original: String,
    pub value: String,
}

impl TextField {
    pub fn new(original: impl Into<String>) -> Self {
        let original = original.into();
        let value = original.clone();
        Self { original, value }
    }

    /// Compares trimmed values so surrounding whitespace typed by mistake
    /// does not count as an edit.
    pub fn is_changed(&self) -> bool {
        self.value.trim() != self.original.trim()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ItemEditorField {
    SavePath,
    Referrer,
}

impl ItemEditorField {
    pub fn next(self) -> Self {
        match self {
            ItemEditorField::SavePath => ItemEditorField::Referrer,
            ItemEditorField::Referrer => ItemEditorField::SavePath,
        }
    }

    pub fn prev(self) -> Self {
        // Only two fields, so prev and next coincide.
        self.next()
    }
}

#[derive(Debug, Clone)]
pub struct ItemEditorState {
    /// From `get_target_download_ids()` at open time (selection or focused item).
    pub targets: Vec<Uuid>,
    pub focus: ItemEditorField,
    pub save_path: TextField,
    pub referrer: TextField,
    /// Non-Referer headers of the focused task, name-sorted, values already masked.
    pub other_headers: Vec<(String, String)>,
}

impl ItemEditorState {
    /// Prefill from the focused task; `targets` may contain more ids (multi-select).
    pub fn from_task(task: &DownloadTask, targets: Vec<Uuid>) -> Self {
        let referrer = find_header(task, "Referer").unwrap_or("");
        Self {
            targets,
            focus: ItemEditorField::Referrer,
            save_path: TextField::new(task.save_path.to_string_lossy().into_owned()),
            referrer: TextField::new(referrer),
            other_headers: masked_headers(task),
        }
    }

    /// Value of the focused field.
    pub fn focused_mut(&mut self) -> &mut String {
        match self.focus {
            ItemEditorField::SavePath => &mut self.save_path.value,
            ItemEditorField::Referrer => &mut self.referrer.value,
        }
    }

    /// None when the referrer is unchanged. Empty (after trim) => clear_referrer = true,
    /// so the folder/app referrer policy applies again at start time.
    pub fn header_edit(&self) -> Option<HeaderEdit> {
        if !self.referrer.is_changed() {
            return None;
        }
        let trimmed = self.referrer.value.trim();
        if trimmed.is_empty() {
            Some(HeaderEdit { clear_referrer: true, ..Default::default() })
        } else {
            Some(HeaderEdit { referrer: Some(trimmed.to_string()), ..Default::default() })
        }
    }

    /// None when unchanged or empty (empty save path is not a valid edit).
    pub fn new_save_path(&self) -> Option<PathBuf> {
        if !self.save_path.is_changed() {
            return None;
        }
        let trimmed = self.save_path.value.trim();
        if trimmed.is_empty() {
            None
        } else {
            Some(PathBuf::from(trimmed))
        }
    }

    /// Err(message id) when the referrer contains control characters (CR/LF etc.),
    /// which would fail in `HttpClient::build_headers` at download time.
    pub fn validate(&self) -> Result<(), &'static str> {
        if is_valid_referrer(&self.referrer.value) {
            Ok(())
        } else {
            Err("dialog-item-edit-invalid-referrer")
        }
    }
}

/// Case-insensitive lookup of a header in `task.headers`. `DownloadTask` has
/// no such getter (only `set_header`/`remove_header`, which mutate).
pub fn find_header<'a>(task: &'a DownloadTask, name: &str) -> Option<&'a str> {
    task.headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case(name))
        .map(|(_, v)| v.as_str())
}

/// Non-Referer headers, sorted by name (case-insensitive), values masked with
/// `crate::util::redact::display_header_value`.
pub fn masked_headers(task: &DownloadTask) -> Vec<(String, String)> {
    let mut headers: Vec<(String, String)> = task
        .headers
        .iter()
        .filter(|(k, _)| !k.eq_ignore_ascii_case("Referer"))
        .map(|(k, v)| (k.clone(), crate::util::redact::display_header_value(k, v).to_string()))
        .collect();
    headers.sort_by(|(a, _), (b, _)| a.to_ascii_lowercase().cmp(&b.to_ascii_lowercase()));
    headers
}

/// Referrer check shared by the editor and the add dialog: true when it has no control chars.
pub fn is_valid_referrer(value: &str) -> bool {
    !value.chars().any(|c| c.is_control())
}

/// Sets `Referer` on a newly built task when `referrer` (trimmed) is non-empty; no-op otherwise.
pub fn apply_add_referrer(task: &mut DownloadTask, referrer: &str) {
    let trimmed = referrer.trim();
    if !trimmed.is_empty() {
        task.set_header("Referer", trimmed);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::download::task::DownloadTask;
    use std::path::PathBuf;

    fn task_with_headers(headers: &[(&str, &str)]) -> DownloadTask {
        let mut task = DownloadTask::new("https://example.org/f".to_string(), PathBuf::from("/tmp/f"));
        for (name, value) in headers {
            task.set_header(name, value);
        }
        task
    }

    #[test]
    fn from_task_prefills_referrer_case_insensitively() {
        let task = task_with_headers(&[("referer", "https://a.example/")]);
        let state = ItemEditorState::from_task(&task, vec![task.id]);
        assert_eq!(state.referrer.value, "https://a.example/");
        assert_eq!(state.referrer.original, "https://a.example/");
    }

    #[test]
    fn unchanged_fields_produce_no_edit() {
        let task = task_with_headers(&[("Referer", "https://a.example/")]);
        let state = ItemEditorState::from_task(&task, vec![task.id]);
        assert_eq!(state.header_edit(), None);
        assert_eq!(state.new_save_path(), None);
    }

    #[test]
    fn changed_referrer_sets_referrer() {
        let task = task_with_headers(&[]);
        let mut state = ItemEditorState::from_task(&task, vec![task.id]);
        state.referrer.value = "https://b.example/".to_string();
        assert_eq!(
            state.header_edit(),
            Some(HeaderEdit { referrer: Some("https://b.example/".to_string()), ..Default::default() })
        );
    }

    #[test]
    fn emptied_referrer_clears_it() {
        let task = task_with_headers(&[("Referer", "https://a.example/")]);
        let mut state = ItemEditorState::from_task(&task, vec![task.id]);
        state.referrer.value = "".to_string();
        assert_eq!(
            state.header_edit(),
            Some(HeaderEdit { clear_referrer: true, ..Default::default() })
        );
    }

    #[test]
    fn whitespace_only_change_is_not_a_change() {
        let task = task_with_headers(&[("Referer", "https://a.example/")]);
        let mut state = ItemEditorState::from_task(&task, vec![task.id]);
        state.referrer.value = "  https://a.example/  ".to_string();
        assert_eq!(state.header_edit(), None);
    }

    #[test]
    fn empty_save_path_is_not_an_edit() {
        let task = task_with_headers(&[]);
        let mut state = ItemEditorState::from_task(&task, vec![task.id]);
        state.save_path.value = "   ".to_string();
        assert_eq!(state.new_save_path(), None);
    }

    #[test]
    fn referrer_with_newline_fails_validation() {
        let task = task_with_headers(&[]);
        let mut state = ItemEditorState::from_task(&task, vec![task.id]);
        state.referrer.value = "https://a.example/\r\nEvil-Header: x".to_string();
        assert_eq!(state.validate(), Err("dialog-item-edit-invalid-referrer"));
    }

    #[test]
    fn masked_headers_hides_cookie_and_excludes_referer() {
        let task = task_with_headers(&[
            ("Referer", "https://a.example/"),
            ("Cookie", "session=secret"),
            ("X-Custom", "value"),
        ]);
        let masked = masked_headers(&task);
        assert_eq!(
            masked,
            vec![
                ("Cookie".to_string(), "<redacted>".to_string()),
                ("X-Custom".to_string(), "value".to_string()),
            ]
        );
    }

    #[test]
    fn focus_wraps_both_ways() {
        assert_eq!(ItemEditorField::SavePath.next(), ItemEditorField::Referrer);
        assert_eq!(ItemEditorField::Referrer.next(), ItemEditorField::SavePath);
        assert_eq!(ItemEditorField::SavePath.prev(), ItemEditorField::Referrer);
        assert_eq!(ItemEditorField::Referrer.prev(), ItemEditorField::SavePath);
    }

    #[test]
    fn add_referrer_sets_header_when_non_empty() {
        let mut task = task_with_headers(&[]);
        apply_add_referrer(&mut task, "  https://a.example/  ");
        assert_eq!(find_header(&task, "Referer"), Some("https://a.example/"));
    }

    #[test]
    fn add_referrer_empty_is_noop() {
        let mut task = task_with_headers(&[]);
        apply_add_referrer(&mut task, "   ");
        assert_eq!(find_header(&task, "Referer"), None);
    }
}
