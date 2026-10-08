//! The window (engine/koetama.py): pick the game, see whether it is connected, choose the microphone and the
//! speakers, the volume; it shows what it hears and the speech-to-text's progress, and offers updates.
use crate::runtime::{Options, Runtime, Status};
use crate::settings::Settings;
use crate::theme;
use eframe::egui::{self, Color32, RichText};
use kd_common::{paths, Log};
use kd_update::Release;
use std::sync::mpsc::{channel, Receiver, Sender};
use std::time::{Duration, Instant};

const DEFAULT: &str = "(system default)";

/// A language as the window shows it: its own name, and the English one when it differs.
fn lang_name(code: &str) -> String {
    match kd_speech::lang_info(code) {
        Some(l) if l.name != l.english => format!("{} ({})", l.name, l.english),
        Some(l) => l.name.to_string(),
        None => code.to_string(),
    }
}

/// A model's state in words, and its colour.
fn model_state(row: &crate::runtime::ModelRow) -> (String, Color32) {
    use kd_speech::ModelState::*;
    match &row.state {
        Loaded if row.needed => ("loaded".into(), theme::GOOD),
        Loaded => ("letting go".into(), theme::MUTED),
        Loading => ("loading".into(), theme::WARN),
        Downloading(f, d, t) => (
            format!("downloading {}", download_text(&(f.clone(), *d, *t))),
            theme::WARN,
        ),
        NotLoaded if row.needed => ("loads when the game wants your voice".into(), theme::MUTED),
        NotLoaded => ("not needed".into(), theme::MUTED),
    }
}

/// (file, bytes done, bytes total) -> "42 % of 640 MB" (or "120 MB" when the size is not known)
/// The voice chat as the window says it: off, connecting, in the room (and how many voices it hears).
pub fn voice_text(v: &kd_voice::VoiceStatus) -> (String, Color32) {
    match (v.state, v.heard) {
        ("connected", 0) => ("in the room".into(), theme::GOOD),
        ("connected", 1) => ("in the room · hearing 1 voice".into(), theme::GOOD),
        ("connected", n) => (format!("in the room · hearing {n} voices"), theme::GOOD),
        ("connecting", _) => ("connecting...".into(), theme::WARN),
        _ => ("off".into(), theme::MUTED),
    }
}

/// A language's English name ("Japanese"; a code Koetama does not know: the code).
fn english_name(code: &str) -> String {
    kd_speech::lang_info(code).map_or_else(|| code.to_string(), |l| l.english.to_string())
}

/// A translation rule as the window says it: "Japanese → English", its state in words and its colour.
pub fn rule_text(r: &kd_translate::service::RuleStatus) -> (String, String, Color32) {
    use kd_translate::service::State;
    let rule = format!("{} → {}", english_name(&r.from), english_name(&r.to));
    let (state, colour) = match r.state {
        State::Ready => ("ready".to_string(), theme::GOOD),
        State::Downloading(f) => (format!("downloading {} %", kd_translate::service::percent(f)), theme::WARN),
        State::Loading => ("loading".to_string(), theme::WARN),
        State::Unavailable => ("no model for it".to_string(), theme::MUTED),
        State::Error => ("failed - trying again in a minute".to_string(), theme::BAD),
    };
    (rule, state, colour)
}

