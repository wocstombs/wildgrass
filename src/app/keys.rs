//! Shared keyboard navigation helpers across views.

use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

/// Checks if `key` matches Ctrl plus `letter` (case-insensitive).
pub(super) fn is_ctrl(key: KeyEvent, letter: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL)
        && matches!(key.code, KeyCode::Char(c) if c.eq_ignore_ascii_case(&letter))
}

/// Moves list selection up or down via w/s or arrow keys within `[0, length - 1]`.
pub(super) fn navigate(cursor: &mut usize, length: usize, code: KeyCode) {
    let last = length.saturating_sub(1);
    *cursor = match code {
        KeyCode::Char('s' | 'S') | KeyCode::Down => (*cursor + 1).min(last),
        KeyCode::Char('w' | 'W') | KeyCode::Up => cursor.saturating_sub(1),
        _ => return,
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_navigate_clamps_to_list_bounds() {
        let mut cursor = 0;
        let mut after = |code| {
            navigate(&mut cursor, 3, code);
            cursor
        };
        assert_eq!(after(KeyCode::Char('w')), 0);
        assert_eq!(after(KeyCode::Down), 1);
        assert_eq!(after(KeyCode::Char('s')), 2);
        assert_eq!(after(KeyCode::Char('W')), 1);
        assert_eq!(after(KeyCode::Char('S')), 2);
        assert_eq!(after(KeyCode::Down), 2);
        assert_eq!(after(KeyCode::Up), 1);
        assert_eq!(after(KeyCode::Char('x')), 1);
    }
}
