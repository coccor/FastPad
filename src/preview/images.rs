//! Image decoding off the UI thread. A short-lived worker decodes queued files with WIC into
//! premultiplied BGRA pixels and posts `message` to `notify`; the UI thread drains results and
//! creates Direct2D bitmaps on demand, keeping the pixels so bitmaps can be rebuilt after device
//! loss.

use crate::preview::dwrite::hresult_error;
use crate::{FastPadError, Result};
use std::collections::{HashMap, VecDeque};
use std::os::windows::ffi::OsStrExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Direct2D::Common::{
    D2D_SIZE_U, D2D1_ALPHA_MODE_PREMULTIPLIED, D2D1_PIXEL_FORMAT,
};
use windows::Win32::Graphics::Direct2D::{D2D1_BITMAP_PROPERTIES, ID2D1Bitmap, ID2D1RenderTarget};
use windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_B8G8R8A8_UNORM;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory,
    WICBitmapDitherTypeNone, WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom,
    WICDecodeMetadataCacheOnDemand,
};
use windows::Win32::System::Com::{
    CLSCTX_INPROC_SERVER, COINIT_MULTITHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize,
};
use windows::core::{Interface, PCWSTR};
use windows_sys::Win32::Foundation::HWND;
use windows_sys::Win32::UI::WindowsAndMessaging::PostMessageW;

pub const MAX_IMAGE_PIXELS: u64 = 64_000_000;

pub struct DecodedImage {
    /// Decoded pixel size; smaller than the natural size when the decode was scaled down.
    pub width: u32,
    pub height: u32,
    /// The file's own pixel size, which layout uses so an image's DIP size never depends on how
    /// large it happened to be decoded.
    pub natural_width: u32,
    pub natural_height: u32,
    pub pixels: Vec<u8>,
}

struct ComScope(bool);

impl ComScope {
    fn enter() -> Self {
        Self(unsafe { CoInitializeEx(None, COINIT_MULTITHREADED) }.is_ok())
    }
}

impl Drop for ComScope {
    fn drop(&mut self) {
        if self.0 {
            unsafe { CoUninitialize() };
        }
    }
}

pub fn decode_image(path: &Path, max_width: u32) -> Result<DecodedImage> {
    let _com = ComScope::enter();
    let factory: IWICImagingFactory =
        unsafe { CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER) }
            .map_err(hresult_error)?;
    if path
        .extension()
        .is_some_and(|extension| extension.eq_ignore_ascii_case("svg"))
    {
        return crate::preview::svg::decode_svg(&factory, path, max_width);
    }
    let wide = path
        .as_os_str()
        .encode_wide()
        .chain(Some(0))
        .collect::<Vec<u16>>();
    unsafe {
        let decoder = factory
            .CreateDecoderFromFilename(
                PCWSTR(wide.as_ptr()),
                None,
                GENERIC_READ,
                WICDecodeMetadataCacheOnDemand,
            )
            .map_err(hresult_error)?;
        let frame = decoder.GetFrame(0).map_err(hresult_error)?;
        let (mut width, mut height) = (0_u32, 0_u32);
        frame
            .GetSize(&mut width, &mut height)
            .map_err(hresult_error)?;
        if width == 0 || height == 0 || u64::from(width) * u64::from(height) > MAX_IMAGE_PIXELS {
            return Err(FastPadError::Invariant(
                "image is empty or larger than 64 megapixels",
            ));
        }
        let (natural_width, natural_height) = (width, height);
        let mut source: IWICBitmapSource = frame.cast().map_err(hresult_error)?;
        if max_width > 0 && width > max_width {
            let scaled_height =
                ((u64::from(height) * u64::from(max_width)) / u64::from(width)).max(1) as u32;
            let scaler = factory.CreateBitmapScaler().map_err(hresult_error)?;
            scaler
                .Initialize(
                    &source,
                    max_width,
                    scaled_height,
                    WICBitmapInterpolationModeFant,
                )
                .map_err(hresult_error)?;
            source = scaler.cast().map_err(hresult_error)?;
            (width, height) = (max_width, scaled_height);
        }
        let converter = factory.CreateFormatConverter().map_err(hresult_error)?;
        converter
            .Initialize(
                &source,
                &GUID_WICPixelFormat32bppPBGRA,
                WICBitmapDitherTypeNone,
                None,
                0.0,
                WICBitmapPaletteTypeCustom,
            )
            .map_err(hresult_error)?;
        let mut pixels = vec![0_u8; width as usize * height as usize * 4];
        converter
            .CopyPixels(std::ptr::null(), width * 4, &mut pixels)
            .map_err(hresult_error)?;
        Ok(DecodedImage {
            width,
            height,
            natural_width,
            natural_height,
            pixels,
        })
    }
}

enum EntryState {
    Pending,
    Ready(DecodedImage),
    Failed,
}

struct Entry {
    state: EntryState,
    bitmap: Option<ID2D1Bitmap>,
    /// The largest `max_width` queued so far (0 = unlimited).
    requested: u32,
}

