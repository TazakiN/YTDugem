//! Listening stats (a YTDugem addition): recording plays from playback progress, saving the log
//! in batches off the GUI thread, and the Stats page with its yearly "Wrapped" card. Everything
//! stays on this computer.
use super::*;
use crate::stats::{self, Listen, Period, PlayEvent, Ranked, StatsLog, Summary};

/// Unsaved plays are written after this long, and on exit.
const SAVE_AFTER: Duration = Duration::from_secs(30);
const TOP: usize = 10;

/// The aggregated numbers for one period, rebuilt only when the log or the period changes.
struct Report {
    key: (Period, u64, i64),
    summary: Summary,
    tracks: Vec<Ranked>,
    artists: Vec<Ranked>,
    albums: Vec<Ranked>,
    hours: [f64; 24],
    weekdays: [f64; 7],
    year: i64,
    wrapped: Summary,
    wrapped_track: Option<Ranked>,
    wrapped_artist: Option<Ranked>,
    wrapped_album: Option<Ranked>,
    wrapped_hour: Option<usize>,
}

pub(super) struct Stats {
    log: StatsLog,
    path: PathBuf,
    /// The file could not be read: nothing is recorded or saved, so it is never overwritten.
    error: Option<String>,
    listen: Option<(u64, Listen)>,
    /// Bumped on every change, for saving order and the page's cache.
    revision: u64,
    unsaved_since: Option<Instant>,
    /// The revision last written; a slower, older save never replaces a newer file.
    written: Arc<Mutex<u64>>,
    period: Period,
    confirm_clear: bool,
    report: Option<Report>,
}

impl Stats {
    pub(super) fn load(library: &Path) -> Self {
        let path = stats::stats_path(library);
        let (log, error) = match StatsLog::load(&path) {
            Ok(log) => (log, None),
            Err(e) => (
                StatsLog::default(),
                Some(format!(
                    "{e:#}. Listening stats are paused to preserve the file."
                )),
            ),
        };
        Self {
            log,
            path,
            error,
            listen: None,
            revision: 0,
            unsaved_since: None,
            written: Arc::default(),
            period: Period::default(),
            confirm_clear: false,
            report: None,
        }
    }
    pub(super) fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }
    fn changed(&mut self) {
        self.revision += 1;
        self.unsaved_since.get_or_insert_with(Instant::now);
    }
    /// Write the log: in the background, or now (on exit). Errors are returned, not shown.
    fn save(&mut self, wait: bool, tx: &Sender<Event>) {
        if self.error.is_some() || self.unsaved_since.take().is_none() {
            return;
        }
        let log = StatsLog {
            events: self.log.events.clone(),
        };
        let (path, written, revision) = (self.path.clone(), self.written.clone(), self.revision);
        let job = move || -> anyhow::Result<()> {
            let mut last = written
                .lock()
                .map_err(|_| anyhow::anyhow!("Stats saver failed"))?;
            if *last >= revision {
                return Ok(());
            }
            log.save(&path)?;
            *last = revision;
            Ok(())
        };
        if wait {
            if let Err(e) = job() {
                eprintln!("YTDugem: could not save listening stats: {e:#}");
            }
        } else {
            let tx = tx.clone();
            std::thread::spawn(move || {
                if let Err(e) = job() {
                    let _ = tx.send(Event::StatsSaveFailed(err(e)));
                }
            });
        }
    }
}

fn listening_time(seconds: f64) -> String {
    let minutes = (seconds / 60.0).round() as u64;
    match minutes {
        0 if seconds > 0.0 => "< 1 min".into(),
        0..60 => format!("{minutes} min"),
        _ => format!("{} h {} min", minutes / 60, minutes % 60),
    }
}

fn plays(n: usize) -> String {
    if n == 1 {
        "1 play".into()
    } else {
        format!("{n} plays")
    }
}

fn hour_label(hour: usize) -> String {
    format!("{hour:02}:00")
}

