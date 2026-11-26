//! Milestone 5 phase 0, spikes (c) and (d): this PC's side of the SFTP
//! screen. Lists drives with their type and label, lists a folder showing
//! which entries are hidden, and moves a file and a folder to the Recycle Bin
//! (`SHFileOperationW` with undo), checking they left the folder.
//!
//!   cargo run -p holonet-app --example m5_local --release -- <scratch folder>

#[cfg(windows)]
fn main() {
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::fs::MetadataExt;
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW,
    };

    const FILE_ATTRIBUTE_HIDDEN: u32 = 0x2;
    const FILE_ATTRIBUTE_SYSTEM: u32 = 0x4;
    let scratch = std::path::PathBuf::from(std::env::args().nth(1).expect("scratch folder"));

    // (d) Drives.
    let t = std::time::Instant::now();
    let mask = unsafe { GetLogicalDrives() };
    for i in 0..26u32 {
        if mask & (1 << i) == 0 {
            continue;
        }
        let root = format!("{}:\\", (b'A' + i as u8) as char);
        let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
        let kind = match unsafe { GetDriveTypeW(wide.as_ptr()) } {
            2 => "removable",
            3 => "fixed",
            4 => "network",
            5 => "CD",
            6 => "RAM disk",
            _ => "other",
        };
        let mut label = [0u16; 261];
        let ok = unsafe {
            GetVolumeInformationW(
                wide.as_ptr(),
                label.as_mut_ptr(),
                label.len() as u32,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                0,
            )
        } != 0;
        let end = label.iter().position(|c| *c == 0).unwrap_or(0);
        let label = if ok { String::from_utf16_lossy(&label[..end]) } else { "(no label or not ready)".into() };
        println!("drive {root} {kind} {label:?}");
    }
    println!("drives listed in {:.1} ms", t.elapsed().as_secs_f64() * 1000.0);

    // (d) Hidden entries in the profile folder (names only, not printed in
    // full: counts and two examples).
    let profile = std::env::var_os("USERPROFILE").map(std::path::PathBuf::from).expect("USERPROFILE");
    let mut hidden = Vec::new();
    let mut shown = 0;
    let t = std::time::Instant::now();
    for e in std::fs::read_dir(&profile).expect("read profile").flatten() {
        let attrs = e.metadata().map(|m| m.file_attributes()).unwrap_or(0);
        let dot = e.file_name().to_string_lossy().starts_with('.');
        if attrs & (FILE_ATTRIBUTE_HIDDEN | FILE_ATTRIBUTE_SYSTEM) != 0 {
            hidden.push((e.file_name().to_string_lossy().into_owned(), dot));
        } else {
            shown += 1;
        }
    }
    let dot_not_hidden = std::fs::read_dir(&profile)
        .expect("read profile")
        .flatten()
        .filter(|e| e.file_name().to_string_lossy().starts_with('.'))
        .filter(|e| e.metadata().map(|m| m.file_attributes() & FILE_ATTRIBUTE_HIDDEN == 0).unwrap_or(false))
        .count();
    println!(
        "profile folder: {shown} shown, {} hidden or system (e.g. {:?}), {dot_not_hidden} dot-names without the hidden attribute; {:.1} ms",
        hidden.len(),
        hidden.iter().take(2).map(|(n, _)| n.as_str()).collect::<Vec<_>>(),
        t.elapsed().as_secs_f64() * 1000.0
    );

    // (c) Recycle Bin: a file and a folder with a file inside.
    let file = scratch.join("holonet-m5-recycle-test.txt");
    let folder = scratch.join("holonet-m5-recycle-folder");
    std::fs::write(&file, "recycle me").unwrap();
    std::fs::create_dir_all(&folder).unwrap();
    std::fs::write(folder.join("inside.txt"), "inside").unwrap();
    // pFrom: paths separated by NULs, ending with two NULs.
    let mut from: Vec<u16> = Vec::new();
    for p in [&file, &folder] {
        from.extend(p.as_os_str().encode_wide());
        from.push(0);
    }
    from.push(0);
    let mut op = SHFILEOPSTRUCTW {
        hwnd: std::ptr::null_mut(),
        wFunc: FO_DELETE,
        pFrom: from.as_ptr(),
        pTo: std::ptr::null(),
        fFlags: (FOF_ALLOWUNDO | FOF_NOCONFIRMATION | FOF_SILENT | FOF_NOERRORUI) as u16,
        fAnyOperationsAborted: 0,
        hNameMappings: std::ptr::null_mut(),
        lpszProgressTitle: std::ptr::null(),
    };
    let t = std::time::Instant::now();
    let result = unsafe { SHFileOperationW(&mut op) };
    println!(
        "SHFileOperationW delete with undo: result {result}, aborted {}, {:.0} ms; file still there: {}, folder still there: {}",
        op.fAnyOperationsAborted,
        t.elapsed().as_secs_f64() * 1000.0,
        file.exists(),
        folder.exists()
    );
}

#[cfg(not(windows))]
fn main() {}
