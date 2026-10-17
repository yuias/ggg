//! Events delivered to the winit loop from other threads.

use std::time::Duration;
use winit::window::WindowId;

pub enum UserEvent {
    /// From each egui `Context`'s repaint callback, which can fire on any thread.
    Repaint { window: WindowId, delay: Duration },
}
