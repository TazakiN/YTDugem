//! The Now Playing sheet's queue pane (upstream `InlineQueue`): what is playing, an AutoPlay
//! heading and the queue, with rows that play on click, drop on the X and reorder by their handle.
use super::*;

const ROW: f32 = 72.0;
const THUMB: f32 = 56.0;

/// Where a row dragged to height `y` would be inserted, given the centres of all the rows:
/// 0 is before the first, `centers.len()` after the last.
fn drop_slot(centers: &[f32], y: f32) -> usize {
    centers.iter().filter(|c| **c < y).count()
}

/// Moves `items[from]` so it sits before what was at `slot` (as `drop_slot` counts it).
fn move_to_slot<T>(items: &mut Vec<T>, from: usize, slot: usize) {
    if from >= items.len() {
        return;
    }
    let to = if slot > from { slot - 1 } else { slot }.min(items.len() - 1);
    if to != from {
        let item = items.remove(from);
        items.insert(to, item);
    }
}

/// What happened to one queue row this frame.
struct RowEvents {
    rect: Rect,
    clicked: bool,
    remove: bool,
    dragging: bool,
    drag_stopped: bool,
}

fn bold(size: f32) -> FontId {
    FontId::new(size, egui::FontFamily::Name("bold".into()))
}

impl YTDugem {
    pub(super) fn queue_pane(&mut self, ui: &mut egui::Ui, rect: Rect) {
        fit_text(
            ui.painter(),
            Pos2::new(rect.left(), rect.top() + 18.0),
            egui::Align2::LEFT_CENTER,
            "Queue",
            bold(24.0),
            WHITE,
            rect.width(),
        );
        let list = Rect::from_min_max(Pos2::new(rect.left(), rect.top() + 44.0), rect.max);
        region(ui, list, |panel| {
            soft_controls(panel);
            egui::ScrollArea::vertical()
                .id_salt("sheet-queue")
                .auto_shrink([false, false])
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(panel, |u| self.queue_list(u));
        });
    }

    fn queue_list(&mut self, u: &mut egui::Ui) {
        let width = u.available_width();
        u.spacing_mut().item_spacing.y = 0.0;
        if let Some(t) = self.current.clone() {
            section_label(u, "Now playing");
            let events = self.queue_row(u, &t, None, width);
            if events.clicked {
                self.toggle();
            } else if events.remove {
                self.next(false);
            }
        }
        // As upstream, the AutoPlay heading sits between what is playing and the list.
        if self.library.autoplay {
            u.add_space(10.0);
            let (r, _) = u.allocate_exact_size(Vec2::new(width, 56.0), egui::Sense::hover());
            widgets::paint(
                u.painter(),
                Icon::Infinity,
                Rect::from_center_size(Pos2::new(r.left() + 20.0, r.center().y), Vec2::splat(22.0)),
                widgets::white_alpha(190),
            );
            fit_text(
                u.painter(),
                Pos2::new(r.left() + 42.0, r.center().y - 10.0),
                egui::Align2::LEFT_CENTER,
                "AutoPlay",
                bold(16.0),
                WHITE,
                width - 42.0,
            );
            fit_text(
                u.painter(),
                Pos2::new(r.left() + 42.0, r.center().y + 11.0),
                egui::Align2::LEFT_CENTER,
                "Similar music, selected to play next",
                FontId::proportional(13.0),
                MUTED,
                width - 42.0,
            );
        }
        if !self.library.queue.is_empty() {
            u.add_space(10.0);
            let clear = section_label(u, "Next in queue");
            if clear {
                self.library.queue.clear();
                self.save();
            }
        }
        let mut play = None;
        let mut remove = None;
        let mut centers = Vec::new();
        let mut rects = Vec::new();
        let mut dragging = None;
        let mut dropped = None;
        for i in 0..self.library.queue.len() {
            let t = self.library.queue[i].clone();
            u.push_id(("queue", i), |u| {
                let e = self.queue_row(u, &t, Some(i), width);
                centers.push(e.rect.center().y);
                rects.push(e.rect);
                if e.clicked {
                    play = Some(i);
                } else if e.remove {
                    remove = Some(i);
                }
                if e.dragging {
                    dragging = Some(i);
                }
                if e.drag_stopped {
                    dropped = Some(i);
                }
            });
        }
        // Reorder: an insertion line follows the pointer while a handle is held.
        if let (Some(from), Some(pos)) = (
            dragging.or(dropped),
            u.input(|i| i.pointer.interact_pos().or(i.pointer.latest_pos())),
        ) {
            let slot = drop_slot(&centers, pos.y);
            if dragging.is_some() {
                u.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
                let y = match slot {
                    0 => rects[0].top(),
                    n => rects[n - 1].bottom(),
                };
                u.painter().line_segment(
                    [
                        Pos2::new(rects[0].left() + 8.0, y),
                        Pos2::new(rects[0].right() - 8.0, y),
                    ],
                    Stroke::new(2.0, WHITE),
                );
            } else {
                move_to_slot(&mut self.library.queue, from, slot);
                self.save();
            }
        }
        if let Some(i) = remove {
            self.library.queue.remove(i);
            self.save();
        } else if let Some(i) = play {
            let t = self.library.queue.remove(i);
            if let Some(old) = self.current.clone() {
                self.previous.push(old);
            }
            self.play(t);
            self.save();
        }

        if self.library.queue.is_empty() {
            u.add_space(20.0);
            u.vertical_centered(|u| {
                u.label(RichText::new("Your queue is empty").color(MUTED).size(15.0));
            });
        }
        u.add_space(16.0);
    }

