//! System-wide open-handle enumeration.
//!
//! Restart Manager only reports *files* and cannot see directory handles, so a
//! terminal whose working directory is inside a folder (the single most common
//! reason a folder "can't be deleted") is invisible to it. To catch those we
//! ask the kernel for every handle in the system, keep the ones whose object
//! type is "File", duplicate each into our own process and ask what path it
//! refers to.
//!
//! Two hazards are handled here:
//! * Asking for the name of some handles (synchronous named pipes, some
//!   device objects) can block forever. We only query handles whose
//!   `GetFileType` is `FILE_TYPE_DISK`, and we additionally do the query on a
//!   worker thread with a timeout so one bad handle can never freeze a scan.
//! * Handle values are only meaningful inside their own process, so all work
//!   goes through `DuplicateHandle` and the duplicate is closed afterwards.

use std::collections::HashMap;
use std::sync::mpsc;
use std::time::Duration;

use windows_sys::Wdk::System::SystemInformation::NtQuerySystemInformation;
use windows_sys::Win32::Foundation::{
    DUPLICATE_SAME_ACCESS, DuplicateHandle, GENERIC_READ, HANDLE,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_BACKUP_SEMANTICS, FILE_NAME_NORMALIZED, FILE_SHARE_DELETE,
    FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TYPE_DISK, GetFileType, GetFinalPathNameByHandleW,
    OPEN_EXISTING, VOLUME_NAME_DOS,
};
use windows_sys::Win32::System::Threading::{
    GetCurrentProcess, GetCurrentProcessId, OpenProcess, PROCESS_DUP_HANDLE,
};

use crate::win::{Handle, wide};

/// `SystemExtendedHandleInformation` – returns 64-bit-safe PIDs and handle values.
const SYSTEM_EXTENDED_HANDLE_INFORMATION: i32 = 64;
const STATUS_INFO_LENGTH_MISMATCH: i32 = 0xC000_0004_u32 as i32;

/// Layout of one entry as returned by the kernel for class 64.
#[repr(C)]
#[derive(Clone, Copy)]
struct HandleEntry {
    object: usize,
    pid: usize,
    handle_value: usize,
    granted_access: u32,
    creator_back_trace_index: u16,
    object_type_index: u16,
    handle_attributes: u32,
    reserved: u32,
}

#[repr(C)]
struct HandleInfoHeader {
    count: usize,
    reserved: usize,
}

/// One open file/directory handle that some process holds.
#[derive(Clone, Debug)]
pub struct OpenHandle {
    pub pid: u32,
    pub handle_value: usize,
    /// Full path in `\\?\` form.
    pub path: String,
}

/// Snapshot the kernel handle table.
fn query_handle_table() -> Result<Vec<HandleEntry>, String> {
    let mut size: usize = 1 << 20;
    loop {
        let mut buf: Vec<u8> = vec![0; size];
        let mut needed: u32 = 0;
        let status = unsafe {
            NtQuerySystemInformation(
                SYSTEM_EXTENDED_HANDLE_INFORMATION,
                buf.as_mut_ptr().cast(),
                size as u32,
                &mut needed,
            )
        };
        if status == STATUS_INFO_LENGTH_MISMATCH {
            // The table grows while we look at it; overshoot generously.
            size = (needed as usize).max(size * 2) + (1 << 16);
            if size > (1 << 30) {
                return Err("handle table is unreasonably large".into());
            }
            continue;
        }
        if status < 0 {
            return Err(format!(
                "NtQuerySystemInformation failed (0x{:08X})",
                status as u32
            ));
        }
        let header = unsafe { &*(buf.as_ptr() as *const HandleInfoHeader) };
        let entries_ptr =
            unsafe { buf.as_ptr().add(size_of::<HandleInfoHeader>()) as *const HandleEntry };
        let max_fit = (buf.len() - size_of::<HandleInfoHeader>()) / size_of::<HandleEntry>();
        let count = header.count.min(max_fit);
        let mut out = Vec::with_capacity(count);
        for i in 0..count {
            out.push(unsafe { std::ptr::read_unaligned(entries_ptr.add(i)) });
        }
        return Ok(out);
    }
}

/// The numeric object-type index of "File" differs between Windows builds, so
/// discover it empirically: open a file ourselves and find that handle.
fn file_type_index(table: &[HandleEntry], probe: HANDLE) -> Option<u16> {
    let me = unsafe { GetCurrentProcessId() } as usize;
    table
        .iter()
        .find(|e| e.pid == me && e.handle_value == probe as usize)
        .map(|e| e.object_type_index)
}

