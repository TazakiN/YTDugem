//! The floating player bar (`PlayerBarStyle::Floating`): one pill over the bottom of the page
//! with the transport, what is playing with a seekable progress line, and the lyrics, queue and
//! volume buttons. A YTDugem addition modelled on the Apple Music web mini-player.
use super::*;

const HEIGHT: f32 = 64.0;
/// From the bottom of the page to the bar.
const GAP: f32 = 18.0;
/// The least room left on either side of the bar.
const SIDE_MARGIN: f32 = 24.0;
const MAX_WIDTH: f32 = 880.0;
/// Inside the pill, from its rounded ends to the first and last button.
const PAD: f32 = 16.0;
/// Between the button clusters and the now-playing block.
const CLUSTER_GAP: f32 = 18.0;
/// Below this the now-playing block is too cramped, so optional buttons go first.
const MIN_CENTER: f32 = 200.0;
const BUTTON: f32 = 32.0;
const SIDE_BUTTON: f32 = 30.0;
const PLAY: f32 = 36.0;
const SPACING: f32 = 2.0;
const COVER: f32 = 42.0;
/// Space the page keeps below its content so the last row can scroll clear of the bar.
pub(super) const RESERVED: f32 = HEIGHT + GAP;

/// Which optional buttons fit a bar of a given width.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
struct Fit {
    /// Shuffle and repeat.
    extras: bool,
    lyrics: bool,
}

impl Fit {
    fn left(self) -> f32 {
        if self.extras {
            2.0 * SIDE_BUTTON + 2.0 * BUTTON + PLAY + 4.0 * SPACING
        } else {
            2.0 * BUTTON + PLAY + 2.0 * SPACING
        }
    }
    fn right(self) -> f32 {
        // Queue and volume, and lyrics when it fits.
        if self.lyrics {
            3.0 * BUTTON + 2.0 * SPACING
        } else {
            2.0 * BUTTON + SPACING
        }
    }
    /// Width left for the now-playing block in a bar `width` wide.
    fn center(self, width: f32) -> f32 {
        (width - 2.0 * PAD - 2.0 * CLUSTER_GAP - self.left() - self.right()).max(0.0)
    }
}

/// Shuffle and repeat go first, then lyrics, so the now-playing block keeps `MIN_CENTER`.
fn fit(width: f32) -> Fit {
    [
        Fit {
            extras: true,
            lyrics: true,
        },
        Fit {
            extras: false,
            lyrics: true,
        },
    ]
    .into_iter()
    .find(|f| f.center(width) >= MIN_CENTER)
    .unwrap_or(Fit {
        extras: false,
        lyrics: false,
    })
}

/// The bar within `page` (the area right of the sidebar, below the top bar): centred, at most
/// `MAX_WIDTH` wide, `GAP` above the bottom edge.
fn bar_rect(page: Rect) -> Rect {
    let width = (page.width() - 2.0 * SIDE_MARGIN).clamp(0.0, MAX_WIDTH);
    Rect::from_center_size(
        Pos2::new(page.center().x, page.bottom() - GAP - HEIGHT / 2.0),
        Vec2::new(width, HEIGHT),
    )
}

/// Seconds at pointer `x` on a progress line from `left` to `right`.
fn seek_target(x: f32, left: f32, right: f32, duration: f64) -> f64 {
    if right <= left || duration <= 0.0 || !duration.is_finite() {
        return 0.0;
    }
    f64::from(((x - left) / (right - left)).clamp(0.0, 1.0)) * duration
}

