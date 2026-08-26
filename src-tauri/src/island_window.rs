//! Island overlay as NSPanel — required to float above native macOS fullscreen apps.
//!
//! Uses the same flags as tauri-macos-spotlight-example:
//! MoveToActiveSpace + FullScreenAuxiliary + NonActivatingPanel.

use std::sync::atomic::{AtomicBool, Ordering};

use tauri::{AppHandle, Manager, WebviewWindow};

static ISLAND_PANEL_READY: AtomicBool = AtomicBool::new(false);
/// A converted macOS NSPanel stays ordered in to keep the WebView surface warm
/// between dictations. In that mode native hide/show reconciliation is not
/// needed; the Island document hides its own content while idle.
static ISLAND_PANEL_WARM: AtomicBool = AtomicBool::new(false);
static ISLAND_VISIBLE: AtomicBool = AtomicBool::new(false);
static ISLAND_RECONCILE_SCHEDULED: AtomicBool = AtomicBool::new(false);
static ISLAND_DESIRED_VISIBLE: AtomicBool = AtomicBool::new(false);
static ISLAND_HAS_PARTIAL: AtomicBool = AtomicBool::new(false);
static ISLAND_HAS_WIDE_CAPTION: AtomicBool = AtomicBool::new(false);
static ISLAND_WIDE_APPLIED: AtomicBool = AtomicBool::new(false);
/// While VoiceFlow is delivering text, the HUD must stay click-through and
/// must not order itself in front of the target app's key window.
static ISLAND_PASTE_YIELD: AtomicBool = AtomicBool::new(false);
/// The dictionary learn toast is the only HUD surface that needs mouse hits.
static ISLAND_LEARN_TOAST: AtomicBool = AtomicBool::new(false);

pub fn ensure_panel(window: &WebviewWindow) {
    if ISLAND_PANEL_READY.load(Ordering::Acquire) {
        return;
    }
    let _ = window.set_visible_on_all_workspaces(true);
    #[cfg(target_os = "macos")]
    ensure_panel_macos(window);
    #[cfg(not(target_os = "macos"))]
    ISLAND_PANEL_READY.store(true, Ordering::Release);
}

pub fn show_overlay(app: &AppHandle) {
    // Reconcile on every show request, even when the panel is already warm.
    // The active display can change while VoiceFlow is running; keeping the
    // initial frame would leave the HUD partly or completely outside the new
    // screen. `ISLAND_RECONCILE_SCHEDULED` coalesces repeated state updates.
    ISLAND_DESIRED_VISIBLE.store(true, Ordering::Release);
    schedule_reconcile(app);
}

pub fn hide_overlay(app: &AppHandle) {
    let was_desired = ISLAND_DESIRED_VISIBLE.swap(false, Ordering::AcqRel);
    if ISLAND_PANEL_WARM.load(Ordering::Acquire)
        || (!was_desired && !ISLAND_VISIBLE.load(Ordering::Acquire))
    {
        return;
    }
    schedule_reconcile(app);
}

/// The HUD is click-through by default. The dictionary learn toast is the only
/// surface that needs the native panel to accept mouse events.
pub fn set_interactive(app: &AppHandle, interactive: bool) {
    if ISLAND_PASTE_YIELD.load(Ordering::Acquire) && interactive {
        return;
    }
    let Some(window) = app.get_webview_window("island") else {
        return;
    };
    if let Err(error) = window.set_ignore_cursor_events(!interactive) {
        log::debug!("failed to update island mouse mode: {error}");
    }
}

pub fn is_yielding_for_paste() -> bool {
    ISLAND_PASTE_YIELD.load(Ordering::Acquire)
}

pub fn is_learn_toast_interactive() -> bool {
    ISLAND_LEARN_TOAST.load(Ordering::Acquire)
}

pub fn set_learn_toast_interactive(app: &AppHandle, interactive: bool) {
    ISLAND_LEARN_TOAST.store(interactive, Ordering::Release);
    set_interactive(app, interactive);
}

