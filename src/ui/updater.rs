//! The "Update available" dialog and the Settings "About" group (upstream `DesktopApp`'s update
//! dialog over `DesktopUpdateChecker`). A portable install updates itself; anything else opens
//! the release page, as upstream does.
use super::*;
use crate::update::{self, Release};

pub(super) enum UpdateEvent {
    /// (asked from Settings, result)
    Checked(bool, Result<Option<Release>, String>),
    Progress(f32),
    Staged(Result<PathBuf, String>),
    Applied(Result<(), String>),
}

#[derive(Default)]
pub(super) struct Updates {
    checking: bool,
    available: Option<Release>,
    /// The dialog is open.
    prompt: bool,
    /// Download progress, 0 to 1, while the package downloads.
    downloading: Option<f32>,
    /// The extracted package, ready to swap in.
    staged: Option<PathBuf>,
    applying: bool,
    /// The last check's outcome, for Settings.
    status: String,
}

impl YTDugem {
    pub fn with_updated(mut self, updated: bool) -> Self {
        if updated {
            self.message = format!("YTDugem was updated to {}", update::CURRENT);
        }
        self
    }

    /// Once at start-up (silent on failure, like upstream) and from Settings.
    pub(super) fn check_updates(&mut self, manual: bool) {
        if self.updates.checking {
            return;
        }
        self.updates.checking = true;
        self.updates.status = "Checking…".into();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = update::check().map_err(err);
            let _ = tx.send(Event::Update(UpdateEvent::Checked(manual, result)));
        });
    }

    pub(super) fn update_event(&mut self, ctx: &egui::Context, event: UpdateEvent) {
        match event {
            UpdateEvent::Checked(manual, result) => {
                self.updates.checking = false;
                match result {
                    Ok(Some(release)) => {
                        self.updates.status = format!("Version {} is available", release.version);
                        // A screenshot run shows the app as it is, without the dialog.
                        self.updates.prompt = self.screenshot.is_none();
                        self.updates.available = Some(release);
                    }
                    Ok(None) => self.updates.status = "You have the latest version".into(),
                    Err(e) => {
                        self.updates.status = format!("Could not check for updates: {e}");
                        if manual {
                            self.message = self.updates.status.clone();
                        }
                    }
                }
            }
            UpdateEvent::Progress(f) => self.updates.downloading = Some(f),
            UpdateEvent::Staged(result) => {
                self.updates.downloading = None;
                match result {
                    Ok(path) => self.updates.staged = Some(path),
                    Err(e) => {
                        self.updates.status = format!("Update failed: {e}");
                        self.message = self.updates.status.clone();
                    }
                }
            }
            UpdateEvent::Applied(result) => {
                self.updates.applying = false;
                let restarted = result.and_then(|()| {
                    let dir = update::portable_dir().ok_or("The app folder changed")?;
                    update::restart(&dir).map_err(err)
                });
                match restarted {
                    Ok(()) => {
                        self.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                    Err(e) => {
                        self.updates.status = e.clone();
                        self.message = e;
                    }
                }
            }
        }
    }

    fn download_update(&mut self) {
        let (Some(dir), Some(package)) = (
            update::portable_dir(),
            self.updates
                .available
                .as_ref()
                .and_then(|r| r.package.clone()),
        ) else {
            return;
        };
        self.updates.downloading = Some(0.0);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let progress = |f| {
                let _ = tx.send(Event::Update(UpdateEvent::Progress(f)));
            };
            let result = update::download(&package, &dir, progress).map_err(err);
            let _ = tx.send(Event::Update(UpdateEvent::Staged(result)));
        });
    }

    /// Stops playback (mpv runs from `tools`, which is replaced), swaps the files, restarts.
    fn apply_update(&mut self) {
        let (Some(dir), Some(staged)) = (update::portable_dir(), self.updates.staged.clone())
        else {
            return;
        };
        if self
            .downloads
            .iter()
            .any(|d| !matches!(d.state, DownloadState::Failed(_)))
        {
            self.message = "Wait for downloads to finish, then restart to update".into();
            return;
        }
        self.stop();
        self.prepared = None;
        self.updates.applying = true;
        let version = self
            .updates
            .available
            .as_ref()
            .map(|r| r.version.clone())
            .unwrap_or_default();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            // A stream lookup may still be running a bundled tool; give it a few seconds.
            let mut result = update::apply(&dir, &staged);
            for _ in 0..5 {
                if result.is_ok() {
                    break;
                }
                std::thread::sleep(Duration::from_secs(1));
                result = update::apply(&dir, &staged);
            }
            if result.is_ok() {
                update::record_installed_version(&dir, &version);
            }
            let _ = tx.send(Event::Update(UpdateEvent::Applied(result.map_err(err))));
        });
    }

    fn open_release_page(&mut self) {
        if let Some(release) = &self.updates.available {
            let page = release.page.clone();
            self.report(update::open_page(&page));
        }
    }

    pub(super) fn update_dialog(&mut self, ctx: &egui::Context) {
        let Some(release) = self
            .updates
            .available
            .clone()
            .filter(|_| self.updates.prompt)
        else {
            return;
        };
        let installable = release.package.is_some() && update::portable_dir().is_some();
        let (mut later, mut install, mut restart, mut page) = (false, false, false, false);
        egui::Window::new("Update available")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(380.0);
                ui.label(format!(
                    "YTDugem {} is out — you have {}.",
                    release.version,
                    update::CURRENT
                ));
                let notes = release.notes.trim();
                if !notes.is_empty() {
                    ui.add_space(6.0);
                    egui::ScrollArea::vertical()
                        .max_height(160.0)
                        .show(ui, |ui| muted(ui, notes));
                }
                ui.add_space(8.0);
                if self.updates.applying {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label("Updating…");
                    });
                } else if self.updates.staged.is_some() {
                    muted(
                        ui,
                        "Ready. YTDugem will close and open again; playback stops.",
                    );
                    ui.horizontal(|ui| {
                        restart = ui.button("Restart now").clicked();
                        later = ui.button("Later").clicked();
                    });
                } else if let Some(f) = self.updates.downloading {
                    ui.add(
                        egui::ProgressBar::new(f)
                            .desired_width(360.0)
                            .text(format!("Downloading… {:.0}%", f * 100.0)),
                    );
                    later = ui.button("Hide").clicked();
                } else {
                    ui.horizontal(|ui| {
                        if installable {
                            install = ui.button("Update now").clicked();
                            page = ui.button("Release notes").clicked();
                        } else {
                            page = ui.button("Download").clicked();
                        }
                        later = ui.button("Later").clicked();
                    });
                }
            });
        if install {
            self.download_update();
        }
        if restart {
            self.apply_update();
        }
        if page {
            self.open_release_page();
        }
        if later {
            self.updates.prompt = false;
        }
    }

    /// Settings → About: the running version and a manual check.
    pub(super) fn about_settings(&mut self, ui: &mut egui::Ui, query: &str) -> bool {
        let rows = ["Version", "Check for updates"];
        let Some(q) = group_query(query, "About", &rows) else {
            return false;
        };
        settings_group(ui, "About", |ui| {
            if hit(&q, rows[0]) {
                setting_row(ui, rows[0], "", |ui| {
                    ui.label(RichText::new(update::CURRENT).color(MUTED));
                });
            }
            if hit(&q, rows[1]) {
                let status = self.updates.status.clone();
                setting_row(ui, rows[1], &status, |ui| {
                    if self.updates.available.is_some() {
                        if ui.button("Show update").clicked() {
                            self.updates.prompt = true;
                        }
                    } else if ui
                        .add_enabled(!self.updates.checking, egui::Button::new("Check now"))
                        .clicked()
                    {
                        self.check_updates(true);
                    }
                });
            }
        });
        true
    }
}