impl YTDugem {
    /// Draws the bar over the bottom of `page`. `shown` is the Now Playing sheet's slide (0 to
    /// 1): the bar fades and drops away as the sheet comes up.
    pub(super) fn floating_bar(&mut self, ui: &mut egui::Ui, page: Rect, shown: f32) {
        let visible = 1.0 - shown;
        if visible <= 0.0 || page.width() < 2.0 * SIDE_MARGIN + 100.0 {
            return;
        }
        let bar = bar_rect(page).translate(Vec2::new(0.0, shown * RESERVED));
        let mut ui = ui.new_child(
            egui::UiBuilder::new()
                .max_rect(bar)
                .layout(egui::Layout::left_to_right(Align::Center)),
        );
        ui.set_clip_rect(page);
        ui.set_opacity(visible);
        let radius = HEIGHT / 2.0;
        ui.painter().add(
            egui::epaint::Shadow {
                offset: [0, 8],
                blur: 28,
                spread: 0,
                color: Color32::from_black_alpha(110),
            }
            .as_shape(bar, radius),
        );
        ui.painter().rect(
            bar,
            radius,
            Color32::from_rgba_unmultiplied(30, 31, 30, 240),
            Stroke::new(1.0_f32, Color32::from_gray(70)),
            egui::StrokeKind::Inside,
        );
        // Claims the whole pill first, so clicks and hovers never reach the page underneath.
        ui.interact(
            bar,
            ui.make_persistent_id("floating-bar"),
            egui::Sense::click(),
        );

        let fit = fit(bar.width());
        let row = |left: f32, width: f32| {
            Rect::from_min_size(
                Pos2::new(left, bar.top()),
                Vec2::new(width.max(0.0), HEIGHT),
            )
        };
        region(&mut ui, row(bar.left() + PAD, fit.left()), |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = SPACING;
                self.transport_buttons(
                    ui,
                    Transport {
                        side: SIDE_BUTTON,
                        skip: BUTTON,
                        play: PLAY,
                        previous: Icon::Rewind,
                        next: Icon::FastForward,
                        extras: fit.extras,
                    },
                );
            });
        });
        let center_left = bar.left() + PAD + fit.left() + CLUSTER_GAP;
        self.floating_now_playing(&mut ui, row(center_left, fit.center(bar.width())));
        let tools_left = bar.right() - PAD - fit.right();
        region(&mut ui, row(tools_left, fit.right()), |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = SPACING;
                self.floating_tools(ui, fit.lyrics);
            });
        });
    }

    /// Cover, title, "artist — album", the progress line and the track's "•••" menu.
    fn floating_now_playing(&mut self, ui: &mut egui::Ui, block: Rect) {
        if block.width() < COVER {
            return;
        }
        let cover = Rect::from_min_size(
            Pos2::new(block.left(), block.top() + 8.0),
            Vec2::splat(COVER),
        );
        let text_left = cover.right() + 10.0;
        let Some(track) = self.current.clone() else {
            ui.painter()
                .rect_filled(cover, 8.0, Color32::from_rgb(51, 53, 48));
            widgets::paint(
                ui.painter(),
                Icon::Logo,
                Rect::from_center_size(cover.center(), Vec2::splat(20.0)),
                Color32::from_gray(105),
            );
            fit_text(
                ui.painter(),
                Pos2::new(text_left, cover.center().y),
                egui::Align2::LEFT_CENTER,
                "Not playing",
                FontId::proportional(12.5),
                MUTED,
                block.right() - text_left,
            );
            return;
        };
        let more = Rect::from_center_size(
            Pos2::new(block.right() - BUTTON / 2.0, cover.center().y),
            Vec2::splat(BUTTON),
        );
        let text_width = (more.left() - 8.0 - text_left).max(0.0);
        // Cover and text open Now Playing; the artist links drawn later take their own clicks.
        let open = ui.interact(
            Rect::from_min_max(
                block.min + Vec2::new(0.0, 4.0),
                Pos2::new(text_left + text_width, cover.bottom() + 1.0),
            ),
            ui.make_persistent_id("floating-open-player"),
            egui::Sense::click(),
        );
        if open.clone().on_hover_text("Open Now Playing").clicked() {
            self.expanded = true;
            self.tab = PlayerPane::Main;
        }
        draw_cover(ui, &track, cover, Some(self.cover.as_ref()));
        if self.loading {
            self.now_playing_badge(ui, cover);
        }
        if text_width > 8.0 {
            fit_text(
                ui.painter(),
                Pos2::new(text_left, block.top() + 19.0),
                egui::Align2::LEFT_CENTER,
                &track.title,
                FontId::new(13.0, egui::FontFamily::Name("bold".into())),
                WHITE,
                text_width,
            );
            self.floating_credit(
                ui,
                &track,
                Pos2::new(text_left, block.top() + 38.0),
                text_width,
            );
        }
        let response = region(ui, more, |ui| {
            widgets::button(ui, Icon::More, BUTTON, false, "More")
        });
        egui::Popup::menu(&response)
            .align(egui::RectAlign::TOP_END)
            .show(|ui| self.track_menu(ui, &track));
        self.floating_progress(
            ui,
            Rect::from_min_max(
                Pos2::new(block.left(), block.top() + 52.0),
                Pos2::new(block.right(), block.top() + 62.0),
            ),
        );
    }

    /// The artist credit (its names link to their pages), then " — album" while it fits.
    fn floating_credit(&mut self, ui: &mut egui::Ui, track: &Track, pos: Pos2, width: f32) {
        let font = FontId::proportional(12.0);
        let credit = artist::track_credit(track);
        let credit_width: f32 = credit
            .iter()
            .map(|(text, _)| {
                ui.painter()
                    .layout_no_wrap(text.clone(), font.clone(), MUTED)
                    .size()
                    .x
            })
            .sum();
        let album = track.album.trim();
        // The album only gets what the artists leave, and is dropped when that is too little.
        let album_room = width - credit_width;
        let show_album = !album.is_empty() && album_room >= 60.0;
        if let Some(link) = artist::credit_text(
            ui,
            ui.make_persistent_id("floating-credit"),
            pos,
            &credit,
            font.clone(),
            MUTED,
            if show_album { credit_width } else { width },
        ) {
            self.open_artist_link(link);
        }
        if show_album {
            fit_text(
                ui.painter(),
                Pos2::new(pos.x + credit_width, pos.y),
                egui::Align2::LEFT_CENTER,
                &format!(" — {album}"),
                font,
                MUTED,
                album_room,
            );
        }
    }

    /// A thin line across the now-playing block; click or drag it to seek.
    fn floating_progress(&mut self, ui: &mut egui::Ui, hit: Rect) {
        let duration = self.duration();
        let response = ui.interact(
            hit,
            ui.make_persistent_id("floating-seek"),
            egui::Sense::click_and_drag(),
        );
        let response = response.on_hover_text("Seek playback position");
        let mut position = self.status.position;
        if duration > 0.0 && (response.clicked() || response.dragged()) {
            if let Some(pos) = response.interact_pointer_pos() {
                let target = seek_target(pos.x, hit.left(), hit.right(), duration);
                if (target - position).abs() > 0.02 && self.player.is_active() {
                    let r = self.player.seek(target);
                    self.report(r);
                }
                position = target;
            }
        }
        if response.hovered() || response.dragged() {
            ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
        }
        let fraction = if duration > 0.0 {
            (position / duration).clamp(0.0, 1.0) as f32
        } else {
            0.0
        };
        let thick = ui.ctx().animate_value_with_time(
            response.id.with("thickness"),
            if response.hovered() || response.dragged() {
                4.0
            } else {
                2.0
            },
            0.12,
        );
        let line = Rect::from_center_size(hit.center(), Vec2::new(hit.width(), thick));
        ui.painter()
            .rect_filled(line, thick / 2.0, Color32::from_gray(70));
        ui.painter().rect_filled(
            Rect::from_min_max(
                line.min,
                Pos2::new(line.left() + line.width() * fraction, line.bottom()),
            ),
            thick / 2.0,
            WHITE,
        );
    }

    /// Lyrics (when it fits), queue, and a volume button whose popup holds the slider.
    fn floating_tools(&mut self, ui: &mut egui::Ui, lyrics: bool) {
        let lyrics_open = self.expanded && self.tab == PlayerPane::Lyrics;
        if lyrics && widgets::button(ui, Icon::Lyrics, BUTTON, lyrics_open, "Lyrics").clicked() {
            self.expanded = true;
            self.tab = PlayerPane::Lyrics;
        }
        if widgets::button(ui, Icon::ListBullets, BUTTON, false, "Queue").clicked() {
            self.expanded = true;
            self.tab = PlayerPane::Queue;
        }
        let icon = if self.library.volume < 50.0 {
            Icon::VolumeLow
        } else {
            Icon::Volume
        };
        let response = widgets::button(ui, icon, BUTTON, false, "Volume");
        egui::Popup::from_toggle_button_response(&response)
            .align(egui::RectAlign::TOP)
            .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
            .show(|ui| {
                ui.horizontal(|ui| {
                    self.volume_slider(ui, 140.0, "floating-volume");
                    ui.label(
                        RichText::new(format!("{:.0}", self.library.volume))
                            .size(12.0)
                            .color(MUTED),
                    );
                });
            });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn optional_buttons_drop_shuffle_and_repeat_first_then_lyrics() {
        let all = Fit {
            extras: true,
            lyrics: true,
        };
        assert_eq!(fit(880.0), all);
        // The narrowest page: a 900 px window with the sidebar open.
        let narrowest = bar_rect(Rect::from_min_size(
            Pos2::ZERO,
            Vec2::new(900.0 - 258.0, 500.0),
        ));
        assert_eq!(fit(narrowest.width()), all);
        let no_extras = Fit {
            extras: false,
            lyrics: true,
        };
        let w = 2.0 * PAD + 2.0 * CLUSTER_GAP + no_extras.left() + no_extras.right() + MIN_CENTER;
        assert_eq!(fit(w), no_extras);
        assert_eq!(fit(w - 1.0), Fit::default());
        assert_eq!(fit(0.0), Fit::default());
        for width in [0.0, 300.0, 420.0, 594.0, 880.0] {
            let f = fit(width);
            // Clusters and the centre never overlap: together they never exceed the bar.
            let used = 2.0 * PAD + 2.0 * CLUSTER_GAP + f.left() + f.right() + f.center(width);
            assert!(f.center(width) == 0.0 || used <= width + 0.01, "{width}");
        }
    }

    #[test]
    fn bar_is_centred_capped_and_above_the_bottom() {
        let page = Rect::from_min_max(Pos2::new(258.0, 78.0), Pos2::new(2258.0, 1078.0));
        let bar = bar_rect(page);
        assert_eq!(bar.width(), MAX_WIDTH);
        assert_eq!(bar.height(), HEIGHT);
        assert_eq!(bar.center().x, page.center().x);
        assert_eq!(bar.bottom(), page.bottom() - GAP);
        let narrow = Rect::from_min_size(Pos2::ZERO, Vec2::new(500.0, 400.0));
        assert_eq!(bar_rect(narrow).width(), 500.0 - 2.0 * SIDE_MARGIN);
        assert_eq!(
            bar_rect(Rect::from_min_size(Pos2::ZERO, Vec2::splat(10.0))).width(),
            0.0
        );
    }

    #[test]
    fn seeking_maps_the_pointer_onto_the_track() {
        assert_eq!(seek_target(50.0, 0.0, 100.0, 200.0), 100.0);
        assert_eq!(seek_target(-5.0, 0.0, 100.0, 200.0), 0.0);
        assert_eq!(seek_target(150.0, 0.0, 100.0, 200.0), 200.0);
        assert_eq!(seek_target(50.0, 0.0, 100.0, 0.0), 0.0);
        assert_eq!(seek_target(50.0, 0.0, 100.0, f64::NAN), 0.0);
        assert_eq!(seek_target(50.0, 10.0, 10.0, 200.0), 0.0);
    }
}
