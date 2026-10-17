//! UI -> core actions.
//!
//! Every core mutation is async while the winit loop must never wait, so the
//! UI hands an [`Action`] to [`Actions::dispatch`], which runs it on the
//! runtime. The outcome comes back as an [`ActionResult`] on a channel the UI
//! drains once per frame.

// The views that dispatch most of these actions are added after the bridge.
#![allow(dead_code)]

use crate::i18n::Tr;
use fluent_bundle::FluentArgs;
use ggg::{
    AppState, Config,
    download::{manager::DownloadManager, task::DownloadTask},
    ipc::control::protocol::HeaderEdit,
    ui::{
        commands::{Command, CommandError, CommandResponse, handle_command},
        item_edit::{ItemEditReport, apply_item_edit},
        queue_ops::{self, AddReport, AddRequest, OpReport},
    },
};
use std::{
    path::PathBuf,
    sync::{
        Arc,
        mpsc::{self, Receiver, Sender},
    },
};
use tokio::{runtime::Handle, sync::Notify};
use uuid::Uuid;

pub enum Action {
    Toggle { reference: Option<Uuid>, targets: Vec<Uuid> },
    Retry { targets: Vec<Uuid> },
    Delete { targets: Vec<Uuid> },
    Restore { batch: Vec<DownloadTask> },
    MoveToFolder { targets: Vec<Uuid>, folder_id: String },
    EditItems {
        targets: Vec<Uuid>,
        header_edit: Option<HeaderEdit>,
        new_save_path: Option<PathBuf>,
        /// Only ever set for a single target that has not started.
        new_filename: Option<String>,
    },
    Add(AddRequest),
    StartFolder(String),
    StopFolder(String),
    DeleteFolderTasks(String),
    ResumeAll,
    PauseAll,
    ClearHistory,
    ApplySettings { commands: Vec<Command> },
    CreateFolder,
    DeleteFolder(String),
    ReloadScripts,
    ReloadConfig,
}

/// Which action a result belongs to, for results that share a shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionKind {
    Toggle,
    Retry,
    Delete,
    Restore,
    MoveToFolder,
    EditItems,
    Add,
    StartFolder,
    StopFolder,
    DeleteFolderTasks,
    ResumeAll,
    PauseAll,
    ClearHistory,
    ApplySettings,
    CreateFolder,
    DeleteFolder,
    ReloadScripts,
    ReloadConfig,
}

impl Action {
    pub fn kind(&self) -> ActionKind {
        match self {
            Action::Toggle { .. } => ActionKind::Toggle,
            Action::Retry { .. } => ActionKind::Retry,
            Action::Delete { .. } => ActionKind::Delete,
            Action::Restore { .. } => ActionKind::Restore,
            Action::MoveToFolder { .. } => ActionKind::MoveToFolder,
            Action::EditItems { .. } => ActionKind::EditItems,
            Action::Add(_) => ActionKind::Add,
            Action::StartFolder(_) => ActionKind::StartFolder,
            Action::StopFolder(_) => ActionKind::StopFolder,
            Action::DeleteFolderTasks(_) => ActionKind::DeleteFolderTasks,
            Action::ResumeAll => ActionKind::ResumeAll,
            Action::PauseAll => ActionKind::PauseAll,
            Action::ClearHistory => ActionKind::ClearHistory,
            Action::ApplySettings { .. } => ActionKind::ApplySettings,
            Action::CreateFolder => ActionKind::CreateFolder,
            Action::DeleteFolder(_) => ActionKind::DeleteFolder,
            Action::ReloadScripts => ActionKind::ReloadScripts,
            Action::ReloadConfig => ActionKind::ReloadConfig,
        }
    }
}

#[derive(Debug)]
pub enum ActionResult {
    Report { action: ActionKind, report: OpReport },
    /// Tasks a delete removed, for the undo stack.
    Deleted { batch: Vec<DownloadTask>, report: OpReport },
    Added(AddReport),
    Edited(ItemEditReport),
    /// `applied` commands succeeded; `error` holds the index of the first one
    /// that failed (the rest were not run). `config` is read back afterwards.
    Settings {
        applied: usize,
        error: Option<(usize, CommandError)>,
        config: Arc<Config>,
    },
    /// `config` is read back after the command, so a window that shows the
    /// configuration does not have to wait for the next snapshot.
    Command {
        action: ActionKind,
        result: Result<serde_json::Value, CommandError>,
        config: Arc<Config>,
    },
}

/// Receiving end of the results channel; the UI thread owns it.
pub struct ActionResults(Receiver<ActionResult>);

impl ActionResults {
    /// Results that arrived since the last call, without blocking.
    pub fn drain(&self) -> Vec<ActionResult> {
        self.0.try_iter().collect()
    }
}

