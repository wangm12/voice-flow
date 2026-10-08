//! Phase 2 one-shot window capture + on-device OCR. Images stay in memory.
#![allow(dead_code)]

use std::sync::atomic::{AtomicUsize, Ordering};

use crate::context::ContextFamily;
use crate::screen_text::{
    ContextEvidenceItem, ContextEvidenceKind, ContextEvidenceSource, ScreenTextContext,
    ScreenTextSource, MAX_TOKENS,
};

static VISION_CAPTURE_COUNT: AtomicUsize = AtomicUsize::new(0);

pub const MAX_LONG_EDGE: u32 = 1280;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MemoryImage {
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
}

impl Drop for MemoryImage {
    fn drop(&mut self) {
        self.png.fill(0);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CapturedContextImage {
    pub context: ScreenTextContext,
    pub image: MemoryImage,
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
    matches!(family, ContextFamily::Terminal | ContextFamily::FormFilling)
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
    let window_id = window_id
        .filter(|id| *id != 0)
        .ok_or(CaptureError::NoWindow)?;
    capture_locked_window_id(window_id)
}

/// Capture a current window only after its caller verifies the App grant and
/// Screen Recording permission. Automatic fallback and the manual action both
/// use the same in-memory one-window path.
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
    merge_image_terms(
        ctx,
        tokens,
        ContextEvidenceSource::Ocr,
        ScreenTextSource::AxOcr,
    )
}

pub fn merge_vision_terms(ctx: &ScreenTextContext, tokens: Vec<String>) -> ScreenTextContext {
    merge_image_terms(
        ctx,
        tokens,
        ContextEvidenceSource::CloudVision,
        ScreenTextSource::CloudVision,
    )
}

fn merge_image_terms(
    ctx: &ScreenTextContext,
    tokens: Vec<String>,
    source: ContextEvidenceSource,
    source_label: ScreenTextSource,
) -> ScreenTextContext {
    let mut merged = ctx.clone();
    for line in tokens {
        for token in crate::screen_text::terms_from_ocr_line(&line) {
            if merged
                .evidence
                .items
                .iter()
                .any(|item| item.kind == ContextEvidenceKind::Term && item.value == token)
            {
                continue;
            }
            let current_terms = merged
                .evidence
                .items
                .iter()
                .filter(|item| item.kind == ContextEvidenceKind::Term)
                .count();
            if current_terms >= MAX_TOKENS {
                merged.truncated = true;
                merged.evidence.truncated = true;
                return merged;
            }
            let next_chars = merged.usable_chars() + token.chars().count();
            if next_chars > crate::screen_text::MAX_CHARS {
                merged.truncated = true;
                merged.evidence.truncated = true;
                return merged;
            }
            merged.evidence.items.push(ContextEvidenceItem {
                source,
                kind: ContextEvidenceKind::Term,
                value: token,
                confidence_milli: None,
                truncated: false,
            });
        }
    }
    if merged
        .evidence
        .items
        .iter()
        .any(|item| item.source == source)
    {
        merged.source = source_label;
    }
    merged
}

/// Capture at most one current window image for a recording. Local OCR and a
/// permitted cloud-vision fallback can share the returned in-memory image.
#[allow(clippy::too_many_arguments)]
pub fn capture_for_context_with<C, O>(
    context_enabled: bool,
    source_granted: bool,
    run_local_ocr: bool,
    recording_ok: bool,
    family: ContextFamily,
    sensitive: bool,
    ctx: &ScreenTextContext,
    window_id: Option<u64>,
    capture: C,
    ocr: O,
) -> Option<CapturedContextImage>
where
    C: FnOnce(Option<u64>) -> Result<MemoryImage, CaptureError>,
    O: FnOnce(&MemoryImage) -> Vec<String>,
{
    if !context_enabled
        || !source_granted
        || !recording_ok
        || sensitive
        || family_blocks_ocr(family)
        || !ctx.is_thin()
        || window_id.is_none_or(|id| id == 0)
    {
        return None;
    }
    let image = capture(window_id).ok()?;
    let context = if run_local_ocr {
        merge_ocr_tokens(ctx, ocr(&image))
    } else {
        ctx.clone()
    };
    Some(CapturedContextImage { context, image })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn term(value: &str) -> ContextEvidenceItem {
        ContextEvidenceItem {
            source: ContextEvidenceSource::Ax,
            kind: ContextEvidenceKind::Term,
            value: value.into(),
            confidence_milli: None,
            truncated: false,
        }
    }

    fn thin_ctx() -> ScreenTextContext {
        ScreenTextContext {
            evidence: crate::screen_text::ContextEvidence {
                items: vec![term("Hi")],
                ..Default::default()
            },
            family: ContextFamily::PersonalChat,
            source: ScreenTextSource::Ax,
            provider_source: None,
            truncated: false,
        }
    }

    fn thick_ctx() -> ScreenTextContext {
        ScreenTextContext {
            evidence: crate::screen_text::ContextEvidence {
                items: vec![term(
                    "abcdefghijklmnopqrstuvwxyz0123456789 extra words here",
                )],
                ..Default::default()
            },
            family: ContextFamily::PersonalChat,
            source: ScreenTextSource::Ax,
            provider_source: None,
            truncated: false,
        }
    }

    #[test]
    fn ocr_skipped_when_phase1_has_fifty_chars() {
        let ctx = thick_ctx();
        assert!(ctx.usable_chars() >= 50);
        assert!(capture_for_context_with(
            true,
            true,
            true,
            true,
            ContextFamily::PersonalChat,
            false,
            &ctx,
            Some(42),
            |_| panic!("AX-sufficient context must skip window capture"),
            |_| panic!("AX-sufficient context must skip OCR"),
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
    fn ocr_skipped_when_disabled_or_no_permission_or_sensitive() {
        let ctx = thin_ctx();
        let skipped =
            |context_enabled, source_granted, recording_ok, family, sensitive, window_id| {
                capture_for_context_with(
                    context_enabled,
                    source_granted,
                    true,
                    recording_ok,
                    family,
                    sensitive,
                    &ctx,
                    window_id,
                    |_| panic!("guarded capture path must not capture"),
                    |_| panic!("guarded capture path must not OCR"),
                )
                .is_none()
            };
        assert!(skipped(
            false,
            true,
            true,
            ContextFamily::PersonalChat,
            false,
            Some(1)
        ));
        assert!(skipped(
            true,
            false,
            true,
            ContextFamily::PersonalChat,
            false,
            Some(1)
        ));
        assert!(skipped(
            true,
            true,
            false,
            ContextFamily::PersonalChat,
            false,
            Some(1)
        ));
        assert!(skipped(
            true,
            true,
            true,
            ContextFamily::Terminal,
            false,
            Some(1)
        ));
        assert!(skipped(
            true,
            true,
            true,
            ContextFamily::FormFilling,
            false,
            Some(1)
        ));
        assert!(skipped(
            true,
            true,
            true,
            ContextFamily::PersonalChat,
            true,
            Some(1)
        ));
        assert!(skipped(
            true,
            true,
            true,
            ContextFamily::PersonalChat,
            false,
            None
        ));
    }

    #[test]
    fn a_single_current_window_image_is_shared_with_local_ocr() {
        use std::cell::Cell;
        let capture_calls = Cell::new(0);
        let ocr_calls = Cell::new(0);
        let captured = capture_for_context_with(
            true,
            true,
            true,
            true,
            ContextFamily::PersonalChat,
            false,
            &thin_ctx(),
            Some(5),
            |window_id| {
                assert_eq!(window_id, Some(5));
                capture_calls.set(capture_calls.get() + 1);
                Ok(MemoryImage {
                    png: b"memory-only-image".to_vec(),
                    width: 10,
                    height: 10,
                })
            },
            |image| {
                assert_eq!(image.png, b"memory-only-image");
                ocr_calls.set(ocr_calls.get() + 1);
                vec!["VoiceFlow".into()]
            },
        )
        .expect("permitted thin context captures one window");
        assert_eq!(capture_calls.get(), 1);
        assert_eq!(ocr_calls.get(), 1);
        assert_eq!(captured.image.png, b"memory-only-image");
        assert!(captured.context.evidence.items.iter().any(|item| {
            item.source == ContextEvidenceSource::Ocr && item.value == "VoiceFlow"
        }));
    }

    #[test]
    fn vision_only_capture_skips_local_ocr_and_retains_one_memory_image() {
        use std::cell::Cell;
        let capture_calls = Cell::new(0);
        let ocr_calls = Cell::new(0);
        let captured = capture_for_context_with(
            true,
            true,
            false,
            true,
            ContextFamily::PersonalChat,
            false,
            &thin_ctx(),
            Some(6),
            |_| {
                capture_calls.set(capture_calls.get() + 1);
                Ok(MemoryImage {
                    png: b"memory-only-image".to_vec(),
                    width: 10,
                    height: 10,
                })
            },
            |_| {
                ocr_calls.set(ocr_calls.get() + 1);
                Vec::new()
            },
        )
        .expect("cloud fallback can reuse the one captured image");
        assert_eq!(capture_calls.get(), 1);
        assert_eq!(ocr_calls.get(), 0);
        assert_eq!(captured.image.png, b"memory-only-image");
        assert_eq!(captured.context.source, ScreenTextSource::Ax);
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
