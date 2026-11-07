//! The Now Playing sheet's lyrics pane: the lyric sheet, and under it the romanize / source / translate bar (upstream `LandscapeLyricsPane`).
use super::*;

impl YTDugem {
    pub(super) fn translation_target(&self) -> String {
        if self.library.translation_language.is_empty() {
            translate::system_language()
        } else {
            self.library.translation_language.clone()
        }
    }
    /// Translate the lyrics on screen into the chosen language, unless already done for them.
    pub(super) fn request_translation(&mut self) {
        let Some(key) = self.current.as_ref().map(|t| t.key().to_owned()) else {
            return;
        };
        let target = self.translation_target();
        let done = self
            .translation
            .as_ref()
            .is_some_and(|(k, lang, _)| *k == key && *lang == target);
        if done || self.lyrics.is_empty() || self.translating {
            return;
        }
        self.translation_request += 1;
        self.translating = true;
        let (id, lines, tx) = (
            self.translation_request,
            self.lyrics.clone(),
            self.tx.clone(),
        );
        std::thread::spawn(move || {
            let _ = tx.send(Event::Translated(
                id,
                translate::translate(&lines, &target).map_err(err),
            ));
        });
    }
    /// Romanize the lyrics on screen, unless already done for them.
    pub(super) fn request_romanization(&mut self) {
        let Some(key) = self.current.as_ref().map(|t| t.key().to_owned()) else {
            return;
        };
        let done = self.romanization.as_ref().is_some_and(|(k, _)| *k == key);
        if done || self.lyrics.is_empty() || self.romanizing {
            return;
        }
        self.romanization_request += 1;
        self.romanizing = true;
        let (id, lines, tx) = (
            self.romanization_request,
            self.lyrics.clone(),
            self.tx.clone(),
        );
        std::thread::spawn(move || {
            let _ = tx.send(Event::Romanized(
                id,
                key,
                translate::romanize(&lines).map_err(err),
            ));
        });
    }

    /// Per original line, the text drawn under it for the current mode (if any).
    fn sub_lines(&self) -> Vec<Option<String>> {
        let key = self.current.as_ref().map(Track::key);
        let lines = match self.lyrics_mode {
            LyricsMode::Original => None,
            LyricsMode::Romanized => match &self.romanization {
                Some((k, translate::Romanization::Romanized { lines, .. }))
                    if Some(k.as_str()) == key =>
                {
                    Some(lines)
                }
                _ => None,
            },
            LyricsMode::Translated => match &self.translation {
                Some((k, lang, Translation::Translated { lines, .. }))
                    if Some(k.as_str()) == key && *lang == self.translation_target() =>
                {
                    Some(lines)
                }
                _ => None,
            },
        };
        self.lyrics
            .iter()
            .enumerate()
            .map(|(i, line)| {
                let sub = lines.and_then(|l| l.get(i)).map(|l| l.text.as_str());
                sub_line(&line.text, sub).map(str::to_owned)
            })
            .collect()
    }

    /// The bar's centre text for the current mode.
    fn lyrics_status(&self) -> String {
        let key = self.current.as_ref().map(Track::key);
        let target = self.translation_target();
        let (prep, lang) = match self.lyrics_mode {
            LyricsMode::Original => (Prep::Ready, target),
            LyricsMode::Romanized => {
                let prep = match &self.romanization {
                    Some((k, translate::Romanization::Romanized { .. }))
                        if Some(k.as_str()) == key =>
                    {
                        Prep::Ready
                    }
                    Some((k, translate::Romanization::AlreadyLatin)) if Some(k.as_str()) == key => {
                        Prep::Same
                    }
                    _ if self.romanizing => Prep::Busy,
                    _ => Prep::Missing,
                };
                (prep, target)
            }
            LyricsMode::Translated => match &self.translation {
                Some((k, lang, Translation::Translated { .. }))
                    if Some(k.as_str()) == key && *lang == target =>
                {
                    (Prep::Ready, lang.clone())
                }
                Some((k, _, Translation::SameLanguage(lang))) if Some(k.as_str()) == key => {
                    (Prep::Same, lang.clone())
                }
                _ if self.translating => (Prep::Busy, target),
                _ => (Prep::Missing, target),
            },
        };
        status_text(
            self.lyrics_mode,
            &self.lyrics_provider,
            &self.lyrics_message,
            prep,
            &language_name(&lang),
        )
    }

    /// The list of lyric sources, opened from the status text; picking one loads the lyrics from it.
    fn lyrics_source_picker(&mut self, ui: &mut egui::Ui, anchor: &egui::Response) {
        let popup = ui.make_persistent_id("lyrics-source-popup");
        if anchor.clicked() {
            egui::Popup::toggle_id(ui.ctx(), popup);
        }
        let mut pick = None;
        popup_below_widget(
            popup,
            anchor,
            egui::PopupCloseBehavior::CloseOnClick,
            |ui| {
                ui.set_min_width(230.0);
                ui.spacing_mut().item_spacing.y = 2.0;
                ui.spacing_mut().button_padding = Vec2::new(8.0, 5.0);
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(ui, |ui| {
                        ui.set_min_width(230.0);
                        let current = self.lyrics_override.clone();
                        if ui
                            .selectable_label(current.is_none(), "Automatic (enabled order)")
                            .clicked()
                        {
                            pick = Some(None);
                        }
                        ui.separator();
                        for provider in services::lyrics::PROVIDERS {
                            let chosen = current.as_deref() == Some(provider.id);
                            if ui
                                .selectable_label(chosen, provider.label)
                                .on_hover_text(provider.detail)
                                .clicked()
                            {
                                pick = Some(Some(provider.id.to_owned()));
                            }
                        }
                    });
            },
        );
        if let Some(choice) = pick {
            self.lyrics_override = choice.clone();
            self.request_lyrics(choice);
        }
    }

