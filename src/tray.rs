//! Notification-area icon and menu, as upstream `DesktopTrayMenu`: what is playing, Play/Pause,
//! Next, Previous, Open, Settings and Quit. Clicking the icon brings the window back.
//!
//! The menu is the system's own, drawn dark to match the app, with the app's Phosphor icons
//! rasterized beside each entry. Its tooltip names the track, as upstream's does.

use eframe::egui;
use egui_phosphor::{bytes, regular};
use std::sync::mpsc::{self, Receiver};
use tray_icon::{
    menu::{Icon, IconMenuItem, Menu, MenuEvent, PredefinedMenuItem},
    MouseButton, MouseButtonState, TrayIcon, TrayIconBuilder, TrayIconEvent,
};

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum TrayAction {
    Toggle,
    Next,
    Previous,
    Open,
    Settings,
    Quit,
}

/// Icon ink on the dark menu, and the app's accent for the now-playing line.
const INK: [u8; 3] = [232, 232, 232];
const ACCENT: [u8; 3] = [178, 208, 158];
/// Menu text is capped well below Windows' limit so the menu stays a sensible width.
const MENU_CHARS: usize = 56;
/// `NOTIFYICONDATAW::szTip` holds 128 UTF-16 units, terminator included.
const TOOLTIP_CHARS: usize = 120;
/// Windows draws menu bitmaps at 16×16 whatever the display scale (muda's `to_hbitmap`).
const ICON_PX: u16 = 16;

/// What the menu last showed: the track (title, artist) and whether it plays.
type Shown = (Option<(String, String)>, bool);

pub struct Tray {
    icon: TrayIcon,
    now_playing: IconMenuItem,
    play: IconMenuItem,
    skips: [IconMenuItem; 2],
    /// Play, then Pause.
    play_icons: [Option<Icon>; 2],
    actions: Receiver<TrayAction>,
    shown: Option<Shown>,
}

impl Tray {
    /// Build the icon on the GUI thread (its hidden window lives on that thread's message loop).
    /// Clicks wake the app with a repaint; the app keeps running while in the tray (see
    /// [`hide_window`]) and restores the window itself.
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        dark_menus();
        let item = |text: &str, font: &[u8], glyph: &str, ink| {
            IconMenuItem::new(text, true, menu_icon(font, glyph, ink), None)
        };
        let now_playing = item(
            "Nothing playing",
            bytes::fill::FONT,
            regular::MUSIC_NOTES,
            ACCENT,
        );
        now_playing.set_enabled(false);
        let play_icons =
            [regular::PLAY, regular::PAUSE].map(|g| menu_icon(bytes::fill::FONT, g, INK));
        let play = IconMenuItem::new("Play", true, play_icons[0].clone(), None);
        let next = item("Next", bytes::fill::FONT, regular::SKIP_FORWARD, INK);
        let previous = item("Previous", bytes::fill::FONT, regular::SKIP_BACK, INK);
        let open = item(
            "Open YTDugem",
            bytes::regular::FONT,
            regular::APP_WINDOW,
            INK,
        );
        let settings = item("Settings", bytes::regular::FONT, regular::GEAR_SIX, INK);
        let quit = item("Quit YTDugem", bytes::regular::FONT, regular::POWER, INK);
        let menu = Menu::new();
        menu.append_items(&[
            &now_playing,
            &PredefinedMenuItem::separator(),
            &play,
            &next,
            &previous,
            &PredefinedMenuItem::separator(),
            &open,
            &settings,
            &PredefinedMenuItem::separator(),
            &quit,
        ])
        .ok()?;

        let image = image::load_from_memory(include_bytes!("../assets/icon.png"))
            .ok()?
            .resize(32, 32, image::imageops::FilterType::Triangle)
            .into_rgba8();
        let (width, height) = image.dimensions();
        let icon = tray_icon::Icon::from_rgba(image.into_raw(), width, height).ok()?;
        let tray = TrayIconBuilder::new()
            .with_tooltip("YTDugem")
            .with_icon(icon)
            .with_menu(Box::new(menu))
            .with_menu_on_left_click(false)
            .build()
            .ok()?;

