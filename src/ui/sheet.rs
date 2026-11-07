//! The Now Playing sheet: upstream `DesktopPlayerSheet` (a drawer that rises over the window,
//! dragged down or closed by its handle or Escape) around `LandscapePlayerLayout` (two equal
//! columns: the sleeve with the lyrics / output / queue row on the left, and on the right the
//! credits and transport, the lyrics or the queue).
use super::*;

/// Room above the columns for the drag handle (upstream `LANDSCAPE_HANDLE_STRIP`).
const HANDLE_STRIP: f32 = 24.0;
/// The widest the two columns get between them (upstream `LANDSCAPE_PLAYER_MAX_WIDTH`).
const MAX_WIDTH: f32 = 1100.0;
/// Upstream `PLAYER_GUTTER` and `PLAYER_MAX_WIDTH`.
const GUTTER: f32 = 30.0;
const MAIN_MAX_WIDTH: f32 = 560.0;
/// The bottom row's glyphs and capsule height (upstream `BOTTOM_ACTION_SIZE`).
const ACTION: f32 = 44.0;
const SEGMENT: f32 = 64.0;
const SEGMENT_TRIPLE: f32 = 52.0;
/// A drag past a quarter of the window, or a flick, closes the sheet (upstream values).
const DISMISS_FRACTION: f32 = 0.25;
const DISMISS_VELOCITY: f32 = 1600.0;
/// The sleeve shrinks a little while paused (upstream `ARTWORK_PAUSE_SHRINK_SCALE`).
const PAUSED_ART_SCALE: f32 = 0.88;

/// Shown in the lyric strip before the first line is sung (upstream `lyrics_intro_lines`).
const INTRO_LINES: [&str; 11] = [
    "The song is starting",
    "Warming up",
    "Setting the mood",
    "Bass first, words later",
    "Wait for it",
    "Feel that build",
    "Just the groove for now",
    "The hook is on the way",
    "Let it breathe",
    "Cue the vocals",
    "First notes in",
];
/// Shown while the lyrics are being looked up (upstream `lyrics_loading_lines`).
const LOADING_LINES: [&str; 6] = [
    "Getting the lyrics",
    "Finding the words",
    "Fetching the verses",
    "Lyrics are loading",
    "Words are on the way",
    "Finding the right words",
];

/// One phrase per song, so it does not change while the song plays.
fn phrase<'a>(lines: &[&'a str], key: &str) -> &'a str {
    let hash = key
        .bytes()
        .fold(0u32, |h, b| h.wrapping_mul(31).wrapping_add(u32::from(b)));
    lines[hash as usize % lines.len()]
}

impl YTDugem {
    /// Escape walks back one step: the output dialog, an open menu, the lyrics or queue pane,
    /// and only then the sheet itself (upstream `PlayerBackHandler` order).
    pub(super) fn sheet_back(&mut self, ctx: &egui::Context) {
        if !self.expanded {
            return;
        }
        if self.output_open {
            self.output_open = false;
        } else if egui::Popup::is_any_open(ctx) {
            egui::Popup::close_all(ctx);
        } else if self.tab != PlayerPane::Main {
            self.tab = PlayerPane::Main;
        } else {
            self.expanded = false;
        }
    }

