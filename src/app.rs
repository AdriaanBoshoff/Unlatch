//! The window: shows what is locking the chosen paths and offers to end those
//! processes, close individual handles, or send the item to the Recycle Bin.

use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::mpsc;

use eframe::egui::{
    self, Align, Color32, CornerRadius, Frame, Layout, Margin, RichText, ScrollArea, Stroke,
    TextureHandle, Vec2,
};

use crate::scan::{self, Holder, LockHow, ScanResult};
use crate::{procs, shell};

pub const ICON_RGBA: &[u8] = include_bytes!("../assets/unlatch_128.rgba");
pub const ICON_SIZE: u32 = 128;

const DANGER: Color32 = Color32::from_rgb(220, 53, 69);
const OK_GREEN: Color32 = Color32::from_rgb(25, 160, 95);
const ACCENT: Color32 = Color32::from_rgb(79, 110, 235);

/// Work done off the UI thread; always ends with a fresh scan.
struct TaskOutput {
    messages: Vec<Message>,
    scan: Option<ScanResult>,
    deleted: bool,
}

#[derive(Clone)]
struct Message {
    text: String,
    error: bool,
}

enum Confirm {
    EndProcesses(Vec<u32>),
    CloseHandle {
        pid: u32,
        handle: usize,
        path: String,
    },
    Delete,
}

pub struct UnlatchApp {
    paths: Vec<PathBuf>,
    /// Remembered when the paths are chosen, so labels survive a delete.
    path_is_dir: Vec<bool>,
    result: Option<ScanResult>,
    task: Option<mpsc::Receiver<TaskOutput>>,
    busy_label: &'static str,
    messages: Vec<Message>,
    confirm: Option<Confirm>,
    elevated: bool,
    installed: bool,
    deleted: bool,
    logo: Option<TextureHandle>,
    expanded: HashSet<u32>,
}

impl UnlatchApp {
    pub fn new(cc: &eframe::CreationContext<'_>, paths: Vec<PathBuf>) -> Self {
        style(&cc.egui_ctx);
        let logo = cc.egui_ctx.load_texture(
            "logo",
            egui::ColorImage::from_rgba_unmultiplied([ICON_SIZE as usize; 2], ICON_RGBA),
            egui::TextureOptions::LINEAR,
        );
        let mut app = Self {
            paths: Vec::new(),
            path_is_dir: Vec::new(),
            result: None,
            task: None,
            busy_label: "",
            messages: Vec::new(),
            confirm: None,
            elevated: procs::is_elevated(),
            installed: shell::is_installed(),
            deleted: false,
            logo: Some(logo),
            expanded: HashSet::new(),
        };
        if !paths.is_empty() {
            app.set_paths(paths, &cc.egui_ctx);
        }
        app
    }

    fn set_paths(&mut self, paths: Vec<PathBuf>, ctx: &egui::Context) {
        self.path_is_dir = paths.iter().map(|p| p.is_dir()).collect();
        self.paths = paths;
        self.result = None;
        self.deleted = false;
        self.messages.clear();
        self.expanded.clear();
        self.rescan(ctx);
    }

    fn rescan(&mut self, ctx: &egui::Context) {
        self.run("Looking for programs using this…", ctx, |_| {
            (Vec::new(), false)
        });
    }

    /// Run `work` on a background thread, then rescan (unless it deleted the
    /// target), and wake the UI when done.
    fn run(
        &mut self,
        label: &'static str,
        ctx: &egui::Context,
        work: impl FnOnce(&[PathBuf]) -> (Vec<Message>, bool) + Send + 'static,
    ) {
        let (tx, rx) = mpsc::channel();
        let paths = self.paths.clone();
        let ctx = ctx.clone();
        std::thread::spawn(move || {
            let (messages, deleted) = work(&paths);
            let scan = (!deleted).then(|| scan::scan(&paths));
            let _ = tx.send(TaskOutput {
                messages,
                scan,
                deleted,
            });
            ctx.request_repaint();
        });
        self.task = Some(rx);
        self.busy_label = label;
    }

