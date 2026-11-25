//! Windows Terminal detection for profile management and window resizing.

use std::env;

/// Checks whether the running session is hosted inside Windows Terminal.
/// Inspects `WT_SESSION` (inherited across processes and WSL) or checks console window hierarchy.
pub(super) fn in_windows_terminal() -> bool {
    env::var_os("WT_SESSION").is_some() || console_owned_by_windows_terminal()
}

/// Checks if the Win32 console window is hosted inside a Windows Terminal window.
/// Handles cases where a console was spawned externally and attached to Windows Terminal.
#[cfg(windows)]
#[expect(
    unsafe_code,
    reason = "the console's window and the window that owns it are only reachable through \
              windows-sys's unsafe calls"
)]
pub(super) fn console_owned_by_windows_terminal() -> bool {
    use windows_sys::Win32::System::Console::GetConsoleWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{GetAncestor, GetClassNameW, GA_ROOTOWNER};

    // SAFETY: Win32 calls on window handles; returns null or fails safely on invalid handles
    unsafe {
        let console = GetConsoleWindow();
        if console.is_null() {
            return false;
        }
        let owner = GetAncestor(console, GA_ROOTOWNER);
        if owner.is_null() {
            return false;
        }
        let mut class = [0u16; 64];
        let length = GetClassNameW(owner, class.as_mut_ptr(), class.len() as i32);
        length > 0
            && String::from_utf16_lossy(&class[..length as usize])
                == "CASCADIA_HOSTING_WINDOW_CLASS"
    }
}

/// Returns false on non-Windows platforms or under WSL.
#[cfg(not(windows))]
pub(super) fn console_owned_by_windows_terminal() -> bool {
    false
}
