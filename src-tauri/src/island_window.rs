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

/// The HUD is click-through while idle, but its action buttons need the native
/// panel to accept mouse events during an active dictation session.
pub fn set_interactive(app: &AppHandle, interactive: bool) {
    let Some(window) = app.get_webview_window("island") else {
        return;
    };
    if let Err(error) = window.set_ignore_cursor_events(!interactive) {
        log::debug!("failed to update island mouse mode: {error}");
    }
}

/// Expand the transparent island surface while HUD partials are visible, then
/// shrink it again so idle does not keep a 400px hit target in WindowServer.
pub fn set_has_partial(app: &AppHandle, has_partial: bool) {
    let previous = ISLAND_HAS_PARTIAL.swap(has_partial, Ordering::AcqRel);
    if previous == has_partial {
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

fn show_overlay_on_main(window: &tauri::WebviewWindow, _app: &AppHandle) {
    position_overlay_on_main(window, _app);

    if ISLAND_VISIBLE.swap(true, Ordering::AcqRel) {
        return;
    }

    // Keep the WebView warm, but do not keep the transparent native surface in
    // WindowServer's composition tree while the HUD is idle. This native
    // boundary is crossed only once per visible HUD session, never per frame.
    if let Err(error) = window.show() {
        ISLAND_VISIBLE.store(false, Ordering::Release);
        log::warn!("failed to show island overlay: {error}");
    }
}

fn position_overlay_on_main(window: &tauri::WebviewWindow, app: &AppHandle) {
    let placement = crate::notch::placement_for_cursor_screen_with_width(
        app,
        crate::notch::pill_window_width(ISLAND_HAS_PARTIAL.load(Ordering::Acquire)),
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
    use tauri_nspanel::cocoa::appkit::{NSMainMenuWindowLevel, NSWindowCollectionBehavior};
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
    panel.set_level(NSMainMenuWindowLevel + 2);
    panel.set_floating_panel(true);
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
    ISLAND_PANEL_WARM.store(warmed, Ordering::Release);
    ISLAND_PANEL_READY.store(true, Ordering::Release);
}

#[cfg(not(target_os = "macos"))]
fn ensure_panel_macos(_window: &WebviewWindow) {}
