//! Small Windows queries that GPUI does not expose.

/// Whether Windows asks apps to avoid animation (Settings, Accessibility,
/// Visual effects, "Animation effects" off). Blinking cursors stop then
/// (TerminalText.md).
#[cfg(windows)]
pub fn reduced_motion() -> bool {
    use windows_sys::Win32::UI::WindowsAndMessaging::{SPI_GETCLIENTAREAANIMATION, SystemParametersInfoW};
    let mut animate: i32 = 1;
    // SAFETY: SPI_GETCLIENTAREAANIMATION writes one BOOL to the pointer given.
    let ok = unsafe { SystemParametersInfoW(SPI_GETCLIENTAREAANIMATION, 0, (&mut animate as *mut i32).cast(), 0) };
    ok != 0 && animate == 0
}

#[cfg(not(windows))]
pub fn reduced_motion() -> bool {
    false
}
