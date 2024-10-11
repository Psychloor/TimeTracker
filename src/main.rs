#![windows_subsystem = "windows"]

use eframe;
use eframe::egui;
use eframe::egui::{Context, ViewportCommand};
use std::collections::HashMap;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessStatus, ProcessesToUpdate, System};

const REFRESH_DURATION: Duration = Duration::from_millis(100);

struct ProcessApp {
    tracked_duration: Duration,
    duration_text: String,
    last_update: Instant,

    tracked_process: Option<Pid>,
    tracked_process_name: String,
    system: System,

    paused: bool,
    process_window_open: bool,

    process_filter: String,
    filtered_processes: HashMap<Pid, String>,
}

impl ProcessApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        ProcessApp {
            tracked_duration: Duration::default(),
            duration_text: String::default(),
            last_update: Instant::now(),
            tracked_process: None,
            tracked_process_name: String::default(),
            process_window_open: false,
            system: System::new_all(),
            paused: false,
            process_filter: String::default(),
            filtered_processes: HashMap::default(),
        }
    }

    fn filter_processes(&mut self) {
        self.filtered_processes = self
            .system
            .processes()
            .iter()
            .filter(|(_, pro)| {
                if self.process_filter.is_empty() {
                    true
                } else {
                    pro.name()
                        .to_str()
                        .unwrap_or_default()
                        .contains(self.process_filter.as_str())
                }
            })
            .map(|(&p, process)| (p, process.name().to_str().unwrap_or_default().to_string()))
            .collect();
    }

    fn open_process_list_window(&mut self, ctx: &Context) {
        egui::Window::new("Select Process")
            .title_bar(true)
            .movable(true)
            .collapsible(false)
            .scroll([false, true])
            .show(ctx, |ui| {
                // Filter textbox
                if ui
                    .add(
                        egui::TextEdit::singleline(&mut self.process_filter)
                            .hint_text("Name Filter..."),
                    )
                    .changed()
                {
                    self.filter_processes();
                }

                ui.separator();

                // List of processes
                for (&pid, process) in self.filtered_processes.iter_mut() {
                    if ui.selectable_label(false, process.as_str()).clicked() {
                        if self.tracked_process != Some(pid) {
                            self.last_update = Instant::now();
                            self.tracked_process = Some(pid);
                            self.tracked_process_name = process.clone();
                            self.process_window_open = false;
                            self.tracked_duration = Duration::from_secs(0);

                            ctx.send_viewport_cmd(ViewportCommand::Title(format!(
                                "Time Tracker - {}",
                                process
                            )));

                            self.filtered_processes.clear();
                            return;
                        }
                    }
                }
            });
    }
}

impl eframe::App for ProcessApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        egui::TopBottomPanel::top("Process").show(ctx, |ui| {
            ui.horizontal(|ui_hor| {
                ui_hor.label(format!("Selected Process: {}", self.tracked_process_name));
                ui_hor.add_space(32f32);
                if ui_hor.button("Select").clicked() {
                    self.system.refresh_processes(ProcessesToUpdate::All, true);
                    self.process_filter.clear();
                    self.filter_processes();
                    self.process_window_open = true;
                }

                if ui_hor.button("Pause").clicked() {
                    self.paused = !self.paused;
                    if !self.paused {
                        self.last_update = Instant::now();
                    }
                }

                if ui_hor.button("Stop").clicked() {
                    self.tracked_process = None;
                    self.tracked_process_name = String::default();
                    ctx.send_viewport_cmd(ViewportCommand::Title("Time Tracker".to_string()));
                }

                if self.process_window_open {
                    self.open_process_list_window(ctx);
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            ui.label(
                egui::RichText::new(&self.duration_text)
                    .size(48f32)
                    .monospace(),
            );
        });

        if let Some(pid) = self.tracked_process {
            if !self.paused {
                self.system
                    .refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

                if let Some(process) = self.system.process(pid) {
                    if process.status() == ProcessStatus::Run {
                        let now = Instant::now();
                        self.tracked_duration += now.duration_since(self.last_update);
                        self.last_update = now;

                        let duration = self.tracked_duration.as_secs();
                        let seconds = duration % 60;
                        let minutes = duration / 60;
                        let hours = duration / 3600;
                        self.duration_text =
                            format!("{:02}:{:02}:{:02}", hours, minutes, seconds).to_string();
                    }
                }

                ctx.request_repaint_after(REFRESH_DURATION);
            }
        }
    }
}

fn main() {
    let native_options = eframe::NativeOptions::default();

    let _result = eframe::run_native(
        "Time Tracker",
        native_options,
        Box::new(|cc| Ok(Box::new(ProcessApp::new(cc)))),
    );
}