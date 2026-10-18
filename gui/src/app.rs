//! winit application handler: window registry, event routing and frame loop.

use crate::{
    bridge::{
        actions::{Action, ActionResult, ActionResults, Actions, status_text},
        snapshot::{Snapshot, SnapshotHandle, spawn_snapshot_task},
    },
    commands::{GuiCommand, find_task},
    dnd,
    event::UserEvent,
    gui_config::{GuiConfig, WindowGeometry},
    i18n::Tr,
    instance::GuiIpc,
    menu::MenuBar,
    platform,
    registration::{Plan, Resolved, Source, plan, resolve_urls},
    render::SharedRender,
    settings::{
        model::SettingsModel,
        view::{self as settings_view, SettingsOutcome, SettingsWindow},
    },
    ui::{
        add_dialog::{self, AddDialog, AddDialogModel, AddOutcome},
        main_view::{MainView, ViewEnv, ViewOutput},
    },
    window::{
        ADD_DIALOG_MIN_SIZE, ADD_DIALOG_SIZE, OsWindow, PxRect, SETTINGS_MIN_SIZE, SETTINGS_SIZE,
        MonitorArea, centered_on, dialog_attributes, dialog_size, main_placement,
        main_window_attributes, overlaps_any,
    },
};
use ggg::{
    app::bootstrap::Core,
    download::task::DownloadTask,
    ui::queue_ops::{MAX_UNDO_BATCHES, open_in_file_manager},
    util::{script_files::scan_script_files, url_input::MAX_URLS_PER_REGISTRATION},
};
use fluent_bundle::FluentArgs;
use std::{collections::HashMap, path::PathBuf, time::Instant};
use tokio::runtime::Handle;
use winit::{
    application::ApplicationHandler,
    dpi::{LogicalSize, PhysicalPosition},
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    window::{Theme, WindowId},
};

/// Windows the app opens on top of the main one. Each is OS-modal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModalKind {
    AddDownloads,
    Settings,
}

/// The OS-modal window that is open now. The main window is disabled until it
/// closes, so there is at most one.
struct OpenModal {
    id: WindowId,
    kind: ModalKind,
    /// Main window handle that was disabled; `None` where there is no such call.
    owner: Option<isize>,
}

/// A window a command asked for; created from the event loop callback.
enum OpenRequest {
    AddDownloads { prefill: String, expand_checked: bool },
    /// `folder`: open on the Folders page with this folder selected.
    Settings { folder: Option<String> },
}

pub struct GuiApp {
    proxy: EventLoopProxy<UserEvent>,
    tr: Tr,
    gui_config: GuiConfig,
    /// Kept so the runtime handle outlives every dispatched action.
    #[allow(dead_code)]
    rt: Handle,
    #[allow(dead_code)]
    core: Core,
    snapshot: SnapshotHandle,
    /// Pipe that ggg-dnd and the bridge send links to. `None` where there is
    /// no pipe server.
    shared_pipe: Option<SharedPipe>,
    actions: Actions,
    results: ActionResults,
    /// Text for the status bar; replaced by each finished action.
    status: Option<String>,
    /// Appended to the status of the next finished add, which would
    /// otherwise hide that the list was cut.
    add_note: Option<String>,
    /// Delete batches, newest last, for undo.
    undo: Vec<Vec<DownloadTask>>,
    view: MainView,
    /// Declared before `windows` so it is revoked before the window goes.
    drop_target: Option<dnd::DropRegistration>,
    /// Every open OS window, the main one included.
    windows: HashMap<WindowId, OsWindow>,
    main_id: Option<WindowId>,
    /// Geometry of the main window while it is neither maximized nor
    /// minimized. A maximized window reports the screen rectangle, so this is
    /// what gets saved to make "restore" return to the size the user chose.
    main_normal: Option<WindowGeometry>,
    /// Title last given to the main window, to set it only when it changes.
    main_title: String,
    modal: Option<OpenModal>,
    /// Set by a command; the window is created from the event loop callback.
    open_request: Option<OpenRequest>,
    /// Input of the open Add Downloads window; set exactly while it is open.
    add_dialog: Option<AddDialog>,
    /// State of the open Settings window; set exactly while it is open.
    settings: Option<SettingsWindow>,
    shared: SharedRender,
    /// Where `gui_config` is written when a dialog closes; `None` disables it.
    gui_config_path: Option<PathBuf>,
    /// `None` when the platform has no native menu or building it failed.
    menu: Option<MenuBar>,
    /// Set by the Exit command; the loop is left once the frame is done.
    exit_requested: bool,
    /// Earliest delayed repaint requested by egui, if any.
    repaint_at: Option<Instant>,
}

