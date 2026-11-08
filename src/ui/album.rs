//! The album page, laid out after Apple Music's: a large cover beside the title, artist credit,
//! type and year, Play and Shuffle; numbered rows without per-row artwork; a songs and minutes
//! footer.
use super::*;

/// Rows are this tall; the artist shows under a title only when it differs from the album's.
const ROW: f32 = 54.0;

/// "Album • 2022" as "Album · 2022".
fn album_meta(subtitle: &str) -> String {
    subtitle
        .split(['•', '·'])
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join(" · ")
}

/// "12 songs, 48 minutes".
fn album_footer(tracks: &[Track]) -> String {
    let songs = format!(
        "{} {}",
        tracks.len(),
        if tracks.len() == 1 { "song" } else { "songs" }
    );
    let seconds: f64 = tracks.iter().map(|t| t.duration).sum();
    if seconds < 1.0 {
        return songs;
    }
    let minutes = (seconds / 60.0).round().max(1.0) as u64;
    format!(
        "{songs}, {minutes} {}",
        if minutes == 1 { "minute" } else { "minutes" }
    )
}

/// A pill button; `filled` is the accent call to action.
fn pill_button(ui: &mut egui::Ui, icon: Icon, label: &str, filled: bool) -> egui::Response {
    let font = FontId::new(15.0, egui::FontFamily::Name("bold".into()));
    let galley = ui
        .painter()
        .layout_no_wrap(label.to_owned(), font, Color32::WHITE);
    let size = Vec2::new(galley.size().x + 58.0, 38.0);
    let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let (fill, color) = if filled {
        (
            if response.hovered() {
                ACCENT.gamma_multiply(0.88)
            } else {
                ACCENT
            },
            Color32::from_rgb(20, 24, 18),
        )
    } else {
        (
            widgets::white_alpha(if response.hovered() { 30 } else { 18 }),
            WHITE,
        )
    };
    let p = ui.painter();
    p.rect_filled(rect, 19.0, fill);
    widgets::paint(
        p,
        icon,
        Rect::from_center_size(
            Pos2::new(rect.left() + 26.0, rect.center().y),
            Vec2::splat(18.0),
        ),
        color,
    );
    p.galley(
        Pos2::new(rect.left() + 42.0, rect.center().y - galley.size().y / 2.0),
        galley,
        color,
    );
    response.on_hover_cursor(egui::CursorIcon::PointingHand)
}

impl YTDugem {
    pub(super) fn album_view(&mut self, ui: &mut egui::Ui, c: Collection) {
        let cover = (ui.available_width() * 0.32).clamp(140.0, 260.0);
        ui.horizontal_top(|ui| {
            let (r, _) = ui.allocate_exact_size(Vec2::splat(cover), egui::Sense::hover());
            draw_image(ui, &c.thumbnail, r, None);
            ui.add_space(8.0);
            ui.vertical(|ui| {
                ui.add_space((cover - 150.0).max(0.0) * 0.5);
                ui.add(egui::Label::new(heading(&c.title, 34.0)).wrap());
                if let Some(credit) = &c.credit {
                    let (line, _) = ui.allocate_exact_size(
                        Vec2::new(ui.available_width(), 26.0),
                        egui::Sense::hover(),
                    );
                    if let Some(link) = artist::credit_text(
                        ui,
                        ui.make_persistent_id("collection-credit"),
                        line.left_center(),
                        &artist::credit_segments(credit, &c.artists, false),
                        FontId::new(20.0, egui::FontFamily::Name("bold".into())),
                        ACCENT,
                        line.width(),
                    ) {
                        self.open_artist_link(link);
                    }
                }
                let meta = album_meta(&c.subtitle);
                if !meta.is_empty() {
                    ui.label(RichText::new(meta).size(13.0).color(MUTED));
                }
                ui.add_space(14.0);
                ui.horizontal_wrapped(|ui| {
                    let enabled = !c.tracks.is_empty() && !self.bulk_loading;
                    ui.add_enabled_ui(enabled, |ui| {
                        if pill_button(ui, Icon::Play, "Play", true).clicked() {
                            self.bulk(PlaylistBulk::Play, &c.title, c.tracks.clone(), None);
                        }
                        if pill_button(ui, Icon::Shuffle, "Shuffle", false).clicked() {
                            self.bulk(PlaylistBulk::Shuffle, &c.title, c.tracks.clone(), None);
                        }
                    });
                    for (icon, label, action) in [
                        (Icon::AddToQueue, "Add to queue", PlaylistBulk::Queue),
                        (Icon::Download, "Download all", PlaylistBulk::Download),
                    ] {
                        if icon_action(ui, enabled, icon, label).clicked() {
                            self.bulk(action, &c.title, c.tracks.clone(), None);
                        }
                    }
                });
            });
        });
        ui.add_space(24.0);
        if c.tracks.is_empty() {
            ui.label(
                RichText::new(if self.busy {
                    "Loading music…"
                } else {
                    "No music here yet"
                })
                .color(MUTED),
            );
            return;
        }
        for (i, t) in c.tracks.iter().enumerate() {
            ui.push_id(("album", i), |ui| {
                if self.album_row(ui, &c, t, i) {
                    self.start_tracks(c.tracks.clone(), i, &c.title);
                }
            });
        }
        ui.add_space(14.0);
        ui.label(
            RichText::new(album_footer(&c.tracks))
                .size(13.0)
                .color(MUTED),
        );
        if c.continuation.is_some()
            && ui
                .add_enabled(!self.busy, egui::Button::new("Load more tracks"))
                .clicked()
        {
            self.more_collection();
        }
    }

