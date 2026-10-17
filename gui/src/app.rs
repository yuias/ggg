//! winit application handler: window registry, event routing and frame loop.

use crate::{
    event::UserEvent,
    window::{OsWindow, main_window_attributes},
};
use std::time::Instant;
use winit::{
    application::ApplicationHandler,
    event::WindowEvent,
    event_loop::{ActiveEventLoop, ControlFlow, EventLoopProxy},
    window::WindowId,
};

pub struct GuiApp {
    proxy: EventLoopProxy<UserEvent>,
    main: Option<OsWindow>,
    /// Earliest delayed repaint requested by egui, if any.
    repaint_at: Option<Instant>,
}

impl GuiApp {
    pub fn new(proxy: EventLoopProxy<UserEvent>) -> Self {
        Self {
            proxy,
            main: None,
            repaint_at: None,
        }
    }

    fn draw_main(&mut self) {
        let Some(main) = self.main.as_mut() else {
            return;
        };
        // Placeholder content until the real main view exists.
        main.redraw(|ui| {
            egui::CentralPanel::default().show(ui, |ui| {
                ui.label("ggg");
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
            main_window_attributes("ggg"),
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
