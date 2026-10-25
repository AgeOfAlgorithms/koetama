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

mod llm;
mod models;
mod profiles;
mod updates;

use models::Deleting;
use updates::UpdateMsg;

const DEFAULT: &str = "(system default)";
/// the automatic gain past this (dB): the window says the microphone is quiet (a normal one needs about +5..+15)
const QUIET_MIC_DB: f32 = 22.0;

/// A language as the window shows it: its own name, and the English one when it differs.
fn lang_name(code: &str) -> String {
    match kd_speech::lang_info(code) {
        Some(l) if l.name != l.english => format!("{} ({})", l.name, l.english),
        Some(l) => l.name.to_string(),
        None => code.to_string(),
    }
}

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

/// A translation model's language by Mozilla's code (the models translate writing: Chinese by its script).
fn model_lang(code: &str) -> String {
    match code {
        "zh-Hans" => "Chinese (Simplified)".to_string(),
        "zh-Hant" => "Chinese (Traditional)".to_string(),
        c => english_name(c),
    }
}

/// A translation pair as the window says it: "Japanese → English", its state in words and its colour.
pub fn pair_text(r: &kd_translate::service::PairStatus) -> (String, String, Color32) {
    use kd_translate::service::State;
    let pair = format!("{} → {}", english_name(&r.from), english_name(&r.to));
    let (state, colour) = match r.state {
        State::Ready => ("ready".to_string(), theme::GOOD),
        State::Downloading(f) => (format!("downloading {} %", kd_translate::service::percent(f)), theme::WARN),
        State::Loading => ("loading".to_string(), theme::WARN),
        State::Unavailable => ("no model for it".to_string(), theme::MUTED),
        State::NotDownloaded => ("not downloaded (downloads are off)".to_string(), theme::MUTED),
        State::Error => ("failed - trying again in a minute".to_string(), theme::BAD),
    };
    (pair, state, colour)
}

