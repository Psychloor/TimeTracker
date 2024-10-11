#![windows_subsystem = "windows"]

use eframe;
use eframe::egui;
use eframe::egui::{Context, ViewportCommand};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant};
use sysinfo::{Pid, ProcessStatus, ProcessesToUpdate, System};

struct ProcessApp {
    tracked_duration: Arc<RwLock<Duration>>,
    duration_text: Arc<RwLock<String>>,
    paused: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
    thread: Option<thread::JoinHandle<()>>,

    tracked_process: Option<Pid>,
    tracked_process_name: String,
    system: System,

    process_window_open: bool,

    process_filter: String,
    filtered_processes: HashMap<Pid, String>,
}

fn create_process_watcher(
    pid: Pid,
    tracked_duration: Arc<RwLock<Duration>>,
    duration_text: Arc<RwLock<String>>,
    paused_param: Arc<AtomicBool>,
    stopped: Arc<AtomicBool>,
) {
    const REFRESH_DURATION: Duration = Duration::from_millis(200);
    let mut system = System::new_all();
    let mut last_update = Instant::now();
    let mut last_paused = false;

    // Initialize tracked_duration to default
    if let Ok(mut tracked_duration) = tracked_duration.write() {
        *tracked_duration = Duration::default();
    }

    while !stopped.load(Ordering::Acquire) {
        let paused = paused_param.load(Ordering::Acquire);
        if !paused && last_paused {
            last_update = Instant::now(); // Reset the update time when unpausing
        }
        last_paused = paused;

        if !paused {
            system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

            if let Some(process) = system.process(pid) {
                if process.status() == ProcessStatus::Run {
                    let now = Instant::now();
                    let duration_since_last_update = now.duration_since(last_update);

                    // Update the tracked duration
                    if let Ok(mut duration_guard) = tracked_duration.write() {
                        *duration_guard += duration_since_last_update;

                        // Update the duration_text in a separate scope to reduce lock contention
                        if let Ok(mut duration_text_guard) = duration_text.write() {
                            let total_secs = duration_guard.as_secs();
                            let hours = total_secs / 3600;
                            let minutes = (total_secs % 3600) / 60;
                            let seconds = total_secs % 60;
                            *duration_text_guard =
                                format!("{:02}:{:02}:{:02}", hours, minutes, seconds);
                        }
                    }

                    last_update = now; // Update the last_update timestamp
                }
            } else {
                break; // Exit the loop if the process no longer exists
            }
        }

        // Sleep to prevent CPU overuse
        thread::sleep(REFRESH_DURATION);
    }
}

impl Drop for ProcessApp {
    fn drop(&mut self) {
        self.stopped.store(true, Ordering::SeqCst);
        if self.thread.is_some() {
            self.thread.take().unwrap().join().unwrap();
        }
    }
}

impl ProcessApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        ProcessApp {
            tracked_duration: Arc::new(RwLock::new(Duration::default())),
            duration_text: Arc::new(RwLock::new(String::default())),
            paused: Arc::new(AtomicBool::new(false)),
            stopped: Arc::new(AtomicBool::new(false)),
            thread: None,

            tracked_process: None,
            tracked_process_name: String::default(),
            process_window_open: false,
            system: System::new_all(),

            process_filter: String::default(),
            filtered_processes: HashMap::default(),
        }
    }

    fn stop_and_join_thread(&mut self) {
        // Set the stop signal
        self.stopped.store(true, Ordering::SeqCst);

        // Check if there is a thread to join
        if let Some(thread) = self.thread.take() {
            if let Err(e) = thread.join() {
                // Handle the error if the thread panicked
                eprintln!("Failed to join thread: {:?}", e);
            }
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
                self.stop_and_join_thread();

                // List of processes
                for (&pid, process) in self.filtered_processes.iter_mut() {
                    if ui.selectable_label(false, process.as_str()).clicked() {
                        if self.tracked_process != Some(pid) {
                            self.stopped.store(false, Ordering::Relaxed);
                            self.paused.store(false, Ordering::Relaxed);

                            self.tracked_process = Some(pid);
                            self.tracked_process_name = process.clone();
                            self.process_window_open = false;

                            ctx.send_viewport_cmd(ViewportCommand::Title(format!(
                                "Time Tracker - {}",
                                process
                            )));

                            self.filtered_processes.clear();

                            // Cloning values to move into the new thread safely
                            let tracked_duration = self.tracked_duration.clone();
                            let duration_text = self.duration_text.clone();
                            let paused = self.paused.clone();
                            let stopped = self.stopped.clone();

                            self.thread = Some(thread::spawn(move || {
                                create_process_watcher(
                                    pid,
                                    tracked_duration,
                                    duration_text,
                                    paused,
                                    stopped,
                                );
                            }));

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
                    let paus = self.paused.load(Ordering::SeqCst);
                    self.paused.store(!paus, Ordering::Release);
                }

                if ui_hor.button("Stop").clicked() {
                    self.tracked_process = None;
                    self.tracked_process_name = String::default();
                    self.stop_and_join_thread();
                    ctx.send_viewport_cmd(ViewportCommand::Title("Time Tracker".to_string()));
                }

                if self.process_window_open {
                    self.open_process_list_window(ctx);
                }
            });
        });
        egui::CentralPanel::default().show(ctx, |ui| {
            let mut duration_string = String::default();
            if let Ok(duration_text) = self.duration_text.read() {
                duration_string = duration_text.to_string();
            }

            ui.label(egui::RichText::new(duration_string).size(48f32).monospace());
        });
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