/// Whether a decode limited to `requested` pixels already satisfies a limit of `max_width`.
fn covers(requested: u32, max_width: u32) -> bool {
    requested == 0 || (max_width != 0 && requested >= max_width)
}

impl Entry {
    /// True when a (new) decode is needed for `max_width` device pixels. A ready image is decoded
    /// again only when it was scaled below both the natural width and the new limit.
    fn wants(&self, max_width: u32) -> bool {
        match &self.state {
            EntryState::Failed => false,
            EntryState::Pending => !covers(self.requested, max_width),
            EntryState::Ready(image) => {
                let target = if max_width == 0 {
                    image.natural_width
                } else {
                    max_width.min(image.natural_width)
                };
                image.width < target && !covers(self.requested, target)
            }
        }
    }
}

#[derive(Default)]
struct Shared {
    queue: VecDeque<(PathBuf, u32)>,
    done: Vec<(PathBuf, Result<DecodedImage>)>,
    worker_running: bool,
}

pub struct ImageCache {
    entries: HashMap<PathBuf, Entry>,
    shared: Arc<Mutex<Shared>>,
    /// `HWND` as an integer so the worker closure is `Send`.
    notify: isize,
    message: u32,
}

impl ImageCache {
    pub fn new(notify: HWND, message: u32) -> Self {
        Self {
            entries: HashMap::new(),
            shared: Arc::new(Mutex::new(Shared::default())),
            notify: notify as isize,
            message,
        }
    }

    /// Queues a decode of `path` at most `max_width` pixels wide (0 = natural size). Repeated
    /// requests are free unless the image must be decoded larger than before.
    pub fn request(&mut self, path: &Path, max_width: u32) {
        if let Some(entry) = self.entries.get_mut(path) {
            if !entry.wants(max_width) {
                return;
            }
            // Keep a ready image (and its size) on screen while the sharper decode runs.
            entry.requested = max_width;
        } else {
            let exists = std::fs::metadata(path).is_ok_and(|metadata| metadata.is_file());
            let state = if exists {
                EntryState::Pending
            } else {
                EntryState::Failed
            };
            self.entries.insert(
                path.to_owned(),
                Entry {
                    state,
                    bitmap: None,
                    requested: max_width,
                },
            );
            if !exists {
                return;
            }
        }
        let spawn = {
            let mut shared = self
                .shared
                .lock()
                .unwrap_or_else(|error| error.into_inner());
            shared.queue.push_back((path.to_owned(), max_width));
            !std::mem::replace(&mut shared.worker_running, true)
        };
        if spawn {
            let shared = Arc::clone(&self.shared);
            let (notify, message) = (self.notify, self.message);
            std::thread::spawn(move || {
                loop {
                    let job = {
                        let mut state = shared.lock().unwrap_or_else(|error| error.into_inner());
                        let job = state.queue.pop_front();
                        if job.is_none() {
                            state.worker_running = false;
                        }
                        job
                    };
                    let Some((path, max_width)) = job else {
                        return;
                    };
                    let result = decode_image(&path, max_width);
                    shared
                        .lock()
                        .unwrap_or_else(|error| error.into_inner())
                        .done
                        .push((path, result));
                    unsafe { PostMessageW(notify as HWND, message, 0, 0) };
                }
            });
        }
    }

    /// Moves finished decodes into the cache; true when anything changed.
    pub fn drain(&mut self) -> bool {
        let done = std::mem::take(
            &mut self
                .shared
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .done,
        );
        let changed = !done.is_empty();
        for (path, result) in done {
            if let Some(entry) = self.entries.get_mut(&path) {
                match (result, &entry.state) {
                    (Ok(image), EntryState::Ready(current)) if image.width <= current.width => {}
                    (Ok(image), _) => {
                        entry.state = EntryState::Ready(image);
                        entry.bitmap = None;
                    }
                    // A failed re-decode keeps the smaller image that already works.
                    (Err(_), EntryState::Ready(_)) => {}
                    (Err(_), _) => entry.state = EntryState::Failed,
                }
            }
        }
        changed
    }

    /// The image's natural pixel size once decoded, whatever size it was decoded at.
    pub fn size(&self, path: &Path) -> Option<(u32, u32)> {
        match &self.entries.get(path)?.state {
            EntryState::Ready(image) => Some((image.natural_width, image.natural_height)),
            _ => None,
        }
    }

    pub fn is_failed(&self, path: &Path) -> bool {
        matches!(
            self.entries.get(path).map(|entry| &entry.state),
            Some(EntryState::Failed)
        )
    }

    pub fn bitmap(&mut self, target: &ID2D1RenderTarget, path: &Path) -> Option<ID2D1Bitmap> {
        let entry = self.entries.get_mut(path)?;
        let EntryState::Ready(image) = &entry.state else {
            return None;
        };
        if entry.bitmap.is_none() {
            let properties = D2D1_BITMAP_PROPERTIES {
                pixelFormat: D2D1_PIXEL_FORMAT {
                    format: DXGI_FORMAT_B8G8R8A8_UNORM,
                    alphaMode: D2D1_ALPHA_MODE_PREMULTIPLIED,
                },
                dpiX: 96.0,
                dpiY: 96.0,
            };
            entry.bitmap = unsafe {
                target.CreateBitmap(
                    D2D_SIZE_U {
                        width: image.width,
                        height: image.height,
                    },
                    Some(image.pixels.as_ptr().cast()),
                    image.width * 4,
                    &properties,
                )
            }
            .ok();
        }
        entry.bitmap.clone()
    }