    fn poll_task(&mut self) {
        let Some(rx) = &self.task else { return };
        if let Ok(out) = rx.try_recv() {
            self.task = None;
            self.messages = out.messages;
            if out.deleted {
                self.deleted = true;
                self.result = None;
            } else if let Some(scan) = out.scan {
                for note in &scan.notes {
                    self.messages.push(Message {
                        text: note.clone(),
                        error: false,
                    });
                }
                self.result = Some(scan);
            }
        }
    }

    fn holder(&self, pid: u32) -> Option<&Holder> {
        self.result.as_ref()?.holders.iter().find(|h| h.pid == pid)
    }

    fn end_processes(&mut self, pids: Vec<u32>, ctx: &egui::Context) {
        let explorer: HashSet<u32> = pids
            .iter()
            .copied()
            .filter(|p| self.holder(*p).is_some_and(|h| h.is_explorer()))
            .collect();
        self.run("Ending processes…", ctx, move |_| {
            let mut msgs = Vec::new();
            for pid in &pids {
                match procs::terminate(*pid) {
                    Ok(()) => msgs.push(Message {
                        text: format!("Ended process {pid}."),
                        error: false,
                    }),
                    Err(e) => msgs.push(Message {
                        text: e,
                        error: true,
                    }),
                }
            }
            if !explorer.is_empty() {
                shell::ensure_explorer_running();
            }
            (msgs, false)
        });
    }

    fn close_handle(&mut self, pid: u32, handle: usize, ctx: &egui::Context) {
        self.run("Closing handle…", ctx, move |_| {
            let msg = match procs::close_remote_handle(pid, handle) {
                Ok(()) => Message {
                    text: format!("Closed handle 0x{handle:X} in process {pid}."),
                    error: false,
                },
                Err(e) => Message {
                    text: e,
                    error: true,
                },
            };
            (vec![msg], false)
        });
    }

    fn delete(&mut self, ctx: &egui::Context) {
        let targets: Vec<String> = self
            .result
            .as_ref()
            .map(|r| r.targets.iter().map(|t| t.display.clone()).collect())
            .unwrap_or_default();
        self.run(
            "Moving to the Recycle Bin…",
            ctx,
            move |_| match shell::recycle(&targets) {
                Ok(()) => (
                    vec![Message {
                        text: "Moved to the Recycle Bin.".into(),
                        error: false,
                    }],
                    true,
                ),
                Err(e) => (
                    vec![Message {
                        text: e,
                        error: true,
                    }],
                    false,
                ),
            },
        );
    }
}

fn style(ctx: &egui::Context) {
    ctx.all_styles_mut(|s| {
        s.spacing.item_spacing = Vec2::new(8.0, 8.0);
        s.spacing.button_padding = Vec2::new(12.0, 6.0);
        s.visuals.widgets.inactive.corner_radius = CornerRadius::same(6);
        s.visuals.widgets.hovered.corner_radius = CornerRadius::same(6);
        s.visuals.widgets.active.corner_radius = CornerRadius::same(6);
        s.visuals.selection.bg_fill = ACCENT;
        if let Some(small) = s.text_styles.get_mut(&egui::TextStyle::Small) {
            small.size = 11.5;
        }
    });
}

fn danger_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(Color32::WHITE))
            .fill(DANGER)
            .corner_radius(6),
    )
}

fn primary_button(ui: &mut egui::Ui, text: &str, enabled: bool) -> egui::Response {
    ui.add_enabled(
        enabled,
        egui::Button::new(RichText::new(text).color(Color32::WHITE))
            .fill(ACCENT)
            .corner_radius(6),
    )
}

fn card(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    let v = ui.visuals();
    Frame::new()
        .fill(v.faint_bg_color)
        .stroke(Stroke::new(1.0, v.widgets.noninteractive.bg_stroke.color))
        .corner_radius(10)
        .inner_margin(Margin::same(12))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            add(ui);
        });
}

