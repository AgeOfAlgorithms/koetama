//! The window's model lists (speech to text, translation): each model in memory / on disk / downloading, with Free
//! memory and a bin, and the dialog that confirms deleting one.
use super::*;

/// A model's state in words, and its colour.
pub(super) fn model_state(row: &crate::runtime::ModelRow, on_disk: bool) -> (String, Color32) {
    use kd_speech::ModelState::*;
    match (&row.state, on_disk) {
        (Loaded, _) if row.needed => ("in memory".into(), theme::GOOD),
        (Loaded, _) => ("in memory · letting go".into(), theme::MUTED),
        (Loading, _) => ("loading".into(), theme::WARN),
        (Downloading(f, d, t), _) => (format!("downloading {}", download_text(&(f.clone(), *d, *t))), theme::WARN),
        (NotLoaded, true) if row.needed => ("on disk".into(), theme::MUTED),
        (NotLoaded, true) => ("on disk · not needed".into(), theme::MUTED),
        (NotLoaded, false) => ("not downloaded".into(), theme::MUTED),
    }
}

/// Sizes as the model lists say them: "1.2 GB", "670 MB", "37 MB".
pub(super) fn gb(bytes: u64) -> String {
    if bytes >= 1_000_000_000 {
        format!("{:.1} GB", bytes as f64 / 1e9)
    } else {
        mb(bytes)
    }
}

pub(super) fn mb(bytes: u64) -> String {
    format!("{} MB", (bytes + 500_000) / 1_000_000)
}

/// The translation directions a pair goes through, by folder ("ja-en"; two through English, as catalog::route).
pub(super) fn pair_keys(from: &str, to: &str) -> Vec<String> {
    use kd_translate::catalog::mozilla_code;
    let (Some(f), Some(t)) = (mozilla_code(from, true), mozilla_code(to, false)) else { return Vec::new() };
    if f == "en" || t == "en" {
        vec![format!("{f}-{t}")]
    } else {
        vec![format!("{f}-en"), format!("en-{t}")]
    }
}

/// A model the player asked to delete, until they confirm.
#[derive(Clone)]
pub(super) enum Deleting {
    Speech { name: &'static str, title: &'static str, bytes: u64 },
    Translation { key: String, label: String, bytes: u64 },
}

