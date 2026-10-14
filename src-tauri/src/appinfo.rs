//! Identify the app that currently has focus.
//!
//! Used to attribute each dictation to a target app, so the Insights page can
//! show where you actually dictate. Best-effort: any failure returns `None`
//! rather than erroring, because this is metadata, not the core path.

/// File name of the focused window's process (e.g. `chrome.exe`), falling back
/// to the window title when the process name can't be read.
#[cfg(windows)]
pub fn foreground_app() -> Option<String> {
    use windows::core::PWSTR;
    use windows::Win32::Foundation::{CloseHandle, MAX_PATH};
    use windows::Win32::System::Threading::{
        OpenProcess, QueryFullProcessImageNameW, PROCESS_NAME_WIN32,
        PROCESS_QUERY_LIMITED_INFORMATION,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        GetForegroundWindow, GetWindowTextW, GetWindowThreadProcessId,
    };

    unsafe {
        let hwnd = GetForegroundWindow();
        if hwnd.0.is_null() {
            return None;
        }

        let mut pid = 0u32;
        GetWindowThreadProcessId(hwnd, Some(&mut pid));

        if pid != 0 {
            if let Ok(handle) = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, false, pid) {
                let mut buf = [0u16; MAX_PATH as usize];
                let mut len = buf.len() as u32;
                let ok = QueryFullProcessImageNameW(
                    handle,
                    PROCESS_NAME_WIN32,
                    PWSTR(buf.as_mut_ptr()),
                    &mut len,
                )
                .is_ok();
                let _ = CloseHandle(handle);
                if ok && len > 0 {
                    let path = String::from_utf16_lossy(&buf[..len as usize]);
                    if let Some(name) = path.rsplit(['\\', '/']).next() {
                        if !name.is_empty() {
                            return Some(name.to_string());
                        }
                    }
                }
            }
        }

        // Fall back to the window title.
        let mut buf = [0u16; 256];
        let n = GetWindowTextW(hwnd, &mut buf);
        if n > 0 {
            return Some(String::from_utf16_lossy(&buf[..n as usize]));
        }
        None
    }
}

#[cfg(not(windows))]
pub fn foreground_app() -> Option<String> {
    None
}
