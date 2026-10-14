// Text insertion: clipboard + simulated paste, with UI Automation reads used
// for context and for best-effort verification that the paste landed.
//
// This is the part that separates a toy from something usable. Typing the text
// character-by-character is slow and breaks in IMEs and terminals, so we do
// what reliable desktop dictation does: put the text on the clipboard, send
// Ctrl+V, confirm the focused control changed, then put the user's clipboard
// back.
//
// `paste_text` pastes exactly ONCE, then verifies:
//   1. Write the text to the clipboard and send Ctrl+V, then wait
//      `INITIAL_SETTLE_MS` and read the focused control back with UI Automation.
//   2. If the control is readable and byte-for-byte unchanged, Ctrl+V really did
//      nothing, so try Shift+Insert once (terminals and some apps paste on that).
//   3. Never re-send Ctrl+V after a failed check: every extra send duplicates
//      the text in the target. When the control can't be read we assume success
//      rather than risk a duplicate.
//   4. On every path — success, fallback, or failure — the user's original
//      clipboard is restored.
// The return value is `Ok(true)` when the paste was verified (or when the
// focused control could not be observed at all, in which case we assume success
// rather than cry wolf) and `Ok(false)` when a paste was sent but never
// confirmed.

#[cfg(windows)]
mod win {
    use anyhow::{bail, Result};
    use windows::Win32::Foundation::{HANDLE, HGLOBAL};
    use windows::Win32::System::Com::{
        CoCreateInstance, CoInitializeEx, CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED,
    };
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{
        GlobalAlloc, GlobalLock, GlobalUnlock, GMEM_MOVEABLE,
    };
    use windows::Win32::UI::Accessibility::{
        CUIAutomation, IUIAutomation, IUIAutomationTextPattern, IUIAutomationValuePattern,
        UIA_TextPatternId, UIA_ValuePatternId,
    };
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, KEYBDINPUT, KEYBD_EVENT_FLAGS, KEYEVENTF_KEYUP,
        VIRTUAL_KEY, VK_CONTROL, VK_INSERT, VK_SHIFT, VK_V,
    };

    /// Clipboard text format (CF_UNICODETEXT). Hardcoded to avoid pulling the
    /// whole Ole module for one constant.
    const CF_UNICODETEXT: u32 = 13;

    /// How long to wait after sending the first Ctrl+V before verifying.
    const INITIAL_SETTLE_MS: u64 = 140;

    unsafe fn clipboard_get_text() -> Option<String> {
        if OpenClipboard(None).is_err() {
            return None;
        }
        let mut result = None;
        if let Ok(handle) = GetClipboardData(CF_UNICODETEXT) {
            let glob = HGLOBAL(handle.0);
            let ptr = GlobalLock(glob) as *const u16;
            if !ptr.is_null() {
                let mut len = 0usize;
                while *ptr.add(len) != 0 {
                    len += 1;
                }
                let slice = std::slice::from_raw_parts(ptr, len);
                result = Some(String::from_utf16_lossy(slice));
                let _ = GlobalUnlock(glob);
            }
        }
        let _ = CloseClipboard();
        result
    }

    unsafe fn clipboard_set_text(text: &str) -> bool {
        let wide: Vec<u16> = text.encode_utf16().chain(std::iter::once(0)).collect();
        let bytes = wide.len() * 2;
        let Ok(handle) = GlobalAlloc(GMEM_MOVEABLE, bytes) else {
            return false;
        };
        let ptr = GlobalLock(handle) as *mut u16;
        if ptr.is_null() {
            return false;
        }
        std::ptr::copy_nonoverlapping(wide.as_ptr(), ptr, wide.len());
        let _ = GlobalUnlock(handle);

        if OpenClipboard(None).is_err() {
            return false;
        }
        let _ = EmptyClipboard();
        // On success the system owns the memory; do not free it.
        let ok = SetClipboardData(CF_UNICODETEXT, HANDLE(handle.0)).is_ok();
        let _ = CloseClipboard();
        ok
    }

    fn key_input(vk: u16, key_up: bool) -> INPUT {
        INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: 0,
                    dwFlags: if key_up {
                        KEYEVENTF_KEYUP
                    } else {
                        KEYBD_EVENT_FLAGS(0)
                    },
                    time: 0,
                    dwExtraInfo: 0,
                },
            },
        }
    }

    /// Press and release Ctrl+V in one batch.
    unsafe fn send_ctrl_v() {
        let inputs = [
            key_input(VK_CONTROL.0, false),
            key_input(VK_V.0, false),
            key_input(VK_V.0, true),
            key_input(VK_CONTROL.0, true),
        ];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }

    /// Press and release Shift+Insert in one batch. Terminals and some apps
    /// paste on Shift+Insert rather than Ctrl+V.
    unsafe fn send_shift_insert() {
        let inputs = [
            key_input(VK_SHIFT.0, false),
            key_input(VK_INSERT.0, false),
            key_input(VK_INSERT.0, true),
            key_input(VK_SHIFT.0, true),
        ];
        SendInput(&inputs, std::mem::size_of::<INPUT>() as i32);
    }

    /// Current text of the focused control, via UI Automation. Tries the text
    /// pattern (editors) then the value pattern (single-line fields).
    pub fn focused_text() -> Option<String> {
        unsafe {
            let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
            let automation: IUIAutomation =
                CoCreateInstance(&CUIAutomation, None::<&windows::core::IUnknown>, CLSCTX_INPROC_SERVER).ok()?;
            let elem = automation.GetFocusedElement().ok()?;

            if let Ok(tp) = elem.GetCurrentPatternAs::<IUIAutomationTextPattern>(UIA_TextPatternId) {
                if let Ok(range) = tp.DocumentRange() {
                    if let Ok(text) = range.GetText(-1) {
                        let s = text.to_string();
                        if !s.is_empty() {
                            return Some(s);
                        }
                    }
                }
            }
            if let Ok(vp) = elem.GetCurrentPatternAs::<IUIAutomationValuePattern>(UIA_ValuePatternId)
            {
                if let Ok(v) = vp.CurrentValue() {
                    let s = v.to_string();
                    if !s.is_empty() {
                        return Some(s);
                    }
                }
            }
            None
        }
    }

    /// Decide what the paste attempt tells us, from what we could read.
    ///
    /// Returns `(verified, positively_failed)`:
    /// * `verified` — the text is there, or the control is unreadable (assume
    ///   success rather than risk a duplicate).
    /// * `positively_failed` — we could read the control and it is byte-for-byte
    ///   unchanged, so the paste demonstrably did nothing.
    pub(super) fn classify(before: Option<&str>, after: Option<&str>, needle: &str) -> (bool, bool) {
        match (before, after) {
            (Some(b), Some(a)) => {
                if a != b && a.contains(needle) {
                    (true, false)
                } else if a == b {
                    (false, true)
                } else {
                    // Changed, but we can't see our text (e.g. the field
                    // reformatted it). Treat as landed.
                    (true, false)
                }
            }
            (None, Some(a)) => (a.contains(needle), false),
            // Control unreadable: assume success, never re-paste.
            _ => (true, false),
        }
    }

    pub fn paste_text(text: &str) -> Result<bool> {
        if text.is_empty() {
            bail!("empty text");
        }
        let before = focused_text();
        let saved = unsafe { clipboard_get_text() };

        if !unsafe { clipboard_set_text(text) } {
            bail!("could not write to the clipboard");
        }

        let needle = text.trim();

        // Paste exactly once. Re-sending Ctrl+V on a failed verification is
        // destructive: every extra send duplicates the text in the target. So
        // the only retry we allow is a *fallback* (Shift+Insert, which terminals
        // need), and only when we have positive evidence that nothing landed.
        unsafe { send_ctrl_v() };
        std::thread::sleep(std::time::Duration::from_millis(INITIAL_SETTLE_MS));

        let (mut verified, positively_failed) =
            classify(before.as_deref(), focused_text().as_deref(), needle);

        if !verified && positively_failed {
            unsafe { send_shift_insert() };
            std::thread::sleep(std::time::Duration::from_millis(INITIAL_SETTLE_MS));
            verified = classify(None, focused_text().as_deref(), needle).0;
        }

        // Put the user's clipboard back on every path, including failures.
        if let Some(prev) = saved {
            let _ = unsafe { clipboard_set_text(&prev) };
        }

        Ok(verified)
    }
}