impl GuiApp {
    /// Must be called while the runtime is alive; it starts the snapshot task.
    pub fn new(
        rt: Handle,
        core: Core,
        tr: Tr,
        gui_config: GuiConfig,
        proxy: EventLoopProxy<UserEvent>,
        ipc: GuiIpc,
    ) -> Self {
        let wake_proxy = proxy.clone();
        let snapshot = spawn_snapshot_task(&rt, core.manager.clone(), core.state.clone(), move || {
            // The loop may already be gone during shutdown; nothing to wake then.
            let _ = wake_proxy.send_event(UserEvent::Snapshot);
        });
        let action_proxy = proxy.clone();
        let (actions, results) = Actions::new(
            rt.clone(),
            core.state.clone(),
            core.manager.clone(),
            snapshot.refresh.clone(),
            move || {
                let _ = action_proxy.send_event(UserEvent::ActionDone);
            },
        );
        let shared_pipe = start_ipc(&rt, &proxy, ipc);
        Self {
            proxy,
            tr,
            gui_config,
            rt,
            core,
            snapshot,
            shared_pipe,
            actions,
            results,
            status: None,
            add_note: None,
            undo: Vec::new(),
            view: MainView::default(),
            drop_target: None,
            windows: HashMap::new(),
            main_id: None,
            main_normal: None,
            main_title: String::new(),
            modal: None,
            open_request: None,
            add_dialog: None,
            settings: None,
            shared: SharedRender::new(),
            gui_config_path: None,
            menu: None,
            exit_requested: false,
            repaint_at: None,
        }
    }

    /// Makes dialogs write `gui_config` to `path` when they close, so their
    /// geometry survives a crash.
    pub fn with_gui_config_path(mut self, path: Option<PathBuf>) -> Self {
        self.gui_config_path = path;
        self
    }

    fn main_window(&self) -> Option<&OsWindow> {
        self.main_id.and_then(|id| self.windows.get(&id))
    }

    /// Current GUI state, written to disk by the caller once the loop ends.
    pub fn gui_config(&self) -> &GuiConfig {
        &self.gui_config
    }

    /// Starts `action` on the runtime; the result arrives through `drain_results`.
    pub fn dispatch(&self, action: Action) {
        self.actions.dispatch(action);
    }

    /// Turns finished actions into a status message and undo entries.
    fn drain_results(&mut self) {
        for result in self.results.drain() {
            let mut text = status_text(&self.tr, &result);
            if matches!(result, ActionResult::Added(_)) {
                if let Some(note) = self.add_note.take() {
                    text.push(' ');
                    text.push_str(&note);
                }
            }
            let close_settings = self.settings_result(&result);
            // The window may close right after OK, taking its own notice with
            // it, so the main window has to say that a restart is needed.
            let restart_needed = self.settings.as_ref().is_some_and(|s| s.model.restart_needed);
            if matches!(result, ActionResult::Settings { error: None, .. }) && restart_needed {
                text = self.tr.t("gui-status-settings-applied-restart");
            }
            self.status = Some(text);
            if close_settings {
                self.close_settings();
            }
            if let ActionResult::Deleted { batch, .. } = result {
                if !batch.is_empty() {
                    self.undo.push(batch);
                    if self.undo.len() > MAX_UNDO_BATCHES {
                        self.undo.remove(0);
                    }
                }
            }
        }
    }

    /// Gives a finished action to the open Settings window, which started
    /// some of them. Returns true when the window should close.
    fn settings_result(&mut self, result: &ActionResult) -> bool {
        let Some(settings) = self.settings.as_mut() else {
            return false;
        };
        let close = settings.on_action_result(&self.tr, result);
        if let Some(window) = self.settings_window_id().and_then(|id| self.windows.get(&id)) {
            window.window.request_redraw();
        }
        close
    }

    fn settings_window_id(&self) -> Option<WindowId> {
        self.modal
            .as_ref()
            .filter(|m| m.kind == ModalKind::Settings)
            .map(|m| m.id)
    }

    fn close_settings(&mut self) {
        if let Some(id) = self.settings_window_id() {
            self.close_modal(id);
        }
    }

    fn draw_main(&mut self) {
        self.drain_results();
        let status = self.status.clone();
        let pipe_name = self.shared_pipe.as_ref().map(|pipe| pipe.name().to_string());
        let env = ViewEnv {
            undo_depth: self.undo.len(),
            os_modal_open: self.modal.is_some(),
        };
        // Read once per frame; the borrow must not outlive this statement.
        let snapshot = self.snapshot.rx.borrow().clone();
        let Self {
            windows,
            main_id,
            view,
            tr,
            gui_config,
            menu,
            ..
        } = self;
        let Some(main) = main_id.and_then(|id| windows.get_mut(&id)) else {
            return;
        };
        let mut output = ViewOutput::default();
        main.redraw(|ui| {
            output = view.show(ui, &snapshot, tr, gui_config, status.as_deref(), pipe_name.as_deref(), env);
        });
        if let Some(menu) = menu {
            menu.sync(&view.command_context(), gui_config.details_visible);
        }
        self.update_main_title(&snapshot);
        for action in output.actions {
            self.dispatch(action);
        }
        // egui-winit has already read the clipboard for the paste shortcut;
        // the text belongs to the first PasteUrls command.
        let mut pasted = output.pasted;
        for cmd in output.commands {
            match cmd {
                GuiCommand::PasteUrls if pasted.is_some() => {
                    let text = pasted.take().unwrap_or_default();
                    self.register(Source::PasteKey { shift: false }, &text);
                }
                GuiCommand::PasteUrlsDirect => {
                    self.register_clipboard(Source::PasteKey { shift: true });
                }
                cmd => self.run_command(cmd),
            }
        }
    }

