use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui_code_editor::{CodeEditor, ColorTheme, Syntax};
use stitch_stub::pipe;

use crate::injector;
use crate::process::{self, PythonProcess};

enum ConnEvent {
    Connected,
    Failed(String),
    Output(String),
    Disconnected,
}

pub struct StitchApp {
    processes: Vec<PythonProcess>,
    selected_pid: Option<u32>,
    connected: bool,
    status: String,
    last_output: String,
    code: String,
    scripts_dir: PathBuf,
    scripts: Vec<PathBuf>,
    selected_script: Option<PathBuf>,
    new_script_name: Option<String>,
    cmd_tx: Option<Sender<String>>,
    events_rx: Option<Receiver<ConnEvent>>,
    connecting: bool,
    last_process_refresh: Instant,
    focus_new_script: bool,
    process_scan_rx: Option<Receiver<Vec<PythonProcess>>>,
}

const VSCODE_DARK_PLUS: ColorTheme = ColorTheme {
    name: "VS Code Dark+",
    dark: true,
    bg: "#1e1e1e",
    cursor: "#d4d4d4",
    selection: "#264f78",
    comments: "#7f848e",
    functions: "#dcdcaa",
    keywords: "#569cd6",
    literals: "#569cd6",
    numerics: "#b5cea8",
    punctuation: "#d4d4d4",
    strs: "#ce9178",
    types: "#4ec9b0",
    special: "#d4d4d4",
};

pub fn apply_theme(ctx: &egui::Context) {
    let mut visuals = egui::Visuals::dark();

    let bg = egui::Color32::from_rgb(24, 24, 24);
    let panel = egui::Color32::from_rgb(16, 16, 16);
    let accent = egui::Color32::from_rgb(59, 130, 246);

    visuals.window_fill = bg;
    visuals.panel_fill = panel;
    visuals.faint_bg_color = panel;
    visuals.extreme_bg_color = panel;

    visuals.widgets.noninteractive.bg_fill = panel;
    visuals.widgets.noninteractive.weak_bg_fill = panel;

    visuals.widgets.inactive.bg_fill = panel;
    visuals.widgets.inactive.weak_bg_fill = panel;

    visuals.widgets.hovered.bg_fill = accent;
    visuals.widgets.hovered.weak_bg_fill = accent;
    visuals.widgets.hovered.fg_stroke.color = egui::Color32::WHITE;

    visuals.widgets.active.bg_fill = accent;
    visuals.widgets.active.weak_bg_fill = accent;
    visuals.widgets.active.fg_stroke.color = egui::Color32::WHITE;

    visuals.selection.bg_fill = accent;

    ctx.set_visuals(visuals);
}

impl StitchApp {
    pub fn new() -> Self {
        let mut app = Self {
            processes: Vec::new(),
            selected_pid: None,
            connected: false,
            status: "not connected".to_string(),
            last_output: String::new(),
            code: String::new(),
            scripts_dir: PathBuf::from("./scripts"),
            scripts: Vec::new(),
            selected_script: None,
            new_script_name: None,
            cmd_tx: None,
            events_rx: None,
            connecting: false,
            last_process_refresh: Instant::now(),
            focus_new_script: false,
            process_scan_rx: None,
        };
        app.ensure_default_scripts();
        app.refresh_processes();
        app.refresh_scripts();
        app
    }

    fn ensure_default_scripts(&self) {
        const DUMP_IMPORTS_PY: &str = include_str!("../scripts/dump_imports.py");

        if !self.scripts_dir.exists() {
            let _ = std::fs::create_dir_all(&self.scripts_dir);
        }
        let dump_imports_path = self.scripts_dir.join("dump_imports.py");
        if !dump_imports_path.exists() {
            let _ = std::fs::write(&dump_imports_path, DUMP_IMPORTS_PY);
        }
    }

