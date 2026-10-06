//! The window (engine/kotodama.py): pick the game, see whether it is connected, choose the microphone and the
//! speakers, the volume; it shows what it hears and the speech-to-text's progress, and offers updates.
use crate::runtime::{Options, Runtime, Status};
use crate::settings::Settings;
use eframe::egui::{self, Color32, RichText};
use kd_common::{paths, Log};
use kd_update::Release;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

const DEFAULT: &str = "(system default)";

/// The languages the game offers, by their own names (the game's "Language I speak").
fn lang_name(code: &str) -> String {
    let n = match code {
        "en" => "English",
        "es" => "Español",
        "fr" => "Français",
        "de" => "Deutsch",
        "it" => "Italiano",
        "pt" => "Português",
        "nl" => "Nederlands",
        "pl" => "Polski",
        "uk" => "Українська",
        "ru" => "Русский",
        "zh" => "中文 (普通话)",
        "yue" => "粵語",
        "ja" => "日本語",
        "ko" => "한국어",
        "auto" => "Auto (guess)",
        "cs" => "Čeština",
        "sk" => "Slovenčina",
        "ro" => "Română",
        "hr" => "Hrvatski",
        "bg" => "Български",
        "fi" => "Suomi",
        "sv" => "Svenska",
        "hu" => "Magyar",
        "da" => "Dansk",
        "et" => "Eesti",
        "lv" => "Latviešu",
        "lt" => "Lietuvių",
        "sl" => "Slovenščina",
        "el" => "Ελληνικά",
        "mt" => "Malti",
        other => other,
    };
    n.to_string()
}

/// (file, bytes done, bytes total) -> "42 % of 640 MB" (or "120 MB" when the size is not known)
pub fn download_text(d: &(String, u64, u64)) -> String {
    let (_, done, total) = d;
    if *total > 0 {
        format!("{} % of {:.0} MB", 100 * done / total, *total as f64 / 1e6)
    } else {
        format!("{:.0} MB", *done as f64 / 1e6)
    }
}

enum UpdateMsg {
    Checked(Option<Release>, bool),
    Failed(String, bool),
    Progress(f64),
    Downloaded(std::path::PathBuf),
    DownloadFailed(String),
}

struct App {
    settings: Settings,
    rt: Option<Runtime>,
    game_id: String,
    where_text: String,
    log_rx: Receiver<String>,
    log_tx: Sender<String>,
    log_lines: Vec<String>,
    ins: Vec<String>,
    outs: Vec<String>,
    mic: String,
    out: String,
    volume: f64,
    last_tick: Instant,
    status: Option<Status>,
    upd_tx: Sender<UpdateMsg>,
    upd_rx: Receiver<UpdateMsg>,
    update: Option<Release>,
    upd_text: String,
    upd_busy: bool,
    check_at: Option<Instant>,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> App {
        crate::fonts::install(&cc.egui_ctx);
        let settings = Settings::load();
        let (log_tx, log_rx) = channel();
        let (upd_tx, upd_rx) = channel();
        let ins = kd_audio::input_devices();
        let outs = kd_audio::output_devices();
        let pick = |devs: &Vec<String>, name: Option<String>| name.filter(|n| devs.contains(n)).unwrap_or_else(|| DEFAULT.to_string());
        let mut app = App {
            mic: pick(&ins, settings.str("mic")),
            out: pick(&outs, settings.str("out")),
            volume: settings.f64("volume", 1.0) * 100.0,
            game_id: settings.str("game").unwrap_or_else(|| kd_games::games()[0].id.to_string()),
            check_at: settings.bool("auto_update_check", true).then(|| Instant::now() + Duration::from_secs(3)),
            settings,
            rt: None,
            where_text: String::new(),
            log_rx,
            log_tx,
            log_lines: Vec::new(),
            ins,
            outs,
            last_tick: Instant::now() - Duration::from_secs(1),
            status: None,
            upd_tx,
            upd_rx,
            update: None,
            upd_text: String::new(),
            upd_busy: false,
        };
        app.start_game();
        app
    }

