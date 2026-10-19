#[derive(Clone, Copy, Debug, Default)]
pub struct Rect {
    pub left: i32,
    pub top: i32,
    pub right: i32,
    pub bottom: i32,
}

impl Rect {
    pub fn width(self) -> i32 {
        self.right - self.left
    }
    pub fn height(self) -> i32 {
        self.bottom - self.top
    }
}

/// All rectangles share one physical-pixel coordinate space (screen or parent client).
#[derive(Clone, Copy, Debug)]
pub struct TaskbarLayout {
    taskbar: Rect,
    width: i32,
    height: i32,
    horizontal: bool,
    start: i32,
    end: i32,
    scale: f64,
}

impl TaskbarLayout {
    pub fn new(taskbar: Rect, tray: Option<Rect>, scale: f64) -> Option<Self> {
        Self::with_width(taskbar, tray, scale, (140.0 * scale).round() as i32)
    }

    /// Use the measured readout width rather than reserving invisible space beside text.
    pub fn with_width(taskbar: Rect, tray: Option<Rect>, scale: f64, width: i32) -> Option<Self> {
        // Explorer can briefly report an empty rectangle while rebuilding its taskbar.
        if taskbar.width() <= 0 || taskbar.height() <= 0 || !(0.5..=8.0).contains(&scale) {
            return None;
        }
        let width = width.clamp(1, taskbar.width());
        let height = ((40.0 * scale).round() as i32).min(taskbar.height()).max(1);
        let gap = (8.0 * scale).round() as i32;
        let horizontal = taskbar.width() >= taskbar.height();
        let (start, limit, tray_start, size) = if horizontal {
            (taskbar.left, taskbar.right, tray.map(|r| r.left), width)
        } else {
            (taskbar.top, taskbar.bottom, tray.map(|r| r.top), height)
        };
        // Never place the readout over notification icons. Invalid/transient tray bounds
        // fall back to the end of the taskbar, with the same gap as automatic placement.
        let end = (tray_start
            .filter(|&p| p > start && p <= limit)
            .unwrap_or(limit)
            - gap
            - size)
            .max(start);
        Some(Self {
            taskbar,
            width,
            height,
            horizontal,
            start,
            end,
            scale,
        })
    }

    pub fn position(self, offset_x: i32, position: Option<u16>) -> Rect {
        let along = position.map_or(self.end, |p| {
            self.start
                + ((f64::from(self.end - self.start) * f64::from(p.min(10_000))) / 10_000.0).round()
                    as i32
        });
        let (mut x, y) = if self.horizontal {
            (
                along,
                self.taskbar.top + (self.taskbar.height() - self.height) / 2,
            )
        } else {
            (
                self.taskbar.left + (self.taskbar.width() - self.width) / 2,
                along,
            )
        };
        // Apply the horizontal offset when no relative taskbar position is set.
        if position.is_none() {
            x = (x + (f64::from(offset_x) * self.scale).round() as i32)
                .clamp(self.taskbar.left, self.taskbar.right - self.width);
        }
        Rect {
            left: x,
            top: y,
            right: x + self.width,
            bottom: y + self.height,
        }
    }

    /// Normalize a dragged top-left coordinate so resizing/DPI changes keep its relative spot.
    pub fn dragged_position(self, x: i32, y: i32) -> u16 {
        let coordinate = if self.horizontal { x } else { y }.clamp(self.start, self.end);
        let length = self.end - self.start;
        if length == 0 {
            return 0;
        }
        ((f64::from(coordinate - self.start) / f64::from(length)) * 10_000.0).round() as u16
    }
}

pub fn widget_rect(taskbar: Rect, tray: Option<Rect>, scale: f64, offset_x: i32) -> Option<Rect> {
    Some(TaskbarLayout::new(taskbar, tray, scale)?.position(offset_x, None))
}

/// A click stays a click until the system drag threshold is crossed. Keep the
/// original pointer-to-window offset so grabbing text never makes the widget jump.
#[derive(Clone, Copy, Debug)]
pub struct DragSession {
    pressed: (i32, i32),
    grab: (i32, i32),
    threshold: (i32, i32),
    pub position: Option<u16>,
}

impl DragSession {
    pub fn new(pointer: (i32, i32), origin: (i32, i32), threshold: (i32, i32)) -> Self {
        Self {
            pressed: pointer,
            grab: (pointer.0 - origin.0, pointer.1 - origin.1),
            threshold,
            position: None,
        }
    }

