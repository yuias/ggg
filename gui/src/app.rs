//! winit application handler: window registry, event routing and frame loop.

use crate::{
    bridge::{
        actions::{Action, ActionResult, ActionResults, Actions, status_text},
        snapshot::{SnapshotHandle, spawn_snapshot_task},
    },
    event::UserEvent,
    gui_config::GuiConfig,
    i18n::Tr,
    ui::main_view::MainView,
    window::{OsWindow, main_window_attributes},
};
use ggg::{
    app::bootstrap::Core, download::task::DownloadTask, ui::queue_ops::MAX_UNDO_BATCHES,
};
use std::time::Instant;
use tokio::runtime::Handle;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    window::WindowId,
};

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
    // Dispatched from the views added after the bridge.
    #[allow(dead_code)]
    actions: Actions,
    results: ActionResults,
    /// Text for the status bar; replaced by each finished action.
    status: Option<String>,
    /// Delete batches, newest last, for undo.
    undo: Vec<Vec<DownloadTask>>,
    view: MainView,
    main: Option<OsWindow>,
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
        Self {
            proxy,
            tr,
            gui_config,
            rt,
            core,
            snapshot,
            actions,
            results,
            status: None,
            undo: Vec::new(),
            view: MainView::default(),
            main: None,
            repaint_at: None,
        }
    }

    /// Current GUI state, written to disk by the caller once the loop ends.
    pub fn gui_config(&self) -> &GuiConfig {
        &self.gui_config
    }

    /// Starts `action` on the runtime; the result arrives through `drain_results`.
    #[allow(dead_code)]
    pub fn dispatch(&self, action: Action) {
        self.actions.dispatch(action);
    }

    /// Turns finished actions into a status message and undo entries.
    fn drain_results(&mut self) {
        for result in self.results.drain() {
            self.status = Some(status_text(&self.tr, &result));
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

    fn draw_main(&mut self) {
        self.drain_results();
        let status = self.status.clone();
        let Self {
            main,
            view,
            tr,
            gui_config,
            ..
        } = self;
        let Some(main) = main.as_mut() else {
            return;
        };
        // Read once per frame; the borrow must not outlive this statement.
        let snapshot = self.snapshot.rx.borrow().clone();
        main.redraw(|ui| {
            view.show(ui, &snapshot, tr, gui_config, status.as_deref());
        });
    }
}

impl ApplicationHandler<UserEvent> for GuiApp {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        // `resumed` can fire again on some platforms; keep the existing window.
        if self.main.is_some() {
            return;
        }
        match OsWindow::new(
            event_loop,
            main_window_attributes(&self.tr.t("gui-app-title")),
            self.proxy.clone(),
        ) {
            Ok(window) => {
                tracing::info!("GUI main window created");
                window.window.request_redraw();
                self.main = Some(window);
            }
            Err(e) => {
                tracing::error!("Failed to create the main window: {e:#}");
                event_loop.exit();
            }
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, id: WindowId, event: WindowEvent) {
        let Some(main) = self.main.as_mut().filter(|w| w.window.id() == id) else {
            return;
        };

        match &event {
            WindowEvent::CloseRequested => {
                event_loop.exit();
                return;
            }
            WindowEvent::Resized(size) => main.resize(*size),
            WindowEvent::RedrawRequested => {
                self.draw_main();
                return;
            }
            _ => {}
        }

        if main.on_window_event(&event) {
            main.window.request_redraw();
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: UserEvent) {
        match event {
            UserEvent::Repaint { window, delay } => {
                let Some(main) = self.main.as_ref().filter(|w| w.window.id() == window) else {
                    return;
                };
                if delay.is_zero() {
                    main.window.request_redraw();
                } else {
                    let at = Instant::now() + delay;
                    self.repaint_at = Some(self.repaint_at.map_or(at, |old| old.min(at)));
                }
            }
            UserEvent::Snapshot | UserEvent::ActionDone => {
                if let Some(main) = &self.main {
                    main.window.request_redraw();
                }
            }
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        match self.repaint_at {
            Some(at) if at <= Instant::now() => {
                self.repaint_at = None;
                if let Some(main) = &self.main {
                    main.window.request_redraw();
                }
                event_loop.set_control_flow(ControlFlow::Wait);
            }
            Some(at) => event_loop.set_control_flow(ControlFlow::WaitUntil(at)),
            None => event_loop.set_control_flow(ControlFlow::Wait),
        }
    }
}
