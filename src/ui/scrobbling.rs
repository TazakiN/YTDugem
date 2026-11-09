//! Last.fm and ListenBrainz in the GUI (upstream `DesktopIntegrationsDialog`
//! `ScrobblingSection`): feeding playback to the scrobble tracker, the Settings rows, and the
//! sign-in work, which runs on background threads.
use super::*;
use crate::scrobble::{self, Action, Entry, Notice, ScrobbleSettings, Scrobbler, Service, Tracker};
use std::sync::atomic::AtomicBool;

pub(super) enum ScrobbleEvent {
    /// Whether (Last.fm, ListenBrainz) have a saved or environment credential.
    Configured(Result<(bool, bool), String>),
    LastFmSignedIn(u64, Result<String, String>),
    LastFmForgotten(Result<(), String>),
    /// A token was checked and saved (`Some(user)`) or removed (`None`).
    ListenBrainzSaved(u64, Result<Option<String>, String>),
    Notice(Notice),
}

pub(super) struct Scrobbling {
    tracker: Tracker,
    scrobbler: Scrobbler,
    /// Credentials found, once checked.
    lastfm_ready: bool,
    listenbrainz_ready: bool,
    has_lastfm_keys: bool,
    request: u64,
    /// The Last.fm sign-in waiting for approval in the browser; set to cancel it.
    signing_in: Option<Arc<AtomicBool>>,
    saving_token: bool,
    token_input: String,
}

impl Scrobbling {
    pub(super) fn start(library: &Path, tx: &Sender<Event>) -> Self {
        let notices = tx.clone();
        let scrobbler = Scrobbler::start(scrobble::queue_path(library), move |notice| {
            let _ = notices.send(Event::Scrobble(ScrobbleEvent::Notice(notice)));
        });
        let tx = tx.clone();
        std::thread::spawn(move || {
            let result = scrobble::configured().map_err(err);
            let _ = tx.send(Event::Scrobble(ScrobbleEvent::Configured(result)));
        });
        Self {
            tracker: Tracker::default(),
            scrobbler,
            lastfm_ready: false,
            listenbrainz_ready: false,
            has_lastfm_keys: crate::services::scrobble::lastfm_keys().is_some(),
            request: 0,
            signing_in: None,
            saving_token: false,
            token_input: String::new(),
        }
    }
}

/// The services a listen goes to now, and whether each wants "now playing".
fn targets(s: &ScrobbleSettings, state: &Scrobbling) -> Vec<(Service, bool, bool)> {
    let mut out = vec![];
    if s.lastfm_enabled && state.lastfm_ready && state.has_lastfm_keys {
        out.push((Service::LastFm, s.lastfm_scrobble, s.lastfm_now_playing));
    }
    if s.listenbrainz_enabled && state.listenbrainz_ready {
        out.push((Service::ListenBrainz, true, s.listenbrainz_now_playing));
    }
    out
}

impl YTDugem {
    /// Each poll, after `track_listen`: announce new plays and scrobble heard ones.
    pub(super) fn scrobble_tick(&mut self) {
        let settings = &self.library.scrobble;
        let targets = targets(settings, &self.scrobbling);
        self.scrobbling
            .scrobbler
            .set_active(targets.iter().map(|(s, _, _)| *s).collect());
        if targets.is_empty() {
            self.scrobbling.tracker = Tracker::default();
            return;
        }
        let playing = !self.loading && !self.status.paused && !self.status.ended;
        let actions = self.scrobbling.tracker.tick(
            self.current.as_ref().map(|t| (self.play_request, t)),
            self.status.position,
            self.status.duration,
            playing,
            settings,
            scrobble::now(),
        );
        for action in actions {
            match action {
                Action::NowPlaying { track, duration } => {
                    for &(service, _, now_playing) in &targets {
                        if let Some(entry) = Entry::new(service, settings, &track, duration, 0)
                            .filter(|_| now_playing)
                        {
                            self.scrobbling.scrobbler.now_playing(entry);
                        }
                    }
                }
                Action::Scrobble {
                    track,
                    duration,
                    started,
                } => {
                    let entries = targets
                        .iter()
                        .filter(|(_, scrobble, _)| *scrobble)
                        .filter_map(|&(service, _, _)| {
                            Entry::new(service, settings, &track, duration, started)
                        })
                        .collect();
                    self.scrobbling.scrobbler.submit(entries);
                }
            }
        }
    }

