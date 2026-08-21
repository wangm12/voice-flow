use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::{Duration, Instant};

use tauri::{AppHandle, Emitter};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ModifierBinding {
    Alt,
    Control,
    Shift,
    Command,
    Fn,
}

#[derive(Clone, Default)]
struct ModifierHotkeyConfig {
    binding: Option<ModifierBinding>,
    activation_mode: String,
}

pub const DOUBLE_TAP_MS: u64 = 400;

#[derive(Default)]
struct GestureState {
    key_down: bool,
    awaiting_second_tap: bool,
    last_release_at: Option<Instant>,
    timer_generation: u64,
}

static CONFIG: OnceLock<Arc<Mutex<ModifierHotkeyConfig>>> = OnceLock::new();
static GESTURE: OnceLock<Arc<Mutex<GestureState>>> = OnceLock::new();
static LISTENER: OnceLock<()> = OnceLock::new();
static APP_HANDLE: OnceLock<AppHandle> = OnceLock::new();
/// While a paste is being simulated (enigo injects a synthetic Cmd/Ctrl keystroke),
/// the event tap must ignore modifier events — our own synthetic keys would otherwise
/// be misread as a physical double-tap and re-enter the event system (which crashes
/// the main runloop with an uncaught NSException).
static PASTE_SUPPRESS: AtomicBool = AtomicBool::new(false);

fn lock_recover<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub fn set_paste_suppressed(suppressed: bool) {
    PASTE_SUPPRESS.store(suppressed, Ordering::SeqCst);
    if suppressed {
        reset_state();
    }
}

pub fn is_paste_suppressed() -> bool {
    PASTE_SUPPRESS.load(Ordering::SeqCst)
}

pub fn set_dictation_active(active: bool) {
    if !active {
        reset_state();
    }
}

fn config_store() -> Arc<Mutex<ModifierHotkeyConfig>> {
    CONFIG
        .get_or_init(|| Arc::new(Mutex::new(ModifierHotkeyConfig::default())))
        .clone()
}

fn gesture_store() -> Arc<Mutex<GestureState>> {
    GESTURE
        .get_or_init(|| Arc::new(Mutex::new(GestureState::default())))
        .clone()
}

pub fn is_modifier_only(hotkey: &str) -> bool {
    parse_modifier_hotkey(hotkey).is_some()
}

fn parse_modifier_hotkey(hotkey: &str) -> Option<ModifierBinding> {
    match hotkey.trim() {
        "Alt" | "Option" => Some(ModifierBinding::Alt),
        "Control" | "Ctrl" => Some(ModifierBinding::Control),
        "Shift" => Some(ModifierBinding::Shift),
        "CmdOrControl" | "CommandOrControl" | "CmdOrCtrl" | "Super" | "Meta" | "Command" => {
            Some(ModifierBinding::Command)
        }
        "Fn" | "Function" | "Globe" => Some(ModifierBinding::Fn),
        _ => None,
    }
}

fn keycode_matches_binding(code: u16, binding: ModifierBinding) -> bool {
    use core_graphics::event::KeyCode;

    match binding {
        ModifierBinding::Alt => code == KeyCode::OPTION || code == KeyCode::RIGHT_OPTION,
        ModifierBinding::Command => code == KeyCode::COMMAND || code == KeyCode::RIGHT_COMMAND,
        ModifierBinding::Control => code == KeyCode::CONTROL || code == KeyCode::RIGHT_CONTROL,
        ModifierBinding::Shift => code == KeyCode::SHIFT || code == KeyCode::RIGHT_SHIFT,
        ModifierBinding::Fn => code == KeyCode::FUNCTION,
    }
}

fn modifier_flag(binding: ModifierBinding) -> core_graphics::event::CGEventFlags {
    use core_graphics::event::CGEventFlags;

    match binding {
        ModifierBinding::Alt => CGEventFlags::CGEventFlagAlternate,
        ModifierBinding::Control => CGEventFlags::CGEventFlagControl,
        ModifierBinding::Shift => CGEventFlags::CGEventFlagShift,
        ModifierBinding::Command => CGEventFlags::CGEventFlagCommand,
        ModifierBinding::Fn => CGEventFlags::CGEventFlagSecondaryFn,
    }
}

