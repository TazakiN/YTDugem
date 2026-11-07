//! The player sheet's three-dot menu (upstream `DesktopSongMenuUi` on the player).
use super::*;

const MENU_WIDTH: f32 = 370.0;
const ROW_HEIGHT: f32 = 40.0;

/// What a menu row asks for; applied after the popup closes over `self`.
enum MenuAction {
    Rate(LikeStatus),
    AddToPlaylist,
    Download,
    Radio,
    PlayNext,
    AddToQueue,
    OpenAlbum(String),
    OpenArtist(String),
    SleepIn(u64),
    SleepAfterSong,
    SleepOff,
    Share,
    SleepMenu,
}

/// "12 min left" for the Sleep timer row; whole minutes, rounded up.
fn sleep_remaining(left: Duration) -> String {
    format!("{} min left", left.as_secs().div_ceil(60).max(1))
}

/// One menu row: icon, label, optional trailing text. Returns true when clicked.
fn menu_row(
    ui: &mut egui::Ui,
    icon: Icon,
    label: &str,
    trailing: Option<&str>,
    enabled: bool,
) -> bool {
    let sense = if enabled {
        egui::Sense::click()
    } else {
        egui::Sense::hover()
    };
    let (rect, response) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), ROW_HEIGHT), sense);
    if enabled && response.hovered() {
        ui.painter()
            .rect_filled(rect, 6.0, widgets::white_alpha(18));
    }
    let color = if enabled {
        WHITE
    } else {
        widgets::white_alpha(90)
    };
    widgets::paint(
        ui.painter(),
        icon,
        Rect::from_center_size(
            Pos2::new(rect.left() + 22.0, rect.center().y),
            Vec2::splat(18.0),
        ),
        color,
    );
    ui.painter().text(
        Pos2::new(rect.left() + 44.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(16.0),
        color,
    );
    if let Some(text) = trailing {
        ui.painter().text(
            Pos2::new(rect.right() - 14.0, rect.center().y),
            egui::Align2::RIGHT_CENTER,
            text,
            FontId::proportional(14.0),
            MUTED,
        );
    }
    enabled && response.clicked()
}

fn separator(ui: &mut egui::Ui) {
    ui.add_space(3.0);
    let (rect, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 1.0), egui::Sense::hover());
    ui.painter()
        .rect_filled(rect, 0.0, widgets::white_alpha(24));
    ui.add_space(3.0);
}