    /// `shown` runs 0 to 1 with the slide; `sheet_drag` is where a drag has pulled it down to.
    pub(super) fn expanded_view(&mut self, ctx: &egui::Context, shown: f32) {
        let screen = ctx.content_rect();
        let height = screen.height();
        if !self.expanded && shown <= 0.0 {
            self.sheet_drag = 0.0;
        }
        let drag = self.sheet_drag.clamp(0.0, height);
        // Opening starts from the bottom; closing after a drag carries on from where it was let go.
        let top = screen.top() + height - shown * (height - drag);
        egui::Area::new(egui::Id::unique("player-sheet"))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(screen.left(), top))
            .movable(false)
            .constrain(false)
            .show(ctx, |ui| {
                // Claims the whole sheet, so clicks never reach the page underneath; a drag that
                // nothing on top claims (a slider, the lyrics' scrolling) moves the sheet.
                let (rect, response) =
                    ui.allocate_exact_size(screen.size(), egui::Sense::click_and_drag());
                self.player_sheet(ui, rect);
                self.sheet_drag_step(ui.ctx(), &response, height);
            });
    }

    fn sheet_drag_step(&mut self, ctx: &egui::Context, response: &egui::Response, height: f32) {
        if !self.expanded {
            return;
        }
        if response.dragged() {
            self.sheet_drag = (self.sheet_drag + response.drag_delta().y).clamp(0.0, height);
            ctx.set_cursor_icon(egui::CursorIcon::Grabbing);
        }
        if response.drag_stopped() {
            let velocity = ctx.input(|i| i.pointer.velocity().y);
            if self.sheet_drag > height * DISMISS_FRACTION || velocity > DISMISS_VELOCITY {
                self.expanded = false;
                return;
            }
        }
        if !response.dragged() && self.sheet_drag > 0.0 {
            // Settles back up when let go short of closing (upstream: 220 ms).
            let dt = ctx.input(|i| i.stable_dt).min(0.05);
            self.sheet_drag *= (1.0 - dt * 14.0).max(0.0);
            if self.sheet_drag < 0.5 {
                self.sheet_drag = 0.0;
            }
            ctx.request_repaint();
        }
    }

    fn player_sheet(&mut self, ui: &mut egui::Ui, rect: Rect) {
        self.sheet_background(ui, rect);

        let inner = Rect::from_min_max(
            Pos2::new(rect.left(), rect.top() + HANDLE_STRIP),
            Pos2::new(rect.right(), rect.bottom() - 20.0),
        );
        let width = inner.width().min(MAX_WIDTH);
        let x = inner.center().x - width / 2.0;
        let half = width / 2.0;
        let left = Rect::from_min_max(
            Pos2::new(x + GUTTER, inner.top()),
            Pos2::new(x + half - GUTTER, inner.bottom()),
        );
        let right = Rect::from_min_max(
            Pos2::new(x + half + GUTTER, inner.top()),
            Pos2::new(x + width - GUTTER, inner.bottom()),
        );

        // Left column: the sleeve takes whichever axis runs out first once the row has its room.
        const ROW_BLOCK: f32 = ACTION + 30.0;
        let actions =
            Rect::from_min_max(Pos2::new(left.left(), left.bottom() - ROW_BLOCK), left.max);
        let art_area = Rect::from_min_max(left.min, Pos2::new(left.right(), actions.top() - 24.0));
        let side = art_area.width().min(art_area.height()).max(80.0);
        let playing = !self.status.paused && !self.status.ended;
        let scale = ui.ctx().animate_value_with_time(
            egui::Id::unique("sheet-art-scale"),
            if playing || self.current.is_none() {
                1.0
            } else {
                PAUSED_ART_SCALE
            },
            0.3,
        );
        let art = Rect::from_center_size(art_area.center(), Vec2::splat(side * scale));
        self.sheet_artwork(ui, art);
        // With a panel up the sleeve is the way back to the player.
        if self.tab != PlayerPane::Main {
            let r = ui
                .interact(art, ui.scope_id().with("sheet-art"), egui::Sense::click())
                .on_hover_cursor(egui::CursorIcon::PointingHand);
            if r.clicked() {
                self.tab = PlayerPane::Main;
            }
        }
        self.sheet_actions(ui, actions);

        // Right column: one of three panes.
        let mut column = ui.new_child(egui::UiBuilder::new().max_rect(right));
        match self.tab {
            PlayerPane::Main => self.main_pane(&mut column, right),
            PlayerPane::Lyrics => self.lyrics_pane(&mut column, right),
            PlayerPane::Queue => self.queue_pane(&mut column, right),
        }

        self.sheet_handle(ui, rect);
        if self.output_open {
            self.output_dialog(ui, rect);
        }
        // The page's status line is hidden behind the sheet; failures must still be seen.
        if !self.message.is_empty() {
            fit_text(
                ui.painter(),
                Pos2::new(rect.center().x, rect.bottom() - 10.0),
                egui::Align2::CENTER_CENTER,
                &self.message,
                FontId::proportional(12.0),
                MUTED,
                rect.width() - 80.0,
            );
        }
    }

    fn sheet_background(&self, ui: &egui::Ui, rect: Rect) {
        let painter = ui.painter();
        painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 19, 17));
        if let Some(back) = &self.backdrop {
            painter.image(
                back.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(120));
        // Darker towards the bottom, where the row and the lyric bar sit.
        let mut shade = egui::Mesh::default();
        let clear = Color32::TRANSPARENT;
        let dark = Color32::from_black_alpha(110);
        shade.colored_vertex(Pos2::new(rect.left(), rect.center().y), clear);
        shade.colored_vertex(Pos2::new(rect.right(), rect.center().y), clear);
        shade.colored_vertex(rect.right_bottom(), dark);
        shade.colored_vertex(rect.left_bottom(), dark);
        shade.add_triangle(0, 1, 2);
        shade.add_triangle(0, 2, 3);
        painter.add(shade);
    }

    /// The handle at the top edge: it promises the drag, and closes the sheet on a click.
    fn sheet_handle(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let bar = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.top() + 10.5),
            Vec2::new(38.0, 5.0),
        );
        let hit = Rect::from_center_size(
            Pos2::new(rect.center().x, rect.top() + HANDLE_STRIP / 2.0),
            Vec2::new(rect.width() * 0.2, HANDLE_STRIP),
        );
        let response = ui
            .interact(
                hit,
                ui.scope_id().with("sheet-handle"),
                egui::Sense::click(),
            )
            .on_hover_text("Close player · Esc");
        response
            .widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, "Close player"));
        let alpha = if response.hovered() { 230 } else { 180 };
        ui.painter()
            .rect_filled(bar, 3.0, Color32::from_white_alpha(alpha));
        if response.clicked() {
            self.expanded = false;
        }
    }

    fn sheet_artwork(&self, ui: &mut egui::Ui, art: Rect) {
        let radius = 10.0;
        if self.cover.is_some() {
            ui.painter().add(
                egui::Shadow {
                    offset: [0, 12],
                    blur: 40,
                    spread: 0,
                    color: Color32::from_black_alpha(140),
                }
                .as_shape(art, radius),
            );
        }
        match (&self.cover, &self.current) {
            (Some(tex), _) => {
                egui::Image::new(tex)
                    .fit_to_exact_size(art.size())
                    .corner_radius(radius)
                    .paint_at(ui, art);
            }
            // Optimistic: the list's thumbnail stands in until the full artwork arrives.
            (None, Some(t)) => draw_image(ui, &t.thumbnail, art, None),
            (None, None) => {
                ui.painter()
                    .rect_filled(art, radius, Color32::from_black_alpha(46));
                widgets::paint(
                    ui.painter(),
                    Icon::Music,
                    art.shrink(art.width() * 0.32),
                    Color32::from_white_alpha(90),
                );
            }
        }
    }

    /// Lyrics, the capsule and the queue, with the output's name under them (upstream
    /// `PlayerActionRow` and `OutputCaption`). The capsule is the output and Listen Together
    /// normally, and shuffle / repeat / AutoPlay while the queue is up.
    fn sheet_actions(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let queue_open = self.tab == PlayerPane::Queue;
        let pill_width = if queue_open {
            SEGMENT_TRIPLE * 3.0 + 2.0
        } else {
            SEGMENT * 2.0 + 1.0
        };
        let widest = ACTION * 2.0 + SEGMENT * 3.0 + 2.0;
        let inset = ((rect.width() - widest) / 4.0).max(0.0);
        let row = Rect::from_min_size(
            Pos2::new(rect.left() + inset, rect.top()),
            Vec2::new(rect.width() - 2.0 * inset, ACTION),
        );

        let lyrics = Rect::from_min_size(row.min, Vec2::splat(ACTION));
        if self
            .bottom_glyph(
                ui,
                lyrics,
                Icon::LyricsQuote,
                self.tab == PlayerPane::Lyrics,
                if self.tab == PlayerPane::Lyrics {
                    "Close lyrics · L"
                } else {
                    "Open lyrics · L"
                },
            )
            .clicked()
        {
            self.tab = if self.tab == PlayerPane::Lyrics {
                PlayerPane::Main
            } else {
                PlayerPane::Lyrics
            };
        }
        let queue = Rect::from_min_size(
            Pos2::new(row.right() - ACTION, row.top()),
            Vec2::splat(ACTION),
        );
        if self
            .bottom_glyph(ui, queue, Icon::ListBullets, queue_open, "Up next · Q")
            .clicked()
        {
            self.tab = if queue_open {
                PlayerPane::Main
            } else {
                PlayerPane::Queue
            };
        }

        let pill = Rect::from_center_size(row.center(), Vec2::new(pill_width, ACTION));
        ui.painter()
            .rect_filled(pill, ACTION / 2.0, Color32::from_white_alpha(31));
        if queue_open {
            let repeat_label = match self.repeat {
                0 => "Repeat off",
                1 => "Repeat all",
                _ => "Repeat one",
            };
            let segments = [
                (Icon::Shuffle, self.shuffle, "Shuffle"),
                (Icon::Repeat, self.repeat > 0, repeat_label),
                (Icon::Infinity, self.library.autoplay, "AutoPlay"),
            ];
            for (i, (icon, on, label)) in segments.into_iter().enumerate() {
                let r = Rect::from_min_size(
                    Pos2::new(pill.left() + i as f32 * (SEGMENT_TRIPLE + 1.0), pill.top()),
                    Vec2::new(SEGMENT_TRIPLE, ACTION),
                );
                let text = (i == 1 && self.repeat == 2).then_some("1");
                if self
                    .pill_segment(ui, pill, r, icon, 24.0, on, label, text)
                    .clicked()
                {
                    match i {
                        0 => self.shuffle = !self.shuffle,
                        1 => self.repeat = (self.repeat + 1) % 3,
                        _ => {
                            self.library.autoplay = !self.library.autoplay;
                            self.save();
                        }
                    }
                }
            }
        } else {
            let output = Rect::from_min_size(pill.min, Vec2::new(SEGMENT, ACTION));
            if self
                .pill_segment(
                    ui,
                    pill,
                    output,
                    Icon::HeadphonesFill,
                    23.0,
                    false,
                    "Audio output",
                    None,
                )
                .clicked()
            {
                self.open_output();
            }
            let party = Rect::from_min_size(
                Pos2::new(output.right() + 1.0, pill.top()),
                Vec2::new(SEGMENT, ACTION),
            );
            // TODO: Listen Together
            let _ = self.pill_segment(
                ui,
                pill,
                party,
                Icon::Person,
                22.0,
                false,
                "Listen together (coming soon)",
                None,
            );
        }
        // The output's name; a click opens the output dialog.
        let caption = self.output_name();
        let caption_rect = Rect::from_center_size(
            Pos2::new(row.center().x, row.bottom() + 22.0),
            Vec2::new(rect.width() * 0.65, 18.0),
        );
        let response = ui
            .interact(
                caption_rect,
                ui.scope_id().with("sheet-output-caption"),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand);
        fit_text(
            ui.painter(),
            caption_rect.center(),
            egui::Align2::CENTER_CENTER,
            &caption,
            FontId::new(11.0, egui::FontFamily::Name("bold".into())),
            Color32::from_white_alpha(if response.hovered() { 200 } else { 140 }),
            caption_rect.width(),
        );
        if response.clicked() {
            self.open_output();
        }
    }

    fn open_output(&mut self) {
        self.output_open = true;
        self.load_audio_devices();
    }

    /// The output device's name as the dialog lists it.
    pub(super) fn output_name(&self) -> String {
        let id = &self.library.audio.output_device;
        if id.is_empty() {
            return "System default".into();
        }
        self.audio_devices
            .iter()
            .find(|(d, _)| d == id)
            .map_or_else(|| id.clone(), |(_, name)| name.clone())
    }

    /// A glyph in the bottom row, a disc behind it while its pane is open (upstream `BottomGlyph`).
    fn bottom_glyph(
        &self,
        ui: &mut egui::Ui,
        rect: Rect,
        icon: Icon,
        highlighted: bool,
        label: &str,
    ) -> egui::Response {
        let response = ui
            .interact(
                rect,
                ui.scope_id().with(("bottom-glyph", label)),
                egui::Sense::click(),
            )
            .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, highlighted, label)
        });
        if highlighted || response.hovered() {
            ui.painter().circle_filled(
                rect.center(),
                rect.width() / 2.0,
                Color32::from_white_alpha(if highlighted { 36 } else { 20 }),
            );
        }
        widgets::paint(
            ui.painter(),
            icon,
            Rect::from_center_size(rect.center(), Vec2::splat(26.0)),
            Color32::from_white_alpha(if highlighted || response.hovered() {
                255
            } else {
                200
            }),
        );
        response
    }

    /// One control inside the capsule; its highlight fills its share edge to edge.
    #[allow(clippy::too_many_arguments)]
    fn pill_segment(
        &self,
        ui: &mut egui::Ui,
        pill: Rect,
        rect: Rect,
        icon: Icon,
        size: f32,
        highlighted: bool,
        label: &str,
        text: Option<&str>,
    ) -> egui::Response {
        let response = ui
            .interact(
                rect,
                ui.scope_id().with(("pill-segment", label)),
                egui::Sense::click(),
            )
            .on_hover_text(label);
        response.widget_info(|| {
            egui::WidgetInfo::selected(egui::Role::Button, true, highlighted, label)
        });
        let fill = if highlighted {
            Some(Color32::from_white_alpha(36))
        } else if response.hovered() {
            Some(Color32::from_white_alpha(16))
        } else {
            None
        };
        if let Some(fill) = fill {
            // Rounded only where the segment meets the capsule's ends.
            let r = ACTION / 2.0;
            let corner = egui::CornerRadius {
                nw: if rect.left() <= pill.left() + 0.5 {
                    r as u8
                } else {
                    0
                },
                sw: if rect.left() <= pill.left() + 0.5 {
                    r as u8
                } else {
                    0
                },
                ne: if rect.right() >= pill.right() - 0.5 {
                    r as u8
                } else {
                    0
                },
                se: if rect.right() >= pill.right() - 0.5 {
                    r as u8
                } else {
                    0
                },
            };
            ui.painter().rect_filled(rect, corner, fill);
        }
        let tint = Color32::from_white_alpha(if highlighted { 255 } else { 190 });
        match text {
            Some(text) => {
                ui.painter().text(
                    rect.center(),
                    egui::Align2::CENTER_CENTER,
                    text,
                    FontId::new(17.0, egui::FontFamily::Name("bold".into())),
                    tint,
                );
            }
            None => widgets::paint(
                ui.painter(),
                icon,
                Rect::from_center_size(rect.center(), Vec2::splat(size)),
                tint,
            ),
        }
        if rect.right() < pill.right() - 1.0 {
            ui.painter().line_segment(
                [
                    Pos2::new(rect.right() + 0.5, pill.top()),
                    Pos2::new(rect.right() + 0.5, pill.bottom()),
                ],
                Stroke::new(1.0, Color32::from_white_alpha(51)),
            );
        }
        response
    }

    /// The right column's main state: where the session came from, the credits, the current
    /// line, the scrubber, the transport and the volume bar (upstream `LandscapeMainPane`),
    /// centred in the column.
    fn main_pane(&mut self, ui: &mut egui::Ui, rect: Rect) {
        const CAPTION: f32 = 26.0;
        const CREDITS: f32 = 60.0;
        const STRIP: f32 = 34.0;
        const SCRUB: f32 = 44.0;
        const TRANSPORT: f32 = 96.0;
        const VOLUME: f32 = 32.0;
        let show_caption = self.current.is_some() && !self.now_origin.is_empty();
        let height = if show_caption { CAPTION } else { 0.0 }
            + CREDITS
            + 10.0
            + STRIP
            + SCRUB
            + 8.0
            + TRANSPORT
            + 12.0
            + VOLUME;
        let width = rect.width().min(MAIN_MAX_WIDTH);
        let mut y = rect.top() + ((rect.height() - height) / 2.0).max(0.0);
        let x = rect.left();
        let row = |y: f32, h: f32| Rect::from_min_size(Pos2::new(x, y), Vec2::new(width, h));

        if show_caption {
            fit_text(
                ui.painter(),
                Pos2::new(x, y + CAPTION / 2.0 - 3.0),
                egui::Align2::LEFT_CENTER,
                &format!("Playing from {}", self.now_origin),
                FontId::new(13.0, egui::FontFamily::Name("bold".into())),
                WHITE,
                width,
            );
            y += CAPTION;
        }
        self.credits(ui, row(y, CREDITS));
        y += CREDITS + 10.0;
        self.lyric_strip(ui, row(y, STRIP));
        y += STRIP;
        self.scrubber(ui, row(y, SCRUB));
        y += SCRUB + 8.0;
        self.transport(ui, row(y, TRANSPORT));
        y += TRANSPORT + 12.0;
        let volume = row(y, VOLUME);
        region(ui, volume, |u| {
            u.horizontal_centered(|u| {
                u.spacing_mut().item_spacing.x = 10.0;
                let quiet = Color32::from_white_alpha(128);
                let (r, _) = u.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
                widgets::paint(u.painter(), Icon::VolumeLow, r, quiet);
                self.volume_slider(u, (width - 60.0).max(40.0), "sheet-volume");
                let (r, _) = u.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
                widgets::paint(u.painter(), Icon::Volume, r, quiet);
            });
        });
    }

    /// Title, artist, like and the three-dot menu (upstream `LandscapeCredits`).
    fn credits(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let can_like =
            self.account.is_some() && self.current.as_ref().is_some_and(is_youtube_track);
        let buttons = 34.0 + if can_like { 42.0 } else { 0.0 };
        let text_width = rect.width() - buttons - 10.0;
        let (title, artist) =
            self.current
                .as_ref()
                .map_or(("Nothing playing".to_owned(), String::new()), |t| {
                    (
                        t.title.clone(),
                        if t.artist.is_empty() {
                            "Local audio".to_owned()
                        } else {
                            t.artist.clone()
                        },
                    )
                });
        fit_text(
            ui.painter(),
            Pos2::new(rect.left(), rect.top() + 18.0),
            egui::Align2::LEFT_CENTER,
            &title,
            FontId::new(23.0, egui::FontFamily::Name("bold".into())),
            WHITE,
            text_width,
        );
        fit_text(
            ui.painter(),
            Pos2::new(rect.left(), rect.top() + 46.0),
            egui::Align2::LEFT_CENTER,
            &artist,
            FontId::proportional(17.0),
            Color32::from_white_alpha(140),
            text_width,
        );
        let mut x = rect.right() - 34.0;
        let more = Rect::from_min_size(Pos2::new(x, rect.center().y - 17.0), Vec2::splat(34.0));
        if self.current.is_some() {
            let response = circle_glyph(ui, more, Icon::More, false, "More");
            self.more_menu(ui, &response);
        }
        if can_like {
            x -= 42.0;
            let heart = Rect::from_min_size(Pos2::new(x, more.top()), Vec2::splat(34.0));
            let liked = self.like_status() == Some(LikeStatus::Like);
            if circle_glyph(
                ui,
                heart,
                if liked { Icon::HeartFill } else { Icon::Heart },
                liked,
                if liked { "Remove from liked" } else { "Like" },
            )
            .clicked()
            {
                self.rate(if liked {
                    LikeStatus::Indifferent
                } else {
                    LikeStatus::Like
                });
            }
        }
    }

    /// The current lyric on one line above the scrubber, or the line saying why there isn't
    /// one; a click opens the lyrics (upstream `CurrentLyricStrip`).
    fn lyric_strip(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let Some(key) = self.current.as_ref().map(|t| t.key().to_owned()) else {
            return;
        };
        let position = self.status.position;
        let (text, intro) = if !self.lyrics.is_empty() {
            match self.lyrics.iter().rposition(|l| position >= l.time) {
                Some(i) if !self.lyrics[i].text.trim().is_empty() => {
                    (self.lyrics[i].text.clone(), false)
                }
                Some(_) => return,
                None => (phrase(&INTRO_LINES, &key).to_owned(), true),
            }
        } else if self.library.lyrics_enabled && self.lyrics_message.starts_with("Finding") {
            (phrase(&LOADING_LINES, &key).to_owned(), true)
        } else {
            return;
        };
        let response = ui
            .interact(
                rect,
                ui.scope_id().with("lyric-strip"),
                egui::Sense::click(),
            )
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Open lyrics");
        let color = if intro {
            Color32::from_white_alpha(150)
        } else {
            WHITE
        };
        let mut left = rect.left();
        if intro {
            widgets::paint(
                ui.painter(),
                Icon::Music,
                Rect::from_min_size(
                    Pos2::new(left + 2.0, rect.center().y - 9.0),
                    Vec2::splat(18.0),
                ),
                color,
            );
            left += 28.0;
        }
        let drawn = fit_text(
            ui.painter(),
            Pos2::new(left, rect.center().y),
            egui::Align2::LEFT_CENTER,
            &text,
            FontId::new(18.0, egui::FontFamily::Name("bold".into())),
            color,
            rect.right() - left - 30.0,
        );
        widgets::paint(
            ui.painter(),
            Icon::CaretRight,
            Rect::from_min_size(
                Pos2::new(drawn.right() + 8.0, rect.center().y - 7.0),
                Vec2::splat(14.0),
            ),
            Color32::from_white_alpha(if response.hovered() { 230 } else { 150 }),
        );
        if response.clicked() {
            self.tab = PlayerPane::Lyrics;
        }
    }

    fn scrubber(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let width = rect.width();
        region(ui, rect, |u| {
            u.spacing_mut().item_spacing.y = 0.0;
            u.add_enabled_ui(!self.loading && self.player.is_active(), |u| {
                self.seekbar(u, width, "sheet-seek");
            });
        });
        let small = FontId::new(13.0, egui::FontFamily::Name("bold".into()));
        let y = rect.top() + 34.0;
        let d = self.duration();
        let elapsed = if self.loading {
            "Opening stream…".to_owned()
        } else {
            clock(self.status.position)
        };
        let remaining = if self.loading || d <= 0.0 {
            clock(d)
        } else {
            format!("-{}", clock((d - self.status.position).max(0.0)))
        };
        let color = Color32::from_white_alpha(150);
        ui.painter().text(
            Pos2::new(rect.left(), y),
            egui::Align2::LEFT_CENTER,
            elapsed,
            small.clone(),
            color,
        );
        ui.painter().text(
            Pos2::new(rect.right(), y),
            egui::Align2::RIGHT_CENTER,
            remaining,
            small,
            color,
        );
    }

    /// Back, play/pause and next, spaced around the column (upstream `TransportRow`).
    fn transport(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let playing = !self.status.paused && !self.status.ended;
        let slot = rect.width() / 3.0;
        let center = |i: f32| Pos2::new(rect.left() + slot * (i + 0.5), rect.center().y);
        let has_previous = !self.previous.is_empty() || self.status.position > 3.0;
        let has_next = !self.library.queue.is_empty() || self.library.autoplay;
        let items = [
            (Icon::Rewind, 48.0, has_previous, "Previous · Alt+Left"),
            (
                if playing { Icon::Pause } else { Icon::Play },
                64.0,
                !self.loading,
                if playing {
                    "Pause · Space"
                } else {
                    "Play · Space"
                },
            ),
            (Icon::FastForward, 48.0, has_next, "Next · Alt+Right"),
        ];
        for (i, (icon, size, enabled, label)) in items.into_iter().enumerate() {
            let touch = Rect::from_center_size(
                center(i as f32),
                Vec2::splat(if i == 1 { 92.0 } else { 60.0 }),
            );
            let response = ui
                .interact(
                    touch,
                    ui.scope_id().with(("transport", i)),
                    egui::Sense::click(),
                )
                .on_hover_text(label);
            response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, label));
            if i == 1 && self.loading {
                egui::Spinner::new()
                    .size(38.0)
                    .color(Color32::WHITE)
                    .paint_at(
                        ui,
                        Rect::from_center_size(touch.center(), Vec2::splat(38.0)),
                    );
                continue;
            }
            let pressed = response.is_pointer_button_down_on() && enabled;
            let size = if pressed { size * 0.92 } else { size };
            let alpha = if !enabled {
                77
            } else if response.hovered() {
                255
            } else {
                235
            };
            widgets::paint(
                ui.painter(),
                icon,
                Rect::from_center_size(touch.center(), Vec2::splat(size)),
                Color32::from_white_alpha(alpha),
            );
            if response.clicked() && enabled {
                match i {
                    0 => self.previous(),
                    1 => self.toggle(),
                    _ => self.next(false),
                }
            }
        }
    }
}

/// The translucent disc behind the like and menu glyphs (upstream `CircleGlyph`); `active`
/// brightens the disc, since the sleeve behind can be any colour.
pub(super) fn circle_glyph(
    ui: &mut egui::Ui,
    rect: Rect,
    icon: Icon,
    active: bool,
    label: &str,
) -> egui::Response {
    let response = ui
        .interact(
            rect,
            ui.scope_id().with(("circle-glyph", label)),
            egui::Sense::click(),
        )
        .on_hover_text(label);
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    let alpha = if active {
        87
    } else if response.hovered() {
        66
    } else {
        46
    };
    ui.painter().circle_filled(
        rect.center(),
        rect.width() / 2.0,
        Color32::from_white_alpha(alpha),
    );
    widgets::paint(
        ui.painter(),
        icon,
        Rect::from_center_size(rect.center(), Vec2::splat(19.0)),
        Color32::WHITE,
    );
    response
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_song_keeps_its_phrase() {
        assert_eq!(phrase(&INTRO_LINES, "abc"), phrase(&INTRO_LINES, "abc"));
        assert!(INTRO_LINES.contains(&phrase(&INTRO_LINES, "")));
    }
}
