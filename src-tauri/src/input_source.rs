//! Keyboard input-source helpers for paste.
//!
//! The CJK switch predicate is platform-independent so it can be unit-tested
//! without calling Text Input Source Services. Live TIS calls stay behind
//! `cfg(target_os = "macos")`.

/// Substrings found in CJK input-source IDs or localized names. Includes the
/// Task 7 markers plus real TIS families observed on macOS (`TYIM`,
/// `Japanese`).
const CJK_SOURCE_MARKERS: &[&str] = &[
    "Hans", "Hant", "Kotoeri", "Pinyin", "IMK", "SCIM", "TCIM", "TYIM", "Hiragana", "Korean",
    "Japanese",
];

pub fn should_switch_input_source(id: &str, localized_name: &str) -> bool {
    CJK_SOURCE_MARKERS.iter().any(|marker| {
        contains_ignore_ascii_case(id, marker) || contains_ignore_ascii_case(localized_name, marker)
    })
}

fn contains_ignore_ascii_case(haystack: &str, needle: &str) -> bool {
    if needle.is_empty() || haystack.len() < needle.len() {
        return false;
    }
    haystack
        .as_bytes()
        .windows(needle.len())
        .any(|window| window.eq_ignore_ascii_case(needle.as_bytes()))
}

/// macOS 26 HIToolbox asserts the main dispatch queue inside
/// `TISGetInputSourceProperty` for IME sources. Paste runs on a tokio
/// blocking worker, so TIS must hop when this is true.
pub fn should_hop_tis_to_main_queue(on_main_thread: bool) -> bool {
    !on_main_thread
}

/// Switch to a Latin layout around `paste` when the current source looks CJK.
/// `select_latin` must return true only if the source actually changed.
/// `restore` then runs on the way out, including when `paste` fails.
pub fn run_with_latin_layout_if_cjk<T>(
    current_id: &str,
    current_name: &str,
    select_latin: impl FnOnce() -> bool,
    restore: impl FnOnce(),
    paste: impl FnOnce() -> T,
) -> T {
    let switched = should_switch_input_source(current_id, current_name) && select_latin();
    struct RestoreOnDrop<F: FnOnce()>(Option<F>);
    impl<F: FnOnce()> Drop for RestoreOnDrop<F> {
        fn drop(&mut self) {
            if let Some(restore) = self.0.take() {
                restore();
            }
        }
    }
    let _guard = RestoreOnDrop(switched.then_some(restore));
    paste()
}

/// RAII switch to ABC/US around Cmd+V. No-op when the current source is already
/// Latin, when TIS cannot identify the source, or on non-macOS.
pub struct AbcLayoutGuard {
    _private: (),
    #[cfg(target_os = "macos")]
    previous: Option<RetainedInputSource>,
}

impl AbcLayoutGuard {
    pub fn acquire() -> Self {
        #[cfg(target_os = "macos")]
        {
            macos::acquire_guard()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Self { _private: () }
        }
    }
}

#[cfg(target_os = "macos")]
struct RetainedInputSource(*const std::ffi::c_void);

#[cfg(target_os = "macos")]
impl Drop for RetainedInputSource {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                core_foundation::base::CFRelease(self.0 as core_foundation::base::CFTypeRef);
            }
            self.0 = std::ptr::null();
        }
    }
}