/// Make the HUD click-through and resign key before synthesized Cmd+V.
/// Blocks until the main-thread resign finishes so activate/paste cannot race it.
pub fn prepare_for_paste(app: &AppHandle) {
    ISLAND_PASTE_YIELD.store(true, Ordering::Release);
    set_interactive(app, false);
    let Some(window) = app.get_webview_window("island") else {
        return;
    };
    let (tx, rx) = std::sync::mpsc::channel();
    let app = app.clone();
    if window
        .run_on_main_thread(move || {
            resign_key_and_ignore_mouse(&app);
            let _ = tx.send(());
        })
        .is_err()
    {
        return;
    }
    let _ = rx.recv_timeout(std::time::Duration::from_millis(200));
}

pub fn end_paste_yield() {
    ISLAND_PASTE_YIELD.store(false, Ordering::Release);
}

fn resign_key_and_ignore_mouse(app: &AppHandle) {
    let Some(window) = app.get_webview_window("island") else {
        return;
    };
    if let Err(error) = window.set_ignore_cursor_events(true) {
        log::debug!("failed to ignore island mouse events before paste: {error}");
    }
    #[cfg(target_os = "macos")]
    {
        use tauri_nspanel::ManagerExt;
        if let Ok(panel) = app.get_webview_panel("island") {
            panel.set_ignore_mouse_events(true);
        }
        resign_island_key_window(&window);
    }
}

#[cfg(target_os = "macos")]
fn resign_island_key_window(window: &WebviewWindow) {
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    let Ok(ptr) = window.ns_window() else {
        return;
    };
    unsafe {
        let ns_window = ptr as *mut Object;
        let is_key: bool = msg_send![ns_window, isKeyWindow];
        if is_key {
            let _: () = msg_send![ns_window, resignKeyWindow];
        }
        let _: () = msg_send![ns_window, setIgnoresMouseEvents: true];
    }
}

/// Expand the transparent island surface while HUD partials or long status
/// captions are visible, then shrink it again so idle does not keep a 400px
/// hit target in WindowServer.
pub fn set_has_partial(app: &AppHandle, has_partial: bool) {
    ISLAND_HAS_PARTIAL.store(has_partial, Ordering::Release);
    apply_island_width(app);
}

pub fn set_has_wide_caption(app: &AppHandle, wide: bool) {
    ISLAND_HAS_WIDE_CAPTION.store(wide, Ordering::Release);
    apply_island_width(app);
}

fn island_should_expand() -> bool {
    ISLAND_HAS_PARTIAL.load(Ordering::Acquire) || ISLAND_HAS_WIDE_CAPTION.load(Ordering::Acquire)
}

fn apply_island_width(app: &AppHandle) {
    let next = island_should_expand();
    let previous = ISLAND_WIDE_APPLIED.swap(next, Ordering::AcqRel);
    if previous == next {
        return;
    }
    let Some(window) = app.get_webview_window("island") else {
        return;
    };
    let app = app.clone();
    if let Err(error) = window.run_on_main_thread(move || {
        if let Some(window) = app.get_webview_window("island") {
            position_overlay_on_main(&window, &app);
        }
    }) {
        log::debug!("failed to resize island for partial: {error}");
    }
}

fn schedule_reconcile(app: &AppHandle) {
    if ISLAND_RECONCILE_SCHEDULED
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return;
    }
    let Some(window) = app.get_webview_window("island") else {
        ISLAND_RECONCILE_SCHEDULED.store(false, Ordering::Release);
        return;
    };
    let app = app.clone();
    if let Err(error) = window.run_on_main_thread(move || {
        reconcile_on_main(&app);
        ISLAND_RECONCILE_SCHEDULED.store(false, Ordering::Release);
        if !ISLAND_PANEL_WARM.load(Ordering::Acquire)
            && ISLAND_DESIRED_VISIBLE.load(Ordering::Acquire)
                != ISLAND_VISIBLE.load(Ordering::Acquire)
        {
            schedule_reconcile(&app);
        }
    }) {
        ISLAND_RECONCILE_SCHEDULED.store(false, Ordering::Release);
        log::debug!("failed to schedule island visibility update: {error}");
    }
}