impl YTDugem {
    /// Each poll: count playback progress toward the current play, and record the previous
    /// play once the track changes.
    pub(super) fn track_listen(&mut self) {
        if !self.library.record_stats || self.stats.error.is_some() {
            self.stats.listen = None;
            return;
        }
        let now_playing = self
            .current
            .as_ref()
            .map(|t| (self.play_request, t.key().to_owned()));
        let tracked = self
            .stats
            .listen
            .as_ref()
            .map(|(request, l)| (*request, l.key().to_owned()));
        if tracked != now_playing {
            self.finish_listen();
            if let Some(track) = self.current.clone() {
                self.stats.listen = Some((self.play_request, Listen::new(track, stats::now())));
            }
        }
        let playing = !self.loading && !self.status.paused && !self.status.ended;
        if let Some((_, listen)) = &mut self.stats.listen {
            if self.status.duration > 0.0 {
                listen.track.duration = self.status.duration;
            }
            listen.advance(self.status.position, playing);
        }
    }

    fn finish_listen(&mut self) {
        if let Some(event) = self
            .stats
            .listen
            .take()
            .and_then(|(_, listen)| listen.finish(0.0))
        {
            self.stats.log.push(event);
            self.stats.changed();
        }
    }

    /// Saves recorded plays once they have waited `SAVE_AFTER`.
    pub(super) fn stats_save_step(&mut self) {
        if self
            .stats
            .unsaved_since
            .is_some_and(|t| t.elapsed() >= SAVE_AFTER)
        {
            self.stats.save(false, &self.tx);
        }
    }

    /// On exit: count the play in progress and write the log before the window closes.
    pub(super) fn stats_on_exit(&mut self) {
        self.finish_listen();
        self.stats.save(true, &self.tx);
    }

    fn clear_stats(&mut self) {
        self.stats.listen = None;
        self.stats.log.events.clear();
        self.stats.changed();
        self.stats.save(false, &self.tx);
    }

    fn stats_report(&mut self) -> &Report {
        let now = stats::now();
        let offset = stats::local_offset();
        // The day is part of the key so "Last 7 days" moves on while the page stays open.
        let key = (
            self.stats.period,
            self.stats.revision,
            (now + offset).div_euclid(86_400),
        );
        if self.stats.report.as_ref().is_none_or(|r| r.key != key) {
            let events = &self.stats.log.events;
            let shown = stats::in_period(events, self.stats.period, now, offset);
            let year: Vec<&PlayEvent> = stats::in_period(events, Period::Year, now, offset);
            let hours = stats::by_hour(&year, offset);
            let busiest = (0..24)
                .filter(|h| hours[*h] > 0.0)
                .max_by(|a, b| hours[*a].total_cmp(&hours[*b]));
            self.stats.report = Some(Report {
                key,
                summary: stats::summary(&shown),
                tracks: stats::top_tracks(&shown, TOP),
                artists: stats::top_artists(&shown, TOP),
                albums: stats::top_albums(&shown, TOP),
                hours: stats::by_hour(&shown, offset),
                weekdays: stats::by_weekday(&shown, offset),
                year: stats::local_year(now, offset),
                wrapped: stats::summary(&year),
                wrapped_track: stats::top_tracks(&year, 1).pop(),
                wrapped_artist: stats::top_artists(&year, 1).pop(),
                wrapped_album: stats::top_albums(&year, 1).pop(),
                wrapped_hour: busiest,
            });
        }
        self.stats.report.as_ref().expect("report built above")
    }