    fn logger(&self) -> Log {
        let tx = std::sync::Mutex::new(self.log_tx.clone());
        std::sync::Arc::new(move |s: &str| {
            let _ = tx.lock().unwrap().send(s.to_string());
        })
    }

    fn device(name: &str) -> Option<String> {
        (name != DEFAULT).then(|| name.to_string())
    }

    // ---- the runtime
    fn start_game(&mut self) {
        let kind = kd_games::by_id(&self.game_id);
        let opts = Options {
            out_device: Self::device(&self.out),
            mic_device: Self::device(&self.mic),
            volume: self.volume / 100.0,
            ..Default::default()
        };
        let rt = Runtime::start(kind, self.logger(), opts, None);
        let (found, where_) = rt.game.lock().unwrap().locate();
        self.where_text = if found { format!("{}: {where_}", kind.name) } else { where_ };
        self.game_id = kind.id.to_string();
        self.settings.set("game", kind.id);
        self.settings.save();
        self.rt = Some(rt);
    }

    fn switch_game(&mut self, id: &str) {
        if let Some(mut rt) = self.rt.take() {
            rt.stop();
        }
        self.game_id = id.to_string();
        self.start_game();
    }

    // ---- updates
    fn check_updates(&mut self, quiet: bool) {
        if self.update.is_some() {
            return self.do_update();
        }
        if !quiet {
            self.upd_text = "checking...".into();
        }
        let tx = self.upd_tx.clone();
        std::thread::spawn(move || {
            let _ = match kd_update::check(Duration::from_secs(10)) {
                Ok(r) => tx.send(UpdateMsg::Checked(r, quiet)),
                Err(e) => tx.send(UpdateMsg::Failed(e, quiet)),
            };
        });
    }

    fn do_update(&mut self) {
        let Some(info) = self.update.clone() else { return };
        if !(cfg!(windows) && info.installer_url.is_some()) {
            open_url(&info.page);
            return;
        }
        self.upd_busy = true;
        let tx = self.upd_tx.clone();
        std::thread::spawn(move || {
            let p = tx.clone();
            let r = kd_update::download(&info, &move |f| {
                let _ = p.send(UpdateMsg::Progress(f));
            });
            let _ = match r {
                Ok(path) => tx.send(UpdateMsg::Downloaded(path)),
                Err(e) => tx.send(UpdateMsg::DownloadFailed(e)),
            };
        });
    }

    fn handle_updates(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.upd_rx.try_recv() {
            match m {
                UpdateMsg::Checked(r, quiet) => {
                    self.upd_text = match &r {
                        None if quiet => String::new(),
                        None => "You have the latest version.".into(),
                        Some(i) if cfg!(windows) && i.installer_url.is_some() => "A new version is ready.".into(),
                        Some(_) => "A new version is out (download page).".into(),
                    };
                    self.update = r;
                }
                UpdateMsg::Failed(e, quiet) => {
                    if !quiet {
                        self.upd_text = format!("could not check for updates ({e})");
                    }
                }
                UpdateMsg::Progress(f) => self.upd_text = format!("downloading {:.0} %", f * 100.0),
                UpdateMsg::Downloaded(path) => {
                    self.upd_text = "installing...".into();
                    if let Some(mut rt) = self.rt.take() {
                        rt.stop();
                    }
                    match kd_update::install(&path) {
                        Ok(()) => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
                        Err(e) => {
                            self.upd_text = format!("update failed: {e}");
                            self.upd_busy = false;
                        }
                    }
                }
                UpdateMsg::DownloadFailed(e) => {
                    self.upd_text = format!("update failed: {e}");
                    self.upd_busy = false;
                }
            }
        }
    }

    fn licenses(&self) {
        let p = paths::app_root().join("THIRD_PARTY_NOTICES.txt");
        let p = if p.exists() { Some(p) } else { paths::repo_root().map(|r| r.join("THIRD_PARTY_NOTICES.txt")).filter(|p| p.exists()) };
        match p {
            Some(p) => open_url(&p.display().to_string()),
            None => open_url(&format!("https://github.com/{}/blob/main/THIRD_PARTY_NOTICES.txt", paths::REPO)),
        }
    }

