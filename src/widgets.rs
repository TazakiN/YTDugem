use eframe::egui::{self, Color32, Pos2, Rect, Response, Sense, Vec2};
use std::sync::Arc;

#[allow(dead_code)] // New variants are wired up by later UI work.
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
    Queue,
    Lyrics,
    Volume,
    Headphones,
    User,
    Back,
    Collapse,
    Raise,
    Sidebar,
    Translate,
    Refresh,
    Close,
    Edit,
    Description,
    Trash,
    Plus,
    Check,
    Reset,
    AddToQueue,
    Music,
    Broom,
    LyricsQuote,
    More,
    Heart,
    HeartFill,
    Dislike,
    Radio,
    Moon,
    Share,
    Disc,
    Infinity,
    Globe,
    Speaker,
    Waveform,
    CaretRight,
    VolumeLow,
    DragHandle,
    Playlist,
    ListBullets,
    PlayNext,
    Rewind,
    FastForward,
    Person,
    HeadphonesFill,
    Trending,
    Smiley,
}

/// White at `alpha` opacity, as egui 0.31's `Color32::from_white_alpha` made it: the opacity is
/// linear light, so the premultiplied channels are the gamma encoding of `alpha / 255` and far
/// brighter than `alpha` itself. egui 0.37 uses `alpha` for every channel, which would draw the
/// translucent overlays here much darker.
pub fn white_alpha(alpha: u8) -> Color32 {
    let linear = f32::from(alpha) / 255.0;
    let gamma = if linear <= 0.003_130_8 {
        3294.6 * linear
    } else {
        269.025 * linear.powf(1.0 / 2.4) - 14.025
    };
    let channel = (gamma + 0.5) as u8;
    Color32::from_rgba_premultiplied(channel, channel, channel, alpha)
}

