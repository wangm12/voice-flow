//! Phase 2 one-shot window capture + on-device OCR. Images stay in memory.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::context::ContextFamily;
use crate::screen_text::{ScreenTextContext, ScreenTextSource, MAX_CHARS, MAX_TOKENS};

static VISION_CAPTURE_COUNT: AtomicUsize = AtomicUsize::new(0);

pub const MAX_LONG_EDGE: u32 = 1280;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CaptureError {
    NoWindow,
    Unavailable,
}

pub fn scaled_size(width: u32, height: u32) -> (u32, u32) {
    let long = width.max(height);
    if long == 0 || long <= MAX_LONG_EDGE {
        return (width, height);
    }
    let scale = MAX_LONG_EDGE as f64 / long as f64;
    let next_width = ((width as f64) * scale).round().max(1.0) as u32;
    let next_height = ((height as f64) * scale).round().max(1.0) as u32;
    (next_width, next_height)
}

fn family_blocks_ocr(family: ContextFamily) -> bool {
    matches!(
        family,
        ContextFamily::Terminal | ContextFamily::FormFilling
    )
}

pub fn is_png(bytes: &[u8]) -> bool {
    bytes.len() >= 8 && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A])
}

pub fn encode_rgba_png(rgba: &[u8], width: u32, height: u32) -> Result<Vec<u8>, CaptureError> {
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageEncoder};

    if width == 0 || height == 0 {
        return Err(CaptureError::Unavailable);
    }
    let expected = (width as usize)
        .checked_mul(height as usize)
        .and_then(|pixels| pixels.checked_mul(4))
        .ok_or(CaptureError::Unavailable)?;
    if rgba.len() < expected {
        return Err(CaptureError::Unavailable);
    }
    let mut png = Vec::new();
    PngEncoder::new(&mut png)
        .write_image(&rgba[..expected], width, height, ExtendedColorType::Rgba8)
        .map_err(|_| CaptureError::Unavailable)?;
    if !is_png(&png) {
        return Err(CaptureError::Unavailable);
    }
    Ok(png)
}

pub fn memory_image(png: Vec<u8>, width: u32, height: u32) -> Result<MemoryImage, CaptureError> {
    if width == 0 || height == 0 || !is_png(&png) {
        return Err(CaptureError::Unavailable);
    }
    Ok(MemoryImage { png, width, height })
}

pub fn capture_locked_window(window_id: Option<u64>) -> Result<MemoryImage, CaptureError> {
    let window_id = window_id.filter(|id| *id != 0).ok_or(CaptureError::NoWindow)?;
    capture_locked_window_id(window_id)
}

/// Phase 3 capture. Dictation stop must never call this.
pub fn capture_for_vision(window_id: Option<u64>) -> Result<MemoryImage, CaptureError> {
    VISION_CAPTURE_COUNT.fetch_add(1, Ordering::SeqCst);
    capture_locked_window(window_id)
}

pub fn vision_capture_count() -> usize {
    VISION_CAPTURE_COUNT.load(Ordering::SeqCst)
}

pub fn reset_vision_capture_count() {
    VISION_CAPTURE_COUNT.store(0, Ordering::SeqCst);
}

#[cfg(target_os = "macos")]
fn capture_locked_window_id(window_id: u64) -> Result<MemoryImage, CaptureError> {
    std::panic::catch_unwind(|| capture_locked_window_id_inner(window_id))
        .unwrap_or(Err(CaptureError::Unavailable))
}