        let (tx, actions) = mpsc::channel();
        let ids = [
            (now_playing.id().clone(), TrayAction::Open),
            (play.id().clone(), TrayAction::Toggle),
            (next.id().clone(), TrayAction::Next),
            (previous.id().clone(), TrayAction::Previous),
            (open.id().clone(), TrayAction::Open),
            (settings.id().clone(), TrayAction::Settings),
            (quit.id().clone(), TrayAction::Quit),
        ];
        let menu_tx = tx.clone();
        let menu_ctx = ctx.clone();
        MenuEvent::set_event_handler(Some(move |event: MenuEvent| {
            if let Some((_, action)) = ids.iter().find(|(id, _)| *id == event.id) {
                let _ = menu_tx.send(*action);
                menu_ctx.request_repaint();
            }
        }));
        let icon_ctx = ctx.clone();
        TrayIconEvent::set_event_handler(Some(move |event: TrayIconEvent| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let _ = tx.send(TrayAction::Open);
                icon_ctx.request_repaint();
            }
        }));
        Some(Self {
            icon: tray,
            now_playing,
            play,
            skips: [next, previous],
            play_icons,
            actions,
            shown: None,
        })
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        self.actions.try_iter().collect()
    }

    /// Keep the menu's track line, its Play/Pause entry and the tooltip in step with playback.
    /// With nothing loaded there is nothing to open, play or skip, so those entries grey out.
    pub fn update(&mut self, track: Option<(&str, &str)>, playing: bool) {
        let shown = (track.map(|(t, a)| (t.to_owned(), a.to_owned())), playing);
        if self.shown.as_ref() == Some(&shown) {
            return;
        }
        let line = track.map(|(title, artist)| match artist.trim() {
            "" => title.to_owned(),
            artist => format!("{title} · {artist}"),
        });
        let label = line
            .as_deref()
            .map_or("Nothing playing".into(), |l| clip(l, MENU_CHARS));
        // `&` marks a mnemonic in a Win32 menu label.
        self.now_playing.set_text(label.replace('&', "&&"));
        let tooltip = line.map_or("YTDugem".into(), |l| {
            clip(&format!("YTDugem\n{l}"), TOOLTIP_CHARS)
        });
        let _ = self.icon.set_tooltip(Some(tooltip));
        let loaded = track.is_some();
        // The player reports "not paused" before anything is loaded.
        let playing = playing && loaded;
        self.play.set_text(if playing { "Pause" } else { "Play" });
        self.play
            .set_icon(self.play_icons[usize::from(playing)].clone());
        // Windows greys a disabled entry's icon too, so the track line stays live: it opens the app.
        self.now_playing.set_enabled(loaded);
        self.play.set_enabled(loaded);
        for item in &self.skips {
            item.set_enabled(loaded);
        }
        self.shown = Some(shown);
    }
}

/// `text`, cut to `max` characters with an ellipsis when longer.
fn clip(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_owned();
    }
    let mut cut: String = text.chars().take(max - 1).collect();
    cut.truncate(cut.trim_end().len());
    cut.push('…');
    cut
}

/// Menus draw dark for this whole process, as the app itself is. `SetPreferredAppMode`
/// (`uxtheme` ordinal 135, Windows 10 1809+) is how Explorer and Windows Terminal do it; it
/// has no named export. The app draws all its other menus itself, so only the tray menu changes.
fn dark_menus() {
    use windows_sys::Win32::System::LibraryLoader::{GetProcAddress, LoadLibraryW};
    const FORCE_DARK: i32 = 2;
    let name: Vec<u16> = "uxtheme.dll\0".encode_utf16().collect();
    // SAFETY: `uxtheme` stays loaded for the process; ordinals 135 and 136 are
    // `SetPreferredAppMode(i32) -> i32` and `FlushMenuThemes()` on every build that exports them.
    unsafe {
        let module = LoadLibraryW(name.as_ptr());
        if module.is_null() {
            return;
        }
        let ordinal = |n: usize| GetProcAddress(module, n as *const u8);
        let (Some(set_mode), Some(flush)) = (ordinal(135), ordinal(136)) else {
            return;
        };
        let set_mode: unsafe extern "system" fn(i32) -> i32 = std::mem::transmute(set_mode);
        let flush: unsafe extern "system" fn() = std::mem::transmute(flush);
        set_mode(FORCE_DARK);
        flush();
    }
}

/// One Phosphor glyph as a menu icon.
fn menu_icon(font: &[u8], glyph: &str, ink: [u8; 3]) -> Option<Icon> {
    let rgba = glyph_rgba(font, glyph.chars().next()?, ink)?;
    Icon::from_rgba(rgba, ICON_PX.into(), ICON_PX.into()).ok()
}

/// `glyph` filled in `ink`, centred in the icon square with a pixel of margin, as straight RGBA.
fn glyph_rgba(font: &[u8], glyph: char, [r, g, b]: [u8; 3]) -> Option<Vec<u8>> {
    use skrifa::{
        instance::{LocationRef, Size},
        outline::DrawSettings,
        MetadataProvider,
    };
    use vello_cpu::kurbo::{Affine, BezPath, Shape};

    let font = skrifa::FontRef::new(font).ok()?;
    let id = font.charmap().map(glyph)?;
    let mut path = BezPath::new();
    font.outline_glyphs()
        .get(id)?
        .draw(
            DrawSettings::unhinted(Size::new(f32::from(ICON_PX)), LocationRef::default()),
            &mut Pen(&mut path),
        )
        .ok()?;
    // Font outlines grow upwards: fit the glyph's box into the square, flipped.
    let bounds = path.bounding_box();
    let half = f64::from(ICON_PX) / 2.0;
    let scale = (half - 1.0) * 2.0 / bounds.width().max(bounds.height());
    if !scale.is_finite() {
        return None;
    }
    let fit = Affine::translate((half, half))
        * Affine::scale_non_uniform(scale, -scale)
        * Affine::translate(-bounds.center().to_vec2());

    let mut ctx = vello_cpu::RenderContext::new(ICON_PX, ICON_PX);
    ctx.set_transform(fit);
    ctx.set_paint(vello_cpu::color::OpaqueColor::<vello_cpu::color::Srgb>::WHITE);
    ctx.fill_path(&path);
    let mut pixmap = vello_cpu::Pixmap::new(ICON_PX, ICON_PX);
    ctx.render(&mut pixmap, &mut vello_cpu::Resources::new());
    // Painted white, so alpha is the coverage.
    Some(
        pixmap
            .data_as_u8_slice()
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|&[.., a]| [r, g, b, a])
            .collect(),
    )
}