    pub fn update(&mut self, layout: TaskbarLayout, pointer: (i32, i32)) {
        if self.position.is_none()
            && (pointer.0 - self.pressed.0).abs() < self.threshold.0
            && (pointer.1 - self.pressed.1).abs() < self.threshold.1
        {
            return;
        }
        self.position =
            Some(layout.dragged_position(pointer.0 - self.grab.0, pointer.1 - self.grab.1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn horizontal(scale: f64) -> TaskbarLayout {
        TaskbarLayout::new(
            Rect {
                left: -2560,
                top: 1400,
                right: 0,
                bottom: 1460,
            },
            Some(Rect {
                left: -300,
                top: 1400,
                right: 0,
                bottom: 1460,
            }),
            scale,
        )
        .unwrap()
    }

    #[test]
    fn drag_keeps_grab_offset_and_click_jitter_does_not_change_position() {
        let layout = horizontal(1.5);
        let origin = layout.position(0, None);
        let pointer = (origin.left + 80, origin.top + 15);
        let mut drag = DragSession::new(pointer, (origin.left, origin.top), (6, 6));
        drag.update(layout, (pointer.0 - 3, pointer.1 + 2));
        assert_eq!(drag.position, None);
        drag.update(layout, (pointer.0 - 700, pointer.1 + 200));
        let moved = layout.position(0, drag.position);
        assert!((moved.left - (origin.left - 700)).abs() <= 1);
        assert_eq!(moved.top, origin.top);
        // Returning near the original pointer still counts as an active drag.
        drag.update(layout, pointer);
        assert!(drag.position.is_some());
    }

    #[test]
    fn compact_readout_can_reach_left_edge_when_grabbed_near_its_right_edge() {
        for left in [0, -2560] {
            let taskbar = Rect {
                left,
                top: 1040,
                right: left + 1920,
                bottom: 1080,
            };
            let layout = TaskbarLayout::with_width(taskbar, None, 1.0, 82).unwrap();
            let origin = layout.position(0, None);
            let mut drag = DragSession::new(
                (origin.right - 4, origin.top + 10),
                (origin.left, origin.top),
                (4, 4),
            );
            // The cursor cannot travel past the left edge of the monitor. Even
            // grabbing the far end of the readout must still let it reach the edge.
            drag.update(layout, (left, origin.top + 10));
            let position = layout.position(0, drag.position);
            assert_eq!(drag.position, Some(0));
            assert_eq!(position.left, left);
            assert_eq!(position.width(), 82);
            assert_eq!(layout.position(0, Some(10_000)).right, taskbar.right - 8);
        }
    }

    #[test]
    fn dragging_is_clamped_at_taskbar_edges_before_the_tray() {
        let layout = horizontal(1.5);
        assert_eq!(layout.dragged_position(-10_000, 0), 0);
        assert_eq!(layout.dragged_position(10_000, 0), 10_000);
        let start = layout.position(0, Some(0));
        let end = layout.position(0, Some(10_000));
        assert_eq!(start.left, -2560);
        assert_eq!(end.right, -312); // 8 logical pixels before the tray at 150% DPI.
    }

    #[test]
    fn saved_position_keeps_its_relative_spot_after_resolution_and_dpi_change() {
        for scale in [1.0, 1.5, 2.0] {
            let layout = horizontal(scale);
            let start = layout.position(0, Some(0)).left;
            let end = layout.position(0, Some(10_000)).left;
            let middle = layout.position(0, Some(5_000));
            assert!((middle.left - (start + end) / 2).abs() <= 1);
            assert!(
                (i32::from(layout.dragged_position(middle.left, middle.top)) - 5_000).abs() <= 3
            );
        }
        let resized = TaskbarLayout::new(
            Rect {
                left: 0,
                top: 1040,
                right: 1920,
                bottom: 1080,
            },
            None,
            1.0,
        )
        .unwrap();
        assert_eq!(resized.position(0, Some(5_000)).left, (1920 - 8 - 140) / 2);
    }

    #[test]
    fn vertical_drag_moves_only_along_the_taskbar_and_tiny_taskbars_are_safe() {
        let layout = TaskbarLayout::new(
            Rect {
                left: -48,
                top: -1080,
                right: 0,
                bottom: 0,
            },
            Some(Rect {
                left: -48,
                top: -100,
                right: 0,
                bottom: 0,
            }),
            1.0,
        )
        .unwrap();
        let position = layout.dragged_position(999, -800);
        let moved = layout.position(0, Some(position));
        assert_eq!(moved.left, -48);
        assert!((moved.top + 800).abs() <= 1);
        assert_eq!(layout.position(0, Some(10_000)).bottom, -108);
        let tiny = TaskbarLayout::new(
            Rect {
                left: 0,
                top: 0,
                right: 20,
                bottom: 20,
            },
            None,
            2.0,
        )
        .unwrap();
        assert_eq!(tiny.dragged_position(999, -999), 0);
        assert_eq!(tiny.position(0, Some(10_000)).right, 20);
    }

    #[test]
    fn high_dpi_negative_monitor_and_extreme_offset_stay_inside_taskbar() {
        let taskbar = Rect {
            left: -2560,
            top: 1400,
            right: 0,
            bottom: 1440,
        };
        let tray = Rect {
            left: -300,
            top: 1400,
            right: 0,
            bottom: 1440,
        };
        for offset in [-1000, 0, 1000] {
            let rect = widget_rect(taskbar, Some(tray), 1.5, offset).unwrap();
            assert_eq!(rect.width(), 210);
            assert!(rect.left >= taskbar.left && rect.right <= taskbar.right);
            assert!(rect.top >= taskbar.top && rect.bottom <= taskbar.bottom);
        }
    }

    #[test]
    fn vertical_taskbar_is_bounded() {
        let taskbar = Rect {
            left: 0,
            top: 0,
            right: 48,
            bottom: 1080,
        };
        let rect = widget_rect(taskbar, None, 1.0, 0).unwrap();
        assert_eq!(rect.width(), 48);
        assert!(rect.bottom <= taskbar.bottom);
    }

    #[test]
    fn transient_empty_taskbar_and_invalid_dpi_do_not_panic() {
        assert!(widget_rect(Rect::default(), None, 1.0, 0).is_none());
        let taskbar = Rect {
            left: 0,
            top: 1000,
            right: 1920,
            bottom: 1040,
        };
        assert!(widget_rect(taskbar, None, f64::NAN, 0).is_none());
        assert!(widget_rect(taskbar, None, 0.0, 0).is_none());
    }
}
