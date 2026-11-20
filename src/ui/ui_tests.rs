//! Headless screens: the app drawn from fixture data (no network, mpv or data folder) and
//! compared with the reference images in `tests/snapshots`.
//!
//! The references come from this project's Windows development machine. Another GPU or a
//! software renderer anti-aliases differently, so CI (`CI` set) draws every screen, which still
//! catches panics and missing glyphs, but skips the image comparison.
//! `UPDATE_SNAPSHOTS=1 cargo test ui_tests` rewrites the references.
use super::*;
use egui_kittest::Harness;

fn track(i: usize) -> Track {
    const SONGS: [(&str, &str); 6] = [
        ("Kangen", "Dewa 19"),
        ("Separuh Nafas", "Dewa 19"),
        ("Pupus", "Dewa 19"),
        ("Rain on the Window", "Lofi Collective"),
        ("Evening Drive", "Night Tapes"),
        ("Cerita Cinta", "Kahitna"),
    ];
    let (title, artist) = SONGS[i % SONGS.len()];
    Track {
        id: format!("video{i:02}"),
        title: if i < SONGS.len() {
            title.to_owned()
        } else {
            format!("{title} (Live {i})")
        },
        artist: artist.to_owned(),
        album: "Fixture Album".into(),
        duration: 180.0 + 7.0 * i as f64,
        source: format!("https://music.youtube.com/watch?v=video{i:02}"),
        ..Default::default()
    }
}

fn item(title: &str, subtitle: &str, i: usize) -> ShelfItem {
    ShelfItem {
        title: format!("{title} {}", i + 1),
        subtitle: subtitle.into(),
        browse_id: Some(format!("MPREb_fixture{i}")),
        ..Default::default()
    }
}

fn shelves() -> Vec<HomeShelf> {
    vec![
        HomeShelf {
            title: "Listen again".into(),
            subtitle: String::new(),
            items: (0..10)
                .map(|i| item("Album", "Album • Dewa 19", i))
                .collect(),
        },
        HomeShelf {
            title: "Music videos for you".into(),
            subtitle: String::new(),
            items: (0..8)
                .map(|i| ShelfItem {
                    video_id: Some(format!("mv{i}")),
                    wide: true,
                    ..item("Video", "Night Tapes • 1.2M views", i)
                })
                .collect(),
        },
        HomeShelf {
            title: "Mixed for you".into(),
            subtitle: "Playlists picked from your listening".into(),
            items: (0..9)
                .map(|i| item("Mix", "Playlist • YouTube Music", i))
                .collect(),
        },
    ]
}

/// The app with a fixture library and feed, read-only so nothing is saved.
fn app() -> YTDugem {
    let dir = std::env::temp_dir().join("ytdugem-ui-tests-no-data");
    let path = dir.join("library.json");
    let library = Library {
        history: (0..14).map(track).collect(),
        recent_searches: vec!["dewa 19".into(), "lofi rain".into(), "kahitna".into()],
        ..Default::default()
    };
    let stats = stats_page::Stats::load(&path);
    let mut app = YTDugem::with_parts(
        library,
        path,
        true,
        String::new(),
        None,
        stats,
        Integrations::new(),
    );
    app.home = shelves();
    app.recents = (0..14).map(track).collect();
    app
}

fn search_app() -> YTDugem {
    let mut app = app();
    app.query = "dewa 19".into();
    app.submitted_query = "dewa 19".into();
    app.results = (0..12).map(track).collect();
    app.artists.results = vec![ShelfItem {
        title: "Dewa 19".into(),
        subtitle: "Artist • 4.1M subscribers".into(),
        browse_id: Some("UCdewa19".into()),
        page_type: Some("MUSIC_PAGE_TYPE_ARTIST".into()),
        ..Default::default()
    }];
    app.artists.playlists = (0..6)
        .map(|i| item("Dewa 19 Hits", "Playlist • Fans", i))
        .collect();
    app.page = Page::Search;
    app
}

fn harness(app: YTDugem, width: f32, height: f32) -> Harness<'static, YTDugem> {
    let mut harness = Harness::builder()
        .with_size(Vec2::new(width, height))
        .with_pixels_per_point(1.0)
        .with_context_setup(|ctx| configure_style(ctx, store::Theme::Classic))
        .build_ui_state(
            |ui, app: &mut YTDugem| {
                // The harness frames its ui with an 8 px margin; the window has none.
                let screen = ui.ctx().content_rect();
                let mut ui = ui.new_child(egui::UiBuilder::new().max_rect(screen));
                ui.set_clip_rect(screen);
                app.draw(&mut ui);
            },
            app,
        );
    harness.run_steps(3);
    harness
}

fn snapshot(harness: &mut Harness<'_, YTDugem>, name: &str) {
    if std::env::var_os("CI").is_some() {
        eprintln!("CI: drew {name} but skipped the image comparison");
        return;
    }
    harness.snapshot(name);
}

#[test]
fn home_wide() {
    let mut h = harness(app(), 1440.0, 900.0);
    snapshot(&mut h, "home_1440x900");
}

#[test]
fn home_narrow() {
    let mut h = harness(app(), 1000.0, 800.0);
    snapshot(&mut h, "home_1000x800");
}

#[test]
fn search_results() {
    let mut h = harness(search_app(), 1440.0, 900.0);
    snapshot(&mut h, "search_results_1440x900");
}

#[test]
fn search_dropdown() {
    let mut app = app();
    app.query = "dewa".into();
    app.search_box_fixture("dewa", &["dewa 19 kangen", "dewa 19 pupus", "dewa budjana"]);
    let mut h = harness(app, 1440.0, 900.0);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::F);
    h.run_steps(3);
    snapshot(&mut h, "search_dropdown_1440x900");
}
