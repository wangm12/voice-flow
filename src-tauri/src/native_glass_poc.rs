//! Isolated, opt-in AppKit glass proof of concept. This entrypoint deliberately
//! does not initialize product state, plugins, credentials, recording, or paste.

use std::io::{BufRead, BufReader, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, TcpStream};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use crate::island_window;
use tauri::{Emitter, Listener, Manager};

const POC_IDENTIFIER: &str = "com.voiceflow.desktop.native-glass-poc";
const POC_SERVER_PORT: u16 = 14217;
const POC_SERVER_PATH: &str = "/__voiceflow-glass-poc";
const FRONTEND_READY_EVENT: &str = "native-glass-poc://frontend-ready";
const FRONTEND_CHALLENGE_EVENT: &str = "native-glass-poc://ready";
const PHASE_IDLE: u8 = 0;
const PHASE_PROCESSING: u8 = 1;
const PHASE_PASTE_DISPATCHED: u8 = 2;

static CURRENT_GENERATION: AtomicU64 = AtomicU64::new(0);
static CURRENT_PHASE: AtomicU8 = AtomicU8::new(PHASE_IDLE);

pub fn run() {
    if let Err(error) = run_macos() {
        eprintln!("voiceflow_glass_poc_status=failed detail={error}");
        std::process::exit(2);
    }
}

fn run_macos() -> Result<(), String> {
    verify_poc_server()?;

    let context = tauri::generate_context!("glass-poc/tauri.conf.json", test = true);
    let config = context.config();
    if config.identifier != POC_IDENTIFIER
        || config
            .build
            .dev_url
            .as_ref()
            .is_none_or(|url| url.as_str() != "http://127.0.0.1:14217/")
        || config.app.windows.len() != 1
        || config.app.windows[0].label != "island"
    {
        return Err("unexpected Tauri config identity, dev URL, or window set".to_owned());
    }

    let variant = match std::env::var("VOICEFLOW_GLASS_POC_VARIANT")
        .unwrap_or_else(|_| "regular".to_owned())
        .as_str()
    {
        "regular" => POC_VARIANT_REGULAR,
        "classic" => POC_VARIANT_CLASSIC,
        value => return Err(format!("invalid variant: {value}")),
    };

    let builder = tauri::Builder::default()
        .plugin(tauri_nspanel::init())
        .setup(move |app| {
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let window = app
                .get_webview_window("island")
                .ok_or_else(|| std::io::Error::other("isolated island window missing"))?;
            // Reuse the production NSPanel conversion and click-through defaults.
            island_window::ensure_panel(&window);
            let native_window = window.ns_window().map_err(std::io::Error::other)? as usize;
            let window_number = native_window_number(native_window);

            crate::native_glass::capture_webview(&window);
            let glass_mode = install_native_glass(&window, variant);
            let ready = Arc::new(AtomicBool::new(false));
            let ready_listener = Arc::clone(&ready);
            let _frontend_ready_listener = app.listen(FRONTEND_READY_EVENT, move |_| {
                ready_listener.store(true, Ordering::Release);
            });

            start_stdin_controls(app.handle().clone(), native_window);
            announce_frontend_readiness(
                app.handle().clone(),
                ready,
                native_window,
                window_number,
                glass_mode,
            );
            Ok(())
        });

    builder.run(context).map_err(|error| error.to_string())
}

#[cfg(target_os = "macos")]
const POC_VARIANT_REGULAR: u8 = 0;
#[cfg(target_os = "macos")]
const POC_VARIANT_CLASSIC: u8 = 1;

