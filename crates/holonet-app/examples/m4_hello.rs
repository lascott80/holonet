//! Milestone 4 phase 0, spike (a): can Windows Hello protect the vault key?
//!
//! Opens an ordinary window (standing in for Holonet's), then on a worker
//! thread creates a Hello key credential named `holonet-phase0-spike`, signs
//! the same challenge twice, and compares the signatures. About 0.8 s into
//! each prompt it reports whether the Hello dialog ("Credential Dialog Xaml
//! Host") is the foreground window. With `--fix` it brings the dialog forward
//! when it is not (the KeePassXC and Chromium workaround). The credential is
//! deleted at the end.
//!
//!   cargo run -p holonet-app --example m4_hello --release -- [--fix]

#[cfg(windows)]
fn main() {
    use std::time::{Duration, Instant};
    use windows::Security::Credentials::{KeyCredentialCreationOption, KeyCredentialManager, KeyCredentialStatus};
    use windows::Security::Cryptography::CryptographicBuffer;
    use windows::core::{Array, HSTRING};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, WPARAM};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::WindowsAndMessaging::*;

    let fix = std::env::args().any(|a| a == "--fix");
    let wide = |s: &str| s.encode_utf16().chain(Some(0)).collect::<Vec<u16>>();

    // Report on the Hello dialog shortly after a prompt starts.
    let watch = move |label: &'static str| {
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(800));
            let class = "Credential Dialog Xaml Host".encode_utf16().chain(Some(0)).collect::<Vec<u16>>();
            unsafe {
                let dialog = FindWindowW(class.as_ptr(), std::ptr::null());
                if dialog.is_null() {
                    println!("  [{label}] no Hello dialog window found after 0.8 s");
                    return;
                }
                let front = GetForegroundWindow() == dialog;
                println!("  [{label}] Hello dialog found; in front: {front}");
                if !front && fix {
                    let ok = SetForegroundWindow(dialog) != 0;
                    println!(
                        "  [{label}] SetForegroundWindow: {ok}, now in front: {}",
                        GetForegroundWindow() == dialog
                    );
                }
            }
        });
    };

    std::thread::spawn(move || {
        let run = || -> windows::core::Result<()> {
            let supported = KeyCredentialManager::IsSupportedAsync()?.join()?;
            println!("Hello supported: {supported}");
            if !supported {
                return Ok(());
            }
            let name = HSTRING::from("holonet-phase0-spike");
            let t = Instant::now();
            watch("create");
            let created =
                KeyCredentialManager::RequestCreateAsync(&name, KeyCredentialCreationOption::ReplaceExisting)?
                    .join()?;
            println!("create: {:?} after {:.1} s", created.Status()?, t.elapsed().as_secs_f64());
            if created.Status()? != KeyCredentialStatus::Success {
                return Ok(());
            }
            let credential = created.Credential()?;
            let challenge: Vec<u8> = (0..32u8).map(|i| i.wrapping_mul(37).wrapping_add(11)).collect();
            let mut signatures = Vec::new();
            for label in ["sign 1", "sign 2"] {
                let t = Instant::now();
                watch(label);
                let data = CryptographicBuffer::CreateFromByteArray(&challenge)?;
                let result = credential.RequestSignAsync(&data)?.join()?;
                println!("{label}: {:?} after {:.1} s", result.Status()?, t.elapsed().as_secs_f64());
                if result.Status()? != KeyCredentialStatus::Success {
                    break;
                }
                let mut bytes = Array::<u8>::new();
                CryptographicBuffer::CopyToByteArray(&result.Result()?, &mut bytes)?;
                signatures.push(bytes.to_vec());
            }
            if let [a, b] = signatures.as_slice() {
                println!(
                    "signature length {} bytes; the two signatures are {}",
                    a.len(),
                    if a == b { "IDENTICAL" } else { "DIFFERENT" }
                );
            }
            KeyCredentialManager::DeleteAsync(&name)?.join()?;
            println!("spike credential deleted");
            Ok(())
        };
        if let Err(e) = run() {
            println!("error: {e}");
        }
        std::process::exit(0);
    });

    // The stand-in app window, in front, pumping messages until the worker exits.
    unsafe extern "system" fn proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }
    let class = wide("HolonetHelloSpike");
    let title = wide("Holonet Hello spike (stand-in app window)");
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
            title.as_ptr(),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            200,
            200,
            900,
            600,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            instance,
            std::ptr::null(),
        );
        SetForegroundWindow(hwnd);
        let mut msg: MSG = std::mem::zeroed();
        while GetMessageW(&mut msg, std::ptr::null_mut(), 0, 0) > 0 {
            TranslateMessage(&msg);
            DispatchMessageW(&msg);
        }
    }
}

#[cfg(not(windows))]
fn main() {}