    /// A 34 px disc toggle: brighter when on, a spinner in place of the glyph while `busy`.
    fn lyrics_toggle(
        ui: &mut egui::Ui,
        rect: Rect,
        icon: Icon,
        on: bool,
        busy: bool,
        label: &str,
    ) -> egui::Response {
        let response = ui
            .interact(
                rect,
                ui.scope_id().with(("lyrics-toggle", label)),
                egui::Sense::click(),
            )
            .on_hover_text(label);
        response.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, label));
        let alpha = if on {
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
        let glyph = Rect::from_center_size(rect.center(), Vec2::splat(19.0));
        if busy {
            egui::Spinner::new()
                .size(19.0)
                .color(Color32::WHITE)
                .paint_at(ui, glyph);
        } else {
            widgets::paint(ui.painter(), icon, glyph, Color32::WHITE);
        }
        response
    }

    fn lyrics_bar(&mut self, ui: &mut egui::Ui, bar: Rect) {
        const TOGGLE: f32 = 34.0;
        let has_lyrics = !self.lyrics.is_empty();
        if has_lyrics {
            let left = Rect::from_min_size(
                Pos2::new(bar.left(), bar.center().y - TOGGLE / 2.0),
                Vec2::splat(TOGGLE),
            );
            let on = self.lyrics_mode == LyricsMode::Romanized;
            let busy = on && self.romanizing;
            let label = if on {
                "Show the original lyrics"
            } else {
                "Show romanized pronunciation"
            };
            if Self::lyrics_toggle(ui, left, Icon::Globe, on, busy, label).clicked() {
                if on {
                    self.lyrics_mode = LyricsMode::Original;
                } else {
                    self.lyrics_mode = LyricsMode::Romanized;
                    self.request_romanization();
                }
            }
            let right = Rect::from_min_size(
                Pos2::new(bar.right() - TOGGLE, left.top()),
                Vec2::splat(TOGGLE),
            );
            let on = self.lyrics_mode == LyricsMode::Translated;
            let busy = on && self.translating;
            let label = if on {
                "Show the original lyrics"
            } else {
                "Translate the lyrics"
            };
            if Self::lyrics_toggle(ui, right, Icon::Translate, on, busy, label).clicked() {
                if on {
                    self.lyrics_mode = LyricsMode::Original;
                } else {
                    self.lyrics_mode = LyricsMode::Translated;
                    self.request_translation();
                }
            }
        }
        if self.current.is_none() {
            return;
        }
        let text = if has_lyrics {
            self.lyrics_status()
        } else {
            "Choose lyrics source".to_owned()
        };
        let width = (bar.width() - 2.0 * (TOGGLE + 12.0)).max(40.0);
        let id = ui.scope_id().with("lyrics-status");
        let hovered = ui.ctx().read_response(id).is_some_and(|r| r.hovered());
        let color = widgets::white_alpha(if hovered { 200 } else { 140 });
        let drawn = fit_text(
            ui.painter(),
            bar.center(),
            egui::Align2::CENTER_CENTER,
            &text,
            FontId::proportional(17.0),
            color,
            width,
        );
        let response = ui
            .interact(drawn.expand2(Vec2::new(4.0, 6.0)), id, egui::Sense::click())
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text("Choose where the lyrics come from");
        ui.painter().line_segment(
            [
                Pos2::new(drawn.left(), drawn.bottom() + 1.0),
                Pos2::new(drawn.right(), drawn.bottom() + 1.0),
            ],
            Stroke::new(1.0, color),
        );
        self.lyrics_source_picker(ui, &response);
    }

    pub(super) fn lyrics_pane(&mut self, ui: &mut egui::Ui, rect: Rect) {
        const BAR: f32 = 44.0;
        let bar = Rect::from_min_max(Pos2::new(rect.left(), rect.bottom() - BAR), rect.max);
        let sheet = Rect::from_min_max(rect.min, Pos2::new(rect.right(), bar.top() - 6.0));
        region(ui, sheet, |panel| {
            soft_controls(panel);
            if self.lyrics.is_empty() {
                panel.add_space((sheet.height() * 0.3).max(0.0));
                panel.vertical_centered(|ui| {
                    let waiting = self.lyrics_message.starts_with("Finding");
                    if waiting {
                        ui.add(egui::Spinner::new().size(22.0).color(MUTED));
                        ui.add_space(10.0);
                    }
                    let message = ui.label(
                        RichText::new(if self.current.is_none() {
                            "Play a song to see its lyrics"
                        } else if self.lyrics_message.is_empty() {
                            "Lyrics not available"
                        } else {
                            &self.lyrics_message
                        })
                        .color(MUTED)
                        .size(17.0),
                    );
                    if !waiting && !self.lyrics_detail.is_empty() {
                        message.on_hover_text(self.lyrics_detail.replace("; ", "\n"));
                    }
                    if !waiting && self.current.is_some() && self.library.lyrics_enabled {
                        ui.add_space(10.0);
                        if ui.button("Try again").clicked() {
                            self.request_lyrics(self.lyrics_override.clone());
                        }
                    }
                });
                return;
            }
            let ctx = panel.ctx().clone();
            let position = self.playhead(&ctx);
            let lines = self.lyrics.clone();
            let (subs, reveal) = self.sub_reveal(panel.ctx());
            let mut active_idx = None;
            let mut active_gap = None;
            let mut animating = false;
            let mut seek_to = None;
            // Lines with timings follow the song: the sung one sharp, the rest falling away
            // either side. Plain lyrics have no "now" to fall away from and stay sharp.
            let synced = lines.iter().any(|l| l.time > 0.0);
            // Synced lyrics also carry a row for each long instrumental break (upstream's blank
            // gap lines); the falloff is measured in rows, so the break is the focus while it plays.
            let rows = if synced {
                lyric_rows(&lines)
            } else {
                (0..lines.len()).map(Row::Line).collect()
            };
            // The row the falloff is measured from: the latest one reached, if any.
            let focus = synced.then(|| focus_row(&rows, &lines, position)).flatten();
            let gap_id = egui::Id::unique("lyrics-gap-scrolled");
            let scrolled_gap = ctx.data(|d| d.get_temp::<usize>(gap_id));
            egui::ScrollArea::vertical()
                .id_salt("sheet-lyrics")
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(panel, |u| {
                    u.spacing_mut().item_spacing.y = 4.0;
                    // Room above the first and below the last line, so either can sit centred.
                    let margin = (sheet.height() * 0.35).max(0.0);
                    u.add_space(margin * 0.4);
                    let width = u.available_width();
                    for (ri, row) in rows.iter().enumerate() {
                        let active = focus == Some(ri);
                        let (alpha, blur) = line_falloff(synced, active, focus, ri);
                        // Eased into place as the song moves on (upstream `LYRIC_SETTLE_MS`).
                        let alpha = ctx.animate_value_with_time(
                            egui::Id::unique(("lyric-alpha", ri)),
                            alpha,
                            LYRIC_SETTLE,
                        );
                        let blur = ctx.animate_value_with_time(
                            egui::Id::unique(("lyric-blur", ri)),
                            blur,
                            LYRIC_SETTLE,
                        );
                        let i = match *row {
                            Row::Line(i) => i,
                            Row::Gap { start, end } => {
                                // The row opens as the singing stops and closes as it returns.
                                let swell = ctx.animate_value_with_time(
                                    egui::Id::unique(("lyric-gap-swell", ri)),
                                    if active { 1.0 } else { 0.0 },
                                    if active { 0.4 } else { 0.35 },
                                );
                                if swell <= 0.001 {
                                    continue;
                                }
                                let (rect, hit) = u.allocate_exact_size(
                                    Vec2::new(width, (GAP_ROW_HEIGHT + GAP_ROW_SPACING) * swell),
                                    egui::Sense::click(),
                                );
                                let fill = gap_fill(position, start, end);
                                paint_gap(u.painter(), rect, fill, alpha, swell);
                                if hit.clicked() {
                                    seek_to = Some(start);
                                }
                                if active {
                                    active_gap = Some(ri);
                                    animating = true;
                                    // While it opens its centre keeps moving, so keep centring.
                                    if self.follow_lyrics
                                        && (scrolled_gap != Some(ri) || swell < 0.999)
                                    {
                                        hit.scroll_to_me(Some(Align::Center));
                                    }
                                }
                                continue;
                            }
                        };
                        let line = &lines[i];
                        if active {
                            active_idx = Some(i);
                        }
                        let sweep = if synced {
                            line_sweep(line, position)
                        } else {
                            Sweep::DONE
                        };
                        let background = u.painter().add(egui::Shape::Noop);
                        let framed = egui::Frame::new()
                            .inner_margin(egui::Margin::symmetric(10, 6))
                            .show(u, |u| {
                                u.set_width(width - 20.0);
                                u.spacing_mut().item_spacing.y = 2.0;
                                let total = line_text(line).chars().count() as f32;
                                let chars = sweep.chars();
                                let galley = lyric_galley(u, line, chars >= total);
                                let (rect, _) =
                                    u.allocate_exact_size(galley.size(), egui::Sense::hover());
                                if chars > 0.0 && chars < total {
                                    animating = true;
                                    let lit = lyric_galley(u, line, true);
                                    paint_sweep(u.painter(), rect.min, &galley, &lit, sweep, alpha);
                                } else {
                                    paint_blurred(u.painter(), rect.min, &galley, alpha, blur);
                                }
                                if let Some(sub) = subs.get(i).and_then(Option::as_ref) {
                                    // Only the lines in front of the reader get the sparkle.
                                    let near = active_idx
                                        .or(self.active_lyric)
                                        .map_or(i < 4, |a| i.abs_diff(a) <= 1);
                                    // Not lit until its line is reached, like the line itself.
                                    let lit = !synced || position >= line.time;
                                    let alpha = if lit { alpha } else { alpha * UNSUNG_ALPHA };
                                    sub_lyric(u, sub, alpha, blur, &reveal, near, i);
                                }
                            });
                        let block = framed.response.rect;
                        let hit = u.interact(
                            block,
                            u.scope_id().with(("lyric-line", i)),
                            egui::Sense::click(),
                        );
                        if hit.hovered() {
                            u.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            u.painter().set(
                                background,
                                egui::Shape::rect_filled(block, 12.0, widgets::white_alpha(10)),
                            );
                        }
                        if hit.clicked() {
                            seek_to = Some(line.time);
                        }
                        if active && self.follow_lyrics && self.active_lyric != Some(i) {
                            hit.scroll_to_me(Some(Align::Center));
                        }
                        u.add_space(8.0);
                    }
                    u.add_space(margin);
                });
            ctx.data_mut(|d| d.insert_temp(gap_id, active_gap.unwrap_or(usize::MAX)));
            if animating && self.status_playing() {
                ctx.request_repaint();
            }
            self.active_lyric = active_idx;
            if let Some(time) = seek_to {
                let result = self.player.seek(time);
                self.report(result);
            }
        });
        self.lyrics_bar(ui, bar);
    }
}

