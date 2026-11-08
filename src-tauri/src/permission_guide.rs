//! User-initiated macOS permission companion. File drag is AppKit NSURL drag;
//! native permission readback alone establishes authorization.
use serde::Serialize;

#[derive(Clone, Serialize)]
pub(crate) struct GuideReceipt {
    guide_id: String,
    permission: String,
    bundle_path: Option<String>,
    bundle_identifier: Option<String>,
    signature_status: String,
    placement: String,
    drag_available: bool,
    permission_allowed: bool,
    window_open: bool,
    detail: Option<String>,
}
fn allowed(permission: &str) -> bool {
    let status = crate::permissions::check();
    match permission {
        "accessibility" => status.accessibility,
        "screen" => status.screen_recording,
        _ => false,
    }
}
fn validate(permission: &str, guide_id: &str) -> Result<(), String> {
    if !matches!(permission, "accessibility" | "screen") {
        return Err(
            "Permission guide supports explicit accessibility or screen actions only".into(),
        );
    }
    crate::onboarding::validate_id(guide_id)
}
#[tauri::command]
pub(crate) async fn show_permission_guide(
    app: tauri::AppHandle,
    permission: String,
    guide_id: String,
) -> Result<GuideReceipt, String> {
    validate(&permission, &guide_id)?;
    #[cfg(target_os = "macos")]
    {
        use tauri::Manager;
        let language = app
            .try_state::<crate::AppState>()
            .map(|state| crate::lock_recover(&state.settings).ui_language.clone())
            .unwrap_or_else(|| "system".into());
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            let _ = tx.send(native::show(permission, guide_id, language));
        })
        .map_err(|e| e.to_string())?;
        rx.await
            .map_err(|_| "Permission guide preparation was interrupted".to_owned())?
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Err("Permission companion requires macOS".into())
    }
}
#[tauri::command]
pub(crate) async fn close_permission_guide(
    app: tauri::AppHandle,
    guide_id: String,
) -> Result<(), String> {
    crate::onboarding::validate_id(&guide_id)?;
    #[cfg(target_os = "macos")]
    {
        let (tx, rx) = tokio::sync::oneshot::channel();
        app.run_on_main_thread(move || {
            native::close(Some(&guide_id));
            let _ = tx.send(());
        })
        .map_err(|e| e.to_string())?;
        rx.await
            .map_err(|_| "Permission guide close was interrupted".to_owned())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = app;
        Ok(())
    }
}
pub(crate) fn close_all(app: &tauri::AppHandle) {
    #[cfg(target_os = "macos")]
    if let Err(error) = app.run_on_main_thread(|| native::close(None)) {
        // Native owner remains retained if cleanup could not be scheduled.
        log::warn!("permission guide cleanup could not be scheduled: {error}");
    }
    #[cfg(not(target_os = "macos"))]
    let _ = app;
}

