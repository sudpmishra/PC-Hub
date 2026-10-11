//! The hub's window: full screen on a chosen screen, or a window that goes back where it
//! was left. Also the connected screens and files dropped on the window, which Slint only
//! offers through winit.

use std::path::PathBuf;

#[cfg(not(windows))]
use slint::winit_030::{winit, EventResult};
use slint::winit_030::WinitWindowAccessor;
use slint::{PhysicalPosition, PhysicalSize};

/// Where the window should be.
pub struct Placement {
    pub fullscreen: bool,
    /// Where it was left out of full screen: x, y, width, height in physical pixels.
    pub geometry: Option<[i32; 4]>,
    /// Top-left corner of the screen to go full screen on.
    pub screen: Option<[i32; 2]>,
}

/// How far into a screen a window is put when moved there, so its edges stay reachable.
const INSET: i32 = 40;

pub fn place(window: &slint::Window, placement: &Placement) {
    // Leave full screen first: winit fills whichever screen the window is on.
    if window.is_fullscreen() {
        window.set_fullscreen(false);
    }
    if placement.fullscreen {
        if let Some([x, y]) = placement.screen {
            window.set_position(PhysicalPosition::new(x, y));
        }
        window.set_fullscreen(true);
    } else if let Some([x, y, width, height]) = placement.geometry {
        window.set_position(PhysicalPosition::new(x, y));
        window.set_size(PhysicalSize::new(width as u32, height as u32));
    } else if let Some([x, y]) = placement.screen {
        window.set_position(PhysicalPosition::new(x + INSET, y + INSET));
    }
}

/// Moves a window that isn't full screen onto another screen, keeping its size.
pub fn move_to_screen(window: &slint::Window, [x, y]: [i32; 2]) {
    window.set_position(PhysicalPosition::new(x + INSET, y + INSET));
}

/// How close, in physical pixels, a dragged window's edge must come to a screen edge to
/// snap to it.
const SNAP: i32 = 24;

/// Moves a window that isn't full screen to `position`, snapping its edges to the edges
/// of the screen it's on when they come close.
pub fn move_snapped(window: &slint::Window, position: PhysicalPosition) {
    let size = window.size();
    let screen = window
        .with_winit_window(|w| w.current_monitor().map(|m| (m.position(), m.size())))
        .flatten();
    let (x, y) = match screen {
        Some((corner, screen_size)) => (
            snap(position.x, size.width as i32, corner.x, screen_size.width as i32),
            snap(position.y, size.height as i32, corner.y, screen_size.height as i32),
        ),
        None => (position.x, position.y),
    };
    window.set_position(PhysicalPosition::new(x, y));
}

/// One axis of snapping: a window from `start` to `start + length` on a screen from
/// `edge` to `edge + span`.
fn snap(start: i32, length: i32, edge: i32, span: i32) -> i32 {
    let far = edge + span - length;
    if (start - edge).abs() <= SNAP {
        edge
    } else if (start - far).abs() <= SNAP {
        far
    } else {
        start
    }
}

/// The window's place and size, for remembering where it was left.
pub fn geometry(window: &slint::Window) -> [i32; 4] {
    let (position, size) = (window.position(), window.size());
    [position.x, position.y, size.width as i32, size.height as i32]
}

pub struct Screen {
    /// Top-left corner in physical pixels.
    pub corner: [i32; 2],
    /// Whether the window is on this screen now.
    pub current: bool,
}

/// The connected screens from left to right (then top to bottom).
pub fn screens(window: &slint::Window) -> Vec<Screen> {
    window
        .with_winit_window(|w| {
            let current = w.current_monitor().map(|m| m.position());
            let mut screens: Vec<Screen> = w
                .available_monitors()
                .map(|m| {
                    let p = m.position();
                    Screen {
                        corner: [p.x, p.y],
                        current: current == Some(p),
                    }
                })
                .collect();
            screens.sort_by_key(|s| (s.corner[0], s.corner[1]));
            screens
        })
        .unwrap_or_default()
}

pub enum FileEvent {
    /// A file or link is being dragged over the window.
    Hovering,
    /// The drag left the window or was cancelled.
    Left,
    /// A file was dropped; dropping several sends one of these each.
    Dropped(PathBuf),
    /// A link was dropped from a browser: its address, and the page's title if the
    /// browser gave one.
    Link { url: String, title: Option<String> },
}

/// Calls `handler` as files, and links from a browser, are dragged over and dropped on the
/// window. Call it once the window is showing.
#[cfg(windows)]
pub fn on_file_events(window: &slint::Window, handler: impl FnMut(FileEvent) + 'static) {
    if let Err(err) = drop_target::register(window, Box::new(handler)) {
        eprintln!("window: dropping files and links won't work: {err}");
    }
}