    pub(super) fn scrobble_event(&mut self, event: ScrobbleEvent) {
        match event {
            ScrobbleEvent::Configured(Ok((lastfm, listenbrainz))) => {
                self.scrobbling.lastfm_ready = lastfm;
                self.scrobbling.listenbrainz_ready = listenbrainz;
            }
            ScrobbleEvent::Configured(Err(e)) => {
                self.toast_error(format!("Scrobbling credentials could not be read: {e}"));
            }
            ScrobbleEvent::LastFmSignedIn(id, result) if id == self.scrobbling.request => {
                self.scrobbling.signing_in = None;
                match result {
                    Ok(name) => {
                        self.scrobbling.lastfm_ready = true;
                        let s = &mut self.library.scrobble;
                        s.lastfm_username = name.clone();
                        s.lastfm_enabled = true;
                        self.save();
                        self.toast_success(format!("Scrobbling to Last.fm as {name}"));
                    }
                    Err(e) => self.toast_error(e),
                }
            }
            ScrobbleEvent::LastFmForgotten(result) => {
                if let Err(e) = result {
                    self.toast_error(format!("Could not remove the Last.fm session: {e}"));
                }
            }
            ScrobbleEvent::ListenBrainzSaved(id, result) if id == self.scrobbling.request => {
                self.scrobbling.saving_token = false;
                match result {
                    Ok(Some(user)) => {
                        self.scrobbling.token_input.clear();
                        self.scrobbling.listenbrainz_ready = true;
                        let s = &mut self.library.scrobble;
                        s.listenbrainz_username = user.clone();
                        s.listenbrainz_enabled = true;
                        self.save();
                        self.toast_success(if user.is_empty() {
                            "Scrobbling to ListenBrainz".to_owned()
                        } else {
                            format!("Scrobbling to ListenBrainz as {user}")
                        });
                    }
                    Ok(None) => {
                        self.scrobbling.scrobbler.forget(Service::ListenBrainz);
                        let s = &mut self.library.scrobble;
                        s.listenbrainz_username.clear();
                        s.listenbrainz_enabled = false;
                        self.save();
                        // An environment token still counts after the saved one is removed.
                        let tx = self.tx.clone();
                        std::thread::spawn(move || {
                            let result = scrobble::configured().map_err(err);
                            let _ = tx.send(Event::Scrobble(ScrobbleEvent::Configured(result)));
                        });
                    }
                    Err(e) => self.toast_error(e),
                }
            }
            ScrobbleEvent::LastFmSignedIn(..) | ScrobbleEvent::ListenBrainzSaved(..) => {}
            ScrobbleEvent::Notice(Notice::AuthFailed(service, message)) => {
                let s = &mut self.library.scrobble;
                match service {
                    Service::LastFm => s.lastfm_enabled = false,
                    Service::ListenBrainz => s.listenbrainz_enabled = false,
                }
                self.save();
                self.toast_error(format!(
                    "{message}. {} scrobbling is off until you reconnect in Settings.",
                    service.label()
                ));
            }
            ScrobbleEvent::Notice(Notice::QueueError(message)) => {
                self.toast_error(format!("Scrobble queue: {message}"));
            }
        }
    }

    fn lastfm_connect(&mut self) {
        if self.scrobbling.signing_in.is_some() {
            return;
        }
        self.scrobbling.request += 1;
        let (id, tx) = (self.scrobbling.request, self.tx.clone());
        let cancel = Arc::new(AtomicBool::new(false));
        self.scrobbling.signing_in = Some(cancel.clone());
        std::thread::spawn(move || {
            let result = scrobble::lastfm_sign_in(&cancel).map_err(err);
            let _ = tx.send(Event::Scrobble(ScrobbleEvent::LastFmSignedIn(id, result)));
        });
    }

