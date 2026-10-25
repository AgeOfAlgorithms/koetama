//! The window's updates: checking for a new Koetama, downloading it, and running its installer.
use super::*;

pub(super) enum UpdateMsg {
    Checked(Option<Release>),
    Failed(String, bool),
    Progress(f64),
    Downloaded(std::path::PathBuf),
    DownloadFailed(String),
}

impl App {
    // ---- updates
    pub(super) fn check_updates(&mut self, quiet: bool) {
        if self.update.is_some() {
            return self.do_update();
        }
        if !quiet {
            self.upd_text = "checking...".into();
        }
        let tx = self.upd_tx.clone();
        std::thread::spawn(move || {
            let _ = match kd_update::check(Duration::from_secs(10)) {
                Ok(r) => tx.send(UpdateMsg::Checked(r)),
                Err(e) => tx.send(UpdateMsg::Failed(e, quiet)),
            };
        });
    }

    pub(super) fn do_update(&mut self) {
        let Some(info) = self.update.clone() else {
            return;
        };
        if !(cfg!(windows) && info.installable()) {
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

    pub(super) fn handle_updates(&mut self, ctx: &egui::Context) {
        while let Ok(m) = self.upd_rx.try_recv() {
            match m {
                // (the check at start too: its answer shows, so the player sees it was done and need not click)
                UpdateMsg::Checked(r) => {
                    self.upd_text = match &r {
                        None => "You have the latest version.".into(),
                        Some(i) if cfg!(windows) && i.installable() => {
                            "A new version is ready.".into()
                        }
                        // (its checksums are not signed with the release key, or do not hold: the page only)
                        Some(Release { refused: Some(why), .. }) if cfg!(windows) => {
                            format!("A new version is out, but not installed from here: {why}. Get it from the download page.")
                        }
                        Some(_) => "A new version is out (download page).".into(),
                    };
                    self.update = r;
                }
                UpdateMsg::Failed(e, quiet) => {
                    self.upd_text = match e.strip_prefix(kd_update::NO_RELEASES) {
                        // (not "the latest version": nothing to compare with - the page may have moved)
                        Some(at) => format!("No releases found on GitHub ({}).", at.trim_start_matches(" at ")),
                        // (at start, offline: said briefly - the button tries again)
                        None if quiet => "Could not check for updates at start.".into(),
                        None => format!("could not check for updates ({e})"),
                    };
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
}