    // ---- every 250 ms
    fn refresh(&mut self) {
        if let Some(rt) = self.rt.as_mut() {
            rt.tick();
            self.status = Some(rt.status());
        }
        while let Ok(line) = self.log_rx.try_recv() {
            self.log_lines.push(format!("{} {line}", clock()));
        }
        let n = self.log_lines.len();
        if n > 200 {
            self.log_lines.drain(..n - 200);
        }
    }

    fn device_box(ui: &mut egui::Ui, id: &str, current: &mut String, devs: &[String]) -> bool {
        let mut changed = false;
        egui::ComboBox::from_id_salt(id).width(330.0).selected_text(current.clone()).show_ui(ui, |ui| {
            for name in std::iter::once(DEFAULT.to_string()).chain(devs.iter().cloned()) {
                if ui.selectable_value(current, name.clone(), name).changed() {
                    changed = true;
                }
            }
        });
        changed
    }
}

/// Opens a web page or a file with the system's program for it.
fn open_url(target: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd").args(["/C", "start", "", target]).spawn();
    #[cfg(target_os = "macos")]
    let _ = std::process::Command::new("open").arg(target).spawn();
    #[cfg(all(unix, not(target_os = "macos")))]
    let _ = std::process::Command::new("xdg-open").arg(target).spawn();
}