/// Upstream `SUB_LYRIC_OPEN_MS` / `SUB_LYRIC_CLOSE_MS` and `TRANSLATION_MOTION_MS`.
const SUB_OPEN: f64 = 0.46;
const SUB_CLOSE: f64 = 0.26;
const PARTICLE_MOTION: f64 = 0.54;
const PARTICLES: usize = 18;
/// Sub-lines sit a little under the line's own opacity (upstream `SUB_LYRIC_ALPHA`).
const SUB_ALPHA: f32 = 0.85;

/// The sub-lines' open/close animation: which set is shown, since when, and the lines kept
/// while they close so the fade-out still has text to draw.
#[derive(Clone, Default)]
struct SubReveal {
    shown: Option<u64>,
    start: f64,
    closing: bool,
    lines: Arc<Vec<Option<String>>>,
}

/// How far a sub-line is shown (0..1, eased) and how far the sparkle has run, or `None` once done.
struct Reveal {
    open: f32,
    sparkle: Option<f32>,
}

impl YTDugem {
    /// The sub-lines to draw this frame and how far they are revealed. A new set (turning the
    /// mode on, switching between romanized and translated, another song) grows in under its
    /// line and fades up as upstream's `revealBelow`; turning it off folds it away.
    fn sub_reveal(&self, ctx: &egui::Context) -> (Arc<Vec<Option<String>>>, Reveal) {
        let now = ctx.input(|i| i.time);
        let subs = self.sub_lines();
        let key = subs.iter().any(Option::is_some).then(|| {
            use std::hash::{Hash, Hasher};
            let mut h = std::collections::hash_map::DefaultHasher::new();
            (
                self.lyrics_mode as u8,
                self.current.as_ref().map(Track::key),
            )
                .hash(&mut h);
            h.finish()
        });
        let id = egui::Id::unique("lyrics-sub-reveal");
        let mut state: SubReveal = ctx.data(|d| d.get_temp(id)).unwrap_or_default();
        match key {
            Some(k) if state.shown != Some(k) || state.closing => {
                state = SubReveal {
                    shown: Some(k),
                    start: now,
                    closing: false,
                    lines: Arc::new(subs),
                };
            }
            Some(_) => state.lines = Arc::new(subs),
            None if state.shown.is_some() && !state.closing => {
                state.closing = true;
                state.start = now;
            }
            None => {}
        }
        let elapsed = now - state.start;
        let reveal = if state.shown.is_none() {
            Reveal {
                open: 0.0,
                sparkle: None,
            }
        } else if state.closing {
            let t = (elapsed / SUB_CLOSE).clamp(0.0, 1.0) as f32;
            if t >= 1.0 {
                state = SubReveal::default();
            }
            Reveal {
                open: 1.0 - t * t,
                sparkle: None,
            }
        } else {
            let t = (elapsed / SUB_OPEN).clamp(0.0, 1.0) as f32;
            let s = (elapsed / PARTICLE_MOTION) as f32;
            Reveal {
                open: 1.0 - (1.0 - t).powi(3),
                sparkle: (s < 1.0).then_some(s),
            }
        };
        if state.shown.is_some() && (state.closing || elapsed < PARTICLE_MOTION) {
            ctx.request_repaint();
        }
        let lines = state.lines.clone();
        ctx.data_mut(|d| d.insert_temp(id, state));
        (lines, reveal)
    }
}

