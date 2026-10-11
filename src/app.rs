use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui_code_editor::{CodeEditor, ColorTheme, Syntax};
use stitch_stub::pipe;

use crate::builder;
use crate::injector;
use crate::process::{self, PythonProcess};

enum ConnEvent {
    Connected,
    Failed(String),
    Output(String),
    Modules(String),
    Threads(String),
    Globals(String),
    Disconnected,
}

enum Request {
    Exec(String),
    Modules,
    Threads,
    Globals,
}

#[derive(PartialEq, Clone, Copy)]
enum InspectorTab {
    Modules,
    Threads,
    Globals,
}

const MODULES_SCRIPT: &str = "import sys
for __n, __m in sorted(sys.modules.items()):
    if __m is None:
        continue
    __p = getattr(__m, '__file__', None) or ''
    __v = getattr(__m, '__version__', '')
    print('\\t'.join((__n, str(__p), str(__v))))
";

const THREADS_SCRIPT: &str = "import sys, threading, traceback
__names = {t.ident: t.name for t in threading.enumerate()}
__blocks = []
for __tid, __frame in sys._current_frames().items():
    __name = __names.get(__tid, '')
    __stack = ''.join(traceback.format_stack(__frame))
    __blocks.append('%d\\t%s\\n%s' % (__tid, __name, __stack))
print('\\x1e'.join(__blocks), end='')
";

const GLOBALS_SCRIPT: &str = "import sys, inspect
__m = sys.modules.get('__main__')
__d = vars(__m) if __m is not None else {}
for __name in sorted(__d):
    if __name.startswith('__') and __name.endswith('__'):
        continue
    try:
        __obj = __d[__name]
    except Exception:
        continue
    if inspect.isclass(__obj):
        try:
            __bases = ', '.join(b.__name__ for b in __obj.__bases__)
        except Exception:
            __bases = ''
        __detail = '(%s)' % __bases if __bases else ''
        print('c\\t%s\\t%s' % (__name, __detail))
    elif inspect.isroutine(__obj):
        try:
            __sig = str(inspect.signature(__obj))
        except Exception:
            __sig = '(...)'
        print('f\\t%s\\t%s' % (__name, __sig))
    else:
        try:
            __r = repr(__obj)
        except Exception:
            __r = '<unreprable>'
        __r = __r.replace('\\t', ' ').replace('\\n', ' ')
        if len(__r) > 200:
            __r = __r[:200] + '...'
        print('v\\t%s\\t%s = %s' % (__name, type(__obj).__name__, __r))