/// The time of day, HH:MM:SS (local time on Windows; UTC elsewhere is good enough for a log box).
fn clock() -> String {
    #[cfg(windows)]
    unsafe {
        let mut t = std::mem::zeroed::<windows_sys::Win32::Foundation::SYSTEMTIME>();
        windows_sys::Win32::System::SystemInformation::GetLocalTime(&mut t);
        format!("{:02}:{:02}:{:02}", t.wHour, t.wMinute, t.wSecond)
    }
    #[cfg(not(windows))]
    {
        let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60)
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = ui.ctx().clone();
        if self.last_tick.elapsed() >= Duration::from_millis(250) {
            self.last_tick = Instant::now();
            self.refresh();
        }
        if self.check_at.is_some_and(|t| Instant::now() >= t) {
            self.check_at = None;
            self.check_updates(true);
        }
        self.handle_updates(&ctx);
        ctx.request_repaint_after(Duration::from_millis(250));

        let (game_name, needs) = {
            let k = kd_games::by_id(&self.game_id);
            (k.name, k.needs)
        };
        egui::Frame::central_panel(ui.style()).show(ui, |ui| {
            // ---- the game and its state
            let mut switch = None;
            ui.horizontal(|ui| {
                ui.label("Game");
                egui::ComboBox::from_id_salt("game").width(200.0).selected_text(game_name).show_ui(ui, |ui| {
                    for g in kd_games::games() {
                        if ui.selectable_label(g.id == self.game_id, g.name).clicked() && g.id != self.game_id {
                            switch = Some(g.id.to_string());
                        }
                    }
                });
                let (text, colour) = match &self.status {
                    Some(st) if !st.error.is_empty() => (st.error.clone(), Color32::from_rgb(0xc0, 0x39, 0x2b)),
                    Some(st) if st.state == "connected" => (format!("Connected to {game_name}"), Color32::from_rgb(0x2a, 0x9d, 0x4b)),
                    Some(st) if st.state == "paused" => (format!("{game_name} paused (or the level ended)"), Color32::from_rgb(0xd0, 0xa0, 0x00)),
                    _ => (format!("Waiting for {game_name}: start a level with {needs}"), Color32::from_rgb(0xd0, 0xa0, 0x00)),
                };
                let (rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), egui::Sense::hover());
                ui.painter().circle_filled(rect.center(), 5.0, colour);
                ui.add(egui::Label::new(text).wrap());
            });
            if let Some(id) = switch {
                self.switch_game(&id);
            }
            ui.add(egui::Label::new(RichText::new(&self.where_text).color(Color32::GRAY)).wrap());
            ui.add_space(4.0);

            // ---- sound
            let inner = ui.available_width() - 2.0 * (ui.style().spacing.window_margin.left as f32).max(6.0) - 2.0;
            ui.group(|ui| {
                ui.set_width(inner);
                ui.label(RichText::new("Sound").strong());
                egui::Grid::new("sound").num_columns(3).spacing([10.0, 6.0]).show(ui, |ui| {
                    ui.label("Microphone");
                    if Self::device_box(ui, "mic", &mut self.mic, &self.ins) {
                        self.settings.set("mic", self.mic.clone());
                        self.settings.save();
                        let d = Self::device(&self.mic);
                        if let Some(rt) = self.rt.as_mut() {
                            rt.set_mic(d);
                        }
                    }
                    let st = self.status.as_ref();
                    let lvl = st.filter(|s| s.mic == "listening" || s.mic == "talking").map(|s| ((s.level + 60.0) / 60.0).clamp(0.0, 1.0)).unwrap_or(0.0);
                    let (rect, _) = ui.allocate_exact_size(egui::vec2(120.0, 12.0), egui::Sense::hover());
                    ui.painter().rect_stroke(rect, 0.0, egui::Stroke::new(1.0, Color32::GRAY), egui::StrokeKind::Inside);
                    let mut bar = rect;
                    bar.set_width(120.0 * lvl as f32);
                    let talking = st.is_some_and(|s| s.mic == "talking");
                    ui.painter().rect_filled(bar, 0.0, if talking { Color32::from_rgb(0x2a, 0x9d, 0x4b) } else { Color32::from_rgb(0x7f, 0xbf, 0x8f) });
                    ui.end_row();
                    ui.label("Speakers");
                    if Self::device_box(ui, "out", &mut self.out, &self.outs) {
                        self.settings.set("out", self.out.clone());
                        self.settings.save();
                        let d = Self::device(&self.out);
                        if let Some(rt) = self.rt.as_mut() {
                            rt.set_output(d);
                        }
                    }
                    ui.end_row();
                    ui.label("Volume");
                    ui.spacing_mut().slider_width = 330.0;
                    if ui.add(egui::Slider::new(&mut self.volume, 0.0..=100.0).show_value(false)).changed() {
                        self.settings.set("volume", (self.volume / 100.0 * 100.0).round() / 100.0);
                        if let Some(rt) = self.rt.as_mut() {
                            rt.set_volume(self.volume / 100.0);
                        }
                    }
                    ui.end_row();
                });
            });
            if ui.ctx().input(|i| i.pointer.any_released()) {
                self.settings.save(); // (the volume: saved when the slider is let go)
            }

            // ---- speech to text
            ui.group(|ui| {
                ui.set_width(inner);
                ui.label(RichText::new("Speech to text").strong());
                if let Some(st) = &self.status {
                    let mic = match (&st.download, st.mic) {
                        (Some(d), _) => format!("downloading the speech model, {}", download_text(d)),
                        (None, "wanted") => "starting".into(),
                        (None, "loading") => "loading the speech models...".into(),
                        (None, "listening") => "listening".into(),
                        (None, "talking") => "hearing you".into(),
                        _ => "off".into(),
                    };
                    ui.label(format!("Language: {}  (set in the game)    Microphone: {mic}", lang_name(&st.lang)));
                    let hear = if !st.live.is_empty() {
                        format!("Hearing: {}", st.live)
                    } else if !st.last.is_empty() {
                        format!("You said: {}", st.last)
                    } else {
                        String::new()
                    };
                    let n = hear.chars().count();
                    ui.add(egui::Label::new(hear.chars().skip(n.saturating_sub(160)).collect::<String>()).wrap());
                }
            });

            // ---- the log, the buttons
            let bottom_h = 34.0;
            let h = (ui.available_height() - bottom_h).max(60.0);
            egui::Frame::new().fill(ui.visuals().extreme_bg_color).inner_margin(6.0).show(ui, |ui| {
                ui.set_min_width(ui.available_width());
                egui::ScrollArea::vertical().max_height(h).stick_to_bottom(true).auto_shrink([false, false]).show(ui, |ui| {
                    for line in &self.log_lines {
                        ui.label(RichText::new(line).monospace().size(12.0));
                    }
                });
            });
            ui.horizontal(|ui| {
                let label = match &self.update {
                    Some(i) if cfg!(windows) && i.installer_url.is_some() => format!("Update to {}", i.version),
                    Some(i) => format!("Get {}", i.version),
                    None => "Check for updates".into(),
                };
                if ui.add_enabled(!self.upd_busy, egui::Button::new(label)).clicked() {
                    self.check_updates(false);
                }
                if ui.button("Licenses").clicked() {
                    self.licenses();
                }
                ui.label(RichText::new(&self.upd_text).color(Color32::GRAY));
            });
        });
    }

    fn on_exit(&mut self, _gl: Option<&eframe::glow::Context>) {
        self.settings.save();
        if let Some(mut rt) = self.rt.take() {
            rt.stop();
        }
    }
}