#[cfg(windows)]
pub use win::{focused_text, paste_text};

#[cfg(not(windows))]
mod other {
    use anyhow::Result;
    pub fn paste_text(_text: &str) -> Result<bool> {
        anyhow::bail!("UnoFlow text insertion is implemented for Windows only")
    }
    pub fn focused_text() -> Option<String> {
        None
    }
}

#[cfg(not(windows))]
pub use other::{focused_text, paste_text};

#[cfg(all(test, windows))]
mod tests {
    use super::win::classify;

    // Regression: a failed verification used to re-send Ctrl+V up to 4 times,
    // duplicating the transcript. These lock in the "paste once" behaviour.

    #[test]
    fn text_present_is_verified() {
        let (ok, failed) = classify(Some(""), Some("hello world"), "hello world");
        assert!(ok);
        assert!(!failed);
    }

    #[test]
    fn unreadable_control_assumes_success_and_never_retries() {
        // Both reads unavailable: we must NOT claim a positive failure, or we'd
        // re-paste and duplicate.
        let (ok, failed) = classify(None, None, "hello");
        assert!(ok, "unreadable control should be treated as success");
        assert!(!failed, "must not be a positive failure");

        let (ok2, failed2) = classify(Some("x"), None, "hello");
        assert!(ok2);
        assert!(!failed2);
    }

    #[test]
    fn unchanged_control_is_positive_failure() {
        // Readable and byte-for-byte identical => Ctrl+V did nothing.
        let (ok, failed) = classify(Some("same"), Some("same"), "hello");
        assert!(!ok);
        assert!(failed, "unchanged readable control is a positive failure");
    }

    #[test]
    fn changed_but_missing_text_is_treated_as_landed() {
        // The field reformatted our text; do not re-paste.
        let (ok, failed) = classify(Some("before"), Some("HELLO WORLD"), "hello world");
        assert!(ok);
        assert!(!failed);
    }
}