/// One romanized or translated line under its lyric: it grows in from the top edge, slides
/// down into place and fades up; while it arrives a few motes drift off its letters
/// (upstream `revealBelow` and `lyricParticles`).
fn sub_lyric(
    ui: &mut egui::Ui,
    text: &str,
    alpha: f32,
    blur: f32,
    reveal: &Reveal,
    sparkle: bool,
    index: usize,
) {
    let open = reveal.open;
    if open <= 0.0 {
        return;
    }
    let galley = egui::WidgetText::from(heading(text, 20.0)).into_galley(
        ui,
        Some(egui::TextWrapMode::Wrap),
        ui.available_width(),
        egui::TextStyle::Body,
    );
    let size = galley.size();
    let (rect, _) = ui.allocate_exact_size(Vec2::new(size.x, size.y * open), egui::Sense::hover());
    // Room for the blur to spill past the clip it grows out of.
    let clip = rect.expand2(Vec2::new(blur * 2.0, 0.0));
    let painter = ui.painter().with_clip_rect(clip.intersect(ui.clip_rect()));
    let top = rect.top() - (1.0 - open) * 10.0;
    let text_rect = Rect::from_min_size(Pos2::new(rect.left(), top), size);
    paint_blurred(
        &painter,
        text_rect.min,
        &galley,
        SUB_ALPHA * open * alpha,
        blur,
    );

    let (Some(progress), true) = (reveal.sparkle, sparkle) else {
        return;
    };
    // The motes belong to the sub-line: drawn under its clip (upstream draws them inside the
    // revealing layer), so none rises over the lyric above while the line slides in, and at
    // the sub-line's own opacity, so a dim or unlit line does not get bright specks.
    let mut painter = painter;
    painter.multiply_opacity(SUB_ALPHA * open * alpha);
    // On real letters, as upstream anchors them on glyph boxes: a point picked anywhere in the
    // text's bounding box lands in the empty end of a wrapped row or between words.
    let letters: Vec<Pos2> = galley
        .rows
        .iter()
        .flat_map(|placed| {
            placed
                .row
                .glyphs
                .iter()
                .filter(|g| g.chr.is_alphanumeric())
                .map(move |g| {
                    text_rect.min
                        + placed.pos.to_vec2()
                        + Vec2::new(g.pos.x + g.advance_width / 2.0, placed.row.size.y * 0.55)
                })
        })
        .collect();
    if letters.is_empty() {
        return;
    }
    let mut seed = text
        .bytes()
        .fold(index as u32 ^ 0x9e37_79b9, |h, b| {
            h.wrapping_mul(16_777_619) ^ u32::from(b)
        })
        .max(1);
    let mut random = move || {
        // xorshift: fixed per line, so the motes do not jump between frames.
        seed ^= seed << 13;
        seed ^= seed >> 17;
        seed ^= seed << 5;
        (seed % 10_000) as f32 / 10_000.0
    };
    for _ in 0..PARTICLES {
        let anchor = letters[((random() * letters.len() as f32) as usize).min(letters.len() - 1)];
        let drift = Vec2::new((random() - 0.5) * 12.0, -(5.0 + random() * 11.0));
        let radius = 0.65 + random() * 0.65;
        let delay = 0.16 * random();
        let t = ((progress - delay) / 0.84).clamp(0.0, 1.0);
        let envelope = (std::f32::consts::PI * t).sin();
        if envelope <= 0.0 {
            continue;
        }
        let ease = 1.0 - (1.0 - t) * (1.0 - t);
        let center = anchor + Vec2::new(drift.x * ease, drift.y * ease + 3.0 * t * t);
        painter.circle_filled(
            center,
            radius * 2.7,
            Color32::WHITE.gamma_multiply(envelope * 0.07),
        );
        painter.circle_filled(
            center,
            radius,
            Color32::WHITE.gamma_multiply(envelope * 0.58),
        );
    }
}

/// How the stack falls away either side of the sung line, by distance from it (upstream
/// `LINE_FALLOFF_ALPHA` / `LINE_FALLOFF_BLUR`; the blur in points, scaled up from upstream's
/// dp because egui's blur below is a sampled approximation that reads softer).
const FALLOFF_ALPHA: [f32; 5] = [1.0, 0.8, 0.7, 0.58, 0.46];
const FALLOFF_BLUR: [f32; 5] = [0.0, 1.2, 1.2, 2.0, 2.85];
/// Upstream `LYRIC_SETTLE_MS`.
const LYRIC_SETTLE: f32 = 0.4;
/// How bright words not sung yet are: a line lights up only as the song reaches it.
const UNSUNG_ALPHA: f32 = 0.45;
/// Upstream `WIPE_FEATHER` (30 dp), scaled for our slightly smaller 30 pt font.
const WIPE_FEATHER: f32 = 26.0;
/// Slices the feathered edge of the sweep is cut into.
const FEATHER_STEPS: usize = 10;
/// Upstream `MIN_GAP_MS`: shorter silences are not worth a gap row.
const MIN_GAP: f64 = 4.0;
/// Upstream `GAP_ROW_HEIGHT` / `GAP_ROW_SPACING` / `GAP_DOT_SIZE` / `GAP_DOT_GAP` /
/// `GAP_DOT_REST` / `GAP_REST_SCALE`.
const GAP_ROW_HEIGHT: f32 = 40.0;
const GAP_ROW_SPACING: f32 = 16.0;
const GAP_DOTS: usize = 3;
const GAP_DOT_SIZE: f32 = 13.0;
const GAP_DOT_GAP: f32 = 5.0;
const GAP_DOT_REST: f32 = 0.25;
const GAP_REST_SCALE: f32 = 0.76;
/// The status position updates about ten times a second; the clock runs on between readings
/// but never further than this ahead of the last one.
const PLAYHEAD_LEAD: f64 = 0.25;

