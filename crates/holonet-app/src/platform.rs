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

/// Windows' accent color as `0xRRGGBB`: the lighter shade Windows itself
/// uses on dark backgrounds (AccentPalette's second entry), so it reads on
/// Holonet's dark surfaces.
#[cfg(windows)]
pub fn windows_accent() -> Option<u32> {
    use windows_sys::Win32::System::Registry::{HKEY_CURRENT_USER, RRF_RT_REG_BINARY, RegGetValueW};
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
    let (key, value) = (wide(r"Software\Microsoft\Windows\CurrentVersion\Explorer\Accent"), wide("AccentPalette"));
    let mut palette = [0u8; 32];
    let mut len = palette.len() as u32;
    // SAFETY: reads at most `len` bytes into `palette`; strings are NUL-terminated.
    let status = unsafe {
        RegGetValueW(
            HKEY_CURRENT_USER,
            key.as_ptr(),
            value.as_ptr(),
            RRF_RT_REG_BINARY,
            std::ptr::null_mut(),
            palette.as_mut_ptr().cast(),
            &mut len,
        )
    };
    if status != 0 || len < 8 {
        return None;
    }
    // Entries are R, G, B, A; the second is "light 2".
    let [r, g, b] = [palette[4], palette[5], palette[6]];
    Some(u32::from(r) << 16 | u32::from(g) << 8 | u32::from(b))
}

#[cfg(not(windows))]
pub fn windows_accent() -> Option<u32> {
    None
}

/// "Use Windows accent color": switch the accent and redraw.
pub fn apply_accent(on: bool, cx: &mut gpui::App) {
    crate::theme::set_accent(if on { windows_accent() } else { None });
    cx.refresh_windows();
}

/// Something about this PC changed that can break connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    /// The PC is going to sleep.
    Suspend,
    /// The PC woke up.
    Resume,
    /// A network interface came, went or changed, or connectivity changed.
    NetworkChanged,
    /// Windows locked (Win L, or the screen locking by itself).
    SessionLocked,
    /// Windows unlocked.
    SessionUnlocked,
}

/// Report sleep, wake, network changes, and Windows locking and unlocking,
/// for the rest of the process's life. The power and network callbacks need
/// no window (milestone 3 phase 0, spike b); each sends on `tx`, which the
/// registrations keep alive. Lock and unlock come to a message-only window
/// on a thread of its own (milestone 4 phase 0, spike b).
#[cfg(windows)]
pub fn watch_system(tx: tokio::sync::mpsc::UnboundedSender<SystemEvent>) {
    use std::ffi::c_void;
    use windows_sys::Win32::Foundation::HANDLE;
    use windows_sys::Win32::NetworkManagement::IpHelper::{
        MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE, NotifyIpInterfaceChange, NotifyNetworkConnectivityHintChange,
    };
    use windows_sys::Win32::Networking::WinSock::{AF_UNSPEC, NL_NETWORK_CONNECTIVITY_HINT};
    use windows_sys::Win32::System::Power::{
        DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification,
    };

    type Tx = tokio::sync::mpsc::UnboundedSender<SystemEvent>;
    const DEVICE_NOTIFY_CALLBACK: u32 = 2;
    const PBT_APMSUSPEND: u32 = 0x4;
    const PBT_APMRESUMESUSPEND: u32 = 0x7;
    const PBT_APMRESUMEAUTOMATIC: u32 = 0x12;

    unsafe extern "system" fn on_power(ctx: *const c_void, kind: u32, _: *const c_void) -> u32 {
        // SAFETY: `ctx` is the leaked `Tx` below, alive for the process.
        let tx = unsafe { &*(ctx as *const Tx) };
        let event = match kind {
            PBT_APMSUSPEND => Some(SystemEvent::Suspend),
            PBT_APMRESUMESUSPEND | PBT_APMRESUMEAUTOMATIC => Some(SystemEvent::Resume),
            _ => None,
        };
        if let Some(e) = event {
            let _ = tx.send(e);
        }
        0
    }

    unsafe extern "system" fn on_interface(
        ctx: *const c_void,
        _: *const MIB_IPINTERFACE_ROW,
        kind: MIB_NOTIFICATION_TYPE,
    ) {
        // 3 is the initial notification, sent only when asked for.
        if kind != 3 {
            // SAFETY: as above.
            let _ = unsafe { &*(ctx as *const Tx) }.send(SystemEvent::NetworkChanged);
        }
    }

    unsafe extern "system" fn on_hint(ctx: *const c_void, _: NL_NETWORK_CONNECTIVITY_HINT) {
        // SAFETY: as above.
        let _ = unsafe { &*(ctx as *const Tx) }.send(SystemEvent::NetworkChanged);
    }

    // Leaked on purpose: the registrations last as long as the process.
    let ctx = Box::into_raw(Box::new(tx)) as *mut c_void;
    let params = Box::into_raw(Box::new(DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(on_power), Context: ctx }));
    // SAFETY: the callbacks match the documented signatures and `ctx` and
    // `params` outlive every registration.
    unsafe {
        let mut power: *mut c_void = std::ptr::null_mut();
        PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, params as HANDLE, &mut power);
        let mut iface: HANDLE = std::ptr::null_mut();
        NotifyIpInterfaceChange(AF_UNSPEC, Some(on_interface), ctx, 0, &mut iface);
        let mut hint: HANDLE = std::ptr::null_mut();
        // Not asking for the initial hint (0): only changes matter.
        NotifyNetworkConnectivityHintChange(Some(on_hint), ctx, 0, &mut hint);
    }
    // SAFETY: `ctx` is the leaked `Tx`, alive for the process.
    watch_session(unsafe { &*(ctx as *const Tx) }.clone());
}

