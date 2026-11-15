//! Explorer integration and shell actions: the right-click menu entries,
//! per-user install/uninstall, Recycle Bin deletes and UAC relaunch.
//!
//! Everything lives under HKEY_CURRENT_USER and %LOCALAPPDATA%, so installing
//! never needs administrator rights.

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, HKEY_LOCAL_MACHINE, KEY_READ, REG_DWORD, REG_SZ, RRF_RT_REG_DWORD,
    RegCloseKey, RegDeleteTreeW, RegGetValueW, RegOpenKeyExW, RegSetKeyValueW,
};
use windows_sys::Win32::UI::Shell::{
    FO_DELETE, FOF_ALLOWUNDO, FOF_NOCONFIRMATION, SHCNE_ASSOCCHANGED, SHCNF_IDLIST, SHChangeNotify,
    SHFILEOPSTRUCTW, SHFileOperationW, ShellExecuteW,
};
use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;

use crate::win::wide;

const APP: &str = "Unlatch";
const VERB_LABEL: &str = "Unlock with Unlatch";
const ICON_BYTES: &[u8] = include_bytes!("../assets/unlatch.ico");

/// (registry key under HKCU\Software\Classes, placeholder Explorer substitutes)
const MENU_KEYS: &[(&str, &str)] = &[
    (r"*\shell\Unlatch", "%1"),                    // any file
    (r"Directory\shell\Unlatch", "%1"),            // a folder
    (r"Directory\Background\shell\Unlatch", "%V"), // empty space inside a folder
    (r"Drive\shell\Unlatch", "%1"),                // a drive
];
const UNINSTALL_KEY: &str = r"Software\Microsoft\Windows\CurrentVersion\Uninstall\Unlatch";

// Windows 11 top-level menu: a sparse package (packaging/AppxManifest.xml)
// that registers unlatch_shell.dll as an IExplorerCommand handler. The DLL
// is optional; without it next to the exe, only the classic entries are added.
const PACKAGE_NAME: &str = "Unlatch.ContextMenu";
const PACKAGE_MANIFEST: &str = include_str!("../packaging/AppxManifest.xml");
const PACKAGE_LOGOS: &[(&str, &[u8])] = &[
    (
        "Square44x44Logo.png",
        include_bytes!("../packaging/Assets/Square44x44Logo.png"),
    ),
    (
        "Square150x150Logo.png",
        include_bytes!("../packaging/Assets/Square150x150Logo.png"),
    ),
    (
        "StoreLogo.png",
        include_bytes!("../packaging/Assets/StoreLogo.png"),
    ),
];
const SHELL_DLL: &str = "unlatch_shell.dll";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// What happened to the Windows 11 top-level menu entry during install.
pub enum ModernMenu {
    Added,
    /// unlatch_shell.dll was not next to the exe.
    NotBundled,
    Failed(String),
}

pub struct Installed {
    pub exe: PathBuf,
    pub modern: ModernMenu,
}

impl Installed {
    /// Short explanation for the user of where to find the menu entry.
    pub fn summary(&self) -> String {
        let where_ = match &self.modern {
            ModernMenu::Added => {
                "Right-click any file or folder and choose Unlock with Unlatch.".to_string()
            }
            ModernMenu::NotBundled => {
                "Right-click any file or folder and choose Show more options > Unlock with Unlatch."
                    .to_string()
            }
            ModernMenu::Failed(e) => format!(
                "Right-click any file or folder and choose Show more options > Unlock with Unlatch.\n\n\
                 The Windows 11 top-level menu entry could not be added:\n{e}"
            ),
        };
        format!(
            "Unlatch is installed to\n{}\n\n{where_}",
            self.exe.display()
        )
    }
}

pub fn install_dir() -> PathBuf {
    let base = std::env::var_os("LOCALAPPDATA")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    base.join("Programs").join(APP)
}

fn set_sz(key: &str, name: Option<&str>, value: &str) -> Result<(), String> {
    let k = wide(key);
    let n = name.map(wide);
    let v = wide(value);
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ref().map_or(std::ptr::null(), |n| n.as_ptr()),
            REG_SZ,
            v.as_ptr().cast(),
            (v.len() * 2) as u32,
        )
    };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("writing HKCU\\{key} failed ({rc})"))
    }
}

