//! Which download rows a front end shows for the selected tree node.

use crate::download::task::DownloadTask;
use std::collections::HashMap;

/// The node selected in the folder tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueueView {
    /// A folder, by id.
    Folder(String),
    /// The "Completed" node.
    History,
}

/// Rows for `view`.
///
/// - `Folder`: that folder's tasks in queue order, unfiltered (search applies
///   to History only, as in the TUI). An unknown folder yields no rows.
/// - `History`: case-insensitive substring match of `search` against
///   `filename`; an empty search returns everything.
pub fn view_rows<'a>(
    folder_downloads: &'a HashMap<String, Vec<DownloadTask>>,
    history: &'a [DownloadTask],
    view: &QueueView,
    search: &str,
) -> Vec<&'a DownloadTask> {
    match view {
        QueueView::History => {
            if search.is_empty() {
                return history.iter().collect();
            }
            // Lowercase the query once, not per item.
            let query = search.to_lowercase();
            history
                .iter()
                .filter(|task| task.filename.to_lowercase().contains(&query))
                .collect()
        }
        QueueView::Folder(folder_id) => folder_downloads
            .get(folder_id)
            .map(|tasks| tasks.iter().collect())
            .unwrap_or_default(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn task(name: &str) -> DownloadTask {
        let mut task = DownloadTask::new("https://host/f".to_string(), PathBuf::from("."));
        task.filename = name.to_string();
        task
    }

    fn names(rows: &[&DownloadTask]) -> Vec<String> {
        rows.iter().map(|t| t.filename.clone()).collect()
    }

    #[test]
    fn folder_view_returns_queue_order() {
        let mut folders = HashMap::new();
        folders.insert("a".to_string(), vec![task("z.bin"), task("a.bin"), task("m.bin")]);
        folders.insert("b".to_string(), vec![task("other.bin")]);
        let history = vec![task("old.bin")];

        // A search string must not filter folder rows.
        let rows = view_rows(&folders, &history, &QueueView::Folder("a".into()), "zzz");
        assert_eq!(names(&rows), ["z.bin", "a.bin", "m.bin"]);
    }

    #[test]
    fn history_search_is_case_insensitive() {
        let folders = HashMap::new();
        let history = vec![task("Movie.MKV"), task("notes.txt"), task("my movie.mp4")];

        let rows = view_rows(&folders, &history, &QueueView::History, "MoVie");
        assert_eq!(names(&rows), ["Movie.MKV", "my movie.mp4"]);
    }

    #[test]
    fn history_empty_search_returns_all() {
        let folders = HashMap::new();
        let history = vec![task("one"), task("two")];

        let rows = view_rows(&folders, &history, &QueueView::History, "");
        assert_eq!(names(&rows), ["one", "two"]);
    }

    #[test]
    fn unknown_folder_is_empty() {
        let mut folders = HashMap::new();
        folders.insert("a".to_string(), vec![task("x")]);
        let history = vec![task("h")];

        let rows = view_rows(&folders, &history, &QueueView::Folder("missing".into()), "");
        assert!(rows.is_empty());
    }
}
