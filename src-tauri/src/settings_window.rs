//! A fixed settings window that fits the current display's usable area.

use tauri::{LogicalSize, Manager, PhysicalSize, WebviewWindow};

pub(crate) fn configure(window: &WebviewWindow) {
    fit_to_monitor(window);
    // Disabling zoom alone does not disable the native View menu's fullscreen action.
    #[cfg(target_os = "macos")]
    {
        // Tauri's default View menu contains only the fullscreen action.
        // Remove the now-inapplicable menu instead of leaving a no-op command.
        if let Some(menu) = window.app_handle().menu() {
            for item in menu.items().unwrap_or_default() {
                if item
                    .as_submenu()
                    .is_some_and(|submenu| submenu.text().ok().as_deref() == Some("View"))
                {
                    let _ = menu.remove(&item);
                }
            }
        }
        let native_window = window.clone();
        let _ = window.run_on_main_thread(move || {
            use objc::{msg_send, sel, sel_impl};
            let Ok(pointer) = native_window.ns_window() else {
                return;
            };
            let window = pointer as *mut objc::runtime::Object;
            unsafe {
                let behavior: usize = msg_send![window, collectionBehavior];
                // Public NSWindowCollectionBehavior: FullScreenPrimary (7),
                // FullScreenAuxiliary (8), FullScreenNone (9).
                let behavior = (behavior & !((1 << 7) | (1 << 8))) | (1 << 9);
                let _: () = msg_send![window, setCollectionBehavior: behavior];
                // Public NSWindowTitlebarSeparatorStyle::None. The native
                // controls remain over the sidebar without a separate header line.
                let _: () = msg_send![window, setTitlebarSeparatorStyle: 1usize];
            }
        });
    }
}

fn fitted_size(
    preferred: LogicalSize<f64>,
    work_area: PhysicalSize<u32>,
    scale: f64,
    frame: PhysicalSize<u32>,
) -> LogicalSize<f64> {
    // Leave 16 logical pixels around the outer window, including its titlebar.
    let width = (work_area.width.saturating_sub(frame.width) as f64 / scale - 32.0)
        .floor()
        .max(1.0);
    let height = (work_area.height.saturating_sub(frame.height) as f64 / scale - 32.0)
        .floor()
        .max(1.0);
    LogicalSize::new(preferred.width.min(width), preferred.height.min(height))
}

pub(crate) fn fit_to_monitor(window: &WebviewWindow) {
    let Some(config) = window
        .app_handle()
        .config()
        .app
        .windows
        .iter()
        .find(|config| config.label == window.label())
    else {
        return;
    };
    let monitor = window
        .current_monitor()
        .ok()
        .flatten()
        .or_else(|| window.primary_monitor().ok().flatten());
    let (Some(monitor), Ok(inner), Ok(outer)) = (monitor, window.inner_size(), window.outer_size())
    else {
        return;
    };
    let size = fitted_size(
        LogicalSize::new(config.width, config.height),
        monitor.work_area().size,
        monitor.scale_factor(),
        PhysicalSize::new(
            outer.width.saturating_sub(inner.width),
            outer.height.saturating_sub(inner.height),
        ),
    );
    // Size events also reach here. Comparing physical sizes avoids a resize loop.
    if inner != size.to_physical::<u32>(monitor.scale_factor()) {
        if let Err(error) = window.set_size(size) {
            log::warn!("could not fit settings window to display: {error}");
        } else {
            let _ = window.center();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keeps_preferred_size_on_a_large_display() {
        let size = fitted_size(
            LogicalSize::new(1035.0, 750.0),
            PhysicalSize::new(1440, 900),
            1.0,
            PhysicalSize::new(0, 28),
        );
        assert_eq!(size, LogicalSize::new(1035.0, 750.0));
    }

    #[test]
    fn accounts_for_retina_scale_and_titlebar_on_a_small_display() {
        let size = fitted_size(
            LogicalSize::new(1035.0, 750.0),
            PhysicalSize::new(1500, 1050),
            2.0,
            PhysicalSize::new(0, 56),
        );
        assert_eq!(size, LogicalSize::new(718.0, 465.0));
    }

    #[test]
    fn leaves_room_for_the_dock_without_shrinking_the_width() {
        let size = fitted_size(
            LogicalSize::new(1035.0, 750.0),
            PhysicalSize::new(1366, 720),
            1.0,
            PhysicalSize::new(0, 28),
        );
        assert_eq!(size, LogicalSize::new(1035.0, 660.0));
    }
}