/// One row of the lyric list: a line, or an instrumental break (upstream's blank gap line).
#[derive(Clone, Copy, Debug, PartialEq)]
enum Row {
    Line(usize),
    Gap { start: f64, end: f64 },
}

/// The list's rows for synced lyrics. As upstream, the run-up before the first line and any
/// silence of `MIN_GAP` or more after a line whose singing end is known (it has word timings)
/// get a gap row; a line-synced source cannot tell a slow line from a break, so gets none.
fn lyric_rows(lines: &[LyricLine]) -> Vec<Row> {
    let mut rows = Vec::with_capacity(lines.len() + 2);
    if let Some(first) = lines.first().filter(|l| l.time >= MIN_GAP) {
        rows.push(Row::Gap {
            start: 0.0,
            end: first.time,
        });
    }
    for (i, line) in lines.iter().enumerate() {
        rows.push(Row::Line(i));
        let (Some(next), Some(last)) = (lines.get(i + 1), line.words.last()) else {
            continue;
        };
        if last.end > line.time && next.time - last.end >= MIN_GAP {
            rows.push(Row::Gap {
                start: last.end,
                end: next.time,
            });
        }
    }
    rows
}

/// The row the song has reached: the latest whose start has passed. Before anything is
/// reached (an intro too short for a gap row) it is the first row, so the stack already falls
/// away from where the singing will start instead of sitting level and sharp.
fn focus_row(rows: &[Row], lines: &[LyricLine], position: f64) -> Option<usize> {
    rows.iter()
        .rposition(|row| {
            let start = match *row {
                Row::Line(i) => lines[i].time,
                Row::Gap { start, .. } => start,
            };
            position >= start
        })
        .or((!rows.is_empty()).then_some(0))
}

/// How far through a break the song is, 0..1.
fn gap_fill(position: f64, start: f64, end: f64) -> f32 {
    (((position - start) / (end - start).max(0.001)) as f32).clamp(0.0, 1.0)
}

/// How lit dot `dot` is when the break is `through` done: each owns a third of it, so they
/// light left to right and the last finishes as the next line starts.
fn dot_lit(through: f32, dot: usize) -> f32 {
    (through * GAP_DOTS as f32 - dot as f32).clamp(0.0, 1.0)
}

/// The gap row: three big dots in turn filling from dim to white, left-aligned with the text.
fn paint_gap(painter: &egui::Painter, rect: Rect, through: f32, alpha: f32, swell: f32) {
    let painter = painter.with_clip_rect(rect.intersect(painter.clip_rect()));
    let grow = GAP_REST_SCALE + (1.0 - GAP_REST_SCALE) * swell;
    let radius = GAP_DOT_SIZE / 2.0 * grow;
    let stride = (GAP_DOT_SIZE + GAP_DOT_GAP) * grow;
    for dot in 0..GAP_DOTS {
        let lit = GAP_DOT_REST + (1.0 - GAP_DOT_REST) * dot_lit(through, dot);
        painter.circle_filled(
            Pos2::new(
                rect.left() + 10.0 + GAP_DOT_SIZE / 2.0 * grow + dot as f32 * stride,
                rect.center().y,
            ),
            radius,
            Color32::WHITE.gamma_multiply(lit * alpha * swell),
        );
    }
}

/// The song's position now: the last reading plus the time since it arrived while playing,
/// so the sweep moves every frame between the ~10 Hz status updates.
fn extrapolate(reading: f64, since: f64, playing: bool) -> f64 {
    if playing {
        reading + since.clamp(0.0, PLAYHEAD_LEAD)
    } else {
        reading
    }
}

impl YTDugem {
    fn status_playing(&self) -> bool {
        self.player.is_active() && !self.status.paused && !self.status.ended
    }
    fn playhead(&self, ctx: &egui::Context) -> f64 {
        let id = egui::Id::unique("lyrics-playhead");
        let now = ctx.input(|i| i.time);
        let reading = self.status.position;
        let playing = self.status_playing();
        let (last, mut at) = ctx
            .data(|d| d.get_temp::<(f64, f64)>(id))
            .unwrap_or((reading, now));
        if last != reading || !playing {
            at = now;
        }
        ctx.data_mut(|d| d.insert_temp(id, (reading, at)));
        extrapolate(reading, now - at, playing)
    }
}

/// A row's opacity and blur radius. Plain lyrics and the sung row are sharp; synced rows
/// fade and blur with their distance from `focus`, also while the list is scrolled by hand.
fn line_falloff(synced: bool, active: bool, focus: Option<usize>, index: usize) -> (f32, f32) {
    if !synced {
        return (0.95, 0.0);
    }
    if active {
        return (1.0, 0.0);
    }
    // Before the first row (a short intro has no gap row) there is nothing to measure from.
    let step = focus.map_or(0, |f| f.abs_diff(index).min(FALLOFF_ALPHA.len() - 1));
    (FALLOFF_ALPHA[step], FALLOFF_BLUR[step])
}

/// The text a line is drawn with: its words joined by single spaces when it has word timings.
fn line_text(line: &LyricLine) -> String {
    if line.words.is_empty() {
        line.text.clone()
    } else {
        line.words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ")
    }
}

/// How far the singing has got through a line: `from + (to - from) * t` characters. Within a
/// word the span is the word, so the edge crosses its width at an even pace; in the pause
/// between two words it is the space after the first, filling over the pause (as upstream
/// `revealedChars`). A line without word timings lights whole when it is reached.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Sweep {
    from: usize,
    to: usize,
    t: f32,
}

impl Sweep {
    const DONE: Sweep = Sweep {
        from: usize::MAX / 2,
        to: usize::MAX / 2,
        t: 1.0,
    };
    fn chars(self) -> f32 {
        self.from as f32 + (self.to - self.from) as f32 * self.t
    }
}

