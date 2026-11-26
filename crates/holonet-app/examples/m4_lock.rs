//! Milestone 4 phase 0, spike (b): hear Windows lock and unlock without GPUI.
//! A thread of its own creates two windows, a message-only window
//! (`HWND_MESSAGE`) and a hidden ordinary window, registers both with
//! `WTSRegisterSessionNotification`, and prints which one gets
//! `WM_WTSSESSION_CHANGE`. Runs until `seconds` (default 90) have passed.
//!
//!   cargo run -p holonet-app --example m4_lock --release -- [seconds]

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::RemoteDesktop::{
        NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification, WTSUnRegisterSessionNotification,
    };
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
    static START: std::sync::OnceLock<Instant> = std::sync::OnceLock::new();

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_WTSSESSION_CHANGE {
            let what = match wparam as u32 {
                0x7 => "WTS_SESSION_LOCK",
                0x8 => "WTS_SESSION_UNLOCK",
                0x5 => "WTS_SESSION_LOGON",
                0x6 => "WTS_SESSION_LOGOFF",
                0x1 => "WTS_CONSOLE_CONNECT",
                0x2 => "WTS_CONSOLE_DISCONNECT",
                _ => "other",
            };
            // GWLP_USERDATA holds 1 for the message-only window, 2 for the hidden one.
            let which = match unsafe { GetWindowLongPtrW(hwnd, GWLP_USERDATA) } {
                1 => "message-only",
                _ => "hidden window",
            };
            let t = START.get().map_or(0.0, |s| s.elapsed().as_secs_f64());
            println!("{t:>7.1} s  {which:<13} {what} (0x{:x})", wparam);
        }
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    START.get_or_init(Instant::now);
    let seconds: u64 = std::env::args().nth(1).and_then(|s| s.parse().ok()).unwrap_or(90);
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let class = wide("HolonetLockSpike");
    unsafe {
        let instance = GetModuleHandleW(std::ptr::null());
        let wc = WNDCLASSW {
            lpfnWndProc: Some(proc),
            hInstance: instance,
            lpszClassName: class.as_ptr(),
            ..std::mem::zeroed()
        };
        RegisterClassW(&wc);
        let title = wide("");
        let message_only = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            0,
            0,
            0,
            0,
            0,
            HWND_MESSAGE,
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        let hidden = CreateWindowExW(
            0,
            class.as_ptr(),
            title.as_ptr(),
            WS_OVERLAPPED,
            0,
            0,
            0,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        SetWindowLongPtrW(message_only, GWLP_USERDATA, 1);
        SetWindowLongPtrW(hidden, GWLP_USERDATA, 2);
        let a = WTSRegisterSessionNotification(message_only, NOTIFY_FOR_THIS_SESSION);
        let b = WTSRegisterSessionNotification(hidden, NOTIFY_FOR_THIS_SESSION);
        println!("registered: message-only {}, hidden window {}", a != 0, b != 0);
        println!("lock the PC (Win L), unlock it, and wait; stopping after {seconds} s");
        let deadline = Instant::now() + Duration::from_secs(seconds);
        SetTimer(std::ptr::null_mut(), 0, 500, None);
        let mut msg: MSG = std::mem::zeroed();
        while Instant::now() < deadline && GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
        WTSUnRegisterSessionNotification(message_only);
        WTSUnRegisterSessionNotification(hidden);
        DestroyWindow(message_only);
        DestroyWindow(hidden);
        println!("done");
    }
}

#[cfg(not(windows))]
fn main() {}