    fn set_status(&mut self, text: String) {
        self.status = Some(text);
        // The status bar is drawn by the main window only.
        if let Some(main) = self.main_window() {
            main.window.request_redraw();
        }
    }

    /// Adds the URLs in `text` or opens the dialog for them, as `source` says.
    pub fn register(&mut self, source: Source, text: &str) {
        self.register_urls(source, ggg::util::url_input::parse_url_text(text));
    }

    /// [`Self::register`] for URLs that were already extracted, such as the
    /// ones in dropped `.url` files.
    pub fn register_urls(&mut self, source: Source, urls: Vec<String>) {
        let folder_id = {
            let snapshot = self.snapshot.rx.borrow();
            add_dialog::initial_folder(self.view.target_folder(), &snapshot)
        };
        let expand = self.gui_config.expand_ranges_on_direct_add;
        match resolve_urls(source, urls, expand, &folder_id) {
            Resolved::Dialog { prefill, expand_checked } => {
                if self.modal.is_some() {
                    let text = self.tr.t("gui-status-close-dialog-first");
                    self.set_status(text);
                    return;
                }
                self.open_add_dialog(prefill, expand_checked);
            }
            Resolved::NoUrl => {
                let text = self.tr.t("gui-status-no-url");
                self.set_status(text);
            }
            Resolved::Add { request, truncated } => {
                if truncated {
                    let mut args = FluentArgs::new();
                    args.set("max", MAX_URLS_PER_REGISTRATION);
                    self.add_note = Some(self.tr.t_args("gui-status-add-truncated", &args));
                }
                self.dispatch(Action::Add(request));
            }
        }
    }

    /// `register` for a paste that has to read the clipboard itself. Text-less
    /// clipboards count as empty text, so the dialog opens empty and a direct
    /// add reports that nothing was found.
    fn register_clipboard(&mut self, source: Source) {
        let text = platform::clipboard_text().unwrap_or_default();
        self.register(source, &text);
    }

    /// Runs a command from a menu, shortcut or context menu. Commands that
    /// are not enabled right now are dropped.
    pub fn run_command(&mut self, cmd: GuiCommand) {
        // The view learns about a new dialog only on its next frame, so a
        // command queued in between has to be refused here.
        if self.modal.is_some() {
            return;
        }
        let snapshot = self.snapshot.rx.borrow().clone();
        // Enabled state is checked again here: an earlier command of the same
        // batch may have opened a dialog.
        let Some(cmd) = self.view.route(cmd, &snapshot, &mut self.gui_config) else {
            return;
        };
        match cmd {
            GuiCommand::StartPause => {
                let targets = self.view.targets();
                self.dispatch(Action::Toggle {
                    reference: self.view.focus(),
                    targets,
                });
            }
            GuiCommand::Retry => {
                let targets = self.view.targets();
                self.dispatch(Action::Retry { targets });
            }
            GuiCommand::Undo => {
                if let Some(batch) = self.undo.pop() {
                    self.dispatch(Action::Restore { batch });
                }
            }
            GuiCommand::CopyUrls => {
                let urls: Vec<&str> = self
                    .view
                    .targets()
                    .into_iter()
                    .filter_map(|id| find_task(&snapshot, id))
                    .map(|task| task.url.as_str())
                    .collect();
                if let Some(main) = self.main_window() {
                    main.ctx().copy_text(urls.join("\n"));
                    // The copy is delivered with the next frame's output.
                    main.window.request_redraw();
                }
            }
            GuiCommand::OpenContainingFolder => {
                // One file manager window; several selected rows would open a
                // window each.
                let targets = self.view.targets();
                let lead = self
                    .view
                    .focus()
                    .filter(|id| targets.contains(id))
                    .or_else(|| targets.first().copied());
                if let Some(task) = lead.and_then(|id| find_task(&snapshot, id)) {
                    open_in_file_manager(&task.save_path);
                }
            }
            GuiCommand::ResumeAll => self.dispatch(Action::ResumeAll),
            GuiCommand::PauseAll => self.dispatch(Action::PauseAll),
            GuiCommand::StartFolder => {
                if let Some(folder) = self.view.tree_folder().map(str::to_string) {
                    self.dispatch(Action::StartFolder(folder));
                }
            }
            GuiCommand::StopFolder => {
                if let Some(folder) = self.view.tree_folder().map(str::to_string) {
                    self.dispatch(Action::StopFolder(folder));
                }
            }
            GuiCommand::Refresh => self.snapshot.refresh.notify_one(),
            GuiCommand::Exit => self.exit_requested = true,
            GuiCommand::NewDownload => {
                if let Plan::Dialog { expand_checked } =
                    plan(Source::NewDownload, self.gui_config.expand_ranges_on_direct_add)
                {
                    self.open_add_dialog(String::new(), expand_checked);
                }
            }
            GuiCommand::PasteUrls => self.register_clipboard(Source::MenuPaste { shift: false }),
            GuiCommand::PasteUrlsDirect => {
                self.register_clipboard(Source::MenuPaste { shift: true });
            }
            GuiCommand::OpenSettings => self.open_settings(None),
            GuiCommand::FolderSettings => {
                let folder = self.view.tree_folder().map(str::to_string);
                self.open_settings(folder);
            }
            // `route` handled these inside the view.
            GuiCommand::SelectAll
            | GuiCommand::SelectNone
            | GuiCommand::Delete
            | GuiCommand::Find
            | GuiCommand::EditItem
            | GuiCommand::MoveToFolder
            | GuiCommand::DeleteFolderTasks
            | GuiCommand::ClearHistory
            | GuiCommand::ToggleDetails
            | GuiCommand::ShowShortcuts
            | GuiCommand::About => {}
        }
    }
}

