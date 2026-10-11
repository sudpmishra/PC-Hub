//! Clipboard history: text copied anywhere is remembered (in memory only, never saved),
//! and tapping an entry on the hub pastes it into the app in front. Password managers mark
//! what they copy as private; that is skipped.

/// How many entries are kept.
const KEEP: usize = 20;

#[derive(Default)]
pub struct History {
    /// Newest first.
    pub items: Vec<String>,
    /// Windows' count of clipboard changes when last looked at.
    seen: u32,
}

impl History {
    /// Looks for something newly copied; returns true when the list changed.
    pub fn poll(&mut self) -> bool {
        let sequence = imp::sequence();
        if sequence == self.seen {
            return false;
        }
        self.seen = sequence;
        match imp::read_text() {
            Some(text) => self.remember(text),
            None => false,
        }
    }

    fn remember(&mut self, text: String) -> bool {
        if text.trim().is_empty() {
            return false;
        }
        if self.items.first() == Some(&text) {
            return false;
        }
        self.items.retain(|item| *item != text);
        self.items.insert(0, text);
        self.items.truncate(KEEP);
        true
    }

    /// Puts entry `index` on the clipboard and presses Ctrl+V in the app in front.
    pub fn paste(&mut self, index: usize) -> Result<(), String> {
        let text = self.items.get(index).cloned().ok_or("that entry is gone")?;
        imp::write_text(&text)?;
        // Our own change isn't something newly copied.
        self.seen = imp::sequence();
        self.remember(text);
        crate::actions::send_keys("Ctrl+V")
    }

    pub fn clear(&mut self) {
        self.items.clear();
    }
}

/// One line of an entry for the hub: whitespace squeezed, cut to fit.
pub fn preview(text: &str) -> String {
    let line = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if line.chars().count() > 120 {
        format!("{}…", line.chars().take(119).collect::<String>())
    } else {
        line
    }
}

#[cfg(windows)]
mod imp {
    use windows::core::w;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, GetClipboardData, GetClipboardSequenceNumber, IsClipboardFormatAvailable,
        OpenClipboard, RegisterClipboardFormatW, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE};

    const CF_UNICODETEXT: u32 = 13;

    pub fn sequence() -> u32 {
        unsafe { GetClipboardSequenceNumber() }
    }

    /// Opens the clipboard, retrying for a moment while another app has it open.
    fn open() -> Result<(), String> {
        for _ in 0..10 {
            if unsafe { OpenClipboard(None) }.is_ok() {
                return Ok(());
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        Err("another app is using the clipboard".into())
    }

    /// Marks password managers and other apps use for things history shouldn't keep.
    fn private() -> bool {
        unsafe {
            let exclude = RegisterClipboardFormatW(w!("ExcludeClipboardContentFromMonitorProcessing"));
            let history = RegisterClipboardFormatW(w!("CanIncludeInClipboardHistory"));
            let ignore = RegisterClipboardFormatW(w!("Clipboard Viewer Ignore"));
            if IsClipboardFormatAvailable(exclude).is_ok() || IsClipboardFormatAvailable(ignore).is_ok() {
                return true;
            }
            // A DWORD of 0 means "keep this out of history".
            if IsClipboardFormatAvailable(history).is_ok() {
                if let Ok(handle) = GetClipboardData(history) {
                    let memory = HGLOBAL(handle.0);
                    let pointer = GlobalLock(memory) as *const u32;
                    let allowed = pointer.is_null() || GlobalSize(memory) < 4 || pointer.read_unaligned() != 0;
                    let _ = GlobalUnlock(memory);
                    return !allowed;
                }
            }
            false
        }
    }

    pub fn read_text() -> Option<String> {
        unsafe {
            if IsClipboardFormatAvailable(CF_UNICODETEXT).is_err() {
                return None;
            }
            open().ok()?;
            let text = (|| {
                if private() {
                    return None;
                }
                let handle = GetClipboardData(CF_UNICODETEXT).ok()?;
                let memory = HGLOBAL(handle.0);
                let pointer = GlobalLock(memory) as *const u16;
                if pointer.is_null() {
                    return None;
                }
                let max = GlobalSize(memory) / 2;
                let length = (0..max).position(|i| *pointer.add(i) == 0).unwrap_or(max);
                let text = String::from_utf16_lossy(std::slice::from_raw_parts(pointer, length));
                let _ = GlobalUnlock(memory);
                Some(text)
            })();
            let _ = CloseClipboard();
            text
        }
    }

    pub fn write_text(text: &str) -> Result<(), String> {
        let wide: Vec<u16> = text.encode_utf16().chain([0]).collect();
        unsafe {
            let memory = GlobalAlloc(GMEM_MOVEABLE, wide.len() * 2).map_err(|e| e.to_string())?;
            let pointer = GlobalLock(memory) as *mut u16;
            if pointer.is_null() {
                let _ = GlobalFree(Some(memory));
                return Err("out of memory".into());
            }
            std::ptr::copy_nonoverlapping(wide.as_ptr(), pointer, wide.len());
            let _ = GlobalUnlock(memory);
            if let Err(err) = open() {
                let _ = GlobalFree(Some(memory));
                return Err(err);
            }
            let _ = EmptyClipboard();
            // The clipboard owns the memory once this succeeds.
            let result = SetClipboardData(CF_UNICODETEXT, Some(HANDLE(memory.0)));
            if result.is_err() {
                let _ = GlobalFree(Some(memory));
            }
            let _ = CloseClipboard();
            result.map(drop).map_err(|e| e.to_string())
        }
    }
}

#[cfg(not(windows))]
mod imp {
    pub fn sequence() -> u32 {
        0
    }
    pub fn read_text() -> Option<String> {
        None
    }
    pub fn write_text(_text: &str) -> Result<(), String> {
        Err("only supported on Windows".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn newest_first_without_repeats() {
        let mut history = History::default();
        assert!(history.remember("a".into()));
        assert!(history.remember("b".into()));
        assert!(!history.remember("b".into()));
        assert!(history.remember("a".into()));
        assert_eq!(history.items, ["a", "b"]);
        assert!(!history.remember("  ".into()));
        for i in 0..30 {
            history.remember(i.to_string());
        }
        assert_eq!(history.items.len(), KEEP);
    }

    #[test]
    fn previews_fit_on_one_line() {
        assert_eq!(preview("one\n  two\tthree"), "one two three");
        assert_eq!(preview(&"x".repeat(200)).chars().count(), 120);
    }
}
