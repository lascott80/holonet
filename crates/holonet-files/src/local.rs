//! This PC's files: drives, folders, and deleting to the Recycle Bin
//! (`decisions.md`). Hidden means the hidden or system attribute, Windows'
//! own meaning; a leading dot does not hide a file here (milestone 5 phase
//! 0: the profile folder has many dot-folders without the attribute).

use std::path::{Path, PathBuf};

use crate::{Entry, FileError, Kind, Result};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum DriveKind {
    Fixed,
    Removable,
    Network,
    Optical,
    Other,
}

/// A drive for the "This PC" level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Drive {
    /// `C:\`
    pub root: PathBuf,
    /// The volume label; empty when there is none or the drive is not ready.
    pub label: String,
    pub kind: DriveKind,
}

impl Drive {
    /// "Local Disk (C:)" style, as Explorer shows it.
    pub fn display(&self) -> String {
        let letter = self.root.to_string_lossy().trim_end_matches('\\').to_string();
        if self.label.is_empty() { letter } else { format!("{} ({letter})", self.label) }
    }
}

/// The Windows profile folder: where the local pane starts by default.
pub fn profile() -> PathBuf {
    std::env::var_os("USERPROFILE").map(PathBuf::from).unwrap_or_else(|| PathBuf::from("C:\\"))
}

#[cfg(windows)]
pub fn drives() -> Vec<Drive> {
    use windows_sys::Win32::Storage::FileSystem::{GetDriveTypeW, GetLogicalDrives, GetVolumeInformationW};
    // SAFETY: no arguments; returns a bit mask.
    let mask = unsafe { GetLogicalDrives() };
    (0..26u32)
        .filter(|i| mask & (1 << i) != 0)
        .map(|i| {
            let root = format!("{}:\\", (b'A' + i as u8) as char);
            let wide: Vec<u16> = root.encode_utf16().chain(Some(0)).collect();
            // SAFETY: a NUL-terminated root path.
            let kind = match unsafe { GetDriveTypeW(wide.as_ptr()) } {
                2 => DriveKind::Removable,
                3 => DriveKind::Fixed,
                4 => DriveKind::Network,
                5 => DriveKind::Optical,
                _ => DriveKind::Other,
            };
            let mut label = [0u16; 261];
            // SAFETY: the buffer and its length match; the other outputs are
            // optional and null.
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
            Drive {
                root: PathBuf::from(root),
                label: if ok { String::from_utf16_lossy(&label[..end]) } else { String::new() },
                kind,
            }
        })
        .collect()
}

#[cfg(not(windows))]
pub fn drives() -> Vec<Drive> {
    vec![Drive { root: PathBuf::from("/"), label: String::new(), kind: DriveKind::Fixed }]
}

#[cfg(windows)]
fn hidden(meta: &std::fs::Metadata) -> bool {
    use std::os::windows::fs::MetadataExt;
    const HIDDEN: u32 = 0x2;
    const SYSTEM: u32 = 0x4;
    meta.file_attributes() & (HIDDEN | SYSTEM) != 0
}

#[cfg(not(windows))]
fn hidden(_: &std::fs::Metadata) -> bool {
    false
}

fn modified(meta: &std::fs::Metadata) -> Option<i64> {
    let t = meta.modified().ok()?;
    let d = t.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(d.as_secs() as i64)
}

/// One entry for `path`, not following a link.
pub fn entry(path: &Path) -> Result<Entry> {
    let meta = std::fs::symlink_metadata(path)?;
    let name = path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    Ok(make_entry(name, path, &meta))
}

fn make_entry(name: String, path: &Path, meta: &std::fs::Metadata) -> Entry {
    let kind = if meta.file_type().is_symlink() {
        match std::fs::metadata(path) {
            Ok(target) => Kind::Link { folder: target.is_dir(), broken: false },
            Err(_) => Kind::Link { folder: false, broken: true },
        }
    } else if meta.is_dir() {
        Kind::Folder
    } else if meta.is_file() {
        Kind::File
    } else {
        Kind::Other
    };
    Entry {
        raw_name: name.as_bytes().to_vec(),
        name,
        kind,
        size: if kind.opens() { 0 } else { meta.len() },
        modified: modified(meta),
        mode: None,
        uid: None,
        gid: None,
        hidden: hidden(meta),
    }
}

/// A folder's entries, unsorted. Entries that cannot be read are skipped.
pub fn list(dir: &Path) -> Result<Vec<Entry>> {
    let mut out = Vec::new();
    for item in std::fs::read_dir(dir)? {
        let Ok(item) = item else { continue };
        let Ok(meta) = std::fs::symlink_metadata(item.path()) else { continue };
        out.push(make_entry(item.file_name().to_string_lossy().into_owned(), &item.path(), &meta));
    }
    Ok(out)
}

/// Rename (or move) on this PC. Refuses to replace something.
pub fn rename(from: &Path, to: &Path) -> Result<()> {
    if std::fs::symlink_metadata(to).is_ok() {
        return Err(FileError::AlreadyExists);
    }
    Ok(std::fs::rename(from, to)?)
}