pub fn button(ui: &mut egui::Ui, icon: Icon, size: f32, selected: bool, label: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, ui.is_enabled(), label));
    if response.hovered() || selected {
        // A true circle: `rect_filled` with a corner radius above half the side drew stray arcs.
        ui.painter().circle_filled(
            rect.center(),
            size * 0.5 - 1.0,
            white_alpha(if selected { 32 } else { 16 }),
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

/// Registers the Phosphor icon fonts (MIT, https://phosphoricons.com) with egui 0.37 types.
/// Only the font bytes of the `egui-phosphor` crate are used; its egui 0.36 API is never called.
/// The regular font is also a `Proportional` fallback so glyph constants work inside text.
pub fn install_icon_fonts(fonts: &mut egui::FontDefinitions) {
    for (name, bytes) in [
        (REGULAR_FAMILY, egui_phosphor::bytes::regular::FONT),
        (FILL_FAMILY, egui_phosphor::bytes::fill::FONT),
    ] {
        fonts
            .font_data
            .insert(name.into(), Arc::new(egui::FontData::from_static(bytes)));
        fonts
            .families
            .insert(egui::FontFamily::Name(name.into()), vec![name.into()]);
    }
    fonts
        .families
        .entry(egui::FontFamily::Proportional)
        .or_default()
        .push(REGULAR_FAMILY.into());
}

const REGULAR_FAMILY: &str = "phosphor";
const FILL_FAMILY: &str = "phosphor-fill";

/// Phosphor draws its glyphs in about 88% of the em square.
const GLYPH_SCALE: f32 = 1.12;

/// The glyph and whether it comes from the fill font.
fn glyph(icon: Icon) -> (&'static str, bool) {
    use egui_phosphor::{fill, regular};
    match icon {
        Icon::Logo => (regular::MUSIC_NOTES, false),
        Icon::Home => (regular::HOUSE, false),
        Icon::Compass => (regular::COMPASS, false),
        Icon::Library => (regular::BOOKS, false),
        Icon::Search => (regular::MAGNIFYING_GLASS, false),
        Icon::History => (regular::CLOCK_COUNTER_CLOCKWISE, false),
        Icon::Download => (regular::DOWNLOAD_SIMPLE, false),
        Icon::Settings => (regular::GEAR, false),
        Icon::Play => (fill::PLAY, true),
        Icon::Pause => (fill::PAUSE, true),
        Icon::Next => (fill::SKIP_FORWARD, true),
        Icon::Previous => (fill::SKIP_BACK, true),
        Icon::Shuffle => (regular::SHUFFLE, false),
        Icon::Repeat => (regular::REPEAT, false),
        Icon::Queue => (regular::QUEUE, false),
        Icon::Lyrics => (regular::MICROPHONE_STAGE, false),
        Icon::Volume => (regular::SPEAKER_HIGH, false),
        Icon::Headphones => (regular::HEADPHONES, false),
        Icon::User => (regular::USER, false),
        Icon::Back => (regular::CARET_LEFT, false),
        Icon::Collapse => (regular::CARET_DOWN, false),
        Icon::Raise => (regular::CARET_UP, false),
        Icon::Sidebar => (regular::SIDEBAR, false),
        Icon::Translate => (regular::TRANSLATE, false),
        Icon::Refresh => (regular::ARROWS_CLOCKWISE, false),
        Icon::Close => (regular::X, false),
        Icon::Edit => (regular::PENCIL_SIMPLE, false),
        Icon::Description => (regular::NOTE_PENCIL, false),
        Icon::Trash => (regular::TRASH, false),
        Icon::Plus => (regular::PLUS, false),
        Icon::Check => (regular::CHECK, false),
        Icon::Reset => (regular::ARROW_COUNTER_CLOCKWISE, false),
        Icon::AddToQueue => (regular::LIST_PLUS, false),
        Icon::Music => (regular::MUSIC_NOTES, false),
        Icon::Broom => (regular::BROOM, false),
        Icon::LyricsQuote => (regular::CHAT_TEARDROP_TEXT, false),
        Icon::More => (regular::DOTS_THREE, false),
        Icon::Heart => (regular::HEART, false),
        Icon::HeartFill => (fill::HEART, true),
        Icon::Dislike => (regular::THUMBS_DOWN, false),
        Icon::Radio => (regular::RADIO, false),
        Icon::Moon => (fill::MOON, true),
        Icon::Share => (regular::SHARE_NETWORK, false),
        Icon::Disc => (regular::VINYL_RECORD, false),
        Icon::Infinity => (regular::INFINITY, false),
        Icon::Globe => (regular::GLOBE, false),
        Icon::Speaker => (regular::SPEAKER_HIFI, false),
        Icon::Waveform => (regular::WAVEFORM, false),
        Icon::CaretRight => (regular::CARET_RIGHT, false),
        Icon::VolumeLow => (regular::SPEAKER_LOW, false),
        Icon::DragHandle => (regular::EQUALS, false),
        Icon::Playlist => (regular::PLAYLIST, false),
        Icon::ListBullets => (regular::LIST_BULLETS, false),
        Icon::PlayNext => (regular::ARROW_BEND_DOWN_RIGHT, false),
        Icon::Rewind => (fill::REWIND, true),
        Icon::FastForward => (fill::FAST_FORWARD, true),
        Icon::Person => (fill::USER, true),
        Icon::HeadphonesFill => (fill::HEADPHONES, true),
        Icon::Trending => (regular::TREND_UP, false),
        Icon::Smiley => (regular::SMILEY, false),
    }
}

fn family(fill: bool) -> egui::FontFamily {
    egui::FontFamily::Name(if fill { FILL_FAMILY } else { REGULAR_FAMILY }.into())
}

pub fn paint(p: &egui::Painter, icon: Icon, r: Rect, color: Color32) {
    let (text, fill) = glyph(icon);
    p.text(
        r.center(),
        egui::Align2::CENTER_CENTER,
        text,
        egui::FontId::new(r.width().min(r.height()) * GLYPH_SCALE, family(fill)),
        color,
    );
}

pub fn slider(
    ui: &mut egui::Ui,
    id: impl std::hash::Hash + std::fmt::Debug,
    value: &mut f64,
    maximum: f64,
    width: f32,
    label: &str,
) -> Response {
    let (rect, _) = ui.allocate_exact_size(Vec2::new(width.max(24.0), 24.0), Sense::hover());
    let response = ui.interact(rect, ui.make_persistent_id(id), Sense::click_and_drag());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Slider, ui.is_enabled(), label));
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
    // Thicker while held, as upstream `ThinSlider` (6 dp idle, 10 dp active), with round ends.
    let active = response.hovered() || response.dragged();
    let thickness = ui.ctx().animate_value_with_time(
        response.id.with("thickness"),
        if active { 8.0 } else { 6.0 },
        0.12,
    );
    let bar = |to: Pos2| {
        Rect::from_min_max(
            Pos2::new(a.x - thickness / 2.0, a.y - thickness / 2.0),
            Pos2::new(to.x + thickness / 2.0, to.y + thickness / 2.0),
        )
    };
    let radius = thickness / 2.0;
    ui.painter().rect_filled(bar(b), radius, white_alpha(50));
    ui.painter()
        .rect_filled(bar(knob), radius, Color32::from_gray(235));
    ui.painter().circle_filled(
        knob,
        if active { thickness * 0.85 } else { radius },
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

    #[test]
    fn white_overlays_keep_the_egui_031_brightness() {
        assert_eq!(white_alpha(0), Color32::TRANSPARENT);
        assert_eq!(white_alpha(255), Color32::WHITE);
        // Opacity is linear light, so the channels are far brighter than the alpha itself.
        let [r, g, b, a] = white_alpha(30).to_array();
        assert_eq!((r, g, b, a), (96, 96, 96, 30));
        assert!(white_alpha(16).r() > 16);
    }

    /// Reads a table offset from a TrueType file's directory.
    fn table(font: &[u8], tag: &[u8; 4]) -> usize {
        let count = u16::from_be_bytes([font[4], font[5]]) as usize;
        (0..count)
            .map(|i| 12 + i * 16)
            .find(|&o| &font[o..o + 4] == tag)
            .map(|o| u32::from_be_bytes(font[o + 8..o + 12].try_into().unwrap()) as usize)
            .expect("table present")
    }

    fn i16_at(font: &[u8], at: usize) -> f32 {
        f32::from(i16::from_be_bytes([font[at], font[at + 1]]))
    }

    #[test]
    fn phosphor_glyphs_are_vertically_centred() {
        for bytes in [
            egui_phosphor::bytes::regular::FONT,
            egui_phosphor::bytes::fill::FONT,
        ] {
            let head = table(bytes, b"head");
            let hhea = table(bytes, b"hhea");
            let units = f32::from(u16::from_be_bytes([bytes[head + 18], bytes[head + 19]]));
            // The row spans descent..ascent around the baseline and `CENTER_CENTER` centres
            // that row on the target, so the ink must be centred in the row too.
            let row_centre = (i16_at(bytes, hhea + 4) + i16_at(bytes, hhea + 6)) / 2.0;
            let ink_centre = (i16_at(bytes, head + 38) + i16_at(bytes, head + 42)) / 2.0;
            let off = (row_centre - ink_centre) / units * 16.0 * GLYPH_SCALE;
            assert!(off.abs() < 1.0, "glyphs sit {off}px off centre");
        }
    }

    #[test]
    fn icon_glyphs_lay_out_in_their_families() {
        let mut fonts = egui::FontDefinitions::default();
        install_icon_fonts(&mut fonts);
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            for icon in [Icon::Play, Icon::Home, Icon::Broom, Icon::Description] {
                let (text, fill) = glyph(icon);
                let galley = ui.painter().layout_no_wrap(
                    text.into(),
                    egui::FontId::new(16.0, family(fill)),
                    Color32::WHITE,
                );
                assert!(galley.rect.width() > 4.0, "missing glyph for {text:?}");
            }
        });
        output.textures_delta.clear();
    }
}
