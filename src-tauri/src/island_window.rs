//! Island overlay as NSPanel — required to float above native macOS fullscreen apps.
//!
//! Uses the same flags as tauri-macos-spotlight-example:
//! MoveToActiveSpace + FullScreenAuxiliary + NonActivatingPanel.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

use tauri::{AppHandle, Manager, WebviewWindow};

static ISLAND_PANEL_READY: AtomicBool = AtomicBool::new(false);
/// A converted macOS NSPanel stays ordered in to keep the WebView surface warm
/// between dictations. In that mode native hide/show reconciliation is not
/// needed; the Island document hides its own content while idle.
static ISLAND_PANEL_WARM: AtomicBool = AtomicBool::new(false);
static ISLAND_VISIBLE: AtomicBool = AtomicBool::new(false);
static ISLAND_RECONCILE_SCHEDULED: AtomicBool = AtomicBool::new(false);
static ISLAND_DESIRED_VISIBLE: AtomicBool = AtomicBool::new(false);
static ISLAND_HAS_WIDE_CAPTION: AtomicBool = AtomicBool::new(false);
static ISLAND_WIDE_APPLIED: AtomicBool = AtomicBool::new(false);
static ISLAND_HAS_WARNING_CAPTION: AtomicBool = AtomicBool::new(false);
static ISLAND_WARNING_APPLIED: AtomicBool = AtomicBool::new(false);
static ISLAND_LEARN_APPLIED: AtomicBool = AtomicBool::new(false);
static ISLAND_DETAILS_APPLIED: AtomicBool = AtomicBool::new(false);
// 220px monochrome toast with 12px side gutters and a 4px stage gap.
const LEARN_TOAST_WINDOW_WIDTH: f64 = 244.0;
const LEARN_TOAST_BAND_HEIGHT: f64 = 52.0;
const RECOVERY_CAPSULE_WINDOW_WIDTH: f64 = 320.0;
const RECOVERY_CAPSULE_HEIGHT: f64 = 116.0;
/// While VoiceFlow is delivering text, the HUD must stay click-through and
/// must not order itself in front of the target app's key window.
static ISLAND_PASTE_YIELD: AtomicBool = AtomicBool::new(false);
/// The learn toast and terminal recovery actions are the only HUD surfaces that
/// need mouse hits. All active/idle pill states remain click-through.
static ISLAND_LEARN_TOAST: AtomicBool = AtomicBool::new(false);
static ISLAND_RECOVERY_ACTIONS: AtomicBool = AtomicBool::new(false);
/// Active controls in the primary capsule are mouse-enabled only while their
/// state event is visible. This flag is published atomically because state
/// publication can happen while the dictation manager is locked.
static ISLAND_HUD_ACTIONS: AtomicBool = AtomicBool::new(false);
/// An explicitly opened details tray needs mouse input and the larger native
/// surface. It is independent of the primary stop/cancel action.
static ISLAND_DETAILS_OWNER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

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
    if ISLAND_PANEL_WARM.load(Ordering::Acquire) {
        if was_desired || crate::native_glass::material_is_visible() {
            schedule_reconcile(app);
        }
        return;
    }
    if !was_desired && !ISLAND_VISIBLE.load(Ordering::Acquire) {
        return;
    }
    schedule_reconcile(app);
}

/// The capsule material follows the primary pill and recovery controls. The
/// independent learning toast can keep the panel visible without drawing a
/// second, empty glass capsule behind it.
pub fn hud_should_show_native_material() -> bool {
    !ISLAND_PASTE_YIELD.load(Ordering::Acquire)
        && (ISLAND_HUD_ACTIONS.load(Ordering::Acquire)
            || are_details_visible()
            || ISLAND_RECOVERY_ACTIONS.load(Ordering::Acquire))
}

/// The HUD is click-through by default. Active stop/cancel, explicit details,
/// learning, and recovery controls opt into mouse events. Paste yield always
/// wins so a visible button can never take the target app's input.
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

pub fn is_recovery_actions_visible() -> bool {
    ISLAND_RECOVERY_ACTIONS.load(Ordering::Acquire)
}