    pub(super) fn stats_view(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(heading("Listening stats", 26.0));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let enabled = !self.stats.log.events.is_empty() && self.stats.error.is_none();
                if ui
                    .add_enabled(enabled, egui::Button::new("Clear listening stats"))
                    .clicked()
                {
                    self.stats.confirm_clear = true;
                }
            });
        });
        ui.label(
            RichText::new(
                "Kept only on this computer. A song counts after 30 seconds of listening, or half of a shorter song.",
            )
            .color(MUTED)
            .size(12.5),
        );
        if let Some(e) = self.stats.error.clone() {
            ui.add_space(8.0);
            ui.label(RichText::new(e).color(ERROR));
        } else if !self.library.record_stats {
            ui.add_space(8.0);
            ui.label(
                RichText::new("Recording is paused. Turn it on in Settings › Privacy.")
                    .color(MUTED),
            );
        }
        ui.add_space(14.0);
        ui.horizontal(|ui| {
            for period in Period::ALL {
                if ui
                    .selectable_label(self.stats.period == period, period.label())
                    .clicked()
                {
                    self.stats.period = period;
                }
            }
        });
        ui.add_space(14.0);
        // Rows are drawn from a snapshot so clicks can call back into the app.
        let report = self.stats_report();
        let summary = report.summary.clone();
        let (tracks, artists, albums) = (
            report.tracks.clone(),
            report.artists.clone(),
            report.albums.clone(),
        );
        let (hours, weekdays) = (report.hours, report.weekdays);
        let wrapped = (
            report.year,
            report.wrapped.clone(),
            report.wrapped_track.clone(),
            report.wrapped_artist.clone(),
            report.wrapped_album.clone(),
            report.wrapped_hour,
        );

        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::splat(12.0);
            tile(ui, "Listening time", &listening_time(summary.seconds));
            tile(ui, "Plays", &summary.plays.to_string());
            tile(ui, "Songs", &summary.tracks.to_string());
            tile(ui, "Artists", &summary.artists.to_string());
        });
        ui.add_space(18.0);
        if wrapped.1.plays > 0 {
            self.wrapped_card(ui, wrapped);
            ui.add_space(18.0);
        }
        if summary.plays == 0 {
            ui.label(RichText::new("No plays in this period yet.").color(MUTED));
            return;
        }
        let wide = ui.available_width() >= 760.0;
        ui.horizontal_top(|ui| {
            let width = if wide {
                (ui.available_width() - 24.0) / 2.0
            } else {
                ui.available_width()
            };
            ui.vertical(|ui| {
                ui.set_width(width);
                section_heading(ui, "By hour of day");
                bar_chart(ui, &hours, width, |i| {
                    if i % 6 == 0 {
                        hour_label(i)
                    } else {
                        String::new()
                    }
                });
            });
            if wide {
                ui.add_space(24.0);
                ui.vertical(|ui| {
                    ui.set_width(width);
                    section_heading(ui, "By day of week");
                    bar_chart(ui, &weekdays, width, |i| WEEKDAYS[i].to_owned());
                });
            }
        });
        if !wide {
            section_heading(ui, "By day of week");
            bar_chart(ui, &weekdays, ui.available_width(), |i| {
                WEEKDAYS[i].to_owned()
            });
        }
        ui.add_space(18.0);
        let columns = if ui.available_width() >= 1000.0 { 3 } else { 1 };
        let mut play = None;
        let mut artist = None;
        ui.columns(columns, |cols| {
            // Side by side when wide, else stacked in the one column.
            let ui = &mut cols[0];
            section_heading(ui, "Top songs");
            for (i, row) in tracks.iter().enumerate() {
                if ranked_row(ui, i, row, false).clicked() {
                    play = Some(row.track.clone());
                }
            }
            let ui = if columns == 3 {
                &mut cols[1]
            } else {
                &mut cols[0]
            };
            ui.add_space(if columns == 3 { 0.0 } else { 16.0 });
            section_heading(ui, "Top artists");
            for (i, row) in artists.iter().enumerate() {
                if ranked_row(ui, i, row, true).clicked() {
                    artist = Some(artist::ArtistLink {
                        name: row.title.clone(),
                        browse_id: row.browse_id.clone(),
                    });
                }
            }
            let ui = if columns == 3 {
                &mut cols[2]
            } else {
                &mut cols[0]
            };
            ui.add_space(if columns == 3 { 0.0 } else { 16.0 });
            section_heading(ui, "Top albums");
            for (i, row) in albums.iter().enumerate() {
                ranked_row(ui, i, row, false);
            }
        });
        if let Some(track) = play {
            self.start_tracks(vec![track], 0, "Stats");
        }
        if let Some(link) = artist {
            self.open_artist_link(link);
        }
    }

    fn wrapped_card(
        &mut self,
        ui: &mut egui::Ui,
        (year, summary, track, artist, album, hour): (
            i64,
            Summary,
            Option<Ranked>,
            Option<Ranked>,
            Option<Ranked>,
            Option<usize>,
        ),
    ) {
        egui::Frame::new()
            .fill(Color32::from_rgb(52, 66, 44))
            .stroke(Stroke::new(1.0_f32, Color32::from_rgb(92, 112, 78)))
            .corner_radius(14.0)
            .inner_margin(egui::Margin::same(20))
            .show(ui, |ui| {
                ui.set_width(ui.available_width());
                ui.label(heading(format!("Your {year} Wrapped"), 22.0));
                ui.add_space(4.0);
                ui.label(
                    RichText::new(format!(
                        "{} across {} · {} songs · {} artists",
                        listening_time(summary.seconds),
                        plays(summary.plays),
                        summary.tracks,
                        summary.artists
                    ))
                    .color(WHITE),
                );
                ui.add_space(12.0);
                ui.horizontal_wrapped(|ui| {
                    ui.spacing_mut().item_spacing = Vec2::new(28.0, 12.0);
                    for (label, row) in [
                        ("Top song", &track),
                        ("Top artist", &artist),
                        ("Top album", &album),
                    ] {
                        if let Some(row) = row {
                            ui.horizontal(|ui| {
                                let (r, _) =
                                    ui.allocate_exact_size(Vec2::splat(56.0), egui::Sense::hover());
                                draw_image(ui, &row.thumbnail, r, None);
                                ui.vertical(|ui| {
                                    ui.add_space(4.0);
                                    ui.label(RichText::new(label).size(11.5).color(ACCENT));
                                    ui.label(
                                        RichText::new(truncate(&row.title, 28))
                                            .strong()
                                            .color(WHITE),
                                    );
                                    ui.label(
                                        RichText::new(plays(row.plays)).size(12.0).color(MUTED),
                                    );
                                });
                            });
                        }
                    }
                    if let Some(h) = hour {
                        ui.vertical(|ui| {
                            ui.add_space(4.0);
                            ui.label(RichText::new("Favourite hour").size(11.5).color(ACCENT));
                            ui.label(RichText::new(hour_label(h)).strong().color(WHITE));
                        });
                    }
                });
            });
    }

    pub(super) fn stats_clear_dialog(&mut self, ctx: &egui::Context) {
        if !self.stats.confirm_clear {
            return;
        }
        let (mut close, mut confirmed) = (false, false);
        egui::Window::new("Clear listening stats")
            .collapsible(false)
            .resizable(false)
            .frame(widgets::dialog_card())
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label("Every recorded play will be deleted from this computer. This cannot be undone.");
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    confirmed = widgets::action(ui, widgets::Action::Danger, "Clear").clicked();
                    close = widgets::action(ui, widgets::Action::Ghost, "Cancel").clicked();
                });
            });
        if confirmed {
            self.clear_stats();
            self.toast_success("Listening stats cleared");
        }
        if confirmed || close {
            self.stats.confirm_clear = false;
        }
    }

    /// Settings › Privacy: recording stats and the recent-search list.
    pub(super) fn privacy_settings(&mut self, ui: &mut egui::Ui, query: &str) -> bool {
        let rows = [
            "Record listening stats",
            "Listening stats",
            "Recent searches",
        ];
        let Some(q) = group_query(query, "Privacy", &rows) else {
            return false;
        };
        settings_group(ui, "Privacy", |ui| {
            if hit(&q, rows[0]) {
                let mut on = self.library.record_stats;
                if toggle_row(
                    ui,
                    rows[0],
                    "Count plays for the Stats page. Kept only on this computer",
                    &mut on,
                ) {
                    self.library.record_stats = on;
                    if !on {
                        self.stats.listen = None;
                    }
                    self.save();
                }
            }
            if hit(&q, rows[1]) {
                let count = self.stats.log.events.len();
                setting_row(ui, rows[1], &format!("{count} plays recorded"), |ui| {
                    if ui
                        .add_enabled(count > 0, egui::Button::new("Clear…"))
                        .clicked()
                    {
                        self.stats.confirm_clear = true;
                    }
                    if ui.button("Open").clicked() {
                        self.navigate(Page::Stats);
                    }
                });
            }
            if hit(&q, rows[2]) {
                let count = self.library.recent_searches.len();
                setting_row(
                    ui,
                    rows[2],
                    "Shown under the search box; remove one with its ×",
                    |ui| {
                        if ui
                            .add_enabled(count > 0, egui::Button::new("Clear"))
                            .clicked()
                        {
                            self.clear_recent_searches();
                        }
                    },
                );
            }
        });
        true
    }
}