fn open_probe() -> Option<Handle> {
    let exe = std::env::current_exe().ok()?;
    let w = wide(exe.as_os_str());
    let h = unsafe {
        CreateFileW(
            w.as_ptr(),
            GENERIC_READ,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    Handle::new(h)
}

/// Resolve a (duplicated, locally owned) handle to a path, or `None` if it is
/// not an on-disk file.
fn resolve_local(h: HANDLE) -> Option<String> {
    unsafe {
        if GetFileType(h) != FILE_TYPE_DISK {
            return None;
        }
        let mut buf = vec![0u16; 1024];
        loop {
            let n = GetFinalPathNameByHandleW(
                h,
                buf.as_mut_ptr(),
                buf.len() as u32,
                FILE_NAME_NORMALIZED | VOLUME_NAME_DOS,
            ) as usize;
            if n == 0 {
                return None;
            }
            if n < buf.len() {
                return Some(String::from_utf16_lossy(&buf[..n]));
            }
            buf.resize(n + 1, 0);
        }
    }
}

/// Runs `resolve_local` on a background thread so a hang costs us a timeout
/// instead of the whole scan. Items are processed in order; if a reply does
/// not arrive in time, the item being worked on is the hung one, the thread is
/// abandoned and a fresh worker picks up after it.
struct Resolver {
    tx: mpsc::Sender<usize>,
    rx: mpsc::Receiver<Option<String>>,
}

impl Resolver {
    fn spawn() -> Self {
        let (tx, worker_rx) = mpsc::channel::<usize>();
        let (worker_tx, rx) = mpsc::channel::<Option<String>>();
        std::thread::spawn(move || {
            for raw in worker_rx {
                let owned = Handle(raw as HANDLE);
                let path = resolve_local(owned.0);
                drop(owned);
                if worker_tx.send(path).is_err() {
                    break;
                }
            }
        });
        Self { tx, rx }
    }
}

const RESOLVE_TIMEOUT: Duration = Duration::from_millis(250);
const MAX_HUNG_WORKERS: usize = 16;

/// Enumerate every open on-disk file or directory handle in the system and
/// return the ones `wanted` accepts. `wanted` receives a lower-cased `\\?\`
/// path.
/// The second value is a warning when the scan had to stop early.
pub fn find_open_handles(
    wanted: &dyn Fn(&str) -> bool,
) -> Result<(Vec<OpenHandle>, Option<String>), String> {
    let probe = open_probe().ok_or("could not open probe file")?;
    let table = query_handle_table()?;
    let file_type =
        file_type_index(&table, probe.0).ok_or("could not determine the File object type")?;
    drop(probe);

    let me = unsafe { GetCurrentProcessId() } as usize;
    let mut by_pid: HashMap<usize, Vec<usize>> = HashMap::new();
    for e in table
        .iter()
        .filter(|e| e.object_type_index == file_type && e.pid != me && e.pid != 0)
    {
        by_pid.entry(e.pid).or_default().push(e.handle_value);
    }

    let mut resolver = Resolver::spawn();
    let mut hung = 0usize;
    let mut found = Vec::new();
    let current = unsafe { GetCurrentProcess() };

    for (pid, handles) in by_pid {
        let Some(proc) = Handle::new(unsafe { OpenProcess(PROCESS_DUP_HANDLE, 0, pid as u32) })
        else {
            continue; // access denied: protected process, or not elevated
        };
        for hv in handles {
            let mut dup: HANDLE = std::ptr::null_mut();
            let ok = unsafe {
                DuplicateHandle(
                    proc.0,
                    hv as HANDLE,
                    current,
                    &mut dup,
                    0,
                    0,
                    DUPLICATE_SAME_ACCESS,
                )
            };
            if ok == 0 || dup.is_null() {
                continue;
            }
            if resolver.tx.send(dup as usize).is_err() {
                resolver = Resolver::spawn();
                continue;
            }
            match resolver.rx.recv_timeout(RESOLVE_TIMEOUT) {
                Ok(Some(path)) => {
                    if wanted(&path.to_lowercase()) {
                        found.push(OpenHandle {
                            pid: pid as u32,
                            handle_value: hv,
                            path,
                        });
                    }
                }
                Ok(None) => {}
                Err(_) => {
                    hung += 1;
                    if hung > MAX_HUNG_WORKERS {
                        let warn =
                            "Some handles stopped responding; the handle scan is incomplete."
                                .to_string();
                        return Ok((found, Some(warn)));
                    }
                    resolver = Resolver::spawn();
                }
            }
        }
    }
    Ok((found, None))
}
