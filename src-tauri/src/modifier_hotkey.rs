//! Fn/Globe and held-combination release observation. The listener reads only
//! flags and physical key state, never characters or typed text.
use crate::dictation::{HotkeySource, ModeTrigger};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use tauri::AppHandle;

#[derive(Clone)]
struct FnConfig {
    source: HotkeySource,
    mode: String,
}

#[derive(Default)]
struct FnGesture {
    key_down: bool,
    chorded: bool,
    blocked_until_release: bool,
    gesture_id: u64,
}

impl FnGesture {
    fn press(&mut self, mode: &str, chorded: bool) -> Option<(ModeTrigger, u64)> {
        if self.key_down || self.blocked_until_release {
            return None;
        }
        self.key_down = true;
        self.chorded = chorded;
        self.gesture_id = crate::hotkey::next_gesture_id();
        (mode == "hold_to_talk" && !chorded).then_some((ModeTrigger::Press, self.gesture_id))
    }
    fn chord(&mut self, mode: &str) -> Option<(ModeTrigger, u64)> {
        if !self.key_down || self.chorded {
            return None;
        }
        self.chorded = true;
        (mode == "hold_to_talk").then_some((ModeTrigger::Cancel, self.gesture_id))
    }
    fn release(&mut self, mode: &str) -> Option<(ModeTrigger, u64)> {
        self.blocked_until_release = false;
        if !std::mem::take(&mut self.key_down) || std::mem::take(&mut self.chorded) {
            return None;
        }
        Some((
            if mode == "hold_to_talk" {
                ModeTrigger::Release
            } else {
                ModeTrigger::Toggle
            },
            self.gesture_id,
        ))
    }
    fn reset(&mut self) {
        self.blocked_until_release |= self.key_down;
        self.key_down = false;
        self.chorded = false;
    }
}

static CONFIG: OnceLock<Arc<Mutex<Option<FnConfig>>>> = OnceLock::new();
static GESTURE: OnceLock<Arc<Mutex<FnGesture>>> = OnceLock::new();
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
fn gesture_store() -> Arc<Mutex<FnGesture>> {
    GESTURE
        .get_or_init(|| Arc::new(Mutex::new(FnGesture::default())))
        .clone()
}

pub fn set_paste_suppressed(suppressed: bool) {
    if suppressed {
        if PASTE_SUPPRESS.fetch_add(1, Ordering::SeqCst) == 0 {
            reset_state();
            crate::hotkey::reset_pressed_state();
        }
    } else {
        let _ = PASTE_SUPPRESS.fetch_update(Ordering::SeqCst, Ordering::SeqCst, |depth| {
            depth.checked_sub(1)
        });
    }
}
pub fn is_paste_suppressed() -> bool {
    PASTE_SUPPRESS.load(Ordering::SeqCst) > 0
}
pub fn set_dictation_active(active: bool) {
    if !active {
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
    fn CGEventSourceKeyState(state_id: i32, key: u16) -> bool;
    fn CGEventSourceFlagsState(state_id: i32) -> u64;
    fn CGEventTapEnable(tap: *const std::ffi::c_void, enable: bool);
}

#[cfg(target_os = "macos")]
fn other_keys_down() -> bool {
    (0..128)
        .filter(|code| *code != 63)
        .any(|code| unsafe { CGEventSourceKeyState(0, code) })
}

pub async fn wait_for_key_release() {
    #[cfg(target_os = "macos")]
    while unsafe { CGEventSourceFlagsState(0) }
        & ((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20) | (1 << 23))
        != 0
        || (0..128).any(|key| unsafe { CGEventSourceKeyState(0, key) })
    {
        tokio::time::sleep(std::time::Duration::from_millis(15)).await;
    }
}

#[cfg(target_os = "macos")]
pub fn ensure_listener(_app: AppHandle) -> Result<(), String> {
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
            let mut state = lock_recover(&store);
            let action = if matches!(event_type, CGEventType::FlagsChanged) && code == 63 {
                if fn_down {
                    let has_modifiers =
                        flags & ((1 << 17) | (1 << 18) | (1 << 19) | (1 << 20)) != 0;
                    state.press(&config.mode, has_modifiers || other_keys_down())
                } else {
                    state.release(&config.mode)
                }
            } else if fn_down
                && (matches!(event_type, CGEventType::KeyDown)
                    || matches!(event_type, CGEventType::FlagsChanged))
            {
                state.chord(&config.mode)
            } else {
                None
            };
            if let Some((trigger, id)) = action {
                crate::dictation::enqueue_hotkey(config.source, trigger, id);
            }
            None
        },
    )
    .map_err(|_| "无法安装按键监听，请在系统设置中开启 VoiceFlow 的辅助功能权限".to_owned())?;
    unsafe {
        let source = tap
            .mach_port
            .create_runloop_source(0)
            .map_err(|_| "无法创建按键监听 run loop source".to_owned())?;
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
    *lock_recover(&gesture_store()) = FnGesture::default();
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
    fn fn_tap_only_fires_on_clean_release_and_deduplicates_repeat() {
        let mut state = FnGesture::default();
        assert!(state.press("tap", false).is_none());
        assert!(state.press("tap", false).is_none());
        let first = state.release("tap").unwrap();
        assert_eq!(first.0, ModeTrigger::Toggle);
        assert!(state.release("tap").is_none());
        state.press("tap", false);
        assert_ne!(first.1, state.release("tap").unwrap().1);
    }
    #[test]
    fn fn_tap_chords_never_toggle() {
        for preexisting_chord in [false, true] {
            let mut state = FnGesture::default();
            state.press("tap", preexisting_chord);
            state.chord("tap");
            assert!(state.release("tap").is_none());
        }
    }
    #[test]
    fn fn_hold_short_release_stops_and_chord_cancels_once() {
        let mut state = FnGesture::default();
        let (_, id) = state.press("hold_to_talk", false).unwrap();
        assert_eq!(
            state.release("hold_to_talk"),
            Some((ModeTrigger::Release, id))
        );
        let (_, id) = state.press("hold_to_talk", false).unwrap();
        assert_eq!(state.chord("hold_to_talk"), Some((ModeTrigger::Cancel, id)));
        assert!(state.chord("hold_to_talk").is_none());
        assert!(state.release("hold_to_talk").is_none());
        assert!(state.press("hold_to_talk", true).is_none());
    }
    #[test]
    fn reset_waits_for_release_and_discards_old_gesture() {
        let mut state = FnGesture::default();
        state.press("hold_to_talk", false);
        state.reset();
        assert!(state.press("hold_to_talk", false).is_none());
        assert!(state.release("hold_to_talk").is_none());
        assert!(state.press("hold_to_talk", false).is_some());
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
}
