//! Milestone 3 phase 0, spike (b): can a desktop app hear about sleep, resume
//! and network changes without a window of its own?
//!
//!   cargo run -p holonet-app --example power_spike --release
//!
//! Logs every notification with a timestamp, plus a clock check every 2 s that
//! shows whether the monotonic clock kept counting while the PC slept.

use std::ffi::c_void;
use std::time::{Duration, Instant, SystemTime};

use windows_sys::Win32::Foundation::HANDLE;
use windows_sys::Win32::NetworkManagement::IpHelper::{
    MIB_IPINTERFACE_ROW, MIB_NOTIFICATION_TYPE, NotifyIpInterfaceChange, NotifyNetworkConnectivityHintChange,
};
use windows_sys::Win32::Networking::WinSock::{AF_UNSPEC, NL_NETWORK_CONNECTIVITY_HINT};
use windows_sys::Win32::System::Power::{DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS, PowerRegisterSuspendResumeNotification};

const DEVICE_NOTIFY_CALLBACK: u32 = 2;

fn now() -> String {
    let t = SystemTime::now().duration_since(SystemTime::UNIX_EPOCH).unwrap();
    let s = t.as_secs() % 86_400;
    format!("{:02}:{:02}:{:02}.{:03} UTC", s / 3600, s / 60 % 60, s % 60, t.subsec_millis())
}

unsafe extern "system" fn on_power(_ctx: *const c_void, kind: u32, _setting: *const c_void) -> u32 {
    let name = match kind {
        4 => "PBT_APMSUSPEND (going to sleep)",
        7 => "PBT_APMRESUMESUSPEND (resumed by the user)",
        0x12 => "PBT_APMRESUMEAUTOMATIC (resumed)",
        0x0A => "PBT_APMPOWERSTATUSCHANGE",
        0x8013 => "PBT_POWERSETTINGCHANGE",
        _ => "other",
    };
    println!("{} power: {name} ({kind:#x})", now());
    0
}

unsafe extern "system" fn on_interface(
    _ctx: *const c_void,
    row: *const MIB_IPINTERFACE_ROW,
    kind: MIB_NOTIFICATION_TYPE,
) {
    let (index, family, connected) = if row.is_null() {
        (0, 0, false)
    } else {
        let r = unsafe { &*row };
        (r.InterfaceIndex, r.Family, r.Connected != 0)
    };
    let kind = match kind {
        0 => "parameter change",
        1 => "added",
        2 => "deleted",
        3 => "initial",
        _ => "other",
    };
    println!("{} interface: {kind}, index {index}, family {family}, connected {connected}", now());
}

unsafe extern "system" fn on_hint(_ctx: *const c_void, hint: NL_NETWORK_CONNECTIVITY_HINT) {
    println!(
        "{} connectivity hint: level {}, cost {}, approaching data limit {}, over data limit {}, roaming {}",
        now(),
        hint.ConnectivityLevel,
        hint.ConnectivityCost,
        hint.ApproachingDataLimit,
        hint.OverDataLimit,
        hint.Roaming
    );
}

fn main() {
    unsafe {
        let mut params = DEVICE_NOTIFY_SUBSCRIBE_PARAMETERS { Callback: Some(on_power), Context: std::ptr::null_mut() };
        let mut power: *mut c_void = std::ptr::null_mut();
        let rc =
            PowerRegisterSuspendResumeNotification(DEVICE_NOTIFY_CALLBACK, &mut params as *mut _ as HANDLE, &mut power);
        println!("{} PowerRegisterSuspendResumeNotification -> {rc}", now());

        let mut iface: HANDLE = std::ptr::null_mut();
        let rc = NotifyIpInterfaceChange(AF_UNSPEC, Some(on_interface), std::ptr::null(), 0, &mut iface);
        println!("{} NotifyIpInterfaceChange -> {rc}", now());

        let mut hint: HANDLE = std::ptr::null_mut();
        let rc = NotifyNetworkConnectivityHintChange(Some(on_hint), std::ptr::null(), 1, &mut hint);
        println!("{} NotifyNetworkConnectivityHintChange -> {rc}", now());
    }

    let (mut mono, mut wall) = (Instant::now(), SystemTime::now());
    loop {
        std::thread::sleep(Duration::from_secs(2));
        let m = mono.elapsed();
        let w = wall.elapsed().unwrap_or_default();
        if w > Duration::from_secs(5) || m > Duration::from_secs(5) {
            println!(
                "{} clock gap: wall {:.1} s, monotonic {:.1} s since the last tick",
                now(),
                w.as_secs_f64(),
                m.as_secs_f64()
            );
        }
        (mono, wall) = (Instant::now(), SystemTime::now());
    }
}
