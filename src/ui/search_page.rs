//! The search results page: the top result beside the first songs, then the Artists and
//! Playlists rows, then the remaining songs.
use super::*;

/// Songs shown beside (or under) the top result.
const FIRST_SONGS: usize = 4;
/// From this page width the top result and the first songs sit side by side.
const TWO_COLUMNS: f32 = 900.0;

/// What a search shows first.
#[derive(Clone, Debug)]
pub(super) enum TopResult {
    Artist(ShelfItem),
    Song(Track),
}

/// The best artist match, else the first song.
pub(super) fn top_result(artists: &[ShelfItem], songs: &[Track]) -> Option<TopResult> {
    artists
        .first()
        .cloned()
        .map(TopResult::Artist)
        .or_else(|| songs.first().cloned().map(TopResult::Song))
}

impl YTDugem {
    pub(super) fn search_view(&mut self, ui: &mut egui::Ui) {
        ui.label(heading(
            if self.submitted_query.is_empty() {
                "Search"
            } else {
                &self.submitted_query
            },
            26.0,
        ));
        let songs = self.results.clone();
        let Some(top) = top_result(&self.artists.results, &songs) else {
            // Nothing yet (or no matches): the playlists, if any, and the list's own message.
            self.artist_results(ui);
            self.track_list(ui, songs, "Search");
            return;
        };
        ui.add_space(8.0);
        // A song on top is not listed again beside it.
        let from = usize::from(matches!(top, TopResult::Song(_)));
        let first = from..(from + FIRST_SONGS).min(songs.len());
        let gap = 24.0;
        let wide = ui.available_width() >= TWO_COLUMNS;
        let rows = first.len().max(2) as f32;
        let height = rows * 68.0 + (rows - 1.0) * ui.spacing().item_spacing.y;
        let first_songs = |app: &mut Self, ui: &mut egui::Ui| {
            for i in first.clone() {
                ui.push_id(("search-first", i), |ui| {
                    if app.song_row(ui, &songs[i], None, ui.available_width()) {
                        app.start_tracks(songs.clone(), i, "Search");
                    }
                });
            }
        };
        if wide {
            ui.horizontal_top(|ui| {
                ui.spacing_mut().item_spacing.x = gap;
                let card = (ui.available_width() - gap) * 0.4;
                self.top_result_card(ui, &top, &songs, Vec2::new(card, height));
                ui.vertical(|ui| first_songs(self, ui));
            });
        } else {
            let size = Vec2::new(ui.available_width(), 180.0);
            self.top_result_card(ui, &top, &songs, size);
            ui.add_space(8.0);
            first_songs(self, ui);
        }
        ui.add_space(12.0);
        self.artist_results(ui);
        if songs.len() > first.end {
            ui.add_space(10.0);
            ui.label(heading("Songs", 20.0));
            ui.add_space(4.0);
            self.track_list_with(ui, songs, "Search", true, first.end);
        }
    }

    /// The top result as a large card: artwork, name, kind, and Play (a song) or Open (an
    /// artist). Clicking anywhere does the same.
    pub(super) fn top_result_card(
        &mut self,
        ui: &mut egui::Ui,
        top: &TopResult,
        songs: &[Track],
        size: Vec2,
    ) {
        let (rect, response) = ui.allocate_exact_size(size, egui::Sense::click());
        let response = response.on_hover_cursor(egui::CursorIcon::PointingHand);
        let fill = if response.hovered() {
            Color32::from_rgb(47, 49, 45)
        } else {
            Color32::from_rgb(39, 41, 37)
        };
        ui.painter().rect_filled(rect, 12.0, fill);
        let side = (rect.height() - 40.0).clamp(40.0, 160.0);
        let art = Rect::from_min_size(
            Pos2::new(rect.left() + 20.0, rect.center().y - side / 2.0),
            Vec2::splat(side),
        );
        let (title, subtitle, icon, label) = match top {
            TopResult::Artist(item) => {
                artist::round_image(ui, &item.thumbnail, &item.title, art);
                (&item.title, item.subtitle.clone(), Icon::CaretRight, "Open")
            }
            TopResult::Song(t) => {
                draw_cover(ui, t, art, None);
                (&t.title, format!("Song • {}", t.artist), Icon::Play, "Play")
            }
        };
        let text_x = art.right() + 20.0;
        let text_width = rect.right() - text_x - 16.0;
        let top_y = rect.center().y - 58.0;
        let p = ui.painter();
        p.text(
            Pos2::new(text_x, top_y),
            egui::Align2::LEFT_TOP,
            "Top result",
            FontId::proportional(13.0),
            MUTED,
        );
        fit_text(
            p,
            Pos2::new(text_x, top_y + 38.0),
            egui::Align2::LEFT_CENTER,
            title,
            FontId::new(26.0, egui::FontFamily::Name("bold".into())),
            WHITE,
            text_width,
        );
        fit_text(
            p,
            Pos2::new(text_x, top_y + 68.0),
            egui::Align2::LEFT_CENTER,
            &subtitle,
            FontId::proportional(14.0),
            MUTED,
            text_width,
        );
        let button = Rect::from_min_size(Pos2::new(text_x, top_y + 88.0), Vec2::new(160.0, 40.0));
        let pressed = region(ui, button, |ui| {
            artist::pill(ui, icon, label, true, true).clicked()
        });
        if pressed || response.clicked() {
            match top {
                TopResult::Artist(item) => self.open_item(item.clone()),
                TopResult::Song(t) => {
                    let index = songs.iter().position(|s| s.key() == t.key());
                    match index {
                        Some(i) => self.start_tracks(songs.to_vec(), i, "Search"),
                        None => self.start_tracks(vec![t.clone()], 0, "Search"),
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_top_result_is_the_best_artist_else_the_first_song() {
        let artist = ShelfItem {
            title: "Dewa 19".into(),
            ..Default::default()
        };
        let song = Track {
            id: "a".into(),
            ..Default::default()
        };
        assert!(matches!(
            top_result(&[artist], &[song.clone()]),
            Some(TopResult::Artist(a)) if a.title == "Dewa 19"
        ));
        assert!(matches!(
            top_result(&[], &[song]),
            Some(TopResult::Song(t)) if t.id == "a"
        ));
        assert!(top_result(&[], &[]).is_none());
    }
}
