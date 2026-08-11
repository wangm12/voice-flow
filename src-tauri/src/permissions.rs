use serde::Serialize;

#[cfg(target_os = "macos")]
#[link(name = "ApplicationServices", kind = "framework")]
unsafe extern "C" {
    fn AXIsProcessTrustedWithOptions(options: *const std::ffi::c_void) -> bool;
}

#[derive(Debug, Serialize, Clone)]
pub struct PermissionStatus {
    pub microphone: bool,
    pub microphone_status: String,
    pub accessibility: bool,
}
#[cfg(target_os = "macos")]
#[link(name = "AVFoundation", kind = "framework")]
extern "C" {
    static AVMediaTypeAudio: *const objc::runtime::Object;
}
#[cfg(target_os = "macos")]
fn accessibility_is_trusted() -> bool {
    // A null options dictionary performs a non-interactive check. This is the
    // same native API used by Enigo before it creates the keyboard injector,
    // so the settings card and the actual paste path agree on the result.
    unsafe { AXIsProcessTrustedWithOptions(std::ptr::null()) }
}

#[cfg(target_os = "macos")]
pub fn check() -> PermissionStatus {
    unsafe {
        use objc::{class, msg_send, sel, sel_impl};
        let status: i64 =
            msg_send![class!(AVCaptureDevice), authorizationStatusForMediaType: AVMediaTypeAudio];
        PermissionStatus {
            microphone: status == 3,
            microphone_status: match status {
                0 => "not_determined",
                1 => "restricted",
                2 => "denied",
                3 => "authorized",
                _ => "unknown",
            }
            .into(),
            accessibility: accessibility_is_trusted(),
        }
    }
}
#[cfg(not(target_os = "macos"))]
pub fn check() -> PermissionStatus {
    PermissionStatus {
        microphone: true,
        microphone_status: "authorized".into(),
        accessibility: true,
    }
}
#[cfg(target_os = "macos")]
pub fn request_microphone(result: tokio::sync::oneshot::Sender<bool>) -> Result<(), String> {
    use std::sync::{Arc, Mutex};

    use block2::RcBlock;
    use objc2::runtime::Bool;
    use objc2_av_foundation::{AVCaptureDevice, AVCaptureDeviceInput, AVMediaTypeAudio};

    let result = Arc::new(Mutex::new(Some(result)));
    let result_for_completion = Arc::clone(&result);
    let completion = RcBlock::new(move |granted: Bool| {
        if let Some(result) = result_for_completion
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .take()
        {
            let _ = result.send(granted.as_bool());
        }
    });

    let media_type =
        unsafe { AVMediaTypeAudio }.ok_or_else(|| "AVMediaTypeAudio is unavailable".to_owned())?;
    let device = unsafe { AVCaptureDevice::defaultDeviceWithMediaType(media_type) }
        .ok_or_else(|| "no microphone device is available".to_owned())?;

    // AVFoundation automatically displays the microphone authorization prompt
    // when it creates an input while the permission is not determined. This
    // only creates the input; recording does not start until the user does so.
    unsafe { AVCaptureDeviceInput::deviceInputWithDevice_error(&device) }
        .map_err(|error| format!("failed to prepare microphone permission request: {error}"))?;

    unsafe {
        AVCaptureDevice::requestAccessForMediaType_completionHandler(media_type, &completion);
    }
    Ok(())
}

pub fn open_privacy_settings(pane: &str) -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        let url = match pane {
            "accessibility" => "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Accessibility",
            "microphone" => "x-apple.systempreferences:com.apple.settings.PrivacySecurity.extension?Privacy_Microphone",
            _ => return Err(format!("unknown pane: {pane}")),
        };
        std::process::Command::new("open")
            .arg(url)
            .spawn()
            .map_err(|e| e.to_string())?;
        Ok(())
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = pane;
        Err("unsupported platform".into())
    }
}

#[cfg(target_os = "macos")]
pub fn request_accessibility() -> bool {
    accessibility_is_trusted()
}

#[cfg(not(target_os = "macos"))]
pub fn request_accessibility() -> bool {
    true
}