/// How the window is drawn: wgpu (Direct3D 12 on Windows: it falls back to Windows' own software renderer where
/// there is no graphics driver - a virtual machine, a remote desktop - where OpenGL is only 1.1 and egui's OpenGL
/// renderer cannot start), OpenGL (glow) elsewhere. KOTODAMA_RENDERER=wgpu|glow chooses.
pub fn renderer() -> eframe::Renderer {
    match std::env::var("KOTODAMA_RENDERER").ok().as_deref() {
        Some("glow") => eframe::Renderer::Glow,
        Some("wgpu") => eframe::Renderer::Wgpu,
        _ if cfg!(windows) => eframe::Renderer::Wgpu,
        _ => eframe::Renderer::Glow,
    }
}

pub fn main() -> i32 {
    if !crate::instance::single_instance() {
        message(&format!("{} is already running.", paths::APP_NAME));
        return 0;
    }
    let first = renderer();
    let opts = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title(format!("{} {}", paths::APP_NAME, paths::VERSION))
            .with_inner_size([760.0, 520.0])
            .with_min_inner_size([560.0, 460.0]),
        renderer: first,
        ..Default::default()
    };
    match eframe::run_native(paths::APP_NAME, opts, Box::new(|cc| Ok(Box::new(App::new(cc))))) {
        Ok(()) => 0,
        Err(e) if std::env::var_os("KOTODAMA_RENDERER").is_none() => {
            // (that renderer would not start here: once more with the other one - in a new process, as a window
            //  system can be set up only once per process)
            let other = if first == eframe::Renderer::Wgpu { "glow" } else { "wgpu" };
            crate::instance::release();
            let args: Vec<String> = std::env::args().skip(1).collect();
            match std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(args).env("KOTODAMA_RENDERER", other).status()) {
                Ok(st) => st.code().unwrap_or(1),
                Err(e2) => {
                    message(&format!("{} could not open its window: {e} ({e2})", paths::APP_NAME));
                    1
                }
            }
        }
        Err(e) => {
            message(&format!("{} could not open its window: {e}", paths::APP_NAME));
            1
        }
    }
}

/// A message box (Windows), or a line on the console.
fn message(text: &str) {
    #[cfg(windows)]
    unsafe {
        use windows_sys::Win32::UI::WindowsAndMessaging::{MessageBoxW, MB_ICONINFORMATION, MB_OK};
        let t: Vec<u16> = format!("{text}\0").encode_utf16().collect();
        let c: Vec<u16> = format!("{}\0", paths::APP_NAME).encode_utf16().collect();
        MessageBoxW(std::ptr::null_mut(), t.as_ptr(), c.as_ptr(), MB_OK | MB_ICONINFORMATION);
    }
    #[cfg(not(windows))]
    eprintln!("{text}");
}
