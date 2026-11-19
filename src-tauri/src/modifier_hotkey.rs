//! Fn/Globe and held-combination release observation. The listener reads only
//! flags and physical key state, never characters or typed text.
use crate::dictation::HotkeySource;
use crate::hotkey::ModeGesture;
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use tauri::AppHandle;

#[derive(Clone)]
struct FnConfig {
    source: HotkeySource,
    mode: String,
}

static CONFIG: OnceLock<Arc<Mutex<Option<FnConfig>>>> = OnceLock::new();
static GESTURE: OnceLock<Arc<Mutex<ModeGesture>>> = OnceLock::new();
static LISTENER: OnceLock<()> = OnceLock::new();
static TAP_PORT: AtomicUsize = AtomicUsize::new(0);
static SLEEP_OBSERVER: OnceLock<()> = OnceLock::new();
static PASTE_SUPPRESS: AtomicU32 = AtomicU32::new(0);

pub struct PasteSuppressGuard {
    _private: (),
}
impl PasteSuppressGuard {
    pub fn new() -> Self {
        set_paste_suppressed(true);
        Self { _private: () }
    }
}
impl Drop for PasteSuppressGuard {
    fn drop(&mut self) {
        set_paste_suppressed(false);
    }
}

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
fn config_store() -> Arc<Mutex<Option<FnConfig>>> {
    CONFIG.get_or_init(|| Arc::new(Mutex::new(None))).clone()
}
fn gesture_store() -> Arc<Mutex<ModeGesture>> {
    GESTURE
        .get_or_init(|| Arc::new(Mutex::new(ModeGesture::default())))
        .clone()
}

pub fn set_paste_suppressed(suppressed: bool) {
    if suppressed {
        if PASTE_SUPPRESS.fetch_add(1, Ordering::SeqCst) == 0 {
            reset_state();
            crate::hotkey::reset_pressed_state();
        }
    } else {
        release_paste_suppression(&PASTE_SUPPRESS);
    }
}

fn release_paste_suppression(counter: &AtomicU32) {
    let mut depth = counter.load(Ordering::SeqCst);
    while depth > 0 {
        match counter.compare_exchange_weak(depth, depth - 1, Ordering::SeqCst, Ordering::SeqCst) {
            Ok(_) => return,
            Err(current) => depth = current,
        }
    }
}

pub fn is_paste_suppressed() -> bool {
    PASTE_SUPPRESS.load(Ordering::SeqCst) > 0
}
pub fn set_dictation_active(active: bool) {
    if !active && !crate::hotkey::hybrid_restart_handoff_pending() {
        reset_state();
        crate::hotkey::reset_pressed_state();
    }
}

/// Used to identify deprecated saved bindings as well as the supported Fn key.
pub fn is_modifier_only(hotkey: &str) -> bool {
    matches!(
        hotkey.trim(),
        "Alt"
            | "Option"
            | "Control"
            | "Ctrl"
            | "Shift"
            | "CmdOrControl"
            | "CommandOrControl"
            | "CmdOrCtrl"
            | "Super"
            | "Meta"
            | "Command"
            | "Fn"
            | "Function"
            | "Globe"
    )
}
pub fn is_fn_only(hotkey: &str) -> bool {
    matches!(hotkey.trim(), "Fn" | "Function" | "Globe")
}

#[cfg(target_os = "macos")]
#[link(name = "CoreGraphics", kind = "framework")]
unsafe extern "C" {
    fn CGPreflightListenEventAccess() -> bool;
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
    fn CGEventSourceFlagsState(state_id: i32) -> u64;
    fn CGEventTapEnable(tap: *const std::ffi::c_void, enable: bool);
}

#[cfg(target_os = "macos")]
pub fn input_monitoring_is_allowed() -> bool {
    unsafe { CGPreflightListenEventAccess() }
}

#[cfg(not(target_os = "macos"))]
pub fn input_monitoring_is_allowed() -> bool {
    false
}

#[cfg(target_os = "macos")]
fn other_keys_down() -> bool {
    (0..128)
        .filter(|code| *code != 63)
        .any(|code| unsafe { CGEventSourceKeyState(0, code) })
}

pub async fn wait_for_key_release() {
    #[cfg(target_os = "macos")]
    {
        let start = std::time::Instant::now();
        let timeout = std::time::Duration::from_millis(1500);
        while unsafe { CGEventSourceFlagsState(0) }
            & ((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 23))
            != 0
            || (0..128).any(|key| unsafe { CGEventSourceKeyState(0, key) })
        {
            if start.elapsed() >= timeout {
                log::warn!(
                    "modifier_hotkey::wait_for_key_release timed out after 1500ms; proceeding"
                );
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(15)).await;
        }
    }
}

