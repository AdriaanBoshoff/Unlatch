//! "Unlock with Unlatch" in the Windows 11 compact (top-level) context menu.
//!
//! Windows 11 only shows `IExplorerCommand` handlers that come from a
//! packaged app in its new menu; classic registry verbs are pushed down into
//! "Show more options". This DLL is that handler. It is registered through a
//! sparse package (see `packaging/AppxManifest.xml`) whose external location
//! is the Unlatch install folder, so it sits next to `Unlatch.exe`.
//!
//! When clicked it starts `Unlatch.exe` with every selected path, so a
//! multi-selection opens a single window.
//!
//! This code runs inside Explorer: it must never panic (the release profile
//! aborts on panic, which would take Explorer down with it), so every
//! fallible call returns an error instead of unwrapping.

use std::ffi::c_void;
use std::os::windows::process::CommandExt;
use std::path::PathBuf;

use windows::Win32::Foundation::{
    CLASS_E_CLASSNOTAVAILABLE, CLASS_E_NOAGGREGATION, E_NOTIMPL, E_POINTER, HMODULE, S_FALSE,
};
use windows::Win32::System::Com::{CoTaskMemFree, IBindCtx, IClassFactory, IClassFactory_Impl};
use windows::Win32::System::LibraryLoader::{
    GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS, GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
    GetModuleFileNameW, GetModuleHandleExW,
};
use windows::Win32::UI::Shell::{
    ECF_DEFAULT, ECS_ENABLED, IEnumExplorerCommand, IExplorerCommand, IExplorerCommand_Impl,
    IShellItemArray, SHStrDupW, SIGDN_FILESYSPATH,
};
use windows::core::{BOOL, GUID, HRESULT, HSTRING, IUnknown, Interface, PWSTR, Ref, Result};
use windows_core::implement;

/// Must match the `Clsid` / `Id` values in `packaging/AppxManifest.xml`.
pub const CLSID_UNLATCH_COMMAND: GUID = GUID::from_u128(0xd30556c9_3fa0_4866_ae9a_5c91e9d43d9d);

const TITLE: &str = "Unlock with Unlatch";
const CREATE_NO_WINDOW: u32 = 0x0800_0000;

/// Folder this DLL was loaded from (the Unlatch install folder).
fn module_dir() -> Option<PathBuf> {
    // Any address inside this DLL identifies the module that contains it.
    static ANCHOR: u8 = 0;
    let mut module = HMODULE::default();
    unsafe {
        GetModuleHandleExW(
            GET_MODULE_HANDLE_EX_FLAG_FROM_ADDRESS | GET_MODULE_HANDLE_EX_FLAG_UNCHANGED_REFCOUNT,
            windows::core::PCWSTR(&ANCHOR as *const u8 as *const u16),
            &mut module,
        )
        .ok()?;
    }
    let mut buf = vec![0u16; 32768];
    let len = unsafe { GetModuleFileNameW(Some(module), &mut buf) } as usize;
    if len == 0 || len >= buf.len() {
        return None;
    }
    let path = PathBuf::from(String::from_utf16_lossy(&buf[..len]));
    path.parent().map(PathBuf::from)
}

fn unlatch_exe() -> Option<PathBuf> {
    let exe = module_dir()?.join("Unlatch.exe");
    exe.exists().then_some(exe)
}

/// Copy a string into shell-owned (CoTaskMem) memory, as the interface expects.
fn shell_string(s: &str) -> Result<PWSTR> {
    unsafe { SHStrDupW(&HSTRING::from(s)) }
}

/// File-system paths of everything in the selection; items that are not
/// real files (Control Panel, phones, …) are skipped.
fn selected_paths(items: &IShellItemArray) -> Vec<String> {
    let mut out = Vec::new();
    let Ok(count) = (unsafe { items.GetCount() }) else {
        return out;
    };
    for i in 0..count {
        let Ok(item) = (unsafe { items.GetItemAt(i) }) else {
            continue;
        };
        if let Ok(name) = unsafe { item.GetDisplayName(SIGDN_FILESYSPATH) } {
            if let Ok(path) = unsafe { name.to_string() } {
                out.push(path);
            }
            unsafe { CoTaskMemFree(Some(name.0 as *const c_void)) };
        }
    }
    out
}

