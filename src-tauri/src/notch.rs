//! Floating pill overlay placement — bottom-center of the active monitor (Typeless-style).

use serde::Serialize;
use tauri::{AppHandle, Monitor};

/// Logical window size in points (pill animates inside, centered).
const PILL_WIDTH: f64 = 172.0;
pub const PILL_WIDTH_WITH_PARTIAL: f64 = 400.0;
const PILL_HEIGHT: f64 = 68.0;
/// Gap between pill bottom edge and top of dock / screen edge.
const BOTTOM_GAP: f64 = 12.0;
/// Used only when the reported work area is the full frame and does not
/// subtract the Dock. Default macOS Dock + padding is about this tall.
const MIN_DOCK_INSET: f64 = 70.0;

pub fn pill_window_width(has_partial: bool) -> f64 {
    if has_partial {
        PILL_WIDTH_WITH_PARTIAL
    } else {
        PILL_WIDTH
    }
}

#[derive(Debug, Clone, Copy)]
struct ScreenRect {
    x: f64,
    y: f64,
    width: f64,
    height: f64,
}

impl ScreenRect {
    fn contains(self, point: (f64, f64)) -> bool {
        point.0 >= self.x
            && point.0 < self.x + self.width
            && point.1 >= self.y
            && point.1 < self.y + self.height
    }
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct IslandPlacement {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

fn placement_for_monitor(monitor: &Monitor, pill_width: f64) -> IslandPlacement {
    let scale = monitor.scale_factor();
    let pos = monitor.position();
    let size = monitor.size();
    let work_area = monitor.work_area();
    placement_for_monitor_rect(
        ScreenRect {
            x: pos.x as f64,
            y: pos.y as f64,
            width: size.width as f64,
            height: size.height as f64,
        },
        ScreenRect {
            x: work_area.position.x as f64,
            y: work_area.position.y as f64,
            width: work_area.size.width as f64,
            height: work_area.size.height as f64,
        },
        scale,
        pill_width,
    )
}

fn monitor_for_cursor(app: &AppHandle) -> Option<Monitor> {
    let cursor = app.cursor_position().ok()?;
    let monitors = app.available_monitors().ok()?;
    let primary_scale = app
        .primary_monitor()
        .ok()
        .flatten()
        .map(|monitor| monitor.scale_factor())
        .unwrap_or(1.0);
    if let Some(monitor) = monitors.iter().find(|monitor| {
        let p = monitor.position();
        let s = monitor.size();
        point_in_monitor_logical(
            (cursor.x, cursor.y),
            primary_scale,
            ScreenRect {
                x: p.x as f64,
                y: p.y as f64,
                width: s.width as f64,
                height: s.height as f64,
            },
            monitor.scale_factor(),
        )
    }) {
        return Some(monitor.clone());
    }
    monitors.into_iter().find(|monitor| {
        let p = monitor.position();
        let s = monitor.size();
        ScreenRect {
            x: p.x as f64,
            y: p.y as f64,
            width: s.width as f64,
            height: s.height as f64,
        }
        .contains((cursor.x, cursor.y))
    })
}

fn point_in_monitor_logical(
    cursor_physical: (f64, f64),
    primary_scale: f64,
    monitor_physical: ScreenRect,
    monitor_scale: f64,
) -> bool {
    let cursor = (
        cursor_physical.0 / primary_scale.max(1.0),
        cursor_physical.1 / primary_scale.max(1.0),
    );
    let scale = monitor_scale.max(1.0);
    ScreenRect {
        x: monitor_physical.x / scale,
        y: monitor_physical.y / scale,
        width: monitor_physical.width / scale,
        height: monitor_physical.height / scale,
    }
    .contains(cursor)
}

pub fn placement_for_cursor_screen(app: &AppHandle) -> IslandPlacement {
    placement_for_cursor_screen_with_width(app, pill_window_width(false))
}

pub fn placement_for_cursor_screen_with_width(app: &AppHandle, pill_width: f64) -> IslandPlacement {
    if let Some(monitor) = monitor_for_cursor(app).or_else(|| app.primary_monitor().ok().flatten())
    {
        return placement_for_monitor(&monitor, pill_width);
    }

    placement_for_monitor_at_scale_with_width(0.0, 0.0, 1440.0, 900.0, 1.0, 56.0, pill_width)
}

fn placement_for_monitor_at_scale(
    origin_x: f64,
    origin_y: f64,
    monitor_width: f64,
    monitor_height: f64,
    scale: f64,
    dock_inset: f64,
) -> IslandPlacement {
    placement_for_monitor_at_scale_with_width(
        origin_x,
        origin_y,
        monitor_width,
        monitor_height,
        scale,
        dock_inset,
        PILL_WIDTH,
    )
}

fn placement_for_monitor_at_scale_with_width(
    origin_x: f64,
    origin_y: f64,
    monitor_width: f64,
    monitor_height: f64,
    scale: f64,
    dock_inset: f64,
    pill_width: f64,
) -> IslandPlacement {
    placement_for_monitor_rect(
        ScreenRect {
            x: origin_x,
            y: origin_y,
            width: monitor_width,
            height: monitor_height,
        },
        ScreenRect {
            x: origin_x,
            y: origin_y,
            width: monitor_width,
            height: monitor_height - dock_inset * scale,
        },
        scale,
        pill_width,
    )
}

fn work_area_above_dock(frame: ScreenRect, reported: ScreenRect, scale: f64) -> ScreenRect {
    let frame_bottom = frame.y + frame.height;
    let reported_bottom = reported.y + reported.height;
    let bottom_inset = (frame_bottom - reported_bottom).max(0.0);
    if bottom_inset > scale {
        return reported;
    }
    let inset = MIN_DOCK_INSET * scale;
    ScreenRect {
        x: reported.x,
        y: reported.y,
        width: reported.width,
        height: (reported.height - inset).max(0.0),
    }
}

fn placement_for_monitor_rect(
    frame: ScreenRect,
    work_area: ScreenRect,
    scale: f64,
    pill_width: f64,
) -> IslandPlacement {
    let work_area = work_area_above_dock(frame, work_area, scale);
    let width = pill_width * scale;
    let height = PILL_HEIGHT * scale;
    let raw_x = work_area.x + (work_area.width - width) / 2.0;
    let raw_y = work_area.y + work_area.height - height - BOTTOM_GAP * scale;
    let min_x = frame.x;
    let min_y = frame.y;
    let max_x = (work_area.x + work_area.width - width).max(min_x);
    let max_y = (work_area.y + work_area.height - height - BOTTOM_GAP * scale).max(min_y);
    IslandPlacement {
        x: raw_x.clamp(min_x, max_x),
        y: raw_y.clamp(min_y, max_y),
        width,
        height,
    }
}

#[tauri::command]
pub fn island_placement(app: AppHandle) -> IslandPlacement {
    placement_for_cursor_screen(&app)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn centers_pill_above_dock() {
        let p = placement_for_monitor_at_scale(100.0, 20.0, 1440.0, 900.0, 1.0, 56.0);
        assert_eq!((p.x, p.y, p.width, p.height), (734.0, 784.0, 172.0, 68.0));
        assert_eq!(pill_window_width(false), PILL_WIDTH);
    }

    #[test]
    fn uses_partial_width_when_expected() {
        assert_eq!(pill_window_width(true), PILL_WIDTH_WITH_PARTIAL);
        assert_eq!(PILL_WIDTH_WITH_PARTIAL, 400.0);
        let p = placement_for_monitor_at_scale_with_width(
            100.0,
            20.0,
            1440.0,
            900.0,
            1.0,
            56.0,
            pill_window_width(true),
        );
        assert_eq!((p.x, p.y, p.width, p.height), (620.0, 784.0, 400.0, 68.0));
    }

    #[test]
    fn scales_for_retina() {
        let p = placement_for_monitor_at_scale(0.0, 0.0, 3024.0, 1964.0, 2.0, 56.0);
        assert_eq!(
            (p.x, p.y, p.width, p.height),
            (1340.0, 1692.0, 344.0, 136.0)
        );
    }

    #[test]
    fn keeps_pill_above_the_dock_when_work_area_matches_the_full_frame() {
        let frame = ScreenRect {
            x: 0.0,
            y: 0.0,
            width: 1440.0,
            height: 900.0,
        };
        let p = placement_for_monitor_rect(frame, frame, 1.0, PILL_WIDTH);
        let pill_bottom = p.y + p.height;
        assert!(
            900.0 - pill_bottom >= 70.0,
            "pill bottom {pill_bottom} overlaps the dock"
        );
    }

    #[test]
    fn keeps_island_inside_a_small_monitor() {
        let p = placement_for_monitor_at_scale(0.0, 0.0, 160.0, 100.0, 1.0, 56.0);
        assert_eq!((p.x, p.y), (0.0, 0.0));
    }

    #[test]
    fn matches_cursor_on_a_mixed_scale_secondary_monitor() {
        let secondary = ScreenRect {
            x: 1440.0,
            y: 0.0,
            width: 1920.0,
            height: 1080.0,
        };
        // The cursor is reported in the primary screen's 2x physical space,
        // while the secondary monitor is 1x.
        assert!(point_in_monitor_logical(
            (3000.0, 500.0),
            2.0,
            secondary,
            1.0
        ));
        assert!(!point_in_monitor_logical(
            (7000.0, 500.0),
            2.0,
            secondary,
            1.0
        ));
    }
}
