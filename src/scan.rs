//! Combines the three detection techniques into one answer:
//!
//! 1. open handles (files *and* directories, with exact paths),
//! 2. loaded executable images (a running .exe / loaded .dll inside the target),
//! 3. Restart Manager (memory-mapped files, friendly app + service names).

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use crate::rstmgr::{self, AppKind};
use crate::win::display_path;
use crate::{handles, procs};

/// Folder scans hand at most this many files to Restart Manager; beyond that
/// it becomes slow and the handle scan already covers the common cases.
const RM_FOLDER_FILE_LIMIT: usize = 4000;

#[derive(Clone, Debug)]
pub struct Target {
    pub display: String,
    /// Lower-cased `\\?\` form, used for prefix matching.
    key: String,
    pub is_dir: bool,
}

impl Target {
    pub fn new(p: &Path) -> Result<Self, String> {
        let canon = std::fs::canonicalize(p).map_err(|e| format!("{}: {e}", p.display()))?;
        let s = canon.to_string_lossy().to_string();
        Ok(Self {
            display: display_path(&s),
            key: s.trim_end_matches('\\').to_lowercase(),
            is_dir: canon.is_dir(),
        })
    }

    /// `key_path` must already be lower-case `\\?\` form.
    fn covers(&self, key_path: &str) -> bool {
        if key_path == self.key {
            return true;
        }
        self.is_dir
            && key_path.len() > self.key.len()
            && key_path.starts_with(&self.key)
            && key_path.as_bytes()[self.key.len()] == b'\\'
    }
}

/// Normalise an ordinary Win32 path to the lower-case `\\?\` matching form.
fn to_key(p: &str) -> String {
    let long = if p.starts_with(r"\\?\") {
        p.to_string()
    } else if let Some(rest) = p.strip_prefix(r"\\") {
        format!(r"\\?\UNC\{rest}")
    } else {
        format!(r"\\?\{p}")
    };
    long.to_lowercase()
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum LockHow {
    /// An open handle; the value can be closed remotely.
    Handle(usize),
    /// The file is a running program or loaded library.
    Module,
    /// Restart Manager says the process uses it (no handle detail).
    InUse,
}

#[derive(Clone, Debug)]
pub struct Lock {
    pub path: String,
    pub how: LockHow,
}

#[derive(Clone, Debug)]
pub struct Holder {
    pub pid: u32,
    pub exe_name: String,
    pub app_name: Option<String>,
    pub exe_path: Option<String>,
    pub service: Option<String>,
    pub kind: AppKind,
    pub locks: Vec<Lock>,
}

impl Holder {
    /// The kernel ("System", PID 4) and critical processes cannot be ended.
    pub fn is_protected(&self) -> bool {
        self.pid <= 4 || self.kind == AppKind::Critical
    }

    pub fn title(&self) -> &str {
        match &self.app_name {
            Some(n) if !n.is_empty() => n,
            _ => &self.exe_name,
        }
    }

    pub fn is_explorer(&self) -> bool {
        self.exe_name.eq_ignore_ascii_case("explorer.exe")
    }
}

#[derive(Clone, Debug, Default)]
pub struct ScanResult {
    pub targets: Vec<Target>,
    pub holders: Vec<Holder>,
    pub notes: Vec<String>,
}

fn looks_executable(path: &str) -> bool {
    const EXTS: &[&str] = &[
        "exe", "dll", "sys", "ocx", "cpl", "drv", "scr", "node", "pyd", "mui", "winmd", "ax",
        "efi", "so",
    ];
    Path::new(path)
        .extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

/// Collect regular files below `dir` without following links/junctions.
fn walk_files(dir: &Path, limit: usize, out: &mut Vec<String>) -> bool {
    let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
    while let Some(d) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&d) else {
            continue;
        };
        for entry in rd.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            if ft.is_symlink() {
                continue;
            }
            if ft.is_dir() {
                stack.push(entry.path());
            } else if ft.is_file() {
                if out.len() >= limit {
                    return false;
                }
                out.push(entry.path().to_string_lossy().to_string());
            }
        }
    }
    true
}

fn entry<'a>(
    holders: &'a mut HashMap<u32, Holder>,
    names: &HashMap<u32, String>,
    pid: u32,
) -> &'a mut Holder {
    holders.entry(pid).or_insert_with(|| Holder {
        pid,
        exe_name: names
            .get(&pid)
            .cloned()
            .unwrap_or_else(|| format!("PID {pid}")),
        app_name: None,
        exe_path: procs::image_path(pid),
        service: None,
        kind: AppKind::Unknown,
        locks: Vec::new(),
    })
}