pub fn are_hud_actions_visible() -> bool {
    ISLAND_HUD_ACTIONS.load(Ordering::Acquire)
}

fn native_status_revision(identifier: &str) -> Option<(&str, u64, bool)> {
    let fields = identifier.strip_prefix("com.voiceflow.dictation.settlement.v1|")?;
    let mut process = None;
    let mut revision = None;
    let mut revision_available = None;
    for field in fields.split('|') {
        if let Some(value) = field.strip_prefix("process=") {
            process = Some(value);
        } else if let Some(value) = field.strip_prefix("revision=") {
            revision = value.parse::<u64>().ok();
        } else if let Some(value) = field.strip_prefix("revision_available=") {
            revision_available = match value {
                "0" => Some(false),
                "1" => Some(true),
                _ => None,
            };
        }
    }
    Some((process?, revision?, revision_available?))
}

/// Keep the retained native identifier monotonic within one process. Equal
/// revisions are allowed because a resolved UI-language change only relabels
/// the same backend snapshot; an exhausted proof cannot be revived.
fn should_apply_native_delivery_status(current: Option<&str>, incoming: &str) -> bool {
    let Some((incoming_process, incoming_revision, incoming_available)) =
        native_status_revision(incoming)
    else {
        return false;
    };
    let Some(current) = current.and_then(native_status_revision) else {
        return true;
    };
    let (current_process, current_revision, current_available) = current;
    if current_process != incoming_process {
        return true;
    }
    if !current_available {
        return !incoming_available && incoming_revision == current_revision;
    }
    incoming_revision >= current_revision && (incoming_available || incoming_revision == u64::MAX)
}

pub fn are_details_visible() -> bool {
    ISLAND_DETAILS_OWNER.load(Ordering::Acquire) != 0
}

pub fn details_owner_generation() -> Option<u64> {
    let owner = ISLAND_DETAILS_OWNER.load(Ordering::Acquire);
    (owner != 0).then_some(owner.wrapping_sub(1))
}

pub fn set_hud_actions_visible(app: &AppHandle, visible: bool) {
    let changed = ISLAND_HUD_ACTIONS.swap(visible, Ordering::AcqRel) != visible;
    if visible {
        ISLAND_DESIRED_VISIBLE.store(true, Ordering::Release);
    }
    sync_mouse_from_published_state(app);
    if visible || changed {
        schedule_reconcile(app);
    }
}

pub fn set_details_visible(app: &AppHandle, generation: u64, visible: bool) -> bool {
    let owner = generation.wrapping_add(1).max(1);
    if visible {
        ISLAND_DETAILS_OWNER.store(owner, Ordering::Release);
    } else if ISLAND_DETAILS_OWNER
        .compare_exchange(owner, 0, Ordering::AcqRel, Ordering::Acquire)
        .is_err()
    {
        return false;
    }
    if visible {
        ISLAND_DESIRED_VISIBLE.store(true, Ordering::Release);
    }
    sync_mouse_from_published_state(app);
    apply_island_size(app);
    schedule_reconcile(app);
    true
}

pub fn clear_details_visible(app: &AppHandle) {
    let changed = ISLAND_DETAILS_OWNER.swap(0, Ordering::AcqRel) != 0;
    sync_mouse_from_published_state(app);
    apply_island_size(app);
    if changed {
        schedule_reconcile(app);
    }
}

fn sync_mouse_from_published_state(app: &AppHandle) {
    set_interactive(
        app,
        action_surface_needs_mouse(
            ISLAND_HUD_ACTIONS.load(Ordering::Acquire),
            are_details_visible(),
            ISLAND_LEARN_TOAST.load(Ordering::Acquire),
            ISLAND_RECOVERY_ACTIONS.load(Ordering::Acquire),
        ),
    );
}

fn action_surface_needs_mouse(
    hud_actions: bool,
    details_visible: bool,
    learn_toast: bool,
    recovery_actions: bool,
) -> bool {
    hud_actions || details_visible || learn_toast || recovery_actions
}