/// `WM_WTSSESSION_CHANGE` to a message-only window, on its own thread.
#[cfg(windows)]
fn watch_session(tx: tokio::sync::mpsc::UnboundedSender<SystemEvent>) {
    use std::sync::OnceLock;
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::System::RemoteDesktop::{NOTIFY_FOR_THIS_SESSION, WTSRegisterSessionNotification};
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        CreateWindowExW, DefWindowProcW, DispatchMessageW, GetMessageW, HWND_MESSAGE, MSG, RegisterClassW,
        TranslateMessage, WNDCLASSW,
    };

    const WM_WTSSESSION_CHANGE: u32 = 0x02B1;
    const WTS_SESSION_LOCK: usize = 0x7;
    const WTS_SESSION_UNLOCK: usize = 0x8;
    static TX: OnceLock<tokio::sync::mpsc::UnboundedSender<SystemEvent>> = OnceLock::new();
    if TX.set(tx).is_err() {
        return; // already watching
    }

    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_WTSSESSION_CHANGE {
            let event = match wparam {
                WTS_SESSION_LOCK => Some(SystemEvent::SessionLocked),
                WTS_SESSION_UNLOCK => Some(SystemEvent::SessionUnlocked),
                _ => None,
            };
            if let (Some(e), Some(tx)) = (event, TX.get()) {
                let _ = tx.send(e);
            }
        }
        // SAFETY: the arguments are the ones Windows passed in.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    std::thread::Builder::new()
        .name("session-watch".into())
        .spawn(|| {
            let class: Vec<u16> = "HolonetSessionWatch".encode_utf16().chain(Some(0)).collect();
            // SAFETY: a plain window class and message-only window, used only
            // on this thread, which runs its message loop for the process's life.
            unsafe {
                let instance = GetModuleHandleW(std::ptr::null());
                let wc = WNDCLASSW {
                    lpfnWndProc: Some(proc),
                    hInstance: instance,
                    lpszClassName: class.as_ptr(),
                    ..std::mem::zeroed()
                };
                RegisterClassW(&wc);
                let hwnd = CreateWindowExW(
                    0,
                    class.as_ptr(),
                    class.as_ptr(),
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
                if hwnd.is_null() || WTSRegisterSessionNotification(hwnd, NOTIFY_FOR_THIS_SESSION) == 0 {
                    return;
                }
                let mut msg: MSG = std::mem::zeroed();
                while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
        })
        .ok();
}

#[cfg(not(windows))]
pub fn watch_system(_tx: tokio::sync::mpsc::UnboundedSender<SystemEvent>) {}