#[cfg(target_os = "macos")]
#[allow(deprecated)]
mod native {
    use super::*;
    use objc::{
        class,
        declare::ClassDecl,
        msg_send,
        runtime::{Class, Object, Protocol, Sel, NO, YES},
        sel, sel_impl,
    };
    use std::{
        cell::RefCell,
        ffi::CStr,
        path::{Path, PathBuf},
        sync::OnceLock,
    };
    use tauri_nspanel::cocoa::foundation::{NSPoint, NSRect, NSSize};
    type Id = *mut Object;
    const NIL: Id = std::ptr::null_mut();
    // All AppKit objects are owned and used exclusively by the main thread.
    struct Guide {
        chinese: bool,
        window: Id,
        controller: Id,
        receipt: GuideReceipt,
        path: Id,
        icon: Id,
        status: Id,
    }
    thread_local! { static GUIDE: RefCell<Option<Guide>> = const { RefCell::new(None) }; }
    unsafe fn string(value: &str) -> Id {
        let object: Id = msg_send![class!(NSString), alloc];
        msg_send![object, initWithBytes: value.as_ptr() length: value.len() encoding: 4usize]
    }
    unsafe fn rust_string(value: Id) -> Option<String> {
        if value.is_null() {
            return None;
        }
        let bytes: *const std::ffi::c_char = msg_send![value, UTF8String];
        if bytes.is_null() {
            None
        } else {
            CStr::from_ptr(bytes).to_str().ok().map(str::to_owned)
        }
    }
    fn rect(x: f64, y: f64, w: f64, h: f64) -> NSRect {
        NSRect::new(NSPoint::new(x, y), NSSize::new(w, h))
    }
    unsafe fn label(view: Id, frame: NSRect, text: &str, size: f64) -> Id {
        let value: Id = msg_send![class!(NSTextField), alloc];
        let value: Id = msg_send![value, initWithFrame: frame];
        let text = string(text);
        let _: () = msg_send![value, setStringValue: text];
        let _: () = msg_send![text, release];
        let _: () = msg_send![value, setEditable: NO];
        let _: () = msg_send![value, setSelectable: NO];
        let _: () = msg_send![value, setBezeled: NO];
        let _: () = msg_send![value, setDrawsBackground: NO];
        let font: Id = msg_send![class!(NSFont), systemFontOfSize: size];
        let _: () = msg_send![value, setFont: font];
        let _: () = msg_send![view, addSubview: value];
        let _: () = msg_send![value, release];
        value
    }
    unsafe fn button(view: Id, controller: Id, frame: NSRect, title: &str, action: Sel) {
        let value: Id = msg_send![class!(NSButton), alloc];
        let value: Id = msg_send![value, initWithFrame: frame];
        let title = string(title);
        let _: () = msg_send![value, setTitle: title];
        let _: () = msg_send![title, release];
        let _: () = msg_send![value, setBezelStyle: 1usize];
        let _: () = msg_send![value, setTarget: controller];
        let _: () = msg_send![value, setAction: action];
        let _: () = msg_send![view, addSubview: value];
        let _: () = msg_send![value, release];
    }
    fn bundle_identity() -> (Option<PathBuf>, Option<String>, String, Option<String>) {
        unsafe {
            let bundle: Id = msg_send![class!(NSBundle), mainBundle];
            let path: Id = msg_send![bundle, bundlePath];
            let identifier: Id = msg_send![bundle, bundleIdentifier];
            let identifier = rust_string(identifier);
            let path = rust_string(path).and_then(|value| Path::new(&value).canonicalize().ok());
            let path = path.filter(|value| {
                value.extension().is_some_and(|ext| ext == "app")
                    && std::env::current_exe()
                        .ok()
                        .and_then(|exe| exe.canonicalize().ok())
                        .is_some_and(|exe| exe.starts_with(value.join("Contents/MacOS")))
            });
            let Some(path) = path else {
                return (None, identifier, "unknown".into(), Some("Current process is not inside a canonical .app bundle. Use a bundled build; a development executable cannot be dragged as an App.".into()));
            };
            let value = string(path.to_string_lossy().as_ref());
            let url: Id = msg_send![class!(NSURL), fileURLWithPath: value isDirectory: YES];
            let _: () = msg_send![value, release];
            #[link(name = "Security", kind = "framework")]
            unsafe extern "C" {
                fn SecStaticCodeCreateWithPath(
                    path: *const std::ffi::c_void,
                    flags: u32,
                    code: *mut *const std::ffi::c_void,
                ) -> i32;
                fn SecStaticCodeCheckValidity(
                    code: *const std::ffi::c_void,
                    flags: u32,
                    requirement: *const std::ffi::c_void,
                ) -> i32;
            }
            #[link(name = "CoreFoundation", kind = "framework")]
            unsafe extern "C" {
                fn CFRelease(value: *const std::ffi::c_void);
            }
            let mut code = std::ptr::null();
            let created = SecStaticCodeCreateWithPath(url.cast(), 0, &mut code);
            let status = if created == 0 && !code.is_null() {
                SecStaticCodeCheckValidity(code, 0, std::ptr::null())
            } else {
                created
            };
            if !code.is_null() {
                CFRelease(code);
            }
            let signature = if status == 0 {
                "valid"
            } else if status == -67062 {
                "unsigned"
            } else {
                "unknown"
            };
            let detail = if status != 0 {
                Some(format!("Current bundle signature is {signature} (Security status {status}). If the permission list does not accept it, use a signed bundled build and the add/Open path."))
            } else {
                None
            };
            (Some(path), identifier, signature.into(), detail)
        }
    }
    fn choose<'a>(chinese: bool, zh: &'a str, en: &'a str) -> &'a str {
        if chinese {
            zh
        } else {
            en
        }
    }
    unsafe fn is_chinese(language: &str) -> bool {
        if language == "zh" {
            return true;
        }
        if language == "en" {
            return false;
        }
        let languages: Id = msg_send![class!(NSLocale), preferredLanguages];
        let count: usize = msg_send![languages, count];
        if count == 0 {
            return false;
        }
        let first: Id = msg_send![languages, objectAtIndex: 0usize];
        rust_string(first).is_some_and(|value| value.starts_with("zh"))
    }
    unsafe fn place(window: Id) -> &'static str {
        let bundle = string("com.apple.systempreferences");
        let apps: Id = msg_send![class!(NSRunningApplication), runningApplicationsWithBundleIdentifier: bundle];
        let _: () = msg_send![bundle, release];
        let count: usize = msg_send![apps, count];
        let Some((x, y, w, h)) = (if count == 1 {
            let app: Id = msg_send![apps, objectAtIndex: 0usize];
            let pid: i32 = msg_send![app, processIdentifier];
            crate::context::sole_visible_window_bounds(pid)
        } else {
            None
        }) else {
            let _: () = msg_send![window, center];
            return "independent_movable";
        };
        let screens: Id = msg_send![class!(NSScreen), screens];
        let count: usize = msg_send![screens, count];
        if count == 0 {
            let _: () = msg_send![window, center];
            return "independent_movable";
        }
        let primary: Id = msg_send![screens, objectAtIndex: 0usize];
        let primary_frame: NSRect = msg_send![primary, frame];
        // CG window metadata uses top-left of primary display; AppKit uses
        // bottom-left global coordinates. Retain negative multi-screen origins.
        let settings = rect(
            x,
            primary_frame.origin.y + primary_frame.size.height - y - h,
            w,
            h,
        );
        let guide_frame: NSRect = msg_send![window, frame];
        for index in 0..count {
            let screen: Id = msg_send![screens, objectAtIndex: index];
            let area: NSRect = msg_send![screen, visibleFrame];
            let center = NSPoint::new(
                settings.origin.x + settings.size.width / 2.0,
                settings.origin.y + settings.size.height / 2.0,
            );
            if center.x < area.origin.x
                || center.x > area.origin.x + area.size.width
                || center.y < area.origin.y
                || center.y > area.origin.y + area.size.height
            {
                continue;
            }
            let gy = settings
                .origin
                .y
                .max(area.origin.y + 12.0)
                .min(area.origin.y + area.size.height - guide_frame.size.height - 12.0);
            let right = settings.origin.x + settings.size.width + 12.0;
            let left = settings.origin.x - guide_frame.size.width - 12.0;
            let gx = if right + guide_frame.size.width <= area.origin.x + area.size.width - 12.0 {
                Some(right)
            } else if left >= area.origin.x + 12.0 {
                Some(left)
            } else {
                None
            };
            if let Some(gx) = gx.filter(|_| guide_frame.size.height + 24.0 <= area.size.height) {
                let _: () = msg_send![window, setFrameOrigin: NSPoint::new(gx,gy)];
                return "adjacent_settings_window";
            }
        }
        let _: () = msg_send![window, center];
        "independent_movable"
    }
    extern "C" fn hit_test(this: &Object, _: Sel, _: NSPoint) -> Id {
        this as *const Object as Id
    }
    extern "C" fn drag_mask(_: &Object, _: Sel, _: Id, _: i64) -> usize {
        1
    } // copy, never move the App
    extern "C" fn mouse_down(this: &Object, _: Sel, event: Id) {
        unsafe {
            let path = *this.get_ivar::<Id>("vfPath");
            let icon = *this.get_ivar::<Id>("vfIcon");
            if path.is_null() || icon.is_null() {
                return;
            }
            let url: Id = msg_send![class!(NSURL), fileURLWithPath: path isDirectory: YES];
            let item: Id = msg_send![class!(NSDraggingItem), alloc];
            let item: Id = msg_send![item, initWithPasteboardWriter: url];
            let _: () = msg_send![item, setDraggingFrame: rect(16.0,16.0,64.0,64.0) contents: icon];
            let items: Id = msg_send![class!(NSArray), arrayWithObject: item];
            let _: Id =
                msg_send![this, beginDraggingSessionWithItems: items event: event source: this];
            let _: () = msg_send![item, release];
        }
    }
    fn drag_class() -> &'static Class {
        static CLASS: OnceLock<&'static Class> = OnceLock::new();
        CLASS.get_or_init(|| {
            let mut decl = ClassDecl::new("VoiceFlowPermissionAppDrag", class!(NSView))
                .expect("unique drag class");
            decl.add_ivar::<Id>("vfPath");
            decl.add_ivar::<Id>("vfIcon");
            if let Some(protocol) = Protocol::get("NSDraggingSource") {
                decl.add_protocol(protocol);
            }
            unsafe {
                decl.add_method(
                    sel!(mouseDown:),
                    mouse_down as extern "C" fn(&Object, Sel, Id),
                );
                decl.add_method(
                    sel!(hitTest:),
                    hit_test as extern "C" fn(&Object, Sel, NSPoint) -> Id,
                );
                decl.add_method(
                    sel!(draggingSession:sourceOperationMaskForDraggingContext:),
                    drag_mask as extern "C" fn(&Object, Sel, Id, i64) -> usize,
                );
            }
            decl.register()
        })
    }
    extern "C" fn refresh(_: &Object, _: Sel, _: Id) {
        GUIDE.with(|cell| {
            let mut state = cell.borrow_mut();
            if let Some(guide) = state.as_mut() {
                guide.receipt.permission_allowed = allowed(&guide.receipt.permission);
                unsafe {
                    let text = string(if guide.receipt.permission_allowed { choose(guide.chinese, "本机权限检查已通过，可以返回 VoiceFlow。", "Allowed by the native permission check. Return to VoiceFlow.") } else { choose(guide.chinese, "权限仍不可用。打开设置或拖动 App 不代表已授权。", "Permission is still unavailable. Opening or dropping the App does not grant it.") });
                    let _: () = msg_send![guide.status, setStringValue: text];
                    let _: () = msg_send![text, release];
                }
            }
        });
    }
    extern "C" fn open_settings(_: &Object, _: Sel, _: Id) {
        let pane = GUIDE.with(|cell| cell.borrow().as_ref().map(|g| g.receipt.permission.clone()));
        if let Some(pane) = pane {
            let _ = crate::permissions::open_privacy_settings(&pane);
        }
    }
    extern "C" fn reveal(_: &Object, _: Sel, _: Id) {
        GUIDE.with(|cell| {
            if let Some(guide) = cell.borrow().as_ref().filter(|g| !g.path.is_null()) {
                unsafe {
                    let workspace: Id = msg_send![class!(NSWorkspace), sharedWorkspace];
                    let url: Id =
                        msg_send![class!(NSURL), fileURLWithPath: guide.path isDirectory: YES];
                    let urls: Id = msg_send![class!(NSArray), arrayWithObject: url];
                    let _: () = msg_send![workspace, activateFileViewerSelectingURLs: urls];
                }
            }
        });
    }
    extern "C" fn back(_: &Object, _: Sel, _: Id) {
        close(None);
    }
    extern "C" fn window_closed(_: &Object, _: Sel, _: Id) {
        cleanup();
    }
    fn controller_class() -> &'static Class {
        static CLASS: OnceLock<&'static Class> = OnceLock::new();
        CLASS.get_or_init(|| {
            let mut decl = ClassDecl::new("VoiceFlowPermissionGuideController", class!(NSObject))
                .expect("unique guide controller");
            unsafe {
                decl.add_method(sel!(refresh:), refresh as extern "C" fn(&Object, Sel, Id));
                decl.add_method(
                    sel!(openSettings:),
                    open_settings as extern "C" fn(&Object, Sel, Id),
                );
                decl.add_method(sel!(reveal:), reveal as extern "C" fn(&Object, Sel, Id));
                decl.add_method(sel!(back:), back as extern "C" fn(&Object, Sel, Id));
                decl.add_method(
                    sel!(windowWillClose:),
                    window_closed as extern "C" fn(&Object, Sel, Id),
                );
            }
            decl.register()
        })
    }
    fn cleanup() {
        let guide = GUIDE.with(|cell| cell.borrow_mut().take());
        if let Some(guide) = guide {
            unsafe {
                let _: () = msg_send![guide.window, setDelegate: NIL];
                let _: () = msg_send![guide.window, release];
                let _: () = msg_send![guide.controller, release];
                if !guide.path.is_null() {
                    let _: () = msg_send![guide.path, release];
                }
                if !guide.icon.is_null() {
                    let _: () = msg_send![guide.icon, release];
                }
            }
        }
    }
    pub(super) fn close(guide_id: Option<&str>) {
        let window = GUIDE.with(|cell| {
            cell.borrow()
                .as_ref()
                .filter(|g| guide_id.is_none_or(|id| id == g.receipt.guide_id))
                .map(|g| g.window)
        });
        if let Some(window) = window {
            unsafe {
                let _: Id = msg_send![window, retain];
                let _: () = msg_send![window, close];
                cleanup();
                let _: () = msg_send![window, release];
            }
        }
    }
    pub(super) fn show(
        permission: String,
        guide_id: String,
        language: String,
    ) -> Result<GuideReceipt, String> {
        if GUIDE.with(|cell| {
            cell.borrow().as_ref().is_some_and(|g| {
                g.receipt.guide_id == guide_id && g.receipt.permission == permission
            })
        }) {
            let controller = GUIDE.with(|cell| cell.borrow().as_ref().unwrap().controller);
            refresh(unsafe { &*controller }, sel!(refresh:), NIL);
            return Ok(GUIDE.with(|cell| cell.borrow().as_ref().unwrap().receipt.clone()));
        }
        close(None);
        let (path, bundle_identifier, signature_status, detail) = bundle_identity();
        let chinese = unsafe { is_chinese(&language) };
        let mut receipt = GuideReceipt {
            guide_id,
            permission,
            placement: "independent_movable".into(),
            bundle_path: path.as_ref().map(|v| v.to_string_lossy().into_owned()),
            bundle_identifier,
            signature_status,
            drag_available: path.is_some(),
            permission_allowed: false,
            window_open: true,
            detail,
        };
        unsafe {
            let controller: Id = msg_send![controller_class(), new];
            let window: Id = msg_send![class!(NSWindow), alloc];
            let window: Id = msg_send![window, initWithContentRect: rect(0.0,0.0,420.0,340.0) styleMask: 15usize backing: 2usize defer: NO];
            if window.is_null() {
                let _: () = msg_send![controller, release];
                return Err("AppKit permission companion could not be created".into());
            }
            let _: () = msg_send![window, setReleasedWhenClosed: NO];
            let title = string(choose(
                chinese,
                "VoiceFlow · 权限操作提示",
                "VoiceFlow · Permission guide",
            ));
            let _: () = msg_send![window, setTitle: title];
            let _: () = msg_send![title, release];
            let _: () = msg_send![window, setDelegate: controller];
            let _: () = msg_send![window, setLevel: 3i64];
            let _: () = msg_send![window, setContentMinSize: NSSize::new(420.0,340.0)];
            let view: Id = msg_send![window, contentView];
            label(
                view,
                rect(20.0, 294.0, 380.0, 28.0),
                choose(
                    chinese,
                    "找到 VoiceFlow，开启右侧开关",
                    "Find VoiceFlow and turn on its switch",
                ),
                16.0,
            );
            let purpose = if receipt.permission == "accessibility" {
                choose(
                    chinese,
                    "辅助功能用于可选的自动粘贴。",
                    "Accessibility enables optional automatic paste.",
                )
            } else {
                choose(
                    chinese,
                    "屏幕权限仅用于主动选择的看屏幕操作。",
                    "Screen capture is only for the look-at-screen action.",
                )
            };
            label(view, rect(20.0, 264.0, 380.0, 26.0), purpose, 12.0);
            label(view, rect(20.0,220.0,380.0,42.0), choose(chinese, "找不到条目？拖动下面当前 App，或点击列表 + 按钮，选择 App 后点打开。", "Missing from the list? Drag the current App below, or use the + button and choose it with Open."), 12.0);
            let path_object = path
                .as_ref()
                .map(|p| string(p.to_string_lossy().as_ref()))
                .unwrap_or(NIL);
            let workspace: Id = msg_send![class!(NSWorkspace), sharedWorkspace];
            let icon: Id = if path_object.is_null() {
                NIL
            } else {
                msg_send![workspace, iconForFile: path_object]
            };
            if !icon.is_null() {
                let _: Id = msg_send![icon, retain];
            }
            let card: Id = msg_send![drag_class(), alloc];
            let card: Id = msg_send![card, initWithFrame: rect(20.0,120.0,380.0,96.0)];
            (*card).set_ivar("vfPath", path_object);
            (*card).set_ivar("vfIcon", icon);
            let _: () = msg_send![card, setWantsLayer: YES];
            let layer: Id = msg_send![card, layer];
            let background: Id = msg_send![class!(NSColor), controlBackgroundColor];
            let background: *const std::ffi::c_void = msg_send![background, CGColor];
            let border: Id = msg_send![class!(NSColor), separatorColor];
            let border: *const std::ffi::c_void = msg_send![border, CGColor];
            let _: () = msg_send![layer, setBackgroundColor: background];
            let _: () = msg_send![layer, setBorderColor: border];
            let _: () = msg_send![layer, setBorderWidth: 1.0f64];
            let _: () = msg_send![layer, setCornerRadius: 10.0f64];
            let access_label = string(choose(chinese, "将当前 VoiceFlow App 文件拖入系统设置。键盘替代方法：在 Finder 中显示，使用系统列表添加按钮，然后打开。", "Drag the current VoiceFlow App file to System Settings. Keyboard alternative: Show in Finder, then use the list add button and Open."));
            let _: () = msg_send![card, setAccessibilityLabel: access_label];
            let _: () = msg_send![access_label, release];
            if !icon.is_null() {
                let image: Id = msg_send![class!(NSImageView), alloc];
                let image: Id = msg_send![image, initWithFrame: rect(16.0,16.0,64.0,64.0)];
                let _: () = msg_send![image, setImage: icon];
                let _: () = msg_send![card, addSubview: image];
                let _: () = msg_send![image, release];
            }
            label(card, rect(94.0, 48.0, 270.0, 24.0), "VoiceFlow", 18.0);
            label(
                card,
                rect(94.0, 15.0, 270.0, 34.0),
                if path.is_some() {
                    choose(
                        chinese,
                        "拖动 App 文件 · 已安装 App 保持原位",
                        "Drag App file · keep the installed App in place",
                    )
                } else {
                    choose(
                        chinese,
                        "没有 .app bundle · 请使用打包版本",
                        "No .app bundle · use a bundled build",
                    )
                },
                11.0,
            );
            let _: () = msg_send![view, addSubview: card];
            let _: () = msg_send![card, release];
            let status = label(
                view,
                rect(20.0, 70.0, 380.0, 44.0),
                receipt.detail.as_deref().unwrap_or(choose(
                    chinese,
                    "等待本机权限检查。拖动完成不代表已授权。",
                    "Waiting for native permission readback. Dragging does not grant permission.",
                )),
                11.0,
            );
            button(
                view,
                controller,
                rect(14.0, 29.0, 125.0, 30.0),
                choose(chinese, "打开系统设置", "Open Settings"),
                sel!(openSettings:),
            );
            button(
                view,
                controller,
                rect(143.0, 29.0, 125.0, 30.0),
                choose(chinese, "在 Finder 中显示", "Show in Finder"),
                sel!(reveal:),
            );
            button(
                view,
                controller,
                rect(274.0, 29.0, 125.0, 30.0),
                choose(chinese, "重新检测", "Check permission"),
                sel!(refresh:),
            );
            button(
                view,
                controller,
                rect(290.0, 0.0, 110.0, 28.0),
                choose(chinese, "返回 / 关闭", "Return / Close"),
                sel!(back:),
            );
            let _ = crate::permissions::open_privacy_settings(&receipt.permission);
            receipt.placement = place(window).into();
            GUIDE.with(|cell| {
                *cell.borrow_mut() = Some(Guide {
                    chinese,
                    window,
                    controller,
                    receipt: receipt.clone(),
                    path: path_object,
                    icon,
                    status,
                })
            });
            let _: () = msg_send![window, orderFrontRegardless];
        }
        refresh(
            unsafe { &*GUIDE.with(|cell| cell.borrow().as_ref().unwrap().controller) },
            sel!(refresh:),
            NIL,
        );
        Ok(GUIDE.with(|cell| cell.borrow().as_ref().unwrap().receipt.clone()))
    }
}
