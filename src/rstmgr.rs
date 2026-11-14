//! Restart Manager: the documented Windows API for "who is using these files".
//!
//! It is good at what handle enumeration misses: memory-mapped files and
//! loaded images, and it hands back friendly application names and service
//! names. It cannot see directory handles, which is why `handles.rs` exists.

use windows_sys::Win32::Foundation::{ERROR_MORE_DATA, ERROR_SUCCESS};
use windows_sys::Win32::System::RestartManager::{
    CCH_RM_SESSION_KEY, RM_PROCESS_INFO, RmEndSession, RmGetList, RmRegisterResources,
    RmStartSession,
};

use crate::win::{from_wide, wide};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AppKind {
    Unknown,
    Window,
    Service,
    Explorer,
    Console,
    Critical,
}

impl AppKind {
    fn from_raw(v: i32) -> Self {
        match v {
            1 | 2 => Self::Window,
            3 => Self::Service,
            4 => Self::Explorer,
            5 => Self::Console,
            1000 => Self::Critical,
            _ => Self::Unknown,
        }
    }
}

#[derive(Clone, Debug)]
pub struct RmProcess {
    pub pid: u32,
    pub app_name: String,
    pub service: Option<String>,
    pub kind: AppKind,
}

/// Register the files with a fresh session and list the processes using any
/// of them. Files are registered in chunks to keep individual calls small.
pub fn who_uses(files: &[String]) -> Result<Vec<RmProcess>, String> {
    if files.is_empty() {
        return Ok(Vec::new());
    }
    let mut session: u32 = 0;
    let mut key = [0u16; CCH_RM_SESSION_KEY as usize + 1];
    let rc = unsafe { RmStartSession(&mut session, 0, key.as_mut_ptr()) };
    if rc != ERROR_SUCCESS {
        return Err(format!("RmStartSession failed ({rc})"));
    }
    let result = (|| {
        for chunk in files.chunks(256) {
            let wides: Vec<Vec<u16>> = chunk.iter().map(wide).collect();
            let ptrs: Vec<*const u16> = wides.iter().map(|w| w.as_ptr()).collect();
            let rc = unsafe {
                RmRegisterResources(
                    session,
                    ptrs.len() as u32,
                    ptrs.as_ptr(),
                    0,
                    std::ptr::null(),
                    0,
                    std::ptr::null(),
                )
            };
            if rc != ERROR_SUCCESS {
                return Err(format!("RmRegisterResources failed ({rc})"));
            }
        }

        let mut infos: Vec<RM_PROCESS_INFO> = Vec::new();
        loop {
            let mut needed = 0u32;
            let mut count = infos.capacity() as u32;
            let mut reasons = 0u32;
            let rc = unsafe {
                RmGetList(
                    session,
                    &mut needed,
                    &mut count,
                    infos.as_mut_ptr(),
                    &mut reasons,
                )
            };
            if rc == ERROR_MORE_DATA {
                infos = Vec::with_capacity(needed as usize + 4);
                continue;
            }
            if rc != ERROR_SUCCESS {
                return Err(format!("RmGetList failed ({rc})"));
            }
            unsafe { infos.set_len(count as usize) };
            break;
        }

        Ok(infos
            .iter()
            .map(|i| {
                let service = from_wide(&i.strServiceShortName);
                RmProcess {
                    pid: i.Process.dwProcessId,
                    app_name: from_wide(&i.strAppName),
                    service: (!service.is_empty()).then_some(service),
                    kind: AppKind::from_raw(i.ApplicationType),
                }
            })
            .collect())
    })();
    unsafe { RmEndSession(session) };
    result
}