/// What a click in a model list asks for (done once the list is drawn).
pub(super) enum ModelAction {
    FreeSpeech(&'static str),
    FreeTranslation(String),
    Delete(Deleting),
}

/// A model's two buttons: Free memory (when it is in memory) and the bin (Ok: Koetama may delete its files, and the
/// hover text; Err: why not). Clicked: (free, delete).
pub(super) fn model_buttons(ui: &mut egui::Ui, in_memory: bool, free_hint: &str, delete: Result<String, String>) -> (bool, bool) {
    // (only the buttons that do something: greyed-out ones looked broken - the user, 2026-10-24; the status says why)
    let free = if in_memory {
        ui.add(egui::Button::new(RichText::new("Free memory").size(12.0)).small()).on_hover_text(free_hint).clicked()
    } else {
        ui.label("");
        false
    };
    let del = match delete {
        Ok(hint) => theme::remove_button(ui, 22.0).on_hover_text(hint).clicked(),
        Err(_) => {
            ui.label("");
            false
        }
    };
    (free, del)
}

impl App {
    /// The translation models: every one on this PC or on its way, each in memory / on disk / downloading, with Free
    /// memory and a bin; then the pairs that have no model (none exists, downloads off, failed).
    pub(super) fn translation_models_ui(&mut self, ui: &mut egui::Ui) {
        use kd_translate::catalog;
        use kd_translate::service::State;
        if self.downloaded_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.downloaded = catalog::downloaded(&catalog::root());
            self.downloaded_at = Some(Instant::now());
        }
        let st = self.status.as_ref().and_then(|s| s.translate.clone()).unwrap_or_default();
        struct Row {
            key: String,
            label: String,
            bytes: Option<u64>,
            memory: bool,
            busy: Option<(String, Color32)>,
        }
        fn row_for(rows: &mut Vec<Row>, key: &str) -> usize {
            if let Some(i) = rows.iter().position(|r| r.key == key) {
                return i;
            }
            let (f, t) = match (key.strip_suffix("-en"), key.strip_prefix("en-")) {
                (Some(f), _) => (f, "en"),
                (_, Some(t)) => ("en", t),
                _ => (key, ""),
            };
            let label = format!("{} → {}", model_lang(f), model_lang(t));
            rows.push(Row { key: key.to_string(), label, bytes: None, memory: false, busy: None });
            rows.len() - 1
        }
        let mut rows: Vec<Row> = Vec::new();
        for d in &self.downloaded {
            let i = row_for(&mut rows, &d.key());
            rows[i].bytes = Some(rows[i].bytes.unwrap_or(0) + d.bytes);
        }
        for id in &st.loaded {
            let i = row_for(&mut rows, id.split_once('/').map_or(id.as_str(), |(k, _)| k));
            rows[i].memory = true;
        }
        let mut problems = Vec::new();
        for p in &st.pairs {
            match p.state {
                State::Downloading(_) | State::Loading => {
                    let (_, text, colour) = pair_text(p);
                    for k in pair_keys(&p.from, &p.to) {
                        let i = row_for(&mut rows, &k);
                        if !rows[i].memory {
                            rows[i].busy = Some((text.clone(), colour));
                        }
                    }
                }
                State::Unavailable | State::NotDownloaded | State::Error => problems.push(pair_text(p)),
                State::Ready => {}
            }
        }
        if rows.is_empty() && problems.is_empty() {
            return;
        }
        rows.sort_by(|a, b| a.label.cmp(&b.label));
        let memory: u64 = rows.iter().filter(|r| r.memory).filter_map(|r| r.bytes).sum();
        let disk: u64 = rows.iter().filter_map(|r| r.bytes).sum();
        ui.add_space(4.0);
        ui.label(
            RichText::new(format!("Translation models · about {} in memory · {} on disk", mb(memory), mb(disk)))
                .size(12.5)
                .color(theme::MUTED),
        );
        let mut act = None;
        egui::Grid::new("translation-models").num_columns(5).spacing([10.0, 4.0]).show(ui, |ui| {
            for r in &rows {
                ui.label(RichText::new(&r.label).color(theme::FG));
                ui.label(RichText::new(r.bytes.map(mb).unwrap_or_default()).color(theme::MUTED));
                let (text, colour) = match (&r.busy, r.memory, r.bytes.is_some()) {
                    (Some(b), _, _) => b.clone(),
                    (None, true, _) => ("in memory".to_string(), theme::GOOD),
                    (None, false, true) => ("on disk".to_string(), theme::MUTED),
                    (None, false, false) => ("not on disk".to_string(), theme::MUTED),
                };
                theme::pill(ui, &text, colour);
                let delete = match (r.bytes, &r.busy) {
                    (_, Some(_)) => Err("Wait until it is ready".to_string()),
                    (Some(b), None) => Ok(format!("Delete it ({}): it downloads again the next time a chat line needs it", mb(b))),
                    (None, None) => Err("Not on disk".to_string()),
                };
                let hint = "Let go of it now: it loads again the next time a chat line needs it";
                let (free, del) = model_buttons(ui, r.memory, hint, delete);
                if free {
                    act = Some(ModelAction::FreeTranslation(r.key.clone()));
                }
                if del {
                    let bytes = r.bytes.unwrap_or(0);
                    act = Some(ModelAction::Delete(Deleting::Translation { key: r.key.clone(), label: r.label.clone(), bytes }));
                }
                ui.end_row();
            }
            for (pair, text, colour) in &problems {
                ui.label(RichText::new(pair).color(theme::FG));
                ui.label("");
                theme::pill(ui, text, *colour);
                ui.end_row();
            }
        });
        if let Some(a) = act {
            self.model_action(a);
        }
    }

    /// A click in a model list: free a model's memory now, or ask before deleting it.
    pub(super) fn model_action(&mut self, a: ModelAction) {
        match a {
            ModelAction::FreeSpeech(name) => {
                if let Some(rt) = self.rt.as_mut() {
                    rt.free_speech_model(name);
                }
            }
            ModelAction::FreeTranslation(key) => {
                if let Some(rt) = self.rt.as_ref() {
                    rt.free_translation(&key);
                }
            }
            ModelAction::Delete(d) => self.deleting = Some(d),
        }
    }

