/// Auto-launch ggg-dnd.exe if not already running (detected via Named Mutex).
pub fn auto_launch_ggg_dnd(pipe_name: &str) {
    use windows::Win32::Foundation::CloseHandle;
    use windows::Win32::System::Threading::OpenMutexW;

    // Check if ggg-dnd is already running via Named Mutex
    let already_running = unsafe {
        match OpenMutexW(
            windows::Win32::System::Threading::MUTEX_ALL_ACCESS,
            false,
            windows::core::w!("Global\\ggg-dnd-running"),
        ) {
            Ok(handle) => {
                let _ = CloseHandle(handle);
                true
            }
            Err(_) => false,
        }
    };

    if already_running {
        tracing::info!("ggg-dnd is already running, skipping auto-launch");
        return;
    }

    // Look for ggg-dnd.exe next to ggg.exe
    let dnd_exe = std::env::current_exe()
        .ok()
        .and_then(|p| p.parent().map(|d| d.join("ggg-dnd.exe")));

    match dnd_exe {
        Some(exe_path) if exe_path.exists() => {
            match std::process::Command::new(&exe_path).arg(pipe_name).spawn() {
                Ok(_) => tracing::info!("Auto-launched ggg-dnd: {:?}", exe_path),
                Err(e) => tracing::warn!("Failed to auto-launch ggg-dnd: {}", e),
            }
        }
        _ => {
            tracing::debug!("ggg-dnd.exe not found next to ggg.exe, skipping auto-launch");
        }
    }
}