pub fn scan(paths: &[PathBuf]) -> ScanResult {
    let mut result = ScanResult::default();
    for p in paths {
        match Target::new(p) {
            Ok(t) => result.targets.push(t),
            Err(e) => result.notes.push(format!("Can't open {e}")),
        }
    }
    if result.targets.is_empty() {
        return result;
    }
    let targets = result.targets.clone();
    let covered = |key: &str| targets.iter().any(|t| t.covers(key));

    let names = procs::process_names();
    let me = std::process::id();
    let mut holders: HashMap<u32, Holder> = HashMap::new();
    let push_lock = |h: &mut Holder, lock: Lock| {
        if !h
            .locks
            .iter()
            .any(|l| l.path == lock.path && l.how == lock.how)
        {
            h.locks.push(lock);
        }
    };

    // 1. Open handles.
    match handles::find_open_handles(&covered) {
        Ok((found, warn)) => {
            for oh in found {
                let h = entry(&mut holders, &names, oh.pid);
                push_lock(
                    h,
                    Lock {
                        path: display_path(&oh.path),
                        how: LockHow::Handle(oh.handle_value),
                    },
                );
            }
            result.notes.extend(warn);
        }
        Err(e) => result.notes.push(format!("Handle scan unavailable: {e}")),
    }

    // 2. Loaded executable images.
    if targets
        .iter()
        .any(|t| t.is_dir || looks_executable(&t.display))
    {
        for &pid in names.keys() {
            if pid <= 4 || pid == me {
                continue;
            }
            for module in procs::loaded_modules(pid) {
                if covered(&to_key(&module)) {
                    let h = entry(&mut holders, &names, pid);
                    push_lock(
                        h,
                        Lock {
                            path: module,
                            how: LockHow::Module,
                        },
                    );
                }
            }
        }
    }

    // 3. Restart Manager, per target.
    for t in &targets {
        let files = if t.is_dir {
            let mut files = Vec::new();
            if !walk_files(Path::new(&t.display), RM_FOLDER_FILE_LIMIT, &mut files) {
                result.notes.push(format!(
                    "{} has more than {RM_FOLDER_FILE_LIMIT} files; deep file checks were limited to the first {RM_FOLDER_FILE_LIMIT}.",
                    t.display
                ));
            }
            files
        } else {
            vec![t.display.clone()]
        };
        match rstmgr::who_uses(&files) {
            Ok(list) => {
                for rp in list {
                    if rp.pid == me {
                        continue;
                    }
                    let h = entry(&mut holders, &names, rp.pid);
                    if !rp.app_name.is_empty() {
                        h.app_name = Some(rp.app_name);
                    }
                    if rp.service.is_some() {
                        h.service = rp.service;
                    }
                    h.kind = rp.kind;
                    if h.locks.is_empty() {
                        let path = if t.is_dir {
                            format!("{} (a file inside)", t.display)
                        } else {
                            t.display.clone()
                        };
                        push_lock(
                            h,
                            Lock {
                                path,
                                how: LockHow::InUse,
                            },
                        );
                    }
                }
            }
            Err(e) => result
                .notes
                .push(format!("Restart Manager check failed: {e}")),
        }
    }

    let mut list: Vec<Holder> = holders.into_values().collect();
    for h in &mut list {
        h.locks.sort_by_key(|a| a.path.to_lowercase());
    }
    list.sort_by(|a, b| {
        a.title()
            .to_lowercase()
            .cmp(&b.title().to_lowercase())
            .then(a.pid.cmp(&b.pid))
    });
    result.holders = list;
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    fn target(key: &str, is_dir: bool) -> Target {
        Target {
            display: String::new(),
            key: key.to_lowercase(),
            is_dir,
        }
    }

    #[test]
    fn folder_covers_children_but_not_siblings() {
        let t = target(r"\\?\C:\Work\App", true);
        assert!(t.covers(r"\\?\c:\work\app"));
        assert!(t.covers(r"\\?\c:\work\app\src\main.rs"));
        assert!(!t.covers(r"\\?\c:\work\app2\main.rs"));
        assert!(!t.covers(r"\\?\c:\work"));
    }

    #[test]
    fn file_covers_only_itself() {
        let t = target(r"\\?\C:\a.txt", false);
        assert!(t.covers(r"\\?\c:\a.txt"));
        assert!(!t.covers(r"\\?\c:\a.txt\x"));
    }

    #[test]
    fn keys_normalise_dos_and_unc() {
        assert_eq!(to_key(r"C:\X\y.dll"), r"\\?\c:\x\y.dll");
        assert_eq!(to_key(r"\\srv\share\a"), r"\\?\unc\srv\share\a");
        assert_eq!(to_key(r"\\?\D:\Q"), r"\\?\d:\q");
    }
}