struct Pen<'a>(&'a mut vello_cpu::kurbo::BezPath);

impl skrifa::outline::OutlinePen for Pen<'_> {
    fn move_to(&mut self, x: f32, y: f32) {
        self.0.move_to((f64::from(x), f64::from(y)));
    }
    fn line_to(&mut self, x: f32, y: f32) {
        self.0.line_to((f64::from(x), f64::from(y)));
    }
    fn quad_to(&mut self, cx: f32, cy: f32, x: f32, y: f32) {
        self.0
            .quad_to((f64::from(cx), f64::from(cy)), (f64::from(x), f64::from(y)));
    }
    fn curve_to(&mut self, c0x: f32, c0y: f32, c1x: f32, c1y: f32, x: f32, y: f32) {
        self.0.curve_to(
            (f64::from(c0x), f64::from(c0y)),
            (f64::from(c1x), f64::from(c1y)),
            (f64::from(x), f64::from(y)),
        );
    }
    fn close(&mut self) {
        self.0.close_path();
    }
}

/// Send the window to the tray. A hidden window gets no frames from eframe, which would stall
/// auto-next and crossfades, so it is minimized instead and loses its taskbar button by becoming
/// a tool window (Windows parks minimized windows off-screen).
pub fn hide_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    // SAFETY: `hwnd` is this process's own top-level window, alive for the app's lifetime;
    // these calls only change its style and show state.
    unsafe {
        ShowWindow(hwnd, SW_HIDE);
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        SetWindowLongPtrW(
            hwnd,
            GWL_EXSTYLE,
            (style | WS_EX_TOOLWINDOW as isize) & !(WS_EX_APPWINDOW as isize),
        );
        ShowWindow(hwnd, SW_SHOWMINNOACTIVE);
    }
}

/// Bring the window back from the tray with its taskbar button.
pub fn restore_window(hwnd: isize) {
    use windows_sys::Win32::UI::WindowsAndMessaging::*;
    let hwnd = hwnd as windows_sys::Win32::Foundation::HWND;
    // SAFETY: as in `hide_window`.
    unsafe {
        let style = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        if style & WS_EX_TOOLWINDOW as isize != 0 {
            ShowWindow(hwnd, SW_HIDE);
            SetWindowLongPtrW(hwnd, GWL_EXSTYLE, style & !(WS_EX_TOOLWINDOW as isize));
        }
        ShowWindow(hwnd, SW_RESTORE);
        SetForegroundWindow(hwnd);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clip_keeps_short_text_and_ellipsizes_long_text() {
        assert_eq!(clip("Pulang · For Revenge", 56), "Pulang · For Revenge");
        let long = clip(&"a ".repeat(40), 10);
        assert_eq!(long, "a a a a a…");
    }

    #[test]
    fn every_menu_glyph_rasterizes_inside_the_square() {
        let glyphs = [
            (bytes::fill::FONT, regular::MUSIC_NOTES),
            (bytes::fill::FONT, regular::PLAY),
            (bytes::fill::FONT, regular::PAUSE),
            (bytes::fill::FONT, regular::SKIP_FORWARD),
            (bytes::fill::FONT, regular::SKIP_BACK),
            (bytes::regular::FONT, regular::APP_WINDOW),
            (bytes::regular::FONT, regular::GEAR_SIX),
            (bytes::regular::FONT, regular::POWER),
        ];
        for (font, glyph) in glyphs {
            let rgba = glyph_rgba(font, glyph.chars().next().unwrap(), INK).unwrap();
            assert_eq!(rgba.len(), 16 * 16 * 4);
            let covered = rgba.chunks_exact(4).filter(|px| px[3] > 128).count();
            assert!(
                (10..200).contains(&covered),
                "{glyph:?} covers {covered} px"
            );
            // Nothing is cut off at the edge.
            let edge = |i: usize| i % 16 == 0 || i % 16 == 15 || i < 16 || i >= 240;
            let alpha = rgba.chunks_exact(4).map(|px| px[3]);
            assert!(alpha.enumerate().all(|(i, a)| !edge(i) || a < 64));
        }
    }
}
