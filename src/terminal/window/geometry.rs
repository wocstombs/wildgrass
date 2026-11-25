//! Screen geometry calculations and window boundary snapping.

use super::action::Direction;

/// Movement step per key press: 50 px at 96 DPI (matches `windows_helper.ps1`).
pub(super) const STEP: i32 = 50;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Rect {
    pub(super) x: i32,
    pub(super) y: i32,
    pub(super) width: i32,
    pub(super) height: i32,
}

impl Rect {
    fn right(self) -> i32 {
        self.x + self.width
    }

    fn bottom(self) -> i32 {
        self.y + self.height
    }

    fn contains(self, x: i32, y: i32) -> bool {
        x >= self.x && x < self.right() && y >= self.y && y < self.bottom()
    }

    pub(super) fn intersection(self, other: Self) -> Option<Self> {
        let (x, y) = (self.x.max(other.x), self.y.max(other.y));
        let (right, bottom) = (
            self.right().min(other.right()),
            self.bottom().min(other.bottom()),
        );
        (right > x && bottom > y).then_some(Self {
            x,
            y,
            width: right - x,
            height: bottom - y,
        })
    }

    pub(super) fn grown(self, by: Insets) -> Self {
        Self {
            x: self.x - by.left,
            y: self.y - by.top,
            width: self.width + by.left + by.right,
            height: self.height + by.top + by.bottom,
        }
    }