pub struct Actions {
    rt: Handle,
    state: AppState,
    manager: DownloadManager,
    tx: Sender<ActionResult>,
    /// Asks the snapshot task for an immediate rebuild.
    refresh: Arc<Notify>,
    wake: Arc<dyn Fn() + Send + Sync>,
}

impl Actions {
    /// `wake` runs after each result is queued and must not block (it posts an
    /// event to the UI loop).
    pub fn new(
        rt: Handle,
        state: AppState,
        manager: DownloadManager,
        refresh: Arc<Notify>,
        wake: impl Fn() + Send + Sync + 'static,
    ) -> (Self, ActionResults) {
        let (tx, rx) = mpsc::channel();
        let actions = Self {
            rt,
            state,
            manager,
            tx,
            refresh,
            wake: Arc::new(wake),
        };
        (actions, ActionResults(rx))
    }

    /// Runs `action` on the runtime and returns immediately.
    pub fn dispatch(&self, action: Action) {
        let state = self.state.clone();
        let manager = self.manager.clone();
        let tx = self.tx.clone();
        let refresh = self.refresh.clone();
        let wake = self.wake.clone();

        self.rt.spawn(async move {
            let result = run(action, &manager, &state).await;

            // Same as the TUI: persist after each mutation, so a crash loses
            // at most the action in flight.
            if let Err(e) = manager.save_queue_to_folders().await {
                tracing::error!("Failed to save the queue after an action: {e:#}");
            }
            refresh.notify_one();
            // The receiver is gone only while the app shuts down.
            let _ = tx.send(result);
            wake();
        });
    }
}

async fn run(action: Action, manager: &DownloadManager, state: &AppState) -> ActionResult {
    let kind = action.kind();
    match action {
        Action::Toggle { reference, targets } => ActionResult::Report {
            action: kind,
            report: queue_ops::toggle_tasks(manager, state, reference, &targets).await,
        },
        Action::Retry { targets } => ActionResult::Report {
            action: kind,
            report: queue_ops::retry_tasks(manager, state, &targets).await,
        },
        Action::Delete { targets } => {
            let (batch, report) = queue_ops::delete_tasks(manager, &targets).await;
            ActionResult::Deleted { batch, report }
        }
        Action::Restore { batch } => ActionResult::Report {
            action: kind,
            report: queue_ops::restore_tasks(manager, state, batch).await,
        },
        Action::MoveToFolder { targets, folder_id } => {
            let mut report = OpReport::default();
            for id in targets {
                let command = Command::ChangeFolder {
                    id: id.to_string(),
                    folder_id: folder_id.clone(),
                };
                match handle_command(command, state.clone(), manager.clone()).await {
                    CommandResponse::Success { .. } => report.affected += 1,
                    CommandResponse::Error(e) => {
                        tracing::warn!("Failed to move download {id}: {e}");
                        report.failed += 1;
                    }
                }
            }
            ActionResult::Report { action: kind, report }
        }
        Action::EditItems { targets, header_edit, new_save_path, new_filename } => {
            ActionResult::Edited(
                apply_item_edit(
                    manager,
                    &targets,
                    header_edit.as_ref(),
                    new_save_path.as_deref(),
                    new_filename.as_deref(),
                )
                .await,
            )
        }
        Action::Add(request) => ActionResult::Added(queue_ops::add_urls(manager, state, request).await),
        Action::StartFolder(folder_id) => {
            let affected = manager
                .start_folder_tasks(&folder_id, state.script_sender.clone(), state.config.clone())
                .await;
            count_report(kind, affected)
        }
        Action::StopFolder(folder_id) => {
            count_report(kind, manager.stop_folder_tasks(&folder_id).await)
        }
        Action::DeleteFolderTasks(folder_id) => ActionResult::Report {
            action: kind,
            report: queue_ops::delete_folder_tasks(manager, &folder_id).await,
        },
        Action::ResumeAll => {
            let affected = manager
                .resume_all(state.script_sender.clone(), state.config.clone())
                .await;
            count_report(kind, affected)
        }
        Action::PauseAll => count_report(kind, manager.pause_all().await),
        Action::ClearHistory => {
            let affected = manager.history_len().await;
            manager.clear_history().await;
            count_report(kind, affected)
        }
        Action::ApplySettings { commands } => {
            let mut applied = 0;
            let mut error = None;
            for (index, command) in commands.into_iter().enumerate() {
                match handle_command(command, state.clone(), manager.clone()).await {
                    CommandResponse::Success { .. } => applied += 1,
                    CommandResponse::Error(e) => {
                        // Earlier commands stay applied; the caller resends
                        // only the remaining difference.
                        error = Some((index, e));
                        break;
                    }
                }
            }
            let config = Arc::new(state.config.read().await.clone());
            ActionResult::Settings { applied, error, config }
        }
        Action::CreateFolder => command_result(kind, Command::CreateFolder, manager, state).await,
        Action::DeleteFolder(folder_id) => {
            command_result(kind, Command::DeleteFolder { folder_id }, manager, state).await
        }
        Action::ReloadScripts => command_result(kind, Command::ReloadScripts, manager, state).await,
        Action::ReloadConfig => command_result(kind, Command::ReloadConfig, manager, state).await,
    }
}

