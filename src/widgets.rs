use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Stroke, Vec2};

#[derive(Clone, Copy)]
pub enum Icon {
    Logo,
    Home,
    Compass,
    Library,
    Search,
    History,
    Download,
    Settings,
    Play,
    Pause,
    Next,
    Previous,
    Shuffle,
    Repeat,
    Heart,
    Queue,
    Lyrics,
    Volume,
    Headphones,
    User,
    Back,
    Collapse,
    Raise,
    Close,
}

pub fn button(ui: &mut egui::Ui, icon: Icon, size: f32, selected: bool, label: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), label)
    });
    if response.hovered() || selected {
        // A true circle: `rect_filled` with a corner radius above half the side drew stray arcs.
        ui.painter().circle_filled(
            rect.center(),
            size * 0.5 - 1.0,
            Color32::from_white_alpha(if selected { 32 } else { 16 }),
        );
    }
    if matches!(icon, Icon::Logo) {
        egui::Image::from_bytes(
            "bytes://ytdugem-icon.png",
            include_bytes!("../assets/icon.png").as_slice(),
        )
        .paint_at(ui, rect.shrink(3.0));
        return response.on_hover_text(label);
    }
    paint(
        ui.painter(),
        icon,
        rect.shrink(size * 0.25),
        if ui.is_enabled() {
            if selected || response.hovered() {
                Color32::WHITE
            } else {
                Color32::from_gray(170)
            }
        } else {
            Color32::from_gray(72)
        },
    );
    response.on_hover_text(label)
}

