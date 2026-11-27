//! User-chosen icons of devices (#476). ICO and PNG (and anything else Windows Imaging
//! Component reads) are decoded once into a small premultiplied ARGB sprite and cached, so a
//! repaint is cheap. The file path is kept per device in the registry, see `settings::icon_file`.
use std::collections::HashMap;
use std::path::Path;
use std::sync::Mutex;

use windows::core::{HSTRING, Interface};
use windows::Win32::Foundation::GENERIC_READ;
use windows::Win32::Graphics::Imaging::{
    CLSID_WICImagingFactory, GUID_WICPixelFormat32bppPBGRA, IWICBitmapSource, IWICImagingFactory, WICBitmapDitherTypeNone,
    WICBitmapInterpolationModeFant, WICBitmapPaletteTypeCustom, WICDecodeMetadataCacheOnLoad,
};
use windows::Win32::System::Com::{CLSCTX_INPROC_SERVER, COINIT_APARTMENTTHREADED, CoCreateInstance, CoInitializeEx, CoUninitialize};

/// A decoded icon: `w * h` premultiplied ARGB pixels (alpha in the high byte), top-down.
#[derive(Clone, Debug, PartialEq)]
pub struct Sprite {
    pub w: i32,
    pub h: i32,
    pub px: Vec<u32>,
}

static CACHE: Mutex<Option<HashMap<String, Option<Sprite>>>> = Mutex::new(None);

/// Decodes `path`, scaled so its longer side is at most `size` pixels. A failed decode is cached
/// too, so a missing or damaged file is not read again on every paint.
pub fn load(path: &str, size: u32) -> Option<Sprite> {
    let key = format!("{path}|{size}");
    if let Ok(g) = CACHE.lock()
        && let Some(c) = g.as_ref()
        && let Some(v) = c.get(&key)
    {
        return v.clone();
    }
    let sprite = decode(path, size);
    if let Ok(mut g) = CACHE.lock() {
        g.get_or_insert_with(HashMap::new).insert(key, sprite.clone());
    }
    sprite
}

/// Forgets cached images of `path` after the file was changed or the setting removed.
pub fn forget(path: &str) {
    if let Ok(mut g) = CACHE.lock()
        && let Some(c) = g.as_mut()
    {
        c.retain(|k, _| !k.starts_with(&format!("{path}|")));
    }
}

fn decode(path: &str, size: u32) -> Option<Sprite> {
    if size == 0 || !Path::new(path).is_file() {
        return None;
    }
    unsafe {
        // WIC needs COM on the calling thread. The UI thread normally has it already.
        let init = CoInitializeEx(None, COINIT_APARTMENTTHREADED);
        // A poll thread may already be in the MTA; then apartment init fails, but WIC works there.
        let own = init.is_ok() || CoInitializeEx(None, windows::Win32::System::Com::COINIT_MULTITHREADED).is_ok();
        let out = decode_inner(path, size);
        if own {
            CoUninitialize();
        }
        out
    }
}

unsafe fn decode_inner(path: &str, size: u32) -> Option<Sprite> {
    unsafe {
        let factory: IWICImagingFactory = CoCreateInstance(&CLSID_WICImagingFactory, None, CLSCTX_INPROC_SERVER).ok()?;
        let dec = factory
            .CreateDecoderFromFilename(&HSTRING::from(path), None, GENERIC_READ, WICDecodeMetadataCacheOnLoad)
            .ok()?;
        let frame = dec.GetFrame(0).ok()?;
        let source: IWICBitmapSource = frame.cast().ok()?;
        let (mut w, mut h) = (0u32, 0u32);
        source.GetSize(&mut w, &mut h).ok()?;
        if w == 0 || h == 0 {
            return None;
        }
        let scale = (size as f32 / w.max(h) as f32).min(1.0);
        let (sw, sh) = (((w as f32 * scale).round() as u32).max(1), ((h as f32 * scale).round() as u32).max(1));
        let scaled = if sw != w || sh != h {
            let scaler = factory.CreateBitmapScaler().ok()?;
            scaler.Initialize(&source, sw, sh, WICBitmapInterpolationModeFant).ok()?;
            let s: IWICBitmapSource = scaler.cast().ok()?;
            s
        } else {
            source
        };
        let conv = factory.CreateFormatConverter().ok()?;
        conv.Initialize(
            &scaled,
            &GUID_WICPixelFormat32bppPBGRA,
            WICBitmapDitherTypeNone,
            None,
            0.0,
            WICBitmapPaletteTypeCustom,
        )
        .ok()?;
        let mut bytes = vec![0u8; (sw * sh * 4) as usize];
        conv.CopyPixels(std::ptr::null(), sw * 4, &mut bytes).ok()?;
        // WIC delivers premultiplied B, G, R, A; the canvas uses premultiplied A, R, G, B.
        let px = bytes
            .chunks_exact(4)
            .map(|c| u32::from_be_bytes([c[3], c[2], c[1], c[0]]))
            .collect();
        Some(Sprite { w: sw as i32, h: sh as i32, px })
    }
}
