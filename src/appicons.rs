//! The real icons of the apps, files and folders that `run` tiles open, as Explorer shows
//! them. Looked up on a background thread, since the shell can be slow, and cached.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{LazyLock, Mutex};

use slint::{Rgba8Pixel, SharedPixelBuffer};

/// Pixels per side. Tiles draw icons at 30 logical pixels, so this stays sharp at 2x scaling.
const SIZE: i32 = 64;

/// Premultiplied RGBA pixels, by target. None means the target has no icon to show.
type Pixels = SharedPixelBuffer<Rgba8Pixel>;
static CACHE: LazyLock<Mutex<HashMap<PathBuf, Option<Pixels>>>> = LazyLock::new(Default::default);

/// The file a `run` target opens, if it's one on disk: a full path, or a program name
/// such as "wt" or "notepad" found on the PATH. URLs and URIs have no file.
pub fn target_file(run: &str) -> Option<PathBuf> {
    let run = run.trim().trim_matches('"');
    if run.is_empty() || run.contains("://") || (run.contains(':') && !run.contains('\\') && !run.contains('/')) {
        return None;
    }
    let path = Path::new(run);
    if path.components().count() > 1 {
        return path.exists().then(|| path.to_path_buf());
    }
    let path_var = std::env::var_os("PATH")?;
    std::env::split_paths(&path_var).find_map(|dir| {
        [dir.join(run), dir.join(format!("{run}.exe"))]
            .into_iter()
            .find(|candidate| candidate.is_file())
    })
}

/// Looks up the icons for `targets` (tile index, file) on a background thread and calls
/// `found` with each one there is.
pub fn load(targets: Vec<(usize, PathBuf)>, found: impl Fn(usize, Pixels) + Send + 'static) {
    if targets.is_empty() {
        return;
    }
    let spawned = std::thread::Builder::new().name("icons".into()).spawn(move || {
        imp::init_thread();
        for (index, target) in targets {
            let cached = CACHE.lock().ok().and_then(|cache| cache.get(&target).cloned());
            let pixels = match cached {
                Some(pixels) => pixels,
                None => {
                    let pixels = imp::extract(&target);
                    if let Ok(mut cache) = CACHE.lock() {
                        cache.insert(target, pixels.clone());
                    }
                    pixels
                }
            };
            if let Some(pixels) = pixels {
                found(index, pixels);
            }
        }
    });
    if let Err(err) = spawned {
        eprintln!("icons: could not start: {err}");
    }
}

#[cfg(windows)]
mod imp {
    use std::path::Path;

    use slint::SharedPixelBuffer;
    use windows::core::HSTRING;
    use windows::Win32::Foundation::SIZE;
    use windows::Win32::Graphics::Gdi::{
        CreateCompatibleDC, DeleteDC, DeleteObject, GetDIBits, GetObjectW, BITMAP, BITMAPINFO, BITMAPINFOHEADER,
        BI_RGB, DIB_RGB_COLORS, HGDIOBJ,
    };
    use windows::Win32::System::Com::{CoInitializeEx, COINIT_APARTMENTTHREADED};
    use windows::Win32::UI::Shell::{
        IShellItemImageFactory, SHCreateItemFromParsingName, SIIGBF_BIGGERSIZEOK, SIIGBF_ICONONLY,
    };

    use super::{Pixels, SIZE};

    pub fn init_thread() {
        // The shell's image factories want a single-threaded apartment.
        let _ = unsafe { CoInitializeEx(None, COINIT_APARTMENTTHREADED) };
    }

    pub fn extract(target: &Path) -> Option<Pixels> {
        unsafe {
            let factory: IShellItemImageFactory =
                SHCreateItemFromParsingName(&HSTRING::from(target.as_os_str()), None).ok()?;
            let bitmap = factory
                .GetImage(SIZE { cx: SIZE, cy: SIZE }, SIIGBF_ICONONLY | SIIGBF_BIGGERSIZEOK)
                .ok()?;
            let object = HGDIOBJ(bitmap.0);

            let mut info = BITMAP::default();
            let read = GetObjectW(object, std::mem::size_of::<BITMAP>() as i32, Some((&mut info as *mut BITMAP).cast()));
            let (width, height) = (info.bmWidth, info.bmHeight.abs());
            let mut pixels = vec![0u8; (width.max(0) * height * 4) as usize];
            let mut header = BITMAPINFO {
                bmiHeader: BITMAPINFOHEADER {
                    biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                    biWidth: width,
                    // Negative: rows from the top down.
                    biHeight: -height,
                    biPlanes: 1,
                    biBitCount: 32,
                    biCompression: BI_RGB.0,
                    ..Default::default()
                },
                ..Default::default()
            };
            let dc = CreateCompatibleDC(None);
            let lines = if read > 0 && width > 0 && height > 0 {
                GetDIBits(dc, bitmap, 0, height as u32, Some(pixels.as_mut_ptr().cast()), &mut header, DIB_RGB_COLORS)
            } else {
                0
            };
            let _ = DeleteDC(dc);
            let _ = DeleteObject(object);
            if lines <= 0 {
                return None;
            }

            bgra_to_rgba(&mut pixels);
            Some(SharedPixelBuffer::clone_from_slice(&pixels, width as u32, height as u32))
        }
    }

    /// Windows gives premultiplied BGRA. Old icons without transparency come back with
    /// every alpha at zero, which would be invisible, so make those opaque.
    fn bgra_to_rgba(pixels: &mut [u8]) {
        let opaque = pixels.as_chunks::<4>().0.iter().all(|p| p[3] == 0);
        for pixel in pixels.as_chunks_mut::<4>().0 {
            pixel.swap(0, 2);
            if opaque {
                pixel[3] = 255;
            }
        }
    }

    #[cfg(test)]
    mod tests {
        #[test]
        fn converts_and_fixes_missing_alpha() {
            let mut pixels = vec![1, 2, 3, 0, 4, 5, 6, 0];
            super::bgra_to_rgba(&mut pixels);
            assert_eq!(pixels, [3, 2, 1, 255, 6, 5, 4, 255]);
            let mut pixels = vec![1, 2, 3, 128];
            super::bgra_to_rgba(&mut pixels);
            assert_eq!(pixels, [3, 2, 1, 128]);
        }

        #[test]
        fn finds_an_icon_for_notepad() {
            super::init_thread();
            let target = crate::appicons::target_file("notepad").expect("notepad is on the PATH");
            let pixels = super::extract(&target).expect("notepad has an icon");
            assert!(pixels.width() >= 32);
        }
    }
}

#[cfg(not(windows))]
mod imp {
    use std::path::Path;

    use super::Pixels;

    pub fn init_thread() {}

    pub fn extract(_target: &Path) -> Option<Pixels> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::target_file;

    #[test]
    fn urls_and_uris_have_no_file() {
        assert_eq!(target_file("https://www.google.com"), None);
        assert_eq!(target_file("ms-settings:sound"), None);
        assert_eq!(target_file("spotify:"), None);
        assert_eq!(target_file(""), None);
        assert_eq!(target_file(r"C:\does\not\exist.exe"), None);
    }

    #[test]
    fn paths_and_program_names_resolve() {
        let temp = std::env::temp_dir();
        assert_eq!(target_file(&temp.display().to_string()), Some(temp));
        #[cfg(windows)]
        assert!(target_file("cmd").is_some_and(|p| p.ends_with("cmd.exe")));
    }
}
