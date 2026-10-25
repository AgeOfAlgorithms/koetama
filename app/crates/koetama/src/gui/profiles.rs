//! The window's game mod profiles: adding one (a file -> a preview of what it does -> installed) and removing one.
use super::*;

impl App {
    // ---- game mod profiles: add (a file -> a preview of what it does -> installed), remove
    pub(super) fn add_profile(&mut self) {
        let (tx, rx) = channel();
        std::thread::spawn(move || {
            let _ = tx.send(crate::dialog::pick_profile());
        });
        self.picking = Some(rx);
    }

    pub(super) fn profiles_ui(&mut self, ctx: &egui::Context) {
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
                        theme::game_row(ui, &k.name, &k.mod_name, &k.author, false, true, 500.0);
                        ui.label(RichText::new(&k.mod_url).size(12.5).color(theme::MUTED));
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
}