fn line_sweep(line: &LyricLine, position: f64) -> Sweep {
    let total = line_text(line).chars().count();
    let at = |n: usize, t: f32| Sweep { from: n, to: n, t };
    if line.words.is_empty() {
        return if position >= line.time {
            at(total, 1.0)
        } else {
            at(0, 0.0)
        };
    }
    let mut offset = 0;
    for (k, word) in line.words.iter().enumerate() {
        let end = offset + word.text.chars().count();
        if position < word.start {
            return at(offset, 0.0);
        }
        if position < word.end {
            let t = ((position - word.start) / (word.end - word.start).max(0.001)) as f32;
            return Sweep {
                from: offset,
                to: end,
                t,
            };
        }
        if let Some(next) = line.words.get(k + 1).filter(|n| position < n.start) {
            let t = ((position - word.end) / (next.start - word.end).max(0.001)) as f32;
            return Sweep {
                from: end,
                to: end + 1,
                t,
            };
        }
        offset = end + 1;
    }
    at(total, 1.0)
}

/// Where character offset `chars` sits across a row whose glyphs are `(x, advance)` pairs.
fn row_x(glyphs: &[(f32, f32)], chars: f32) -> f32 {
    let Some(&(last_x, last_w)) = glyphs.last() else {
        return 0.0;
    };
    if chars <= 0.0 {
        return glyphs[0].0;
    }
    let k = chars.floor() as usize;
    match glyphs.get(k) {
        Some(&(x, w)) => x + (chars - k as f32) * w,
        None => last_x + last_w,
    }
}

/// The lit copy of a line over its unlit one, revealed up to `sweep` with a feathered leading
/// edge: per row, everything left of the edge is lit, and the last `WIPE_FEATHER` of it fades
/// out in slices (upstream `sweepTo`). Rows wrap, so each is cut on its own.
fn paint_sweep(
    painter: &egui::Painter,
    origin: Pos2,
    dim: &Arc<egui::Galley>,
    lit: &Arc<egui::Galley>,
    sweep: Sweep,
    alpha: f32,
) {
    let mut base = painter.clone();
    base.multiply_opacity(alpha);
    base.galley(origin, dim.clone(), Color32::WHITE);
    let chars = sweep.chars();
    let mut start = 0usize;
    for placed in &lit.rows {
        let glyphs: Vec<(f32, f32)> = placed
            .row
            .glyphs
            .iter()
            .map(|g| (g.pos.x, g.advance_width))
            .collect();
        let (first, count) = (start as f32, glyphs.len());
        start += count;
        if chars <= first || glyphs.is_empty() {
            continue;
        }
        let top = origin.y + placed.pos.y;
        let band = |left: f32, right: f32| {
            Rect::from_min_max(
                Pos2::new(origin.x + left, top),
                Pos2::new(origin.x + right, top + placed.row.size.y),
            )
        };
        let row_left = placed.pos.x + glyphs[0].0;
        // Room for a glyph's overhang either side of the row's advance box.
        let (min, max) = (row_left - 12.0, placed.pos.x + placed.row.size.x + 12.0);
        let local = chars - first;
        let edge = if local >= count as f32 {
            None
        } else if sweep.from as f32 >= first && sweep.to as f32 <= first + count as f32 {
            let a = row_x(&glyphs, sweep.from as f32 - first);
            let b = row_x(&glyphs, sweep.to as f32 - first);
            Some(placed.pos.x + a + (b - a) * sweep.t)
        } else {
            Some(placed.pos.x + row_x(&glyphs, local))
        };
        let Some(edge) = edge else {
            let mut p = painter.with_clip_rect(band(min, max).intersect(painter.clip_rect()));
            p.multiply_opacity(alpha);
            p.galley(origin, lit.clone(), Color32::WHITE);
            continue;
        };
        let feather_from = (edge - WIPE_FEATHER).max(row_left);
        let mut solid =
            painter.with_clip_rect(band(min, feather_from).intersect(painter.clip_rect()));
        solid.multiply_opacity(alpha);
        solid.galley(origin, lit.clone(), Color32::WHITE);
        let step = (edge - feather_from) / FEATHER_STEPS as f32;
        if step <= 0.0 {
            continue;
        }
        for k in 0..FEATHER_STEPS {
            let left = feather_from + step * k as f32;
            let mut p =
                painter.with_clip_rect(band(left, left + step).intersect(painter.clip_rect()));
            p.multiply_opacity(alpha * (1.0 - (k as f32 + 0.5) / FEATHER_STEPS as f32));
            p.galley(origin, lit.clone(), Color32::WHITE);
        }
    }
}

/// A lyric line laid out in one piece, either fully lit or unlit (the sweep draws both).
fn lyric_galley(ui: &egui::Ui, line: &LyricLine, lit: bool) -> Arc<egui::Galley> {
    let font = FontId::new(30.0, egui::FontFamily::Name("bold".into()));
    let color = if lit {
        Color32::WHITE
    } else {
        Color32::WHITE.gamma_multiply(UNSUNG_ALPHA)
    };
    let mut job = egui::text::LayoutJob::default();
    job.append(&line_text(line), 0.0, egui::TextFormat::simple(font, color));
    job.wrap.max_width = ui.available_width();
    ui.painter().layout_job(job)
}

/// Paints `galley` at `alpha`, softened by `blur` points: egui has no blur filter, so the text
/// is stamped a few times on two rings around its place, each stamp faint enough that the
/// pile adds up to about the same weight as one sharp copy.
fn paint_blurred(
    painter: &egui::Painter,
    pos: Pos2,
    galley: &Arc<egui::Galley>,
    alpha: f32,
    blur: f32,
) {
    if alpha <= 0.0 {
        return;
    }
    if blur < 0.25 {
        let mut p = painter.clone();
        p.multiply_opacity(alpha);
        p.galley(pos, galley.clone(), Color32::WHITE);
        return;
    }
    let [centre, ring1, ring2] = stamp_opacities(alpha);
    let mut p = painter.clone();
    p.multiply_opacity(centre);
    p.galley(pos, galley.clone(), Color32::WHITE);
    for (opacity, ring, radius) in [(ring1, 0.0f32, blur * 0.5), (ring2, 0.5, blur)] {
        let mut p = painter.clone();
        p.multiply_opacity(opacity);
        for k in 0..BLUR_RING {
            let angle = (k as f32 + ring) * std::f32::consts::TAU / BLUR_RING as f32;
            let offset = Vec2::new(angle.cos(), angle.sin()) * radius;
            p.galley(pos + offset, galley.clone(), Color32::WHITE);
        }
    }
}

/// Stamps per blur ring.
const BLUR_RING: usize = 6;
/// Gaussian-like relative weights of the centre stamp, the inner ring and the outer ring.
const BLUR_WEIGHTS: [f32; 3] = [1.0, 0.35, 0.12];