pub fn create_folder(path: &Path) -> Result<()> {
    Ok(std::fs::create_dir(path)?)
}

/// Move files and folders to the Recycle Bin, without Windows' own
/// confirmation or progress (Holonet asks first).
#[cfg(windows)]
pub fn recycle(paths: &[PathBuf]) -> Result<()> {
    use std::os::windows::ffi::OsStrExt;
    use windows_sys::Win32::UI::Shell::{
        FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, FOF_NOERRORUI, FOF_SILENT, SHFILEOPSTRUCTW, SHFileOperationW,
    };
    if paths.is_empty() {
        return Ok(());
    }
    for p in paths {
        if std::fs::symlink_metadata(p).is_err() {
            return Err(FileError::NotFound);
        }
    }
    // pFrom: absolute paths separated by NULs, ending with two NULs.
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        let absolute = std::path::absolute(p)?;
        from.extend(absolute.as_os_str().encode_wide());
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
    // SAFETY: `from` is double-NUL terminated and outlives the call.
    let result = unsafe { SHFileOperationW(&mut op) };
    if result != 0 {
        return Err(FileError::Other(format!("Windows could not move these to the Recycle Bin (error {result:#x}).")));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err(FileError::Other("Moving to the Recycle Bin was cancelled.".into()));
    }
    Ok(())
}

#[cfg(not(windows))]
pub fn recycle(_paths: &[PathBuf]) -> Result<()> {
    Err(FileError::Other("The Recycle Bin is only on Windows.".into()))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("holonet-files-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[test]
    fn lists_renames_and_creates_folders() {
        let dir = temp("local");
        std::fs::write(dir.join("a.txt"), "hello").unwrap();
        create_folder(&dir.join("sub")).unwrap();
        assert_eq!(create_folder(&dir.join("sub")), Err(FileError::AlreadyExists));
        let mut list = list(&dir).unwrap();
        crate::sort(&mut list);
        let rows: Vec<(String, Kind, u64)> = list.iter().map(|e| (e.name.clone(), e.kind, e.size)).collect();
        assert_eq!(rows, [("sub".into(), Kind::Folder, 0), ("a.txt".into(), Kind::File, 5)]);
        assert!(list[1].modified.is_some() && list[1].mode.is_none());
        std::fs::write(dir.join("b.txt"), "b").unwrap();
        assert_eq!(rename(&dir.join("a.txt"), &dir.join("b.txt")), Err(FileError::AlreadyExists));
        rename(&dir.join("a.txt"), &dir.join("c.txt")).unwrap();
        assert!(dir.join("c.txt").exists() && !dir.join("a.txt").exists());
        assert_eq!(entry(&dir.join("missing")).unwrap_err(), FileError::NotFound);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(windows)]
    #[test]
    fn hidden_follows_the_attribute_not_the_dot() {
        use std::os::windows::ffi::OsStrExt;
        use windows_sys::Win32::Storage::FileSystem::{FILE_ATTRIBUTE_HIDDEN, SetFileAttributesW};
        let dir = temp("hidden");
        std::fs::write(dir.join(".dotted"), "").unwrap();
        std::fs::write(dir.join("marked"), "").unwrap();
        let wide: Vec<u16> = dir.join("marked").as_os_str().encode_wide().chain(Some(0)).collect();
        // SAFETY: a NUL-terminated path.
        assert!(unsafe { SetFileAttributesW(wide.as_ptr(), FILE_ATTRIBUTE_HIDDEN) } != 0);
        let list = list(&dir).unwrap();
        let hidden: Vec<(&str, bool)> = {
            let mut v: Vec<(&str, bool)> = list.iter().map(|e| (e.name.as_str(), e.hidden)).collect();
            v.sort();
            v
        };
        assert_eq!(hidden, [(".dotted", false), ("marked", true)]);
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn drives_include_the_system_drive() {
        let drives = drives();
        assert!(!drives.is_empty());
        #[cfg(windows)]
        assert!(drives.iter().any(|d| d.root == Path::new("C:\\") && d.kind == DriveKind::Fixed));
    }

    /// Puts two items in this PC's Recycle Bin, so it runs only on request:
    /// `$env:HOLONET_TEST_RECYCLE = "1"`. It leaves `holonet-recycle-test.txt` and `holonet-recycle-folder` there.
    #[cfg(windows)]
    #[test]
    fn recycle_moves_files_and_folders_to_the_recycle_bin() {
        if std::env::var("HOLONET_TEST_RECYCLE").as_deref() != Ok("1") {
            eprintln!("skipped: set HOLONET_TEST_RECYCLE=1 (puts two test items in the Recycle Bin)");
            return;
        }
        let dir = temp("recycle");
        let file = dir.join("holonet-recycle-test.txt");
        let folder = dir.join("holonet-recycle-folder");
        std::fs::write(&file, "x").unwrap();
        std::fs::create_dir_all(&folder).unwrap();
        std::fs::write(folder.join("inside.txt"), "y").unwrap();
        recycle(&[file.clone(), folder.clone()]).unwrap();
        assert!(!file.exists() && !folder.exists());
        assert_eq!(recycle(std::slice::from_ref(&file)), Err(FileError::NotFound));
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