fn count_report(action: ActionKind, affected: usize) -> ActionResult {
    ActionResult::Report {
        action,
        report: OpReport { affected, ..OpReport::default() },
    }
}

async fn command_result(
    action: ActionKind,
    command: Command,
    manager: &DownloadManager,
    state: &AppState,
) -> ActionResult {
    let result = match handle_command(command, state.clone(), manager.clone()).await {
        CommandResponse::Success { data } => Ok(data),
        CommandResponse::Error(e) => Err(e),
    };
    let config = Arc::new(state.config.read().await.clone());
    ActionResult::Command { action, result, config }
}

/// Status-bar text for a finished action.
pub fn status_text(tr: &Tr, result: &ActionResult) -> String {
    match result {
        ActionResult::Report { action, report } => {
            let key = match action {
                ActionKind::Toggle => "gui-status-toggled",
                ActionKind::Retry => "gui-status-retried",
                ActionKind::Restore => "gui-status-restored",
                ActionKind::MoveToFolder => "gui-status-moved",
                ActionKind::StartFolder => "gui-status-folder-started",
                ActionKind::StopFolder => "gui-status-folder-stopped",
                ActionKind::DeleteFolderTasks => "gui-status-folder-emptied",
                ActionKind::ResumeAll => "gui-status-resumed-all",
                ActionKind::PauseAll => "gui-status-paused-all",
                ActionKind::ClearHistory => "gui-status-history-cleared",
                _ => "gui-status-done",
            };
            with_skipped(tr, tr_count(tr, key, report.affected), report)
        }
        ActionResult::Deleted { report, .. } => {
            with_skipped(tr, tr_count(tr, "gui-status-deleted", report.affected), report)
        }
        ActionResult::Added(report) => {
            let mut text = tr_count(tr, "gui-status-added", report.added.len());
            if report.duplicates > 0 {
                text.push(' ');
                text.push_str(&tr_count(tr, "gui-status-add-duplicates", report.duplicates));
            }
            if !report.start_errors.is_empty() {
                text.push(' ');
                text.push_str(&tr_count(tr, "gui-status-add-start-errors", report.start_errors.len()));
            }
            text
        }
        ActionResult::Edited(report) => {
            let mut text = tr_count(tr, "gui-status-edited", report.changed);
            let skipped = report.busy + report.not_found + report.failed;
            if skipped > 0 {
                text.push(' ');
                text.push_str(&tr_count(tr, "gui-status-edit-skipped", skipped));
            }
            text
        }
        ActionResult::Settings { error: None, .. } => tr.t("gui-status-settings-applied"),
        ActionResult::Settings { error: Some((_, e)), .. } => {
            error_text(tr, "gui-status-settings-failed", e)
        }
        ActionResult::Command { action, result: Ok(_), .. } => tr.t(match action {
            ActionKind::CreateFolder => "gui-status-folder-created",
            ActionKind::DeleteFolder => "gui-status-folder-deleted",
            ActionKind::ReloadScripts => "gui-status-scripts-reloaded",
            ActionKind::ReloadConfig => "gui-status-config-reloaded",
            _ => "gui-status-done",
        }),
        ActionResult::Command { result: Err(e), .. } => {
            error_text(tr, "gui-status-command-failed", e)
        }
    }
}

fn tr_count(tr: &Tr, key: &str, count: usize) -> String {
    let mut args = FluentArgs::new();
    args.set("count", count);
    tr.t_args(key, &args)
}

fn with_skipped(tr: &Tr, mut text: String, report: &OpReport) -> String {
    if report.busy > 0 || report.failed > 0 {
        let mut args = FluentArgs::new();
        args.set("busy", report.busy);
        args.set("failed", report.failed);
        text.push(' ');
        text.push_str(&tr.t_args("gui-status-skipped", &args));
    }
    text
}

fn error_text(tr: &Tr, key: &str, err: &CommandError) -> String {
    let mut args = FluentArgs::new();
    args.set("error", tr.command_error(err));
    tr.t_args(key, &args)
}

#[cfg(test)]
mod tests {
    use super::*;
    use ggg::download::enqueue::{EnqueueOptions, resolve_target_folder};
    use std::{
        sync::atomic::{AtomicUsize, Ordering},
        time::Duration,
    };

