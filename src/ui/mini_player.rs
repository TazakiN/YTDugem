//! The mini player (a YTDugem addition): a small always-on-top window with the artwork, what is
//! playing, previous/play-pause/next and a seek line. Opening it minimizes the main window;
//! closing it (or Ctrl+M) brings the main window back.
use super::*;

const WIDTH: f32 = 360.0;
const HEIGHT: f32 = 112.0;
const PAD: f32 = 12.0;
const COVER: f32 = 64.0;

fn viewport_id() -> egui::ViewportId {
    egui::ViewportId::from_hash_of("ytdugem-mini-player")
}

/// Where the parts of the mini player sit in a window whose content starts at `origin`.
struct Layout {
    cover: Rect,
    text_left: f32,
    text_right: f32,
    controls: Rect,
    seek: Rect,
}

fn layout(origin: Pos2) -> Layout {
    let cover = Rect::from_min_size(origin + Vec2::splat(PAD), Vec2::splat(COVER));
    let text_left = cover.right() + 12.0;
    let text_right = origin.x + WIDTH - PAD;
    Layout {
        cover,
        text_left,
        text_right,
        controls: Rect::from_min_max(
            Pos2::new(text_left - 4.0, cover.bottom() - 32.0),
            Pos2::new(text_right, cover.bottom() + 2.0),
        ),
        seek: Rect::from_min_max(
            Pos2::new(origin.x + PAD, origin.y + HEIGHT - 18.0),
            Pos2::new(origin.x + WIDTH - PAD, origin.y + HEIGHT - 6.0),
        ),
    }
}

impl YTDugem {
    pub(super) fn toggle_mini_player(&mut self, ctx: &egui::Context) {
        if self.mini_player {
            self.close_mini_player(ctx);
        } else {
            self.mini_player = true;
            self.mini_minimized = false;
        }
    }

    fn close_mini_player(&mut self, ctx: &egui::Context) {
        self.mini_player = false;
        if self.mini_minimized {
            self.mini_minimized = false;
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Minimized(false),
            );
            ctx.send_viewport_cmd_to(egui::ViewportId::ROOT, egui::ViewportCommand::Focus);
        }
    }

    /// Shows the mini window while it is on; called once per frame from the main window.
    pub(super) fn mini_player_step(&mut self, ctx: &egui::Context) {
        if !self.mini_player {
            return;
        }
        let builder = egui::ViewportBuilder::default()
            .with_title("YTDugem mini player")
            .with_inner_size([WIDTH, HEIGHT])
            .with_min_inner_size([WIDTH, HEIGHT])
            .with_resizable(false)
            .with_maximize_button(false)
            .with_always_on_top();
        let (close, class) = ctx.show_viewport_immediate(viewport_id(), builder, |ui, class| {
            let close = self.mini_player_ui(ui);
            let closed = ui.input(|i| i.viewport().close_requested());
            (close || closed, class)
        });
        if close {
            self.close_mini_player(ctx);
        } else if class == egui::ViewportClass::Immediate && !self.mini_minimized {
            // Only a real second window replaces the main one; an embedded fallback lives in it.
            self.mini_minimized = true;
            ctx.send_viewport_cmd_to(
                egui::ViewportId::ROOT,
                egui::ViewportCommand::Minimized(true),
            );
        }
    }

    /// Draws the mini player; returns true when it asks to go back to the main window.
    fn mini_player_ui(&mut self, ui: &mut egui::Ui) -> bool {
        let (close_key, space, prev, next) = ui.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::CTRL, egui::Key::M)
                    || i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Space),
                i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft),
                i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowRight),
            )
        });
        if space {
            self.toggle();
        }
        if prev {
            self.previous();
        }
        if next {
            self.next(false);
        }
        let (area, _) = ui.allocate_exact_size(Vec2::new(WIDTH, HEIGHT), egui::Sense::hover());
        ui.painter()
            .rect_filled(ui.max_rect().union(area), 0.0, SIDE);
        let l = layout(area.min);
        let text_width = (l.text_right - l.text_left).max(0.0);
        match self.current.clone() {
            Some(track) => {
                draw_cover(ui, &track, l.cover, Some(self.cover.as_ref()));
                if self.loading {
                    self.now_playing_badge(ui, l.cover);
                }
                fit_text(
                    ui.painter(),
                    Pos2::new(l.text_left, l.cover.top() + 9.0),
                    egui::Align2::LEFT_CENTER,
                    &track.title,
                    FontId::new(13.5, egui::FontFamily::Name("bold".into())),
                    WHITE,
                    text_width,
                );
                fit_text(
                    ui.painter(),
                    Pos2::new(l.text_left, l.cover.top() + 27.0),
                    egui::Align2::LEFT_CENTER,
                    &track.artist,
                    FontId::proportional(12.0),
                    MUTED,
                    text_width,
                );
            }
            None => {
                ui.painter()
                    .rect_filled(l.cover, 8.0, Color32::from_rgb(51, 53, 48));
                widgets::paint(
                    ui.painter(),
                    Icon::Logo,
                    Rect::from_center_size(l.cover.center(), Vec2::splat(26.0)),
                    Color32::from_gray(105),
                );
                fit_text(
                    ui.painter(),
                    Pos2::new(l.text_left, l.cover.top() + 9.0),
                    egui::Align2::LEFT_CENTER,
                    "Not playing",
                    FontId::proportional(13.0),
                    MUTED,
                    text_width,
                );
            }
        }
        let back = region(ui, l.controls, |ui| {
            ui.horizontal_centered(|ui| {
                ui.spacing_mut().item_spacing.x = 2.0;
                self.transport_buttons(
                    ui,
                    Transport {
                        side: 28.0,
                        skip: 30.0,
                        play: 34.0,
                        previous: Icon::Previous,
                        next: Icon::Next,
                        extras: false,
                    },
                );
                ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                    widgets::button(ui, Icon::Raise, 30.0, false, "Back to full player · Ctrl+M")
                        .clicked()
                })
                .inner
            })
            .inner
        });
        self.floating_progress(ui, l.seek, "mini-seek");
        back || close_key
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parts_fit_inside_the_window_without_overlapping() {
        let origin = Pos2::new(5.0, 7.0);
        let window = Rect::from_min_size(origin, Vec2::new(WIDTH, HEIGHT));
        let l = layout(origin);
        for r in [l.cover, l.controls, l.seek] {
            assert!(window.contains_rect(r), "{r:?}");
        }
        assert!(l.text_left > l.cover.right());
        assert!(l.controls.left() > l.cover.right());
        assert!(l.seek.top() > l.cover.bottom());
        assert!(l.text_right - l.text_left > 200.0);
    }
}