#[cfg(target_os = "macos")]
pub fn ensure_listener(_app: AppHandle) -> Result<(), String> {
    if !input_monitoring_is_allowed() {
        return Err("input_monitoring_required".into());
    }
    if LISTENER.get().is_some() {
        return Ok(());
    }
    use core_foundation::{
        base::TCFType,
        runloop::{kCFRunLoopCommonModes, CFRunLoop},
    };
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
        EventField,
    };
    let tap = CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::FlagsChanged, CGEventType::KeyDown],
        move |_, event_type, event| {
            if matches!(
                event_type,
                CGEventType::TapDisabledByTimeout | CGEventType::TapDisabledByUserInput
            ) {
                reset_state();
                crate::hotkey::reset_pressed_state();
                crate::dictation::interrupt_hotkeys();
                let port = TAP_PORT.load(Ordering::Acquire);
                if port != 0 {
                    unsafe {
                        CGEventTapEnable(port as *const std::ffi::c_void, true);
                    }
                }
                return None;
            }
            if crate::hotkey::is_suspended() || is_paste_suppressed() {
                return None;
            }
            let flags = event.get_flags().bits();
            if matches!(event_type, CGEventType::FlagsChanged) {
                crate::hotkey::modifiers_changed(flags);
            }
            let config = lock_recover(&config_store()).clone()?;
            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            let fn_down = flags & (1 << 23) != 0;
            let store = gesture_store();
            let now = std::time::Instant::now();
            let actions = {
                let mut state = lock_recover(&store);
                if matches!(event_type, CGEventType::FlagsChanged) && code == 63 {
                    if fn_down {
                        let has_modifiers =
                            flags & ((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20)) != 0;
                        state.pressed(&config.mode, has_modifiers || other_keys_down(), false, now)
                    } else {
                        state.released(&config.mode, true, now)
                    }
                } else if code != 63
                    && fn_down
                    && (matches!(event_type, CGEventType::KeyDown)
                        || matches!(event_type, CGEventType::FlagsChanged))
                {
                    state.chorded(&config.mode)
                } else {
                    Vec::new()
                }
            };
            for timeout in crate::hotkey::dispatch_gesture_triggers(config.source, actions) {
                crate::hotkey::schedule_mode_hybrid_timeout(store.clone(), config.source, timeout);
            }
            None
        },
    )
    .map_err(|_| "modifier_event_tap_unavailable".to_owned())?;
    unsafe {
        let source = tap
            .mach_port
            .create_runloop_source(0)
            .map_err(|_| "modifier_event_tap_unavailable".to_owned())?;
        CFRunLoop::get_main().add_source(&source, kCFRunLoopCommonModes);
        TAP_PORT.store(
            tap.mach_port.as_concrete_TypeRef() as usize,
            Ordering::Release,
        );
        tap.enable();
        // The main runloop owns the tap for the process lifetime; keep its
        // callback alive until process exit to avoid dereferencing freed state.
        std::mem::forget(tap);
    }
    let _ = LISTENER.set(());
    Ok(())
}
#[cfg(not(target_os = "macos"))]
pub fn ensure_listener(_app: AppHandle) -> Result<(), String> {
    Err("Fn and held-combination observation require macOS".into())
}

pub fn register(app: &AppHandle, source: HotkeySource, mode: &str) -> Result<(), String> {
    ensure_listener(app.clone())?;
    reset_state();
    *lock_recover(&config_store()) = Some(FnConfig {
        source,
        mode: mode.into(),
    });
    Ok(())
}
pub fn arm_after_release() {
    *lock_recover(&gesture_store()) = ModeGesture::default();
}
pub fn reset_state() {
    lock_recover(&gesture_store()).reset();
}
pub fn unregister_source(source: HotkeySource) {
    let store = config_store();
    let mut config = lock_recover(&store);
    if config
        .as_ref()
        .is_some_and(|config| config.source == source)
    {
        *config = None;
        reset_state();
    }
}
pub fn unregister() {
    reset_state();
    *lock_recover(&config_store()) = None;
}