    /// One queue row; `index` is `None` for the current track.
    fn queue_row(
        &mut self,
        ui: &mut egui::Ui,
        track: &Track,
        index: Option<usize>,
        width: f32,
    ) -> RowEvents {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, ROW), egui::Sense::click());
        let hovered = ui.rect_contains_pointer(rect);
        let current = index.is_none();
        let mut events = RowEvents {
            rect,
            clicked: false,
            remove: false,
            dragging: false,
            drag_stopped: false,
        };
        let base = ui.scope_id().with("queue-row");
        let mut handle_active = false;
        let mut left = rect.left() + 6.0;
        if index.is_some() {
            let handle = ui
                .interact(
                    Rect::from_center_size(
                        Pos2::new(rect.left() + 20.0, rect.center().y),
                        Vec2::new(28.0, 44.0),
                    ),
                    base.with("handle"),
                    egui::Sense::drag(),
                )
                .on_hover_cursor(egui::CursorIcon::Grab);
            events.dragging = handle.dragged();
            events.drag_stopped = handle.drag_stopped();
            handle_active = handle.dragged();
            widgets::paint(
                ui.painter(),
                Icon::DragHandle,
                Rect::from_center_size(
                    Pos2::new(rect.left() + 20.0, rect.center().y),
                    Vec2::splat(22.0),
                ),
                widgets::white_alpha(if handle.hovered() || handle_active {
                    230
                } else {
                    130
                }),
            );
            left = rect.left() + 38.0;
        }
        if hovered || handle_active {
            ui.painter()
                .rect_filled(rect, 14.0, widgets::white_alpha(12));
        }
        let art = Rect::from_min_size(
            Pos2::new(left, rect.center().y - THUMB / 2.0),
            Vec2::splat(THUMB),
        );
        draw_cover(ui, track, art, None);

        let close = Rect::from_center_size(
            Pos2::new(rect.right() - 22.0, rect.center().y),
            Vec2::splat(34.0),
        );
        let x = ui
            .interact(close, base.with("remove"), egui::Sense::click())
            .on_hover_text(if current { "Skip" } else { "Remove from queue" });
        widgets::paint(
            ui.painter(),
            Icon::Close,
            Rect::from_center_size(close.center(), Vec2::splat(20.0)),
            widgets::white_alpha(if x.hovered() { 255 } else { 170 }),
        );
        let mut right = close.left() - 6.0;
        if current && !self.loading && !self.status.paused && !self.status.ended {
            widgets::paint(
                ui.painter(),
                Icon::Waveform,
                Rect::from_center_size(Pos2::new(right - 12.0, rect.center().y), Vec2::splat(22.0)),
                WHITE,
            );
            right -= 32.0;
        }
        let text_x = art.right() + 14.0;
        let max = right - text_x;
        fit_text(
            ui.painter(),
            Pos2::new(text_x, rect.center().y - 11.0),
            egui::Align2::LEFT_CENTER,
            &track.title,
            bold(16.0),
            WHITE,
            max,
        );
        fit_text(
            ui.painter(),
            Pos2::new(text_x, rect.center().y + 11.0),
            egui::Align2::LEFT_CENTER,
            if track.artist.is_empty() {
                "Local audio"
            } else {
                &track.artist
            },
            FontId::proportional(14.0),
            MUTED,
            max,
        );
        response.context_menu(|ui| self.track_menu(ui, track));
        events.remove = x.clicked();
        events.clicked = response.clicked() && !events.remove;
        events
    }
}

/// A small muted heading; the result is whether a trailing "Clear" was clicked (only drawn
/// for "Next in queue", as upstream).
fn section_label(ui: &mut egui::Ui, text: &str) -> bool {
    let (r, _) =
        ui.allocate_exact_size(Vec2::new(ui.available_width(), 30.0), egui::Sense::hover());
    fit_text(
        ui.painter(),
        Pos2::new(r.left(), r.center().y),
        egui::Align2::LEFT_CENTER,
        text,
        bold(14.0),
        widgets::white_alpha(190),
        r.width() - 70.0,
    );
    if text != "Next in queue" {
        return false;
    }
    let clear = Rect::from_center_size(
        Pos2::new(r.right() - 28.0, r.center().y),
        Vec2::new(56.0, 26.0),
    );
    let response = ui
        .interact(
            clear,
            ui.scope_id().with("queue-clear"),
            egui::Sense::click(),
        )
        .on_hover_text("Clear queue");
    ui.painter().text(
        clear.right_center(),
        egui::Align2::RIGHT_CENTER,
        "Clear",
        bold(14.0),
        widgets::white_alpha(if response.hovered() { 255 } else { 190 }),
    );
    response.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slot_counts_rows_above_the_pointer() {
        let centers = [10.0, 30.0, 50.0];
        assert_eq!(drop_slot(&centers, 0.0), 0);
        assert_eq!(drop_slot(&centers, 40.0), 2);
        assert_eq!(drop_slot(&centers, 99.0), 3);
        assert_eq!(drop_slot(&[], 5.0), 0);
    }

    #[test]
    fn rows_move_to_their_slot() {
        let mut v = vec![0, 1, 2, 3];
        move_to_slot(&mut v, 0, 3);
        assert_eq!(v, [1, 2, 0, 3]);
        let mut v = vec![0, 1, 2, 3];
        move_to_slot(&mut v, 3, 0);
        assert_eq!(v, [3, 0, 1, 2]);
        let mut v = vec![0, 1, 2, 3];
        move_to_slot(&mut v, 1, 1);
        move_to_slot(&mut v, 1, 2);
        move_to_slot(&mut v, 2, 4);
        assert_eq!(v, [0, 1, 3, 2]);
        move_to_slot(&mut v, 9, 0);
        assert_eq!(v, [0, 1, 3, 2]);
    }
}
