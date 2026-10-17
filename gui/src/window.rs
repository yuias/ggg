//! One OS window together with its egui renderer.

use crate::{
    event::UserEvent,
    gui_config::WindowGeometry,
    render::{Renderer, SharedRender},
};
use anyhow::{Context as _, Result};
use std::sync::Arc;
use winit::{
    dpi::LogicalSize,
    event_loop::{ActiveEventLoop, EventLoopProxy},
    window::{Window, WindowAttributes, WindowButtons},
};

pub struct OsWindow {
    // The renderer goes first so its surface releases the window before ours.
    renderer: Renderer,
    pub window: Arc<Window>,
}

/// Default and minimum client size of the Add Downloads window, in logical px.
pub const ADD_DIALOG_SIZE: (f64, f64) = (560.0, 440.0);
pub const ADD_DIALOG_MIN_SIZE: (f64, f64) = (440.0, 340.0);

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

/// Attributes for a dialog. It starts hidden so it can be placed before it is
/// shown, and has no minimize or maximize button.
pub fn dialog_attributes(
    title: &str,
    size: LogicalSize<f64>,
    min_size: LogicalSize<f64>,
) -> WindowAttributes {
    let attrs = Window::default_attributes()
        .with_title(title)
        .with_inner_size(size)
        .with_min_inner_size(min_size)
        .with_enabled_buttons(WindowButtons::CLOSE)
        .with_visible(false);
    #[cfg(windows)]
    let attrs = {
        use winit::platform::windows::WindowAttributesExtWindows as _;
        // The main window owns the only drop target.
        attrs.with_drag_and_drop(false)
    };
    attrs
}

impl OsWindow {
    pub fn new(
        event_loop: &ActiveEventLoop,
        attributes: WindowAttributes,
        shared: &mut SharedRender,
        proxy: EventLoopProxy<UserEvent>,
    ) -> Result<Self> {
        let window = Arc::new(
            event_loop
                .create_window(attributes)
                .context("create window")?,
        );
        let renderer = Renderer::new(&window, shared, proxy)?;
        Ok(Self { renderer, window })
    }

    pub fn ctx(&self) -> &egui::Context {
        &self.renderer.ctx
    }

    /// Forwards a winit event to egui; returns true when a repaint is needed.
    pub fn on_window_event(&mut self, event: &winit::event::WindowEvent) -> bool {
        self.renderer.on_window_event(&self.window, event)
    }

    pub fn resize(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        self.renderer.resize(size);
    }

    /// Runs one egui pass with `ui` and paints the result.
    pub fn redraw(&mut self, ui: impl FnMut(&mut egui::Ui)) {
        self.renderer.redraw(&self.window, ui);
    }
}

/// A rectangle in physical px: x, y, width, height.
pub type PxRect = (i32, i32, u32, u32);

/// Whether `rect` overlaps any of `monitors`, so a saved position from a
/// disconnected monitor is not restored off-screen.
pub fn overlaps_any(rect: PxRect, monitors: &[PxRect]) -> bool {
    let (x, y, w, h) = rect.map_i64();
    monitors.iter().any(|monitor| {
        let (mx, my, mw, mh) = monitor.map_i64();
        x < mx + mw && x + w > mx && y < my + mh && y + h > my
    })
}

trait MapI64 {
    fn map_i64(self) -> (i64, i64, i64, i64);
}

impl MapI64 for PxRect {
    fn map_i64(self) -> (i64, i64, i64, i64) {
        (self.0.into(), self.1.into(), self.2.into(), self.3.into())
    }
}

/// Client size to create a dialog with: the saved size, never below `min`.
pub fn dialog_size(
    saved: Option<WindowGeometry>,
    default: (f64, f64),
    min: (f64, f64),
) -> LogicalSize<f64> {
    match saved {
        Some(g) => LogicalSize::new(f64::from(g.width).max(min.0), f64::from(g.height).max(min.1)),
        None => LogicalSize::new(default.0, default.1),
    }
}

/// Top-left outer position that centres a window of `own` size on `outer`.
pub fn centered_on(outer: PxRect, own: (u32, u32)) -> (i32, i32) {
    (
        outer.0 + (outer.2 as i32 - own.0 as i32) / 2,
        outer.1 + (outer.3 as i32 - own.1 as i32) / 2,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn geometry(width: u32, height: u32) -> WindowGeometry {
        WindowGeometry { x: 10, y: 20, width, height, maximized: false }
    }

    #[test]
    fn saved_size_is_used_but_not_below_the_minimum() {
        let min = ADD_DIALOG_MIN_SIZE;
        let size = dialog_size(Some(geometry(700, 500)), ADD_DIALOG_SIZE, min);
        assert_eq!((size.width, size.height), (700.0, 500.0));
        let size = dialog_size(Some(geometry(100, 100)), ADD_DIALOG_SIZE, min);
        assert_eq!((size.width, size.height), min);
    }

    #[test]
    fn default_size_applies_without_saved_geometry() {
        let size = dialog_size(None, ADD_DIALOG_SIZE, ADD_DIALOG_MIN_SIZE);
        assert_eq!((size.width, size.height), ADD_DIALOG_SIZE);
    }

    #[test]
    fn centering_uses_the_outer_rect_of_the_main_window() {
        assert_eq!(centered_on((100, 50, 1000, 800), (400, 300)), (400, 300));
        // A dialog larger than its owner hangs over the left and top edges.
        assert_eq!(centered_on((100, 50, 200, 200), (300, 300)), (50, 0));
    }

    #[test]
    fn saved_position_on_a_removed_monitor_is_rejected() {
        let monitors = [(0, 0, 1920, 1080), (1920, 0, 1920, 1080)];
        assert!(overlaps_any((100, 100, 500, 400), &monitors));
        assert!(overlaps_any((1800, 100, 500, 400), &monitors));
        assert!(!overlaps_any((3840, 0, 500, 400), &monitors));
        assert!(!overlaps_any((-2000, 100, 500, 400), &monitors));
        assert!(!overlaps_any((100, 100, 500, 400), &[]));
    }
}