/// The translation as the command line says it: "ja → en ready, ko → en 42 %" ("no rules" while the game sets none).
pub fn translate_line(rules: &[kd_translate::service::RuleStatus]) -> String {
    use kd_translate::service::State;
    if rules.is_empty() {
        return "no rules".into();
    }
    let parts: Vec<String> = rules
        .iter()
        .map(|r| {
            let state = match r.state {
                State::Downloading(f) => format!("{} %", kd_translate::service::percent(f)),
                ref s => s.word().to_string(),
            };
            format!("{} → {} {state}", r.from, r.to)
        })
        .collect();
    parts.join(", ")
}

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
    /// the languages the player speaks (empty: the game's "Language I speak")
    langs: Vec<String>,
    choosing_langs: bool,
    /// the window's look done once it exists (its dark title bar)
    dressed: bool,
    /// the game mods (built-in, then profile files) and the one in use
    kinds: Vec<kd_games::GameKind>,
    kind: kd_games::GameKind,
    /// adding a profile: the file dialog's answer, then the preview of what it does (or why it does not load)
    picking: Option<Receiver<Option<std::path::PathBuf>>>,
    preview: Option<(std::path::PathBuf, Result<kd_games::GameKind, String>)>,
    /// a community game mod to remove, once the player confirms
    removing: Option<kd_games::GameKind>,
    profile_msg: String,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> App {
        crate::fonts::install(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        let settings = Settings::load();
        let (log_tx, log_rx) = channel();
        let (upd_tx, upd_rx) = channel();
        let ins = kd_audio::input_devices();
        let outs = kd_audio::output_devices();
        let pick = |devs: &Vec<String>, name: Option<String>| {
            name.filter(|n| devs.contains(n))
                .unwrap_or_else(|| DEFAULT.to_string())
        };
        let mut app = App {
            mic: pick(&ins, settings.str("mic")),
            out: pick(&outs, settings.str("out")),
            volume: settings.f64("volume", 1.0) * 100.0,
            game_id: settings
                .str("game")
                .unwrap_or_else(|| kd_games::games()[0].id.to_string()),
            check_at: settings
                .bool("auto_update_check", true)
                .then(|| Instant::now() + Duration::from_secs(3)),
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
            langs: Vec::new(),
            choosing_langs: false,
            dressed: false,
            kinds: Vec::new(),
            kind: kd_games::by_id(""),
            picking: None,
            preview: None,
            removing: None,
            profile_msg: String::new(),
        };
        app.kinds = kd_games::games();
        app.langs = app
            .settings
            .0
            .get("languages")
            .and_then(|v| v.as_array())
            .map(|a| {
                a.iter()
                    .filter_map(|l| l.as_str().map(String::from))
                    .collect()
            })
            .unwrap_or_default();
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
            langs: self.langs.clone(),
            ..Default::default()
        };
        let rt = Runtime::start(kind.clone(), self.logger(), opts, None);
        let (found, where_) = rt.game.lock().unwrap().locate();
        self.where_text = if found { format!("{}: {where_}", kind.name) } else { where_ };
        self.game_id = kind.id.clone();
        self.settings.set("game", kind.id.clone());
        self.settings.save();
        self.kind = kind;
        self.rt = Some(rt);
    }

    // ---- game mod profiles: add (a file -> a preview of what it does -> installed), remove
    fn add_profile(&mut self) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::dialog::pick_profile());
        });
        self.picking = Some(rx);
    }

    fn profiles_ui(&mut self, ctx: &egui::Context) {
        if let Some(rx) = &self.picking {
            if let Ok(picked) = rx.try_recv() {
                self.picking = None;
                if let Some(path) = picked {
                    let r = kd_games::load_profile(&path);
                    self.preview = Some((path, r));
                }
            }
        }
        if let Some((path, r)) = self.preview.clone() {
            let mut close = false;
            egui::Modal::new(egui::Id::new("profile-preview")).show(ctx, |ui| {
                ui.set_width(520.0);
                match &r {
                    Ok(k) => {
                        ui.label(RichText::new("Add this game mod?").size(18.0).family(theme::semibold()));
                        theme::game_row(ui, &k.name, &k.mod_name, false, true, 500.0);
                        ui.label(RichText::new(format!("by {} · {}", k.author, k.mod_url)).size(12.5).color(theme::MUTED));
                        ui.add_space(4.0);
                        ui.label(RichText::new("What it does on this PC").family(theme::semibold()));
                        for line in &k.summary {
                            ui.label(format!("• {line}"));
                        }
                        ui.add_space(2.0);
                        ui.label(
                            RichText::new("A profile is not a program: it only points Koetama's own connectors at these \
                                 files and ports. Add it if you trust where it came from.")
                                .size(12.5)
                                .color(theme::MUTED),
                        );
                        ui.add_space(6.0);
                        ui.horizontal(|ui| {
                            if theme::primary_button(ui, "Add game mod", true).clicked() {
                                match kd_games::install_profile(&path) {
                                    Ok(k) => {
                                        self.kinds = kd_games::games();
                                        self.profile_msg = format!("Added {} ({} mod).", k.name, k.mod_name);
                                        self.switch_game(&k.id.clone());
                                    }
                                    Err(e) => self.profile_msg = format!("Could not add it: {e}"),
                                }
                                close = true;
                            }
                            if ui.button("Cancel").clicked() {
                                close = true;
                            }
                        });
                    }
                    Err(e) => {
                        ui.label(RichText::new("This file is not a game mod profile Koetama can use").size(16.0).family(theme::semibold()));
                        ui.label(RichText::new(path.display().to_string()).size(12.5).color(theme::MUTED));
                        ui.label(RichText::new(e).color(theme::BAD));
                        if ui.button("Close").clicked() {
                            close = true;
                        }
                    }
                }
            });
            if close {
                self.preview = None;
            }
        }
        if let Some(k) = self.removing.clone() {
            let mut close = false;
            egui::Modal::new(egui::Id::new("profile-remove")).show(ctx, |ui| {
                ui.set_width(420.0);
                ui.label(RichText::new(format!("Remove {} ({} mod)?", k.name, k.mod_name)).size(16.0).family(theme::semibold()));
                ui.label(RichText::new("Its profile file is deleted. You can add it again later.").color(theme::MUTED));
                ui.horizontal(|ui| {
                    if ui.button("Remove").clicked() {
                        match kd_games::remove_profile(&k.id) {
                            Ok(()) => {
                                self.profile_msg = format!("Removed {}.", k.name);
                                self.kinds = kd_games::games();
                                if self.game_id == k.id {
                                    let first = self.kinds[0].id.clone();
                                    self.switch_game(&first);
                                }
                            }
                            Err(e) => self.profile_msg = format!("Could not remove it: {e}"),
                        }
                        close = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
            if close {
                self.removing = None;
            }
        }
    }

    /// The player ticked or unticked a language (none: follow the game's setting).
    fn set_langs(&mut self, langs: Vec<String>) {
        self.langs = langs;
        self.settings
            .set("languages", serde_json::Value::from(self.langs.clone()));
        self.settings.save();
        if let Some(rt) = self.rt.as_mut() {
            rt.set_languages(self.langs.clone());
        }
    }

    /// "Languages I speak": what is in use (chips), and (Choose) every language by how well it is written.
    fn languages_ui(&mut self, ui: &mut egui::Ui) {
        let (langs, from_game) = match &self.status {
            Some(st) => (st.langs.clone(), st.langs_from_game),
            None => (self.langs.clone(), self.langs.is_empty()),
        };
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Languages I speak").color(theme::MUTED));
            for l in &langs {
                theme::chip(ui, &lang_name(l));
            }
            if from_game {
                ui.label(RichText::new("(the game's setting)").color(theme::MUTED));
            }
            let label = if self.choosing_langs {
                "Done"
            } else {
                "Choose..."
            };
            if ui.button(label).clicked() {
                self.choosing_langs = !self.choosing_langs;
            }
        });
        if langs.len() > 1 && !from_game {
            ui.label(
                RichText::new("Several languages: Koetama tells them apart as you speak.")
                    .size(12.5)
                    .color(theme::MUTED),
            );
        }
        if !self.choosing_langs {
            return;
        }
        let mut chosen = self.langs.clone();
        let mut changed = false;
        theme::sunk(ui, |ui| {
            egui::ScrollArea::vertical().max_height(240.0).show(ui, |ui| {
                ui.label(
                    RichText::new("Tick every language you speak. Fewer is lighter and more accurate: each needs its speech \
                         model in memory.")
                        .size(12.5)
                        .color(theme::MUTED),
                );
                for tier in [kd_speech::Tier::Full, kd_speech::Tier::Soft, kd_speech::Tier::Weak] {
                    ui.add_space(6.0);
                    ui.label(RichText::new(tier.label()).family(theme::semibold()).color(theme::FG));
                    egui::Grid::new(format!("langs-{tier:?}")).num_columns(3).spacing([18.0, 3.0]).show(ui, |ui| {
                        for (k, l) in kd_speech::LANGS.iter().filter(|l| l.tier == tier).enumerate() {
                            let mut on = chosen.iter().any(|c| c == l.code);
                            if ui.checkbox(&mut on, lang_name(l.code)).changed() {
                                changed = true;
                                if on {
                                    chosen.push(l.code.to_string());
                                } else {
                                    chosen.retain(|c| c != l.code);
                                }
                            }
                            if k % 3 == 2 {
                                ui.end_row();
                            }
                        }
                    });
                }
                ui.add_space(6.0);
                if ui.add_enabled(!chosen.is_empty(), egui::Button::new("Use the game's setting instead")).clicked() {
                    chosen.clear();
                    changed = true;
                }
            });
        });
        if changed {
            // (in the order of the list: the first is the fallback for a short line before any other was heard)
            chosen.sort_by_key(|c| kd_speech::LANGS.iter().position(|l| l.code == c));
            self.set_langs(chosen);
        }
    }

    /// The speech models: which the languages need, which are loaded, and about how much memory each takes.
    fn models_ui(&self, ui: &mut egui::Ui) {
        let Some(st) = &self.status else { return };
        let loaded: u32 = st
            .models
            .iter()
            .filter(|m| m.state == kd_speech::ModelState::Loaded)
            .map(|m| m.memory_mb)
            .sum();
        ui.label(
            RichText::new(format!(
                "Speech models · about {:.1} GB in memory",
                loaded as f64 / 1000.0
            ))
            .size(12.5)
            .color(theme::MUTED),
        );
        egui::Grid::new("models")
            .num_columns(4)
            .spacing([8.0, 4.0])
            .show(ui, |ui| {
                for row in &st.models {
                    if !row.needed && row.state == kd_speech::ModelState::NotLoaded {
                        continue; // (not needed, not loaded: nothing to show)
                    }
                    let (text, colour) = model_state(row);
                    theme::dot(
                        ui,
                        colour,
                        row.state == kd_speech::ModelState::Loaded && row.needed,
                    );
                    ui.label(row.title);
                    ui.label(
                        RichText::new(format!("{:.1} GB", row.memory_mb as f64 / 1000.0))
                            .color(theme::MUTED),
                    );
                    theme::pill(ui, &text, colour);
                    ui.end_row();
                }
            });
    }

    /// The bottom row: updates (the gradient: the main action) and what the update check says. (No licenses button -
    /// the user, 2026-10-06: players do not need it; THIRD_PARTY_NOTICES.txt ships in the install folder.)
    fn footer_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let label = match &self.update {
                Some(i) if cfg!(windows) && i.installer_url.is_some() => {
                    format!("Update to {}", i.version)
                }
                Some(i) => format!("Get {}", i.version),
                None => "Check for updates".into(),
            };
            if theme::primary_button(ui, &label, !self.upd_busy).clicked() {
                self.check_updates(false);
            }
            ui.label(RichText::new(&self.upd_text).color(theme::MUTED));
        });
    }

    /// The game mod picker (a card; its list in a popup, each with its mod's page) and the connection.
    fn header_ui(&mut self, ui: &mut egui::Ui) {
        let kind = self.kind.clone();
        let mut switch = None;
        let mut remove = None;
        let mut add = false;
        ui.horizontal(|ui| {
            let resp = theme::game_button(ui, &kind.name, &kind.mod_name, 330.0);
            egui::Popup::from_toggle_button_response(&resp).width(360.0).show(|ui| {
                ui.label(RichText::new("GAME MODS").size(11.5).family(theme::semibold()).color(theme::MUTED));
                for g in &self.kinds {
                    if theme::game_row(ui, &g.name, &g.mod_name, g.id == self.game_id, !g.builtin, 346.0).clicked() && g.id != self.game_id {
                        switch = Some(g.id.clone());
                    }
                    ui.horizontal(|ui| {
                        if !g.mod_url.is_empty() && ui.link(RichText::new("Mod page ↗").size(12.5).color(theme::ACCENT_TEXT)).clicked() {
                            open_url(&g.mod_url);
                        }
                        if !g.builtin && ui.link(RichText::new("Remove").size(12.5).color(theme::MUTED)).clicked() {
                            remove = Some(g.clone());
                        }
                    });
                    ui.add_space(2.0);
                }
                for (path, why) in kd_games::bad_profiles() {
                    let name = path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default();
                    ui.label(RichText::new(format!("{name} not loaded: {why}")).size(12.0).color(theme::WARN));
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui.button("Add game mod...").clicked() {
                        add = true;
                    }
                    if ui.link(RichText::new("Open the profiles folder").size(12.5).color(theme::ACCENT_TEXT)).clicked() {
                        let dir = kd_games::profiles_dir();
                        let _ = std::fs::create_dir_all(&dir);
                        open_url(&dir.display().to_string());
                    }
                });
                ui.label(RichText::new("A game mod made for Koetama comes with a profile file (.json): add it here.").size(12.0).color(theme::MUTED));
            });
            let (text, colour) = match &self.status {
                Some(st) if !st.error.is_empty() => (st.error.clone(), theme::BAD),
                Some(st) if st.state == "connected" => (format!("Connected to {}", kind.name), theme::GOOD),
                Some(st) if st.state == "paused" => (format!("{} paused (or the level ended)", kind.name), theme::WARN),
                _ => (format!("Waiting for {}: start it with {}", kind.name, kind.needs), theme::WARN),
            };
            ui.add_space(4.0);
            theme::dot(ui, colour, true);
            ui.add(egui::Label::new(RichText::new(text).family(theme::semibold())).wrap());
        });
        if let Some(id) = switch {
            self.switch_game(&id);
        }
        if let Some(k) = remove {
            self.removing = Some(k);
        }
        if add {
            self.add_profile();
        }
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new(&self.where_text).size(12.5).color(theme::MUTED));
            if !kind.mod_url.is_empty()
                && ui.link(RichText::new(format!("Get the {} mod ↗", kind.mod_name)).size(12.5).color(theme::ACCENT_TEXT)).clicked()
            {
                open_url(&kind.mod_url);
            }
            if !self.profile_msg.is_empty() {
                ui.label(RichText::new(&self.profile_msg).size(12.5).color(theme::MUTED));
            }
        });
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
        let Some(info) = self.update.clone() else {
            return;
        };
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
                        Some(i) if cfg!(windows) && i.installer_url.is_some() => {
                            "A new version is ready.".into()
                        }
                        Some(_) => "A new version is out (download page).".into(),
                    };
                    self.update = r;
                }
                UpdateMsg::Failed(e, quiet) => {
                    if !quiet {
                        self.upd_text = match e.strip_prefix(kd_update::NO_RELEASES) {
                            // (not "the latest version": nothing to compare with - the page may have moved)
                            Some(at) => format!("No releases found on GitHub ({}).", at.trim_start_matches(" at ")),
                            None => format!("could not check for updates ({e})"),
                        };
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
        egui::ComboBox::from_id_salt(id)
            .width(330.0)
            .selected_text(current.clone())
            .show_ui(ui, |ui| {
                for name in std::iter::once(DEFAULT.to_string()).chain(devs.iter().cloned()) {
                    if ui.selectable_value(current, name.clone(), name).changed() {
                        changed = true;
                    }
                }
            });
        changed
    }
}