    fn album_row(
        &mut self,
        ui: &mut egui::Ui,
        album: &Collection,
        track: &Track,
        i: usize,
    ) -> bool {
        let (rect, response) =
            ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW), egui::Sense::click());
        let active = self
            .current
            .as_ref()
            .is_some_and(|t| t.key() == track.key());
        let p = ui.painter();
        if response.hovered() || active {
            p.rect_filled(rect, 6.0, widgets::white_alpha(if active { 15 } else { 8 }));
        }
        p.hline(
            rect.x_range().shrink(8.0),
            rect.bottom(),
            Stroke::new(1.0, Color32::from_gray(40)),
        );
        let slot = Rect::from_center_size(
            Pos2::new(rect.left() + 28.0, rect.center().y),
            Vec2::splat(18.0),
        );
        if active {
            widgets::paint(p, Icon::Volume, slot, ACCENT);
        } else if response.hovered() {
            widgets::paint(p, Icon::Play, slot, WHITE);
        } else {
            p.text(
                slot.center(),
                egui::Align2::CENTER_CENTER,
                format!("{}", i + 1),
                FontId::proportional(14.0),
                MUTED,
            );
        }
        let text_x = rect.left() + 60.0;
        let max = rect.right() - text_x - 120.0;
        let own_artist = !track.artist.trim().is_empty()
            && album.credit.as_deref().map(str::trim) != Some(track.artist.trim());
        let title_y = if own_artist {
            rect.top() + 19.0
        } else {
            rect.center().y
        };
        fit_text(
            ui.painter(),
            Pos2::new(text_x, title_y),
            egui::Align2::LEFT_CENTER,
            &track.title,
            FontId::new(15.0, egui::FontFamily::Name("bold".into())),
            if active { ACCENT } else { WHITE },
            max,
        );
        if own_artist {
            let credit = ui.scope_id().with(("credit", track.key()));
            if let Some(link) = artist::credit_text(
                ui,
                credit,
                Pos2::new(text_x, rect.top() + 37.0),
                &artist::track_credit(track),
                FontId::proportional(13.0),
                MUTED,
                max,
            ) {
                self.open_artist_link(link);
            }
        }
        ui.painter().text(
            Pos2::new(rect.right() - 65.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            clock(track.duration),
            FontId::proportional(13.0),
            MUTED,
        );
        region(
            ui,
            Rect::from_min_size(
                Pos2::new(rect.right() - 35.0, rect.center().y - 16.0),
                Vec2::new(34.0, 32.0),
            ),
            |ui| {
                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.spacing_mut().button_padding = Vec2::new(6.0, 4.0);
                ui.menu_button("···", |ui| self.track_menu(ui, track));
            },
        );
        response.context_menu(|ui| self.track_menu(ui, track));
        response.clicked()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track(seconds: f64) -> Track {
        Track {
            duration: seconds,
            ..Default::default()
        }
    }

    #[test]
    fn meta_joins_type_and_year() {
        assert_eq!(album_meta("Album • 2022"), "Album · 2022");
        assert_eq!(album_meta(""), "");
    }

    #[test]
    fn footer_counts_songs_and_minutes() {
        let tracks: Vec<_> = (0..12).map(|_| track(240.0)).collect();
        assert_eq!(album_footer(&tracks), "12 songs, 48 minutes");
        assert_eq!(album_footer(&[track(30.0)]), "1 song, 1 minute");
        assert_eq!(album_footer(&[track(0.0)]), "1 song");
    }
}
