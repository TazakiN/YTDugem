//! Headless screens: the app drawn from fixture data (no network, mpv or data folder) and
//! compared with the reference images in `tests/snapshots`, which git ignores.
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
    app.artists.results = [
        ("Dewa 19", "4.1M"),
        ("Dewa Budjana", "310K"),
        ("Dewata", "12K"),
    ]
    .into_iter()
    .map(|(name, subscribers)| ShelfItem {
        title: name.into(),
        subtitle: format!("Artist • {subscribers} subscribers"),
        browse_id: Some(format!("UC{name}")),
        page_type: Some("MUSIC_PAGE_TYPE_ARTIST".into()),
        ..Default::default()
    })
    .collect();
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

fn snapshot<S>(harness: &mut Harness<'_, S>, name: &str) {
    if std::env::var_os("CI").is_some() {
        eprintln!("CI: drew {name} but skipped the image comparison");
        return;
    }
    harness.snapshot(name);
}

#[test]
fn home_wide() {
    let mut h = harness(app(), 1440.0, 900.0);
    // Hover a recents row: its highlight must frame the cover, not start at the cover's edge.
    h.hover_at(egui::Pos2::new(700.0, 186.0));
    h.run();
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
fn search_results_narrow() {
    let mut h = harness(search_app(), 900.0, 700.0);
    snapshot(&mut h, "search_results_900x700");
}

#[test]
fn search_dropdown() {
    let mut app = app();
    app.query = "dewa".into();
    let songs: Vec<Track> = (0..6).map(track).collect();
    let artist = search_app().artists.results[0].clone();
    app.search_box_fixture(
        "dewa",
        &["dewa 19 kangen", "dewa 19 pupus", "dewa budjana"],
        &[artist],
        &songs,
    );
    let mut h = harness(app, 1440.0, 900.0);
    h.key_press_modifiers(egui::Modifiers::CTRL, egui::Key::F);
    h.run_steps(3);
    snapshot(&mut h, "search_dropdown_1440x900");
}

#[test]
fn emoji_in_headings() {
    if !font_definitions().font_data.contains_key("emoji") {
        eprintln!("no emoji font installed; skipping");
        return;
    }
    // The harness panics on any character no font can draw, so drawing it checks coverage.
    // U+FE0F (emoji presentation) follows the cloud, as in YouTube's own titles.
    let mut h = Harness::builder()
        .with_size(Vec2::new(420.0, 60.0))
        .with_pixels_per_point(1.0)
        .with_context_setup(|ctx| configure_style(ctx, store::Theme::Classic))
        .build_ui(|ui| {
            ui.label(heading(
                "Rain + Feels \u{2614} \u{1F327}\u{FE0F} \u{1F3B5}",
                24.0,
            ));
        });
    h.run();
    snapshot(&mut h, "emoji_heading");
}

#[test]
fn library_owns_its_sub_pages_and_other_pages_light_no_tab() {
    for (page, _) in TABS {
        assert_eq!(owning_tab(&page), Some(page.clone()));
    }
    for page in [
        Page::History,
        Page::Downloads,
        Page::Local,
        Page::Playlist(3),
        Page::Stats,
    ] {
        assert_eq!(owning_tab(&page), Some(Page::Library), "{page:?}");
    }
    for page in [
        Page::Search,
        Page::Settings,
        Page::Collection,
        Page::Artist("UC".into()),
    ] {
        assert_eq!(owning_tab(&page), None, "{page:?}");
    }
}

#[test]
fn tabs_and_library_entries_navigate() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(app(), 1440.0, 900.0);
    h.get_by_label("Library").click();
    h.run_steps(2);
    assert_eq!(h.state().page, Page::Library);
    h.get_by_label("History").click();
    h.run_steps(2);
    assert_eq!(h.state().page, Page::History);
    h.get_by_label("Back").click();
    h.run_steps(2);
    assert_eq!(h.state().page, Page::Library);
    h.get_by_label("Settings").click();
    h.run_steps(2);
    assert_eq!(h.state().page, Page::Settings);
}

#[test]
fn recents_take_one_column_per_320_points_up_to_three() {
    assert_eq!(recent_columns(200.0), 1);
    assert_eq!(recent_columns(639.0), 1);
    assert_eq!(recent_columns(640.0), 2);
    assert_eq!(recent_columns(944.0), 2);
    assert_eq!(recent_columns(960.0), 3);
    assert_eq!(recent_columns(1240.0), 3);
}

#[test]
fn shelf_arrows_page_the_row() {
    use egui_kittest::kittest::Queryable;
    let mut h = harness(app(), 1440.0, 900.0);
    assert!(h.query_by_label("Scroll left").is_none());
    h.get_all_by_label("Scroll right").next().unwrap().click();
    h.run_steps(3);
    assert!(h.query_by_label("Scroll left").is_some());
}

fn snapshot_discord_preview(name: &str, card: crate::discord::Card) {
    let song = Track {
        title: "Museum Duka".into(),
        artist: "Revenge, YB & Tepe".into(),
        duration: 209.0,
        source: "https://music.youtube.com/watch?v=video00".into(),
        ..Default::default()
    };
    let mut h = Harness::builder()
        .with_size(Vec2::new(380.0, 300.0))
        .with_pixels_per_point(1.0)
        .with_context_setup(|ctx| configure_style(ctx, store::Theme::Classic))
        .build_ui(|ui| {
            ui.painter().rect_filled(ui.max_rect(), 0.0, SIDE);
            discord_preview(ui, &card, &song, 90.0, 209.0, None);
        });
    h.run_steps(2);
    snapshot(&mut h, name);
}

#[test]
fn discord_preview_default() {
    snapshot_discord_preview("discord_preview_default", crate::discord::Card::default());
}

#[test]
fn discord_preview_customized() {
    let card = crate::discord::Card {
        activity: crate::discord::Activity::Playing,
        name: "my radio".into(),
        lead_with_song: true,
        button_text: ["Listen to {song_name}".into(), String::new()],
        ..Default::default()
    };
    snapshot_discord_preview("discord_preview_custom", card);
}