fn set_dword(key: &str, name: &str, value: u32) -> Result<(), String> {
    let k = wide(key);
    let n = wide(name);
    let rc = unsafe {
        RegSetKeyValueW(
            HKEY_CURRENT_USER,
            k.as_ptr(),
            n.as_ptr(),
            REG_DWORD,
            (&value as *const u32).cast(),
            4,
        )
    };
    if rc == ERROR_SUCCESS {
        Ok(())
    } else {
        Err(format!("writing HKCU\\{key} failed ({rc})"))
    }
}

fn delete_tree(key: &str) {
    let k = wide(key);
    unsafe { RegDeleteTreeW(HKEY_CURRENT_USER, k.as_ptr()) };
}

fn key_exists(key: &str) -> bool {
    let k = wide(key);
    let mut h: HKEY = std::ptr::null_mut();
    let rc = unsafe { RegOpenKeyExW(HKEY_CURRENT_USER, k.as_ptr(), 0, KEY_READ, &mut h) };
    if rc == ERROR_SUCCESS {
        unsafe { RegCloseKey(h) };
        true
    } else {
        false
    }
}

pub fn is_installed() -> bool {
    key_exists(&format!(r"Software\Classes\{}", MENU_KEYS[0].0))
}

fn notify_shell() {
    unsafe {
        SHChangeNotify(
            SHCNE_ASSOCCHANGED as i32,
            SHCNF_IDLIST,
            std::ptr::null(),
            std::ptr::null(),
        )
    };
}

/// Copy ourselves to %LOCALAPPDATA%\Programs\Unlatch and register the
/// right-click entries plus an "Installed apps" uninstall entry.
pub fn install() -> Result<Installed, String> {
    let dir = install_dir();
    std::fs::create_dir_all(&dir).map_err(|e| format!("creating {}: {e}", dir.display()))?;
    let exe = dir.join("Unlatch.exe");
    let current = std::env::current_exe().map_err(|e| e.to_string())?;
    if !same_file(&current, &exe) {
        std::fs::copy(&current, &exe).map_err(|e| format!("copying to {}: {e}", exe.display()))?;
    }
    let icon = dir.join("Unlatch.ico");
    std::fs::write(&icon, ICON_BYTES).map_err(|e| format!("writing icon: {e}"))?;

    let exe_s = exe.to_string_lossy();
    let icon_s = icon.to_string_lossy();
    for (key, placeholder) in MENU_KEYS {
        let key = format!(r"Software\Classes\{key}");
        set_sz(&key, Some("MUIVerb"), VERB_LABEL)?;
        set_sz(&key, Some("Icon"), &icon_s)?;
        set_sz(
            &format!(r"{key}\command"),
            None,
            &format!("\"{exe_s}\" \"{placeholder}\""),
        )?;
    }

    set_sz(UNINSTALL_KEY, Some("DisplayName"), APP)?;
    set_sz(
        UNINSTALL_KEY,
        Some("DisplayVersion"),
        env!("CARGO_PKG_VERSION"),
    )?;
    set_sz(UNINSTALL_KEY, Some("Publisher"), APP)?;
    set_sz(UNINSTALL_KEY, Some("DisplayIcon"), &icon_s)?;
    set_sz(
        UNINSTALL_KEY,
        Some("InstallLocation"),
        &dir.to_string_lossy(),
    )?;
    set_sz(
        UNINSTALL_KEY,
        Some("UninstallString"),
        &format!("\"{exe_s}\" --uninstall"),
    )?;
    set_dword(UNINSTALL_KEY, "NoModify", 1)?;
    set_dword(UNINSTALL_KEY, "NoRepair", 1)?;

    let modern = install_modern_menu(&current, &dir);
    notify_shell();
    Ok(Installed { exe, modern })
}