/// Emit a hotkey event to the frontend listeners. `app.emit` is thread-safe (it posts
/// to webviews via Tauri IPC, not AppKit), so we can call it directly from the event
/// handler — no `run_on_main_thread`, no re-entrancy into the main runloop.
fn emit_hotkey(app: &AppHandle, event_name: &'static str) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let _ = app.emit(event_name, ());
    });
}

fn app_handle() -> Option<AppHandle> {
    APP_HANDLE.get().cloned()
}

fn reset_gesture_state(state: &mut GestureState) {
    state.key_down = false;
    state.awaiting_second_tap = false;
    state.last_release_at = None;
    state.timer_generation = state.timer_generation.wrapping_add(1);
}

fn schedule_tap_timeout(generation: u64) {
    thread::spawn(move || {
        thread::sleep(Duration::from_millis(DOUBLE_TAP_MS));
        let store = gesture_store();
        let mut state = lock_recover(&store);
        if state.timer_generation == generation && state.awaiting_second_tap {
            state.awaiting_second_tap = false;
            state.last_release_at = None;
        }
    });
}

fn on_modifier_press(app: &AppHandle) {
    let store = gesture_store();
    let mut state = lock_recover(&store);
    let now = Instant::now();
    if lock_recover(&config_store()).activation_mode != "double_tap" || state.key_down {
        return;
    }

    if state.awaiting_second_tap
        && state
            .last_release_at
            .map(|t| now.duration_since(t).as_millis() < DOUBLE_TAP_MS as u128)
            .unwrap_or(false)
    {
        state.awaiting_second_tap = false;
        state.last_release_at = None;
        state.timer_generation = state.timer_generation.wrapping_add(1);
        drop(state);
        emit_hotkey(app, "hotkey://double_tap");
        return;
    }
    state.key_down = true;
}

fn on_modifier_release(_app: &AppHandle) {
    let store = gesture_store();
    let mut state = lock_recover(&store);
    if !state.key_down {
        return;
    }
    let now = Instant::now();
    state.key_down = false;
    state.awaiting_second_tap = true;
    state.last_release_at = Some(now);
    state.timer_generation = state.timer_generation.wrapping_add(1);
    let generation = state.timer_generation;
    drop(state);
    schedule_tap_timeout(generation);
}

/// Install the modifier event tap on the MAIN CFRunLoop (the same architecture as
/// the production Voxt / openwhispr hotkey managers). Handling events on the main
/// runloop — instead of a background CFRunLoop thread that re-dispatches to main via
/// `run_on_main_thread` — eliminates the cross-thread re-entrancy that was crashing
/// the process with an uncaught NSException on every double-tap.
///
/// Must be called on AppKit's main thread.
#[cfg(target_os = "macos")]
fn ensure_listener_on_main() -> Result<(), String> {
    if LISTENER.get().is_some() {
        return Ok(());
    }
    use core_foundation::runloop::{kCFRunLoopCommonModes, CFRunLoop};
    use core_graphics::event::{
        CGEventTap, CGEventTapLocation, CGEventTapOptions, CGEventTapPlacement, CGEventType,
        EventField,
    };

    let tap = match CGEventTap::new(
        CGEventTapLocation::HID,
        CGEventTapPlacement::HeadInsertEventTap,
        CGEventTapOptions::ListenOnly,
        vec![CGEventType::FlagsChanged],
        move |_proxy, event_type, event| {
            if !matches!(event_type, CGEventType::FlagsChanged) || crate::hotkey::is_suspended() {
                return None;
            }

            // Ignore our own synthetic paste keystrokes (and any modifier event
            // delivered while pasting) to avoid phantom double-taps.
            if is_paste_suppressed() {
                return None;
            }

            let config = lock_recover(&config_store()).clone();
            let binding = config.binding?;

            let code = event.get_integer_value_field(EventField::KEYBOARD_EVENT_KEYCODE) as u16;
            if !keycode_matches_binding(code, binding) {
                return None;
            }

            if config.activation_mode != "double_tap" {
                return None;
            }
            let is_press = event.get_flags().contains(modifier_flag(binding));

            // We are already on the main thread (the tap lives on the main runloop),
            // so gesture detection + `app.emit` run synchronously here. `app.emit` is
            // thread-safe and posts to webviews via Tauri IPC — no AppKit call, no
            // nested run_on_main_thread, no re-entrancy.
            let app = app_handle()?;
            if is_press {
                on_modifier_press(&app);
            } else {
                on_modifier_release(&app);
            }

            None
        },
    ) {
        Ok(tap) => tap,
        Err(()) => {
            log::error!("modifier hotkey event tap failed to create");
            return Err("无法安装功能键监听，请在系统设置中开启 VoiceFlow 的辅助功能权限".into());
        }
    };

    unsafe {
        let loop_source = tap
            .mach_port
            .create_runloop_source(0)
            .map_err(|_| "无法创建功能键监听 run loop source".to_owned())?;
        let run_loop = CFRunLoop::get_main();
        run_loop.add_source(&loop_source, kCFRunLoopCommonModes);
        tap.enable();
        // The tap is installed for the process lifetime and never removed, so we must
        // keep the `CGEventTap` (and its boxed callback, which the OS dereferences via
        // `user_info` on every event) alive forever. Dropping it here would free the
        // callback while the tap is still installed on the runloop -> the next modifier
        // event dereferences freed memory (SIGSEGV at 0x28).
        std::mem::forget(tap);
    }
    let _ = LISTENER.set(());
    Ok(())
}