#[cfg(target_os = "macos")]
impl Drop for AbcLayoutGuard {
    fn drop(&mut self) {
        if let Some(previous) = self.previous.take() {
            macos::select_source(previous.0);
        }
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::{should_switch_input_source, AbcLayoutGuard, RetainedInputSource};
    use core::ffi::c_void;
    use core_foundation::array::{CFArray, CFArrayRef};
    use core_foundation::base::{CFRelease, CFType, CFTypeRef, TCFType};
    use core_foundation::dictionary::{CFDictionary, CFDictionaryRef};
    use core_foundation::string::{CFString, CFStringRef};
    use std::thread;
    use std::time::Duration;

    type TISInputSourceRef = *const c_void;

    #[repr(C)]
    struct DispatchQueue {
        _opaque: [usize; 0],
    }

    #[link(name = "System", kind = "dylib")]
    unsafe extern "C" {
        static _dispatch_main_q: DispatchQueue;
        fn dispatch_sync_f(
            queue: *const DispatchQueue,
            context: *mut c_void,
            work: unsafe extern "C" fn(*mut c_void),
        );
    }

    #[link(name = "Carbon", kind = "framework")]
    unsafe extern "C" {
        fn TISCopyCurrentKeyboardInputSource() -> TISInputSourceRef;
        fn TISCopyCurrentASCIICapableKeyboardInputSource() -> TISInputSourceRef;
        fn TISCreateInputSourceList(
            properties: CFDictionaryRef,
            include_all_installed: u8,
        ) -> CFArrayRef;
        fn TISSelectInputSource(input_source: TISInputSourceRef) -> i32;
        fn TISGetInputSourceProperty(
            input_source: TISInputSourceRef,
            property_key: CFStringRef,
        ) -> CFTypeRef;
        static kTISPropertyInputSourceID: CFStringRef;
        static kTISPropertyLocalizedName: CFStringRef;
    }

    const ABC_LAYOUT_ID: &str = "com.apple.keylayout.ABC";
    const US_LAYOUT_ID: &str = "com.apple.keylayout.US";

    fn is_main_thread() -> bool {
        unsafe { libc::pthread_main_np() != 0 }
    }

    fn on_main_queue<T, F: FnOnce() -> T>(f: F) -> T {
        if !super::should_hop_tis_to_main_queue(is_main_thread()) {
            return f();
        }
        struct Job<T, F> {
            work: Option<F>,
            result: Option<T>,
        }
        unsafe extern "C" fn run<T, F: FnOnce() -> T>(ctx: *mut c_void) {
            let job = unsafe { &mut *(ctx as *mut Job<T, F>) };
            let work = job.work.take().expect("main-queue job missing work");
            job.result = Some(work());
        }
        let mut job = Job {
            work: Some(f),
            result: None,
        };
        unsafe {
            dispatch_sync_f(
                &_dispatch_main_q,
                &mut job as *mut Job<T, F> as *mut c_void,
                run::<T, F>,
            );
        }
        job.result.expect("main-queue job did not complete")
    }

    pub(super) fn acquire_guard() -> AbcLayoutGuard {
        // TIS/IME queries must run on the main dispatch queue. The 30 ms
        // settle stays on the paste worker so we do not freeze AppKit.
        let previous = on_main_queue(switch_to_latin_if_cjk);
        if previous.is_some() {
            thread::sleep(Duration::from_millis(30));
        }
        AbcLayoutGuard {
            _private: (),
            previous,
        }
    }

    fn switch_to_latin_if_cjk() -> Option<RetainedInputSource> {
        let current = copy_current_source()?;
        let id =
            source_property(current.0, unsafe { kTISPropertyInputSourceID }).unwrap_or_default();
        let name =
            source_property(current.0, unsafe { kTISPropertyLocalizedName }).unwrap_or_default();
        if !should_switch_input_source(&id, &name) {
            return None;
        }
        if !select_latin_layout() {
            return None;
        }
        Some(current)
    }

    pub(super) fn select_source(source: TISInputSourceRef) {
        if source.is_null() {
            return;
        }
        on_main_queue(|| unsafe {
            let _ = TISSelectInputSource(source);
        });
    }

    fn copy_current_source() -> Option<RetainedInputSource> {
        let source = unsafe { TISCopyCurrentKeyboardInputSource() };
        if source.is_null() {
            None
        } else {
            Some(RetainedInputSource(source))
        }
    }

    fn source_property(source: TISInputSourceRef, key: CFStringRef) -> Option<String> {
        if source.is_null() || key.is_null() {
            return None;
        }
        let value = unsafe { TISGetInputSourceProperty(source, key) };
        if value.is_null() {
            return None;
        }
        let cf_string = unsafe { CFString::wrap_under_get_rule(value as _) };
        Some(cf_string.to_string())
    }

    fn select_latin_layout() -> bool {
        if select_source_id(ABC_LAYOUT_ID) || select_source_id(US_LAYOUT_ID) {
            return true;
        }
        let ascii = unsafe { TISCopyCurrentASCIICapableKeyboardInputSource() };
        if ascii.is_null() {
            return false;
        }
        let id = source_property(ascii, unsafe { kTISPropertyInputSourceID }).unwrap_or_default();
        let name = source_property(ascii, unsafe { kTISPropertyLocalizedName }).unwrap_or_default();
        if should_switch_input_source(&id, &name) {
            unsafe { CFRelease(ascii as CFTypeRef) };
            return false;
        }
        let status = unsafe { TISSelectInputSource(ascii) };
        unsafe { CFRelease(ascii as CFTypeRef) };
        status == 0
    }

    fn select_source_id(id: &str) -> bool {
        let Some(source) = copy_source_with_id(id) else {
            return false;
        };
        unsafe { TISSelectInputSource(source.0) == 0 }
    }

    fn copy_source_with_id(id: &str) -> Option<RetainedInputSource> {
        let id_key = unsafe { CFString::wrap_under_get_rule(kTISPropertyInputSourceID) };
        let id_value = CFString::new(id);
        let filter = CFDictionary::from_CFType_pairs(&[(id_key, id_value)]);
        let list_ref = unsafe { TISCreateInputSourceList(filter.as_concrete_TypeRef(), 0) };
        if list_ref.is_null() {
            return None;
        }
        let list: CFArray<*const c_void> = unsafe { CFArray::wrap_under_create_rule(list_ref) };
        let raw = list.get(0)?;
        let source = *raw as TISInputSourceRef;
        if source.is_null() {
            None
        } else {
            unsafe {
                let retained = CFType::wrap_under_get_rule(source as CFTypeRef);
                let ptr = retained.as_CFTypeRef() as TISInputSourceRef;
                std::mem::forget(retained);
                Some(RetainedInputSource(ptr))
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{run_with_latin_layout_if_cjk, should_switch_input_source};
    use std::panic::AssertUnwindSafe;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::sync::Arc;

    #[test]
    fn tis_input_source_calls_hop_off_the_tokio_paste_worker() {
        // Crash 2026-08-25: AbcLayoutGuard::acquire on a tokio blocking worker
        // called TISGetInputSourceProperty → islGetInputSourceListWithAdditions
        // → dispatch_assert_queue_fail (EXC_BREAKPOINT / SIGTRAP). Cursor paste
        // now reaches simulate_paste; TIS must run on the main dispatch queue.
        assert!(super::should_hop_tis_to_main_queue(false));
        assert!(!super::should_hop_tis_to_main_queue(true));
    }

    #[test]
    fn abc_and_us_layouts_do_not_switch() {
        assert!(!should_switch_input_source(
            "com.apple.keylayout.ABC",
            "ABC"
        ));
        assert!(!should_switch_input_source(
            "com.apple.keylayout.US",
            "U.S."
        ));
        assert!(!should_switch_input_source(
            "com.apple.keylayout.USExtended",
            "ABC – Extended"
        ));
        assert!(!should_switch_input_source(
            "com.apple.keylayout.British",
            "British"
        ));
    }

    #[test]
    fn simplified_and_traditional_chinese_sources_switch() {
        assert!(should_switch_input_source(
            "com.apple.inputmethod.SCIM.ITABC",
            "Pinyin - Simplified"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.SCIM.Shuangpin",
            "Shuangpin"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.SCIM.WBX",
            "Wubi"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.TCIM.Pinyin",
            "Pinyin - Traditional"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.TCIM.Zhuyin",
            "Zhuyin"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.TCIM.Cangjie",
            "Cangjie"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.TYIM.Stroke",
            "Stroke - Traditional"
        ));
        assert!(should_switch_input_source(
            "com.sogou.inputmethod.sogou.pinyin",
            "搜狗拼音"
        ));
        assert!(should_switch_input_source(
            "com.tencent.inputmethod.wetype.pinyin",
            "微信拼音"
        ));
        assert!(should_switch_input_source("unknown.bundle", "Pinyin"));
        assert!(should_switch_input_source("unknown.bundle.Hans", "简体"));
        assert!(should_switch_input_source("unknown.bundle.Hant", "繁體"));
    }

    #[test]
    fn japanese_and_korean_sources_switch() {
        assert!(should_switch_input_source(
            "com.apple.inputmethod.Kotoeri.Japanese",
            "Hiragana"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.Kotoeri.RomajiTyping.Japanese",
            "Hiragana"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.Japanese.Hiragana",
            "ひらがな"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.Korean.2SetKorean",
            "2-Set Korean"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.Korean.3SetKorean",
            "3-Set Korean"
        ));
        assert!(should_switch_input_source(
            "com.apple.inputmethod.Korean.HNCRomaja",
            "Romaja"
        ));
    }

    #[test]
    fn imk_marker_matches_third_party_input_methods() {
        assert!(should_switch_input_source(
            "com.googlecode.rimeime.inputmethod.Squirrel",
            "Rime IMK"
        ));
        assert!(should_switch_input_source(
            "org.unknown.IMK.Custom",
            "Custom IME"
        ));
    }

    #[test]
    fn empty_or_latin_names_do_not_switch() {
        assert!(!should_switch_input_source("", ""));
        assert!(!should_switch_input_source("com.apple.keylayout.ABC", ""));
        assert!(!should_switch_input_source(
            "com.apple.inputmethod.Roman",
            "Romaji"
        ));
        assert!(!should_switch_input_source(
            "com.apple.CharacterPaletteIM",
            "Emoji & Symbols"
        ));
    }

    #[test]
    fn latin_layout_guard_restores_after_paste_failure() {
        let restored = Arc::new(AtomicBool::new(false));
        let restored_on_drop = restored.clone();
        let result = run_with_latin_layout_if_cjk(
            "com.apple.inputmethod.SCIM.ITABC",
            "Pinyin - Simplified",
            || true,
            move || restored_on_drop.store(true, Ordering::SeqCst),
            || Err::<(), &'static str>("paste failed"),
        );
        assert_eq!(result, Err("paste failed"));
        assert!(restored.load(Ordering::SeqCst));
    }

    #[test]
    fn latin_layout_guard_skips_switch_for_abc() {
        let selected = Arc::new(AtomicUsize::new(0));
        let selected_in_switch = selected.clone();
        run_with_latin_layout_if_cjk(
            "com.apple.keylayout.ABC",
            "ABC",
            move || {
                selected_in_switch.fetch_add(1, Ordering::SeqCst);
                true
            },
            || panic!("ABC must not restore a layout that was never switched"),
            || (),
        );
        assert_eq!(selected.load(Ordering::SeqCst), 0);
    }

    #[test]
    fn latin_layout_guard_does_not_restore_when_switch_fails() {
        let restored = Arc::new(AtomicBool::new(false));
        let restored_on_drop = restored.clone();
        run_with_latin_layout_if_cjk(
            "com.apple.inputmethod.SCIM.ITABC",
            "Pinyin",
            || false,
            move || restored_on_drop.store(true, Ordering::SeqCst),
            || (),
        );
        assert!(!restored.load(Ordering::SeqCst));
    }

    #[test]
    fn latin_layout_guard_restores_when_paste_panics() {
        let restored = Arc::new(AtomicBool::new(false));
        let restored_on_drop = restored.clone();
        let panicked = std::panic::catch_unwind(AssertUnwindSafe(|| {
            run_with_latin_layout_if_cjk(
                "com.apple.inputmethod.SCIM.ITABC",
                "Pinyin",
                || true,
                move || restored_on_drop.store(true, Ordering::SeqCst),
                || panic!("paste panicked"),
            )
        }));
        assert!(panicked.is_err());
        assert!(restored.load(Ordering::SeqCst));
    }
}
