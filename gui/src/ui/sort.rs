//! Display-only ordering of the table rows. The queue itself is never reordered.

use super::queue_table::progress_fraction;
use crate::gui_config::SortState;
use ggg::download::task::{DownloadStatus, DownloadTask};
use std::cmp::Ordering;

/// Sortable table columns, named by the ids `GuiConfig.sort` stores; those
/// strings must stay stable across versions.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SortColumn {
    Name,
    Status,
    Size,
    Progress,
    Speed,
    Eta,
    Added,
    Completed,
    Url,
}

impl SortColumn {
    pub fn from_id(id: &str) -> Option<Self> {
        Some(match id {
            "name" => SortColumn::Name,
            "status" => SortColumn::Status,
            "size" => SortColumn::Size,
            "progress" => SortColumn::Progress,
            "speed" => SortColumn::Speed,
            "eta" => SortColumn::Eta,
            "added" => SortColumn::Added,
            "completed" => SortColumn::Completed,
            "url" => SortColumn::Url,
            _ => return None,
        })
    }
}

/// Header click cycle: ascending, descending, then queue order (`None`).
/// A click on another column always starts at ascending.
pub fn next_sort(current: Option<&SortState>, column: &str) -> Option<SortState> {
    match current {
        Some(state) if state.column == column && state.ascending => Some(SortState {
            column: column.to_string(),
            ascending: false,
        }),
        Some(state) if state.column == column => None,
        _ => Some(SortState {
            column: column.to_string(),
            ascending: true,
        }),
    }
}

/// Sorts `rows` in place; an unknown column leaves the queue order alone.
///
/// The sort is stable and descending flips the comparison rather than the
/// result, so rows that compare equal keep their queue order either way.
pub fn sort_rows(rows: &mut Vec<&DownloadTask>, column: &str, ascending: bool) {
    let Some(column) = SortColumn::from_id(column) else {
        return;
    };
    rows.sort_by(|a, b| {
        let ordering = compare(column, a, b);
        if ascending { ordering } else { ordering.reverse() }
    });
}

fn compare(column: SortColumn, a: &DownloadTask, b: &DownloadTask) -> Ordering {
    match column {
        SortColumn::Name => a.filename.to_lowercase().cmp(&b.filename.to_lowercase()),
        SortColumn::Status => status_rank(a.status).cmp(&status_rank(b.status)),
        SortColumn::Size => a.size.cmp(&b.size),
        SortColumn::Progress => {
            // Unknown progress sorts below 0 %.
            let key = |t: &DownloadTask| progress_fraction(t).unwrap_or(-1.0);
            key(a).total_cmp(&key(b))
        }
        SortColumn::Speed => {
            let key = |t: &DownloadTask| t.speed().unwrap_or(-1.0);
            key(a).total_cmp(&key(b))
        }
        SortColumn::Eta => a.eta_seconds().cmp(&b.eta_seconds()),
        SortColumn::Added => a.created_at.cmp(&b.created_at),
        SortColumn::Completed => a.completed_at.cmp(&b.completed_at),
        SortColumn::Url => a.url.to_lowercase().cmp(&b.url.to_lowercase()),
    }
}

/// Active work first; the textual label is localized and would reorder per language.
fn status_rank(status: DownloadStatus) -> u8 {
    match status {
        DownloadStatus::Downloading => 0,
        DownloadStatus::Pending => 1,
        DownloadStatus::Paused => 2,
        DownloadStatus::Error => 3,
        DownloadStatus::Completed => 4,
        DownloadStatus::Deleted => 5,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn task(name: &str, size: Option<u64>) -> DownloadTask {
        let mut task = DownloadTask::new(format!("https://host/{name}"), PathBuf::from("."));
        task.filename = name.to_string();
        task.size = size;
        task
    }

    fn names(rows: &[&DownloadTask]) -> Vec<String> {
        rows.iter().map(|t| t.filename.clone()).collect()
    }

    #[test]
    fn third_click_restores_queue_order() {
        let tasks = [task("b", Some(2)), task("c", Some(3)), task("a", Some(1))];
        let queue: Vec<&DownloadTask> = tasks.iter().collect();

        let first = next_sort(None, "name");
        assert_eq!(first, Some(SortState { column: "name".into(), ascending: true }));
        let second = next_sort(first.as_ref(), "name");
        assert_eq!(second, Some(SortState { column: "name".into(), ascending: false }));
        let third = next_sort(second.as_ref(), "name");
        assert_eq!(third, None);

        let mut rows = queue.clone();
        sort_rows(&mut rows, "name", true);
        assert_eq!(names(&rows), ["a", "b", "c"]);
        sort_rows(&mut rows, "name", false);
        assert_eq!(names(&rows), ["c", "b", "a"]);

        // Without a sort state the caller keeps the queue order untouched.
        assert_eq!(names(&queue), ["b", "c", "a"]);
    }

    #[test]
    fn another_column_starts_ascending() {
        let by_name_desc = SortState { column: "name".into(), ascending: false };
        assert_eq!(
            next_sort(Some(&by_name_desc), "size"),
            Some(SortState { column: "size".into(), ascending: true })
        );
    }

    #[test]
    fn ties_keep_queue_order() {
        let tasks = [
            task("x1", Some(5)),
            task("x2", Some(1)),
            task("x3", Some(5)),
            task("x4", Some(1)),
        ];
        let mut rows: Vec<&DownloadTask> = tasks.iter().collect();

        sort_rows(&mut rows, "size", true);
        assert_eq!(names(&rows), ["x2", "x4", "x1", "x3"]);
        sort_rows(&mut rows, "size", false);
        assert_eq!(names(&rows), ["x1", "x3", "x2", "x4"]);
    }

    #[test]
    fn unknown_column_keeps_queue_order() {
        let tasks = [task("b", None), task("a", None)];
        let mut rows: Vec<&DownloadTask> = tasks.iter().collect();
        sort_rows(&mut rows, "nonsense", true);
        assert_eq!(names(&rows), ["b", "a"]);
    }

    #[test]
    fn names_sort_case_insensitively() {
        let tasks = [task("beta", None), task("Alpha", None), task("gamma", None)];
        let mut rows: Vec<&DownloadTask> = tasks.iter().collect();
        sort_rows(&mut rows, "name", true);
        assert_eq!(names(&rows), ["Alpha", "beta", "gamma"]);
    }
}