impl eframe::App for UnlatchApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        self.poll_task();
        let busy = self.task.is_some();

        // Drag & drop a file/folder onto the window to inspect it.
        let dropped: Vec<PathBuf> = ctx.input(|i| {
            i.raw
                .dropped_files
                .iter()
                .map(|f| f.path().to_path_buf())
                .filter(|p| !p.as_os_str().is_empty())
                .collect()
        });
        if !dropped.is_empty() && !busy {
            self.set_paths(dropped, &ctx);
        }

        egui::Panel::top("header").frame(Frame::side_top_panel(ui.style()).inner_margin(Margin::symmetric(16, 12))).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(Vec2::splat(36.0)));
                    }
                    ui.vertical(|ui| {
                        ui.label(RichText::new("Unlatch").size(20.0).strong());
                        ui.label(RichText::new("See what's using a file or folder, and set it free").weak());
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        if !self.paths.is_empty() && !self.deleted && ui.add_enabled(!busy, egui::Button::new("Rescan")).clicked() {
                            self.rescan(&ctx);
                        }
                        if !self.elevated
                            && ui
                                .button("Run as administrator")
                                .on_hover_text("Restart with admin rights to see and end processes from services and other users.")
                                .clicked()
                            && shell::relaunch_elevated(&self.paths)
                        {
                            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                        }
                    });
                });
            },
        );

        egui::Panel::bottom("footer").frame(Frame::side_top_panel(ui.style()).inner_margin(Margin::symmetric(16, 10))).show(
            ui,
            |ui| {
                ui.horizontal(|ui| {
                    let label = if self.installed { "Remove from right-click menu" } else { "Add to right-click menu" };
                    if ui.add_enabled(!busy, egui::Button::new(label)).clicked() {
                        let res = if self.installed { shell::uninstall().map(|_| "Removed from the right-click menu.".to_string()) } else {
                            shell::install().map(|_| "Added to the right-click menu. Right-click any file or folder, then Show more options > Unlock with Unlatch.".to_string())
                        };
                        self.messages = vec![match res {
                            Ok(t) => Message { text: t, error: false },
                            Err(e) => Message { text: e, error: true },
                        }];
                        self.installed = shell::is_installed();
                    }
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        let has_targets = self.result.as_ref().is_some_and(|r| !r.targets.is_empty());
                        if has_targets && !self.deleted {
                            if danger_button(ui, "Move to Recycle Bin", !busy).clicked() {
                                self.confirm = Some(Confirm::Delete);
                            }
                            let killable: Vec<u32> = self
                                .result
                                .as_ref()
                                .map(|r| r.holders.iter().filter(|h| !h.is_protected()).map(|h| h.pid).collect())
                                .unwrap_or_default();
                            if killable.len() > 1
                                && ui.add_enabled(!busy, egui::Button::new(format!("End all ({})", killable.len()))).clicked()
                            {
                                self.confirm = Some(Confirm::EndProcesses(killable));
                            }
                        }
                    });
                });
            },
        );

        egui::CentralPanel::default_margins().show(ui, |ui| {
            if !self.elevated && self.result.as_ref().is_some_and(|r| !r.targets.is_empty()) {
                ui.label(
                    RichText::new("Not running as administrator — programs from services or other users may not be listed.")
                        .weak(),
                );
            }
            for m in &self.messages {
                let color = if m.error { DANGER } else { ui.visuals().text_color() };
                ui.label(RichText::new(&m.text).color(color));
            }

            if self.paths.is_empty() {
                self.empty_state(ui, &ctx);
                return;
            }

            // Targets
            for (p, is_dir) in self.paths.iter().zip(&self.path_is_dir) {
                ui.horizontal(|ui| {
                    let kind = if *is_dir { "FOLDER" } else { "FILE" };
                    ui.label(RichText::new(kind).small().strong().color(ACCENT));
                    ui.label(RichText::new(p.display().to_string()).monospace());
                });
            }
            ui.separator();

            if busy {
                ui.horizontal(|ui| {
                    ui.spinner();
                    ui.label(self.busy_label);
                });
                return;
            }
            if self.deleted {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    ui.label(RichText::new("Done").size(22.0).strong().color(OK_GREEN));
                    ui.label(RichText::new("It's in the Recycle Bin.").size(16.0));
                });
                return;
            }
            let Some(result) = &self.result else { return };
            if result.targets.is_empty() {
                return;
            }
            if result.holders.is_empty() {
                ui.vertical_centered(|ui| {
                    ui.add_space(30.0);
                    if let Some(logo) = &self.logo {
                        ui.add(egui::Image::new(logo).fit_to_exact_size(Vec2::splat(64.0)));
                    }
                    ui.label(RichText::new("Nothing is using this").size(18.0).strong().color(OK_GREEN));
                    ui.label(RichText::new("You should be able to rename, move or delete it now.").weak());
                });
                return;
            }

            ui.label(
                RichText::new(format!(
                    "{} {} using this",
                    result.holders.len(),
                    if result.holders.len() == 1 { "program is" } else { "programs are" }
                ))
                .strong(),
            );
            let holders = result.holders.clone();
            ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
                for h in &holders {
                    self.holder_card(ui, h);
                }
            });
        });

        self.confirm_dialog(&ctx);
    }
}