const WEEKDAYS: [&str; 7] = ["Mon", "Tue", "Wed", "Thu", "Fri", "Sat", "Sun"];

fn tile(ui: &mut egui::Ui, label: &str, value: &str) {
    egui::Frame::new()
        .fill(Color32::from_rgb(36, 38, 34))
        .stroke(Stroke::new(1.0_f32, Color32::from_gray(54)))
        .corner_radius(10.0)
        .inner_margin(egui::Margin::symmetric(16, 12))
        .show(ui, |ui| {
            ui.set_min_width(150.0);
            ui.vertical(|ui| {
                ui.label(RichText::new(label).size(12.0).color(MUTED));
                ui.label(heading(value, 22.0));
            });
        });
}

/// Bars of `values` (seconds), labelled under some of them.
fn bar_chart(ui: &mut egui::Ui, values: &[f64], width: f32, label: impl Fn(usize) -> String) {
    let height = 120.0;
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(width, height + 18.0), egui::Sense::hover());
    let max = values.iter().copied().fold(0.0, f64::max).max(1.0);
    let slot = rect.width() / values.len() as f32;
    let painter = ui.painter();
    painter.line_segment(
        [
            Pos2::new(rect.left(), rect.top() + height),
            Pos2::new(rect.right(), rect.top() + height),
        ],
        Stroke::new(1.0_f32, Color32::from_gray(54)),
    );
    let hovered = response
        .hover_pos()
        .map(|p| (((p.x - rect.left()) / slot) as usize).min(values.len() - 1));
    for (i, value) in values.iter().enumerate() {
        let h = (value / max) as f32 * (height - 6.0);
        let x = rect.left() + slot * i as f32;
        let bar = Rect::from_min_max(
            Pos2::new(x + slot * 0.18, rect.top() + height - h),
            Pos2::new(x + slot * 0.82, rect.top() + height),
        );
        let color = if hovered == Some(i) { WHITE } else { ACCENT };
        if h > 0.0 {
            painter.rect_filled(bar, 3.0, color);
        }
        let text = label(i);
        if !text.is_empty() {
            painter.text(
                Pos2::new(x + slot / 2.0, rect.top() + height + 4.0),
                egui::Align2::CENTER_TOP,
                text,
                FontId::proportional(11.0),
                MUTED,
            );
        }
    }
    if let Some(i) = hovered {
        response.on_hover_text(listening_time(values[i]));
    }
}

