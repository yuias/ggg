//! egui rendering for one OS window: its own context, input state and wgpu
//! painter. Windows share the GPU device and the system font provider.
//!
//! One painter keeps one renderer whose textures are keyed by `TextureId`, and
//! every context's font atlas uses the same managed id, so two contexts cannot
//! share a painter. Each window therefore gets a context and a painter of its
//! own, all on `ViewportId::ROOT` (egui's multi-viewport API is not used).

use crate::event::UserEvent;
use anyhow::{Context as _, Result};
use egui::ViewportId;
use egui_system_font_provider::SystemFontProvider;
use egui_wgpu::{
    RendererOptions, WgpuConfiguration, WgpuSetup, WgpuSetupExisting, winit::Painter,
};
use std::{num::NonZeroU32, sync::Arc};
use winit::{event_loop::EventLoopProxy, window::Window};

/// What every window renders with. The device is taken from the first painter
/// so later windows do not create a second device and renderer pipeline.
pub struct SharedRender {
    gpu: Option<WgpuSetupExisting>,
    /// Enumerating the system fonts takes hundreds of milliseconds, so it is
    /// done once and handed to every context.
    fonts: Arc<SystemFontProvider>,
}

impl SharedRender {
    pub fn new() -> Self {
        Self {
            gpu: None,
            fonts: Arc::new(SystemFontProvider::new()),
        }
    }
}

pub struct Renderer {
    pub ctx: egui::Context,
    state: egui_winit::State,
    painter: Painter,
}

impl Renderer {
    pub fn new(
        window: &Arc<Window>,
        shared: &mut SharedRender,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self> {
        let ctx = egui::Context::default();
        ctx.set_theme(egui::ThemePreference::System);
        // Supplies CJK (and any other script the bundled fonts lack) on demand.
        ctx.add_font_provider(shared.fonts.clone());

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
            &**window,
            Some(window.scale_factor() as f32),
            window.theme(),
            None,
        );

        let wgpu_setup = match &shared.gpu {
            Some(gpu) => WgpuSetup::Existing(gpu.clone()),
            None => WgpuConfiguration::default().wgpu_setup,
        };
        // Blocking on the wgpu futures is fine here: they never wait on the
        // tokio runtime.
        let mut painter = pollster::block_on(Painter::new(
            ctx.clone(),
            WgpuConfiguration {
                wgpu_setup,
                ..Default::default()
            },
            false,
            RendererOptions::default(),
        ));
        pollster::block_on(painter.set_window(viewport_id, Some(window.clone())))
            .context("create wgpu surface")?;

        if shared.gpu.is_none() {
            let render_state = painter
                .render_state()
                .context("wgpu render state is missing after creating the surface")?;
            shared.gpu = Some(WgpuSetupExisting {
                instance: render_state.instance.clone(),
                adapter: render_state.adapter.clone(),
                device: render_state.device.clone(),
                queue: render_state.queue.clone(),
            });
        }

        Ok(Self { ctx, state, painter })
    }

    /// Forwards a winit event to egui; returns true when a repaint is needed.
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.state.on_window_event(window, event).repaint
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        // A minimized window reports zero size, which a surface cannot take.
        if let (Some(w), Some(h)) = (NonZeroU32::new(size.width), NonZeroU32::new(size.height)) {
            self.painter.on_window_resized(ViewportId::ROOT, w, h);
        }
    }

    /// Runs one egui pass with `ui` and paints the result.
    pub fn redraw(&mut self, window: &Arc<Window>, ui: impl FnMut(&mut egui::Ui)) {
        let raw_input = self.state.take_egui_input(window);
        let mut output = self.ctx.run_ui(raw_input, ui);
        self.state
            .handle_platform_output(window, output.platform_output);

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
            window,
        );
    }
}

impl Drop for Renderer {
    fn drop(&mut self) {
        self.painter.destroy();
    }
}