";

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
    cmd_tx: Option<Sender<Request>>,
    events_rx: Option<Receiver<ConnEvent>>,
    connecting: bool,
    last_process_refresh: Instant,
    focus_new_script: bool,
    process_scan_rx: Option<Receiver<Vec<PythonProcess>>>,
    show_inspector: bool,
    inspector_tab: InspectorTab,
    module_filter: String,
    modules: Vec<(String, String, String)>,
    threads: Vec<(String, String)>,
    globals_funcs: Vec<(String, String)>,
    globals_classes: Vec<(String, String)>,
    globals_vars: Vec<(String, String)>,
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
            show_inspector: false,
            inspector_tab: InspectorTab::Modules,
            module_filter: String::new(),
            modules: Vec::new(),
            threads: Vec::new(),
            globals_funcs: Vec::new(),
            globals_classes: Vec::new(),
            globals_vars: Vec::new(),
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

        let (cmd_tx, cmd_rx) = mpsc::channel::<Request>();
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

            while let Ok(request) = cmd_rx.recv() {
                let (payload, kind) = match request {
                    Request::Exec(code) => (code, 0u8),
                    Request::Modules => (MODULES_SCRIPT.to_string(), 1u8),
                    Request::Threads => (THREADS_SCRIPT.to_string(), 2u8),
                    Request::Globals => (GLOBALS_SCRIPT.to_string(), 3u8),
                };
                if pipe::write_message(&conn, payload.as_bytes()).is_err() {
                    let _ = event_tx.send(ConnEvent::Disconnected);
                    break;
                }
                match pipe::read_message(&conn) {
                    Ok(Some(bytes)) => {
                        let out = String::from_utf8_lossy(&bytes).into_owned();
                        let event = match kind {
                            1 => ConnEvent::Modules(out),
                            2 => ConnEvent::Threads(out),
                            3 => ConnEvent::Globals(out),
                            _ => ConnEvent::Output(out),
                        };
                        let _ = event_tx.send(event);
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
                let _ = tx.send(Request::Exec(self.code.clone()));
                self.status = "sent".to_string();
            }
            None => self.status = "not connected".to_string(),
        }
    }

    fn request_modules(&mut self) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Request::Modules);
            self.inspector_tab = InspectorTab::Modules;
            self.show_inspector = true;
        }
    }

    fn request_threads(&mut self) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Request::Threads);
            self.inspector_tab = InspectorTab::Threads;
            self.show_inspector = true;
        }
    }

    fn request_globals(&mut self) {
        if let Some(tx) = &self.cmd_tx {
            let _ = tx.send(Request::Globals);
            self.inspector_tab = InspectorTab::Globals;
            self.show_inspector = true;
        }
    }

    fn refresh_inspector(&mut self) {
        match self.inspector_tab {
            InspectorTab::Modules => self.request_modules(),
            InspectorTab::Threads => self.request_threads(),
            InspectorTab::Globals => self.request_globals(),
        }
    }

    fn toggle_inspector(&mut self, tab: InspectorTab) {
        if self.show_inspector && self.inspector_tab == tab {
            self.show_inspector = false;
            return;
        }
        match tab {
            InspectorTab::Modules => self.request_modules(),
            InspectorTab::Threads => self.request_threads(),
            InspectorTab::Globals => self.request_globals(),
        }
    }

    fn inspector_drawer(&mut self, ctx: &egui::Context) {
        const BASE_WIDTH: f32 = 900.0;
        const DRAWER_WIDTH: f32 = 320.0;

        let t = ctx.animate_bool_with_time(
            egui::Id::new("inspector_open"),
            self.show_inspector,
            0.22,
        );
        if t > 0.001 {
            let height = ctx.screen_rect().height();
            ctx.send_viewport_cmd(egui::ViewportCommand::InnerSize(egui::vec2(
                BASE_WIDTH + DRAWER_WIDTH * t,
                height,
            )));
        }

        let extra = (ctx.screen_rect().width() - BASE_WIDTH).max(0.0);
        if extra < 1.0 {
            return;
        }
        let mut do_refresh = false;
        let mut do_close = false;
        egui::SidePanel::right("inspector_drawer")
            .resizable(false)
            .exact_width(extra)
            .show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    let title = match self.inspector_tab {
                        InspectorTab::Modules => "Modules",
                        InspectorTab::Threads => "Threads",
                        InspectorTab::Globals => "Globals",
                    };
                    ui.label(egui::RichText::new(title).strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("×").on_hover_text("Close").clicked() {
                            do_close = true;
                        }
                        if ui.small_button("🔄").on_hover_text("Refresh").clicked() {
                            do_refresh = true;
                        }
                    });
                });
                ui.separator();
                match self.inspector_tab {
                    InspectorTab::Modules => {
                        ui.horizontal(|ui| {
                            ui.label("Filter");
                            ui.text_edit_singleline(&mut self.module_filter);
                        });
                        let needle = self.module_filter.to_lowercase();
                        ui.label(format!("{} modules", self.modules.len()));
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                            for (name, path, version) in &self.modules {
                                if !needle.is_empty()
                                    && !name.to_lowercase().contains(needle.as_str())
                                {
                                    continue;
                                }
                                let label = if version.is_empty() {
                                    name.clone()
                                } else {
                                    format!("{name}  ·  v{version}")
                                };
                                ui.label(egui::RichText::new(label).monospace())
                                    .on_hover_text(path.as_str());
                            }
                        });
                    }
                    InspectorTab::Threads => {
                        ui.label(format!("{} threads", self.threads.len()));
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            for (i, (head, stack)) in self.threads.iter().enumerate() {
                                egui::CollapsingHeader::new(egui::RichText::new(head).strong())
                                    .id_salt(("thread", i))
                                    .default_open(false)
                                    .show(ui, |ui| {
                                        ui.add(
                                            egui::Label::new(
                                                egui::RichText::new(stack).monospace(),
                                            )
                                            .selectable(true),
                                        );
                                    });
                            }
                        });
                    }
                    InspectorTab::Globals => {
                        ui.horizontal(|ui| {
                            ui.label("Filter");
                            ui.text_edit_singleline(&mut self.module_filter);
                        });
                        let needle = self.module_filter.to_lowercase();
                        ui.separator();
                        egui::ScrollArea::vertical().show(ui, |ui| {
                            ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Truncate);
                            globals_section(
                                ui,
                                "Functions",
                                &self.globals_funcs,
                                &needle,
                                |name, detail| format!("{name}{detail}"),
                            );
                            globals_section(
                                ui,
                                "Classes",
                                &self.globals_classes,
                                &needle,
                                |name, detail| format!("{name}{detail}"),
                            );
                            globals_section(
                                ui,
                                "Variables",
                                &self.globals_vars,
                                &needle,
                                |name, detail| {
                                    if detail.is_empty() {
                                        name.to_string()
                                    } else {
                                        format!("{name}: {detail}")
                                    }
                                },
                            );
                        });
                    }
                }
            });
        if do_refresh {
            self.refresh_inspector();
        }
        if do_close {
            self.show_inspector = false;
        }
    }

    fn build_dll(&mut self) {
        if self.code.trim().is_empty() {
            self.status = "nothing to build".to_string();
            return;
        }
        let default_name = self
            .selected_script
            .as_ref()
            .and_then(|p| p.file_stem())
            .and_then(|s| s.to_str())
            .unwrap_or("payload");
        let Some(path) = rfd::FileDialog::new()
            .add_filter("DLL", &["dll"])
            .set_file_name(format!("{default_name}.dll"))
            .save_file()
        else {
            return;
        };
        match builder::build_payload_dll(&self.code, &path) {
            Ok(()) => self.status = format!("built {}", path.display()),
            Err(e) => self.status = format!("build failed: {e}"),
        }
    }

    fn save_current_script(&mut self) {
        let Some(path) = self.selected_script.clone() else {
            self.status = "no script selected".to_string();
            return;
        };
        match std::fs::write(&path, &self.code) {
            Ok(()) => self.status = format!("saved {}", path.display()),
            Err(e) => self.status = format!("save failed: {e}"),
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
                    ConnEvent::Modules(out) => {
                        self.modules = parse_modules(&out);
                        self.status = "connected".to_string();
                    }
                    ConnEvent::Threads(out) => {
                        self.threads = parse_threads(&out);
                        self.status = "connected".to_string();
                    }
                    ConnEvent::Globals(out) => {
                        let (funcs, classes, vars) = parse_globals(&out);
                        self.globals_funcs = funcs;
                        self.globals_classes = classes;
                        self.globals_vars = vars;
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

fn parse_modules(text: &str) -> Vec<(String, String, String)> {
    text.lines()
        .filter_map(|line| {
            let mut fields = line.split('\t');
            let name = fields.next()?.to_string();
            if name.is_empty() {
                return None;
            }
            let path = fields.next().unwrap_or("").to_string();
            let version = fields.next().unwrap_or("").to_string();
            Some((name, path, version))
        })
        .collect()
}

fn parse_threads(text: &str) -> Vec<(String, String)> {
    text.split('\u{1e}')
        .filter(|block| !block.is_empty())
        .map(|block| {
            let mut parts = block.splitn(2, '\n');
            let head = parts.next().unwrap_or("").replace('\t', " · ");
            let stack = parts.next().unwrap_or("").to_string();
            (head, stack)
        })
        .collect()
}

type GlobalEntries = Vec<(String, String)>;

fn parse_globals(text: &str) -> (GlobalEntries, GlobalEntries, GlobalEntries) {
    let mut funcs = Vec::new();
    let mut classes = Vec::new();
    let mut vars = Vec::new();
    for line in text.lines() {
        let mut fields = line.splitn(3, '\t');
        let category = match fields.next() {
            Some(c) => c,
            None => continue,
        };
        let name = match fields.next() {
            Some(n) if !n.is_empty() => n.to_string(),
            _ => continue,
        };
        let detail = fields.next().unwrap_or("").to_string();
        match category {
            "f" => funcs.push((name, detail)),
            "c" => classes.push((name, detail)),
            "v" => vars.push((name, detail)),
            _ => {}
        }
    }
    (funcs, classes, vars)
}

fn globals_section(
    ui: &mut egui::Ui,
    title: &str,
    entries: &[(String, String)],
    needle: &str,
    join: impl Fn(&str, &str) -> String,
) {
    let matches: Vec<&(String, String)> = entries
        .iter()
        .filter(|(name, _)| needle.is_empty() || name.to_lowercase().contains(needle))
        .collect();
    let header = format!("{title} ({})", matches.len());
    egui::CollapsingHeader::new(egui::RichText::new(header).strong())
        .default_open(true)
        .show(ui, |ui| {
            for (name, detail) in matches {
                ui.label(egui::RichText::new(join(name, detail)).monospace());
            }
        });
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

        if ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::S)) {
            self.save_current_script();
        }
        if self.connected && ctx.input(|i| i.modifiers.command && i.key_pressed(egui::Key::Enter)) {
            self.execute();
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
                            .small_button("🔄")
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

        self.inspector_drawer(ctx);

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
                    if ui
                        .add_enabled(self.selected_script.is_some(), egui::Button::new("💾"))
                        .on_hover_text("Save script (Ctrl+S)")
                        .clicked()
                    {
                        self.save_current_script();
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

                    egui::CollapsingHeader::new(egui::RichText::new("Inspector").strong())
                        .default_open(true)
                        .show(ui, |ui| {
                            if ui
                                .selectable_label(
                                    self.show_inspector
                                        && self.inspector_tab == InspectorTab::Modules,
                                    "📦 Modules",
                                )
                                .clicked()
                            {
                                self.toggle_inspector(InspectorTab::Modules);
                            }
                            if ui
                                .selectable_label(
                                    self.show_inspector
                                        && self.inspector_tab == InspectorTab::Threads,
                                    "📜 Threads",
                                )
                                .clicked()
                            {
                                self.toggle_inspector(InspectorTab::Threads);
                            }
                            if ui
                                .selectable_label(
                                    self.show_inspector
                                        && self.inspector_tab == InspectorTab::Globals,
                                    "📇 Globals",
                                )
                                .clicked()
                            {
                                self.toggle_inspector(InspectorTab::Globals);
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
                    if ui
                        .add_enabled(
                            !self.code.trim().is_empty(),
                            egui::Button::new("Build DLL").min_size(button_size),
                        )
                        .clicked()
                    {
                        self.build_dll();
                    }
                    ui.add_space(8.0);
                    ui.spacing_mut().item_spacing.x = 6.0;
                    let status_color = if self.connected {
                        egui::Color32::from_rgb(60, 200, 90)
                    } else if self.connecting {
                        egui::Color32::from_rgb(230, 180, 60)
                    } else {
                        egui::Color32::from_rgb(140, 140, 140)
                    };
                    let (dot_rect, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter()
                        .circle_filled(dot_rect.center(), 4.0, status_color);
                    ui.label(&self.status);
                });
            });

        if !self.last_output.is_empty() {
            egui::TopBottomPanel::bottom("output").show(ctx, |ui| {
                ui.add_space(4.0);
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new("Output").strong());
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        if ui.small_button("Clear").clicked() {
                            self.last_output.clear();
                        }
                    });
                });
                ui.separator();
                egui::ScrollArea::vertical().max_height(140.0).show(ui, |ui| {
                    ui.add(
                        egui::Label::new(egui::RichText::new(&self.last_output).monospace())
                            .selectable(true),
                    );
                });
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