/// The link pipe server: where ggg-dnd and the bridge send links.
#[cfg(windows)]
struct SharedPipe {
    name: String,
    /// Whether `name` is the default one. When it is not, another process
    /// held the default when this one started.
    holds_default: bool,
    server: tokio::task::JoinHandle<()>,
    tx: tokio::sync::mpsc::Sender<ggg::ipc::pipe_server::IpcEvent>,
}

#[cfg(windows)]
impl SharedPipe {
    fn name(&self) -> &str {
        &self.name
    }

    /// Moves from the fallback name to the default one once its previous
    /// owner is gone. Returns whether the name changed.
    fn reclaim_default(&mut self, rt: &Handle) -> bool {
        use ggg::ipc::pipe_server::{claim_pipe, serve_pipe};

        if self.holds_default {
            return false;
        }
        let default_name = ggg_ipc::default_pipe_name();
        // Claiming and serving need the runtime context.
        let _enter = rt.enter();
        match claim_pipe(&default_name) {
            Ok(claimed) => {
                let server = serve_pipe(claimed, self.tx.clone());
                std::mem::replace(&mut self.server, server).abort();
                self.name = default_name;
                self.holds_default = true;
                tracing::info!("reclaimed shared pipe {}", self.name);
                true
            }
            Err(e) => {
                tracing::debug!("shared pipe {default_name} is still taken: {e}");
                false
            }
        }
    }
}

/// No pipe server exists off Windows, so there is nothing to describe.
#[cfg(not(windows))]
enum SharedPipe {}

#[cfg(not(windows))]
impl SharedPipe {
    fn name(&self) -> &str {
        match *self {}
    }

    fn reclaim_default(&mut self, _rt: &Handle) -> bool {
        match *self {}
    }
}

/// Starts the pipe server for the browser bridge. Links arrive as
/// `UserEvent::IpcText`, so nothing here blocks the event loop.
#[cfg(windows)]
fn start_ipc(rt: &Handle, proxy: &EventLoopProxy<UserEvent>, ipc: GuiIpc) -> Option<SharedPipe> {
    use ggg::ipc::pipe_server::{IpcEvent, start_pipe_server};

    // The channel also carries what the instance pipe receives.
    let GuiIpc { tx: event_tx, rx: mut event_rx } = ipc;
    // `start_pipe_server` spawns its accept loop and needs the runtime context.
    let (name, server) = {
        let _enter = rt.enter();
        start_pipe_server(event_tx.clone())
    };
    tracing::info!("IPC pipe server started: {name}");
    let holds_default = name == ggg_ipc::default_pipe_name();

    let proxy = proxy.clone();
    rt.spawn(async move {
        while let Some(event) = event_rx.recv().await {
            let event = match event {
                IpcEvent::UrlReceived(text) => UserEvent::IpcText(text),
                IpcEvent::Activate(urls) => UserEvent::Activate(urls),
            };
            // The loop is gone during shutdown; stop forwarding then.
            if proxy.send_event(event).is_err() {
                break;
            }
        }
    });

    // ggg-dnd is not launched here: it only exists because the TUI cannot
    // take drops, and this window has its own drop target.
    Some(SharedPipe { name, holds_default, server, tx: event_tx })
}

#[cfg(not(windows))]
fn start_ipc(_rt: &Handle, _proxy: &EventLoopProxy<UserEvent>, _ipc: GuiIpc) -> Option<SharedPipe> {
    None
}

/// Puts the menu on the window and keeps the client size, so the status bar
/// is not cut off: a menu bar takes room from the client area.
fn attach_menu(menu: &MenuBar, window: &mut OsWindow, hwnd: isize, dark: bool) {
    let size = window.window.inner_size();
    let maximized = window.window.is_maximized();
    menu.attach(hwnd, dark);
    // winit adds the menu height to the outer size when it resizes. A
    // maximized window fills the monitor whatever the menu takes, and asking
    // winit for a size would also restore it.
    if !maximized {
        let _ = window.window.request_inner_size(size);
    }
    window.resize(window.window.inner_size());
}

/// Monitors with their scale factors.
fn monitor_areas(event_loop: &ActiveEventLoop) -> Vec<MonitorArea> {
    event_loop
        .available_monitors()
        .map(|m| {
            let (pos, size) = (m.position(), m.size());
            MonitorArea {
                rect: (pos.x, pos.y, size.width, size.height),
                scale: m.scale_factor(),
            }
        })
        .collect()
}

/// Monitors as physical px rectangles.
fn monitor_rects(event_loop: &ActiveEventLoop) -> Vec<PxRect> {
    event_loop
        .available_monitors()
        .map(|m| {
            let (pos, size) = (m.position(), m.size());
            (pos.x, pos.y, size.width, size.height)
        })
        .collect()
}