    /// Points the config directory at a temp dir for one test. `Config::save`
    /// deletes folder directories it does not know, so a test that reaches it
    /// without this would wipe the real configuration. Reset on drop, so a
    /// panic cannot leave the override behind.
    struct ConfigIsolation {
        _dir: tempfile::TempDir,
    }

    impl ConfigIsolation {
        fn new() -> Self {
            let dir = tempfile::TempDir::new().unwrap();
            ggg::util::paths::set_config_dir_override(Some(dir.path().to_path_buf()));
            unsafe { std::env::set_var("GGG_TEST_MODE", "1") };
            Self { _dir: dir }
        }
    }

    impl Drop for ConfigIsolation {
        fn drop(&mut self) {
            ggg::util::paths::set_config_dir_override(None);
            unsafe { std::env::remove_var("GGG_TEST_MODE") };
        }
    }

    struct Harness {
        actions: Actions,
        results: ActionResults,
        manager: DownloadManager,
        state: AppState,
        wakes: Arc<AtomicUsize>,
    }

    fn harness() -> Harness {
        let manager = DownloadManager::new();
        let state = AppState::new(Config::default());
        let wakes = Arc::new(AtomicUsize::new(0));
        let counter = wakes.clone();
        let (actions, results) = Actions::new(
            Handle::current(),
            state.clone(),
            manager.clone(),
            Arc::new(Notify::new()),
            move || {
                counter.fetch_add(1, Ordering::SeqCst);
            },
        );
        Harness { actions, results, manager, state, wakes }
    }

    /// Polls instead of blocking, so the runtime keeps running the action.
    async fn next_result(results: &ActionResults) -> ActionResult {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(result) = results.drain().into_iter().next() {
                    return result;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("action result within 5 s")
    }

    async fn default_folder(state: &AppState) -> String {
        resolve_target_folder(&*state.config.read().await, None).unwrap()
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn delete_action_returns_batch() {
        let _isolation = ConfigIsolation::new();
        let h = harness();
        let folder = default_folder(&h.state).await;
        let config = h.state.config.read().await.clone();
        let task = DownloadTask::new_with_folder("http://example.com/a".into(), folder, &config);
        let id = task.id;
        h.manager.add_download(task).await;

        h.actions.dispatch(Action::Delete { targets: vec![id] });

        match next_result(&h.results).await {
            ActionResult::Deleted { batch, report } => {
                assert_eq!(batch.len(), 1);
                assert_eq!(batch[0].id, id);
                assert_eq!(report.affected, 1);
            }
            other => panic!("expected Deleted, got {other:?}"),
        }
        assert!(h.manager.get_by_id(id).await.is_none());
        assert_eq!(h.wakes.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn apply_settings_stops_at_first_error() {
        let _isolation = ConfigIsolation::new();
        let h = harness();
        let before = h.state.config.read().await.download.max_concurrent;

        h.actions.dispatch(Action::ApplySettings {
            commands: vec![
                Command::UpdateMaxConcurrent { value: before + 1 },
                Command::UpdateDefaultDirectory { value: PathBuf::new() },
                Command::UpdateMaxConcurrent { value: before + 2 },
            ],
        });

        match next_result(&h.results).await {
            ActionResult::Settings { applied, error, config } => {
                assert_eq!(applied, 1);
                assert!(matches!(error, Some((1, CommandError::EmptyDefaultDirectory))));
                // The command after the failure did not run.
                assert_eq!(config.download.max_concurrent, before + 1);
            }
            other => panic!("expected Settings, got {other:?}"),
        }
    }

    #[tokio::test]
    #[serial_test::serial]
    async fn add_action_reports_added_ids() {
        let _isolation = ConfigIsolation::new();
        let h = harness();
        let folder_id = default_folder(&h.state).await;

        h.actions.dispatch(Action::Add(AddRequest {
            urls: vec!["http://example.com/a".into(), "http://example.com/b".into()],
            folder_id,
            options: EnqueueOptions::default(),
            immediate: false,
            save_as: None,
        }));

        match next_result(&h.results).await {
            ActionResult::Added(report) => {
                assert_eq!(report.added.len(), 2);
                assert_eq!(report.duplicates, 0);
                for id in report.added {
                    assert!(h.manager.get_by_id(id).await.is_some());
                }
            }
            other => panic!("expected Added, got {other:?}"),
        }
    }

    #[test]
    fn status_text_uses_the_catalog() {
        let result = ActionResult::Report {
            action: ActionKind::Retry,
            report: OpReport { affected: 2, busy: 1, failed: 0 },
        };
        let text = status_text(&Tr::load("en"), &result);
        assert_eq!(text, "Retrying 2 downloads. (1 busy, 0 refused)");
    }
}