/// Windows: the title bar dark (and, on Windows 11, in the window's own colour) - Windows 10 ignores the theme request.
fn dark_title_bar(frame: &eframe::Frame) {
    #[cfg(windows)]
    {
        use raw_window_handle::{HasWindowHandle, RawWindowHandle};
        use windows_sys::Win32::Graphics::Dwm::DwmSetWindowAttribute;
        let Ok(h) = frame.window_handle() else { return };
        let RawWindowHandle::Win32(w) = h.as_raw() else { return };
        let hwnd = w.hwnd.get() as *mut std::ffi::c_void;
        unsafe {
            let on: i32 = 1;
            // (DWMWA_USE_IMMERSIVE_DARK_MODE: 20; 19 on Windows 10 before 20H1)
            for attr in [20u32, 19] {
                if DwmSetWindowAttribute(hwnd, attr, &on as *const i32 as *const _, 4) == 0 {
                    break;
                }
            }
            let bg = theme::BG;
            let colour: u32 = bg.r() as u32 | (bg.g() as u32) << 8 | (bg.b() as u32) << 16; // (COLORREF: 0x00BBGGRR)
            DwmSetWindowAttribute(hwnd, 35, &colour as *const u32 as *const _, 4); // (DWMWA_CAPTION_COLOR, Windows 11)
        }
    }
    #[cfg(not(windows))]
    let _ = frame;
}

