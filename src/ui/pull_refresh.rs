//! Pull to refresh on Home (a YTDugem addition, after the web's pull-to-refresh): scrolling up
//! past the top of the page, with the wheel or a touchpad, pulls an indicator down, and pulling
//! it all the way reloads Home. The shelves on screen stay until the new ones arrive.
use super::*;

/// How far past the top to pull, in points: three notches of the mouse wheel (40 points each).
pub(super) const THRESHOLD: f32 = 120.0;
/// A pause longer than this, in seconds, ends a scroll gesture.
const GESTURE_GAP: f64 = 0.25;
/// How fast a released pull springs back, per second (exponential).
const SPRING: f32 = 12.0;
/// Height of the indicator slot when fully pulled or refreshing.
const SLOT: f32 = 52.0;
const RING: f32 = 13.0;

/// One pull gesture. Only a gesture that starts with the page already at the top pulls, so
/// scrolling quickly up to the top does not run on into a refresh.
#[derive(Clone, Debug, Default)]
pub(super) struct Pull {
    distance: f32,
    last_input: Option<f64>,
    from_top: bool,
    fired: bool,
}

impl Pull {
    /// Feeds one frame. `delta` is the vertical scroll (positive moves content down, that is,
    /// scrolls up), `at_top` whether the page was at its top, `now` and `dt` in seconds.
    /// Returns true once per gesture, when the pull reaches [`THRESHOLD`].
    pub(super) fn step(&mut self, now: f64, delta: f32, at_top: bool, dt: f32) -> bool {
        let idle = self.last_input.is_none_or(|t| now - t > GESTURE_GAP);
        if delta == 0.0 {
            if idle {
                self.distance *= (1.0 - SPRING * dt).max(0.0);
                if self.distance < 0.5 {
                    self.distance = 0.0;
                }
            }
            return false;
        }
        if idle {
            self.from_top = at_top;
            self.fired = false;
        }
        self.last_input = Some(now);
        if !at_top {
            self.from_top = false;
            self.distance = 0.0;
        }
        if !self.from_top || self.fired {
            return false;
        }
        self.distance = (self.distance + delta).clamp(0.0, THRESHOLD);
        self.fired = self.distance >= THRESHOLD;
        self.fired
    }

    /// How far the pull has come, 0..=1.
    pub(super) fn progress(&self) -> f32 {
        (self.distance / THRESHOLD).clamp(0.0, 1.0)
    }

    /// Still moving: pulled, or springing back.
    pub(super) fn active(&self) -> bool {
        self.distance > 0.0
    }
}

impl YTDugem {
    /// Reads this frame's scroll before the page's scroll area takes it: at the top the page
    /// cannot use an upward scroll, and a touchpad's rubber band would otherwise consume it.
    pub(super) fn feed_home_pull(&mut self, ui: &egui::Ui, area: Rect) {
        let (delta, now, dt) = ui.input(|i| {
            let zooming = i.modifiers.command;
            (
                if zooming {
                    0.0
                } else {
                    i.smooth_scroll_delta.y
                },
                i.time,
                i.stable_dt.min(0.1),
            )
        });
        // Over the page only: not the sidebar, a menu or a dialog above it.
        let delta = if ui.rect_contains_pointer(area) {
            delta
        } else {
            0.0
        };
        let ready = self.home_stale.is_none() && self.home_pending == 0;
        if self.home_pull.step(now, delta, self.page_at_top, dt) && ready {
            self.refresh_home();
        }
        if self.home_pull.active() {
            ui.ctx().request_repaint();
        }
    }

    /// Reloads Home, keeping what is shown until the new shelves have all arrived.
    pub(super) fn refresh_home(&mut self) {
        let shown = (
            std::mem::take(&mut self.home),
            std::mem::take(&mut self.recents),
        );
        self.load_home();
        if !shown.0.is_empty() {
            self.home_stale = Some(shown);
        }
    }

    /// Once every Home request has answered, drop the old shelves, or put them back if the
    /// refresh brought nothing (the error is already in `message`).
    pub(super) fn settle_home_refresh(&mut self) {
        if self.home_pending > 0 {
            return;
        }
        if let Some((shelves, recents)) = self.home_stale.take() {
            if self.home.is_empty() {
                self.home = shelves;
                if self.recents.is_empty() {
                    self.recents = recents;
                }
            }
        }
    }