/// The translation as the command line says it: "into en: ja → en ready, ko → en 42 %" ("off").
pub fn translate_line(st: &kd_translate::service::Status) -> String {
    use kd_translate::service::State;
    if st.into.is_empty() {
        return "off".into();
    }
    let parts: Vec<String> = st
        .pairs
        .iter()
        .map(|r| {
            let state = match r.state {
                State::Downloading(f) => format!("{} %", kd_translate::service::percent(f)),
                State::NotDownloaded => "not downloaded".to_string(),
                ref s => s.word().to_string(),
            };
            format!("{} → {} {state}", r.from, r.to)
        })
        .collect();
    if parts.is_empty() {
        format!("into {}", st.into)
    } else {
        format!("into {}: {}", st.into, parts.join(", "))
    }
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
    /// Mic boost, dB 0..=kd_voice::agc::BOOST_MAX_DB, after the automatic gain (the voice sent; saved as "mic_boost")
    mic_boost: f64,
    /// noise suppression for the voice sent (saved as "noise": off / standard / strong)
    noise: kd_voice::denoise::Level,
    /// the microphone test's "Hear yourself" (not saved)
    hear_self: bool,
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
    /// the language the game's chat is translated into ("": off), whether models download when needed, and the
    /// languages offered (Mozilla's list as kept on this PC)
    translate_into: String,
    translate_downloads: bool,
    /// languages the player speaks but has translated anyway ("translate_also")
    translate_also: Vec<String>,
    targets: Vec<&'static str>,
    /// the host's LLM for game mods' agents (llm_setting.rs), and the key being typed (empty: the saved one stays)
    llm: crate::llm_setting::LlmSetting,
    llm_key: String,
    /// "Try it": what was typed, the request under way, and its answer (or why there is none)
    llm_try: String,
    llm_trying: Option<Receiver<Result<kd_llm::Reply, String>>>,
    llm_tried: Result<String, String>,
    /// what the LLM has cost this month: ONE count, shared with the running game's agents (Options.llm_ledger); the
    /// month it is for (a new one starts it over); OpenRouter's price list on its way (loaded on a thread, once the
    /// host has a model set: a player who never sets one up fetches nothing) and whether it was asked for
    llm_ledger: std::sync::Arc<kd_llm::Ledger>,
    llm_month: String,
    llm_prices: Option<Receiver<Option<kd_llm::Prices>>>,
    llm_prices_asked: bool,
    /// the translation models on this PC (the "Downloaded" list) and when the folder was last looked at
    downloaded: Vec<kd_translate::catalog::OnDisk>,
    downloaded_at: Option<Instant>,
    /// the speech models' files on this PC (looked at every few seconds, as the translation models')
    speech_disk: Vec<(&'static str, Option<kd_speech::OnDisk>)>,
    speech_disk_at: Option<Instant>,
    /// a model to delete, once the player confirms
    deleting: Option<Deleting>,
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
    /// a code typed to join a hosted game, and the one joined (PROTOCOL.md "Hub"; not kept between runs)
    join_input: String,
    joined: Option<String>,
}

impl App {
    fn new(cc: &eframe::CreationContext) -> App {
        crate::fonts::install(&cc.egui_ctx);
        theme::apply(&cc.egui_ctx);
        let settings = Settings::load();
        let llm = crate::llm_setting::LlmSetting::load(&settings);
        let llm_spent = llm.spent_this_month();
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
            mic_boost: settings.f64("mic_boost", 0.0).clamp(0.0, kd_voice::agc::BOOST_MAX_DB as f64),
            noise: settings.str("noise").as_deref().and_then(kd_voice::denoise::Level::from_name).unwrap_or(kd_voice::denoise::Level::Standard),
            hear_self: false,
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
            translate_into: String::new(),
            translate_downloads: true,
            translate_also: Vec::new(),
            targets: kd_translate::catalog::offered_targets(&kd_translate::catalog::root()),
            llm,
            llm_key: String::new(),
            llm_try: String::new(),
            llm_trying: None,
            llm_tried: Ok(String::new()),
            llm_ledger: kd_llm::Ledger::new(llm_spent),
            llm_month: crate::llm_setting::this_month(),
            llm_prices: None,
            llm_prices_asked: false,
            downloaded: Vec::new(),
            downloaded_at: None,
            speech_disk: Vec::new(),
            speech_disk_at: None,
            deleting: None,
            dressed: false,
            kinds: Vec::new(),
            kind: kd_games::by_id(""),
            picking: None,
            preview: None,
            removing: None,
            profile_msg: String::new(),
            join_input: String::new(),
            joined: None,
        };
        app.kinds = kd_games::games();
        app.translate_into = app.settings.str("translate_into").unwrap_or_default();
        app.translate_downloads = app.settings.bool("translate_downloads", true);
        app.translate_also = app
            .settings
            .0
            .get("translate_also")
            .and_then(|v| v.as_array())
            .map(|a| a.iter().filter_map(|l| l.as_str().map(String::from)).collect())
            .unwrap_or_default();
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
        let kind = match &self.joined {
            Some(code) => kd_games::GameKind::joined(code),
            None => kd_games::by_id(&self.game_id),
        };
        let opts = Options {
            out_device: Self::device(&self.out),
            mic_device: Self::device(&self.mic),
            volume: self.volume / 100.0,
            mic_boost_db: self.mic_boost as f32,
            noise: self.noise,
            langs: self.langs.clone(),
            translate_into: Some(self.translate_into.clone()).filter(|l| !l.is_empty()),
            translate_also: self.translate_also.clone(),
            translate_downloads: self.translate_downloads,
            // (the agents' LLM: the host's, for a game whose profile uses it and that they turned it on for)
            llm: if self.joined.is_none() { self.llm.config_for(&kind.id) } else { None },
            llm_ledger: Some(self.llm_ledger.clone()),
            ..Default::default()
        };
        let rt = Runtime::start(kind.clone(), self.logger(), opts, None);
        let (found, where_) = rt.game.lock().unwrap().locate();
        self.where_text = if found { format!("{}: {where_}", kind.name) } else { where_ };
        if self.joined.is_none() {
            self.game_id = kind.id.clone();
            self.settings.set("game", kind.id.clone());
            self.settings.save();
        }
        self.kind = kind;
        self.rt = Some(rt);
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

    /// The player changed the translation setting: saved, and the translator follows.
    fn set_translation(&mut self) {
        self.settings.set("translate_into", self.translate_into.clone());
        self.settings.set("translate_downloads", self.translate_downloads);
        self.settings.set("translate_also", serde_json::Value::from(self.translate_also.clone()));
        self.settings.save();
        let into = Some(self.translate_into.clone()).filter(|l| !l.is_empty());
        if let Some(rt) = self.rt.as_mut() {
            rt.set_translation(into, self.translate_downloads, self.translate_also.clone());
        }
    }

    /// "Translation": the language to translate chat into (Off, or one Mozilla's models translate into), whether
    /// models download when needed, and each pair in use with its state.
    fn translation_ui(&mut self, ui: &mut egui::Ui) {
        let mut changed = false;
        egui::Grid::new("translation-setting").num_columns(2).spacing([12.0, 8.0]).show(ui, |ui| {
            ui.label(RichText::new("Translate chat into").color(theme::MUTED));
            let shown = if self.translate_into.is_empty() { "Off".to_string() } else { lang_name(&self.translate_into) };
            egui::ComboBox::from_id_salt("translate-into").width(330.0).selected_text(shown).show_ui(ui, |ui| {
                changed |= ui.selectable_value(&mut self.translate_into, String::new(), "Off").changed();
                for code in &self.targets {
                    changed |= ui.selectable_value(&mut self.translate_into, code.to_string(), lang_name(code)).changed();
                }
            });
            ui.end_row();
            ui.label("");
            changed |= ui.checkbox(&mut self.translate_downloads, "Download translation models when needed").changed();
            ui.end_row();
            // (the languages the player speaks: left as they are unless unticked here - then translated like any other)
            let mine: Vec<String> = self.langs.iter().filter(|l| **l != self.translate_into).cloned().collect();
            if !mine.is_empty() && !self.translate_into.is_empty() {
                ui.label(RichText::new("Don't translate").color(theme::MUTED));
                ui.horizontal_wrapped(|ui| {
                    for l in &mine {
                        let mut keep = !self.translate_also.contains(l);
                        if ui.checkbox(&mut keep, lang_name(l)).changed() {
                            self.translate_also.retain(|x| x != l);
                            if !keep {
                                self.translate_also.push(l.clone());
                            }
                            changed = true;
                        }
                    }
                });
                ui.end_row();
            }
        });
        if changed {
            self.set_translation();
        }
        if self.translate_into.is_empty() {
            ui.label(
                RichText::new("Off: the game's chat is shown as it is, and nothing is downloaded.")
                    .size(12.5)
                    .color(theme::MUTED),
            );
            self.translation_models_ui(ui);
            return;
        }
        let st = self.status.as_ref().and_then(|s| s.translate.clone()).unwrap_or_default();
        if st.pairs.is_empty() {
            ui.label(
                RichText::new(
                    "Chat lines in a language you don't speak are translated as they come; the languages you speak are \
                     left alone. A language's model is fetched the first time it shows up (about 20-55 MB).",
                )
                .size(12.5)
                .color(theme::MUTED),
            );
        }
        self.translation_models_ui(ui);
        ui.label(RichText::new("on this PC, with Mozilla's translation models").size(12.0).color(theme::MUTED));
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

    /// The bottom row: updates (the gradient: the main action) and what the update check says. (No licenses button -
    /// the user, 2026-10-06: players do not need it; THIRD_PARTY_NOTICES.txt ships in the install folder.)
    fn footer_ui(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            let label = match &self.update {
                Some(i) if cfg!(windows) && i.installable() => {
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
        let mut join = None;
        ui.horizontal(|ui| {
            let resp = theme::game_button(ui, &kind.name, &kind.mod_name, &kind.author, 330.0);
            egui::Popup::from_toggle_button_response(&resp).width(440.0).show(|ui| {
                ui.label(RichText::new("GAME MODS").size(11.5).family(theme::semibold()).color(theme::MUTED));
                for g in &self.kinds {
                    // (right of its row: the mod's page, where it has one, then Remove for a community profile;
                    // a slot left empty keeps the cards lined up)
                    ui.horizontal(|ui| {
                        ui.spacing_mut().item_spacing.x = 4.0;
                        if theme::game_row(ui, &g.name, &g.mod_name, &g.author, g.id == self.game_id, !g.builtin, 350.0).clicked() && g.id != self.game_id {
                            switch = Some(g.id.clone());
                        }
                        if g.mod_url.is_empty() {
                            ui.add_space(26.0);
                        } else if theme::link_button(ui, theme::CARD_H).on_hover_text("Mod page").clicked() {
                            open_url(&g.mod_url);
                        }
                        // (apart from the link: a click meant for one doesn't land on the other)
                        ui.add_space(10.0);
                        if !g.builtin && theme::remove_button(ui, theme::CARD_H).on_hover_text("Remove this game mod").clicked() {
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
                // (joining with a code: only for the games whose profile says their mod runs on the host alone)
                let mut hosted: Vec<String> = kd_games::games().iter().filter(|g| g.profile.hosted).map(|g| g.name.clone()).collect();
                hosted.dedup();
                if !hosted.is_empty() {
                    ui.separator();
                    ui.label(RichText::new("JOIN A HOSTED GAME").size(11.5).family(theme::semibold()).color(theme::MUTED));
                    ui.horizontal(|ui| {
                        ui.add(egui::TextEdit::singleline(&mut self.join_input).hint_text("K7QF-4MXA").desired_width(130.0));
                        let ok = kd_voice::hub::normalize_code(&self.join_input).is_some();
                        if ui.add_enabled(ok, egui::Button::new("Join")).clicked() {
                            join = Some(self.join_input.trim().to_uppercase());
                        }
                    });
                    ui.label(
                        RichText::new(format!("For {}: the host's game shows you a code.", hosted.join(", ")))
                            .size(12.0)
                            .color(theme::MUTED),
                    );
                }
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
        if let Some(code) = join {
            if let Some(mut rt) = self.rt.take() {
                rt.stop();
            }
            self.joined = Some(code);
            self.start_game();
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
        self.joined = None;
        self.game_id = id.to_string();
        self.start_game();
    }

    // ---- every 250 ms
    fn refresh(&mut self) {
        if let Some(rt) = self.rt.as_mut() {
            rt.tick();
            self.status = Some(rt.status());
        }
        // (OpenRouter's prices: asked for once a model is set; once here, the running game's agents count with them)
        if !self.llm_prices_asked && !self.llm.model.trim().is_empty() {
            self.llm_prices_asked = true;
            let (tx, rx) = channel();
            std::thread::spawn(move || {
                let _ = tx.send(crate::llm_setting::load_prices());
            });
            self.llm_prices = Some(rx);
        }
        if let Some(Ok(prices)) = self.llm_prices.as_ref().map(|rx| rx.try_recv()) {
            self.llm_prices = None;
            self.llm.prices = prices;
            if let (Some(cfg), Some(rt)) = (self.llm.config_for(&self.game_id), self.rt.as_mut()) {
                rt.set_llm(cfg);
            }
        }
        // (what the LLM spent this month: one ledger, kept in the settings as it grows; a new month starts it over)
        let month = crate::llm_setting::this_month();
        if month != self.llm_month {
            self.llm_month = month;
            self.llm_ledger.set(0.0);
        }
        let spent = self.llm_ledger.spent();
        if (spent - self.llm.spent_this_month()).abs() > 1e-9 {
            self.llm.set_spent(spent);
            self.llm.save(&mut self.settings);
            self.settings.save();
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

/// Opens a web page (http / https) or a folder with the system's program for it - nothing else. Never through a shell:
/// a profile's URL is someone else's text (`cmd /C start` ran `&calc` in one).
fn open_url(target: &str) {
    let web = target.starts_with("https://") || target.starts_with("http://");
    if !web && !std::path::Path::new(target).is_dir() {
        return;
    }
    #[cfg(windows)]
    {
        use windows_sys::Win32::UI::Shell::ShellExecuteW;
        use windows_sys::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        let wide = |s: &str| s.encode_utf16().chain(std::iter::once(0)).collect::<Vec<u16>>();
        let (verb, file) = (wide("open"), wide(target));
        // SAFETY: both strings are NUL-terminated UTF-16 that outlive the call; the other pointers may be null
        unsafe {
            ShellExecuteW(std::ptr::null_mut(), verb.as_ptr(), file.as_ptr(), std::ptr::null(), std::ptr::null(), SW_SHOWNORMAL);
        }
    }
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
        self.delete_modal(&ctx);
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
                // (the game mod picker stays put while the rest scrolls - the user, 2026-10-24)
                self.header_ui(ui);
                ui.add_space(4.0);
                egui::ScrollArea::vertical()
                    .auto_shrink([false, false])
                    .show(ui, |ui| {

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
                                    // (the microphone test: no game needed - the level, the words, and with the voice
                                    //  chat the player's own voice as the others hear it)
                                    let testing = self.status.as_ref().and_then(|s| s.mic_test);
                                    ui.label("");
                                    ui.horizontal(|ui| {
                                        let label = match testing {
                                            Some(left) => format!("Stop the test ({:.0} s)", left.ceil()),
                                            None => "Test microphone".to_string(),
                                        };
                                        if ui.button(label).clicked() {
                                            if let Some(rt) = self.rt.as_mut() {
                                                rt.set_mic_test(testing.is_none(), self.hear_self);
                                            }
                                        }
                                        if self.kind.voices
                                            && ui.checkbox(&mut self.hear_self, "Hear yourself").on_hover_text(
                                                "Plays your voice back as the other players will hear it (use headphones)",
                                            ).changed()
                                        {
                                            if let Some(rt) = self.rt.as_mut() {
                                                rt.set_hearing(self.hear_self);
                                            }
                                        }
                                    });
                                    ui.end_row();
                                    if testing.is_some() {
                                        ui.label("");
                                        ui.label(
                                            RichText::new("Talk normally: the bar should reach about two thirds. What you say shows under \"You said\".")
                                                .size(12.0)
                                                .color(theme::MUTED),
                                        );
                                        ui.end_row();
                                    }
                                    if self.kind.voices {
                                        // (the voice sent: its automatic gain evens players out; this makes this
                                        //  player louder than that, after it)
                                        ui.label(RichText::new("Mic boost").color(theme::MUTED));
                                        let max = kd_voice::agc::BOOST_MAX_DB as f64;
                                        if ui
                                            .add(egui::Slider::new(&mut self.mic_boost, 0.0..=max).show_value(false))
                                            .on_hover_text("Koetama already evens out quiet and loud microphones: this makes your voice louder than that.")
                                            .changed()
                                        {
                                            self.settings.set("mic_boost", self.mic_boost.round());
                                            if let Some(rt) = self.rt.as_mut() {
                                                rt.set_mic_boost(self.mic_boost as f32);
                                            }
                                        }
                                        ui.label(RichText::new(format!("+{:.0} dB", self.mic_boost)).color(theme::MUTED));
                                        ui.end_row();
                                        // (a quiet microphone: raised a long way, its room noise too - say where to fix it)
                                        if let Some(g) = self.status.as_ref().and_then(|s| s.mic_gain_db).filter(|g| *g > QUIET_MIC_DB) {
                                            ui.label("");
                                            ui.label(
                                                RichText::new(format!(
                                                    "Your microphone is quiet: Koetama raises it by {g:+.0} dB, and the noise around you \
                                                     with it. Turn its level up in Windows (Settings > Sound > your microphone > \
                                                     Input volume), or with the gain knob on your audio interface."
                                                ))
                                                .size(12.0)
                                                .color(theme::WARN),
                                            );
                                            ui.end_row();
                                        }
                                        // (noise suppression for the voice sent: docs/noise-suppression.md)
                                        ui.label(RichText::new("Noise suppression").color(theme::MUTED));
                                        let before = self.noise;
                                        ui.horizontal(|ui| {
                                            use kd_voice::denoise::Level;
                                            ui.selectable_value(&mut self.noise, Level::Off, "Off")
                                                .on_hover_text("Your voice is sent as your microphone hears it.");
                                            ui.selectable_value(&mut self.noise, Level::Standard, "Standard").on_hover_text(
                                                "Takes out steady noise (fans, hum, a vacuum cleaner) and some keyboard clicks. Costs almost nothing (RNNoise).",
                                            );
                                            ui.selectable_value(&mut self.noise, Level::Strong, "Strong").on_hover_text(
                                                "Takes out keyboard and mouse clicks too, keeping your voice natural. Uses more of your CPU (about 8 % of one core) and adds 30 ms; downloads its model (8 MB) the first time (DeepFilterNet 3).",
                                            );
                                        });
                                        ui.label("");
                                        ui.end_row();
                                        if self.noise != before {
                                            self.settings.set("noise", self.noise.name());
                                            self.settings.save();
                                            if let Some(rt) = self.rt.as_mut() {
                                                rt.set_noise_suppression(self.noise);
                                            }
                                        }
                                    }
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

                        // ---- translation (a game that uses it: the player's own setting, here)
                        if self.kind.translate {
                            theme::card(ui, "Translation", |ui| self.translation_ui(ui));
                            ui.add_space(2.0);
                        }

                        // ---- large language models (a game whose profile uses "llm": its agents and more; the host's setting)
                        if self.joined.is_none() {
                            theme::card(ui, "Large Language Models", |ui| self.llm_ui(ui));
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