impl YTDugem {
    /// Hangs the menu off the sheet's "more" button.
    pub(super) fn more_menu(&mut self, ui: &mut egui::Ui, button: &egui::Response) {
        let Some(track) = self.current.clone() else {
            return;
        };
        let popup = ui.make_persistent_id("sheet-more-menu");
        let sleep_page = popup.with("sleep");
        if button.clicked() {
            egui::Popup::toggle_id(ui.ctx(), popup);
            ui.data_mut(|d| d.insert_temp(sleep_page, false));
        }
        if !egui::Popup::is_id_open(ui.ctx(), popup) {
            return;
        }
        let youtube = is_youtube_track(&track);
        let can_rate = youtube && self.account.is_some();
        let like = self.like_status().unwrap_or_default();
        let info = self
            .song_info
            .as_ref()
            .filter(|(k, _)| k == track.key())
            .map(|(_, i)| i.clone());
        let album = info.as_ref().and_then(|i| i.album_browse_id.clone());
        let artist = info.as_ref().and_then(|i| i.artist_browse_id.clone());
        let left = self
            .sleep_until
            .map(|t| t.saturating_duration_since(Instant::now()));
        let timer_text = if self.stop_after_track {
            Some("End of song".to_owned())
        } else {
            left.map(sleep_remaining)
        };
        let on_sleep_page = ui.data(|d| d.get_temp::<bool>(sleep_page)).unwrap_or(false);

        let mut action = None;
        egui::Popup::from_response(button)
            .id(popup)
            .open_memory(None)
            .align(egui::RectAlign::BOTTOM_END)
            .align_alternatives(&[])
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .layout(egui::Layout::top_down_justified(Align::LEFT))
            .frame(
                egui::Frame::new()
                    .fill(Color32::from_rgb(28, 28, 30))
                    .corner_radius(10.0)
                    .inner_margin(egui::Margin::symmetric(6, 6))
                    .stroke(Stroke::new(1.0, widgets::white_alpha(20))),
            )
            .show(|ui| {
                ui.set_width(MENU_WIDTH - 12.0);
                ui.spacing_mut().item_spacing.y = 0.0;
                if on_sleep_page {
                    if menu_row(ui, Icon::Back, "Sleep timer", None, true) {
                        ui.data_mut(|d| d.insert_temp(sleep_page, false));
                    }
                    separator(ui);
                    for minutes in [15, 30, 45, 60] {
                        if menu_row(ui, Icon::Moon, &format!("{minutes} minutes"), None, true) {
                            action = Some(MenuAction::SleepIn(minutes));
                        }
                    }
                    if menu_row(ui, Icon::Moon, "End of song", None, true) {
                        action = Some(MenuAction::SleepAfterSong);
                    }
                    if timer_text.is_some() {
                        separator(ui);
                        if menu_row(ui, Icon::Reset, "Turn off timer", None, true) {
                            action = Some(MenuAction::SleepOff);
                        }
                    }
                    return;
                }
                let (heart, like_label) = if like == LikeStatus::Like {
                    (Icon::HeartFill, "Remove from liked")
                } else {
                    (Icon::Heart, "Like")
                };
                if menu_row(ui, heart, like_label, None, can_rate) {
                    action = Some(MenuAction::Rate(if like == LikeStatus::Like {
                        LikeStatus::Indifferent
                    } else {
                        LikeStatus::Like
                    }));
                }
                let dislike_label = if like == LikeStatus::Dislike {
                    "Remove dislike"
                } else {
                    "Dislike"
                };
                if menu_row(ui, Icon::Dislike, dislike_label, None, can_rate) {
                    action = Some(MenuAction::Rate(if like == LikeStatus::Dislike {
                        LikeStatus::Indifferent
                    } else {
                        LikeStatus::Dislike
                    }));
                }
                if menu_row(ui, Icon::Playlist, "Add to playlist", None, true) {
                    action = Some(MenuAction::AddToPlaylist);
                }
                separator(ui);
                if menu_row(ui, Icon::Download, "Download", None, true) {
                    action = Some(MenuAction::Download);
                }
                if menu_row(ui, Icon::Radio, "Start radio", None, youtube) {
                    action = Some(MenuAction::Radio);
                }
                if menu_row(ui, Icon::PlayNext, "Play next", None, true) {
                    action = Some(MenuAction::PlayNext);
                }
                if menu_row(ui, Icon::AddToQueue, "Add to queue", None, true) {
                    action = Some(MenuAction::AddToQueue);
                }
                separator(ui);
                if menu_row(ui, Icon::Disc, "Open album", None, album.is_some()) {
                    action = album.clone().map(MenuAction::OpenAlbum);
                }
                if menu_row(ui, Icon::Person, "Open artist", None, artist.is_some()) {
                    action = artist.clone().map(MenuAction::OpenArtist);
                }
                separator(ui);
                if menu_row(ui, Icon::Moon, "Sleep timer", timer_text.as_deref(), true) {
                    action = Some(MenuAction::SleepMenu);
                }
                if menu_row(ui, Icon::Share, "Share", None, youtube) {
                    action = Some(MenuAction::Share);
                }
            });
        let Some(action) = action else {
            return;
        };
        if matches!(action, MenuAction::SleepMenu) {
            ui.data_mut(|d| d.insert_temp(sleep_page, true));
            return;
        }
        egui::Popup::close_all(ui.ctx());
        match action {
            MenuAction::SleepMenu => {}
            MenuAction::Rate(status) => self.rate(status),
            MenuAction::AddToPlaylist => {
                self.open_playlist_dialog(PlaylistDialog::AddTo { track });
            }
            MenuAction::Download => self.download(track),
            MenuAction::Radio => {
                self.now_origin = format!("{} Radio", track.title);
                self.library.queue.clear();
                self.radio_pending = false;
                self.fetch_radio();
                self.save();
            }
            MenuAction::PlayNext => {
                self.library.queue.insert(0, track);
                self.save();
            }
            MenuAction::AddToQueue => {
                self.library.queue.push(track);
                self.save();
            }
            MenuAction::OpenAlbum(id) => {
                let title = if track.album.is_empty() {
                    "Album".to_owned()
                } else {
                    track.album.clone()
                };
                self.expanded = false;
                self.open_item(ShelfItem {
                    browse_id: Some(id),
                    title,
                    ..Default::default()
                });
            }
            MenuAction::OpenArtist(id) => {
                self.expanded = false;
                self.open_item(ShelfItem {
                    browse_id: Some(id),
                    title: track.artist.clone(),
                    ..Default::default()
                });
            }
            MenuAction::SleepIn(minutes) => {
                self.stop_after_track = false;
                self.sleep_until = Some(Instant::now() + Duration::from_secs(minutes * 60));
                self.message = format!("Playback will stop in {minutes} minutes");
            }
            MenuAction::SleepAfterSong => {
                self.sleep_until = None;
                self.stop_after_track = true;
                self.message = "Playback will stop after this song".into();
            }
            MenuAction::SleepOff => {
                self.sleep_until = None;
                self.stop_after_track = false;
                self.message = "Sleep timer off".into();
            }
            MenuAction::Share => {
                ui.ctx()
                    .copy_text(format!("https://music.youtube.com/watch?v={}", track.id));
                self.message = "Link copied".into();
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sleep_remaining_rounds_up_to_whole_minutes() {
        assert_eq!(sleep_remaining(Duration::from_secs(1)), "1 min left");
        assert_eq!(sleep_remaining(Duration::from_secs(60)), "1 min left");
        assert_eq!(sleep_remaining(Duration::from_secs(61)), "2 min left");
        assert_eq!(sleep_remaining(Duration::from_secs(15 * 60)), "15 min left");
        assert_eq!(sleep_remaining(Duration::ZERO), "1 min left");
    }
}