#[cfg(target_os = "macos")]
fn capture_locked_window_id_inner(window_id: u64) -> Result<MemoryImage, CaptureError> {
    use core_graphics::color_space::CGColorSpace;
    use core_graphics::context::{CGContext, CGInterpolationQuality};
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use core_graphics::image::CGImageAlphaInfo;
    use core_graphics::window::{
        create_image, kCGWindowImageBoundsIgnoreFraming, kCGWindowListOptionIncludingWindow,
    };

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        static CGRectNull: CGRect;
    }

    let image = create_image(
        unsafe { CGRectNull },
        kCGWindowListOptionIncludingWindow,
        window_id as u32,
        kCGWindowImageBoundsIgnoreFraming,
    )
    .ok_or(CaptureError::Unavailable)?;
    let src_width = image.width() as u32;
    let src_height = image.height() as u32;
    if src_width == 0 || src_height == 0 {
        return Err(CaptureError::Unavailable);
    }
    let (width, height) = scaled_size(src_width, src_height);
    let color_space = CGColorSpace::create_device_rgb();
    let mut ctx = CGContext::create_bitmap_context(
        None,
        width as usize,
        height as usize,
        8,
        width as usize * 4,
        &color_space,
        CGImageAlphaInfo::CGImageAlphaPremultipliedLast as u32,
    );
    ctx.set_interpolation_quality(CGInterpolationQuality::CGInterpolationQualityMedium);
    ctx.draw_image(
        CGRect::new(
            &CGPoint::new(0.0, 0.0),
            &CGSize::new(width as f64, height as f64),
        ),
        &image,
    );
    let bytes_per_row = ctx.bytes_per_row();
    let raw = ctx.data().to_vec();
    let rgba = packed_rgba(&raw, width, height, bytes_per_row)?;
    drop(image);
    memory_image(encode_rgba_png(&rgba, width, height)?, width, height)
}

fn packed_rgba(
    data: &[u8],
    width: u32,
    height: u32,
    bytes_per_row: usize,
) -> Result<Vec<u8>, CaptureError> {
    let row_bytes = (width as usize)
        .checked_mul(4)
        .ok_or(CaptureError::Unavailable)?;
    let needed = bytes_per_row
        .checked_mul(height as usize)
        .ok_or(CaptureError::Unavailable)?;
    if data.len() < needed {
        return Err(CaptureError::Unavailable);
    }
    if bytes_per_row == row_bytes {
        return Ok(data[..needed].to_vec());
    }
    let mut out = Vec::with_capacity(row_bytes.saturating_mul(height as usize));
    for y in 0..height as usize {
        let start = y.saturating_mul(bytes_per_row);
        let end = start.saturating_add(row_bytes);
        if end > data.len() {
            return Err(CaptureError::Unavailable);
        }
        out.extend_from_slice(&data[start..end]);
    }
    Ok(out)
}

#[cfg(not(target_os = "macos"))]
fn capture_locked_window_id(_window_id: u64) -> Result<MemoryImage, CaptureError> {
    Err(CaptureError::Unavailable)
}

pub fn ocr_memory_image(image: &MemoryImage) -> Vec<String> {
    if !is_png(&image.png) {
        return Vec::new();
    }
    ocr_png_bytes(&image.png)
}

#[cfg(target_os = "macos")]
fn ocr_png_bytes(png: &[u8]) -> Vec<String> {
    std::panic::catch_unwind(|| ocr_png_vision(png)).unwrap_or_default()
}

#[cfg(not(target_os = "macos"))]
fn ocr_png_bytes(_png: &[u8]) -> Vec<String> {
    Vec::new()
}

