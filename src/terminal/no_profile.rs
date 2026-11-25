//! Non-Windows stubs for terminal background color and profile management.

pub fn set_background() {}

pub fn reset_background() {}

pub fn reopen_in_profile(_args: &[String]) -> bool {
    false
}
