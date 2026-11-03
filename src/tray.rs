//! Notification-area icon and menu, as upstream `DesktopTrayMenu`: what is playing, Play/Pause,
//! Next, Previous, Open, Settings and Quit. Clicking the icon brings the window back.

use eframe::egui;
use std::sync::mpsc::{self, Receiver};
use tray_icon::{
    menu::{Menu, MenuEvent, MenuItem, PredefinedMenuItem},
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

pub struct Tray {
    _icon: TrayIcon,
    now_playing: MenuItem,
    play: MenuItem,
    actions: Receiver<TrayAction>,
    shown: (String, bool),
}

impl Tray {
    /// Build the icon on the GUI thread (its hidden window lives on that thread's message loop).
    /// Clicks wake the app with a repaint; the app keeps running while in the tray (see
    /// [`hide_window`]) and restores the window itself.
    pub fn new(ctx: &egui::Context) -> Option<Self> {
        let now_playing = MenuItem::new("Nothing playing", false, None);
        let play = MenuItem::new("Play", true, None);
        let next = MenuItem::new("Next", true, None);
        let previous = MenuItem::new("Previous", true, None);
        let open = MenuItem::new("Open YTDugem", true, None);
        let settings = MenuItem::new("Settings", true, None);
        let quit = MenuItem::new("Quit YTDugem", true, None);
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
            _icon: tray,
            now_playing,
            play,
            actions,
            shown: (String::new(), false),
        })
    }

    pub fn poll(&self) -> Vec<TrayAction> {
        self.actions.try_iter().collect()
    }

    /// Keep the menu's first line and Play/Pause label in step with playback.
    pub fn update(&mut self, now_playing: Option<String>, playing: bool) {
        let label = now_playing.unwrap_or_else(|| "Nothing playing".into());
        if self.shown.0 != label {
            // Menu text is capped well below Windows' limit so the menu stays a sensible width.
            let short: String = label.chars().take(60).collect();
            self.now_playing.set_text(short.replace('&', "&&"));
            self.shown.0 = label;
        }
        if self.shown.1 != playing {
            self.play.set_text(if playing { "Pause" } else { "Play" });
            self.shown.1 = playing;
        }
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