impl GuiApp {
    /// Asks for the Add Downloads window with `prefill` (URLs, one per line)
    /// in the URL field. The window opens once the event loop is at hand. Does
    /// nothing while another OS-modal window is open; callers that need to
    /// tell the user check that first.
    pub fn open_add_dialog(&mut self, prefill: String, expand_checked: bool) {
        if self.modal.is_some() {
            return;
        }
        self.open_request = Some(OpenRequest::AddDownloads { prefill, expand_checked });
    }

    /// Asks for the Settings window, on the Folders page when `folder` is set.
    /// Does nothing while another OS-modal window is open.
    pub fn open_settings(&mut self, folder: Option<String>) {
        if self.modal.is_some() {
            return;
        }
        self.open_request = Some(OpenRequest::Settings { folder });
    }

    fn open_settings_window(&mut self, event_loop: &ActiveEventLoop, folder: Option<String>) {
        if self.modal.is_some() {
            return;
        }
        let snapshot = self.snapshot.rx.borrow().clone();
        let mut model = SettingsModel::new(&snapshot.config, &self.gui_config);
        if let Some(folder) = folder {
            model.open_folder(&folder);
        }
        let script_files = scan_script_files(&model.original.scripts.directory);
        self.settings = Some(SettingsWindow::new(model, script_files));
        self.open_modal(event_loop, ModalKind::Settings);
        if self.modal.is_none() {
            self.settings = None;
        }
    }

    fn open_add_downloads(
        &mut self,
        event_loop: &ActiveEventLoop,
        prefill: String,
        expand_checked: bool,
    ) {
        let folder_id = {
            let snapshot = self.snapshot.rx.borrow();
            add_dialog::initial_folder(self.view.target_folder(), &snapshot)
        };
        let model = AddDialogModel::new(prefill, expand_checked, folder_id);
        self.add_dialog = Some(AddDialog::new(model));
        self.open_modal(event_loop, ModalKind::AddDownloads);
        if self.modal.is_none() {
            self.add_dialog = None;
        }
    }

    /// Opens `kind` if no OS-modal window is open. The window is owned by the
    /// main window, which is disabled until the dialog closes.
    fn open_modal(&mut self, event_loop: &ActiveEventLoop, kind: ModalKind) {
        if self.modal.is_some() {
            return;
        }
        let Some(main) = self.main_window() else {
            return;
        };
        let owner = platform::hwnd(&main.window);
        let scale = main.window.scale_factor();

        let (title, default_size, min_size, saved) = match kind {
            ModalKind::AddDownloads => (
                self.tr.t("gui-add-title"),
                ADD_DIALOG_SIZE,
                ADD_DIALOG_MIN_SIZE,
                self.gui_config.add_dialog,
            ),
            ModalKind::Settings => (
                self.tr.t("gui-settings-title"),
                SETTINGS_SIZE,
                SETTINGS_MIN_SIZE,
                self.gui_config.settings_window,
            ),
        };
        let min = LogicalSize::new(min_size.0, min_size.1);
        let monitors = monitor_rects(event_loop);
        let saved = saved.filter(|g| {
            let px = |logical: u32| (f64::from(logical) * scale) as u32;
            overlaps_any((g.x, g.y, px(g.width), px(g.height)), &monitors)
        });
        let size = dialog_size(saved, default_size, min_size);

        let mut attributes = dialog_attributes(&title, size, min);
        if let Some(owner) = owner {
            attributes = platform::owned_by(attributes, owner);
        }
        // The main window's outer rectangle, for centring.
        let main_outer = main.window.outer_position().ok().map(|pos| {
            let outer = main.window.outer_size();
            (pos.x, pos.y, outer.width, outer.height)
        });
        // Creating the window near the main window's centre keeps its scale
        // factor right from the first frame; the decorations are unknown yet.
        let start = saved.map(|g| (g.x, g.y)).or_else(|| {
            main_outer.map(|outer| {
                centered_on(
                    outer,
                    ((size.width * scale) as u32, (size.height * scale) as u32),
                )
            })
        });
        if let Some((x, y)) = start {
            attributes = attributes.with_position(PhysicalPosition::new(x, y));
        }

        let dialog = match OsWindow::new(event_loop, attributes, &mut self.shared, self.proxy.clone())
        {
            Ok(dialog) => dialog,
            Err(e) => {
                tracing::error!("Failed to open the {kind:?} window: {e:#}");
                return;
            }
        };
        if saved.is_none() {
            // The decorations are known only now, so refine the estimate.
            if let Some(outer) = main_outer {
                let own = dialog.window.outer_size();
                let (x, y) = centered_on(outer, (own.width, own.height));
                dialog.window.set_outer_position(PhysicalPosition::new(x, y));
            }
        }

        // Disabling comes after every fallible step, so a failed open never
        // leaves the main window unusable.
        if let Some(owner) = owner {
            platform::set_enabled(owner, false);
        }
        dialog.window.set_visible(true);
        if let Some(hwnd) = platform::hwnd(&dialog.window) {
            platform::bring_to_front(hwnd);
        }
        dialog.window.request_redraw();

        let id = dialog.window.id();
        self.windows.insert(id, dialog);
        self.modal = Some(OpenModal { id, kind, owner });
        tracing::info!("opened {kind:?} window; main window disabled={}", owner.is_some());
        // The main window now shows its commands as disabled.
        if let Some(main) = self.main_window() {
            main.window.request_redraw();
        }
    }

