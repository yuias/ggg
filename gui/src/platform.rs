//! Win32 details that winit does not expose. Non-Windows builds get inert
//! stand-ins so the rest of the crate needs no `cfg`.

use winit::window::Window;

use crate::event::UserEvent;

/// Native handle of `window`, for APIs that take an `HWND`.
#[cfg(windows)]
pub fn hwnd(window: &Window) -> Option<isize> {
    use raw_window_handle::{HasWindowHandle as _, RawWindowHandle};
    match window.window_handle().ok()?.as_raw() {
        RawWindowHandle::Win32(handle) => Some(handle.hwnd.get()),
        _ => None,
    }
}

#[cfg(not(windows))]
pub fn hwnd(_window: &Window) -> Option<isize> {
    None
}

/// Whether Shift is held now. Read inside the menu handler, which runs while
/// the click is still being processed, so it reflects the click's modifiers.
#[cfg(windows)]
pub fn shift_down() -> bool {
    use windows::Win32::UI::Input::KeyboardAndMouse::{GetKeyState, VK_SHIFT};
    // SAFETY: plain query of the calling thread's keyboard state.
    unsafe { GetKeyState(i32::from(VK_SHIFT.0)) < 0 }
}

#[cfg(not(windows))]
pub fn shift_down() -> bool {
    false
}

/// Forwards menu clicks to the event loop. muda accepts the handler only once
/// per process, and a handler replaces its polling channel.
#[cfg(windows)]
pub fn forward_menu_events(proxy: winit::event_loop::EventLoopProxy<UserEvent>) {
    muda::MenuEvent::set_event_handler(Some(move |event: muda::MenuEvent| {
        // The loop may already be gone during shutdown; nothing to wake then.
        let _ = proxy.send_event(UserEvent::Menu {
            id: event.id.0,
            shift: shift_down(),
        });
    }));
}

#[cfg(not(windows))]
pub fn forward_menu_events(_proxy: winit::event_loop::EventLoopProxy<UserEvent>) {}
