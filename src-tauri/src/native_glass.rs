//! Native Liquid Glass backing for the island HUD.
//!
//! The material is attached as a sibling below the existing WKWebView. The
//! window content view and WebView hierarchy remain owned by Wry, and the
//! native view stays hidden until the island publishes visible HUD content.

use serde::Serialize;
#[cfg(target_os = "macos")]
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, AtomicUsize, Ordering};
#[cfg(target_os = "macos")]
use tauri::Manager;
use tauri::{AppHandle, Emitter};

pub const STATUS_EVENT: &str = "hud-appearance://status";
pub const MATERIAL_VISIBILITY_EVENT: &str = "hud-appearance://material-visible";

#[cfg(target_os = "macos")]
const APPEARANCE_CLASSIC: u8 = 0;
#[cfg(target_os = "macos")]
const APPEARANCE_LIQUID_GLASS: u8 = 1;
#[cfg(target_os = "macos")]
const CAPABILITY_UNKNOWN: u8 = 0;
#[cfg(target_os = "macos")]
const CAPABILITY_UNSUPPORTED: u8 = 1;
#[cfg(target_os = "macos")]
const CAPABILITY_SUPPORTED: u8 = 2;

#[cfg(target_os = "macos")]
static CURRENT_REQUEST: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "macos")]
static CAPABILITY: AtomicU8 = AtomicU8::new(CAPABILITY_UNKNOWN);
#[cfg(target_os = "macos")]
static REDUCE_TRANSPARENCY: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static NATIVE_FAILURE: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static APPLIED_APPEARANCE: AtomicU8 = AtomicU8::new(APPEARANCE_CLASSIC);
#[cfg(target_os = "macos")]
static APPLIED_REVISION: AtomicU64 = AtomicU64::new(0);
#[cfg(target_os = "macos")]
static GLASS_VIEW: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_os = "macos")]
static GLASS_HOST_VIEW: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_os = "macos")]
static WEBVIEW_VIEW: AtomicUsize = AtomicUsize::new(0);
#[cfg(target_os = "macos")]
static MATERIAL_VISIBLE: AtomicBool = AtomicBool::new(false);
#[cfg(target_os = "macos")]
static ACCESSIBILITY_OBSERVER_INSTALLED: AtomicBool = AtomicBool::new(false);

#[cfg(target_os = "macos")]
fn applied_appearance() -> &'static str {
    if APPLIED_APPEARANCE.load(Ordering::Acquire) == APPEARANCE_LIQUID_GLASS
        && CAPABILITY.load(Ordering::Acquire) == CAPABILITY_SUPPORTED
        && !REDUCE_TRANSPARENCY.load(Ordering::Acquire)
        && !NATIVE_FAILURE.load(Ordering::Acquire)
        && GLASS_VIEW.load(Ordering::Acquire) != 0
    {
        "liquid_glass"
    } else {
        "classic"
    }
}

#[cfg(not(target_os = "macos"))]
fn applied_appearance() -> &'static str {
    "classic"
}

#[cfg(target_os = "macos")]
fn current_request() -> (u64, u8) {
    let request = CURRENT_REQUEST.load(Ordering::Acquire);
    (request >> 1, (request & 1) as u8)
}

#[cfg(target_os = "macos")]
fn publish_request(requested: &str) -> u64 {
    let appearance = encode_appearance(requested) as u64;
    let mut current = CURRENT_REQUEST.load(Ordering::Acquire);
    loop {
        let revision = (current >> 1) + 1;
        let next = (revision << 1) | appearance;
        match CURRENT_REQUEST.compare_exchange_weak(
            current,
            next,
            Ordering::AcqRel,
            Ordering::Acquire,
        ) {
            Ok(_) => return revision,
            Err(observed) => current = observed,
        }
    }
}

#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct HudAppearanceStatus {
    pub requested: &'static str,
    pub effective: &'static str,
    pub fallback_reason: Option<&'static str>,
    pub pending: bool,
}