    fn refresh_processes(&mut self) {
        if self.process_scan_rx.is_some() {
            return;
        }
        let (tx, rx) = mpsc::channel();
        self.process_scan_rx = Some(rx);
        std::thread::spawn(move || {
            let _ = tx.send(process::find_python_processes());
        });
    }

    fn refresh_scripts(&mut self) {
        self.scripts.clear();
        let dir = self.scripts_dir.clone();
        if !dir.exists() {
            let _ = std::fs::create_dir_all(&dir);
        }
        if let Ok(entries) = std::fs::read_dir(&dir) {
            for entry in entries.flatten() {
                let path = entry.path();
                if path.extension().and_then(|e| e.to_str()) == Some("py") {
                    self.scripts.push(path);
                }
            }
        }
    }

    fn connect(&mut self) {
        if self.connecting || self.connected {
            return;
        }
        let Some(pid) = self.selected_pid else {
            self.status = "select a process first".to_string();
            return;
        };

        let (cmd_tx, cmd_rx) = mpsc::channel::<String>();
        let (event_tx, event_rx) = mpsc::channel::<ConnEvent>();
        self.cmd_tx = Some(cmd_tx);
        self.events_rx = Some(event_rx);
        self.connecting = true;
        self.status = "injecting...".to_string();

        std::thread::spawn(move || {
            let dll_path = match injector::extract_stub_dll() {
                Ok(path) => path,
                Err(e) => {
                    let _ = event_tx.send(ConnEvent::Failed(format!("extract stub dll: {e}")));
                    return;
                }
            };

            let inject_result = injector::inject(pid, &dll_path);
            let _ = std::fs::remove_file(&dll_path);

            if let Err(e) = inject_result {
                let _ = event_tx.send(ConnEvent::Failed(format!("inject failed: {e}")));
                return;
            }

            let conn = match pipe::server::wait_for_client() {
                Ok(c) => c,
                Err(e) => {
                    let _ = event_tx.send(ConnEvent::Failed(format!("pipe connect failed: {e}")));
                    return;
                }
            };

            match pipe::read_message(&conn) {
                Ok(Some(bytes)) => {
                    let msg = String::from_utf8_lossy(&bytes).into_owned();
                    if let Some(reason) = msg.strip_prefix("ERR:") {
                        let _ = event_tx.send(ConnEvent::Failed(format!(
                            "target process: {reason}"
                        )));
                        return;
                    }
                }
                _ => {
                    let _ = event_tx.send(ConnEvent::Failed(
                        "stub DLL closed the pipe before completing its handshake".to_string(),
                    ));
                    return;
                }
            }

            let _ = event_tx.send(ConnEvent::Connected);

            while let Ok(code) = cmd_rx.recv() {
                if pipe::write_message(&conn, code.as_bytes()).is_err() {
                    let _ = event_tx.send(ConnEvent::Disconnected);
                    break;
                }
                match pipe::read_message(&conn) {
                    Ok(Some(bytes)) => {
                        let out = String::from_utf8_lossy(&bytes).into_owned();
                        let _ = event_tx.send(ConnEvent::Output(out));
                    }
                    _ => {
                        let _ = event_tx.send(ConnEvent::Disconnected);
                        break;
                    }
                }
            }
        });
    }

    fn disconnect(&mut self) {
        self.cmd_tx = None;
        self.events_rx = None;
        self.connected = false;
        self.connecting = false;
        self.status = "not connected".to_string();
    }

    fn execute(&mut self) {
        match &self.cmd_tx {
            Some(tx) => {
                let _ = tx.send(self.code.clone());
                self.status = "sent".to_string();
            }
            None => self.status = "not connected".to_string(),
        }
    }

    fn poll_process_scan(&mut self) {
        let Some(rx) = &self.process_scan_rx else {
            return;
        };
        if let Ok(processes) = rx.try_recv() {
            self.processes = processes;
            self.process_scan_rx = None;
        }
    }