    /// Device loss: bitmaps belong to the lost device; pixels stay for rebuilding.
    pub fn release_bitmaps(&mut self) {
        for entry in self.entries.values_mut() {
            entry.bitmap = None;
        }
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 2x2 opaque red PNG.
    const PNG_2X2: [u8; 125] = [
        0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44,
        0x52, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00, 0x00, 0x02, 0x08, 0x06, 0x00, 0x00, 0x00, 0x72,
        0xb6, 0x0d, 0x24, 0x00, 0x00, 0x00, 0x01, 0x73, 0x52, 0x47, 0x42, 0x00, 0xae, 0xce, 0x1c,
        0xe9, 0x00, 0x00, 0x00, 0x04, 0x67, 0x41, 0x4d, 0x41, 0x00, 0x00, 0xb1, 0x8f, 0x0b, 0xfc,
        0x61, 0x05, 0x00, 0x00, 0x00, 0x09, 0x70, 0x48, 0x59, 0x73, 0x00, 0x00, 0x0e, 0xc3, 0x00,
        0x00, 0x0e, 0xc3, 0x01, 0xc7, 0x6f, 0xa8, 0x64, 0x00, 0x00, 0x00, 0x12, 0x49, 0x44, 0x41,
        0x54, 0x18, 0x57, 0x63, 0xf8, 0xcf, 0xc0, 0xf0, 0x1f, 0x84, 0xa1, 0x0c, 0x86, 0xff, 0x00,
        0x47, 0xca, 0x07, 0xf9, 0xac, 0x78, 0x42, 0xbc, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e,
        0x44, 0xae, 0x42, 0x60, 0x82,
    ];

    fn write_png(name: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!("fastpad-{name}-{}.png", std::process::id()));
        std::fs::write(&path, PNG_2X2).unwrap();
        path
    }

    #[test]
    fn decodes_png_to_premultiplied_bgra() {
        let path = write_png("decode");
        let image = decode_image(&path, 0).unwrap();
        assert_eq!((image.width, image.height), (2, 2));
        assert_eq!(image.pixels.len(), 16);
        assert_eq!(&image.pixels[..4], &[0, 0, 255, 255]);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn scales_down_to_the_maximum_width() {
        let path = write_png("scale");
        let image = decode_image(&path, 1).unwrap();
        assert_eq!((image.width, image.height), (1, 1));
        assert_eq!((image.natural_width, image.natural_height), (2, 2));
        let _ = std::fs::remove_file(path);
    }

    fn decoded_width(cache: &ImageCache, path: &Path) -> Option<u32> {
        match &cache.entries.get(path)?.state {
            EntryState::Ready(image) => Some(image.width),
            _ => None,
        }
    }

    fn wait_for(cache: &mut ImageCache, path: &Path, width: u32) {
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
        while decoded_width(cache, path) != Some(width) && std::time::Instant::now() < deadline {
            std::thread::sleep(std::time::Duration::from_millis(10));
            cache.drain();
        }
        assert_eq!(decoded_width(cache, path), Some(width));
    }

    #[test]
    fn missing_files_fail() {
        assert!(decode_image(Path::new(r"C:\definitely\missing.png"), 0).is_err());
    }

    #[test]
    fn the_cache_decodes_on_a_worker_and_reports_sizes() {
        let path = write_png("cache");
        let missing = PathBuf::from(r"C:\definitely\missing.png");
        let mut cache = ImageCache::new(std::ptr::null_mut(), 0);
        cache.request(&path, 1);
        cache.request(&missing, 0);
        wait_for(&mut cache, &path, 1);
        // The size is the natural size even though the decode was scaled to one pixel.
        assert_eq!(cache.size(&path), Some((2, 2)));
        assert!(cache.is_failed(&missing));
        // A narrower request reuses the decode; a wider one decodes again at the larger size.
        cache.request(&path, 1);
        assert_eq!(cache.entries[&path].requested, 1);
        cache.request(&path, 0);
        wait_for(&mut cache, &path, 2);
        assert_eq!(cache.size(&path), Some((2, 2)));
        // Fully decoded images never decode again.
        cache.request(&path, 8);
        assert_eq!(cache.entries[&path].requested, 0);
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn the_cache_decodes_svg_files_and_decodes_them_larger_on_request() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("assets")
            .join("fastpad-icon.svg");
        let mut cache = ImageCache::new(std::ptr::null_mut(), 0);
        cache.request(&path, 16);
        wait_for(&mut cache, &path, 16);
        assert_eq!(cache.size(&path), Some((256, 256)));
        cache.request(&path, 64);
        wait_for(&mut cache, &path, 64);
    }
}