#[cfg(target_os = "macos")]
fn verify_poc_server() -> Result<(), String> {
    let nonce = std::env::var("VOICEFLOW_GLASS_POC_NONCE")
        .map_err(|_| "VOICEFLOW_GLASS_POC_NONCE is required".to_owned())?;
    if nonce.len() != 32
        || !nonce
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err("nonce must be 32 lowercase hexadecimal characters".to_owned());
    }

    let address = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::LOCALHOST, POC_SERVER_PORT));
    let mut stream = TcpStream::connect_timeout(&address, Duration::from_secs(2))
        .map_err(|error| format!("loopback server unavailable: {error}"))?;
    stream
        .set_read_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    stream
        .set_write_timeout(Some(Duration::from_secs(2)))
        .map_err(|error| error.to_string())?;
    write!(
        stream,
        "GET {POC_SERVER_PATH}?nonce={nonce} HTTP/1.1\r\nHost: 127.0.0.1:{POC_SERVER_PORT}\r\nConnection: close\r\n\r\n"
    )
    .map_err(|error| error.to_string())?;
    stream.flush().map_err(|error| error.to_string())?;

    let mut response = Vec::new();
    stream
        .take(4096)
        .read_to_end(&mut response)
        .map_err(|error| error.to_string())?;
    let response = String::from_utf8(response).map_err(|error| error.to_string())?;
    let Some((headers, body)) = response.split_once("\r\n\r\n") else {
        return Err("server response has no HTTP header boundary".to_owned());
    };
    if !headers.lines().next().is_some_and(|status| {
        status.starts_with("HTTP/1.0 200 ") || status.starts_with("HTTP/1.1 200 ")
    }) {
        return Err("server did not return HTTP 200".to_owned());
    }
    if body != format!("VOICEFLOW_GLASS_POC_SERVER:{nonce}") {
        return Err("server challenge response did not match nonce".to_owned());
    }
    Ok(())
}

#[cfg(target_os = "macos")]
#[derive(Clone, Copy)]
enum GlassMode {
    Regular,
    ClassicFallback,
}

#[cfg(target_os = "macos")]
impl GlassMode {
    fn label(self) -> &'static str {
        match self {
            Self::Regular => "regular",
            Self::ClassicFallback => "classic",
        }
    }
}

#[cfg(target_os = "macos")]
fn install_native_glass(window: &tauri::WebviewWindow, variant: u8) -> GlassMode {
    let requested = if variant == POC_VARIANT_CLASSIC {
        "classic"
    } else {
        "liquid_glass"
    };
    let status = crate::native_glass::configure_on_main_thread(window, requested, false);
    if status.effective == "liquid_glass" {
        GlassMode::Regular
    } else {
        GlassMode::ClassicFallback
    }
}

