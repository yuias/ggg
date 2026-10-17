//! Events delivered to the winit loop from other threads.

use crate::dnd::DropPayload;
use std::time::Duration;
use winit::window::WindowId;

pub enum UserEvent {
    /// From each egui `Context`'s repaint callback, which can fire on any thread.
    Repaint { window: WindowId, delay: Duration },
    /// The snapshot task published a new revision.
    Snapshot,
    /// An action finished and its result is waiting in the results channel.
    ActionDone,
    /// A menu bar item was chosen. `shift` is the Shift state at the click.
    // Only the Windows menu sends it.
    #[cfg_attr(not(windows), allow(dead_code))]
    Menu { id: String, shift: bool },
    /// Something was dropped on the main window.
    // Only the Windows drop target sends it.
    #[cfg_attr(not(windows), allow(dead_code))]
    Drop(DropPayload),
}
