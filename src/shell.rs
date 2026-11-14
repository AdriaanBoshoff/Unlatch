//! Explorer integration and shell actions: the right-click menu entries,
//! per-user install/uninstall, Recycle Bin deletes and UAC relaunch.
//!
//! Everything lives under HKEY_CURRENT_USER and %LOCALAPPDATA%, so installing
//! never needs administrator rights.

use std::path::{Path, PathBuf};

use windows_sys::Win32::Foundation::ERROR_SUCCESS;
use windows_sys::Win32::System::Registry::{
    HKEY, HKEY_CURRENT_USER, KEY_READ, REG_DWORD, REG_SZ, RegCloseKey, RegDeleteTreeW,
    RegOpenKeyExW, RegSetKeyValueW,
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
pub fn install() -> Result<PathBuf, String> {
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

    notify_shell();
    Ok(exe)
}

/// Remove the registry entries and the installed copy. A running .exe cannot
/// delete itself, so if we *are* the installed copy, a short-lived `cmd`
/// finishes the job a moment after we exit.
pub fn uninstall() -> Result<(), String> {
    for (key, _) in MENU_KEYS {
        delete_tree(&format!(r"Software\Classes\{key}"));
    }
    delete_tree(UNINSTALL_KEY);
    notify_shell();

    let dir = install_dir();
    if !dir.exists() {
        return Ok(());
    }
    let running_from_dir = std::env::current_exe().is_ok_and(|c| c.starts_with(&dir));
    if running_from_dir {
        let script = format!(
            "ping -n 3 127.0.0.1 >nul & rmdir /s /q \"{}\"",
            dir.display()
        );
        std::process::Command::new("cmd")
            .args(["/d", "/c", &script])
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