/// Per-stamp opacities `[centre, inner ring, outer ring]` for a blurred line at `alpha`. They
/// keep the Gaussian shape of `BLUR_WEIGHTS`, scaled so the glyph core, where every stamp
/// overlaps, composites to `alpha`: blur softens the edges without adding ink.
fn stamp_opacities(alpha: f32) -> [f32; 3] {
    let alpha = alpha.clamp(0.0, 1.0);
    let core = |s: f32| {
        let miss = (1.0 - s * BLUR_WEIGHTS[0])
            * (1.0 - s * BLUR_WEIGHTS[1]).powi(BLUR_RING as i32)
            * (1.0 - s * BLUR_WEIGHTS[2]).powi(BLUR_RING as i32);
        1.0 - miss
    };
    // Core coverage grows with `s`; the centre stamp caps it at s = 1.
    let (mut lo, mut hi) = (0.0f32, 1.0f32);
    for _ in 0..16 {
        let mid = (lo + hi) / 2.0;
        if core(mid) < alpha {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    let s = (lo + hi) / 2.0;
    BLUR_WEIGHTS.map(|w| s * w)
}

/// Said when no source has lyrics for the song: most often a video or a track nobody has
/// transcribed, which is nothing to apologise for in provider error codes.
const NO_LYRICS_LINES: [&str; 8] = [
    "No lyrics for this one. Just enjoy the music",
    "This track is keeping its words to itself",
    "Couldn't find the words this time",
    "No lyrics here. Sing along anyway?",
    "The lyrics are taking a break",
    "Nothing to read, just listen",
    "This one speaks without words",
    "No words found. Hum it your way",
];

/// One of the no-lyrics lines, picked afresh each time a lookup comes back empty.
pub(super) fn no_lyrics_line() -> &'static str {
    use rand::seq::IndexedRandom;
    NO_LYRICS_LINES
        .choose(&mut rand::rng())
        .copied()
        .unwrap_or(NO_LYRICS_LINES[0])
}

/// How far along the current mode's extra text is.
#[derive(Clone, Copy, Debug, PartialEq)]
enum Prep {
    Ready,
    Busy,
    /// Nothing to add: already Latin script / already in the target language.
    Same,
    Missing,
}

/// The text for under a lyric line: skipped when empty or the same as the original.
fn sub_line<'a>(original: &str, sub: Option<&'a str>) -> Option<&'a str> {
    let sub = sub?.trim();
    (!sub.is_empty() && sub != original.trim()).then_some(sub)
}

fn language_name(tag: &str) -> String {
    translate::LANGUAGES
        .iter()
        .find(|(t, _)| *t == tag)
        .map_or_else(|| tag.to_owned(), |(_, name)| (*name).to_owned())
}