#[cfg(not(windows))]
pub fn on_file_events(window: &slint::Window, mut handler: impl FnMut(FileEvent) + 'static) {
    window.on_winit_window_event(move |_, event| {
        use winit::event::WindowEvent;
        match event {
            WindowEvent::HoveredFile(_) => handler(FileEvent::Hovering),
            WindowEvent::HoveredFileCancelled => handler(FileEvent::Left),
            WindowEvent::DroppedFile(path) => handler(FileEvent::Dropped(path.clone())),
            _ => {}
        }
        EventResult::Propagate
    });
}

/// Windows' drag and drop, which winit only uses for files. Browsers drag links as an
/// address (and a title in a .url file description), not as files.
#[cfg(windows)]
mod drop_target {
    use std::cell::{Cell, RefCell};

    use raw_window_handle::{HasWindowHandle, RawWindowHandle};
    use windows::core::{implement, w, Ref, Result};
    use windows::Win32::Foundation::{HGLOBAL, HWND, POINTL};
    use windows::Win32::System::Com::{IDataObject, DVASPECT_CONTENT, FORMATETC, TYMED_HGLOBAL};
    use windows::Win32::System::Memory::{GlobalLock, GlobalSize, GlobalUnlock};
    use windows::Win32::System::Ole::{
        IDropTarget, IDropTarget_Impl, RegisterDragDrop, ReleaseStgMedium, RevokeDragDrop, DROPEFFECT, DROPEFFECT_COPY,
        DROPEFFECT_LINK, DROPEFFECT_NONE,
    };
    use windows::Win32::System::SystemServices::MODIFIERKEYS_FLAGS;
    use windows::Win32::UI::Shell::{DragQueryFileW, HDROP};
    use windows::Win32::System::DataExchange::RegisterClipboardFormatW;

    use super::FileEvent;

    type Handler = Box<dyn FnMut(FileEvent)>;

    const CF_UNICODETEXT: u16 = 13;
    const CF_HDROP: u16 = 15;

    #[implement(IDropTarget)]
    struct Target {
        handler: RefCell<Handler>,
        /// What the drag over the window would do, set when it enters.
        effect: Cell<DROPEFFECT>,
    }

    pub fn register(window: &slint::Window, handler: Handler) -> std::result::Result<(), String> {
        let handle = window.window_handle();
        let hwnd = match handle.window_handle().map_err(|e| e.to_string())?.as_raw() {
            RawWindowHandle::Win32(win32) => HWND(win32.hwnd.get() as *mut _),
            _ => return Err("not a Windows window".into()),
        };
        let target: IDropTarget = Target {
            handler: RefCell::new(handler),
            effect: Cell::new(DROPEFFECT_NONE),
        }
        .into();
        unsafe {
            // winit registered its own (files only); take its place.
            let _ = RevokeDragDrop(hwnd);
            RegisterDragDrop(hwnd, &target).map_err(|e| e.to_string())
        }
    }

    fn format(id: u16) -> FORMATETC {
        FORMATETC {
            cfFormat: id,
            ptd: std::ptr::null_mut(),
            dwAspect: DVASPECT_CONTENT.0,
            lindex: -1,
            tymed: TYMED_HGLOBAL.0 as u32,
        }
    }

    fn registered(name: windows::core::PCWSTR) -> u16 {
        unsafe { RegisterClipboardFormatW(name) as u16 }
    }

    /// The bytes of one format of the dragged data.
    fn bytes(data: &IDataObject, id: u16) -> Option<Vec<u8>> {
        unsafe {
            let mut medium = data.GetData(&format(id)).ok()?;
            let memory: HGLOBAL = medium.u.hGlobal;
            let pointer = GlobalLock(memory) as *const u8;
            let result = (!pointer.is_null()).then(|| std::slice::from_raw_parts(pointer, GlobalSize(memory)).to_vec());
            let _ = GlobalUnlock(memory);
            ReleaseStgMedium(&mut medium);
            result
        }
    }

    fn wide_text(bytes: &[u8]) -> String {
        let wide: Vec<u16> = bytes.as_chunks::<2>().0.iter().map(|c| u16::from_le_bytes(*c)).take_while(|&c| c != 0).collect();
        String::from_utf16_lossy(&wide).trim().to_string()
    }