fn should_show_during_reconcile(
    desired_visible: bool,
    hud_actions: bool,
    details_visible: bool,
    learn_toast: bool,
    recovery_actions: bool,
) -> bool {
    desired_visible
        || action_surface_needs_mouse(hud_actions, details_visible, learn_toast, recovery_actions)
}

fn visibility_reconcile_is_pending(
    desired_visible: bool,
    hud_actions: bool,
    details_visible: bool,
    learn_toast: bool,
    recovery_actions: bool,
    currently_visible: bool,
) -> bool {
    should_show_during_reconcile(
        desired_visible,
        hud_actions,
        details_visible,
        learn_toast,
        recovery_actions,
    ) != currently_visible
}

pub fn set_learn_toast_interactive(app: &AppHandle, interactive: bool) {
    let changed = ISLAND_LEARN_TOAST.swap(interactive, Ordering::AcqRel) != interactive;
    if interactive {
        ISLAND_DESIRED_VISIBLE.store(true, Ordering::Release);
    }
    set_interactive(
        app,
        action_surface_needs_mouse(
            ISLAND_HUD_ACTIONS.load(Ordering::Acquire),
            are_details_visible(),
            interactive,
            ISLAND_RECOVERY_ACTIONS.load(Ordering::Acquire),
        ),
    );
    apply_island_size(app);
    if interactive || changed {
        schedule_reconcile(app);
    }
}