impl UnlatchApp {
    fn empty_state(&mut self, ui: &mut egui::Ui, ctx: &egui::Context) {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.label(
                RichText::new("Drop a file or folder here")
                    .size(20.0)
                    .strong(),
            );
            ui.label(
                RichText::new("or pick one to see which programs are holding on to it").weak(),
            );
            ui.add_space(12.0);
            ui.horizontal(|ui| {
                let w = 300.0;
                ui.add_space((ui.available_width() - w).max(0.0) / 2.0);
                if primary_button(ui, "Choose file…", true).clicked()
                    && let Some(f) = rfd::FileDialog::new().pick_file()
                {
                    self.set_paths(vec![f], ctx);
                }
                if primary_button(ui, "Choose folder…", true).clicked()
                    && let Some(f) = rfd::FileDialog::new().pick_folder()
                {
                    self.set_paths(vec![f], ctx);
                }
            });
            ui.add_space(24.0);
            if !self.installed {
                ui.label(
                    RichText::new(
                        "Tip: add Unlatch to Explorer's right-click menu using the button below.",
                    )
                    .weak(),
                );
            }
        });
    }

    fn holder_card(&mut self, ui: &mut egui::Ui, h: &Holder) {
        let busy = self.task.is_some();
        card(ui, |ui| {
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(h.title()).size(16.0).strong());
                        if h.title() != h.exe_name {
                            ui.label(RichText::new(&h.exe_name).weak());
                        }
                    });
                    ui.horizontal_wrapped(|ui| {
                        ui.label(RichText::new(format!("PID {}", h.pid)).small().weak());
                        if let Some(svc) = &h.service {
                            ui.label(
                                RichText::new(format!("· service: {svc}"))
                                    .small()
                                    .color(ACCENT),
                            );
                        }
                        if h.is_protected() {
                            ui.label(RichText::new("· system process").small().color(DANGER));
                        }
                    });
                    if let Some(p) = &h.exe_path {
                        ui.label(RichText::new(p).small().weak());
                    }
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    if danger_button(ui, "End process", !busy && !h.is_protected())
                        .on_disabled_hover_text("Windows does not allow this process to be ended.")
                        .clicked()
                    {
                        self.confirm = Some(Confirm::EndProcesses(vec![h.pid]));
                    }
                });
            });

            let open = self.expanded.contains(&h.pid);
            let n = h.locks.len();
            let label = format!(
                "{} {} {}",
                if open { "Hide" } else { "Show" },
                n,
                if n == 1 {
                    "locked item"
                } else {
                    "locked items"
                }
            );
            if ui
                .add(egui::Button::new(RichText::new(label).small().color(ACCENT)).frame(false))
                .clicked()
            {
                if open {
                    self.expanded.remove(&h.pid);
                } else {
                    self.expanded.insert(h.pid);
                }
            }
            if open {
                for lock in &h.locks {
                    ui.horizontal(|ui| {
                        let tag = match lock.how {
                            LockHow::Handle(_) => "open",
                            LockHow::Module => "running/loaded",
                            LockHow::InUse => "in use",
                        };
                        ui.label(RichText::new(format!("[{tag}]")).small().color(ACCENT));
                        ui.label(RichText::new(&lock.path).small().monospace());
                        if let LockHow::Handle(hv) = lock.how {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                if ui
                                    .add_enabled(!busy && !h.is_protected(), egui::Button::new(RichText::new("Close handle").small()))
                                    .on_hover_text("Release just this file without ending the program. The program may misbehave or crash.")
                                    .clicked()
                                {
                                    self.confirm =
                                        Some(Confirm::CloseHandle { pid: h.pid, handle: hv, path: lock.path.clone() });
                                }
                            });
                        }
                    });
                }
            }
        });
    }

    fn confirm_dialog(&mut self, ctx: &egui::Context) {
        let Some(confirm) = &self.confirm else { return };
        let (title, body, action) = match confirm {
            Confirm::EndProcesses(pids) => {
                let names: Vec<String> = pids
                    .iter()
                    .filter_map(|p| self.holder(*p))
                    .map(|h| format!("• {} (PID {})", h.title(), h.pid))
                    .collect();
                let mut body = format!(
                    "These will be closed immediately. Unsaved work in them will be lost.\n\n{}",
                    names.join("\n")
                );
                if pids
                    .iter()
                    .any(|p| self.holder(*p).is_some_and(|h| h.service.is_some()))
                {
                    body.push_str("\n\nOne or more of these is a Windows service; it may restart or other features may stop working until you reboot.");
                }
                if pids
                    .iter()
                    .any(|p| self.holder(*p).is_some_and(|h| h.is_explorer()))
                {
                    body.push_str("\n\nExplorer (your taskbar and desktop) will restart.");
                }
                ("End process?", body, "End process")
            }
            Confirm::CloseHandle { pid, path, .. } => (
                "Close this handle?",
                format!(
                    "Process {pid} will lose access to:\n{path}\n\nThe program is not told, so it may show errors, lose data or crash. Ending the process is usually safer."
                ),
                "Close handle",
            ),
            Confirm::Delete => {
                let names: Vec<String> = self
                    .result
                    .as_ref()
                    .map(|r| r.targets.iter().map(|t| t.display.clone()).collect())
                    .unwrap_or_default();
                (
                    "Move to the Recycle Bin?",
                    format!(
                        "{}\n\nYou can restore it from the Recycle Bin later.",
                        names.join("\n")
                    ),
                    "Move to Recycle Bin",
                )
            }
        };

        let mut decision: Option<bool> = None;
        egui::Modal::new(egui::Id::unique("unlatch-confirm")).show(ctx, |ui| {
            ui.set_max_width(460.0);
            ui.label(RichText::new(title).size(18.0).strong());
            ui.add_space(4.0);
            ui.label(body);
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if danger_button(ui, action, true).clicked() {
                    decision = Some(true);
                }
                if ui.button("Cancel").clicked() {
                    decision = Some(false);
                }
            });
        });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            decision = Some(false);
        }

        match decision {
            Some(true) => match self.confirm.take() {
                Some(Confirm::EndProcesses(pids)) => self.end_processes(pids, ctx),
                Some(Confirm::CloseHandle { pid, handle, .. }) => {
                    self.close_handle(pid, handle, ctx)
                }
                Some(Confirm::Delete) => self.delete(ctx),
                None => {}
            },
            Some(false) => self.confirm = None,
            None => {}
        }
    }
}