#[cfg(target_os = "macos")]
pub fn install_sleep_observer() {
    if SLEEP_OBSERVER.set(()).is_err() {
        return;
    }
    use core_foundation::{base::TCFType, string::CFString};
    use objc::{class, msg_send, sel, sel_impl};
    let callback = block2::RcBlock::new(move |_notification: *mut objc2::runtime::AnyObject| {
        reset_state();
        crate::hotkey::reset_pressed_state();
        crate::dictation::interrupt_hotkeys();
    });
    unsafe {
        let workspace: *mut objc::runtime::Object = msg_send![class!(NSWorkspace), sharedWorkspace];
        let center: *mut objc::runtime::Object = msg_send![workspace, notificationCenter];
        for name in [
            "NSWorkspaceWillSleepNotification",
            "NSWorkspaceSessionDidResignActiveNotification",
        ] {
            let name = CFString::new(name);
            let _: *mut objc::runtime::Object = msg_send![center, addObserverForName:name.as_concrete_TypeRef() object:std::ptr::null::<objc::runtime::Object>() queue:std::ptr::null::<objc::runtime::Object>() usingBlock:&*callback];
        }
    }
    std::mem::forget(callback);
}
#[cfg(not(target_os = "macos"))]
pub fn install_sleep_observer() {}

#[cfg(test)]
mod tests {
    use super::*;

    struct HandoffFlagGuard;

    impl Drop for HandoffFlagGuard {
        fn drop(&mut self) {
            crate::hotkey::set_hybrid_restart_handoff_pending(false);
        }
    }

    #[test]
    fn distinguishes_fn_from_deprecated_modifiers() {
        for key in ["Alt", "Control", "Shift", "CmdOrControl", "Fn", "Globe"] {
            assert!(is_modifier_only(key));
        }
        assert!(is_fn_only("Globe"));
        assert!(!is_fn_only("Alt"));
        assert!(!is_modifier_only("CmdOrControl+Alt+Space"));
    }

    #[test]
    fn idle_phase_preserves_a_held_fn_gesture_during_hybrid_restart_handoff() {
        let _handoff_test_lock = crate::hotkey::HYBRID_HANDOFF_TEST_LOCK
            .lock()
            .unwrap_or_else(|e| e.into_inner());
        let _clear_handoff_flag = HandoffFlagGuard;
        let now = std::time::Instant::now();
        *lock_recover(&gesture_store()) = ModeGesture::default();
        let pressed = lock_recover(&gesture_store()).pressed(
            crate::hotkey::HYBRID_HOLD_LOCK,
            false,
            true,
            now,
        );
        let gesture_id = match pressed.first() {
            Some(crate::hotkey::GestureAction::Trigger(
                crate::dictation::ModeTrigger::Press,
                gesture_id,
            )) => *gesture_id,
            action => panic!("expected a Fn hold press action, got {action:?}"),
        };

        crate::hotkey::set_hybrid_restart_handoff_pending(true);
        set_dictation_active(false);
        let released = lock_recover(&gesture_store()).released(
            crate::hotkey::HYBRID_HOLD_LOCK,
            false,
            now + std::time::Duration::from_millis(220),
        );
        crate::hotkey::set_hybrid_restart_handoff_pending(false);
        reset_state();

        assert_eq!(
            released,
            vec![crate::hotkey::GestureAction::Trigger(
                crate::dictation::ModeTrigger::Release,
                gesture_id,
            )]
        );
    }
    #[test]
    fn nested_paste_suppression_keeps_the_outer_window() {
        let baseline = PASTE_SUPPRESS.load(Ordering::SeqCst);
        let outer = PasteSuppressGuard::new();
        let inner = PasteSuppressGuard::new();
        assert_eq!(PASTE_SUPPRESS.load(Ordering::SeqCst), baseline + 2);
        drop(inner);
        assert_eq!(PASTE_SUPPRESS.load(Ordering::SeqCst), baseline + 1);
        drop(outer);
        assert_eq!(PASTE_SUPPRESS.load(Ordering::SeqCst), baseline);
    }

    #[test]
    fn releasing_paste_suppression_at_zero_does_not_underflow() {
        let counter = AtomicU32::new(0);
        release_paste_suppression(&counter);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn concurrent_paste_releases_preserve_the_remaining_suppression() {
        let counter = AtomicU32::new(9);
        std::thread::scope(|scope| {
            for _ in 0..8 {
                scope.spawn(|| release_paste_suppression(&counter));
            }
        });
        assert_eq!(counter.load(Ordering::SeqCst), 1);
        release_paste_suppression(&counter);
        assert_eq!(counter.load(Ordering::SeqCst), 0);
    }
}
