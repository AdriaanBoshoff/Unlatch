//! Embeds the icon, version info and application manifest into Unlatch.exe.
//!
//! Uses the Windows SDK resource compiler (rc.exe). If it can't be found the
//! build still succeeds; the exe just shows a generic icon in Explorer (the
//! window and right-click menu icons are set at runtime and are unaffected).

use std::path::{Path, PathBuf};
use std::process::Command;

fn find_rc() -> Option<PathBuf> {
    if let Some(rc) = std::env::var_os("RC") {
        return Some(rc.into());
    }
    let root = Path::new(r"C:\Program Files (x86)\Windows Kits\10\bin");
    let mut versions: Vec<PathBuf> = std::fs::read_dir(root)
        .ok()?
        .flatten()
        .map(|e| e.path().join("x64").join("rc.exe"))
        .filter(|p| p.exists())
        .collect();
    versions.sort();
    versions.pop()
}

fn main() {
    println!("cargo:rerun-if-changed=assets/unlatch.ico");
    println!("cargo:rerun-if-changed=build.rs");
    if std::env::var("CARGO_CFG_TARGET_OS").as_deref() != Ok("windows") {
        return;
    }
    let Some(rc) = find_rc() else {
        println!("cargo:warning=rc.exe not found; building without embedded icon/manifest");
        return;
    };

    let out = PathBuf::from(std::env::var("OUT_DIR").unwrap());
    let manifest_dir = PathBuf::from(std::env::var("CARGO_MANIFEST_DIR").unwrap());
    let icon = manifest_dir.join("assets").join("unlatch.ico");
    let version = std::env::var("CARGO_PKG_VERSION").unwrap();
    let mut parts: Vec<u32> = version
        .split(['.', '-'])
        .filter_map(|p| p.parse().ok())
        .collect();
    parts.resize(4, 0);
    let commas = parts
        .iter()
        .map(u32::to_string)
        .collect::<Vec<_>>()
        .join(",");

    let manifest = out.join("unlatch.manifest");
    std::fs::write(
        &manifest,
        r#"<?xml version="1.0" encoding="UTF-8" standalone="yes"?>
<assembly xmlns="urn:schemas-microsoft-com:asm.v1" manifestVersion="1.0">
  <assemblyIdentity type="win32" name="Unlatch" version="1.0.0.0"/>
  <trustInfo xmlns="urn:schemas-microsoft-com:asm.v3">
    <security><requestedPrivileges><requestedExecutionLevel level="asInvoker" uiAccess="false"/></requestedPrivileges></security>
  </trustInfo>
  <dependency><dependentAssembly>
    <assemblyIdentity type="win32" name="Microsoft.Windows.Common-Controls" version="6.0.0.0" processorArchitecture="*" publicKeyToken="6595b64144ccf1df" language="*"/>
  </dependentAssembly></dependency>
  <application xmlns="urn:schemas-microsoft-com:asm.v3"><windowsSettings>
    <dpiAware xmlns="http://schemas.microsoft.com/SMI/2005/WindowsSettings">true/pm</dpiAware>
    <dpiAwareness xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">PerMonitorV2</dpiAwareness>
    <longPathAware xmlns="http://schemas.microsoft.com/SMI/2016/WindowsSettings">true</longPathAware>
  </windowsSettings></application>
</assembly>
"#,
    )
    .unwrap();

    let esc = |p: &Path| p.display().to_string().replace('\\', "\\\\");
    let rc_src = format!(
        r#"1 ICON "{icon}"
1 24 "{manifest}"
1 VERSIONINFO
FILEVERSION {commas}
PRODUCTVERSION {commas}
FILEOS 0x40004
FILETYPE 0x1
BEGIN
  BLOCK "StringFileInfo"
  BEGIN
    BLOCK "040904B0"
    BEGIN
      VALUE "CompanyName", "Unlatch"
      VALUE "FileDescription", "Unlatch - find what is locking a file"
      VALUE "FileVersion", "{version}"
      VALUE "InternalName", "Unlatch"
      VALUE "OriginalFilename", "Unlatch.exe"
      VALUE "ProductName", "Unlatch"
      VALUE "ProductVersion", "{version}"
    END
  END
  BLOCK "VarFileInfo"
  BEGIN
    VALUE "Translation", 0x409, 1200
  END
END
"#,
        icon = esc(&icon),
        manifest = esc(&manifest),
    );
    let rc_file = out.join("unlatch.rc");
    std::fs::write(&rc_file, rc_src).unwrap();
    let res = out.join("unlatch.res");
    let status = Command::new(&rc)
        .arg("/nologo")
        .arg("/fo")
        .arg(&res)
        .arg(&rc_file)
        .status();
    match status {
        Ok(s) if s.success() => {
            // Both link.exe (MSVC) and GNU ld accept a compiled .res directly.
            println!("cargo:rustc-link-arg-bins={}", res.display());
        }
        _ => println!("cargo:warning=rc.exe failed; building without embedded icon/manifest"),
    }
}