/// Opens a web page or a file with the system's program for it.
fn open_url(target: &str) {
    #[cfg(windows)]
    let _ = std::process::Command::new("cmd")
        .args(["/C", "start", "", target])
        .spawn();
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
        let s = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        format!("{:02}:{:02}:{:02}", (s / 3600) % 24, (s / 60) % 60, s % 60)
    }
}

impl eframe::App for App {
    fn ui(&mut self, ui: &mut egui::Ui, frame: &mut eframe::Frame) {
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
        self.profiles_ui(&ctx);
        ctx.request_repaint_after(Duration::from_millis(250));

        if !self.dressed {
            // (the dark title bar: asked once the window exists)
            ctx.send_viewport_cmd(egui::ViewportCommand::SetTheme(egui::SystemTheme::Dark));
            dark_title_bar(frame);
            self.dressed = true;
        }
        egui::Panel::bottom("footer")
            .resizable(false)
            .show_separator_line(false)
            .frame(
                egui::Frame::new()
                    .fill(theme::BG)
                    .inner_margin(egui::Margin {
                        left: 14,
                        right: 14,
                        top: 8,
                        bottom: 12,
                    }),
            )
            .show(ui, |ui| self.footer_ui(ui));
        egui::Frame::new()
            .fill(theme::BG)
            .inner_margin(egui::Margin {
                left: 14,
                right: 14,
                top: 14,
                bottom: 0,
            })
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                        self.header_ui(ui);
                        ui.add_space(4.0);

                        // ---- sound
                        // (a translate-only game mod: no sound to set)
                        if self.kind.voices || self.kind.speech { theme::card(ui, "Sound", |ui| {
                            egui::Grid::new("sound")
                                .num_columns(3)
                                .spacing([12.0, 8.0])
                                .show(ui, |ui| {
                                    if self.kind.speech {
ui.label(RichText::new("Microphone").color(theme::MUTED));
                                    if Self::device_box(ui, "mic", &mut self.mic, &self.ins) {
                                        self.settings.set("mic", self.mic.clone());
                                        self.settings.save();
                                        let d = Self::device(&self.mic);
                                        if let Some(rt) = self.rt.as_mut() {
                                            rt.set_mic(d);
                                        }
                                    }
                                    let st = self.status.as_ref();
                                    let lvl = st
                                        .filter(|s| s.mic == "listening" || s.mic == "talking")
                                        .map(|s| ((s.level + 60.0) / 60.0).clamp(0.0, 1.0))
                                        .unwrap_or(0.0);
                                    theme::meter(
                                        ui,
                                        lvl as f32,
                                        egui::vec2(110.0, 8.0),
                                        st.is_some_and(|s| s.mic == "talking"),
                                    );
                                    ui.end_row();
                                    }
if self.kind.voices {
ui.label(RichText::new("Speakers").color(theme::MUTED));
                                    if Self::device_box(ui, "out", &mut self.out, &self.outs) {
                                        self.settings.set("out", self.out.clone());
                                        self.settings.save();
                                        let d = Self::device(&self.out);
                                        if let Some(rt) = self.rt.as_mut() {
                                            rt.set_output(d);
                                        }
                                    }
                                    ui.end_row();
                                    ui.label(RichText::new("Volume").color(theme::MUTED));
                                    if ui
                                        .add(
                                            egui::Slider::new(&mut self.volume, 0.0..=100.0)
                                                .show_value(false),
                                        )
                                        .changed()
                                    {
                                        self.settings.set(
                                            "volume",
                                            (self.volume / 100.0 * 100.0).round() / 100.0,
                                        );
                                        if let Some(rt) = self.rt.as_mut() {
                                            rt.set_volume(self.volume / 100.0);
                                        }
                                    }
                                    ui.label(
                                        RichText::new(format!("{:.0} %", self.volume))
                                            .color(theme::MUTED),
                                    );
                                    ui.end_row();
                                    // (real voices: the room on the relay)
                                    if let Some(v) = self.status.as_ref().and_then(|s| s.voice.as_ref()) {
                                        ui.label(RichText::new("Voice chat").color(theme::MUTED));
                                        let (text, colour) = voice_text(v);
                                        theme::pill(ui, &text, colour);
                                        ui.end_row();
                                    }
}
                                });
                        }); }
                        if ui.ctx().input(|i| i.pointer.any_released()) {
                            self.settings.save(); // (the volume: saved when the slider is let go)
                        }
                        ui.add_space(2.0);

                        // ---- speech to text
                        if self.kind.speech { theme::card(ui, "Speech to text", |ui| {
                            self.languages_ui(ui);
                            ui.add_space(2.0);
                            self.models_ui(ui);
                            if let Some(st) = &self.status {
                                let mic = match (&st.download, st.mic) {
                                    (Some(d), _) => format!(
                                        "downloading the speech model, {}",
                                        download_text(d)
                                    ),
                                    (None, "wanted") => "starting".into(),
                                    (None, "loading") => "loading the speech models...".into(),
                                    (None, "listening") => "listening".into(),
                                    (None, "talking") => "hearing you".into(),
                                    _ => "off".into(),
                                };
                                ui.add_space(2.0);
                                ui.horizontal(|ui| {
                                    ui.label(RichText::new("Microphone").color(theme::MUTED));
                                    let on = st.mic == "listening" || st.mic == "talking";
                                    theme::pill(
                                        ui,
                                        &mic,
                                        if on { theme::GOOD } else { theme::MUTED },
                                    );
                                });
                                let (label, text) = if !st.live.is_empty() {
                                    ("Hearing", st.live.clone())
                                } else if !st.last.is_empty() {
                                    ("You said", st.last.clone())
                                } else {
                                    ("Hearing", String::new())
                                };
                                let n = text.chars().count();
                                let text: String =
                                    text.chars().skip(n.saturating_sub(160)).collect();
                                theme::sunk(ui, |ui| {
                                    ui.horizontal_wrapped(|ui| {
                                        ui.label(
                                            RichText::new(format!("{label} ·")).color(theme::MUTED),
                                        );
                                        if text.is_empty() {
                                            ui.label(
                                                RichText::new("what you say appears here")
                                                    .color(theme::MUTED)
                                                    .italics(),
                                            );
                                        } else {
                                            ui.label(
                                                RichText::new(text)
                                                    .size(15.0)
                                                    .color(theme::ACCENT_TEXT),
                                            );
                                        }
                                    });
                                });
                            }
                        }); }
                        ui.add_space(2.0);

                        // ---- translation (a game that uses it: the rules are set in the game)
                        if let Some(rules) = self.status.as_ref().and_then(|s| s.translate.as_ref()) {
                            theme::card(ui, "Translation", |ui| {
                                if rules.is_empty() {
                                    ui.label(
                                        RichText::new("off - the languages to translate are chosen in the game")
                                            .color(theme::MUTED),
                                    );
                                }
                                egui::Grid::new("translation").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
                                    for r in rules {
                                        let (rule, state, colour) = rule_text(r);
                                        ui.label(RichText::new(rule).color(theme::FG));
                                        theme::pill(ui, &state, colour);
                                        ui.end_row();
                                    }
                                });
                                ui.label(
                                    RichText::new("on this PC, with Mozilla's translation models")
                                        .size(12.0)
                                        .color(theme::MUTED),
                                );
                            });
                            ui.add_space(2.0);
                        }

                        // ---- the log
                        theme::sunk(ui, |ui| {
                            egui::ScrollArea::vertical()
                                .id_salt("log")
                                .max_height(130.0)
                                .stick_to_bottom(true)
                                .auto_shrink([false, true])
                                .show(ui, |ui| {
                                    for line in &self.log_lines {
                                        ui.label(
                                            RichText::new(line).monospace().color(theme::MUTED),
                                        );
                                    }
                                });
                        });
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
/// renderer cannot start), OpenGL (glow) elsewhere. KOETAMA_RENDERER=wgpu|glow chooses.
pub fn renderer() -> eframe::Renderer {
    match std::env::var("KOETAMA_RENDERER").ok().as_deref() {
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
            .with_inner_size([820.0, 780.0])
            .with_min_inner_size([620.0, 560.0])
            .with_icon(egui::IconData {
                rgba: include_bytes!("../../../assets/koetama-128.rgba").to_vec(),
                width: 128,
                height: 128,
            }),
        renderer: first,
        ..Default::default()
    };
    match eframe::run_native(
        paths::APP_NAME,
        opts,
        Box::new(|cc| Ok(Box::new(App::new(cc)))),
    ) {
        Ok(()) => 0,
        Err(e) if std::env::var_os("KOETAMA_RENDERER").is_none() => {
            // (that renderer would not start here: once more with the other one - in a new process, as a window
            //  system can be set up only once per process)
            let other = if first == eframe::Renderer::Wgpu {
                "glow"
            } else {
                "wgpu"
            };
            crate::instance::release();
            let args: Vec<String> = std::env::args().skip(1).collect();
            match std::env::current_exe().and_then(|exe| {
                std::process::Command::new(exe)
                    .args(args)
                    .env("KOETAMA_RENDERER", other)
                    .status()
            }) {
                Ok(st) => st.code().unwrap_or(1),
                Err(e2) => {
                    message(&format!(
                        "{} could not open its window: {e} ({e2})",
                        paths::APP_NAME
                    ));
                    1
                }
            }
        }
        Err(e) => {
            message(&format!(
                "{} could not open its window: {e}",
                paths::APP_NAME
            ));
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
        MessageBoxW(
            std::ptr::null_mut(),
            t.as_ptr(),
            c.as_ptr(),
            MB_OK | MB_ICONINFORMATION,
        );
    }
    #[cfg(not(windows))]
    eprintln!("{text}");
}
