//! The window's "Large Language Models" card: the host's LLM for a game mod's agents, and "Try it".
use super::*;

impl App {
    /// "Large Language Models": the host's LLM for this game's mod, its agents (docs/llm-agents.md): on for this game or
    /// not, the API, address, model, key, the month's limit in tokens in and out, requests a minute; the tokens used
    /// this month, and its state.
    /// Turning it on or off (or it becoming usable) starts the game link again (the hello names "llm" or not).
    pub(super) fn llm_ui(&mut self, ui: &mut egui::Ui) {
        let game = self.kind.id.clone();
        let before = self.llm.config_for(&game).is_some();
        let mut changed = false;
        let mut on = self.llm.on_for(&game);
        if ui.checkbox(&mut on, format!("Let the {} mod use my language model", self.kind.mod_name)).changed() {
            self.llm.set_on(&game, on);
            changed = true;
        }
        ui.label(
            RichText::new(
                "For the host: what the mod sends (what players say to its characters) goes with your key to the \
                 model chosen here, and it costs you. The other players need nothing. Tell them the game uses one.",
            )
            .size(12.0)
            .color(theme::MUTED),
        );
        egui::Grid::new("llm").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
            ui.label(RichText::new("API").color(theme::MUTED));
            let api = self.llm.api;
            egui::ComboBox::from_id_salt("llm-api")
                .width(220.0)
                .selected_text(match api {
                    kd_llm::Api::OpenAi => "OpenAI-compatible",
                    kd_llm::Api::Anthropic => "Anthropic",
                })
                .show_ui(ui, |ui| {
                    changed |= ui.selectable_value(&mut self.llm.api, kd_llm::Api::OpenAi, "OpenAI-compatible").changed();
                    changed |= ui.selectable_value(&mut self.llm.api, kd_llm::Api::Anthropic, "Anthropic").changed();
                });
            ui.end_row();
            ui.label(RichText::new("Address").color(theme::MUTED));
            let r = ui.add(egui::TextEdit::singleline(&mut self.llm.url).hint_text(self.llm.api.default_url()).desired_width(330.0));
            changed |= r.lost_focus();
            ui.end_row();
            ui.label(RichText::new("Model").color(theme::MUTED));
            let r = ui.add(egui::TextEdit::singleline(&mut self.llm.model).hint_text("the model's id, as the API names it").desired_width(330.0));
            changed |= r.lost_focus();
            ui.end_row();
            ui.label(RichText::new("Key").color(theme::MUTED));
            let hint = if self.llm.key.is_empty() { "your API key" } else { "saved (type to replace it)" };
            let r = ui.add(egui::TextEdit::singleline(&mut self.llm_key).password(true).hint_text(hint).desired_width(330.0));
            if r.lost_focus() && !self.llm_key.trim().is_empty() {
                self.llm.key = std::mem::take(&mut self.llm_key).trim().to_string();
                changed = true;
            }
            ui.end_row();
            ui.label(RichText::new("Limit").color(theme::MUTED));
            ui.horizontal(|ui| {
                // (tokens, not dollars - the user, 2026-10-25: a provider's bill can only be guessed, its counts are exact)
                let r = ui.add(tokens(&mut self.llm.limit.input, "tokens in"));
                changed |= r.drag_stopped() || r.lost_focus();
                let r = ui.add(tokens(&mut self.llm.limit.output, "out"));
                changed |= r.drag_stopped() || r.lost_focus();
                ui.label(RichText::new("a month").color(theme::MUTED));
            })
            .response
            .on_hover_text(
                "The most the mod's agents (and Try it) may use each month. Tokens in are everything sent to the model - \
                 cached ones too, as your provider's usage page counts them; out is what it writes.",
            );
            ui.end_row();
            ui.label(RichText::new("Used").color(theme::MUTED));
            ui.horizontal(|ui| {
                let u = self.llm_ledger.used();
                ui.label(RichText::new(format!("{} in, {} out this month", thousands(u.input), thousands(u.output))).color(theme::MUTED))
                    .on_hover_text("The tokens your provider counted for each reply, added up.");
                // (the count can be set back, e.g. to start over - the user, 2026-10-25)
                if ui.small_button("Reset").on_hover_text("Start this month's count over from 0.").clicked() {
                    self.llm_ledger.set(kd_llm::Tokens::default());
                }
            });
            ui.end_row();
            ui.label(RichText::new("Requests").color(theme::MUTED));
            ui.horizontal(|ui| {
                let r = ui.add(egui::DragValue::new(&mut self.llm.per_minute).range(1..=600).suffix(" a minute"));
                changed |= r.drag_stopped() || r.lost_focus();
                ui.label(RichText::new("at most (more lines wait)").size(12.0).color(theme::MUTED));
            });
            ui.end_row();
        });
        self.llm_try_ui(ui);
        if on && !self.llm.complete() {
            ui.label(RichText::new("Add a model and a key to turn it on.").size(12.5).color(theme::WARN));
        }
        if let Some(n) = self.status.as_ref().and_then(|s| s.agent.as_ref()) {
            let (text, colour) = match n.state {
                "ready" => ("ready".to_string(), theme::GOOD),
                "busy" => ("busy: lines wait (requests a minute)".to_string(), theme::WARN),
                "budget" => ("this month's tokens are used up (raise the limit, or Reset)".to_string(), theme::BAD),
                "error" => ("failed".to_string(), theme::BAD),
                _ => ("starting".to_string(), theme::MUTED),
            };
            ui.horizontal(|ui| {
                ui.label(RichText::new("Status").color(theme::MUTED));
                theme::pill(ui, &text, colour);
            });
            // (why it failed, on lines of its own: an API's message can be long)
            if n.state == "error" && !n.why.is_empty() {
                ui.add(egui::Label::new(RichText::new(&n.why).size(12.0).color(theme::BAD)).wrap());
            }
        }
        if changed {
            self.llm.save(&mut self.settings);
            self.settings.save();
            let after = self.llm.config_for(&game);
            if before != after.is_some() {
                // (on or off for this game: the game link starts again, its hello naming "llm" or not)
                self.switch_game(&game);
            } else if let (Some(cfg), Some(rt)) = (after, self.rt.as_mut()) {
                rt.set_llm(cfg);
            }
        }
    }

    /// "Try it": a message to the model as the card is set now (the switch need not be on) and its answer, or exactly
    /// why there is none. Its tokens count in this month's.
    pub(super) fn llm_try_ui(&mut self, ui: &mut egui::Ui) {
        if let Some(rx) = &self.llm_trying {
            if let Ok(r) = rx.try_recv() {
                self.llm_trying = None;
                self.llm_tried = match r {
                    Ok(reply) => {
                        self.llm_ledger.add(reply.tokens());
                        let calls = if reply.calls.is_empty() { String::new() } else { format!(" ({} function calls)", reply.calls.len()) };
                        let cached = if reply.tokens_cached > 0 { format!(" ({} cached)", reply.tokens_cached) } else { String::new() };
                        Ok(format!("{}{calls}  ·  {} tokens in{cached}, {} out", reply.text, reply.tokens_in, reply.tokens_out))
                    }
                    Err(e) => Err(e),
                };
            } else {
                ui.ctx().request_repaint_after(Duration::from_millis(200));
            }
        }
        let cfg = self.llm.config();
        ui.horizontal(|ui| {
            ui.label(RichText::new("Try it").color(theme::MUTED));
            let r = ui.add(egui::TextEdit::singleline(&mut self.llm_try).hint_text("say something to the model").desired_width(330.0));
            let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
            let ready = cfg.is_some() && self.llm_trying.is_none() && !self.llm_try.trim().is_empty();
            let send = ui.add_enabled(ready, egui::Button::new(if self.llm_trying.is_some() { "Waiting..." } else { "Send" })).clicked();
            if (send || enter) && ready {
                let (tx, rx) = channel();
                let (cfg, text) = (cfg.clone().expect("ready"), self.llm_try.trim().to_string());
                std::thread::spawn(move || {
                    let turns = [kd_llm::Turn::User(text)];
                    let system = "You are being tried out from Koetama's settings. Answer briefly.";
                    let _ = tx.send(kd_llm::api::complete(&cfg, system, &[], &turns, 300, Duration::from_secs(60)));
                });
                self.llm_trying = Some(rx);
                self.llm_tried = Ok(String::new());
            }
        });
        if cfg.is_none() {
            ui.label(RichText::new("Add a model and a key to try it.").size(12.0).color(theme::MUTED));
        }
        match &self.llm_tried {
            Ok(t) if !t.is_empty() => {
                ui.label(RichText::new(t).color(theme::FG));
            }
            Err(e) => {
                ui.label(RichText::new(e).size(12.5).color(theme::BAD));
            }
            _ => {}
        }
    }
}

/// A token count to drag or type ("5,000,000 tokens in"; commas and the unit are fine to type).
fn tokens<'a>(v: &'a mut u64, unit: &'static str) -> egui::DragValue<'a> {
    egui::DragValue::new(v)
        .range(0..=10_000_000_000u64)
        .speed(10_000.0)
        .custom_formatter(move |n, _| format!("{} {unit}", thousands(n as u64)))
        .custom_parser(|s| s.chars().filter(char::is_ascii_digit).collect::<String>().parse::<f64>().ok())
}

/// 5000000 -> "5,000,000"
fn thousands(n: u64) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i) % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out
}