pub fn paint(p: &egui::Painter, icon: Icon, r: Rect, color: Color32) {
    let point = |x: f32, y: f32| Pos2::new(r.left() + r.width() * x, r.top() + r.height() * y);
    let line = |a: (f32, f32), b: (f32, f32)| {
        p.line_segment(
            [point(a.0, a.1), point(b.0, b.1)],
            Stroke::new(1.8_f32, color),
        );
    };
    let path = |pts: &[(f32, f32)], closed: bool, fill: bool| {
        let pts = pts.iter().map(|v| point(v.0, v.1)).collect();
        p.add(if fill {
            egui::Shape::convex_polygon(pts, color, Stroke::NONE)
        } else if closed {
            egui::Shape::closed_line(pts, Stroke::new(1.8_f32, color))
        } else {
            egui::Shape::line(pts, Stroke::new(1.8_f32, color))
        });
    };
    match icon {
        Icon::Play => path(&[(0.25, 0.08), (0.9, 0.5), (0.25, 0.92)], true, true),
        Icon::Pause => {
            p.rect_filled(
                Rect::from_min_max(point(0.18, 0.08), point(0.4, 0.92)),
                2.0,
                color,
            );
            p.rect_filled(
                Rect::from_min_max(point(0.6, 0.08), point(0.82, 0.92)),
                2.0,
                color,
            );
        }
        Icon::Next | Icon::Previous => {
            let flip = matches!(icon, Icon::Previous);
            let x = |n: f32| if flip { 1.0 - n } else { n };
            path(
                &[(x(0.05), 0.15), (x(0.54), 0.5), (x(0.05), 0.85)],
                true,
                true,
            );
            path(
                &[(x(0.5), 0.15), (x(0.99), 0.5), (x(0.5), 0.85)],
                true,
                true,
            );
        }
        Icon::Home => {
            path(
                &[
                    (0.08, 0.43),
                    (0.5, 0.08),
                    (0.92, 0.43),
                    (0.86, 0.43),
                    (0.86, 0.94),
                    (0.6, 0.94),
                    (0.6, 0.6),
                    (0.4, 0.6),
                    (0.4, 0.94),
                    (0.14, 0.94),
                    (0.14, 0.43),
                ],
                true,
                true,
            );
        }
        Icon::Search => {
            p.circle_stroke(
                point(0.42, 0.4),
                r.width() * 0.29,
                Stroke::new(1.8_f32, color),
            );
            line((0.64, 0.64), (0.94, 0.94));
        }
        Icon::Compass => {
            p.circle_stroke(r.center(), r.width() * 0.43, Stroke::new(1.8_f32, color));
            path(
                &[(0.67, 0.25), (0.57, 0.6), (0.3, 0.75), (0.4, 0.4)],
                true,
                false,
            );
        }
        Icon::Library => {
            for x in [0.12, 0.32, 0.52] {
                line((x, 0.12), (x, 0.9));
            }
            line((0.69, 0.1), (0.92, 0.88));
        }
        Icon::History => {
            p.circle_stroke(r.center(), r.width() * 0.42, Stroke::new(1.8_f32, color));
            path(&[(0.5, 0.2), (0.5, 0.52), (0.72, 0.65)], false, false);
        }
        Icon::Download => {
            path(&[(0.2, 0.48), (0.5, 0.76), (0.8, 0.48)], false, false);
            line((0.5, 0.05), (0.5, 0.73));
            path(
                &[(0.14, 0.72), (0.14, 0.94), (0.86, 0.94), (0.86, 0.72)],
                false,
                false,
            );
        }
        Icon::Queue => {
            for y in [0.2, 0.5, 0.8] {
                p.circle_filled(point(0.1, y), 1.2, color);
                line((0.3, y), (0.95, y));
            }
        }
        Icon::Lyrics => {
            path(
                &[
                    (0.08, 0.1),
                    (0.92, 0.1),
                    (0.92, 0.76),
                    (0.5, 0.76),
                    (0.25, 0.97),
                    (0.25, 0.76),
                    (0.08, 0.76),
                ],
                true,
                false,
            );
            line((0.34, 0.3), (0.28, 0.55));
            line((0.65, 0.3), (0.59, 0.55));
        }
        Icon::Heart => {
            let pts = (0..40)
                .map(|i| {
                    let t = i as f32 * std::f32::consts::TAU / 40.0;
                    let x = 16.0 * t.sin().powi(3);
                    let y = 13.0 * t.cos()
                        - 5.0 * (2.0 * t).cos()
                        - 2.0 * (3.0 * t).cos()
                        - (4.0 * t).cos();
                    point(0.5 + x / 35.0, 0.43 - y / 35.0)
                })
                .collect();
            p.add(egui::Shape::closed_line(pts, Stroke::new(1.8_f32, color)));
        }
        Icon::Volume => {
            path(
                &[
                    (0.05, 0.38),
                    (0.25, 0.38),
                    (0.5, 0.16),
                    (0.5, 0.84),
                    (0.25, 0.62),
                    (0.05, 0.62),
                ],
                true,
                true,
            );
            path(
                &[(0.68, 0.28), (0.78, 0.42), (0.78, 0.58), (0.68, 0.72)],
                false,
                false,
            );
        }
        Icon::Headphones => {
            path(
                &[
                    (0.1, 0.8),
                    (0.1, 0.38),
                    (0.2, 0.16),
                    (0.4, 0.06),
                    (0.6, 0.06),
                    (0.8, 0.16),
                    (0.9, 0.38),
                    (0.9, 0.8),
                ],
                false,
                false,
            );
            p.rect_filled(
                Rect::from_min_max(point(0.08, 0.5), point(0.27, 0.9)),
                2.0,
                color,
            );
            p.rect_filled(
                Rect::from_min_max(point(0.73, 0.5), point(0.92, 0.9)),
                2.0,
                color,
            );
        }
        Icon::User => {
            p.circle_filled(point(0.5, 0.25), r.width() * 0.19, color);
            path(
                &[
                    (0.14, 0.96),
                    (0.2, 0.65),
                    (0.4, 0.55),
                    (0.6, 0.55),
                    (0.8, 0.65),
                    (0.86, 0.96),
                ],
                true,
                true,
            );
        }
        Icon::Back => path(&[(0.65, 0.13), (0.28, 0.5), (0.65, 0.87)], false, false),
        Icon::Collapse => path(&[(0.15, 0.36), (0.5, 0.68), (0.85, 0.36)], false, false),
        Icon::Raise => path(&[(0.15, 0.64), (0.5, 0.32), (0.85, 0.64)], false, false),
        Icon::Close => {
            line((0.2, 0.2), (0.8, 0.8));
            line((0.8, 0.2), (0.2, 0.8));
        }
        Icon::Repeat => {
            path(
                &[(0.15, 0.55), (0.15, 0.22), (0.82, 0.22), (0.68, 0.08)],
                false,
                false,
            );
            path(
                &[(0.85, 0.45), (0.85, 0.78), (0.18, 0.78), (0.32, 0.92)],
                false,
                false,
            );
        }
        Icon::Shuffle => {
            path(
                &[(0.06, 0.2), (0.3, 0.2), (0.68, 0.8), (0.94, 0.8)],
                false,
                false,
            );
            path(
                &[(0.06, 0.8), (0.3, 0.8), (0.68, 0.2), (0.94, 0.2)],
                false,
                false,
            );
            path(&[(0.8, 0.06), (0.96, 0.2), (0.8, 0.34)], false, false);
            path(&[(0.8, 0.66), (0.96, 0.8), (0.8, 0.94)], false, false);
        }
        Icon::Settings => {
            p.circle_stroke(r.center(), r.width() * 0.3, Stroke::new(3.0_f32, color));
            p.circle_stroke(r.center(), r.width() * 0.1, Stroke::new(1.5_f32, color));
            for i in 0..8 {
                let a = i as f32 * std::f32::consts::TAU / 8.0;
                p.line_segment(
                    [
                        r.center() + Vec2::angled(a) * r.width() * 0.3,
                        r.center() + Vec2::angled(a) * r.width() * 0.44,
                    ],
                    Stroke::new(3.0_f32, color),
                );
            }
        }
        Icon::Logo => {
            p.circle_stroke(
                r.center(),
                r.width() * 0.43,
                Stroke::new(r.width() * 0.075, color),
            );
            p.circle_stroke(
                r.center(),
                r.width() * 0.31,
                Stroke::new(r.width() * 0.025, color),
            );
            path(&[(0.4, 0.29), (0.7, 0.5), (0.4, 0.71)], true, true);
        }
    }
}