#[implement(IExplorerCommand)]
struct UnlockCommand;

impl IExplorerCommand_Impl for UnlockCommand_Impl {
    fn GetTitle(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        shell_string(TITLE)
    }

    fn GetIcon(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        let exe = unlatch_exe().ok_or(windows::core::Error::from(E_NOTIMPL))?;
        shell_string(&format!("{},0", exe.display()))
    }

    fn GetToolTip(&self, _items: Ref<IShellItemArray>) -> Result<PWSTR> {
        shell_string("See what is using this and release it")
    }

    fn GetCanonicalName(&self) -> Result<GUID> {
        Ok(CLSID_UNLATCH_COMMAND)
    }

    fn GetState(&self, _items: Ref<IShellItemArray>, _ok_to_be_slow: BOOL) -> Result<u32> {
        Ok(ECS_ENABLED.0 as u32)
    }

    fn Invoke(&self, items: Ref<IShellItemArray>, _ctx: Ref<IBindCtx>) -> Result<()> {
        let Some(items) = items.as_ref() else {
            return Ok(());
        };
        let paths = selected_paths(items);
        if paths.is_empty() {
            return Ok(());
        }
        let exe = unlatch_exe().ok_or(windows::core::Error::from(E_POINTER))?;
        // Rust quotes each argument correctly, including a trailing `\` on
        // drive roots such as `C:\`.
        std::process::Command::new(exe)
            .args(&paths)
            .creation_flags(CREATE_NO_WINDOW)
            .spawn()
            .map(drop)
            .map_err(|e| {
                windows::core::Error::from_hresult(HRESULT::from_win32(
                    e.raw_os_error().unwrap_or(1) as u32,
                ))
            })
    }

    fn GetFlags(&self) -> Result<u32> {
        Ok(ECF_DEFAULT.0 as u32)
    }

    fn EnumSubCommands(&self) -> Result<IEnumExplorerCommand> {
        Err(E_NOTIMPL.into())
    }
}

#[implement(IClassFactory)]
struct Factory;

impl IClassFactory_Impl for Factory_Impl {
    fn CreateInstance(
        &self,
        outer: Ref<IUnknown>,
        iid: *const GUID,
        object: *mut *mut c_void,
    ) -> Result<()> {
        if object.is_null() || iid.is_null() {
            return Err(E_POINTER.into());
        }
        unsafe { *object = std::ptr::null_mut() };
        if outer.is_some() {
            return Err(CLASS_E_NOAGGREGATION.into());
        }
        let command: IExplorerCommand = UnlockCommand.into();
        unsafe { command.query(iid, object) }.ok()
    }

    fn LockServer(&self, _lock: BOOL) -> Result<()> {
        Ok(())
    }
}

/// COM entry point: hand out a factory for our one class.
///
/// # Safety
/// Called by COM with valid pointers.
#[unsafe(no_mangle)]
pub unsafe extern "system" fn DllGetClassObject(
    clsid: *const GUID,
    iid: *const GUID,
    out: *mut *mut c_void,
) -> HRESULT {
    if out.is_null() || clsid.is_null() || iid.is_null() {
        return E_POINTER;
    }
    unsafe {
        *out = std::ptr::null_mut();
        if *clsid != CLSID_UNLATCH_COMMAND {
            return CLASS_E_CLASSNOTAVAILABLE;
        }
        let factory: IClassFactory = Factory.into();
        factory.query(iid, out)
    }
}

/// Never ask to be unloaded: the DLL is tiny, and staying loaded avoids
/// having to count live objects.
#[unsafe(no_mangle)]
pub extern "system" fn DllCanUnloadNow() -> HRESULT {
    S_FALSE
}
