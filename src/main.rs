//! Unlatch — find and release whatever is locking a file or folder.
//!
//! Usage:
//!   unlatch [PATH...]     open the window for these paths (Explorer passes one)
//!   unlatch --install     add "Unlock with Unlatch" to the right-click menu
//!   unlatch --uninstall   remove it again
//!   unlatch --list PATH   print holders to the console (for scripts/debugging)

#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod handles;
mod procs;
mod rstmgr;
mod scan;
mod shell;
mod win;

use std::path::PathBuf;

use eframe::egui;
use windows_sys::Win32::UI::WindowsAndMessaging::{
    MB_ICONERROR, MB_ICONINFORMATION, MB_OK, MessageBoxW,
};

fn message_box(text: &str, error: bool) {
    let t = win::wide(text);
    let c = win::wide("Unlatch");
    let icon = if error {
        MB_ICONERROR
    } else {
        MB_ICONINFORMATION
    };
    unsafe { MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | icon) };
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();

    match args.first().map(String::as_str) {
        Some("--install") => {
            match shell::install() {
                Ok(done) => message_box(&done.summary(), false),
                Err(e) => message_box(&format!("Install failed: {e}"), true),
            }
            return;
        }
        Some("--uninstall") => {
            match shell::uninstall() {
                Ok(()) => message_box("Unlatch has been removed.", false),
                Err(e) => message_box(&format!("Uninstall finished with a problem: {e}"), true),
            }
            return;
        }
        Some("--list") => {
            list_to_console(&args[1..]);
            return;
        }
        _ => {}
    }

    if procs::is_elevated() {
        procs::enable_debug_privilege();
    }

    // Explorer may pass a trailing quote on drive roots ("C:\"" → `C:"`).
    let paths: Vec<PathBuf> = args
        .iter()
        .map(|a| PathBuf::from(a.trim_end_matches('"')))
        .collect();

    let icon = egui::IconData {
        rgba: app::ICON_RGBA.to_vec(),
        width: app::ICON_SIZE,
        height: app::ICON_SIZE,
    };
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("Unlatch")
            .with_inner_size([760.0, 520.0])
            .with_min_inner_size([520.0, 360.0])
            .with_drag_and_drop(true)
            .with_icon(icon),
        ..Default::default()
    };
    if let Err(e) = eframe::run_native(
        "Unlatch",
        options,
        Box::new(|cc| Ok(Box::new(app::UnlatchApp::new(cc, paths)))),
    ) {
        message_box(&format!("Unlatch could not start: {e}"), true);
    }
}

/// Console mode. Attach to the parent console so output shows up even in the
/// release build (which has no console of its own).
fn list_to_console(args: &[String]) {
    unsafe {
        windows_sys::Win32::System::Console::AttachConsole(
            windows_sys::Win32::System::Console::ATTACH_PARENT_PROCESS,
        )
    };
    if procs::is_elevated() {
        procs::enable_debug_privilege();
    }
    let paths: Vec<PathBuf> = args.iter().map(PathBuf::from).collect();
    let started = std::time::Instant::now();
    let r = scan::scan(&paths);
    for note in &r.notes {
        println!("note: {note}");
    }
    if r.holders.is_empty() {
        println!("Nothing is using {}.", args.join(", "));
    }
    for h in &r.holders {
        println!(
            "{} (PID {}){}{}",
            h.title(),
            h.pid,
            h.service
                .as_ref()
                .map(|s| format!(" [service {s}]"))
                .unwrap_or_default(),
            if h.is_protected() { " [protected]" } else { "" }
        );
        for l in &h.locks {
            println!("    {:?}  {}", l.how, l.path);
        }
    }
    println!("({} ms)", started.elapsed().as_millis());
}