pub fn status(requested: &str) -> HudAppearanceStatus {
    if requested != "liquid_glass" {
        #[cfg(target_os = "macos")]
        let pending = {
            let (request_revision, requested_appearance) = current_request();
            request_revision != APPLIED_REVISION.load(Ordering::Acquire)
                || requested_appearance != APPEARANCE_CLASSIC
        };
        #[cfg(not(target_os = "macos"))]
        let pending = false;
        let effective = applied_appearance();
        #[cfg(target_os = "macos")]
        let fallback_reason = if pending || effective == "classic" {
            None
        } else {
            Some("native_failure")
        };
        #[cfg(not(target_os = "macos"))]
        let fallback_reason = None;
        return HudAppearanceStatus {
            requested: "classic",
            effective,
            fallback_reason,
            pending,
        };
    }

    #[cfg(not(target_os = "macos"))]
    return HudAppearanceStatus {
        requested: "liquid_glass",
        effective: "classic",
        fallback_reason: Some("unsupported_os"),
        pending: false,
    };

    #[cfg(target_os = "macos")]
    {
        let (request_revision, current_appearance) = current_request();
        let pending = request_revision != APPLIED_REVISION.load(Ordering::Acquire)
            || current_appearance != APPEARANCE_LIQUID_GLASS;
        let requested_appearance = encode_appearance(requested);
        let fallback_reason = if pending {
            None
        } else if CAPABILITY.load(Ordering::Acquire) == CAPABILITY_UNSUPPORTED {
            Some("unsupported_os")
        } else if REDUCE_TRANSPARENCY.load(Ordering::Acquire) {
            Some("reduce_transparency")
        } else if requested_appearance != APPEARANCE_LIQUID_GLASS
            || applied_appearance() != "liquid_glass"
        {
            Some("native_failure")
        } else {
            None
        };
        HudAppearanceStatus {
            requested: "liquid_glass",
            effective: if pending {
                applied_appearance()
            } else if fallback_reason.is_none() {
                "liquid_glass"
            } else {
                "classic"
            },
            fallback_reason,
            pending,
        }
    }
}

#[cfg(all(target_os = "macos", feature = "native-glass-poc"))]
pub fn is_installed() -> bool {
    GLASS_VIEW.load(Ordering::Acquire) != 0
}

#[cfg(all(target_os = "macos", feature = "native-glass-poc"))]
pub fn view_pointers() -> (usize, usize, usize) {
    (
        GLASS_HOST_VIEW.load(Ordering::Acquire),
        WEBVIEW_VIEW.load(Ordering::Acquire),
        GLASS_VIEW.load(Ordering::Acquire),
    )
}

#[cfg(target_os = "macos")]
pub fn material_is_visible() -> bool {
    MATERIAL_VISIBLE.load(Ordering::Acquire)
}

#[cfg(target_os = "macos")]
pub fn should_be_material_visible(hud_owner_visible: bool) -> bool {
    hud_owner_visible && applied_appearance() == "liquid_glass"
}

#[cfg(not(target_os = "macos"))]
pub fn material_is_visible() -> bool {
    false
}

#[cfg(not(target_os = "macos"))]
pub fn should_be_material_visible(_hud_owner_visible: bool) -> bool {
    false
}

/// Queue the requested appearance on AppKit's main thread and return the
/// currently applied status without waiting for view installation or display.
/// A newer request supersedes any queued older request.
pub fn configure_and_publish(app: &AppHandle, requested: &str) -> HudAppearanceStatus {
    #[cfg(target_os = "macos")]
    {
        let revision = publish_request(requested);
        let app_handle = app.clone();
        let scheduled_requested = requested.to_owned();
        if let Err(error) = app.run_on_main_thread(move || {
            if current_request().0 != revision
                || APPLIED_REVISION.load(Ordering::Acquire) == revision
            {
                return;
            }
            let visible = crate::island_window::hud_should_show_native_material();
            let status = app_handle
                .get_webview_window("island")
                .map(|window| {
                    configure_on_main_thread_at_revision(
                        &window,
                        &scheduled_requested,
                        visible,
                        revision,
                    )
                })
                .unwrap_or_else(|| {
                    if scheduled_requested == "liquid_glass"
                        && applied_appearance() != "liquid_glass"
                    {
                        NATIVE_FAILURE.store(true, Ordering::Release);
                    }
                    APPLIED_REVISION.store(revision, Ordering::Release);
                    status(&scheduled_requested)
                });
            if current_request().0 == revision {
                let _ = app_handle.emit(MATERIAL_VISIBILITY_EVENT, material_is_visible());
                let _ = app_handle.emit(STATUS_EVENT, &status);
            }
        }) {
            log::warn!("failed to schedule native HUD appearance update: {error}");
            if requested == "liquid_glass" && applied_appearance() != "liquid_glass" {
                NATIVE_FAILURE.store(true, Ordering::Release);
            }
            APPLIED_REVISION.store(revision, Ordering::Release);
            return status(requested);
        }
        return status(requested);
    }

    #[cfg(not(target_os = "macos"))]
    {
        let next = status(requested);
        let _ = app.emit(STATUS_EVENT, &next);
        let _ = app.emit(MATERIAL_VISIBILITY_EVENT, false);
        next
    }
}

