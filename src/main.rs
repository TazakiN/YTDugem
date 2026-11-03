#![cfg_attr(target_os = "windows", windows_subsystem = "windows")]

mod auth;
mod discord;
mod integrations;
mod model;
mod playback_check;
mod player;
mod runtime;
mod services;
mod store;
#[cfg(windows)]
mod tray;
mod ui;
mod widgets;

fn main() -> eframe::Result {
    runtime::configure().map_err(|e| eframe::Error::AppCreation(e.into()))?;
    let args: Vec<_> = std::env::args_os().collect();
    if args.get(1).is_some_and(|s| s == "--check-playback") {
        if args.len() != 4 {
            return Err(eframe::Error::AppCreation(
                "Usage: YTDugem --check-playback SOURCE REPORT.json".into(),
            ));
        }
        return playback_check::run(&args[2].to_string_lossy(), std::path::Path::new(&args[3]))
            .map_err(|e| eframe::Error::AppCreation(e.into()));
    }
    let open_player = args.iter().any(|s| s == "--screenshot-player");
    let screenshot = match args.iter().position(|s| s == "--screenshot") {
        Some(i) => Some(std::path::PathBuf::from(args.get(i + 1).ok_or_else(
            || eframe::Error::AppCreation("Usage: YTDugem --screenshot FILE.png".into()),
        )?)),
        None => None,
    };
    #[cfg(target_os = "linux")]
    if std::env::var_os("WSL_DISTRO_NAME").is_some()
        && std::env::var("BITCHORD_WSL_COMPAT").as_deref() != Ok("0")
    {
        // ponytail: WSLg's GPU/Wayland path can create an invisible window; use Mesa's CPU renderer.
        std::env::set_var("LIBGL_ALWAYS_SOFTWARE", "1");
        std::env::set_var("GALLIUM_DRIVER", "llvmpipe");
        if std::env::var_os("DISPLAY").is_some() {
            std::env::remove_var("WAYLAND_DISPLAY");
        }
        eprintln!(
            "YTDugem: WSL compatibility rendering enabled (BITCHORD_WSL_COMPAT=0 to disable)"
        );
    }
    let _instance =
        store::lock(&store::data_path()).map_err(|e| eframe::Error::AppCreation(e.into()))?;
    let icon = image::load_from_memory(include_bytes!("../assets/icon.png"))
        .ok()
        .map(|image| {
            let image = image.into_rgba8();
            eframe::egui::IconData {
                width: image.width(),
                height: image.height(),
                rgba: image.into_raw(),
            }
        });
    let mut viewport = eframe::egui::ViewportBuilder::default()
        .with_title("YTDugem")
        .with_decorations(false)
        .with_app_id("in.ytdugem.desktop")
        // Upstream Main.kt: rememberWindowState(1220.dp x 780.dp), MIN_WINDOW_*_DP = 900 x 600.
        .with_inner_size([1220.0, 780.0])
        .with_min_inner_size([900.0, 600.0]);
    if let Some(icon) = icon {
        viewport = viewport.with_icon(icon);
    }
    let options = eframe::NativeOptions {
        viewport,
        ..Default::default()
    };
    eframe::run_native(
        "YTDugem",
        options,
        Box::new(move |cc| {
            Ok(Box::new(
                ui::YTDugem::new(cc)
                    .with_screenshot(screenshot)
                    .with_player_open(open_player),
            ))
        }),
    )
}