    fn lastfm_disconnect(&mut self) {
        self.scrobbling.lastfm_ready = false;
        self.scrobbling.scrobbler.forget(Service::LastFm);
        let s = &mut self.library.scrobble;
        s.lastfm_enabled = false;
        s.lastfm_username.clear();
        self.save();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = scrobble::forget_lastfm().map_err(err);
            let _ = tx.send(Event::Scrobble(ScrobbleEvent::LastFmForgotten(result)));
            let result = scrobble::configured().map_err(err);
            let _ = tx.send(Event::Scrobble(ScrobbleEvent::Configured(result)));
        });
    }

    fn save_listenbrainz_token(&mut self) {
        if self.scrobbling.saving_token {
            return;
        }
        self.scrobbling.saving_token = true;
        self.scrobbling.request += 1;
        let (id, tx) = (self.scrobbling.request, self.tx.clone());
        let token = std::mem::take(&mut self.scrobbling.token_input);
        std::thread::spawn(move || {
            let result = scrobble::save_listenbrainz_token(&token).map_err(err);
            let _ = tx.send(Event::Scrobble(ScrobbleEvent::ListenBrainzSaved(
                id, result,
            )));
        });
    }

    pub(super) fn scrobble_settings(&mut self, ui: &mut egui::Ui, query: &str) -> bool {
        let rows = [
            "Last.fm",
            "Last.fm: scrobble tracks",
            "Last.fm: now playing",
            "Last.fm: only scrobble primary artist",
            "ListenBrainz",
            "ListenBrainz: now playing",
            "ListenBrainz: only scrobble primary artist",
            "Minimum song duration",
            "Scrobble delay",
            "Maximum delay",
        ];
        let Some(q) = group_query(query, "Scrobbling", &rows) else {
            return false;
        };
        settings_group(ui, "Scrobbling", |ui| {
            let mut changed = false;
            if rows[..4].iter().any(|r| hit(&q, r)) {
                self.lastfm_rows(ui, &q, &rows, &mut changed);
            }
            if rows[4..7].iter().any(|r| hit(&q, r)) {
                self.listenbrainz_rows(ui, &q, &rows, &mut changed);
            }
            let s = &mut self.library.scrobble;
            if hit(&q, rows[7]) {
                setting_row(ui, rows[7], "Shorter songs will not scrobble", |ui| {
                    ui.label(RichText::new(format!("{} s", s.min_duration)).color(MUTED));
                    changed |= ui
                        .add(egui::Slider::new(&mut s.min_duration, 15..=120).show_value(false))
                        .changed();
                });
            }
            if hit(&q, rows[8]) {
                setting_row(ui, rows[8], "How far into a song to scrobble it", |ui| {
                    let mut percent = (s.delay_percent * 100.0).round() as u32;
                    ui.label(RichText::new(format!("{percent} %")).color(MUTED));
                    if ui
                        .add(egui::Slider::new(&mut percent, 10..=100).show_value(false))
                        .changed()
                    {
                        s.delay_percent = percent as f32 / 100.0;
                        changed = true;
                    }
                });
            }
            if hit(&q, rows[9]) {
                setting_row(
                    ui,
                    rows[9],
                    "Scrobble after this long even if the delay share is not reached",
                    |ui| {
                        ui.label(RichText::new(format!("{} s", s.delay_seconds)).color(MUTED));
                        changed |= ui
                            .add(
                                egui::Slider::new(&mut s.delay_seconds, 30..=300).show_value(false),
                            )
                            .changed();
                    },
                );
            }
            let pending = self.scrobbling.scrobbler.pending();
            if pending > 0 && q.is_empty() {
                setting_row(
                    ui,
                    "Waiting to be sent",
                    &format!(
                        "{pending} listens could not be sent yet and will be retried with their original times"
                    ),
                    |_| {},
                );
            }
            if changed {
                self.library.scrobble.sanitize();
                self.save();
            }
        });
        true
    }

    fn lastfm_rows(&mut self, ui: &mut egui::Ui, q: &str, rows: &[&str], changed: &mut bool) {
        let state = &self.scrobbling;
        let signed_in = state.lastfm_ready;
        let waiting = state.signing_in.is_some();
        let s = &self.library.scrobble;
        let (mut connect, mut disconnect, mut cancel) = (false, false, false);
        if !state.has_lastfm_keys {
            setting_row(
                ui,
                rows[0],
                "This build has no Last.fm API key, so Last.fm is unavailable. ListenBrainz still works.",
                |ui| {
                    ui.label(RichText::new("Unavailable").color(MUTED));
                },
            );
            return;
        }
        let subtitle = if waiting {
            "Approve YTDugem in the Last.fm page that opened in your browser…".to_owned()
        } else if signed_in && s.lastfm_username.is_empty() {
            "Signed in".to_owned()
        } else if signed_in {
            format!("Signed in as {}", s.lastfm_username)
        } else {
            "Sign in through Last.fm in your browser; YTDugem never sees your password".to_owned()
        };
        let mut enabled = s.lastfm_enabled;
        setting_row(ui, rows[0], &subtitle, |ui| {
            if waiting {
                cancel = ui.button("Cancel").clicked();
            } else if signed_in {
                *changed |= toggle_switch(ui, &mut enabled).clicked();
                disconnect = ui.button("Disconnect").clicked();
            } else {
                connect = ui.button("Connect").clicked();
            }
        });
        if signed_in && enabled != self.library.scrobble.lastfm_enabled {
            self.library.scrobble.lastfm_enabled = enabled;
        }
        if signed_in && self.library.scrobble.lastfm_enabled {
            let s = &mut self.library.scrobble;
            let options = [
                (
                    1,
                    "Log plays to your Last.fm timeline",
                    &mut s.lastfm_scrobble,
                ),
                (
                    2,
                    "Update Last.fm with what you are listening to",
                    &mut s.lastfm_now_playing,
                ),
                (
                    3,
                    "Use the first artist when a track credits multiple artists",
                    &mut s.lastfm_primary_artist,
                ),
            ];
            for (i, subtitle, on) in options {
                if hit(q, rows[i]) {
                    let title = rows[i].trim_start_matches("Last.fm: ");
                    *changed |= toggle_row(ui, &capitalize(title), subtitle, on);
                }
            }
        }
        if connect {
            self.lastfm_connect();
        }
        if disconnect {
            self.lastfm_disconnect();
        }
        if cancel {
            if let Some(flag) = self.scrobbling.signing_in.take() {
                flag.store(true, std::sync::atomic::Ordering::Relaxed);
            }
            // The cancelled sign-in's answer is now stale.
            self.scrobbling.request += 1;
        }
    }

    fn listenbrainz_rows(&mut self, ui: &mut egui::Ui, q: &str, rows: &[&str], changed: &mut bool) {
        let ready = self.scrobbling.listenbrainz_ready;
        let busy = self.scrobbling.saving_token;
        let s = &self.library.scrobble;
        let subtitle = if busy {
            "Checking the token with ListenBrainz…".to_owned()
        } else if ready && !s.listenbrainz_username.is_empty() {
            format!("Connected as {}", s.listenbrainz_username)
        } else if ready {
            "Connected".to_owned()
        } else {
            "Paste your user token from listenbrainz.org/settings".to_owned()
        };
        let mut enabled = s.listenbrainz_enabled;
        let mut save = false;
        let input = &mut self.scrobbling.token_input;
        setting_row(ui, rows[4], &subtitle, |ui| {
            if ready {
                *changed |= toggle_switch(ui, &mut enabled).clicked();
            }
            let label = if input.trim().is_empty() && ready {
                "Remove"
            } else {
                "Save"
            };
            let can = !busy && (ready || !input.trim().is_empty());
            save = ui.add_enabled(can, egui::Button::new(label)).clicked();
            let response = ui.add(
                egui::TextEdit::singleline(input)
                    .margin(TEXT_MARGIN)
                    .password(true)
                    .hint_text(if ready {
                        "Paste to replace"
                    } else {
                        "Paste token"
                    })
                    .desired_width(200.0),
            );
            save |= can
                && !input.trim().is_empty()
                && response.lost_focus()
                && ui.input(|i| i.key_pressed(egui::Key::Enter));
        });
        if ready {
            self.library.scrobble.listenbrainz_enabled = enabled;
        }
        if ready && self.library.scrobble.listenbrainz_enabled {
            let s = &mut self.library.scrobble;
            let options = [
                (
                    5,
                    "Tell ListenBrainz what you are listening to",
                    &mut s.listenbrainz_now_playing,
                ),
                (
                    6,
                    "Use the first artist when a track credits multiple artists",
                    &mut s.listenbrainz_primary_artist,
                ),
            ];
            for (i, subtitle, on) in options {
                if hit(q, rows[i]) {
                    let title = rows[i].trim_start_matches("ListenBrainz: ");
                    *changed |= toggle_row(ui, &capitalize(title), subtitle, on);
                }
            }
        }
        if save {
            self.save_listenbrainz_token();
        }
    }
}

fn capitalize(text: &str) -> String {
    let mut chars = text.chars();
    chars
        .next()
        .map(|c| c.to_uppercase().chain(chars).collect())
        .unwrap_or_default()
}