#[cfg(target_os = "macos")]
fn encode_appearance(requested: &str) -> u8 {
    if requested == "liquid_glass" {
        APPEARANCE_LIQUID_GLASS
    } else {
        APPEARANCE_CLASSIC
    }
}

/// Called from Tauri setup after the island's existing NSPanel conversion.
#[cfg(target_os = "macos")]
pub fn configure_on_main_thread(
    window: &tauri::WebviewWindow,
    requested: &str,
    visible: bool,
) -> HudAppearanceStatus {
    let revision = publish_request(requested);
    configure_on_main_thread_at_revision(window, requested, visible, revision)
}

#[cfg(target_os = "macos")]
fn configure_on_main_thread_at_revision(
    window: &tauri::WebviewWindow,
    requested: &str,
    visible: bool,
    revision: u64,
) -> HudAppearanceStatus {
    NATIVE_FAILURE.store(false, Ordering::Release);

    if requested != "liquid_glass" {
        set_hidden(true);
        APPLIED_APPEARANCE.store(APPEARANCE_CLASSIC, Ordering::Release);
        APPLIED_REVISION.store(revision, Ordering::Release);
        return status("classic");
    }

    match runtime_capability() {
        Ok(()) => {
            CAPABILITY.store(CAPABILITY_SUPPORTED, Ordering::Release);
        }
        Err("unsupported_os") => {
            CAPABILITY.store(CAPABILITY_UNSUPPORTED, Ordering::Release);
            set_hidden(true);
            APPLIED_APPEARANCE.store(APPEARANCE_CLASSIC, Ordering::Release);
            APPLIED_REVISION.store(revision, Ordering::Release);
            return status(requested);
        }
        Err("reduce_transparency") => {
            CAPABILITY.store(CAPABILITY_SUPPORTED, Ordering::Release);
            REDUCE_TRANSPARENCY.store(true, Ordering::Release);
            set_hidden(true);
            APPLIED_APPEARANCE.store(APPEARANCE_CLASSIC, Ordering::Release);
            APPLIED_REVISION.store(revision, Ordering::Release);
            return status(requested);
        }
        Err(_) => unreachable!(),
    }

    REDUCE_TRANSPARENCY.store(false, Ordering::Release);
    if GLASS_VIEW.load(Ordering::Acquire) == 0 {
        match install_native_glass(window) {
            Ok(true) => {}
            Ok(false) => {
                NATIVE_FAILURE.store(true, Ordering::Release);
                set_hidden(true);
                APPLIED_APPEARANCE.store(APPEARANCE_CLASSIC, Ordering::Release);
                APPLIED_REVISION.store(revision, Ordering::Release);
                return status(requested);
            }
            Err(error) => {
                log::warn!("native HUD glass unavailable; using classic appearance: {error}");
                NATIVE_FAILURE.store(true, Ordering::Release);
                set_hidden(true);
                APPLIED_APPEARANCE.store(APPEARANCE_CLASSIC, Ordering::Release);
                APPLIED_REVISION.store(revision, Ordering::Release);
                return status(requested);
            }
        }
    }
    APPLIED_APPEARANCE.store(APPEARANCE_LIQUID_GLASS, Ordering::Release);
    set_hud_visible_on_main(visible);
    APPLIED_REVISION.store(revision, Ordering::Release);
    status(requested)
}

#[cfg(target_os = "macos")]
pub fn prewarm_on_main_thread(window: &tauri::WebviewWindow) {
    if current_request().1 == APPEARANCE_CLASSIC {
        prewarm_native_glass(window);
    }
}

#[cfg(target_os = "macos")]
fn prewarm_native_glass(window: &tauri::WebviewWindow) {
    match runtime_capability() {
        Ok(()) => {
            CAPABILITY.store(CAPABILITY_SUPPORTED, Ordering::Release);
            REDUCE_TRANSPARENCY.store(false, Ordering::Release);
            if GLASS_VIEW.load(Ordering::Acquire) == 0 {
                match install_native_glass(window) {
                    Ok(true) => {}
                    Ok(false) => log::debug!("native HUD glass prewarm is unavailable"),
                    Err(error) => log::debug!("native HUD glass prewarm failed: {error}"),
                }
            }
        }
        Err("unsupported_os") => {
            CAPABILITY.store(CAPABILITY_UNSUPPORTED, Ordering::Release);
            REDUCE_TRANSPARENCY.store(false, Ordering::Release);
        }
        Err("reduce_transparency") => {
            CAPABILITY.store(CAPABILITY_SUPPORTED, Ordering::Release);
            REDUCE_TRANSPARENCY.store(true, Ordering::Release);
        }
        Err(_) => unreachable!(),
    }
}

