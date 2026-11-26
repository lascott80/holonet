//! Puts the app icon and version details into holonet.exe (milestone 7
//! phase 9): writes a resource script, compiles it with the Windows SDK's
//! `rc.exe`, and links the result. The icon is resource 1, which GPUI loads
//! for the window. Without the SDK the exe still builds, with a warning.

use std::path::{Path, PathBuf};
use std::process::Command;

fn main() {
    println!("cargo:rerun-if-changed=build.rs");
    println!("cargo:rerun-if-changed=assets/app-icon/holonet.ico");
    println!("cargo:rerun-if-env-changed=RC");
    println!("cargo:rerun-if-env-changed=HOLONET_REQUIRE_RC");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows")
        || std::env::var("CARGO_CFG_TARGET_ENV").as_deref() != Ok("msvc")
    {
        return;
    }
    let Some(rc) = find_rc() else {
        // Release builds (release.yml) must not ship without the icon.
        assert!(std::env::var_os("HOLONET_REQUIRE_RC").is_none(), "rc.exe not found and HOLONET_REQUIRE_RC is set");
        println!("cargo:warning=rc.exe not found: holonet.exe is built without its icon and version details");
        return;
    };
    let manifest = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let icon = manifest.join("assets/app-icon/holonet.ico");
    let script = out.join("holonet.rc");
    let res = out.join("holonet.res");
    std::fs::write(&script, resource_script(&icon)).expect("write holonet.rc");
    let status = Command::new(&rc)
        .args(["/nologo", "/fo"])
        .arg(&res)
        .arg(&script)
        .status()
        .unwrap_or_else(|e| panic!("could not run {}: {e}", rc.display()));
    assert!(status.success(), "rc.exe failed on {}", script.display());
    println!("cargo:rustc-link-arg-bins={}", res.display());
}

/// `RC`, or the newest Windows 10/11 SDK's rc.exe for this PC's processor
/// (or the x64 one, which Windows on ARM runs too).
fn find_rc() -> Option<PathBuf> {
    if let Some(rc) = std::env::var_os("RC") {
        return Some(rc.into());
    }
    let host = match std::env::consts::ARCH {
        "aarch64" => "arm64",
        "x86" => "x86",
        _ => "x64",
    };
    newest_rc(host).or_else(|| newest_rc("x64"))
}

fn newest_rc(host: &str) -> Option<PathBuf> {
    let roots = ["ProgramFiles(x86)", "ProgramFiles"].iter().filter_map(std::env::var_os);
    let mut found: Vec<PathBuf> = roots
        .map(|r| PathBuf::from(r).join("Windows Kits").join("10").join("bin"))
        .filter_map(|bin| std::fs::read_dir(bin).ok())
        .flatten()
        .flatten()
        .map(|e| e.path().join(host).join("rc.exe"))
        .filter(|p| p.is_file())
        .collect();
    // Version folders (10.0.26100.0) sort by their numbers.
    found.sort_by_key(|p| {
        let ver = p.parent().and_then(Path::parent).and_then(Path::file_name);
        ver.map(|v| v.to_string_lossy().split('.').map(|n| n.parse::<u32>().unwrap_or(0)).collect::<Vec<_>>())
    });
    found.pop()
}

fn resource_script(icon: &Path) -> String {
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts = version.split(['.', '-', '+']).map(|p| p.parse::<u16>().unwrap_or(0));
    let [major, minor, patch] = [parts.next(), parts.next(), parts.next()].map(|p| p.unwrap_or(0));
    let icon = icon.display().to_string().replace('\\', "\\\\");
    // Numbers instead of winver.h's names, so no include path is needed:
    // VS_VERSION_INFO 1, VOS_NT_WINDOWS32 0x40004, VFT_APP 1, and
    // 0x0409 04B0 for US English in Unicode.
    format!(
        r#"1 ICON "{icon}"

1 VERSIONINFO
FILEVERSION {major},{minor},{patch},0
PRODUCTVERSION {major},{minor},{patch},0
FILEFLAGSMASK 0x3F
FILEFLAGS 0
FILEOS 0x40004
FILETYPE 1
FILESUBTYPE 0
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Holonet"
      VALUE "FileDescription", "Holonet"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "holonet"
      VALUE "LegalCopyright", "Copyright (c) 2026 the Holonet authors. MIT licence."
      VALUE "OriginalFilename", "holonet.exe"
      VALUE "ProductName", "Holonet"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x0409, 1200
  END
END
"#
    )
}