pub fn slider(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash,
    value: &mut f64,
    maximum: f64,
    width: f32,
    label: &str,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(24.0), 24.0), Sense::hover());
    let response = ui.interact(rect, ui.make_persistent_id(id), Sense::click_and_drag());
    response.widget_info(|| {
        egui::WidgetInfo::labeled(egui::WidgetType::Slider, ui.is_enabled(), label)
    });
    let max = if maximum.is_finite() {
        maximum.max(0.0)
    } else {
        0.0
    };
    if ui.is_enabled() && max > 0.0 && (response.clicked() || response.dragged()) {
        if let Some(pos) = response.interact_pointer_pos() {
            *value = ratio(pos.x, rect.left() + 4.0, rect.right() - 4.0) * max;
        }
    }
    let fraction = if max > 0.0 {
        (*value / max).clamp(0.0, 1.0) as f32
    } else {
        0.0
    };
    let a = Pos2::new(rect.left() + 4.0, rect.center().y);
    let b = Pos2::new(rect.right() - 4.0, rect.center().y);
    let knob = a.lerp(b, fraction);
    ui.painter()
        .line_segment([a, b], Stroke::new(3.5_f32, Color32::from_white_alpha(50)));
    ui.painter()
        .line_segment([a, knob], Stroke::new(3.5_f32, Color32::from_gray(235)));
    ui.painter().circle_filled(
        knob,
        if response.hovered() || response.dragged() {
            5.0
        } else {
            3.2
        },
        Color32::WHITE,
    );
    response.on_hover_text(label)
}

fn ratio(x: f32, left: f32, right: f32) -> f64 {
    if right <= left {
        0.0
    } else {
        ((x - left) / (right - left)).clamp(0.0, 1.0) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn seek_coordinates_are_clamped() {
        assert_eq!(ratio(-10.0, 0.0, 100.0), 0.0);
        assert_eq!(ratio(50.0, 0.0, 100.0), 0.5);
        assert_eq!(ratio(110.0, 0.0, 100.0), 1.0);
        assert_eq!(ratio(10.0, 5.0, 5.0), 0.0);
    }
}