#[cfg(target_os = "macos")]
pub fn capture_webview(window: &tauri::WebviewWindow) {
    match locate_native_webview(window) {
        Ok(Some((host, webview))) => {
            GLASS_HOST_VIEW.store(host, Ordering::Release);
            WEBVIEW_VIEW.store(webview, Ordering::Release);
        }
        Ok(None) => log::debug!("island content view has no direct WKWebView child"),
        Err(error) => log::debug!("island WKWebView lookup failed: {error}"),
    }
}

#[cfg(target_os = "macos")]
pub fn set_hud_visible_on_main(visible: bool) -> bool {
    if applied_appearance() != "liquid_glass" {
        set_hidden(true);
        return false;
    }
    let visible = visible && GLASS_VIEW.load(Ordering::Acquire) != 0;
    set_hidden(!visible);
    visible
}

#[cfg(not(target_os = "macos"))]
pub fn set_hud_visible_on_main(_visible: bool) -> bool {
    false
}

pub fn publish_material_visibility(app: &AppHandle) {
    #[cfg(target_os = "macos")]
    let visible = material_is_visible();
    #[cfg(not(target_os = "macos"))]
    let visible = false;
    let _ = app.emit(MATERIAL_VISIBILITY_EVENT, visible);
}

#[cfg(target_os = "macos")]
pub fn install_accessibility_observer(app: &AppHandle) {
    if ACCESSIBILITY_OBSERVER_INSTALLED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }

    use core_foundation::{base::TCFType, string::CFString};
    use objc::{class, msg_send, sel, sel_impl};

    let app = app.clone();
    let callback = block2::RcBlock::new(move |_notification: *mut objc2::runtime::AnyObject| {
        let app = app.clone();
        let scheduled = app.clone();
        if let Err(error) = scheduled.run_on_main_thread(move || {
            // This is a runtime capability refresh, not a new user request.
            // Keep the current revision so an accessibility notification can
            // never overwrite a newer settings selection.
            let (revision, requested_value) = current_request();
            let requested = decode_appearance(requested_value);
            if current_request().0 != revision {
                return;
            }
            let visible = crate::island_window::hud_should_show_native_material();
            let next = app
                .get_webview_window("island")
                .map(|window| {
                    let next =
                        configure_on_main_thread_at_revision(&window, requested, visible, revision);
                    if requested == "classic" {
                        prewarm_native_glass(&window);
                    }
                    next
                })
                .unwrap_or_else(|| status(requested));
            if current_request().0 == revision {
                publish_material_visibility(&app);
                let _ = app.emit(STATUS_EVENT, next);
            }
        }) {
            log::debug!(
                "failed to refresh native HUD appearance after display option change: {error}"
            );
        }
    });

    unsafe {
        let workspace: *mut objc::runtime::Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let center: *mut objc::runtime::Object = msg_send![workspace, notificationCenter];
        let name = CFString::new("NSWorkspaceAccessibilityDisplayOptionsDidChangeNotification");
        let _: *mut objc::runtime::Object = msg_send![center, addObserverForName: name.as_concrete_TypeRef()
            object: std::ptr::null::<objc::runtime::Object>()
            queue: std::ptr::null::<objc::runtime::Object>()
            usingBlock: &*callback];
    }
    std::mem::forget(callback);
}

#[cfg(not(target_os = "macos"))]
pub fn install_accessibility_observer(_app: &AppHandle) {}

#[cfg(target_os = "macos")]
fn decode_appearance(value: u8) -> &'static str {
    if value == APPEARANCE_LIQUID_GLASS {
        "liquid_glass"
    } else {
        "classic"
    }
}

