#![windows_subsystem = "windows"]

use eframe;
use eframe::egui;
use eframe::egui::{Context, ViewportCommand};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender};
use std::sync::Arc;
use sysinfo::{Pid, ProcessStatus, ProcessesToUpdate, System};
use tokio::runtime::Runtime;
use tokio::time::{sleep, Duration, Instant};
use tokio_util::sync::CancellationToken;

struct ProcessApp {
    duration_text: String,
    paused: Arc<AtomicBool>,
    cancellation_token: Option<CancellationToken>,
    // Sender/Receiver for async notifications.
    tx: Sender<String>,
    rx: Receiver<String>,

    tracked_process: Option<Pid>,
    tracked_process_name: String,
    system: System,

    process_window_open: bool,

    process_filter: String,
    filtered_processes: HashMap<Pid, String>,
}

async fn create_process_watcher(
    pid: Pid,
    paused_param: Arc<AtomicBool>,
    cancellation_token: CancellationToken,
    transmitter: Sender<String>,
) {
    const REFRESH_DURATION: Duration = Duration::from_millis(200);
    let mut system = System::new_all();
    let mut last_update = Instant::now();
    let mut last_paused = false;

    let mut process_duration = Duration::default();

    while !cancellation_token.is_cancelled() {
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
                    process_duration += now.duration_since(last_update);

                    let total_secs = process_duration.as_secs();
                    let hours = total_secs / 3600;
                    let minutes = (total_secs % 3600) / 60;
                    let seconds = total_secs % 60;

                    let _ = transmitter
                        .send(format!("{:02}:{:02}:{:02}", hours, minutes, seconds).to_string());

                    last_update = now; // Update the last_update timestamp
                }
            } else {
                break; // Exit the loop if the process no longer exists
            }
        }

        // Sleep to prevent CPU overuse
        sleep(REFRESH_DURATION).await;
    }
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
        let (tx, rx) = std::sync::mpsc::channel();

        ProcessApp {
            duration_text: String::default(),
            paused: Arc::new(AtomicBool::new(false)),
            cancellation_token: None,
            tx,
            rx,

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
                            if let Some(token) = &self.cancellation_token.take() {
                                token.cancel();
                            }

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
                            let paused = self.paused.clone();
                            let tx = self.tx.clone();

                            let cancellation_token = CancellationToken::new();
                            let token_clone = cancellation_token.clone();
                            self.cancellation_token = Some(cancellation_token);

                            tokio::spawn(async move {
                                create_process_watcher(pid, paused, token_clone, tx).await;
                            });

                            return;
                        }
                    }
                }
            });
    }
}

impl eframe::App for ProcessApp {
    fn update(&mut self, ctx: &Context, _frame: &mut eframe::Frame) {
        while let Ok(duration) = self.rx.try_recv() {
            self.duration_text = duration;
        }

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
            ui.label(
                egui::RichText::new(self.duration_text.as_str())
                    .size(48f32)
                    .monospace(),
            );
        });
    }
}

fn main() {
    let rt = Runtime::new().expect("Unable to create Runtime");

    // Enter the runtime so that `tokio::spawn` is available immediately.
    let _enter = rt.enter();

    // Execute the runtime in its own thread.
    // The future doesn't have to do anything. In this example, it just sleeps forever.
    std::thread::spawn(move || {
        rt.block_on(async {
            loop {
                tokio::time::sleep(Duration::from_secs(3600)).await;
                tokio::task::yield_now().await;
            }
        })
    });

    let native_options = eframe::NativeOptions::default();

    let _result = eframe::run_native(
        "Time Tracker",
        native_options,
        Box::new(|cc| Ok(Box::new(ProcessApp::new(cc)))),
    );
}