    pub(super) fn shrunk(self, by: Insets) -> Self {
        Self {
            x: self.x + by.left,
            y: self.y + by.top,
            width: self.width - by.left - by.right,
            height: self.height - by.top - by.bottom,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) struct Insets {
    pub(super) left: i32,
    pub(super) right: i32,
    pub(super) top: i32,
    pub(super) bottom: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Movement {
    pub(super) right: i32,
    pub(super) down: i32,
}

impl Movement {
    pub(super) const NONE: Self = Self { right: 0, down: 0 };
}

/// Monitor total bounds and usable work area (excluding panels/taskbar).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Screen {
    pub(super) bounds: Rect,
    pub(super) work: Rect,
}

/// Computes window movement in `direction` clamped to the monitor's work area.
pub(super) fn step_within(
    visible: Rect,
    direction: Direction,
    step: i32,
    screens: &[Screen],
) -> Movement {
    let horizontal = |right| Movement { right, down: 0 };
    let vertical = |down| Movement { right: 0, down };
    let (center_x, center_y) = (
        visible.x + visible.width / 2,
        visible.y + visible.height / 2,
    );
    let Some(here) = screens
        .iter()
        .find(|screen| screen.bounds.contains(center_x, center_y))
        .or(screens.first())
    else {
        return match direction {
            Direction::Left => horizontal(-step),
            Direction::Right => horizontal(step),
            Direction::Up => vertical(-step),
            Direction::Down => vertical(step),
        };
    };
    let elsewhere = |x, y| {
        screens
            .iter()
            .any(|screen| screen != here && screen.bounds.contains(x, y))
    };
    // How far the window may go when the edge is `room` pixels away
    let up_to = |room: i32| room.clamp(0, step);
    let work = here.work;
    match direction {
        Direction::Left => {
            let x = visible.x - step;
            let distance = if x < work.x && !elsewhere(x, center_y) {
                up_to(visible.x - work.x)
            } else {
                step
            };
            horizontal(-distance)
        }
        Direction::Right => {
            let right = visible.right() + step;
            let distance = if right > work.right() && !elsewhere(right - 1, center_y) {
                up_to(work.right() - visible.right())
            } else {
                step
            };
            horizontal(distance)
        }
        Direction::Up => {
            let y = visible.y - step;
            let distance = if y < work.y && !elsewhere(center_x, y) {
                up_to(visible.y - work.y)
            } else {
                step
            };
            vertical(-distance)
        }
        Direction::Down => {
            let bottom = visible.bottom() + step;
            let distance = if bottom > work.bottom() && !elsewhere(center_x, bottom - 1) {
                up_to(work.bottom() - visible.bottom())
            } else {
                step
            };
            vertical(distance)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rect(x: i32, y: i32, width: i32, height: i32) -> Rect {
        Rect {
            x,
            y,
            width,
            height,
        }
    }

    fn movement(right: i32, down: i32) -> Movement {
        Movement { right, down }
    }

    /// A 1920x1080 monitor at `x` with a 30-pixel panel along its top.
    fn monitor(x: i32) -> Screen {
        Screen {
            bounds: rect(x, 0, 1920, 1080),
            work: rect(x, 30, 1920, 1050),
        }
    }

    #[test]
    fn test_a_window_away_from_the_edges_moves_a_whole_step() {
        let screens = [monitor(0)];
        let window = rect(500, 500, 400, 300);
        assert_eq!(
            step_within(window, Direction::Left, 50, &screens),
            movement(-50, 0)
        );
        assert_eq!(
            step_within(window, Direction::Right, 50, &screens),
            movement(50, 0)
        );
        assert_eq!(
            step_within(window, Direction::Up, 50, &screens),
            movement(0, -50)
        );
        assert_eq!(
            step_within(window, Direction::Down, 50, &screens),
            movement(0, 50)
        );
    }

    #[test]
    fn test_a_window_stops_at_the_edge_of_the_free_area() {
        let screens = [monitor(0)];
        assert_eq!(
            step_within(rect(20, 500, 400, 300), Direction::Left, 50, &screens),
            movement(-20, 0)
        );
        assert_eq!(
            step_within(rect(1500, 500, 400, 300), Direction::Right, 50, &screens),
            movement(20, 0)
        );
        // The panel takes the top 30 pixels
        assert_eq!(
            step_within(rect(500, 40, 400, 300), Direction::Up, 50, &screens),
            movement(0, -10)
        );
        assert_eq!(
            step_within(rect(500, 760, 400, 300), Direction::Down, 50, &screens),
            movement(0, 20)
        );
    }

    #[test]
    fn test_a_window_past_the_edge_does_not_move_back() {
        let screens = [monitor(0)];
        assert_eq!(
            step_within(rect(-10, 500, 400, 300), Direction::Left, 50, &screens),
            movement(0, 0)
        );
        assert_eq!(
            step_within(rect(500, 10, 400, 300), Direction::Up, 50, &screens),
            movement(0, 0)
        );
    }

    #[test]
    fn test_a_window_crosses_onto_the_next_monitor() {
        let screens = [monitor(0), monitor(1920)];
        assert_eq!(
            step_within(rect(1500, 500, 400, 300), Direction::Right, 50, &screens),
            movement(50, 0)
        );
        assert_eq!(
            step_within(rect(1940, 500, 400, 300), Direction::Left, 50, &screens),
            movement(-50, 0)
        );
        // Nothing lies left of the first monitor
        assert_eq!(
            step_within(rect(20, 500, 400, 300), Direction::Left, 50, &screens),
            movement(-20, 0)
        );
    }

    #[test]
    fn test_without_monitors_a_window_moves_a_whole_step() {
        assert_eq!(
            step_within(rect(0, 0, 400, 300), Direction::Up, 50, &[]),
            movement(0, -50)
        );
    }

    #[test]
    fn test_rect_overlap_and_insets() {
        assert_eq!(
            rect(0, 0, 100, 100).intersection(rect(50, 20, 100, 100)),
            Some(rect(50, 20, 50, 80))
        );
        assert_eq!(rect(0, 0, 10, 10).intersection(rect(10, 0, 10, 10)), None);
        let insets = Insets {
            left: 1,
            right: 2,
            top: 3,
            bottom: 4,
        };
        assert_eq!(rect(10, 10, 100, 100).grown(insets), rect(9, 7, 103, 107));
        assert_eq!(rect(10, 10, 100, 100).shrunk(insets), rect(11, 13, 97, 93));
    }
}