#[cfg(target_os = "macos")]
fn announce_frontend_readiness(
    app: tauri::AppHandle,
    frontend_ready: Arc<AtomicBool>,
    native_window: usize,
    window_number: isize,
    glass_mode: GlassMode,
) {
    tauri::async_runtime::spawn(async move {
        let deadline = Instant::now() + Duration::from_secs(20);
        while !frontend_ready.load(Ordering::Acquire) && Instant::now() < deadline {
            let selected_glass = if crate::native_glass::is_installed() {
                "regular"
            } else {
                "classic"
            };
            let _ = app.emit_to(
                "island",
                FRONTEND_CHALLENGE_EVENT,
                serde_json::json!({ "glass": selected_glass }),
            );
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
        if !frontend_ready.load(Ordering::Acquire) {
            eprintln!("voiceflow_glass_poc_status=frontend_ready_timeout");
            return;
        }
        println!(
            "VOICEFLOW_GLASS_POC_READY pid={} window={} variant={} glass={} stdin=show,paste,move,status,quit",
            std::process::id(),
            window_number,
            glass_mode.label(),
            if crate::native_glass::is_installed() {
                "NSGlassEffectView"
            } else {
                "classic-fallback"
            }
        );
        print_native_status(&app, native_window);
    });
}

#[cfg(target_os = "macos")]
fn native_window_number(native_window: usize) -> isize {
    unsafe {
        use objc::{msg_send, sel, sel_impl};
        msg_send![native_window as *mut objc::runtime::Object, windowNumber]
    }
}

#[cfg(target_os = "macos")]
fn start_stdin_controls(app: tauri::AppHandle, native_window: usize) {
    std::thread::spawn(move || {
        for line in BufReader::new(std::io::stdin()).lines() {
            let Ok(line) = line else {
                break;
            };
            let fields: Vec<&str> = line.split_ascii_whitespace().collect();
            match fields.as_slice() {
                ["show", generation] => {
                    if let Ok(generation) = generation.parse::<u64>() {
                        show_generation(&app, generation);
                    } else {
                        println!("VOICEFLOW_GLASS_POC_REJECT reason=invalid_generation");
                    }
                }
                ["paste", generation] => {
                    if let Ok(generation) = generation.parse::<u64>() {
                        dispatch_synthetic_paste(&app, generation);
                    } else {
                        println!("VOICEFLOW_GLASS_POC_REJECT reason=invalid_generation");
                    }
                }
                ["move", x, y] => {
                    if let (Ok(x), Ok(y)) = (x.parse::<i32>(), y.parse::<i32>()) {
                        move_panel(&app, x, y);
                    } else {
                        println!("VOICEFLOW_GLASS_POC_REJECT reason=invalid_position");
                    }
                }
                ["status"] => print_native_status(&app, native_window),
                ["quit"] => {
                    app.exit(0);
                    break;
                }
                _ => println!("VOICEFLOW_GLASS_POC_REJECT reason=unknown_command"),
            }
        }
    });
}

#[cfg(target_os = "macos")]
fn move_panel(app: &tauri::AppHandle, x: i32, y: i32) {
    let Some(window) = app.get_webview_window("island") else {
        eprintln!("voiceflow_glass_poc_move_failed reason=island_missing");
        return;
    };
    if let Err(error) =
        window.set_position(tauri::Position::Physical(tauri::PhysicalPosition { x, y }))
    {
        eprintln!("voiceflow_glass_poc_move_failed detail={error}");
        return;
    }
    println!("VOICEFLOW_GLASS_POC_MOVED x={x} y={y}");
}

#[cfg(target_os = "macos")]
fn show_generation(app: &tauri::AppHandle, generation: u64) {
    let previous = CURRENT_GENERATION.load(Ordering::Acquire);
    if generation == 0 || generation <= previous {
        println!(
            "VOICEFLOW_GLASS_POC_REJECT action=show generation={generation} current={previous}"
        );
        return;
    }
    CURRENT_GENERATION.store(generation, Ordering::Release);
    CURRENT_PHASE.store(PHASE_PROCESSING, Ordering::Release);
    island_window::set_hud_actions_visible(app, true);
    // The POC remains passive for the independent cross-App click probe even
    // though the shared production owner marks the processing capsule active.
    island_window::set_interactive(app, false);
    island_window::show_overlay(app);
    if let Err(error) = app.emit_to(
        "island",
        "dictation://state",
        serde_json::json!({
            "state": "processing",
            "phase": "delivery",
            "phase_confirmed": true,
            "session_generation": generation
        }),
    ) {
        eprintln!("voiceflow_glass_poc_emit_failed event=dictation_state detail={error}");
        return;
    }
    println!("VOICEFLOW_GLASS_POC_SHOWN generation={generation} synthetic=true");
}

#[cfg(target_os = "macos")]
fn dispatch_synthetic_paste(app: &tauri::AppHandle, generation: u64) {
    let current = CURRENT_GENERATION.load(Ordering::Acquire);
    if generation == 0 || generation != current {
        println!(
            "VOICEFLOW_GLASS_POC_REJECT action=paste generation={generation} current={current}"
        );
        return;
    }
    if CURRENT_PHASE
        .compare_exchange(
            PHASE_PROCESSING,
            PHASE_PASTE_DISPATCHED,
            Ordering::AcqRel,
            Ordering::Acquire,
        )
        .is_err()
    {
        println!(
            "VOICEFLOW_GLASS_POC_REJECT action=paste generation={generation} reason=not_processing"
        );
        return;
    }

    // This simulates the product's generation-checked paste-dispatched owner;
    // it sends only a fixed event payload and never reads or writes a clipboard.
    island_window::set_hud_actions_visible(app, false);
    island_window::clear_details_visible(app);
    island_window::hide_overlay(app);
    if let Err(error) = app.emit_to(
        "island",
        "dictation://paste-dispatched",
        serde_json::json!({ "session_generation": generation }),
    ) {
        eprintln!("voiceflow_glass_poc_emit_failed event=paste_dispatched detail={error}");
        return;
    }
    println!("VOICEFLOW_GLASS_POC_PASTE_DISPATCHED generation={generation} synthetic=true");
}

#[cfg(target_os = "macos")]
fn print_native_status(app: &tauri::AppHandle, native_window: usize) {
    let (sender, receiver) = mpsc::channel();
    let (host, webview, glass) = crate::native_glass::view_pointers();
    let window = native_window;
    let result = app.run_on_main_thread(move || {
        let status = native_status_snapshot(window, host, webview, glass);
        let _ = sender.send(status);
    });
    if let Err(error) = result {
        eprintln!("voiceflow_glass_poc_status_failed detail={error}");
        return;
    }
    match receiver.recv_timeout(Duration::from_secs(2)) {
        Ok(status) => println!("VOICEFLOW_GLASS_POC_STATUS {status}"),
        Err(error) => eprintln!("voiceflow_glass_poc_status_failed detail={error}"),
    }
}

#[cfg(target_os = "macos")]
fn native_status_snapshot(window: usize, host: usize, webview: usize, glass: usize) -> String {
    use core_graphics::geometry::CGRect;
    use objc::runtime::{class_getName, object_getClass, Object};
    use objc::{msg_send, sel, sel_impl};
    use std::ffi::CStr;

    unsafe {
        let native_window = window as *mut Object;
        let runtime_class = object_getClass(native_window as *const Object);
        let class_name = if runtime_class.is_null() {
            "unknown".to_owned()
        } else {
            CStr::from_ptr(class_getName(runtime_class))
                .to_string_lossy()
                .into_owned()
        };
        let style_mask: usize = msg_send![native_window, styleMask];
        let opaque: bool = msg_send![native_window, isOpaque];
        let panel_is_ns_panel = objc::runtime::Class::get("NSPanel").is_some_and(|panel_class| {
            let kind: objc::runtime::BOOL = msg_send![native_window, isKindOfClass: panel_class];
            kind != objc::runtime::NO
        });
        let key: bool = msg_send![native_window, isKeyWindow];
        let main: bool = msg_send![native_window, isMainWindow];
        let ignores_mouse: bool = msg_send![native_window, ignoresMouseEvents];
        let visible: bool = msg_send![native_window, isVisible];
        let window_number: isize = msg_send![native_window, windowNumber];
        let panel_frame: CGRect = msg_send![native_window, frame];
        let nonactivating = style_mask & (1usize << 7) != 0;

        let (glass_visibility, glass_style, glass_bounds, webview_above_glass) = if glass == 0 {
            (
                "absent".to_owned(),
                "none",
                "none".to_owned(),
                "n/a".to_owned(),
            )
        } else {
            let glass_view = glass as *mut Object;
            let hidden: bool = msg_send![glass_view, isHidden];
            let style: isize = msg_send![glass_view, style];
            let bounds: CGRect = msg_send![glass_view, bounds];
            let host_view = host as *mut Object;
            let webview_view = webview as *mut Object;
            let subviews: *mut Object = msg_send![host_view, subviews];
            let glass_index: usize = msg_send![subviews, indexOfObject: glass_view];
            let webview_index: usize = msg_send![subviews, indexOfObject: webview_view];
            (
                if hidden { "hidden" } else { "visible" }.to_owned(),
                if style == 0 { "regular" } else { "other" },
                format!("{:.0}x{:.0}", bounds.size.width, bounds.size.height),
                (webview_index > glass_index).to_string(),
            )
        };
        let webview_bounds: CGRect = if webview == 0 {
            CGRect::new(
                &core_graphics::geometry::CGPoint::new(0.0, 0.0),
                &core_graphics::geometry::CGSize::new(0.0, 0.0),
            )
        } else {
            msg_send![webview as *mut Object, bounds]
        };
        format!(
            "pid={} window={} panel_class={} is_nspanel={} panel_visible={} panel_frame={:.0},{:.0},{:.0},{:.0} style_mask={} nonactivating={} opaque={} key={} main={} ignores_mouse={} glass={} glass_style={} glass_bounds={} webview_above_glass={} webview_bounds={:.0}x{:.0} generation={} phase={}",
            std::process::id(),
            window_number,
            class_name,
            panel_is_ns_panel,
            visible,
            panel_frame.origin.x,
            panel_frame.origin.y,
            panel_frame.size.width,
            panel_frame.size.height,
            style_mask,
            nonactivating,
            opaque,
            key,
            main,
            ignores_mouse,
            glass_visibility,
            glass_style,
            glass_bounds,
            webview_above_glass,
            webview_bounds.size.width,
            webview_bounds.size.height,
            CURRENT_GENERATION.load(Ordering::Acquire),
            match CURRENT_PHASE.load(Ordering::Acquire) {
                PHASE_IDLE => "idle",
                PHASE_PROCESSING => "processing",
                PHASE_PASTE_DISPATCHED => "paste_dispatched",
                _ => "unknown",
            }
        )
    }
}