    /// Closes the OS-modal window `id`: remembers its geometry, enables the
    /// main window again and only then destroys the dialog, so Windows
    /// activates the main window instead of another application.
    fn close_modal(&mut self, id: WindowId) {
        let Some(modal) = self.modal.take_if(|m| m.id == id) else {
            return;
        };
        if let Some(dialog) = self.windows.get(&id) {
            let geometry = window_geometry(&dialog.window);
            match modal.kind {
                ModalKind::AddDownloads => {
                    self.gui_config.add_dialog = geometry.or(self.gui_config.add_dialog);
                }
                ModalKind::Settings => {
                    self.gui_config.settings_window = geometry.or(self.gui_config.settings_window);
                }
            }
        }
        if let Some(owner) = modal.owner {
            platform::set_enabled(owner, true);
        }
        // Dropping the whole window releases the last handle to the native one.
        self.windows.remove(&id);
        self.add_dialog = None;
        self.settings = None;
        tracing::info!("closed {:?} window; main window enabled again", modal.kind);
        if let Some(main) = self.main_window() {
            if let Some(hwnd) = platform::hwnd(&main.window) {
                platform::bring_to_front(hwnd);
            }
            main.window.request_redraw();
        }
        self.store_main_geometry();
        self.save_gui_config();
    }

    /// Remembers the main window's rectangle while it is in its normal state.
    fn track_main_geometry(&mut self) {
        let Some(main) = self.main_window() else {
            return;
        };
        let window = &main.window;
        // A minimized window reports a position far off-screen.
        if window.is_maximized()
            || window.is_minimized() == Some(true)
            || window.fullscreen().is_some()
        {
            return;
        }
        if let Some(geometry) = window_geometry(window) {
            self.main_normal = Some(geometry);
        }
    }

    /// Copies the main window's state into `gui_config`; the caller writes it.
    fn store_main_geometry(&mut self) {
        let Some(main) = self.main_window() else {
            return;
        };
        let maximized = main.window.is_maximized();
        let normal = self.main_normal.or_else(|| window_geometry(&main.window));
        if let Some(geometry) = normal {
            self.gui_config.main_window = Some(WindowGeometry { maximized, ..geometry });
        }
    }

    /// Shows the folder that new downloads go to in the title bar.
    fn update_main_title(&mut self, snapshot: &Snapshot) {
        let folder = self.view.target_folder().and_then(|id| {
            snapshot
                .folders
                .iter()
                .find(|(folder_id, _)| folder_id == id)
                .map(|(_, name)| name.as_str())
        });
        let title = main_title(&self.tr, folder);
        if title == self.main_title {
            return;
        }
        if let Some(main) = self.main_window() {
            main.window.set_title(&title);
        }
        self.main_title = title;
    }

    fn save_gui_config(&self) {
        let Some(path) = &self.gui_config_path else {
            return;
        };
        if let Err(e) = self.gui_config.save_to(path) {
            tracing::warn!("Failed to save GUI state: {e:#}");
        }
    }

    fn draw_dialog(&mut self, id: WindowId) {
        match self.modal.as_ref().filter(|m| m.id == id).map(|m| m.kind) {
            Some(ModalKind::AddDownloads) => self.draw_add_dialog(id),
            Some(ModalKind::Settings) => self.draw_settings(id),
            None => {}
        }
    }

    fn draw_settings(&mut self, id: WindowId) {
        // Results that arrived while the window was not repainting.
        self.drain_results();
        let Self { windows, settings, tr, .. } = self;
        let (Some(window), Some(state)) = (windows.get_mut(&id), settings.as_mut()) else {
            return;
        };
        let mut outcome = SettingsOutcome::default();
        window.redraw(|ui| outcome = settings_view::show(ui, tr, state));
        if let Some(expand) = outcome.expand_ranges {
            self.gui_config.expand_ranges_on_direct_add = expand;
            self.save_gui_config();
        }
        for action in outcome.actions {
            self.dispatch(action);
        }
        if outcome.close {
            self.close_modal(id);
        }
    }

    fn draw_add_dialog(&mut self, id: WindowId) {
        // Read once per frame; the borrow must not outlive this statement.
        let snapshot = self.snapshot.rx.borrow().clone();
        let Self {
            windows,
            add_dialog,
            tr,
            ..
        } = self;
        let (Some(dialog), Some(input)) = (windows.get_mut(&id), add_dialog.as_mut()) else {
            return;
        };
        let mut outcome = AddOutcome::Keep;
        dialog.redraw(|ui| outcome = add_dialog::show(ui, tr, input, &snapshot.folders));
        match outcome {
            AddOutcome::Keep => {}
            AddOutcome::Submit(request) => {
                self.dispatch(Action::Add(request));
                self.close_modal(id);
            }
            AddOutcome::Cancel => self.close_modal(id),
        }
    }

