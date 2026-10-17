//! Win32 details that winit does not expose. Non-Windows builds get inert
//! stand-ins so the rest of the crate needs no `cfg`.

use winit::window::{Window, WindowAttributes};

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

/// Text on the system clipboard, if it holds any. Read on the calling thread
/// only when the user asked for a paste that has no clipboard event (menu
/// click, Ctrl+Shift+V).
pub fn clipboard_text() -> Option<String> {
    match arboard::Clipboard::new().and_then(|mut clipboard| clipboard.get_text()) {
        Ok(text) => Some(text),
        Err(e) => {
            tracing::debug!("No clipboard text: {e}");
            None
        }
    }
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

/// Makes the window an owned popup of `owner`: it stays above it, shares its
/// taskbar entry and Alt+Tab group, and is destroyed with it.
#[cfg(windows)]
pub fn owned_by(attributes: WindowAttributes, owner: isize) -> WindowAttributes {
    use winit::platform::windows::WindowAttributesExtWindows as _;
    attributes.with_owner_window(owner)
}

#[cfg(not(windows))]
pub fn owned_by(attributes: WindowAttributes, _owner: isize) -> WindowAttributes {
    attributes
}

/// Restores `hwnd` if minimized, then asks Windows to make it the foreground
/// window. Returns whether the foreground was granted. winit's own focus call
/// is avoided: it sends a synthetic Alt key press first. The grant here
/// relies on the right the starting process gave this one
/// (`AllowSetForegroundWindow`); without it Windows only flashes the taskbar
/// button and this returns false.
#[cfg(windows)]
pub fn bring_to_front(hwnd: isize) -> bool {
    use windows::Win32::{
        Foundation::HWND,
        UI::WindowsAndMessaging::{IsIconic, SW_RESTORE, SetForegroundWindow, ShowWindow},
    };
    if hwnd == 0 {
        return false;
    }
    let hwnd = HWND(hwnd as *mut std::ffi::c_void);
    // SAFETY: `hwnd` comes from a live winit window of this process. A stale
    // handle only makes the calls fail.
    unsafe {
        if IsIconic(hwnd).as_bool() {
            // The return value is the previous visibility, not an error.
            let _ = ShowWindow(hwnd, SW_RESTORE);
        }
        SetForegroundWindow(hwnd).as_bool()
    }
}

#[cfg(not(windows))]
pub fn bring_to_front(_hwnd: isize) -> bool {
    false
}

/// Enables or disables mouse and keyboard input of a window. The main window is
/// disabled while a dialog it owns is open, which is what makes the dialog
/// modal for the OS (a click on the disabled window beeps).
#[cfg(windows)]
pub fn set_enabled(hwnd: isize, enabled: bool) {
    use windows::Win32::{Foundation::HWND, UI::Input::KeyboardAndMouse::EnableWindow};
    // SAFETY: `hwnd` comes from a live winit window of this thread. A stale
    // handle only makes the call fail.
    unsafe {
        let _ = EnableWindow(HWND(hwnd as *mut std::ffi::c_void), enabled);
    }
}

#[cfg(not(windows))]
pub fn set_enabled(_hwnd: isize, _enabled: bool) {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bring_to_front_rejects_a_null_handle() {
        assert!(!bring_to_front(0));
    }
}
