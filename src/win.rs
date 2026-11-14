//! Small helpers shared by the Win32 modules.

use std::ffi::OsStr;
use std::os::windows::ffi::OsStrExt;

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE, INVALID_HANDLE_VALUE};

/// NUL-terminated UTF-16 for passing to `W` APIs.
pub fn wide(s: impl AsRef<OsStr>) -> Vec<u16> {
    s.as_ref().encode_wide().chain(std::iter::once(0)).collect()
}

/// Read a UTF-16 buffer up to the first NUL.
pub fn from_wide(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end])
}

/// Owned kernel handle that closes itself.
pub struct Handle(pub HANDLE);

impl Handle {
    pub fn new(h: HANDLE) -> Option<Self> {
        if h.is_null() || h == INVALID_HANDLE_VALUE {
            None
        } else {
            Some(Self(h))
        }
    }
}

impl Drop for Handle {
    fn drop(&mut self) {
        unsafe { CloseHandle(self.0) };
    }
}

// Raw handles are plain integers in the kernel's eyes; we only ever use them
// from the thread that owns the wrapper or hand ownership across explicitly.
unsafe impl Send for Handle {}

/// Turn `\\?\C:\x` and `\\?\UNC\srv\share\x` into the familiar display forms.
pub fn display_path(p: &str) -> String {
    if let Some(rest) = p.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{rest}")
    } else if let Some(rest) = p.strip_prefix(r"\\?\") {
        rest.to_string()
    } else {
        p.to_string()
    }
}

pub fn last_error_message() -> String {
    std::io::Error::last_os_error().to_string()
}