pub fn set_recovery_actions_visible(app: &AppHandle, visible: bool) {
    let changed = ISLAND_RECOVERY_ACTIONS.swap(visible, Ordering::AcqRel) != visible;
    if visible {
        ISLAND_DESIRED_VISIBLE.store(true, Ordering::Release);
    }
    set_interactive(
        app,
        action_surface_needs_mouse(
            ISLAND_HUD_ACTIONS.load(Ordering::Acquire),
            are_details_visible(),
            ISLAND_LEARN_TOAST.load(Ordering::Acquire),
            visible,
        ),
    );
    apply_island_size(app);
    if visible || changed {
        schedule_reconcile(app);
    }
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

/// Expand the transparent island surface while long status captions are
/// visible, then shrink it again so idle does not keep a wide hit target in
/// WindowServer.
pub fn set_has_wide_caption(app: &AppHandle, wide: bool) {
    ISLAND_HAS_WIDE_CAPTION.store(wide, Ordering::Release);
    apply_island_size(app);
}

pub fn set_has_warning_caption(app: &AppHandle, warning: bool) {
    ISLAND_HAS_WARNING_CAPTION.store(warning, Ordering::Release);
    apply_island_size(app);
}

fn island_should_expand() -> bool {
    ISLAND_HAS_WIDE_CAPTION.load(Ordering::Acquire)
}

fn island_window_size(
    wide: bool,
    warning: bool,
    learn_toast: bool,
    details_visible: bool,
) -> (f64, f64) {
    let secondary_visible = details_visible;
    let base_width = crate::notch::pill_window_width(wide);
    let width = if secondary_visible {
        base_width.max(RECOVERY_CAPSULE_WINDOW_WIDTH)
    } else {
        base_width
    };
    let height = if secondary_visible {
        RECOVERY_CAPSULE_HEIGHT
    } else {
        crate::notch::pill_window_height(warning)
    };
    // Recovery lives inside the VoicePill capsule. Only the independent
    // learning toast adds a second surface below it.
    let action_rows = usize::from(learn_toast);
    if action_rows > 0 {
        (
            width.max(LEARN_TOAST_WINDOW_WIDTH),
            height + LEARN_TOAST_BAND_HEIGHT * action_rows as f64,
        )
    } else {
        (width, height)
    }
}

fn apply_island_size(app: &AppHandle) {
    let next_wide = island_should_expand();
    let next_tall = ISLAND_HAS_WARNING_CAPTION.load(Ordering::Acquire);
    let next_learn = ISLAND_LEARN_TOAST.load(Ordering::Acquire);
    let next_details = are_details_visible();
    let previous_wide = ISLAND_WIDE_APPLIED.swap(next_wide, Ordering::AcqRel);
    let previous_tall = ISLAND_WARNING_APPLIED.swap(next_tall, Ordering::AcqRel);
    let previous_learn = ISLAND_LEARN_APPLIED.swap(next_learn, Ordering::AcqRel);
    let previous_details = ISLAND_DETAILS_APPLIED.swap(next_details, Ordering::AcqRel);
    if previous_wide == next_wide
        && previous_tall == next_tall
        && previous_learn == next_learn
        && previous_details == next_details
    {
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
        let panel_visibility_pending = !ISLAND_PANEL_WARM.load(Ordering::Acquire)
            && visibility_reconcile_is_pending(
                ISLAND_DESIRED_VISIBLE.load(Ordering::Acquire),
                ISLAND_HUD_ACTIONS.load(Ordering::Acquire),
                are_details_visible(),
                ISLAND_LEARN_TOAST.load(Ordering::Acquire),
                ISLAND_RECOVERY_ACTIONS.load(Ordering::Acquire),
                ISLAND_VISIBLE.load(Ordering::Acquire),
            );
        let material_visibility_pending = crate::native_glass::material_is_visible()
            != crate::native_glass::should_be_material_visible(hud_should_show_native_material());
        if panel_visibility_pending || material_visibility_pending {
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

    let should_show = should_show_during_reconcile(
        ISLAND_DESIRED_VISIBLE.load(Ordering::Acquire),
        ISLAND_HUD_ACTIONS.load(Ordering::Acquire),
        are_details_visible(),
        ISLAND_LEARN_TOAST.load(Ordering::Acquire),
        ISLAND_RECOVERY_ACTIONS.load(Ordering::Acquire),
    );
    if should_show {
        show_overlay_on_main(&window, app);
    } else {
        hide_overlay_on_main(&window, app);
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
            crate::native_glass::set_hud_visible_on_main(false);
            crate::native_glass::publish_material_visibility(app);
            log::warn!("failed to show island overlay: {error}");
            return;
        }
    }
    crate::native_glass::set_hud_visible_on_main(hud_should_show_native_material());
    crate::native_glass::publish_material_visibility(app);
    // `window.show()` can reorder against the focused settings window. Re-apply
    // the overlay level last so the HUD stays above every other app window.
    raise_overlay_above_all_windows(window, app);
}

fn position_overlay_on_main(window: &tauri::WebviewWindow, app: &AppHandle) {
    let (width, height) = island_window_size(
        island_should_expand(),
        ISLAND_HAS_WARNING_CAPTION.load(Ordering::Acquire),
        ISLAND_LEARN_TOAST.load(Ordering::Acquire),
        are_details_visible(),
    );
    let placement = crate::notch::placement_for_cursor_screen_with_size(app, width, height);
    let _ = window.set_position(tauri::Position::Physical(tauri::PhysicalPosition {
        x: placement.x.round() as i32,
        y: placement.y.round() as i32,
    }));
    let _ = window.set_size(tauri::Size::Physical(tauri::PhysicalSize {
        width: placement.width.round().max(1.0) as u32,
        height: placement.height.round().max(1.0) as u32,
    }));
    #[cfg(target_os = "macos")]
    refresh_native_delivery_status_frame(window);
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
fn hide_overlay_on_main(window: &tauri::WebviewWindow, app: &AppHandle) {
    crate::native_glass::set_hud_visible_on_main(false);
    crate::native_glass::publish_material_visibility(app);
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
fn hide_overlay_on_main(window: &tauri::WebviewWindow, app: &AppHandle) {
    crate::native_glass::set_hud_visible_on_main(false);
    crate::native_glass::publish_material_visibility(app);
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

/// Publish the exact completion snapshot through a native NSAccessibilityElement
/// attached to the warm island NSWindow. The identifier contains only bounded
/// opaque transaction metadata; the value is human-readable and content-free.
pub(crate) fn publish_accessible_delivery_status(
    app: &AppHandle,
    registry: Arc<crate::delivery_settlement::DeliverySettlementRegistry>,
    ui_language_uses_chinese: Arc<AtomicBool>,
) {
    #[cfg(target_os = "macos")]
    {
        let Some(window) = app.get_webview_window("island") else {
            return;
        };
        let main_thread_window = window.clone();
        if let Err(error) = window.run_on_main_thread(move || {
            // Resolve actual owner state and the latest effective UI locale at
            // application time, not when this native-main-thread task is queued.
            let ui_language = if ui_language_uses_chinese.load(Ordering::Acquire) {
                "zh"
            } else {
                "en"
            };
            let status = crate::delivery_settlement::current_native_accessible_delivery_status(
                &registry,
                ui_language,
            );
            update_native_delivery_status(&main_thread_window, &status);
        }) {
            log::debug!("failed to queue native dictation status update: {error}");
        }
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = (app, registry, ui_language_uses_chinese);
    }
}

#[cfg(target_os = "macos")]
fn update_native_delivery_status(
    window: &tauri::WebviewWindow,
    status: &crate::delivery_settlement::NativeAccessibleDeliveryStatus,
) {
    use core_graphics::geometry::{CGRect, CGSize};
    use objc::{class, msg_send, runtime::Object, sel, sel_impl};
    use std::ffi::{CStr, CString};
    use std::os::raw::c_char;

    fn ns_string(value: &str) -> Option<*mut Object> {
        let value = CString::new(value).ok()?;
        let native: *mut Object =
            unsafe { msg_send![class!(NSString), stringWithUTF8String: value.as_ptr()] };
        (!native.is_null()).then_some(native)
    }

    fn string_value(native: *mut Object) -> Option<String> {
        if native.is_null() {
            return None;
        }
        let value: *const c_char = unsafe { msg_send![native, UTF8String] };
        if value.is_null() {
            return None;
        }
        unsafe { CStr::from_ptr(value) }
            .to_str()
            .ok()
            .map(str::to_owned)
    }

    let Ok(pointer) = window.ns_window() else {
        return;
    };
    let native_window = pointer as *mut Object;
    let stable_prefix = "com.voiceflow.dictation.settlement.v1|";
    let Some(native_identifier) = ns_string(&status.identifier) else {
        return;
    };
    let Some(native_label) = ns_string(&status.label) else {
        return;
    };
    let Some(native_value) = ns_string(&status.value) else {
        return;
    };

    unsafe {
        // Preserve the existing NSWindow/WKWebView accessibility children and
        // append one retained native status element. Never make the window or
        // its current WebView subtree aria-hidden.
        let children: *mut Object = msg_send![native_window, accessibilityChildren];
        let child_count: usize = if children.is_null() {
            0
        } else {
            msg_send![children, count]
        };
        let mut status_element: *mut Object = std::ptr::null_mut();
        let mut previous_identifier = None;
        let mutable_children: *mut Object =
            msg_send![class!(NSMutableArray), arrayWithCapacity: child_count + 1];
        for index in 0..child_count {
            let child: *mut Object = msg_send![children, objectAtIndex: index];
            let supports_identifier: bool =
                msg_send![child, respondsToSelector: sel!(accessibilityIdentifier)];
            if !supports_identifier {
                let _: () = msg_send![mutable_children, addObject: child];
                continue;
            }
            let child_identifier: *mut Object = msg_send![child, accessibilityIdentifier];
            if string_value(child_identifier).is_some_and(|value| value.starts_with(stable_prefix))
            {
                previous_identifier = string_value(child_identifier);
                status_element = child;
            }
            let _: () = msg_send![mutable_children, addObject: child];
        }

        if status_element.is_null() {
            let role = ns_string("AXStaticText").unwrap_or(std::ptr::null_mut());
            let frame: CGRect = msg_send![native_window, frame];
            let status_size = CGSize::new(1.0, 1.0);
            let status_frame = CGRect::new(&frame.origin, &status_size);
            status_element = msg_send![
                class!(NSAccessibilityElement),
                accessibilityElementWithRole: role
                frame: status_frame
                label: native_label
                parent: native_window
            ];
            if status_element.is_null() {
                return;
            }
            let _: () = msg_send![mutable_children, addObject: status_element];
            let _: () = msg_send![native_window, setAccessibilityChildren: mutable_children];
        }

        if !should_apply_native_delivery_status(previous_identifier.as_deref(), &status.identifier)
        {
            return;
        }

        // Bind the retained status element to its containing and top-level window.
        let _: () = msg_send![status_element, setAccessibilityWindow: native_window];
        let _: () = msg_send![status_element, setAccessibilityTopLevelUIElement: native_window];

        // NSAccessibilityElement is retained by the window's children array.
        // Its changing identifier is intentionally machine-facing; VoiceOver
        // hears only the plain label and value below.
        let _: () = msg_send![status_element, setAccessibilityIdentifier: native_identifier];
        let _: () = msg_send![status_element, setAccessibilityLabel: native_label];
        let _: () = msg_send![status_element, setAccessibilityValue: native_value];
    }
}

/// Keep the off-visual status element anchored to the warm panel after the
/// island moves to another display or changes size. This runs only with the
/// existing main-thread layout reconciliation, not on a timer or render path.
#[cfg(target_os = "macos")]
fn refresh_native_delivery_status_frame(window: &tauri::WebviewWindow) {
    use core_graphics::geometry::{CGRect, CGSize};
    use objc::{msg_send, runtime::Object, sel, sel_impl};
    use std::ffi::CStr;
    use std::os::raw::c_char;

    let Ok(pointer) = window.ns_window() else {
        return;
    };
    let native_window = pointer as *mut Object;
    unsafe {
        let children: *mut Object = msg_send![native_window, accessibilityChildren];
        if children.is_null() {
            return;
        }
        let child_count: usize = msg_send![children, count];
        for index in 0..child_count {
            let child: *mut Object = msg_send![children, objectAtIndex: index];
            let supports_identifier: bool =
                msg_send![child, respondsToSelector: sel!(accessibilityIdentifier)];
            if !supports_identifier {
                continue;
            }
            let identifier: *mut Object = msg_send![child, accessibilityIdentifier];
            if identifier.is_null() {
                continue;
            }
            let bytes: *const c_char = msg_send![identifier, UTF8String];
            if bytes.is_null() {
                continue;
            }
            let is_status = CStr::from_ptr(bytes)
                .to_str()
                .is_ok_and(|value| value.starts_with("com.voiceflow.dictation.settlement.v1|"));
            if !is_status {
                continue;
            }
            let frame: CGRect = msg_send![native_window, frame];
            let status_size = CGSize::new(1.0, 1.0);
            let status_frame = CGRect::new(&frame.origin, &status_size);
            let _: () = msg_send![child, setAccessibilityFrame: status_frame];
            break;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{end_paste_yield, should_order_front_overlay, ISLAND_PASTE_YIELD};
    use std::sync::atomic::Ordering;

    fn should_show(desired: bool, learn: bool, recovery: bool) -> bool {
        super::should_show_during_reconcile(desired, false, false, learn, recovery)
    }

    fn is_pending(desired: bool, learn: bool, recovery: bool, visible: bool) -> bool {
        super::visibility_reconcile_is_pending(desired, false, false, learn, recovery, visible)
    }

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
    fn only_visible_action_rows_enable_hud_mouse_events() {
        assert!(!super::action_surface_needs_mouse(
            false, false, false, false
        ));
        assert!(super::action_surface_needs_mouse(true, false, false, false));
        assert!(super::action_surface_needs_mouse(false, true, false, false));
        assert!(super::action_surface_needs_mouse(false, false, true, false));
        assert!(super::action_surface_needs_mouse(false, false, false, true));
        assert!(super::action_surface_needs_mouse(true, true, true, true));
    }

    #[test]
    fn queued_hide_preserves_new_action_owner_and_explicit_release_can_hide() {
        // A completion may queue a hide before a learning or recovery action
        // claims the native surface. Reconciliation must honor the latest
        // owner even though the queued desired state was already false.
        assert!(should_show(false, true, false));
        assert!(should_show(false, false, true));

        // Once the action owner is released, a later explicit hide settles to
        // hidden without requiring another reconciliation pass.
        assert!(!should_show(false, false, false));
        assert!(should_show(true, false, false));
    }

    #[test]
    fn queued_show_does_not_overwrite_a_newer_hide_request() {
        // Model the production queue boundary: a show snapshot is captured,
        // then hide_overlay updates the latest request before the main-thread
        // reconciliation applies that older snapshot.
        let queued_snapshot = should_show(true, false, false);
        let desired_visible = false;
        let mut actually_visible = queued_snapshot;

        assert!(actually_visible);
        assert!(!desired_visible);
        assert!(is_pending(desired_visible, false, false, actually_visible));

        // The tail schedules one pass using the latest effective request. Once
        // that pass hides the panel, the tail is settled and does not loop.
        actually_visible = should_show(desired_visible, false, false);
        assert!(!actually_visible);
        assert!(!is_pending(desired_visible, false, false, actually_visible));
    }

    #[test]
    fn queued_reconcile_uses_action_owners_without_losing_later_hide() {
        let desired_visible = false;
        let learn_toast = true;
        let recovery_actions = false;

        let applied = should_show(desired_visible, learn_toast, recovery_actions);
        assert!(applied);
        assert!(!is_pending(
            desired_visible,
            learn_toast,
            recovery_actions,
            applied
        ));

        // Releasing the owner and making an explicit hide request must settle
        // through one hide pass, without the old reconcile restoring intent.
        let desired_visible = false;
        let learn_toast = false;
        let recovery_actions = false;
        assert!(is_pending(
            desired_visible,
            learn_toast,
            recovery_actions,
            applied
        ));
        let hidden = should_show(desired_visible, learn_toast, recovery_actions);
        assert!(!hidden);
        assert!(!is_pending(
            desired_visible,
            learn_toast,
            recovery_actions,
            hidden
        ));
    }

    #[test]
    fn learn_toast_and_explicit_details_keep_the_primary_capsule_bottom_anchored() {
        assert_eq!(
            super::island_window_size(false, false, false, false),
            (244.0, 64.0)
        );
        assert_eq!(
            super::island_window_size(false, false, true, false),
            (244.0, 116.0)
        );
        assert_eq!(
            super::island_window_size(true, true, true, false),
            (244.0, 116.0)
        );
        assert_eq!(
            super::island_window_size(true, true, false, true),
            (320.0, 116.0)
        );
        assert_eq!(
            super::island_window_size(false, false, false, true),
            (320.0, 116.0)
        );
        assert_eq!(
            super::island_window_size(true, true, true, true),
            (320.0, 168.0)
        );
        // Dismissal removes only the toast's band while a warning is still visible.
        assert_eq!(
            super::island_window_size(true, true, false, false),
            (244.0, 64.0)
        );
        assert_eq!(
            super::island_window_size(false, false, false, false),
            (244.0, 64.0)
        );
    }

    #[test]
    fn native_status_revision_guard_rejects_an_older_transaction_snapshot() {
        use crate::delivery_settlement::{
            DeliverySettlementOwner, DeliverySettlementRegistry, DeliveryTerminalOutcome,
            DictationTransactionId,
        };

        let registry = DeliverySettlementRegistry::default();
        let transaction_a = DeliverySettlementOwner::new(DictationTransactionId::fresh());
        assert!(registry.begin(transaction_a.clone()));
        let writer_a = transaction_a
            .admit_writer()
            .expect("transaction A writer is admitted");
        transaction_a.seal(DeliveryTerminalOutcome::completed("done", "paste", None));
        drop(writer_a);
        assert!(registry.record_terminal(transaction_a));
        let transaction_a_id = registry.snapshot().native_ax_identifier();

        let transaction_b = DeliverySettlementOwner::new(DictationTransactionId::fresh());
        assert!(registry.begin(transaction_b));
        let transaction_b_id = registry.snapshot().native_ax_identifier();

        assert!(super::should_apply_native_delivery_status(
            None,
            &transaction_a_id
        ));
        assert!(super::should_apply_native_delivery_status(
            Some(&transaction_a_id),
            &transaction_b_id
        ));
        assert!(!super::should_apply_native_delivery_status(
            Some(&transaction_b_id),
            &transaction_a_id
        ));
        // A language-only update has the same backend revision and remains valid.
        assert!(super::should_apply_native_delivery_status(
            Some(&transaction_b_id),
            &transaction_b_id
        ));
    }
}
