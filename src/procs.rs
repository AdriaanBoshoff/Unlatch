//! Process inspection and control: names, image paths, loaded modules,
//! terminating, closing a single remote handle, and privilege checks.

use std::collections::HashMap;
use std::time::Duration;

use windows_sys::Win32::Foundation::{
    DUPLICATE_CLOSE_SOURCE, DuplicateHandle, HANDLE, LUID, WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::{
    AdjustTokenPrivileges, GetTokenInformation, LUID_AND_ATTRIBUTES, LookupPrivilegeValueW,
    SE_PRIVILEGE_ENABLED, TOKEN_ADJUST_PRIVILEGES, TOKEN_ELEVATION, TOKEN_PRIVILEGES, TOKEN_QUERY,
    TokenElevation,
};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, PROCESSENTRY32W,
    Process32FirstW, Process32NextW, TH32CS_SNAPMODULE, TH32CS_SNAPMODULE32, TH32CS_SNAPPROCESS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, OpenProcess, OpenProcessToken, PROCESS_DUP_HANDLE, PROCESS_NAME_WIN32,
    PROCESS_QUERY_LIMITED_INFORMATION, PROCESS_SYNCHRONIZE, PROCESS_TERMINATE,
    QueryFullProcessImageNameW, TerminateProcess, WaitForSingleObject,
};

use crate::win::{Handle, from_wide, last_error_message, wide};

/// PID → executable file name, from a toolhelp snapshot.
pub fn process_names() -> HashMap<u32, String> {
    let mut out = HashMap::new();
    let Some(snap) = Handle::new(unsafe { CreateToolhelp32Snapshot(TH32CS_SNAPPROCESS, 0) }) else {
        return out;
    };
    let mut e: PROCESSENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = size_of::<PROCESSENTRY32W>() as u32;
    let mut ok = unsafe { Process32FirstW(snap.0, &mut e) };
    while ok != 0 {
        out.insert(e.th32ProcessID, from_wide(&e.szExeFile));
        ok = unsafe { Process32NextW(snap.0, &mut e) };
    }
    out
}

/// Full path of a process's executable, if we are allowed to ask.
pub fn image_path(pid: u32) -> Option<String> {
    let proc = Handle::new(unsafe { OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid) })?;
    let mut buf = vec![0u16; 32768];
    let mut len = buf.len() as u32;
    let ok = unsafe {
        QueryFullProcessImageNameW(proc.0, PROCESS_NAME_WIN32, buf.as_mut_ptr(), &mut len)
    };
    (ok != 0).then(|| String::from_utf16_lossy(&buf[..len as usize]))
}

/// Executable images (the .exe and every DLL) mapped into a process.
pub fn loaded_modules(pid: u32) -> Vec<String> {
    let mut out = Vec::new();
    let Some(snap) = Handle::new(unsafe {
        CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)
    }) else {
        return out;
    };
    let mut e: MODULEENTRY32W = unsafe { std::mem::zeroed() };
    e.dwSize = size_of::<MODULEENTRY32W>() as u32;
    let mut ok = unsafe { Module32FirstW(snap.0, &mut e) };
    while ok != 0 {
        out.push(from_wide(&e.szExePath));
        ok = unsafe { Module32NextW(snap.0, &mut e) };
    }
    out
}

/// Forcefully end a process and wait briefly for it to go away.
pub fn terminate(pid: u32) -> Result<(), String> {
    let proc = Handle::new(unsafe { OpenProcess(PROCESS_TERMINATE | PROCESS_SYNCHRONIZE, 0, pid) })
        .ok_or_else(|| format!("cannot open process {pid}: {}", last_error_message()))?;
    if unsafe { TerminateProcess(proc.0, 1) } == 0 {
        return Err(format!(
            "cannot end process {pid}: {}",
            last_error_message()
        ));
    }
    let wait = unsafe { WaitForSingleObject(proc.0, Duration::from_secs(5).as_millis() as u32) };
    if wait != WAIT_OBJECT_0 {
        return Err(format!("process {pid} did not exit within 5 seconds"));
    }
    Ok(())
}

/// Close one handle inside another process without ending it. The owning
/// program is not told, so its next use of that handle will fail.
pub fn close_remote_handle(pid: u32, handle_value: usize) -> Result<(), String> {
    let proc = Handle::new(unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, pid) })
        .ok_or_else(|| format!("cannot open process {pid}: {}", last_error_message()))?;
    let ok = unsafe {
        DuplicateHandle(
            proc.0,
            handle_value as HANDLE,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            0,
            0,
            DUPLICATE_CLOSE_SOURCE,
        )
    };
    if ok == 0 {
        return Err(format!(
            "cannot close handle in process {pid}: {}",
            last_error_message()
        ));
    }
    Ok(())
}

/// Whether the current process token is elevated (UAC "Run as administrator").
pub fn is_elevated() -> bool {
    let mut token: HANDLE = std::ptr::null_mut();
    if unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) } == 0 {
        return false;
    }
    let token = Handle(token);
    let mut elev = TOKEN_ELEVATION { TokenIsElevated: 0 };
    let mut ret = 0u32;
    let ok = unsafe {
        GetTokenInformation(
            token.0,
            TokenElevation,
            (&mut elev as *mut TOKEN_ELEVATION).cast(),
            size_of::<TOKEN_ELEVATION>() as u32,
            &mut ret,
        )
    };
    ok != 0 && elev.TokenIsElevated != 0
}

/// When elevated, SeDebugPrivilege lets us open handles of services and
/// processes belonging to other users. Harmless no-op otherwise.
pub fn enable_debug_privilege() {
    unsafe {
        let mut token: HANDLE = std::ptr::null_mut();
        if OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_ADJUST_PRIVILEGES | TOKEN_QUERY,
            &mut token,
        ) == 0
        {
            return;
        }
        let token = Handle(token);
        let mut luid = LUID {
            LowPart: 0,
            HighPart: 0,
        };
        let name = wide("SeDebugPrivilege");
        if LookupPrivilegeValueW(std::ptr::null(), name.as_ptr(), &mut luid) == 0 {
            return;
        }
        let tp = TOKEN_PRIVILEGES {
            PrivilegeCount: 1,
            Privileges: [LUID_AND_ATTRIBUTES {
                Luid: luid,
                Attributes: SE_PRIVILEGE_ENABLED,
            }],
        };
        AdjustTokenPrivileges(
            token.0,
            0,
            &tp,
            0,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
        );
    }
}