    fn poll_events(&mut self) {
        let mut lost_connection = false;
        if let Some(rx) = &self.events_rx {
            while let Ok(event) = rx.try_recv() {
                match event {
                    ConnEvent::Connected => {
                        self.connected = true;
                        self.connecting = false;
                        self.status = "connected".to_string();
                    }
                    ConnEvent::Failed(e) => {
                        self.status = format!("error: {e}");
                        self.connecting = false;
                        lost_connection = true;
                    }
                    ConnEvent::Output(out) => {
                        self.last_output = out;
                        self.status = "connected".to_string();
                    }
                    ConnEvent::Disconnected => {
                        self.status = "disconnected".to_string();
                        lost_connection = true;
                    }
                }
            }
        }
        if lost_connection {
            self.connected = false;
            self.cmd_tx = None;
            self.events_rx = None;
        }
    }
}

impl eframe::App for StitchApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_events();
        self.poll_process_scan();
        ctx.request_repaint_after(Duration::from_millis(100));

        if self.last_process_refresh.elapsed() >= Duration::from_secs(2) {
            self.refresh_processes();
            self.last_process_refresh = Instant::now();
        }

        egui::TopBottomPanel::top("titlebar")
            .exact_height(32.0)
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(16, 16, 16)))
            .show(ctx, |ui| {
                let rect = ui.max_rect();

                let bar = ui.interact(
                    rect,
                    ui.id().with("titlebar_drag"),
                    egui::Sense::click_and_drag(),
                );
                if bar.double_clicked() {
                    let maximized = ui.input(|i| i.viewport().maximized.unwrap_or(false));
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized));
                } else if bar.is_pointer_button_down_on() {
                    ctx.send_viewport_cmd(egui::ViewportCommand::StartDrag);
                    ctx.request_repaint();
                }

                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    "Stitch",
                    egui::FontId::proportional(15.0),
                    ui.visuals().text_color(),
                );

                let close_size = egui::vec2(40.0, rect.height());
                let close_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.max.x - close_size.x, rect.min.y),
                    close_size,
                );
                if ui
                    .put(
                        close_rect,
                        egui::Button::new(egui::RichText::new("×").size(18.0)).frame(false),
                    )
                    .clicked()
                {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                }
            });

        egui::SidePanel::left("processes_panel")
            .resizable(false)
            .exact_width(180.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Python processes").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui
                            .small_button("⟳")
                            .on_hover_text("Refresh now")
                            .clicked()
                        {
                            self.refresh_processes();
                            self.last_process_refresh = Instant::now();
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                    for proc in self.processes.clone() {
                        let label = format!("{} · {}", proc.exe_name, proc.module_name);
                        let selected = self.selected_pid == Some(proc.pid);
                        let response = ui
                            .selectable_label(selected, label)
                            .on_hover_text(format!("PID {}", proc.pid));
                        if response.clicked() {
                            self.selected_pid = Some(proc.pid);
                        }
                    }
                });
            });

        egui::SidePanel::right("scripts_panel")
            .resizable(false)
            .exact_width(190.0)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.label(egui::RichText::new("Scripts").strong());
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    if ui
                        .button("📁")
                        .on_hover_text("Set scripts folder")
                        .clicked()
                    {
                        if let Some(dir) = rfd::FileDialog::new().pick_folder() {
                            self.scripts_dir = dir;
                            self.refresh_scripts();
                        }
                    }
                    if ui
                        .button("➕")
                        .on_hover_text("New script")
                        .clicked()
                    {
                        self.new_script_name = Some(String::new());
                        self.focus_new_script = true;
                    }
                });

                if let Some(mut name) = self.new_script_name.take() {
                    let mut create = false;
                    let mut cancel = false;
                    ui.horizontal(|ui| {
                        let response = ui.text_edit_singleline(&mut name);
                        if self.focus_new_script {
                            response.request_focus();
                            self.focus_new_script = false;
                        }
                        let confirmed = response.lost_focus()
                            && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        let create_clicked = ui.button("Create").clicked();
                        let cancel_clicked = ui.button("Cancel").clicked();
                        create = confirmed || create_clicked;
                        let clicked_away =
                            response.lost_focus() && !confirmed && !create_clicked && !cancel_clicked;
                        cancel = cancel_clicked || clicked_away;
                    });

                    if create && !name.trim().is_empty() {
                        let mut file_name = name.trim().to_string();
                        if !file_name.ends_with(".py") {
                            file_name.push_str(".py");
                        }
                        let path = self.scripts_dir.join(file_name);
                        if !self.scripts_dir.exists() {
                            let _ = std::fs::create_dir_all(&self.scripts_dir);
                        }
                        let _ = std::fs::write(&path, "");
                        self.refresh_scripts();
                        self.selected_script = Some(path);
                        self.code.clear();
                    } else if cancel {
                    } else {
                        self.new_script_name = Some(name);
                    }
                }

                ui.separator();
                egui::ScrollArea::vertical().show(ui, |ui| {
                    egui::CollapsingHeader::new(egui::RichText::new("My Scripts").strong())
                        .default_open(true)
                        .show(ui, |ui| {
                            let mut to_delete: Option<PathBuf> = None;
                            for path in self.scripts.clone() {
                                let name = path
                                    .file_stem()
                                    .and_then(|s| s.to_str())
                                    .unwrap_or("?")
                                    .to_string();
                                let selected = self.selected_script.as_ref() == Some(&path);
                                ui.horizontal(|ui| {
                                    if ui
                                        .selectable_label(selected, format!("📄 {name}"))
                                        .clicked()
                                    {
                                        if let Ok(contents) = std::fs::read_to_string(&path) {
                                            self.code = contents;
                                            self.selected_script = Some(path.clone());
                                        }
                                    }
                                    if ui
                                        .small_button("🗑")
                                        .on_hover_text("Delete script")
                                        .clicked()
                                    {
                                        to_delete = Some(path.clone());
                                    }
                                });
                            }
                            if let Some(path) = to_delete {
                                let _ = std::fs::remove_file(&path);
                                if self.selected_script.as_ref() == Some(&path) {
                                    self.selected_script = None;
                                    self.code.clear();
                                }
                                self.refresh_scripts();
                            }
                        });
                });
            });

        egui::TopBottomPanel::bottom("buttons")
            .exact_height(28.0)
            .frame(egui::Frame::none().fill(egui::Color32::from_rgb(16, 16, 16)))
            .show(ctx, |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    let button_size = egui::vec2(70.0, 28.0);
                    let connect_label = if self.connected {
                        "Disconnect"
                    } else if self.connecting {
                        "Connecting..."
                    } else {
                        "Connect"
                    };
                    if ui
                        .add_enabled(
                            self.connected || !self.connecting,
                            egui::Button::new(connect_label).min_size(button_size),
                        )
                        .clicked()
                    {
                        if self.connected {
                            self.disconnect();
                        } else {
                            self.connect();
                        }
                    }
                    if ui
                        .add_enabled(
                            self.connected,
                            egui::Button::new("Execute").min_size(button_size),
                        )
                        .clicked()
                    {
                        self.execute();
                    }
                    ui.add_space(8.0);
                    ui.label(&self.status);
                });
            });

        if !self.last_output.is_empty() {
            egui::TopBottomPanel::bottom("output").show(ctx, |ui| {
                ui.label("Output:");
                ui.monospace(&self.last_output);
            });
        }

        egui::CentralPanel::default().show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                CodeEditor::default()
                    .id_source("code_editor")
                    .with_rows(30)
                    .with_fontsize(14.0)
                    .with_theme(VSCODE_DARK_PLUS)
                    .with_syntax(Syntax::python())
                    .with_numlines(true)
                    .show(ui, &mut self.code);
            });
        });
    }
}