#[cfg(target_os = "macos")]
fn ensure_listener(app: AppHandle) -> Result<(), String> {
    let _ = APP_HANDLE.set(app.clone());
    if LISTENER.get().is_some() {
        return Ok(());
    }
    // `modifier_hotkey::register` is only called by the Tauri setup callback or
    // `hotkey::register_on_main`, so it is already running on AppKit's main thread.
    // Installing synchronously lets permission failures reach the settings UI.
    ensure_listener_on_main()
}

#[cfg(not(target_os = "macos"))]
fn ensure_listener(_app: AppHandle) -> Result<(), String> {
    Err("modifier-only hotkeys are only supported on macOS".into())
}

pub fn register(app: &AppHandle, hotkey: &str, activation_mode: &str) -> Result<(), String> {
    let binding = parse_modifier_hotkey(hotkey)
        .ok_or_else(|| format!("unknown modifier-only hotkey `{hotkey}`"))?;
    reset_gesture_state(&mut lock_recover(&gesture_store()));
    ensure_listener(app.clone())?;
    let store = config_store();
    let mut state = lock_recover(&store);
    state.binding = Some(binding);
    state.activation_mode = activation_mode.to_owned();
    Ok(())
}

pub fn reset_state() {
    reset_gesture_state(&mut lock_recover(&gesture_store()));
}

pub fn unregister() {
    reset_gesture_state(&mut lock_recover(&gesture_store()));
    lock_recover(&config_store()).binding = None;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognizes_supported_modifier_only_bindings() {
        for hotkey in ["Alt", "Control", "Shift", "CmdOrControl", "Fn", "Globe"] {
            assert!(
                is_modifier_only(hotkey),
                "expected modifier-only hotkey: {hotkey}"
            );
        }
    }

    #[test]
    fn rejects_combinations_and_unknown_bindings() {
        assert!(!is_modifier_only("CmdOrControl+Shift+Space"));
        assert!(!is_modifier_only("NotAKey"));
    }

    #[test]
    fn reset_clears_pending_gesture_and_invalidates_timer() {
        let mut state = GestureState {
            key_down: true,
            awaiting_second_tap: true,
            last_release_at: Some(Instant::now()),
            timer_generation: 3,
        };
        reset_gesture_state(&mut state);
        assert!(!state.key_down);
        assert!(!state.awaiting_second_tap);
        assert!(state.last_release_at.is_none());
        assert_eq!(state.timer_generation, 4);
    }

    #[test]
    fn production_lock_helper_recovers_poisoned_state() {
        let state = Arc::new(Mutex::new(7));
        let poisoned = state.clone();
        let _ = thread::spawn(move || {
            let _guard = poisoned.lock().expect("initial lock");
            panic!("poison modifier state");
        })
        .join();

        assert_eq!(*lock_recover(&state), 7);
    }
}
