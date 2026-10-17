//! winit application handler: window registry, event routing and frame loop.

use crate::{
    bridge::snapshot::{SnapshotHandle, spawn_snapshot_task},
    event::UserEvent,
    i18n::Tr,
    window::{OsWindow, main_window_attributes},
};
use fluent_bundle::FluentArgs;
use ggg::app::bootstrap::Core;
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
    /// Held for the action bridge, which dispatches onto the runtime.
    #[allow(dead_code)]
    rt: Handle,
    #[allow(dead_code)]
    core: Core,
    snapshot: SnapshotHandle,
    main: Option<OsWindow>,
    /// Earliest delayed repaint requested by egui, if any.
    repaint_at: Option<Instant>,
}

impl GuiApp {
    /// Must be called while the runtime is alive; it starts the snapshot task.
    pub fn new(rt: Handle, core: Core, tr: Tr, proxy: EventLoopProxy<UserEvent>) -> Self {
        let wake_proxy = proxy.clone();
        let snapshot = spawn_snapshot_task(&rt, core.manager.clone(), core.state.clone(), move || {
            // The loop may already be gone during shutdown; nothing to wake then.
            let _ = wake_proxy.send_event(UserEvent::Snapshot);
        });
        Self {
            proxy,
            tr,
            rt,
            core,
            snapshot,
            main: None,
            repaint_at: None,
        }
    }

    fn draw_main(&mut self) {
        let Some(main) = self.main.as_mut() else {
            return;
        };
        // Read once per frame; the borrow must not outlive this statement.
        let snapshot = self.snapshot.rx.borrow().clone();
        let mut args = FluentArgs::new();
        args.set("tasks", snapshot.task_count());
        args.set("folders", snapshot.folders.len());
        let summary = self.tr.t_args("gui-snapshot-summary", &args);
        // Placeholder content until the real main view exists.
        main.redraw(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label("ggg");
                ui.label(&summary);
                ui.label("日本語の表示テスト: ダウンロード マネージャー");
            });
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
            UserEvent::Snapshot => {
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