    /// Deleting a model, once the player confirms: its memory let go first, then its files.
    pub(super) fn delete_modal(&mut self, ctx: &egui::Context) {
        let Some(d) = self.deleting.clone() else { return };
        let (title, body) = match &d {
            Deleting::Speech { title, bytes, .. } => (
                format!("Delete {title}?"),
                format!(
                    "Its files ({}) are deleted from this PC. It downloads again the next time the game wants your voice \
                     in a language that needs it.",
                    gb(*bytes)
                ),
            ),
            Deleting::Translation { label, bytes, .. } => (
                format!("Delete the {label} model?"),
                format!("Its files ({}) are deleted from this PC. It downloads again the next time a chat line needs it.", mb(*bytes)),
            ),
        };
        let mut close = false;
        egui::Modal::new(egui::Id::new("model-delete")).show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(RichText::new(&title).size(16.0).family(theme::semibold()));
            ui.label(RichText::new(&body).color(theme::MUTED));
            ui.horizontal(|ui| {
                if ui.button("Delete").clicked() {
                    let line = match &d {
                        Deleting::Speech { name, title, .. } => {
                            if let Some(rt) = self.rt.as_mut() {
                                rt.free_speech_model(name);
                            }
                            match kd_speech::delete_model(name) {
                                Ok(b) => format!("deleted the speech model {title} ({})", gb(b)),
                                Err(e) => format!("could not delete: {e}"),
                            }
                        }
                        Deleting::Translation { key, label, bytes } => {
                            if let Some(rt) = self.rt.as_ref() {
                                rt.free_translation(key);
                            }
                            match kd_translate::catalog::delete(&kd_translate::catalog::root(), key) {
                                Ok(()) => format!("translation: deleted the {label} model ({})", mb(*bytes)),
                                Err(e) => format!("translation: could not delete the {label} model: {e}"),
                            }
                        }
                    };
                    let _ = self.log_tx.send(line);
                    self.downloaded_at = None;
                    self.speech_disk_at = None;
                    close = true;
                }
                if ui.button("Cancel").clicked() {
                    close = true;
                }
            });
        });
        if close {
            self.deleting = None;
        }
    }


    /// The speech models: each the languages need or that is on this PC - in memory / on disk / not downloaded, its
    /// size (on disk, else its download), Free memory and a bin.
    pub(super) fn models_ui(&mut self, ui: &mut egui::Ui) {
        if self.speech_disk_at.is_none_or(|t| t.elapsed() > Duration::from_secs(3)) {
            self.speech_disk = kd_speech::MODEL_INFO.iter().map(|m| (m.name, kd_speech::on_disk(m.name))).collect();
            self.speech_disk_at = Some(Instant::now());
        }
        let Some(st) = &self.status else { return };
        let disk = |name: &str| self.speech_disk.iter().find(|(n, _)| *n == name).and_then(|(_, d)| d.clone());
        let memory: u32 = st.models.iter().filter(|m| m.state == kd_speech::ModelState::Loaded).map(|m| m.memory_mb).sum();
        let on_disk: u64 = self.speech_disk.iter().filter_map(|(_, d)| d.as_ref().map(|d| d.bytes)).sum();
        ui.label(
            RichText::new(format!(
                "Speech models · about {} in memory · {} on disk. The ones your languages need load when the game \
                 wants your voice (downloaded the first time).",
                gb(memory as u64 * 1_000_000),
                gb(on_disk)
            ))
                .size(12.5)
                .color(theme::MUTED),
        );
        let mut act = None;
        egui::Grid::new("models").num_columns(6).spacing([8.0, 4.0]).show(ui, |ui| {
            for row in &st.models {
                let d = disk(row.name);
                if !row.needed && row.state == kd_speech::ModelState::NotLoaded && d.is_none() {
                    continue; // (not needed, not loaded, not on this PC: nothing to show)
                }
                let loaded = row.state == kd_speech::ModelState::Loaded;
                let busy = matches!(row.state, kd_speech::ModelState::Loading | kd_speech::ModelState::Downloading(..));
                let (mut text, colour) = model_state(row, d.is_some());
                // (on disk but not Koetama's to delete: say whose)
                match &d {
                    Some(d) if !d.ours && row.name == "langid" => text.push_str(" · comes with Koetama"),
                    Some(d) if !d.ours => text.push_str(" · Hugging Face cache"),
                    _ => {}
                }
                theme::dot(ui, colour, loaded && row.needed);
                ui.label(row.title);
                let size = d.as_ref().map_or(row.download_mb as u64 * 1_000_000, |d| d.bytes);
                ui.label(RichText::new(gb(size)).color(theme::MUTED));
                theme::pill(ui, &text, colour);
                let delete = match &d {
                    _ if busy => Err("Wait until it is ready".to_string()),
                    Some(d) if d.ours => Ok(format!("Delete its files ({}): it downloads again when it is needed", gb(d.bytes))),
                    Some(_) if row.name == "langid" => Err("It comes with Koetama".to_string()),
                    Some(_) => Err("It is in the Hugging Face cache, not Koetama's folder: left alone".to_string()),
                    None => Err("Not downloaded".to_string()),
                };
                let hint = if row.needed {
                    "Let go of it now: it loads again the next time the game wants your voice"
                } else {
                    "Let go of it now"
                };
                let (free, del) = model_buttons(ui, loaded, hint, delete);
                if free {
                    act = Some(ModelAction::FreeSpeech(row.name));
                }
                if del {
                    let bytes = d.map_or(0, |d| d.bytes);
                    act = Some(ModelAction::Delete(Deleting::Speech { name: row.name, title: row.title, bytes }));
                }
                ui.end_row();
            }
        });
        if let Some(a) = act {
            self.model_action(a);
        }
    }
}