    /// Registers URL text that arrived over the link pipe.
    fn receive_ipc_text(&mut self, event_loop: &ActiveEventLoop, text: &str) {
        tracing::info!("received {} byte(s) over the IPC pipe", text.len());
        self.register(Source::Ipc, text);
        self.apply_requests(event_loop);
        if let Some(main) = self.main_window() {
            main.window.request_redraw();
        }
    }

    /// Runs what commands asked for while they had no event loop at hand.
    fn apply_requests(&mut self, event_loop: &ActiveEventLoop) {
        match self.open_request.take() {
            Some(OpenRequest::AddDownloads { prefill, expand_checked }) => {
                self.open_add_downloads(event_loop, prefill, expand_checked);
            }
            Some(OpenRequest::Settings { folder }) => self.open_settings_window(event_loop, folder),
            None => {}
        }
        if self.exit_requested {
            self.store_main_geometry();
            self.drop_target = None;
            event_loop.exit();
        }
    }

    fn request_redraw_all(&self) {
        for window in self.windows.values() {
            window.window.request_redraw();
        }
    }
}

/// Window title: the target folder's name and the application name.
fn main_title(tr: &Tr, folder: Option<&str>) -> String {
    let app = tr.t("gui-app-title");
    let Some(folder) = folder else {
        return app;
    };
    let mut args = FluentArgs::new();
    args.set("folder", folder);
    args.set("app", app);
    tr.t_args("gui-window-title", &args)
}

/// Outer position in physical px and inner size in logical px.
fn window_geometry(window: &winit::window::Window) -> Option<WindowGeometry> {
    let pos = window.outer_position().ok()?;
    let size = window.inner_size().to_logical::<f64>(window.scale_factor());
    Some(WindowGeometry {
        x: pos.x,
        y: pos.y,
        width: size.width.round() as u32,
        height: size.height.round() as u32,
        maximized: false,
    })
}