fn status_text(mode: LyricsMode, provider: &str, message: &str, prep: Prep, lang: &str) -> String {
    match (mode, prep) {
        (LyricsMode::Original, _) if !provider.is_empty() => format!("Lyrics by {provider}"),
        (LyricsMode::Original, _) if !message.is_empty() => message.to_owned(),
        (LyricsMode::Original, _) => "Lyrics not available".to_owned(),
        (LyricsMode::Romanized, Prep::Ready) => "Romanized pronunciation".to_owned(),
        (LyricsMode::Romanized, Prep::Same) => "Already in Latin script".to_owned(),
        (LyricsMode::Romanized, Prep::Busy) => "Romanizing…".to_owned(),
        (LyricsMode::Romanized, Prep::Missing) => "Romanization unavailable".to_owned(),
        (LyricsMode::Translated, Prep::Ready) => format!("Translated to {lang}"),
        (LyricsMode::Translated, Prep::Same) => format!("Already in {lang}"),
        (LyricsMode::Translated, Prep::Busy) => "Translating…".to_owned(),
        (LyricsMode::Translated, Prep::Missing) => "Translation unavailable".to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blur_stamps_keep_core_weight_and_fall_off() {
        for alpha in [0.3f32, 0.46, 0.8, 1.0] {
            let [c, r1, r2] = stamp_opacities(alpha);
            let n = BLUR_RING as i32;
            let core = 1.0 - (1.0 - c) * (1.0 - r1).powi(n) * (1.0 - r2).powi(n);
            assert!((core - alpha).abs() <= 0.01, "alpha {alpha}: core {core}");
            assert!(c > r1 && r1 > r2 && r2 > 0.0, "{c} {r1} {r2}");
            assert!(c <= 1.0);
        }
        assert!(stamp_opacities(0.0)[0] < 1e-4);
        assert!(stamp_opacities(2.0)[0] <= 1.0);
    }

    #[test]
    fn sub_line_skips_empty_and_same() {
        assert_eq!(sub_line("Hola", Some(" Hello ")), Some("Hello"));
        assert_eq!(sub_line("Hola", Some("Hola ")), None);
        assert_eq!(sub_line("Hola", Some("  ")), None);
        assert_eq!(sub_line("Hola", None), None);
    }

    #[test]
    fn status_text_per_mode() {
        let s = |m, p, l: &str| status_text(m, "LRCLIB", "Finding lyrics…", p, l);
        assert_eq!(s(LyricsMode::Original, Prep::Ready, ""), "Lyrics by LRCLIB");
        assert_eq!(
            status_text(LyricsMode::Original, "", "Finding lyrics…", Prep::Ready, ""),
            "Finding lyrics…"
        );
        assert_eq!(
            s(LyricsMode::Romanized, Prep::Same, ""),
            "Already in Latin script"
        );
        assert_eq!(
            s(LyricsMode::Romanized, Prep::Missing, ""),
            "Romanization unavailable"
        );
        assert_eq!(
            s(LyricsMode::Translated, Prep::Ready, "English"),
            "Translated to English"
        );
        assert_eq!(
            s(LyricsMode::Translated, Prep::Busy, "English"),
            "Translating…"
        );
    }

    #[test]
    fn only_synced_lyrics_fall_away_from_the_sung_line() {
        // Plain lyrics: every line sharp.
        assert_eq!(line_falloff(false, false, Some(0), 9), (0.95, 0.0));
        // The sung line is sharp; neighbours fade and blur, and far lines hit the floor.
        assert_eq!(line_falloff(true, true, Some(5), 5), (1.0, 0.0));
        assert_eq!(line_falloff(true, false, Some(5), 4), (0.8, 1.2));
        assert_eq!(line_falloff(true, false, Some(5), 6), (0.8, 1.2));
        assert_eq!(line_falloff(true, false, Some(5), 40), (0.46, 2.85));
        // With nothing to measure from (a short intro) nothing recedes.
        assert_eq!(line_falloff(true, false, None, 40), (1.0, 0.0));
    }

    #[test]
    fn no_lyrics_lines_are_short_and_plain() {
        for _ in 0..20 {
            let line = no_lyrics_line();
            assert!(NO_LYRICS_LINES.contains(&line));
            assert!(!line.contains("HTTP") && line.len() < 60);
        }
    }

    #[test]
    fn language_name_falls_back_to_tag() {
        assert_eq!(language_name("en"), "English");
        assert_eq!(language_name("xx"), "xx");
    }

    fn word(start: f64, end: f64, text: &str) -> crate::model::TimedWord {
        crate::model::TimedWord {
            start,
            end,
            text: text.into(),
        }
    }
    fn line(time: f64, words: Vec<crate::model::TimedWord>) -> LyricLine {
        let text = words
            .iter()
            .map(|w| w.text.as_str())
            .collect::<Vec<_>>()
            .join(" ");
        LyricLine { time, text, words }
    }

    #[test]
    fn long_intro_and_interludes_get_gap_rows() {
        let lines = vec![
            line(10.0, vec![word(10.0, 11.0, "a"), word(11.0, 12.0, "b")]),
            // Sung until 12 but the next line only starts at 20: an 8 s break.
            line(20.0, vec![word(20.0, 21.0, "c")]),
            // Short silence before it: no row; its own end to the next line is a break.
            line(23.0, vec![word(23.0, 24.0, "d")]),
            // Line-synced (no words): its end is unknown, so no row after it.
            LyricLine {
                time: 40.0,
                text: "e".into(),
                words: vec![],
            },
            line(60.0, vec![word(60.0, 61.0, "f")]),
        ];
        assert_eq!(
            lyric_rows(&lines),
            vec![
                Row::Gap {
                    start: 0.0,
                    end: 10.0
                },
                Row::Line(0),
                Row::Gap {
                    start: 12.0,
                    end: 20.0
                },
                Row::Line(1),
                Row::Line(2),
                Row::Gap {
                    start: 24.0,
                    end: 40.0
                },
                Row::Line(3),
                Row::Line(4),
            ]
        );
        // A short intro has no row.
        assert_eq!(lyric_rows(&[line(2.0, vec![word(2.0, 3.0, "a")])]).len(), 1);
    }

    #[test]
    fn the_gap_is_the_focus_before_the_first_line() {
        let lines = vec![
            line(10.0, vec![word(10.0, 11.0, "a")]),
            LyricLine {
                time: 20.0,
                text: "b".into(),
                words: vec![],
            },
        ];
        let rows = lyric_rows(&lines);
        assert_eq!(focus_row(&rows, &lines, 0.0), Some(0));
        assert_eq!(focus_row(&rows, &lines, 5.0), Some(0));
        assert_eq!(focus_row(&rows, &lines, 10.0), Some(1));
        // In the intro the first line is one row away and already falls away.
        assert_eq!(line_falloff(true, false, Some(0), 1), (0.8, 1.2));
        assert_eq!(line_falloff(true, true, Some(0), 0), (1.0, 0.0));
        assert_eq!(line_falloff(true, false, Some(0), 9), (0.46, 2.85));
        // A short intro has no gap row: the first line is the focus, so the rest already blur.
        let short = vec![line(2.0, vec![word(2.0, 3.0, "a")])];
        assert_eq!(focus_row(&lyric_rows(&short), &short, 1.0), Some(0));
    }

    #[test]
    fn gap_dots_light_in_turn() {
        assert_eq!(gap_fill(5.0, 10.0, 20.0), 0.0);
        assert_eq!(gap_fill(15.0, 10.0, 20.0), 0.5);
        assert_eq!(gap_fill(25.0, 10.0, 20.0), 1.0);
        assert_eq!(gap_fill(1.0, 1.0, 1.0), 0.0);
        let lit = |t: f32| [0, 1, 2].map(|d| dot_lit(t, d));
        assert_eq!(lit(0.0), [0.0, 0.0, 0.0]);
        assert_eq!(lit(0.5), [1.0, 0.5, 0.0]);
        assert_eq!(lit(1.0), [1.0, 1.0, 1.0]);
    }

    #[test]
    fn sweep_crosses_each_word_then_its_trailing_space() {
        let l = line(1.0, vec![word(1.0, 2.0, "hello"), word(3.0, 4.0, "you")]);
        assert_eq!(line_text(&l), "hello you");
        let sweep = |p| line_sweep(&l, p);
        assert_eq!(sweep(0.5).chars(), 0.0);
        assert_eq!(
            sweep(1.5),
            Sweep {
                from: 0,
                to: 5,
                t: 0.5
            }
        );
        assert_eq!(
            sweep(2.5),
            Sweep {
                from: 5,
                to: 6,
                t: 0.5
            }
        );
        assert_eq!(
            sweep(3.5),
            Sweep {
                from: 6,
                to: 9,
                t: 0.5
            }
        );
        assert_eq!(sweep(9.0).chars(), 9.0);
        // Without word timings the whole line lights when reached.
        let plain = LyricLine {
            time: 5.0,
            text: "plain".into(),
            words: vec![],
        };
        assert_eq!(line_sweep(&plain, 4.9).chars(), 0.0);
        assert_eq!(line_sweep(&plain, 5.0).chars(), 5.0);
    }

    #[test]
    fn row_x_interpolates_within_a_glyph() {
        let glyphs = [(0.0, 10.0), (10.0, 20.0)];
        assert_eq!(row_x(&glyphs, 0.0), 0.0);
        assert_eq!(row_x(&glyphs, 1.5), 20.0);
        assert_eq!(row_x(&glyphs, 2.0), 30.0);
        assert_eq!(row_x(&glyphs, 9.0), 30.0);
        assert_eq!(row_x(&[], 1.0), 0.0);
    }

    #[test]
    fn playhead_runs_on_between_readings_but_not_far() {
        assert_eq!(extrapolate(10.0, 0.1, true), 10.1);
        assert_eq!(extrapolate(10.0, 5.0, true), 10.0 + PLAYHEAD_LEAD);
        assert_eq!(extrapolate(10.0, 0.1, false), 10.0);
    }
}