#[cfg(target_os = "macos")]
fn runtime_capability() -> Result<(), &'static str> {
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    #[repr(C)]
    struct OperatingSystemVersion {
        major: isize,
        minor: isize,
        patch: isize,
    }

    unsafe {
        let Some(process_info_class) = Class::get("NSProcessInfo") else {
            return Err("unsupported_os");
        };
        let process_info: *mut Object = msg_send![process_info_class, processInfo];
        let version: OperatingSystemVersion = msg_send![process_info, operatingSystemVersion];
        if version.major < 26 || Class::get("NSGlassEffectView").is_none() {
            return Err("unsupported_os");
        }
        let workspace_class = Class::get("NSWorkspace").ok_or("unsupported_os")?;
        let workspace: *mut Object = msg_send![workspace_class, sharedWorkspace];
        let reduce_transparency: bool =
            msg_send![workspace, accessibilityDisplayShouldReduceTransparency];
        REDUCE_TRANSPARENCY.store(reduce_transparency, Ordering::Release);
        if reduce_transparency {
            return Err("reduce_transparency");
        }
    }
    Ok(())
}

#[cfg(target_os = "macos")]
fn install_native_glass(window: &tauri::WebviewWindow) -> Result<bool, String> {
    use core_graphics::geometry::{CGPoint, CGRect, CGSize};
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    unsafe {
        let Some(glass_class) = Class::get("NSGlassEffectView") else {
            return Ok(false);
        };
        let Some((content_view, webview)) = locate_native_webview(window)? else {
            return Ok(false);
        };
        let content_view = content_view as *mut Object;
        let webview = webview as *mut Object;
        let content_bounds: CGRect = msg_send![content_view, bounds];
        let x = ((content_bounds.size.width - 220.0) / 2.0).max(0.0);
        let frame = CGRect::new(&CGPoint::new(x, 8.0), &CGSize::new(220.0, 48.0));
        let allocated: *mut Object = msg_send![glass_class, alloc];
        let glass: *mut Object = msg_send![allocated, initWithFrame: frame];
        if glass.is_null() {
            return Err("could not allocate NSGlassEffectView".to_owned());
        }

        // NSGlassEffectViewStyleRegular is 0 in AppKit's public enum.
        // Keep the pill centered when the island window expands horizontally.
        const NS_VIEW_MIN_X_MARGIN: usize = 1;
        const NS_VIEW_MAX_X_MARGIN: usize = 4;
        let _: () = msg_send![glass, setStyle: 0isize];
        let _: () = msg_send![glass, setCornerRadius: 24.0f64];
        let _: () =
            msg_send![glass, setAutoresizingMask: NS_VIEW_MIN_X_MARGIN | NS_VIEW_MAX_X_MARGIN];
        let _: () = msg_send![glass, setHidden: true];

        // Preserve Wry's contentView and WKWebView attachment. Insert only a
        // lower sibling so the existing WebView remains the interaction owner.
        let _: () = msg_send![content_view, addSubview: glass positioned: -1isize
            relativeTo: webview];
        let _: () = msg_send![glass, release];

        GLASS_VIEW.store(glass as usize, Ordering::Release);
        GLASS_HOST_VIEW.store(content_view as usize, Ordering::Release);
        WEBVIEW_VIEW.store(webview as usize, Ordering::Release);
    }
    Ok(true)
}

#[cfg(target_os = "macos")]
fn locate_native_webview(window: &tauri::WebviewWindow) -> Result<Option<(usize, usize)>, String> {
    use objc::runtime::{Class, Object};
    use objc::{msg_send, sel, sel_impl};

    unsafe {
        let Some(webview_class) = Class::get("WKWebView") else {
            return Ok(None);
        };
        let native_window = window.ns_window().map_err(|error| error.to_string())? as *mut Object;
        let content_view: *mut Object = msg_send![native_window, contentView];
        if content_view.is_null() {
            return Err("NSWindow has no content view".to_owned());
        }
        let subviews: *mut Object = msg_send![content_view, subviews];
        let subview_count: usize = msg_send![subviews, count];
        for index in 0..subview_count {
            let child: *mut Object = msg_send![subviews, objectAtIndex: index];
            let is_webview: objc::runtime::BOOL = msg_send![child, isKindOfClass: webview_class];
            if is_webview != objc::runtime::NO {
                return Ok(Some((content_view as usize, child as usize)));
            }
        }
        Ok(None)
    }
}

#[cfg(target_os = "macos")]
fn set_hidden(hidden: bool) {
    let glass = GLASS_VIEW.load(Ordering::Acquire);
    if glass == 0 {
        MATERIAL_VISIBLE.store(false, Ordering::Release);
        return;
    }
    unsafe {
        use objc::{msg_send, sel, sel_impl};
        let _: () = msg_send![glass as *mut objc::runtime::Object, setHidden: hidden];
    }
    MATERIAL_VISIBLE.store(!hidden, Ordering::Release);
}