    /// The indicator above Home: a ring that fills as the page is pulled, then a spinner while
    /// the refresh runs.
    pub(super) fn home_pull_slot(&mut self, ui: &mut egui::Ui) {
        let refreshing = self.home_stale.is_some();
        let target = if refreshing {
            1.0
        } else {
            self.home_pull.progress()
        };
        let open = ui.ctx().animate_value_with_time(
            egui::Id::unique("home-pull-slot"),
            target,
            if self.home_pull.active() { 0.0 } else { 0.15 },
        );
        if open < 0.01 {
            return;
        }
        let (slot, _) = ui.allocate_exact_size(
            Vec2::new(ui.available_width(), SLOT * open),
            egui::Sense::hover(),
        );
        let center = Pos2::new(slot.center().x, slot.bottom() - SLOT / 2.0);
        let painter = ui.painter().with_clip_rect(slot.intersect(ui.clip_rect()));
        let alpha = open.clamp(0.0, 1.0);
        if refreshing {
            egui::Spinner::new()
                .size(RING * 2.0)
                .color(WHITE.gamma_multiply(alpha))
                .paint_at(ui, Rect::from_center_size(center, Vec2::splat(RING * 2.0)));
            return;
        }
        let progress = self.home_pull.progress();
        painter.circle_stroke(
            center,
            RING,
            Stroke::new(2.0, widgets::white_alpha((60.0 * alpha) as u8)),
        );
        if progress > 0.0 {
            let steps = (progress * 48.0).ceil().max(2.0) as usize;
            let arc: Vec<Pos2> = (0..=steps)
                .map(|k| {
                    let a = -std::f32::consts::FRAC_PI_2
                        + std::f32::consts::TAU * progress * k as f32 / steps as f32;
                    center + RING * Vec2::new(a.cos(), a.sin())
                })
                .collect();
            painter.add(egui::Shape::line(
                arc,
                Stroke::new(2.0, WHITE.gamma_multiply(alpha)),
            ));
        }
        let icon = Rect::from_center_size(center, Vec2::splat(RING * 1.2));
        widgets::paint(
            &painter,
            Icon::Refresh,
            icon,
            WHITE.gamma_multiply(0.4 + 0.6 * progress),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const DT: f32 = 1.0 / 60.0;

    /// Scrolls `notches` wheel notches 0.05 s apart from `start`; true if any fired.
    fn wheel(pull: &mut Pull, start: f64, notches: usize, at_top: bool) -> bool {
        (0..notches).fold(false, |fired, k| {
            pull.step(start + k as f64 * 0.05, 40.0, at_top, DT) || fired
        })
    }

    #[test]
    fn pulling_from_the_top_fires_once_at_the_threshold() {
        let mut pull = Pull::default();
        assert!(!wheel(&mut pull, 0.0, 2, true));
        assert!(pull.progress() > 0.6 && pull.progress() < 1.0);
        assert!(pull.step(0.1, 40.0, true, DT), "the third notch reaches it");
        assert!(!wheel(&mut pull, 0.15, 5, true), "once per gesture");
        assert_eq!(pull.progress(), 1.0);
    }

    #[test]
    fn scrolling_up_to_the_top_does_not_run_on_into_a_refresh() {
        let mut pull = Pull::default();
        assert!(!wheel(&mut pull, 0.0, 4, false));
        // Still the same gesture once the page reaches the top.
        assert!(!wheel(&mut pull, 0.2, 10, true));
        assert_eq!(pull.progress(), 0.0);
        // A fresh gesture after a pause does pull.
        assert!(wheel(&mut pull, 1.0, 3, true));
    }

    #[test]
    fn a_released_pull_springs_back_and_can_pull_again() {
        let mut pull = Pull::default();
        assert!(wheel(&mut pull, 0.0, 3, true));
        let mut now = 0.2;
        while pull.active() {
            now += f64::from(DT);
            pull.step(now, 0.0, true, DT);
            assert!(now < 2.0, "settles");
        }
        assert!(
            wheel(&mut pull, now + 0.5, 3, true),
            "a new gesture fires again"
        );
    }

    #[test]
    fn scrolling_down_cancels_the_pull() {
        let mut pull = Pull::default();
        wheel(&mut pull, 0.0, 2, true);
        assert!(pull.active());
        pull.step(0.1, -40.0, true, DT);
        assert!(pull.progress() < 0.5);
        pull.step(0.15, -40.0, false, DT);
        assert!(!pull.active());
        assert!(!wheel(&mut pull, 0.2, 6, true), "the gesture left the top");
    }

    #[test]
    fn holding_still_without_letting_go_keeps_the_pull() {
        let mut pull = Pull::default();
        wheel(&mut pull, 0.0, 2, true);
        let held = pull.progress();
        pull.step(0.1, 0.0, true, DT);
        assert_eq!(pull.progress(), held, "no spring back inside the gesture");
    }
}