fn reconcile_on_main(app: &AppHandle) {
    let Some(window) = app.get_webview_window("island") else {
        return;
    };

    if ISLAND_DESIRED_VISIBLE.load(Ordering::Acquire) {
        show_overlay_on_main(&window, app);
    } else {
        hide_overlay_on_main(&window);
    }
}

fn show_overlay_on_main(window: &tauri::WebviewWindow, app: &AppHandle) {
    position_overlay_on_main(window, app);

    if !ISLAND_VISIBLE.swap(true, Ordering::AcqRel) {
        // Keep the WebView warm, but do not keep the transparent native surface in
        // WindowServer's composition tree while the HUD is idle. This native
        // boundary is crossed only once per visible HUD session, never per frame.
        if let Err(error) = window.show() {
            ISLAND_VISIBLE.store(false, Ordering::Release);
            log::warn!("failed to show island overlay: {error}");
            return;
        }
    }
    // `window.show()` can reorder against the focused settings window. Re-apply
    // the overlay level last so the HUD stays above every other app window.
    raise_overlay_above_all_windows(window, app);
}

fn position_overlay_on_main(window: &tauri::WebviewWindow, app: &AppHandle) {
    let placement = crate::notch::placement_for_cursor_screen_with_width(
        app,
        crate::notch::pill_window_width(island_should_expand()),
    );
    let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
        x: placement.x.round() as i32,
        y: placement.y.round() as i32,
    }));
    let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
        width: placement.width.round().max(1.0) as u32,
        height: placement.height.round().max(1.0) as u32,
    }));
}