impl ApplicationHandler<UserEvent> for GuiApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // `resumed` can fire again on some platforms; keep the existing window.
        if self.main_id.is_some() {
            return;
        }
        let placement = main_placement(self.gui_config.main_window, &monitor_areas(event_loop));
        if self.gui_config.main_window.is_some() && placement.is_none() {
            tracing::info!("Saved main window position is off-screen; using the default placement");
        }
        // Until a frame knows the target folder the title is the plain name.
        self.main_title = self.tr.t("gui-app-title");
        match OsWindow::new(
            event_loop,
            main_window_attributes(&self.main_title, placement),
            &mut self.shared,
            self.proxy.clone(),
        ) {
            Ok(mut window) => {
                tracing::info!("GUI main window created");
                platform::forward_menu_events(self.proxy.clone());
                self.menu = MenuBar::new(&self.tr);
                if let (Some(menu), Some(hwnd)) = (&self.menu, platform::hwnd(&window.window)) {
                    let dark = window.window.theme() == Some(Theme::Dark);
                    attach_menu(menu, &mut window, hwnd, dark);
                }
                if let Some(hwnd) = platform::hwnd(&window.window) {
                    let proxy = self.proxy.clone();
                    self.drop_target = dnd::register_drop_target(hwnd, move |payload| {
                        // The loop may already be gone during shutdown.
                        let _ = proxy.send_event(UserEvent::Drop(payload));
                    });
                    if self.drop_target.is_some() {
                        tracing::info!("Drop target registered on the main window");
                    }
                }
                window.window.request_redraw();
                let id = window.window.id();
                self.windows.insert(id, window);
                self.main_id = Some(id);
                // A window created maximized keeps the saved normal rectangle
                // until the user changes it; otherwise read the real one.
                if placement.is_some_and(|p| p.maximized) {
                    self.main_normal = self.gui_config.main_window;
                } else {
                    self.track_main_geometry();
                }
            }
            Err(e) => {
                tracing::error!("Failed to create the main window: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        if self.modal.as_ref().is_some_and(|m| m.id == id) {
            match &event {
                WindowEvent::CloseRequested => {
                    self.close_modal(id);
                    return;
                }
                WindowEvent::RedrawRequested => {
                    self.draw_dialog(id);
                    return;
                }
                WindowEvent::Resized(size) => {
                    if let Some(dialog) = self.windows.get_mut(&id) {
                        dialog.resize(*size);
                    }
                }
                _ => {}
            }
            if let Some(dialog) = self.windows.get_mut(&id) {
                if dialog.on_window_event(&event) {
                    dialog.window.request_redraw();
                }
            }
            return;
        }
        if self.main_id != Some(id) {
            return;
        }

        match &event {
            WindowEvent::CloseRequested => {
                if let Some(modal) = self.modal.as_ref().map(|m| m.id) {
                    self.close_modal(modal);
                }
                self.store_main_geometry();
                self.drop_target = None;
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => {
                if let Some(main) = self.windows.get_mut(&id) {
                    main.resize(*size);
                }
                self.track_main_geometry();
            }
            WindowEvent::Moved(_) => self.track_main_geometry(),
            WindowEvent::ThemeChanged(theme) => {
                if let (Some(menu), Some(hwnd)) = (
                    &self.menu,
                    self.main_window().and_then(|w| platform::hwnd(&w.window)),
                ) {
                    menu.set_dark(hwnd, *theme == Theme::Dark);
                }
            }
            WindowEvent::RedrawRequested => {
                self.draw_main();
                self.apply_requests(event_loop);
                return;
            }
            _ => {}
        }

        if let Some(main) = self.windows.get_mut(&id) {
            if main.on_window_event(&event) {
                main.window.request_redraw();
            }
        }
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Repaint { window, delay } => {
                let Some(target) = self.windows.get(&window) else {
                    return;
                };
                if delay.is_zero() {
                    target.window.request_redraw();
                } else {
                    let at = Instant::now() + delay;
                    self.repaint_at = Some(self.repaint_at.map_or(at, |old| old.min(at)));
                }
            }
            UserEvent::Snapshot => {
                if let Some(main) = self.main_window() {
                    main.window.request_redraw();
                }
            }
            UserEvent::ActionDone => {
                // The Settings window waits on its results while the disabled
                // main window may not repaint, so results are taken here.
                self.drain_results();
                self.request_redraw_all();
            }
            UserEvent::Drop(payload) => {
                tracing::info!(
                    "dropped {} URL(s) ctrl={} shift={}",
                    payload.urls.len(),
                    payload.ctrl,
                    payload.shift
                );
                let source = Source::Drop {
                    ctrl: payload.ctrl,
                    shift: payload.shift,
                };
                self.register_urls(source, payload.urls);
                // A Ctrl+drop asks for the dialog, which needs the event loop.
                self.apply_requests(event_loop);
                if let Some(main) = self.main_window() {
                    main.window.request_redraw();
                }
            }
            UserEvent::IpcText(text) => self.receive_ipc_text(event_loop, &text),
            UserEvent::Activate(urls) => {
                // The open modal covers the main window, so it is what to raise.
                let target = self
                    .modal
                    .as_ref()
                    .and_then(|modal| self.windows.get(&modal.id))
                    .or_else(|| self.main_window());
                let foreground = target
                    .and_then(|os| platform::hwnd(&os.window))
                    .is_some_and(platform::bring_to_front);
                tracing::info!(
                    "activation request with {} URL(s); foreground={foreground}",
                    urls.len()
                );
                if !urls.is_empty() {
                    self.receive_ipc_text(event_loop, &urls.join("\n"));
                }
                // A GUI that started while the TUI held the default name sits
                // on the fallback. The bridge starts a second GUI when no one
                // serves the default name, and that start ends up here.
                if let Some(pipe) = self.shared_pipe.as_mut()
                    && pipe.reclaim_default(&self.rt)
                {
                    // The status bar tooltip names the pipe.
                    self.request_redraw_all();
                }
            }
            UserEvent::Menu { id, shift } => {
                let Some(menu) = self.menu.as_mut() else {
                    return;
                };
                // A check item flips itself on click; the next sync restores
                // it if the command turns out to be refused.
                menu.invalidate();
                let Some(cmd) = menu.command(&id) else {
                    return;
                };
                tracing::info!("menu command {cmd:?} shift={shift}");
                // Shift on Edit > Paste URLs skips the dialog.
                let cmd = match cmd {
                    GuiCommand::PasteUrls if shift => GuiCommand::PasteUrlsDirect,
                    cmd => cmd,
                };
                self.run_command(cmd);
                self.apply_requests(event_loop);
                if let Some(main) = self.main_window() {
                    main.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.repaint_at {
            Some(at) if at <= Instant::now() => {
                self.repaint_at = None;
                // The request does not say which window asked.
                self.request_redraw_all();
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pipe_tooltip_names_the_pipe_in_every_locale() {
        for language in ["en-US", "ja-JP"] {
            let mut args = FluentArgs::new();
            args.set("name", r"\\.\pipe\ggg-42");
            let text = Tr::load(language).t_args("gui-statusbar-pipe-tooltip", &args);
            assert!(text.contains(r"\\.\pipe\ggg-42"), "{language}: {text:?}");
        }
    }

    #[test]
    fn title_names_the_target_folder_in_every_locale() {
        for language in ["en-US", "ja-JP"] {
            let tr = Tr::load(language);
            let app = tr.t("gui-app-title");
            assert_eq!(main_title(&tr, None), app);
            let title = main_title(&tr, Some("Music"));
            assert!(title.contains("Music") && title.contains(&app), "{language}: {title:?}");
            assert!(!title.contains("gui-window-title"), "{language}: {title:?}");
        }
    }

    #[test]
    fn registration_statuses_exist_in_every_locale() {
        for language in ["en-US", "ja-JP"] {
            let tr = Tr::load(language);
            let mut args = FluentArgs::new();
            args.set("max", MAX_URLS_PER_REGISTRATION);
            for key in [
                "gui-status-close-dialog-first",
                "gui-status-no-url",
                "gui-status-add-truncated",
            ] {
                let text = tr.t_args(key, &args);
                assert!(!text.is_empty() && !text.contains(key), "{language}: {key} -> {text:?}");
            }
            assert!(tr.t_args("gui-status-add-truncated", &args).contains("1000"));
        }
    }
}
