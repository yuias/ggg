//! One OS window together with its egui context and wgpu surface.

use crate::event::UserEvent;
use anyhow::{Context as _, Result};
use egui::ViewportId;
use egui_wgpu::{RendererOptions, WgpuConfiguration, winit::Painter};
use std::{num::NonZeroU32, sync::Arc};
use winit::{
    dpi::LogicalSize,
    event_loop::{ActiveEventLoop, EventLoopProxy},
    window::{Window, WindowAttributes},
};

pub struct OsWindow {
    pub window: Arc<Window>,
    pub ctx: egui::Context,
    state: egui_winit::State,
    painter: Painter,
}

/// Attributes for the main window. Drag and drop is disabled so winit does not
/// register its own drop target, which would conflict with the OLE one the
/// GUI installs itself.
pub fn main_window_attributes(title: &str) -> WindowAttributes {
    let attrs = Window::default_attributes()
        .with_title(title)
        .with_inner_size(LogicalSize::new(1100.0, 700.0));
    #[cfg(windows)]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows as _;
        attrs.with_drag_and_drop(false)
    };
    attrs
}

impl OsWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self> {
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("create window")?,
        );

        let ctx = egui::Context::default();
        ctx.set_theme(egui::ThemePreference::System);
        // Supplies CJK (and any other script the bundled fonts lack) on demand.
        ctx.add_font_provider(Arc::new(
            egui_system_font_provider::SystemFontProvider::new(),
        ));

        let window_id = window.id();
        ctx.set_request_repaint_callback(move |info| {
            // The loop may already be gone during shutdown; nothing to wake then.
            let _ = proxy.send_event(UserEvent::Repaint {
                window: window_id,
                delay: info.delay,
            });
        });

        let viewport_id = ViewportId::ROOT;
        let state = egui_winit::State::new(
            ctx.clone(),
            viewport_id,
            &*window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );

        // Blocking on the wgpu futures is fine here: they never wait on the
        // tokio runtime.
        let mut painter = pollster::block_on(Painter::new(
            ctx.clone(),
            WgpuConfiguration::default(),
            false,
            RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(viewport_id, Some(window.clone())))
            .context("create wgpu surface")?;

        Ok(Self {
            window,
            ctx,
            state,
            painter,
        })
    }

    /// Forwards a winit event to egui; returns true when a repaint is needed.
    pub fn on_window_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(&self.window, event).repaint
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            self.painter.on_window_resized(ViewportId::ROOT, w, h);
        }
    }

    /// Runs one egui pass with `ui` and paints the result.
    pub fn redraw(&mut self, ui: impl FnMut(&mut egui::Ui)) {
        let raw_input = self.state.take_egui_input(&self.window);
        let mut output = self.ctx.run_ui(raw_input, ui);
        self.state
            .handle_platform_output(&self.window, output.platform_output);

        let primitives = self.ctx.tessellate(output.shapes, output.pixels_per_point);
        let clear_color = self
            .ctx
            .global_style()
            .visuals
            .panel_fill
            .to_normalized_gamma_f32();
        self.painter.paint_and_update_textures(
            ViewportId::ROOT,
            output.pixels_per_point,
            clear_color,
            &primitives,
            &mut output.textures_delta,
            Vec::new(),
            &self.window,
        );
    }
}

impl Drop for OsWindow {
    fn drop(&mut self) {
        self.painter.destroy();
    }
}