fn raise_overlay_above_all_windows(window: &tauri::WebviewWindow, app: &AppHandle) {
    // Do not call `set_always_on_top`. Tao maps that to NSFloatingWindowLevel
    // (3) on an async main-queue hop, which is below the Dock (20) and undoes
    // the overlay level we just applied.
    #[cfg(target_os = "macos")]
    {
        apply_overlay_window_level(window);
        if !should_order_front_overlay() {
            return;
        }
        use tauri_nspanel::ManagerExt;
        if let Ok(panel) = app.get_webview_panel("island") {
            panel.set_level(island_overlay_window_level());
            panel.order_front_regardless();
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = window.set_always_on_top(true);
        let _ = app;
    }
}

#[cfg(target_os = "macos")]
fn apply_overlay_window_level(window: &tauri::WebviewWindow) {
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    let Ok(ptr) = window.ns_window() else {
        return;
    };
    unsafe {
        let ns_window = ptr as *mut Object;
        let _: () = msg_send![ns_window, setLevel: island_overlay_window_level()];
    }
}

#[cfg(target_os = "macos")]
fn hide_overlay_on_main(window: &tauri::WebviewWindow) {
    // Keep the transparent, non-activating panel warm between dictations.
    // Ordering the NSPanel out here forces WindowServer/WebKit to rebuild the
    // overlay surface on the next hotkey press, which is visible as a hitch
    // even though the HUD's CSS animation is compositor-friendly. The island
    // document hides its content with opacity/visibility, so a warm
    // transparent panel has no visible idle footprint.
    if ISLAND_PANEL_WARM.load(Ordering::Acquire) {
        return;
    }
    if !ISLAND_VISIBLE.swap(false, Ordering::AcqRel) {
        return;
    }
    if let Err(error) = window.hide() {
        ISLAND_VISIBLE.store(true, Ordering::Release);
        log::warn!("failed to hide island overlay: {error}");
    }
}

#[cfg(not(target_os = "macos"))]
fn hide_overlay_on_main(window: &tauri::WebviewWindow) {
    if !ISLAND_VISIBLE.swap(false, Ordering::AcqRel) {
        return;
    }

    if let Err(error) = window.hide() {
        ISLAND_VISIBLE.store(true, Ordering::Release);
        log::warn!("failed to hide island overlay: {error}");
    }
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
fn ensure_panel_macos(window: &WebviewWindow) {
    use tauri_nspanel::cocoa::appkit::NSWindowCollectionBehavior;
    use tauri_nspanel::WebviewWindowExt;

    const NS_WINDOW_STYLE_MASK_NON_ACTIVATING_PANEL: i32 = 1 << 7;

    let panel = match window.to_panel() {
        Ok(panel) => {
            log::info!("island window converted to NSPanel");
            Some(panel)
        }
        Err(error) => {
            log::error!("failed to convert island window to NSPanel: {error}");
            None
        }
    };

    let Some(panel) = panel else {
        return;
    };

    panel.set_style_mask(NS_WINDOW_STYLE_MASK_NON_ACTIVATING_PANEL);
    // CSS z-index cannot place a WebView above a macOS full-screen Space. The
    // native panel must explicitly participate in those Spaces and remain
    // above normal/status-level application windows.
    panel.set_collection_behaviour(
        NSWindowCollectionBehavior::NSWindowCollectionBehaviorCanJoinAllSpaces
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorFullScreenAuxiliary
            | NSWindowCollectionBehavior::NSWindowCollectionBehaviorIgnoresCycle,
    );
    // `setFloatingPanel:YES` assigns NSFloatingWindowLevel. Set the overlay
    // level after that so settings and other app windows cannot cover the HUD.
    panel.set_floating_panel(true);
    panel.set_works_when_modal(true);
    panel.set_level(island_overlay_window_level());
    apply_overlay_window_level(window);
    panel.set_hides_on_deactivate(false);
    panel.set_has_shadow(false);
    panel.set_becomes_key_only_if_needed(true);
    panel.set_ignore_mouse_events(true);
    // Warm the WebView once at startup and keep the transparent panel ordered
    // in. The island document is hidden while idle; keeping the native surface
    // warm avoids an order-in/WebKit hitch at the first frame of each session.
    panel.set_alpha_value(1.0);
    let warmed = if let Err(error) = window.show() {
        log::warn!("failed to warm island overlay: {error}");
        false
    } else {
        ISLAND_VISIBLE.store(true, Ordering::Release);
        true
    };
    panel.set_level(island_overlay_window_level());
    apply_overlay_window_level(window);
    panel.order_front_regardless();
    ISLAND_PANEL_WARM.store(warmed, Ordering::Release);
    ISLAND_PANEL_READY.store(true, Ordering::Release);
}

#[cfg(not(target_os = "macos"))]
fn ensure_panel_macos(_window: &WebviewWindow) {}

/// `kCGOverlayWindowLevel` (102). Above popup menus (101), the menu bar,
/// status items, and VoiceFlow's own settings window.
pub fn island_overlay_window_level() -> i32 {
    102
}

fn should_order_front_overlay() -> bool {
    !ISLAND_PASTE_YIELD.load(Ordering::Acquire)
}

#[cfg(test)]
mod tests {
    use super::{end_paste_yield, should_order_front_overlay, ISLAND_PASTE_YIELD};
    use std::sync::atomic::Ordering;

    #[test]
    fn island_overlay_sits_above_popup_menus_and_app_windows() {
        assert!(
            super::island_overlay_window_level() > 20,
            "HUD must sit above NSDockWindowLevel (20) so the Dock cannot cover it"
        );
        assert!(
            super::island_overlay_window_level() >= 101,
            "HUD must sit at or above NSPopUpMenuWindowLevel so settings and other app windows cannot cover it"
        );
    }

    #[test]
    fn paste_yield_suppresses_order_front() {
        let previous = ISLAND_PASTE_YIELD.swap(false, Ordering::AcqRel);
        assert!(should_order_front_overlay());
        ISLAND_PASTE_YIELD.store(true, Ordering::Release);
        assert!(!should_order_front_overlay());
        end_paste_yield();
        assert!(should_order_front_overlay());
        ISLAND_PASTE_YIELD.store(previous, Ordering::Release);
    }

    #[test]
    fn learn_toast_is_the_only_interactive_hud_surface() {
        let previous = super::ISLAND_LEARN_TOAST.swap(false, Ordering::AcqRel);
        assert!(!super::is_learn_toast_interactive());
        super::ISLAND_LEARN_TOAST.store(true, Ordering::Release);
        assert!(super::is_learn_toast_interactive());
        super::ISLAND_LEARN_TOAST.store(previous, Ordering::Release);
    }
}
