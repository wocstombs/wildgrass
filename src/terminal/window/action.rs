//! Window actions and z-order layer states.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Left,
    Right,
    Up,
    Down,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowAction {
    Move(Direction),
    /// Cycle the window's z-order: Normal -> Top -> Bottom -> Normal.
    CycleLayer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub(super) enum Layer {
    #[default]
    Normal,
    Top,
    Bottom,
}

impl Layer {
    pub(super) fn next(self) -> Self {
        match self {
            Self::Normal => Self::Top,
            Self::Top => Self::Bottom,
            Self::Bottom => Self::Normal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_layer_cycles_on_top_then_on_the_bottom_then_normal() {
        assert_eq!(Layer::default(), Layer::Normal);
        assert_eq!(Layer::Normal.next(), Layer::Top);
        assert_eq!(Layer::Top.next(), Layer::Bottom);
        assert_eq!(Layer::Bottom.next(), Layer::Normal);
    }
}