#[cfg(target_os = "macos")]
fn ocr_png_vision(png: &[u8]) -> Vec<String> {
    use objc::runtime::Object;
    use objc::{class, msg_send, sel, sel_impl};
    use std::ffi::CStr;
    use std::os::raw::{c_char, c_void};

    #[link(name = "Vision", kind = "framework")]
    extern "C" {}

    unsafe fn ns_string(value: *mut Object) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let bytes: *const c_char = msg_send![value, UTF8String];
        if bytes.is_null() {
            return None;
        }
        CStr::from_ptr(bytes).to_str().ok().map(str::to_owned)
    }

    unsafe {
        let data: *mut Object = msg_send![
            class!(NSData),
            dataWithBytes: png.as_ptr() as *const c_void
            length: png.len()
        ];
        if data.is_null() {
            return Vec::new();
        }
        let request: *mut Object = msg_send![class!(VNRecognizeTextRequest), new];
        if request.is_null() {
            return Vec::new();
        }
        let _: () = msg_send![request, setRecognitionLevel: 0u64];
        let _: () = msg_send![request, setUsesLanguageCorrection: true];
        let options: *mut Object = msg_send![class!(NSDictionary), dictionary];
        let handler: *mut Object = msg_send![class!(VNImageRequestHandler), alloc];
        let handler: *mut Object = msg_send![handler, initWithData: data options: options];
        if handler.is_null() {
            let _: () = msg_send![request, release];
            return Vec::new();
        }
        let requests: *mut Object = msg_send![class!(NSArray), arrayWithObject: request];
        let mut error: *mut Object = std::ptr::null_mut();
        let ok: bool = msg_send![handler, performRequests: requests error: &mut error];
        let mut tokens = Vec::new();
        if ok {
            let results: *mut Object = msg_send![request, results];
            if !results.is_null() {
                let count: usize = msg_send![results, count];
                for index in 0..count {
                    let observation: *mut Object = msg_send![results, objectAtIndex: index];
                    if observation.is_null() {
                        continue;
                    }
                    let candidates: *mut Object = msg_send![observation, topCandidates: 1usize];
                    if candidates.is_null() {
                        continue;
                    }
                    let candidate_count: usize = msg_send![candidates, count];
                    if candidate_count == 0 {
                        continue;
                    }
                    let top: *mut Object = msg_send![candidates, objectAtIndex: 0usize];
                    if let Some(text) = ns_string(msg_send![top, string]) {
                        let trimmed = text.trim();
                        if !trimmed.is_empty() {
                            tokens.push(trimmed.to_owned());
                        }
                    }
                }
            }
        }
        let _: () = msg_send![handler, release];
        let _: () = msg_send![request, release];
        tokens
    }
}

pub fn merge_ocr_tokens(ctx: &ScreenTextContext, tokens: Vec<String>) -> ScreenTextContext {
    let mut merged = ctx.clone();
    merged.source = ScreenTextSource::AxOcr;
    for token in tokens {
        let trimmed = token.trim();
        if trimmed.is_empty() {
            continue;
        }
        if merged.tokens.iter().any(|existing| existing == trimmed) {
            continue;
        }
        if merged.tokens.len() >= MAX_TOKENS {
            merged.truncated = true;
            break;
        }
        let next_chars = merged.usable_chars() + trimmed.chars().count();
        if next_chars > MAX_CHARS {
            merged.truncated = true;
            break;
        }
        merged.tokens.push(trimmed.to_owned());
    }
    merged
}

pub fn maybe_ocr(
    enabled: bool,
    recording_ok: bool,
    family: ContextFamily,
    sensitive: bool,
    ctx: &ScreenTextContext,
    window_id: Option<u64>,
) -> Option<ScreenTextContext> {
    maybe_ocr_with(
        enabled,
        recording_ok,
        family,
        sensitive,
        ctx,
        window_id,
        capture_locked_window,
        ocr_memory_image,
    )
}

