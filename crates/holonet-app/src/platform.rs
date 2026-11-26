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

/// Something about this PC changed that can break connections.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SystemEvent {
    /// The PC is going to sleep.
    Suspend,
    /// The PC woke up.
    Resume,
    /// A network interface came, went or changed, or connectivity changed.
    NetworkChanged,
}

/// Report sleep, wake and network changes for the rest of the process's life.
/// The callbacks need no window (milestone 3 phase 0, spike b); each sends on
/// `tx`, which the registrations keep alive.
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
}

#[cfg(not(windows))]
pub fn watch_system(_tx: tokio::sync::mpsc::UnboundedSender<SystemEvent>) {}