    fn files(data: &IDataObject) -> Vec<std::path::PathBuf> {
        unsafe {
            let Ok(mut medium) = data.GetData(&format(CF_HDROP)) else { return Vec::new() };
            let drop = HDROP(medium.u.hGlobal.0);
            let count = DragQueryFileW(drop, u32::MAX, None);
            let paths = (0..count)
                .map(|i| {
                    let mut buffer = vec![0u16; DragQueryFileW(drop, i, None) as usize + 1];
                    let length = DragQueryFileW(drop, i, Some(&mut buffer)) as usize;
                    std::path::PathBuf::from(String::from_utf16_lossy(&buffer[..length]))
                })
                .collect();
            ReleaseStgMedium(&mut medium);
            paths
        }
    }

    /// A link's address, and its title from the "Title.url" name the browser offers.
    fn link(data: &IDataObject) -> Option<(String, Option<String>)> {
        let url = bytes(data, registered(w!("UniformResourceLocatorW")))
            .map(|b| wide_text(&b))
            .or_else(|| bytes(data, CF_UNICODETEXT).map(|b| wide_text(&b)))
            .filter(|t| super::looks_like_link(t))?;
        // FILEGROUPDESCRIPTORW: a count, then FILEDESCRIPTORW entries whose file name
        // starts 72 bytes in.
        let title = bytes(data, registered(w!("FileGroupDescriptorW")))
            .filter(|b| b.len() >= 4 + 72 + 520)
            .map(|b| wide_text(&b[4 + 72..4 + 72 + 520]))
            .and_then(|name| super::link_title(&name));
        Some((url, title))
    }

    fn accepts(data: &IDataObject) -> DROPEFFECT {
        unsafe {
            if data.QueryGetData(&format(CF_HDROP)).is_ok() {
                DROPEFFECT_COPY
            } else if link(data).is_some() {
                DROPEFFECT_LINK
            } else {
                DROPEFFECT_NONE
            }
        }
    }

    impl IDropTarget_Impl for Target_Impl {
        fn DragEnter(&self, data: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
            let accepted = data.as_ref().map_or(DROPEFFECT_NONE, accepts);
            self.effect.set(accepted);
            unsafe { *effect = accepted };
            if accepted != DROPEFFECT_NONE {
                (self.handler.borrow_mut())(FileEvent::Hovering);
            }
            Ok(())
        }

        fn DragOver(&self, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
            unsafe { *effect = self.effect.get() };
            Ok(())
        }

        fn DragLeave(&self) -> Result<()> {
            (self.handler.borrow_mut())(FileEvent::Left);
            Ok(())
        }

        fn Drop(&self, data: Ref<IDataObject>, _: MODIFIERKEYS_FLAGS, _: &POINTL, effect: *mut DROPEFFECT) -> Result<()> {
            let mut handler = self.handler.borrow_mut();
            handler(FileEvent::Left);
            let Some(data) = data.as_ref() else { return Ok(()) };
            let paths = files(data);
            if !paths.is_empty() {
                for path in paths {
                    handler(FileEvent::Dropped(path));
                }
                unsafe { *effect = DROPEFFECT_COPY };
            } else if let Some((url, title)) = link(data) {
                handler(FileEvent::Link { url, title });
                unsafe { *effect = DROPEFFECT_LINK };
            } else {
                unsafe { *effect = DROPEFFECT_NONE };
            }
            Ok(())
        }
    }
}

/// Text that is a web address (or another link a tile can open).
fn looks_like_link(text: &str) -> bool {
    let text = text.trim();
    !text.contains(char::is_whitespace)
        && ["http://", "https://", "ftp://", "mailto:", "steam://"].iter().any(|p| text.to_ascii_lowercase().starts_with(p))
}

/// "Rust Programming Language.url" -> "Rust Programming Language"
fn link_title(file_name: &str) -> Option<String> {
    let title = file_name.strip_suffix(".url").unwrap_or(file_name).trim();
    (!title.is_empty()).then(|| title.to_string())
}

#[cfg(test)]
mod tests {
    use super::{link_title, looks_like_link, snap};

    #[test]
    fn links() {
        assert!(looks_like_link("https://www.rust-lang.org/"));
        assert!(!looks_like_link("hello there"));
        assert!(!looks_like_link(r"C:\file.txt"));
        assert_eq!(link_title("Rust Programming Language.url").as_deref(), Some("Rust Programming Language"));
        assert_eq!(link_title(".url"), None);
    }

    #[test]
    fn snaps_to_near_edges_only() {
        // A 1000-wide window on a screen from 1920 to 3840.
        assert_eq!(snap(1930, 1000, 1920, 1920), 1920);
        assert_eq!(snap(2830, 1000, 1920, 1920), 2840);
        assert_eq!(snap(2300, 1000, 1920, 1920), 2300);
        assert_eq!(snap(1900, 1000, 1920, 1920), 1920);
    }
}