/// Copy the shell-extension DLL next to the installed exe, write out the
/// embedded package manifest and logos, and register that manifest in place
/// with the install folder as its external location.
fn install_modern_menu(current_exe: &Path, dir: &Path) -> ModernMenu {
    let Some(src_dir) = current_exe.parent() else {
        return ModernMenu::NotBundled;
    };
    let dll_src = src_dir.join(SHELL_DLL);
    if !dll_src.exists() {
        return ModernMenu::NotBundled;
    }
    if !same_file(src_dir, dir)
        && let Err(e) = copy_replacing(&dll_src, &dir.join(SHELL_DLL))
    {
        return ModernMenu::Failed(e);
    }
    if !developer_mode() {
        return ModernMenu::Failed(
            "Developer Mode is off. Turn it on in Settings > System > For developers, \
             then choose Add to right-click menu again."
                .into(),
        );
    }
    let manifest = match write_package_files(dir) {
        Ok(m) => m,
        Err(e) => return ModernMenu::Failed(e),
    };
    // Remove first: registering over an existing registration is refused.
    let script = format!(
        "$ErrorActionPreference='Stop'; \
         Get-AppxPackage -Name '{PACKAGE_NAME}' | Remove-AppxPackage; \
         Add-AppxPackage -Register {} -ExternalLocation {}",
        ps_quote(&manifest),
        ps_quote(dir)
    );
    match powershell(&script) {
        Ok(()) => ModernMenu::Added,
        Err(e) => ModernMenu::Failed(e),
    }
}

/// Write `package\AppxManifest.xml` (version filled in) and its logos.
fn write_package_files(dir: &Path) -> Result<PathBuf, String> {
    let pkg = dir.join("package");
    let assets = pkg.join("Assets");
    std::fs::create_dir_all(&assets).map_err(|e| format!("creating {}: {e}", assets.display()))?;
    // Package versions have four parts: 0.1.0 -> 0.1.0.0
    let version = format!("{}.0", env!("CARGO_PKG_VERSION"));
    let manifest = pkg.join("AppxManifest.xml");
    std::fs::write(&manifest, PACKAGE_MANIFEST.replace("@VERSION@", &version))
        .map_err(|e| format!("writing {}: {e}", manifest.display()))?;
    for (name, bytes) in PACKAGE_LOGOS {
        let path = assets.join(name);
        std::fs::write(&path, bytes).map_err(|e| format!("writing {}: {e}", path.display()))?;
    }
    Ok(manifest)
}

/// Registering an unsigned package from a folder needs Developer Mode.
fn developer_mode() -> bool {
    let key = wide(r"SOFTWARE\Microsoft\Windows\CurrentVersion\AppModelUnlock");
    let name = wide("AllowDevelopmentWithoutDevLicense");
    let mut value = 0u32;
    let mut size = 4u32;
    let rc = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key.as_ptr(),
            name.as_ptr(),
            RRF_RT_REG_DWORD,
            std::ptr::null_mut(),
            (&mut value as *mut u32).cast(),
            &mut size,
        )
    };
    rc == ERROR_SUCCESS && value != 0
}