pub fn maybe_ocr_with<C, O>(
    enabled: bool,
    recording_ok: bool,
    family: ContextFamily,
    sensitive: bool,
    ctx: &ScreenTextContext,
    window_id: Option<u64>,
    capture: C,
    ocr: O,
) -> Option<ScreenTextContext>
where
    C: FnOnce(Option<u64>) -> Result<MemoryImage, CaptureError>,
    O: FnOnce(&MemoryImage) -> Vec<String>,
{
    if !enabled || !recording_ok || sensitive || family_blocks_ocr(family) || !ctx.is_thin() {
        return None;
    }
    let image = capture(window_id).ok()?;
    let tokens = ocr(&image);
    drop(image);
    Some(merge_ocr_tokens(ctx, tokens))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn thin_ctx() -> ScreenTextContext {
        ScreenTextContext {
            tokens: vec!["Hi".into()],
            snippets: Vec::new(),
            family: ContextFamily::PersonalChat,
            source: ScreenTextSource::Ax,
            truncated: false,
        }
    }

    fn thick_ctx() -> ScreenTextContext {
        ScreenTextContext {
            tokens: vec!["abcdefghijklmnopqrstuvwxyz0123456789 extra words here".into()],
            snippets: Vec::new(),
            family: ContextFamily::PersonalChat,
            source: ScreenTextSource::Ax,
            truncated: false,
        }
    }

    #[test]
    fn ocr_skipped_when_phase1_has_fifty_chars() {
        let ctx = thick_ctx();
        assert!(ctx.usable_chars() >= 50);
        assert!(maybe_ocr(
            true,
            true,
            ContextFamily::PersonalChat,
            false,
            &ctx,
            Some(42),
        )
        .is_none());
    }

    #[test]
    fn capture_refuses_without_window_id() {
        assert!(matches!(
            capture_locked_window(None),
            Err(CaptureError::NoWindow)
        ));
        assert!(matches!(
            capture_locked_window(Some(0)),
            Err(CaptureError::NoWindow)
        ));
    }

    #[test]
    fn ocr_does_not_write_disk() {
        let mut wrote_path = false;
        let result = maybe_ocr_with(
            true,
            true,
            ContextFamily::PersonalChat,
            false,
            &thin_ctx(),
            Some(7),
            |window_id| {
                assert_eq!(window_id, Some(7));
                wrote_path = false;
                Ok(MemoryImage {
                    png: b"png-bytes".to_vec(),
                    width: 200,
                    height: 100,
                })
            },
            |image| {
                assert_eq!(image.png, b"png-bytes");
                vec!["晓雯".into()]
            },
        );
        assert!(!wrote_path);
        let merged = result.expect("thin Phase 1 should OCR");
        assert_eq!(merged.source, ScreenTextSource::AxOcr);
        assert!(merged.tokens.iter().any(|token| token == "晓雯"));
    }

    #[test]
    fn ocr_skipped_when_disabled_or_no_permission_or_sensitive() {
        let ctx = thin_ctx();
        assert!(maybe_ocr(false, true, ContextFamily::PersonalChat, false, &ctx, Some(1)).is_none());
        assert!(maybe_ocr(true, false, ContextFamily::PersonalChat, false, &ctx, Some(1)).is_none());
        assert!(maybe_ocr(true, true, ContextFamily::Terminal, false, &ctx, Some(1)).is_none());
        assert!(maybe_ocr(true, true, ContextFamily::FormFilling, false, &ctx, Some(1)).is_none());
        assert!(maybe_ocr(true, true, ContextFamily::PersonalChat, true, &ctx, Some(1)).is_none());
        assert!(maybe_ocr(true, true, ContextFamily::PersonalChat, false, &ctx, None).is_none());
    }

    #[test]
    fn downscale_caps_long_edge_at_1280() {
        assert_eq!(scaled_size(2560, 1600), (1280, 800));
        assert_eq!(scaled_size(800, 600), (800, 600));
        assert_eq!(scaled_size(600, 2400), (320, 1280));
    }

    #[test]
    fn rgba_encodes_to_png_signature() {
        let mut rgba = vec![0u8; 4];
        rgba[0] = 255;
        rgba[3] = 255;
        let png = encode_rgba_png(&rgba, 1, 1).expect("png");
        assert!(is_png(&png));
        let image = memory_image(png, 1, 1).expect("memory image");
        assert_eq!(image.width, 1);
        assert_eq!(image.height, 1);
    }

    #[test]
    fn empty_png_is_unavailable() {
        assert!(matches!(
            memory_image(Vec::new(), 640, 480),
            Err(CaptureError::Unavailable)
        ));
        assert!(ocr_memory_image(&MemoryImage {
            png: Vec::new(),
            width: 640,
            height: 480,
        })
        .is_empty());
    }
}
