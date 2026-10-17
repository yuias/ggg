//! One OS window together with its egui renderer.

use crate::{
    event::UserEvent,
    gui_config::WindowGeometry,
    render::{Renderer, SharedRender},
};
use anyhow::{Context as _, Result};
use std::sync::Arc;
use winit::{
    dpi::{LogicalSize, PhysicalPosition},
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

/// Default and minimum client size of the Settings window, in logical px.
pub const SETTINGS_SIZE: (f64, f64) = (760.0, 540.0);
pub const SETTINGS_MIN_SIZE: (f64, f64) = (600.0, 420.0);

/// Default and minimum client size of the main window, in logical px.
pub const MAIN_SIZE: (f64, f64) = (1100.0, 700.0);
pub const MAIN_MIN_SIZE: (f64, f64) = (480.0, 320.0);

/// Part of a restored window that must lie on a monitor, in physical px, so
/// its title bar can still be grabbed.
const MIN_VISIBLE: (i64, i64) = (96, 48);

/// A monitor's rectangle and scale factor.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MonitorArea {
    pub rect: PxRect,
    pub scale: f64,
}

/// Where and how large to create the main window.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MainPlacement {
    /// Outer position in physical px.
    pub position: (i32, i32),
    /// Client size.
    pub size: LogicalSize<f64>,
    pub maximized: bool,
}

/// Placement for the geometry saved last time, or `None` when it cannot be
/// used: nothing was saved, or the monitor it was on is gone and the window
/// would open off-screen. The caller then lets the OS place the window.
///
/// The saved size is logical, so the physical rectangle depends on the scale
/// of the monitor that shows it; the monitor under the saved position is
/// tried first.
pub fn main_placement(
    saved: Option<WindowGeometry>,
    monitors: &[MonitorArea],
) -> Option<MainPlacement> {
    let g = saved?;
    let size = LogicalSize::new(
        f64::from(g.width).max(MAIN_MIN_SIZE.0),
        f64::from(g.height).max(MAIN_MIN_SIZE.1),
    );
    let contains_origin = |m: &MonitorArea| {
        let (mx, my, mw, mh) = m.rect.map_i64();
        let (x, y) = (i64::from(g.x), i64::from(g.y));
        x >= mx && x < mx + mw && y >= my && y < my + mh
    };
    let mut candidates: Vec<&MonitorArea> = monitors.iter().collect();
    candidates.sort_by_key(|m| !contains_origin(m));
    let shown = candidates.into_iter().any(|m| {
        let rect = (
            g.x,
            g.y,
            (size.width * m.scale).round() as u32,
            (size.height * m.scale).round() as u32,
        );
        visible_part(rect, m.rect)
    });
    shown.then_some(MainPlacement {
        position: (g.x, g.y),
        size,
        maximized: g.maximized,
    })
}

/// Whether enough of `rect` lies on `monitor` to be reachable.
fn visible_part(rect: PxRect, monitor: PxRect) -> bool {
    let (x, y, w, h) = rect.map_i64();
    let (mx, my, mw, mh) = monitor.map_i64();
    let overlap_w = (x + w).min(mx + mw) - x.max(mx);
    let overlap_h = (y + h).min(my + mh) - y.max(my);
    overlap_w >= MIN_VISIBLE.0.min(w) && overlap_h >= MIN_VISIBLE.1.min(h)
}

/// Attributes for the main window. Drag and drop is disabled so winit does not
/// register its own drop target, which would conflict with the OLE one the
/// GUI installs itself.
pub fn main_window_attributes(title: &str, placement: Option<MainPlacement>) -> WindowAttributes {
    let size = placement.map_or(LogicalSize::new(MAIN_SIZE.0, MAIN_SIZE.1), |p| p.size);
    let mut attrs = Window::default_attributes()
        .with_title(title)
        .with_inner_size(size);
    if let Some(p) = placement {
        attrs = attrs
            .with_position(PhysicalPosition::new(p.position.0, p.position.1))
            .with_maximized(p.maximized);
    }
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

    fn saved(x: i32, y: i32, width: u32, height: u32, maximized: bool) -> Option<WindowGeometry> {
        Some(WindowGeometry { x, y, width, height, maximized })
    }

    const PRIMARY: MonitorArea = MonitorArea { rect: (0, 0, 1920, 1080), scale: 1.0 };
    // A second, denser monitor to the right.
    const SECOND: MonitorArea = MonitorArea { rect: (1920, 0, 2880, 1620), scale: 1.5 };

    #[test]
    fn main_window_returns_to_its_saved_monitor() {
        let both = [PRIMARY, SECOND];
        let placement = main_placement(saved(2100, 80, 1000, 600, true), &both).unwrap();
        assert_eq!(placement.position, (2100, 80));
        assert_eq!((placement.size.width, placement.size.height), (1000.0, 600.0));
        assert!(placement.maximized);
    }

    #[test]
    fn main_window_on_an_unplugged_monitor_is_not_restored() {
        // The window sat on the second monitor, which is gone.
        assert_eq!(main_placement(saved(2100, 80, 1000, 600, false), &[PRIMARY]), None);
        assert_eq!(main_placement(saved(2100, 80, 1000, 600, true), &[]), None);
        assert_eq!(main_placement(None, &[PRIMARY]), None);
    }

    #[test]
    fn main_window_needs_a_reachable_part_on_screen() {
        // Only a thin sliver is left on the primary monitor.
        assert_eq!(main_placement(saved(1900, 20, 1000, 600, false), &[PRIMARY]), None);
        assert_eq!(main_placement(saved(100, 1070, 1000, 600, false), &[PRIMARY]), None);
        // A maximized window's frame starts a little off-screen.
        assert!(main_placement(saved(-8, -8, 1936, 1096, true), &[PRIMARY]).is_some());
    }

    #[test]
    fn main_window_size_is_judged_with_the_scale_of_its_monitor() {
        // 3000 logical px at 1.5 wraps past the second monitor's right edge
        // but its origin is visible there.
        let placement = main_placement(saved(2000, 0, 3000, 600, false), &[PRIMARY, SECOND]);
        assert!(placement.is_some());
    }

    #[test]
    fn tiny_saved_main_window_gets_a_usable_size() {
        let placement = main_placement(saved(10, 10, 50, 50, false), &[PRIMARY]).unwrap();
        assert_eq!((placement.size.width, placement.size.height), MAIN_MIN_SIZE);
    }
}