/// A numbered row with artwork (round for artists), title, subtitle and play count.
fn ranked_row(ui: &mut egui::Ui, index: usize, row: &Ranked, round: bool) -> egui::Response {
    let width = ui.available_width();
    let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 52.0), egui::Sense::click());
    if response.hovered() {
        ui.painter().rect_filled(rect, 5.0, widgets::white_alpha(8));
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    ui.painter().text(
        Pos2::new(rect.left() + 12.0, rect.center().y),
        egui::Align2::CENTER_CENTER,
        format!("{}", index + 1),
        FontId::proportional(12.0),
        MUTED,
    );
    let art = Rect::from_min_size(rect.min + Vec2::new(28.0, 6.0), Vec2::splat(40.0));
    if round {
        avatar(ui, &row.thumbnail, &row.title, art);
    } else {
        draw_image(ui, &row.thumbnail, art, None);
    }
    let x = art.right() + 10.0;
    let max = rect.right() - x - 8.0;
    fit_text(
        ui.painter(),
        Pos2::new(x, rect.top() + 17.0),
        egui::Align2::LEFT_CENTER,
        &row.title,
        FontId::new(14.0, egui::FontFamily::Name("bold".into())),
        WHITE,
        max,
    );
    let detail = if row.subtitle.is_empty() {
        format!("{} · {}", plays(row.plays), listening_time(row.seconds))
    } else {
        format!(
            "{} · {} · {}",
            row.subtitle,
            plays(row.plays),
            listening_time(row.seconds)
        )
    };
    fit_text(
        ui.painter(),
        Pos2::new(x, rect.top() + 36.0),
        egui::Align2::LEFT_CENTER,
        &detail,
        FontId::proportional(12.0),
        MUTED,
        max,
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listening_time_is_readable() {
        assert_eq!(listening_time(0.0), "0 min");
        assert_eq!(listening_time(10.0), "< 1 min");
        assert_eq!(listening_time(59.0 * 60.0), "59 min");
        assert_eq!(listening_time(3.0 * 3600.0 + 5.0 * 60.0), "3 h 5 min");
        assert_eq!(plays(1), "1 play");
        assert_eq!(plays(3), "3 plays");
    }
}
