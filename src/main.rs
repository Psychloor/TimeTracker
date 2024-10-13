#![windows_subsystem = "windows"]

use eframe;
use eframe::egui;
use eframe::egui::{Context, ViewportCommand};

use std::collections::HashMap;
use std::fmt::Write;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, RwLock};

use sysinfo::{Pid, ProcessStatus, ProcessesToUpdate, System};
use tokio::runtime::Runtime;
use tokio::select;
use tokio::time::{interval, Duration, Instant};

use tokio_util::sync::CancellationToken;

const REFRESH_INTERVAL: Duration = Duration::from_millis(200);

async fn process_watcher_async(
    pid: Pid,
    paused: Arc<AtomicBool>,
    duration_text: Arc<RwLock<String>>,
    cancellation_token: CancellationToken,
    ctx: Context,
) {
    let mut system = System::new_all();
    let mut last_update = Instant::now();
    let mut last_seconds: u64 = 0;

    let mut process_duration = Duration::default();
    let mut update_interval = interval(REFRESH_INTERVAL);

    loop {
        select! {
            biased;
            _ = cancellation_token.cancelled() => {
                return;
            },
            _ = update_interval.tick() => {
                if !paused.load(Ordering::Relaxed) {
                    system.refresh_processes(ProcessesToUpdate::Some(&[pid]), true);

                    if let Some(process) = system.process(pid) {
                        if process.status() == ProcessStatus::Run {
                            let now = Instant::now();
                            process_duration += now.saturating_duration_since(last_update);
                            last_update = now;

                            let total_secs = process_duration.as_secs();

                            let hours = total_secs / 3600;
                            let minutes = (total_secs % 3600) / 60;
                            let seconds = total_secs % 60;

                            if let Ok(mut duration_txt) = duration_text.write() {
                                duration_txt.clear();
                                if let Err(e) = write!(&mut duration_txt, "{:02}:{:02}:{:02}", hours, minutes, seconds) {
                                    eprintln!("Failed to write duration: {}", e);
                                }
                            }

                            if total_secs != last_seconds {
                                ctx.request_repaint();
                            }
                            last_seconds = total_secs;
                        }
                    } else {
                        return; // Exit the loop if the process no longer exists
                    }
                } else {
                    // Tracking is paused, reset the last update to avoid accumulating paused time
                    last_update = Instant::now();
                }
            }
        }
    }
}

struct ProcessApp {
    duration_text: Arc<RwLock<String>>,
    paused: Arc<AtomicBool>,
    cancellation_token: Option<CancellationToken>,

    tracked_process: Option<Pid>,
    tracked_process_name: String,
    system: System,

    process_window_open: bool,
    process_filter: String,
    filtered_processes: HashMap<Pid, String>,
}

impl Drop for ProcessApp {
    fn drop(&mut self) {
        if let Some(token) = &self.cancellation_token {
            token.cancel();
        }
    }
}

impl ProcessApp {
    fn new(_cc: &eframe::CreationContext<'_>) -> Self {
        ProcessApp {
            duration_text: Arc::new(RwLock::new(String::from("--:--:--"))),
            paused: Arc::new(AtomicBool::new(false)),
            cancellation_token: None,

            tracked_process: None,
            tracked_process_name: String::default(),
            process_window_open: false,
            system: System::new_all(),

            process_filter: String::default(),
            filtered_processes: HashMap::default(),
        }
    }

    fn stop_and_join_thread(&mut self) {
        if let Some(token) = self.cancellation_token.take() {
            token.cancel();
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
        let mut is_window_open = self.process_window_open;
        let mut should_close_window = false;

        egui::Window::new("Select Process")
            .title_bar(true)
            .movable(true)
            .collapsible(false)
            .scroll([false, true])
            .open(&mut is_window_open)
            .resizable(false)
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
                            if let Some(token) = &self.cancellation_token.take() {
                                token.cancel();
                            }
                            self.paused.store(false, Ordering::Release);

                            self.tracked_process = Some(pid);
                            self.tracked_process_name = process.clone();
                            should_close_window = true;

                            ctx.send_viewport_cmd(ViewportCommand::Title(format!(
                                "Time Tracker - {}",
                                process
                            )));

                            // Cloning values to move into the new thread safely
                            let paused = self.paused.clone();
                            let duration = self.duration_text.clone();
                            let context = ctx.clone();

                            let cancellation_token = CancellationToken::new();
                            let token_clone = cancellation_token.clone();
                            self.cancellation_token = Some(cancellation_token);

                            tokio::spawn(async move {
                                process_watcher_async(pid, paused, duration, token_clone, context)
                                    .await;
                            });

                            break;
                        }
                    }
                }
            });

        self.process_window_open = is_window_open;
        if should_close_window {
            self.process_window_open = false;
        }
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

                let paused = self.paused.load(Ordering::Relaxed);
                let paused_text = if paused { "Un-Pause" } else { "Pause" };
                if ui_hor.button(paused_text).clicked() {
                    self.paused.store(!paused, Ordering::Release);
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
            if let Ok(duration) = self.duration_text.read() {
                ui.label(
                    egui::RichText::new(duration.as_str())
                        .size(48f32)
                        .monospace(),
                );
            }
        });
    }
}

fn main() {
    let rt = Runtime::new().expect("Unable to create Runtime");
    let exit_process_token = CancellationToken::new();
    let exit_process_clone = exit_process_token.clone();

    // Enter the runtime so that `tokio::spawn` is available immediately.
    let _enter = rt.enter();

    // Execute the runtime in its own thread.
    // The future doesn't have to do anything. In this example, it just sleeps forever.
    let rt_thread = std::thread::spawn(move || {
        rt.block_on(async move {
            exit_process_clone.cancelled().await;
        })
    });

    let native_options = eframe::NativeOptions::default();

    let _result = eframe::run_native(
        "Time Tracker",
        native_options,
        Box::new(|cc| Ok(Box::new(ProcessApp::new(cc)))),
    );

    exit_process_token.cancel();
    rt_thread.join().unwrap();
}