/// Explorer keeps the shell-extension DLL loaded, so it usually cannot be
/// overwritten in place. A loaded DLL can still be renamed, though: move it
/// aside and copy the new one in. Leftovers are cleaned up on later installs.
fn copy_replacing(src: &Path, dst: &Path) -> Result<(), String> {
    if let Some(dir) = dst.parent()
        && let Ok(entries) = std::fs::read_dir(dir)
    {
        for e in entries.flatten() {
            if e.file_name().to_string_lossy().ends_with(".old") {
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    if std::fs::copy(src, dst).is_ok() {
        return Ok(());
    }
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis());
    let aside = dst.with_extension(format!("dll.{stamp}.old"));
    std::fs::rename(dst, &aside).map_err(|e| format!("replacing {}: {e}", dst.display()))?;
    std::fs::copy(src, dst)
        .map(drop)
        .map_err(|e| format!("copying to {}: {e}", dst.display()))
}

fn ps_quote(p: &Path) -> String {
    format!("'{}'", p.display().to_string().replace('\'', "''"))
}

/// Run a PowerShell snippet without flashing a console window; on failure
/// return PowerShell's own error text.
fn powershell(script: &str) -> Result<(), String> {
    use std::os::windows::process::CommandExt;
    let out = std::process::Command::new("powershell.exe")
        .args(["-NoProfile", "-NonInteractive", "-Command", script])
        .creation_flags(CREATE_NO_WINDOW)
        .output()
        .map_err(|e| format!("starting PowerShell: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        let err = String::from_utf8_lossy(&out.stderr);
        let first = err
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .unwrap_or("PowerShell reported an error");
        Err(first.to_string())
    }
}

/// Remove the registry entries and the installed copy. A running .exe cannot
/// delete itself, so if we *are* the installed copy, a short-lived `cmd`
/// finishes the job a moment after we exit.
pub fn uninstall() -> Result<(), String> {
    for (key, _) in MENU_KEYS {
        delete_tree(&format!(r"Software\Classes\{key}"));
    }
    delete_tree(UNINSTALL_KEY);
    // Ignore failures: the package is not there if the top-level menu was
    // never added.
    let _ = powershell(&format!(
        "Get-AppxPackage -Name '{PACKAGE_NAME}' | Remove-AppxPackage"
    ));
    notify_shell();

    let dir = install_dir();
    if !dir.exists() {
        return Ok(());
    }
    let running_from_dir = std::env::current_exe().is_ok_and(|c| c.starts_with(&dir));
    if running_from_dir {
        use std::os::windows::process::CommandExt;
        // cmd has its own quoting rules: pass the line verbatim (Rust's
        // argument escaping would turn the inner quotes into \"), and start
        // it outside the folder, which cannot be removed while it is a
        // process's current directory.
        let script = format!(
            "\"ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"\"",
            dir.display()
        );
        std::process::Command::new("cmd")
            .args(["/d", "/c"])
            .raw_arg(script)
            .current_dir(std::env::temp_dir())
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map_err(|e| format!("scheduling cleanup: {e}"))?;
    } else {
        std::fs::remove_dir_all(&dir).map_err(|e| format!("removing {}: {e}", dir.display()))?;
    }
    Ok(())
}

fn same_file(a: &Path, b: &Path) -> bool {
    match (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        (Ok(a), Ok(b)) => a == b,
        _ => false,
    }
}

/// Send paths to the Recycle Bin (undoable), letting Explorer report errors.
pub fn recycle(paths: &[String]) -> Result<(), String> {
    let mut from: Vec<u16> = Vec::new();
    for p in paths {
        from.extend(p.encode_utf16());
        from.push(0);
    }
    from.push(0); // list is double-NUL terminated
    let mut op: SHFILEOPSTRUCTW = unsafe { std::mem::zeroed() };
    op.wFunc = FO_DELETE;
    op.pFrom = from.as_ptr();
    op.fFlags = (FOF_ALLOWUNDO | FOF_NOCONFIRMATION) as u16;
    let rc = unsafe { SHFileOperationW(&mut op) };
    if rc != 0 {
        return Err(format!(
            "Windows could not move it to the Recycle Bin (code {rc}). It may still be in use."
        ));
    }
    if op.fAnyOperationsAborted != 0 {
        return Err("The delete was cancelled.".into());
    }
    Ok(())
}

/// Start a new elevated copy with the same paths. Returns true if UAC was
/// accepted and the new instance launched.
pub fn relaunch_elevated(paths: &[PathBuf]) -> bool {
    let Ok(exe) = std::env::current_exe() else {
        return false;
    };
    let params: String = paths
        .iter()
        .map(|p| format!("\"{}\"", p.display()))
        .collect::<Vec<_>>()
        .join(" ");
    let verb = wide("runas");
    let file = wide(exe.as_os_str());
    let params = wide(params);
    let rc = unsafe {
        ShellExecuteW(
            std::ptr::null_mut(),
            verb.as_ptr(),
            file.as_ptr(),
            params.as_ptr(),
            std::ptr::null(),
            SW_SHOWNORMAL,
        )
    };
    rc as isize > 32
}

/// Explorer restarts itself after most crashes, but not always after being
/// ended deliberately; make sure the taskbar comes back.
pub fn ensure_explorer_running() {
    std::thread::sleep(std::time::Duration::from_millis(1500));
    let running = crate::procs::process_names()
        .values()
        .any(|n| n.eq_ignore_ascii_case("explorer.exe"));
    if !running {
        let _ = std::process::Command::new("explorer.exe").spawn();
    }
}
