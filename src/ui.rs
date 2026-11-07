use crate::{
    auth::{self, Account},
    discord::Presence,
    integrations::{Integrations, MediaEvent},
    model::{clock, Track},
    player::{Player, PlayerStatus, Stream},
    services::translate::{self, Romanization, Translation},
    services::{self, Collection, HomeFeed, HomeShelf, LikeStatus, LyricLine, ShelfItem},
    store::{self, Library},
    widgets::{self, Icon},
};
use eframe::egui::{self, Align, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};
use rand::seq::SliceRandom;
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex,
    },
    time::{Duration, Instant},
};

mod explore;
mod sheet;
mod sheet_lyrics;
mod sheet_menu;
mod sheet_output;
mod sheet_queue;

const BG: Color32 = Color32::from_rgb(29, 31, 27);
const SIDE: Color32 = Color32::from_rgb(21, 22, 19);
/// Discord Rich Presence is shown as "Coming soon" until YTDugem has its own Discord
/// application; the upstream application id would show listeners as BitChord.
const DISCORD_READY: bool = false;
const MUTED: Color32 = Color32::from_rgb(150, 151, 147);
const WHITE: Color32 = Color32::from_rgb(244, 244, 242);
/// Calls to action and progress in dialogs.
const ACCENT: Color32 = Color32::from_rgb(178, 208, 158);
const ERROR: Color32 = Color32::from_rgb(232, 120, 110);

#[derive(Clone, Debug, PartialEq, Hash)]
enum Page {
    Home,
    Explore,
    Library,
    Search,
    History,
    Downloads,
    Local,
    Playlist(usize),
    Collection,
    Settings,
}
#[derive(Clone, Copy, PartialEq)]
enum PlayerPane {
    Main,
    Lyrics,
    Queue,
}
/// What the lyrics show under each line. One at a time, as upstream: turning on romanization
/// turns off the translation and the other way round.
#[derive(Clone, Copy, PartialEq, Default)]
enum LyricsMode {
    #[default]
    Original,
    Romanized,
    Translated,
}
// ponytail: low-volume worker events own their payloads; no shared mutable UI state.
#[allow(clippy::large_enum_variant)]
enum Event {
    Playing(u64, Track, Result<Player, String>),
    Prepared(String, Option<Stream>),
    AccountPlaylists(u64, Result<Vec<ShelfItem>, String>),
    Suggestions(u64, String, Result<Vec<String>, String>),
    Translated(u64, Result<Translation, String>),
    Romanized(u64, String, Result<Romanization, String>),
    SongInfo(u64, String, Result<services::SongInfo, String>),
    /// A rating sent for (track key); carries the rating it replaced, to restore on failure.
    Rated(String, LikeStatus, Result<(), String>),
    PlaylistEdited(Result<PlaylistEdit, String>),
    /// Every track of a playlist, fetched for a bulk action.
    BulkTracks(PlaylistBulk, String, Result<Vec<Track>, String>),
    Results(u64, Result<Vec<Track>, String>),
    Home(u64, Result<HomeFeed, String>),
    Supplement(u64, Result<Vec<HomeShelf>, String>),
    Recents(u64, Result<Vec<Track>, String>),
    Collection(u64, Result<Collection, String>),
    Imported(Result<Vec<Track>, String>),
    Lyrics(u64, String, Result<services::lyrics::Lyrics, String>),
    /// A finished download (track key), already imported into local music.
    Downloaded(String, Result<Vec<Track>, String>),
    DownloadProgress(String, f32),
    /// A sign-in or channel switch finished.
    SignedIn(u64, Result<Account, String>),
    /// A browser's session and the accounts it can act as, for the account picker.
    BrowserImported(
        u64,
        &'static str,
        Result<(String, Vec<auth::Channel>), String>,
    ),
    /// The saved session was checked at start-up; `true` with the error when YouTube retired it.
    Renewed(Result<Account, (bool, String)>),
    LyricsKeys(u64, Result<Vec<(String, bool)>, String>),
    LyricsKeySaved(u64, String, Result<bool, String>),
    AudioDevices(Result<Vec<(String, String)>, String>),
    Artwork(String, Result<(egui::ColorImage, egui::ColorImage), String>),
    Radio(u64, String, Result<Vec<Track>, String>),
    /// Shelves for one Explore page, tagged with the request they answer.
    ExploreShelves(u64, explore::Feed, Result<Vec<HomeShelf>, String>),
    Moods(u64, Result<Vec<services::MoodGenreSection>, String>),
    /// A mood/genre tile's cover: (request, category key, url).
    MoodArtwork(u64, String, String),
}

pub struct YTDugem {
    library: Library,
    path: PathBuf,
    read_only: bool,
    player: Player,
    integrations: Integrations,
    status: PlayerStatus,
    current: Option<Track>,
    previous: Vec<Track>,
    play_request: u64,
    loading: bool,
    page: Page,
    nav: Vec<Page>,
    query: String,
    submitted_query: String,
    results: Vec<Track>,
    request: u64,
    busy: bool,
    home: Vec<HomeShelf>,
    home_request: u64,
    home_pending: usize,
    home_token: Option<String>,
    home_more: bool,
    recents: Vec<Track>,
    explore: explore::Explore,
    collection: Option<Collection>,
    collection_request: u64,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    message: String,
    importing: bool,
    downloads: Vec<DownloadItem>,
    expanded: bool,
    tab: PlayerPane,
    shuffle: bool,
    repeat: u8,
    lyrics: Vec<LyricLine>,
    lyrics_message: String,
    /// Why the last lookup found nothing, provider by provider; shown on hover only.
    lyrics_detail: String,
    lyrics_provider: String,
    lyrics_override: Option<String>,
    lyrics_request: u64,
    follow_lyrics: bool,
    active_lyric: Option<usize>,
    cover: Option<egui::TextureHandle>,
    backdrop: Option<egui::TextureHandle>,
    import_path: String,
    download_path: String,
    sleep_until: Option<Instant>,
    stop_after_track: bool,
    account: Option<Account>,
    /// The sign-in dialog while it is open.
    sign_in: SignIn,
    account_request: u64,
    /// The accounts and channels the session can act as, and which one it does.
    channels: Vec<auth::Channel>,
    source: auth::Source,
    radio_request: u64,
    lyric_keys: BTreeMap<String, bool>,
    lyric_key_inputs: BTreeMap<String, String>,
    lyric_keys_loaded: bool,
    lyric_keys_busy: bool,
    audio_devices: Vec<(String, String)>,
    audio_devices_loading: bool,
    audio_devices_loaded: bool,
    lyric_keys_request: u64,
    last_poll: Instant,
    now_origin: String,
    radio_pending: bool,
    discord: Presence,
    /// The left sidebar (navigation); toggled from the top bar.
    sidebar_open: bool,
    palette_open: bool,
    palette_query: String,
    palette_suggestions: Vec<String>,
    palette_selected: Option<usize>,
    palette_focus: bool,
    /// When the query last changed; suggestions wait for a short pause after typing.
    palette_edited: Option<Instant>,
    /// The query the last suggestion request was sent for.
    palette_asked: String,
    palette_request: u64,
    palette_loading: bool,
    #[cfg(windows)]
    tray: Option<crate::tray::Tray>,
    #[cfg(windows)]
    hwnd: Option<isize>,
    /// Set by Quit (Ctrl+Q, the tray): a close that must not be turned into hide-to-tray.
    quitting: bool,
    /// `--screenshot FILE`: save the window after `SCREENSHOT_FRAMES` frames, then exit.
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    /// Filled by the screenshot callback once the frame has been captured.
    screenshot_image: Arc<Mutex<Option<Arc<egui::ColorImage>>>>,
    frames: u32,
    /// The upcoming track's resolved stream (key, stream, when), prepared while this one plays.
    prepared: Option<(String, Stream, Instant)>,
    /// Key of the track being prepared now, so one resolution runs at a time.
    preparing: Option<String>,
    /// With shuffle on, the queue entry Next will take, chosen early so it can be prepared.
    next_pick: Option<String>,
    /// The outgoing track during a crossfade.
    fade: Option<Fade>,
    /// Set while `play` runs for a crossfade: start silent and keep the outgoing player.
    crossfading: bool,
    /// The account's YouTube playlists; `None` until loaded.
    account_playlists: Option<Vec<ShelfItem>>,
    account_playlists_request: u64,
    account_playlists_loading: bool,
    playlist_dialog: Option<PlaylistDialog>,
    /// A download awaiting confirmation before its file is deleted from disk.
    delete_download: Option<Track>,
    /// One account-playlist change at a time.
    playlist_busy: bool,
    /// Set when the open dialog started the running change; its result then closes the dialog or
    /// shows its error.
    playlist_pending: bool,
    playlist_error: String,
    /// A whole playlist is being fetched for Play, Shuffle, Add to queue or Download all.
    bulk_loading: bool,
    /// The lyrics translated for (track key, language), when asked for.
    translation: Option<(String, String, Translation)>,
    /// Romanized or translated sub-lines; stays on across songs, as upstream.
    lyrics_mode: LyricsMode,
    /// The lyrics romanized for (track key), when asked for.
    romanization: Option<(String, Romanization)>,
    romanizing: bool,
    romanization_request: u64,
    /// The Audio output dialog over the player sheet.
    output_open: bool,
    /// Rating and album/artist pages of (track key), for the sheet's like button and menu.
    song_info: Option<(String, services::SongInfo)>,
    song_info_request: u64,
    /// How far the player sheet is dragged down from fully open, in points.
    sheet_drag: f32,
    translating: bool,
    translation_request: u64,
    settings_query: String,
}

const SCREENSHOT_FRAMES: u32 = 120;

impl YTDugem {
    pub fn with_screenshot(mut self, path: Option<PathBuf>) -> Self {
        self.screenshot = path;
        self
    }
    /// `--screenshot-player`: open Now Playing on the most recent history track, for UI checks.
    pub fn with_player_open(mut self, open: bool) -> Self {
        if let Some(t) = self.library.history.first().cloned().filter(|_| open) {
            self.now_origin = "History".into();
            self.expanded = true;
            self.play(t);
        }
        self
    }
    fn screenshot_step(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else {
            return;
        };
        self.frames += 1;
        // Frames come fast while spinners animate; also give artwork a few seconds to arrive.
        if self.frames >= SCREENSHOT_FRAMES
            && ctx.input(|i| i.time) >= 6.0
            && !self.screenshot_requested
        {
            self.screenshot_requested = true;
            let slot = self.screenshot_image.clone();
            let repaint = ctx.clone();
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(
                egui::ScreenshotCallback::new(move |image| {
                    if let Ok(mut slot) = slot.lock() {
                        *slot = Some(image);
                    }
                    repaint.request_repaint();
                }),
            ));
        }
        let image = self.screenshot_image.lock().ok().and_then(|mut s| s.take());
        if let Some(image) = image {
            let [w, h] = image.size;
            let rgba: Vec<u8> = image.pixels.iter().flat_map(|p| p.to_array()).collect();
            if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
                let _ = std::fs::create_dir_all(parent);
            }
            if let Err(e) = image::save_buffer(
                &path,
                &rgba,
                w as u32,
                h as u32,
                image::ExtendedColorType::Rgba8,
            ) {
                eprintln!("YTDugem: could not save screenshot: {e}");
            }
            self.screenshot = None;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }
    pub fn new(cc: &eframe::CreationContext<'_>) -> Self {
        configure_style(&cc.egui_ctx);
        let path = store::data_path();
        let (library, mut message, read_only) = match Library::load(&path) {
            Ok(l) => (l, String::new(), false),
            Err(e) => (
                Library::default(),
                format!("{e:#}. Saving disabled to preserve your library."),
                true,
            ),
        };
        let account = match auth::restore() {
            Ok(a) => a,
            Err(_) => {
                message =
                    "The saved sign-in could not be opened. Sign in again from the account menu."
                        .into();
                None
            }
        };
        #[cfg(windows)]
        let hwnd = {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            cc.window_handle().ok().and_then(|h| match h.as_raw() {
                RawWindowHandle::Win32(w) => Some(w.hwnd.get()),
                _ => None,
            })
        };
        #[cfg(windows)]
        let integrations = Integrations::for_window(hwnd.map(|h| h as *mut std::ffi::c_void));
        #[cfg(not(windows))]
        let integrations = Integrations::new();
        let download_path = library.download_dir.to_string_lossy().into_owned();
        let (tx, rx) = mpsc::channel();
        let mut app = Self {
            library,
            path,
            read_only,
            player: Player::new(),
            integrations,
            status: PlayerStatus::default(),
            current: None,
            previous: vec![],
            play_request: 0,
            loading: false,
            page: Page::Home,
            nav: vec![],
            query: String::new(),
            submitted_query: String::new(),
            results: vec![],
            request: 0,
            busy: false,
            home: vec![],
            home_request: 0,
            home_pending: 0,
            home_token: None,
            home_more: false,
            recents: vec![],
            explore: Default::default(),
            collection: None,
            collection_request: 0,
            tx,
            rx,
            message,
            importing: false,
            downloads: vec![],
            expanded: false,
            tab: PlayerPane::Main,
            shuffle: false,
            repeat: 0,
            lyrics: vec![],
            lyrics_message: String::new(),
            lyrics_detail: String::new(),
            lyrics_provider: String::new(),
            lyrics_override: None,
            lyrics_request: 0,
            follow_lyrics: true,
            active_lyric: None,
            cover: None,
            backdrop: None,
            import_path: String::new(),
            download_path,
            sleep_until: None,
            stop_after_track: false,
            account,
            sign_in: SignIn::default(),
            account_request: 0,
            channels: vec![],
            source: auth::Source::default(),
            radio_request: 0,
            lyric_keys: BTreeMap::new(),
            lyric_key_inputs: BTreeMap::new(),
            lyric_keys_loaded: false,
            lyric_keys_busy: false,
            audio_devices: vec![],
            audio_devices_loading: false,
            audio_devices_loaded: false,
            lyric_keys_request: 0,
            last_poll: Instant::now(),
            now_origin: "Home".into(),
            radio_pending: false,
            discord: Presence::new(),
            sidebar_open: true,
            palette_open: false,
            palette_query: String::new(),
            palette_suggestions: vec![],
            palette_selected: None,
            palette_focus: false,
            palette_edited: None,
            palette_asked: String::new(),
            palette_request: 0,
            palette_loading: false,
            #[cfg(windows)]
            tray: crate::tray::Tray::new(&cc.egui_ctx),
            #[cfg(windows)]
            hwnd,
            quitting: false,
            screenshot: None,
            screenshot_requested: false,
            screenshot_image: Arc::default(),
            frames: 0,
            prepared: None,
            preparing: None,
            next_pick: None,
            fade: None,
            crossfading: false,
            account_playlists: None,
            account_playlists_request: 0,
            account_playlists_loading: false,
            playlist_dialog: None,
            delete_download: None,
            playlist_busy: false,
            playlist_pending: false,
            playlist_error: String::new(),
            bulk_loading: false,
            translation: None,
            lyrics_mode: LyricsMode::Original,
            romanization: None,
            romanizing: false,
            romanization_request: 0,
            output_open: false,
            song_info: None,
            song_info_request: 0,
            sheet_drag: 0.0,
            translating: false,
            translation_request: 0,
            settings_query: String::new(),
        };
        if app.account.is_some() {
            // The saved session is shown at once and checked with YouTube in the background;
            // one YouTube retired is read again from its browser or ends (Meiro `renewSession`).
            let tx = app.tx.clone();
            std::thread::spawn(move || {
                let result = auth::renew().map_err(|e| (e.is::<auth::SessionGone>(), err(e)));
                let _ = tx.send(Event::Renewed(result));
            });
        }
        app.load_home();
        app
    }
    fn save(&mut self) {
        if !self.read_only {
            if let Err(e) = self.library.save(&self.path) {
                self.message = format!("Could not save library: {e:#}");
            }
        }
    }
    fn apply_audio_settings(&mut self) {
        if self.player.is_active() {
            if let Err(error) = self.player.set_audio_filter(&self.library.audio) {
                self.message = format!("Could not update audio filter: {error:#}");
            }
        }
    }
    fn navigate(&mut self, page: Page) {
        if self.page != page {
            self.nav.push(self.page.clone());
            self.page = page;
        }
        self.expanded = false;
    }
    fn report(&mut self, r: anyhow::Result<()>) {
        if let Err(e) = r {
            self.message = format!("{e:#}");
        }
    }
    fn load_home(&mut self) {
        self.home_request += 1;
        let id = self.home_request;
        self.home.clear();
        self.recents.clear();
        self.home_token = None;
        self.home_more = false;
        // Upstream loadHome: Home, signed-in Recently played, then HOME_SUPPLEMENT_BROWSE_IDS.
        self.home_pending =
            1 + services::HOME_SUPPLEMENT_BROWSE_IDS.len() + usize::from(self.account.is_some());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Home(id, services::home().map_err(err)));
        });
        for endpoint in services::HOME_SUPPLEMENT_BROWSE_IDS {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Event::Supplement(
                    id,
                    services::home_supplement(endpoint).map_err(err),
                ));
            });
        }
        if self.account.is_some() {
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Event::Recents(id, services::recent().map_err(err)));
            });
        }
    }
    fn more_home(&mut self) {
        if self.home_more {
            return;
        }
        if let Some(token) = self.home_token.take() {
            self.home_more = true;
            let tx = self.tx.clone();
            let id = self.home_request;
            std::thread::spawn(move || {
                let _ = tx.send(Event::Home(id, services::home_more(&token).map_err(err)));
            });
        }
    }
    fn search(&mut self) {
        if self.query.trim().is_empty() {
            self.navigate(Page::Search);
            return;
        }
        self.request += 1;
        let id = self.request;
        self.busy = true;
        self.results.clear();
        self.submitted_query = self.query.trim().into();
        self.navigate(Page::Search);
        let q = self.submitted_query.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Results(id, services::search(&q).map_err(err)));
        });
    }
    fn open_item(&mut self, item: ShelfItem) {
        if let Some(t) = item.track() {
            self.start_tracks(vec![t], 0, "Home");
        } else if let Some(id) = item.browse_id {
            self.collection_request += 1;
            let request = self.collection_request;
            self.busy = true;
            self.collection = None;
            self.navigate(Page::Collection);
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Event::Collection(
                    request,
                    services::collection(&id, item.params.as_deref()).map_err(err),
                ));
            });
        }
    }
    fn more_collection(&mut self) {
        if self.busy {
            return;
        }
        if let Some(token) = self.collection.as_mut().and_then(|c| c.continuation.take()) {
            self.busy = true;
            let tx = self.tx.clone();
            let id = self.collection_request;
            std::thread::spawn(move || {
                let _ = tx.send(Event::Collection(
                    id,
                    services::collection_more(&token).map_err(err),
                ));
            });
        }
    }
    fn import(&mut self, p: PathBuf) {
        if self.importing {
            return;
        }
        self.importing = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Imported(services::import_local(&p).map_err(err)));
        });
    }
    fn start_tracks(&mut self, tracks: Vec<Track>, index: usize, origin: &str) {
        if let Some(track) = tracks.get(index).cloned() {
            self.library.queue = tracks.into_iter().skip(index + 1).collect();
            self.previous.clear();
            self.now_origin = origin.into();
            self.play(track);
        }
    }
    fn play(&mut self, t: Track) {
        self.play_request += 1;
        let id = self.play_request;
        self.loading = true;
        self.player.stop();
        if !self.crossfading {
            // A track chosen by hand cuts any blend short.
            self.fade = None;
        }
        let prepared = self
            .prepared
            .take()
            .filter(|(key, _, at)| key == t.key() && at.elapsed() < Duration::from_secs(3600))
            .map(|(_, stream, _)| stream);
        self.status = PlayerStatus {
            paused: true,
            ..Default::default()
        };
        let (volume, speed, audio) = (
            if self.crossfading {
                0.0
            } else {
                self.library.volume
            },
            self.library.speed,
            self.library.audio.clone(),
        );
        // Optimistic: the chosen track becomes the current one now, while the stream resolves,
        // so the player, rows and artwork react to the click instead of to mpv seconds later.
        if self.current.as_ref().map(Track::key) != Some(t.key()) {
            self.cover = None;
            self.backdrop = None;
            self.lyrics.clear();
            self.active_lyric = None;
            self.lyrics_override = None;
            self.radio_pending = false;
            if t.thumbnail.starts_with("https://") {
                let tx = self.tx.clone();
                let song = t.clone();
                std::thread::spawn(move || {
                    let _ = tx.send(Event::Artwork(
                        song.key().into(),
                        load_artwork(&song.thumbnail).map_err(err),
                    ));
                });
            }
            self.current = Some(t.clone());
            self.request_song_info();
            if self.library.lyrics_enabled {
                self.request_lyrics(None);
            } else {
                self.lyrics_message = "Lyrics are disabled in Settings".into();
            }
        }
        self.discord.clear();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut p = Player::new();
            let r = p
                .load_prepared(&t.source, prepared, volume, speed, &audio)
                .map(|_| p)
                .map_err(err);
            let _ = tx.send(Event::Playing(id, t, r));
        });
    }
    fn request_lyrics(&mut self, only: Option<String>) {
        if !self.library.lyrics_enabled {
            self.lyrics_message = "Lyrics are disabled in Settings".into();
            return;
        }
        let Some(song) = self.current.clone() else {
            return;
        };
        self.lyrics_request += 1;
        let request = self.lyrics_request;
        let key = song.key().to_owned();
        self.lyrics.clear();
        self.lyrics_provider.clear();
        self.lyrics_message = "Finding lyrics…".into();
        self.lyrics_detail.clear();
        self.translation_request += 1;
        self.translation = None;
        self.translating = false;
        self.romanization_request += 1;
        self.romanization = None;
        self.romanizing = false;
        let (enabled, order) = (
            self.library.lyrics_sources.clone(),
            self.library.lyrics_order.clone(),
        );
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result =
                services::lyrics::lookup(&song, &enabled, &order, only.as_deref()).map_err(err);
            let _ = tx.send(Event::Lyrics(request, key, result));
        });
    }
    /// The current song's rating and album/artist pages, for the player sheet.
    fn request_song_info(&mut self) {
        self.song_info_request += 1;
        self.song_info = None;
        let Some(t) = self.current.clone().filter(is_youtube_track) else {
            return;
        };
        let (id, tx) = (self.song_info_request, self.tx.clone());
        std::thread::spawn(move || {
            let _ = tx.send(Event::SongInfo(
                id,
                t.key().to_owned(),
                services::song_info(&t.id).map_err(err),
            ));
        });
    }
    /// The current song's rating on the account, once known.
    fn like_status(&self) -> Option<LikeStatus> {
        let key = self.current.as_ref()?.key();
        self.song_info
            .as_ref()
            .filter(|(k, _)| k == key)
            .and_then(|(_, info)| info.like)
    }
    /// Rate the current song; shown at once, and put back if YouTube refuses it.
    fn rate(&mut self, status: LikeStatus) {
        let Some(t) = self.current.clone().filter(is_youtube_track) else {
            return;
        };
        if self.account.is_none() {
            self.message = "Sign in to YouTube Music to rate songs".into();
            return;
        }
        let key = t.key().to_owned();
        let previous = self.like_status().unwrap_or_default();
        match self.song_info.as_mut().filter(|(k, _)| *k == key) {
            Some((_, info)) => info.like = Some(status),
            None => {
                self.song_info = Some((
                    key.clone(),
                    services::SongInfo {
                        like: Some(status),
                        ..Default::default()
                    },
                ))
            }
        }
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Rated(
                key,
                previous,
                services::rate_song(&t.id, status).map_err(err),
            ));
        });
    }
    fn lyrics_settings_changed(&mut self, reload: bool) {
        self.lyrics_request += 1;
        self.lyrics.clear();
        self.lyrics_provider.clear();
        self.active_lyric = None;
        self.lyrics_message = if self.library.lyrics_enabled {
            String::new()
        } else {
            "Lyrics are disabled in Settings".into()
        };
        if reload && self.library.lyrics_enabled {
            self.request_lyrics(None);
        }
    }
    fn playing(&mut self, t: Track, result: Result<Player, String>) {
        self.loading = false;
        match result {
            // `play` already showed the track, its artwork and lyrics; only attach the engine.
            Ok(p) => {
                self.player = p;
                self.library.remember(&t);
                self.status = PlayerStatus::default();
                if let Some(fade) = &mut self.fade {
                    fade.started = Some(Instant::now());
                } else if self.player.volume(self.library.volume).is_err() {
                    // Started silent for a blend that has already ended; restore it.
                    self.message = "Could not restore the volume after a crossfade".into();
                }
                self.save();
            }
            Err(e) => {
                self.status.paused = true;
                self.status.ended = true;
                self.fade = None;
                self.message = format!("Playback failed: {e}");
            }
        }
    }
    fn next(&mut self, automatic: bool) {
        self.play_request += 1;
        self.loading = false;
        if automatic && self.stop_after_track {
            self.stop_after_track = false;
            self.stop();
            return;
        }
        if automatic && self.repeat == 2 {
            if let Some(t) = self.current.clone() {
                self.play(t);
            }
            return;
        }
        if self.library.queue.is_empty() {
            if self.repeat == 1 {
                if let Some(t) = self.current.clone() {
                    self.play(t);
                    return;
                }
            }
            if self.library.autoplay
                && self
                    .current
                    .as_ref()
                    .is_some_and(|t| t.source.starts_with("https://"))
            {
                self.fetch_radio();
                self.player.stop();
                self.status.paused = true;
                self.status.ended = true;
                return;
            }
            self.stop();
            return;
        }
        let pick = self.next_pick.take();
        let i = if self.shuffle {
            // The entry chosen (and prepared) ahead of time, if it is still queued.
            pick.and_then(|key| self.library.queue.iter().position(|t| t.key() == key))
                .unwrap_or_else(|| rand::random_range(0..self.library.queue.len()))
        } else {
            0
        };
        let t = self.library.queue.remove(i);
        if let Some(old) = self.current.clone() {
            self.previous.push(old.clone());
            if self.repeat == 1 {
                self.library.queue.push(old);
            }
        }
        self.play(t);
        self.save();
    }
    fn fetch_radio(&mut self) {
        if self.radio_pending {
            return;
        }
        if let Some(t) = self.current.clone() {
            self.radio_pending = true;
            self.radio_request += 1;
            let request = self.radio_request;
            let tx = self.tx.clone();
            std::thread::spawn(move || {
                let _ = tx.send(Event::Radio(
                    request,
                    t.key().into(),
                    services::radio(&t.id).map_err(err),
                ));
            });
        }
    }
    fn invalidate_account_results(&mut self) {
        self.account_playlists = None;
        self.account_playlists_request += 1;
        self.account_playlists_loading = false;
        self.request += 1;
        self.home_request += 1;
        self.collection_request += 1;
        self.lyrics_request += 1;
        self.radio_request += 1;
        self.busy = false;
        self.results.clear();
        self.submitted_query.clear();
        self.home.clear();
        self.recents.clear();
        self.home_token = None;
        self.home_more = false;
        self.home_pending = 0;
        self.explore.reset();
        self.collection = None;
        self.lyrics.clear();
        self.lyrics_provider.clear();
        self.lyrics_message.clear();
        self.active_lyric = None;
        self.radio_pending = false;
    }
    fn sync_account(&mut self) {
        self.invalidate_account_results();
        self.account = auth::current_account();
        (self.channels, self.source) = auth::current_channels();
        self.load_home();
        self.request_lyrics(self.lyrics_override.clone());
    }
    fn load_lyrics_keys(&mut self) {
        if self.lyric_keys_loaded || self.lyric_keys_busy {
            return;
        }
        self.lyric_keys_busy = true;
        self.lyric_keys_request += 1;
        let request = self.lyric_keys_request;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let ids: Vec<String> = services::lyrics::PROVIDERS
                .iter()
                .filter_map(|provider| provider.key_id)
                .map(str::to_owned)
                .collect();
            let result = ids
                .into_iter()
                .collect::<std::collections::BTreeSet<_>>()
                .into_iter()
                .map(|id| services::lyrics::key_configured(&id).map(|configured| (id, configured)))
                .collect::<anyhow::Result<Vec<_>>>()
                .map_err(err);
            let _ = tx.send(Event::LyricsKeys(request, result));
        });
    }
    fn load_audio_devices(&mut self) {
        if self.audio_devices_loading || self.audio_devices_loaded {
            return;
        }
        self.audio_devices_loading = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::AudioDevices(
                crate::player::audio_devices().map_err(err),
            ));
        });
    }
    fn save_lyrics_key(&mut self, key: String) {
        if self.lyric_keys_busy {
            return;
        }
        self.lyric_keys_busy = true;
        self.lyric_keys_request += 1;
        let request = self.lyric_keys_request;
        let value = self.lyric_key_inputs.get(&key).cloned().unwrap_or_default();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = services::lyrics::save_key(&key, &value)
                .and_then(|()| services::lyrics::key_configured(&key))
                .map_err(err);
            let _ = tx.send(Event::LyricsKeySaved(request, key, result));
        });
    }
    fn previous(&mut self) {
        if self.status.position > 3.0 && self.player.is_active() {
            let r = self.player.seek(0.0);
            self.report(r);
        } else if let Some(t) = self.previous.pop() {
            if let Some(old) = self.current.clone() {
                self.library.queue.insert(0, old);
            }
            self.play(t);
        } else if let Some(t) = self.current.clone() {
            self.play(t);
        }
    }
    /// What Next would play, exactly (upstream `prepareNext`): nothing under repeat-one or when
    /// the queue is empty (AutoPlay's radio arrives later), the early pick under shuffle.
    fn upcoming(&mut self) -> Option<Track> {
        if self.repeat == 2 || self.library.queue.is_empty() {
            return None;
        }
        if !self.shuffle {
            return self.library.queue.first().cloned();
        }
        let queue = &self.library.queue;
        let picked = self
            .next_pick
            .as_deref()
            .and_then(|key| queue.iter().find(|t| t.key() == key));
        let t = picked
            .unwrap_or_else(|| &queue[rand::random_range(0..queue.len())])
            .clone();
        self.next_pick = Some(t.key().to_owned());
        Some(t)
    }
    /// Resolve the upcoming YouTube track's stream in the background while this one plays, so
    /// Next and crossfades start without the ~3 s extraction.
    fn prepare_next(&mut self) {
        if self.loading || !self.player.is_active() || self.preparing.is_some() {
            return;
        }
        let Some(next) = self.upcoming().filter(|t| t.source.starts_with("https://")) else {
            return;
        };
        let fresh = self.prepared.as_ref().is_some_and(|(key, _, at)| {
            key == next.key() && at.elapsed() < Duration::from_secs(1800)
        });
        if fresh {
            return;
        }
        let key = next.key().to_owned();
        self.preparing = Some(key.clone());
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Prepared(
                key,
                crate::player::resolve_stream(&next.source),
            ));
        });
    }
    /// Near the end of a track, hand it to a fading player and start the next one silently.
    fn maybe_crossfade(&mut self) {
        let seconds = f64::from(self.library.crossfade_seconds.min(12));
        let s = self.status;
        let remaining = s.duration - s.position;
        // Half a second of lead covers mpv opening an already-prepared stream.
        if seconds <= 0.0
            || self.fade.is_some()
            || self.loading
            || !self.player.is_active()
            || s.paused
            || s.ended
            || s.duration < seconds * 3.0
            || remaining > seconds + 0.5
            || self.stop_after_track
        {
            return;
        }
        let Some(next) = self.upcoming() else { return };
        let ready = !next.source.starts_with("https://")
            || self
                .prepared
                .as_ref()
                .is_some_and(|(key, _, _)| key == next.key());
        if !ready {
            return;
        }
        let old = std::mem::replace(&mut self.player, Player::new());
        self.fade = Some(Fade {
            old,
            started: None,
            seconds: remaining.clamp(1.0, seconds) as f32,
        });
        self.crossfading = true;
        self.next(false);
        self.crossfading = false;
    }
    /// Equal-power blend: the outgoing track follows cos, the incoming one sin.
    fn step_fade(&mut self) {
        let Some(fade) = &mut self.fade else { return };
        let old_alive = fade.old.poll().is_some_and(|s| !s.ended && !s.errored);
        let t = fade.started.map_or(0.0, |at| {
            (at.elapsed().as_secs_f32() / fade.seconds).min(1.0)
        });
        if fade.started.is_some() && old_alive && t < 1.0 {
            let (old, new) = fade_gains(t);
            let volume = self.library.volume;
            let _ = fade.old.volume(volume * old);
            let _ = self.player.volume(volume * new);
        } else if fade.started.is_some() || !old_alive {
            self.finish_fade();
        }
    }
    fn finish_fade(&mut self) {
        if self.fade.take().is_some() && self.player.is_active() {
            let r = self.player.volume(self.library.volume);
            self.report(r);
        }
    }
    fn toggle(&mut self) {
        if self.loading {
            return;
        }
        self.finish_fade();
        if self.player.is_active() {
            let r = self.player.toggle();
            self.report(r);
        } else if let Some(t) = self.current.clone() {
            self.play(t);
        } else if !self.library.queue.is_empty() {
            self.next(false);
        }
    }
    fn stop(&mut self) {
        self.play_request += 1;
        self.loading = false;
        self.fade = None;
        self.player.stop();
        self.status.paused = true;
        self.status.ended = true;
        self.discord.clear();
    }
    /// Queue `t` for download (upstream `DesktopDownloadQueue.enqueue`): a track already queued or
    /// running is not added twice; a failed one is retried.
    fn download(&mut self, t: Track) {
        match self.downloads.iter_mut().find(|d| d.track.key() == t.key()) {
            Some(d) if matches!(d.state, DownloadState::Failed(_)) => {
                d.state = DownloadState::Queued;
                d.cancel = Arc::new(AtomicBool::new(false));
            }
            Some(_) => {
                self.message = format!("{} is already in Downloads", t.title);
                return;
            }
            None => self.downloads.push(DownloadItem {
                track: t,
                state: DownloadState::Queued,
                cancel: Arc::new(AtomicBool::new(false)),
            }),
        }
        self.pump_downloads();
    }
    /// Start queued downloads, [`DOWNLOAD_WORKERS`] at a time, in queue order.
    fn pump_downloads(&mut self) {
        let mut running = self
            .downloads
            .iter()
            .filter(|d| matches!(d.state, DownloadState::Running(_)))
            .count();
        for d in &mut self.downloads {
            if running >= DOWNLOAD_WORKERS {
                break;
            }
            if !matches!(d.state, DownloadState::Queued) {
                continue;
            }
            running += 1;
            d.state = DownloadState::Running(0.0);
            let (tx, track, cancel) = (self.tx.clone(), d.track.clone(), d.cancel.clone());
            let (dir, format) = (
                self.library.download_dir.clone(),
                self.library.download_format.clone(),
            );
            std::thread::spawn(move || {
                let key = track.key().to_owned();
                let mut last = -1.0_f32;
                let result = services::download_with(&track, &dir, &format, &cancel, &mut |f| {
                    if f - last >= 0.01 || f >= 1.0 {
                        last = f;
                        let _ = tx.send(Event::DownloadProgress(key.clone(), f));
                    }
                })
                .and_then(|path| services::import_local(&path))
                .map(|mut tracks| {
                    // Tags carry title and artist; the artwork, and anything the tags lack,
                    // comes from where it was found.
                    for t in &mut tracks {
                        if t.thumbnail.is_empty() {
                            t.thumbnail = track.thumbnail.clone();
                        }
                        if t.artist.is_empty() {
                            t.artist = track.artist.clone();
                        }
                        if t.album.is_empty() {
                            t.album = track.album.clone();
                        }
                    }
                    tracks
                })
                .map_err(err);
                let _ = tx.send(Event::Downloaded(key, result));
            });
        }
    }
    /// Upstream `DesktopDownloadManagerUi`: what is queued, running or failed, above the files.
    fn download_manager(&mut self, ui: &mut egui::Ui) {
        ui.horizontal(|ui| {
            ui.label(heading("Downloads", 26.0));
            ui.add_space(12.0);
            ui.label(RichText::new(download_summary(&self.downloads)).color(MUTED));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                let failed = self
                    .downloads
                    .iter()
                    .any(|d| matches!(d.state, DownloadState::Failed(_)));
                if failed && icon_action(ui, true, Icon::Broom, "Remove failed downloads").clicked()
                {
                    self.downloads
                        .retain(|d| !matches!(d.state, DownloadState::Failed(_)));
                }
                if !self.downloads.is_empty()
                    && icon_action(ui, true, Icon::Close, "Cancel all downloads").clicked()
                {
                    for d in self.downloads.drain(..) {
                        d.cancel.store(true, Ordering::SeqCst);
                    }
                }
            });
        });
        ui.add_space(8.0);
        let mut cancel = None;
        let mut retry = None;
        for d in &self.downloads {
            let (rect, _) =
                ui.allocate_exact_size(Vec2::new(ui.available_width(), 58.0), egui::Sense::hover());
            ui.painter().rect_filled(rect, 8.0, widgets::white_alpha(6));
            let art = Rect::from_min_size(rect.min + Vec2::new(7.0, 7.0), Vec2::splat(44.0));
            draw_cover(ui, &d.track, art, None);
            let x = art.right() + 12.0;
            fit_text(
                ui.painter(),
                Pos2::new(x, rect.top() + 20.0),
                egui::Align2::LEFT_CENTER,
                &d.track.title,
                FontId::new(14.0, egui::FontFamily::Name("bold".into())),
                WHITE,
                rect.right() - 90.0 - x,
            );
            let (status, color) = match &d.state {
                DownloadState::Queued => ("Queued".to_owned(), MUTED),
                DownloadState::Running(f) if *f >= 1.0 => ("Converting…".to_owned(), MUTED),
                DownloadState::Running(f) if *f > 0.0 => {
                    (format!("Downloading · {}%", (f * 100.0) as u32), MUTED)
                }
                DownloadState::Running(_) => ("Downloading".to_owned(), MUTED),
                DownloadState::Failed(e) => {
                    (format!("Failed · {e}"), Color32::from_rgb(232, 120, 110))
                }
            };
            fit_text(
                ui.painter(),
                Pos2::new(x, rect.top() + 39.0),
                egui::Align2::LEFT_CENTER,
                &status,
                FontId::proportional(12.5),
                color,
                rect.right() - 90.0 - x,
            );
            if let DownloadState::Running(f) = d.state {
                let bar = Rect::from_min_size(
                    Pos2::new(x, rect.bottom() - 6.0),
                    Vec2::new(rect.right() - 90.0 - x, 3.0),
                );
                ui.painter().rect_filled(bar, 1.5, widgets::white_alpha(25));
                ui.painter().rect_filled(
                    Rect::from_min_size(bar.min, Vec2::new(bar.width() * f, 3.0)),
                    1.5,
                    WHITE,
                );
            }
            region(
                ui,
                Rect::from_min_size(
                    Pos2::new(rect.right() - 76.0, rect.center().y - 16.0),
                    Vec2::new(72.0, 32.0),
                ),
                |u| {
                    u.with_layout(egui::Layout::right_to_left(Align::Center), |u| {
                        if widgets::button(u, Icon::Close, 30.0, false, "Cancel").clicked() {
                            cancel = Some(d.track.key().to_owned());
                        }
                        if matches!(d.state, DownloadState::Failed(_))
                            && widgets::button(u, Icon::Repeat, 30.0, false, "Retry").clicked()
                        {
                            retry = Some(d.track.clone());
                        }
                    });
                },
            );
            ui.add_space(6.0);
        }
        if let Some(key) = cancel {
            self.cancel_download(&key);
        }
        if let Some(t) = retry {
            self.download(t);
        }
        ui.add_space(14.0);
        ui.label(heading("Downloaded", 20.0));
    }
    fn load_account_playlists(&mut self) {
        self.account_playlists_request += 1;
        let request = self.account_playlists_request;
        if self.account.is_none() {
            self.account_playlists = Some(vec![]);
            return;
        }
        self.account_playlists_loading = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::AccountPlaylists(
                request,
                services::library_playlists().map_err(err),
            ));
        });
    }
    fn open_playlist_dialog(&mut self, dialog: PlaylistDialog) {
        self.playlist_error.clear();
        self.playlist_pending = false;
        if matches!(dialog, PlaylistDialog::AddTo { .. })
            && self.account.is_some()
            && self.account_playlists.is_none()
            && !self.account_playlists_loading
        {
            self.load_account_playlists();
        }
        self.playlist_dialog = Some(dialog);
    }
    /// Run one account-playlist change off the GUI thread; its outcome arrives as `PlaylistEdit`.
    /// False when another change is still running.
    fn edit_account_playlist(
        &mut self,
        job: impl FnOnce() -> anyhow::Result<PlaylistEdit> + Send + 'static,
    ) -> bool {
        if self.playlist_busy {
            self.message = "Another playlist change is still being saved".into();
            return false;
        }
        self.playlist_busy = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::PlaylistEdited(job().map_err(err)));
        });
        true
    }
    /// Like `edit_account_playlist`, for a change the open dialog asked for.
    fn dialog_account_job(
        &mut self,
        job: impl FnOnce() -> anyhow::Result<PlaylistEdit> + Send + 'static,
    ) {
        if self.edit_account_playlist(job) {
            self.playlist_pending = true;
            self.playlist_error.clear();
        }
    }
    fn playlist_edited(&mut self, result: Result<PlaylistEdit, String>) {
        self.playlist_busy = false;
        let pending = std::mem::take(&mut self.playlist_pending);
        let edit = match result {
            Ok(edit) => edit,
            Err(e) => {
                if pending && self.playlist_dialog.is_some() {
                    self.playlist_error = e.clone();
                }
                self.message = e;
                return;
            }
        };
        if pending {
            self.playlist_dialog = None;
            self.playlist_error.clear();
        }
        let open = self.collection.as_ref().and_then(|c| c.playlist_id.clone());
        match edit {
            PlaylistEdit::Created { title, seeded } => {
                self.message = if seeded {
                    format!("Created “{title}” with the song")
                } else {
                    format!("Created “{title}”")
                };
                self.load_account_playlists();
            }
            PlaylistEdit::Deleted { id } => {
                self.message = "Playlist deleted".into();
                if open.as_deref() == Some(id.as_str()) {
                    self.collection = None;
                    self.page = Page::Library;
                }
                if let Some(list) = &mut self.account_playlists {
                    list.retain(|p| p.browse_id.as_deref() != Some(&format!("VL{id}")));
                }
            }
            PlaylistEdit::Renamed { id, name } => {
                self.message = format!("Renamed to “{name}”");
                if open.as_deref() == Some(id.as_str()) {
                    if let Some(c) = &mut self.collection {
                        c.title = name.clone();
                    }
                }
                if let Some(list) = &mut self.account_playlists {
                    for p in list {
                        if p.browse_id.as_deref() == Some(&format!("VL{id}")) {
                            p.title = name.clone();
                        }
                    }
                }
            }
            PlaylistEdit::Added { id, track, set_id } => {
                self.message = format!("Added “{}”", track.title);
                if open.as_deref() == Some(id.as_str()) {
                    if let Some(c) = &mut self.collection {
                        if let Some(set_id) = set_id {
                            c.set_ids.insert(track.id.clone(), set_id);
                        }
                        c.tracks.push(track);
                    }
                }
            }
            PlaylistEdit::Removed { id, video_id } => {
                self.message = "Removed from playlist".into();
                if open.as_deref() == Some(id.as_str()) {
                    if let Some(c) = &mut self.collection {
                        if let Some(i) = c.tracks.iter().position(|t| t.id == video_id) {
                            c.tracks.remove(i);
                        }
                        c.set_ids.remove(&video_id);
                    }
                }
            }
            PlaylistEdit::Saved { id, saved } => {
                self.message = if saved {
                    "Saved to your library".into()
                } else {
                    "Removed from your library".into()
                };
                if open.as_deref() == Some(id.as_str()) {
                    if let Some(c) = &mut self.collection {
                        c.saved = Some(saved);
                    }
                }
                self.load_account_playlists();
            }
            PlaylistEdit::Privacy { id, privacy } => {
                self.message = format!("Playlist is now {}", privacy_label(&privacy));
                if open.as_deref() == Some(id.as_str()) {
                    if let Some(c) = &mut self.collection {
                        c.privacy = Some(privacy);
                    }
                }
            }
            PlaylistEdit::Described { id, description } => {
                self.message = "Description saved".into();
                if open.as_deref() == Some(id.as_str()) {
                    if let Some(c) = &mut self.collection {
                        c.description = description;
                    }
                }
            }
            PlaylistEdit::Reordered(collection) => {
                self.message = "Playlist order saved".into();
                if collection.playlist_id.is_some() && collection.playlist_id == open {
                    self.collection = Some(*collection);
                }
            }
        }
    }
    /// Account playlist `id`'s entry for `video_id` one step up or down, in the saved playlist.
    fn move_account_entry(&mut self, id: String, video_id: String, up: bool) {
        self.edit_account_playlist(move || {
            let mut all = services::playlist_all(&format!("VL{id}"))?;
            let from = all
                .tracks
                .iter()
                .position(|t| t.id == video_id)
                .ok_or_else(|| anyhow::anyhow!("That song is no longer in the playlist"))?;
            let to = step_target(all.tracks.len(), from, up).ok_or_else(|| {
                anyhow::anyhow!(if up {
                    "Already at the top"
                } else {
                    "Already at the bottom"
                })
            })?;
            let current = set_id_order(&all.tracks, &all.set_ids)
                .ok_or_else(|| anyhow::anyhow!("This playlist can't be reordered"))?;
            let target = moved(&current, from, to);
            services::reorder_playlist(&id, &current, &target)?;
            all.tracks = moved(&all.tracks, from, to);
            Ok(PlaylistEdit::Reordered(Box::new(all)))
        });
    }
    /// Run `action` over a playlist's tracks. `browse_id` is set when `tracks` is only the first
    /// page, so the whole playlist is fetched first, off the GUI thread.
    fn bulk(
        &mut self,
        action: PlaylistBulk,
        origin: &str,
        tracks: Vec<Track>,
        browse_id: Option<String>,
    ) {
        let Some(id) = browse_id else {
            self.apply_bulk(action, tracks, origin);
            return;
        };
        if self.bulk_loading {
            return;
        }
        self.bulk_loading = true;
        let (tx, origin) = (self.tx.clone(), origin.to_owned());
        std::thread::spawn(move || {
            let r = services::playlist_all(&id).map(|c| c.tracks).map_err(err);
            let _ = tx.send(Event::BulkTracks(action, origin, r));
        });
    }
    fn apply_bulk(&mut self, action: PlaylistBulk, mut tracks: Vec<Track>, origin: &str) {
        if tracks.is_empty() {
            self.message = "This playlist is empty".into();
            return;
        }
        match action {
            PlaylistBulk::Play => self.start_tracks(tracks, 0, origin),
            PlaylistBulk::Shuffle => {
                tracks.shuffle(&mut rand::rng());
                self.start_tracks(tracks, 0, origin);
            }
            PlaylistBulk::Queue => {
                self.message = format!("Added {} songs to the queue", tracks.len());
                self.library.queue.extend(tracks);
                self.save();
            }
            PlaylistBulk::Download => {
                tracks.retain(is_youtube_track);
                if tracks.is_empty() {
                    self.message = "None of these songs can be downloaded".into();
                    return;
                }
                let n = tracks.len();
                for t in tracks {
                    self.download(t);
                }
                self.message = format!("Downloading {n} songs");
            }
        }
    }
    /// Play, Shuffle, Add to queue and Download all for a playlist page.
    fn bulk_buttons(
        &mut self,
        ui: &mut egui::Ui,
        origin: &str,
        tracks: &[Track],
        browse_id: Option<String>,
    ) {
        let enabled = !tracks.is_empty() && !self.bulk_loading;
        for (icon, label, action) in [
            (Icon::Play, "Play", PlaylistBulk::Play),
            (Icon::Shuffle, "Shuffle", PlaylistBulk::Shuffle),
            (Icon::AddToQueue, "Add to queue", PlaylistBulk::Queue),
            (Icon::Download, "Download all", PlaylistBulk::Download),
        ] {
            if icon_action(ui, enabled, icon, label).clicked() {
                self.bulk(action, origin, tracks.to_vec(), browse_id.clone());
            }
        }
        if self.bulk_loading {
            ui.spinner();
        }
    }
    /// A local playlist removed: forget it in the history and leave its page.
    fn local_playlist_deleted(&mut self) {
        self.nav.retain(|p| !matches!(p, Page::Playlist(_)));
        if matches!(self.page, Page::Playlist(_)) {
            self.page = Page::Library;
        }
    }
    /// "Add to playlist…", which opens the dialog that holds the choices.
    fn playlist_menu(&mut self, ui: &mut egui::Ui, track: &Track) {
        if ui.button("Add to playlist…").clicked() {
            self.open_playlist_dialog(PlaylistDialog::AddTo {
                track: track.clone(),
            });
            ui.close();
        }
    }
    /// Entries that act on the playlist the row is in: remove and move.
    fn playlist_row_menu(&mut self, ui: &mut egui::Ui, track: &Track) {
        let owned = self
            .collection
            .as_ref()
            .filter(|c| self.page == Page::Collection && c.editable)
            .and_then(|c| {
                let position = c.tracks.iter().position(|t| t.id == track.id)?;
                let has_next = position + 1 < c.tracks.len() || c.continuation.is_some();
                Some((
                    c.playlist_id.clone()?,
                    c.set_ids.get(&track.id)?.clone(),
                    position > 0,
                    has_next,
                ))
            });
        if let Some((id, set_id, has_previous, has_next)) = owned {
            if ui.button("Remove from this playlist").clicked() {
                let video_id = track.id.clone();
                let id = id.clone();
                self.edit_account_playlist(move || {
                    services::remove_from_playlist(&id, &video_id, &set_id)?;
                    Ok(PlaylistEdit::Removed { id, video_id })
                });
                ui.close();
            }
            for (label, enabled, up) in [
                ("Move up", has_previous, true),
                ("Move down", has_next, false),
            ] {
                if ui
                    .add_enabled(enabled && !self.playlist_busy, egui::Button::new(label))
                    .clicked()
                {
                    self.move_account_entry(id.clone(), track.id.clone(), up);
                    ui.close();
                }
            }
        }
        if let Page::Playlist(i) = self.page {
            let Some(p) = self.library.playlists.get(i) else {
                return;
            };
            let len = p.tracks.len();
            let Some(position) = p.tracks.iter().position(|t| t.key() == track.key()) else {
                return;
            };
            let mut changed = false;
            for (label, target) in [
                ("Move up", step_target(len, position, true)),
                ("Move down", step_target(len, position, false)),
            ] {
                if ui
                    .add_enabled(target.is_some(), egui::Button::new(label))
                    .clicked()
                {
                    if let Some(to) = target {
                        changed |= self.library.move_in_playlist(i, position, to);
                    }
                    ui.close();
                }
            }
            if ui.button("Remove from playlist").clicked() {
                changed |= self.library.remove_from_playlist(i, track.key());
                ui.close();
            }
            if changed {
                self.save();
            }
        }
    }
    /// The signed-in account's playlists, as cards on the Library page.
    fn account_playlists_view(&mut self, ui: &mut egui::Ui) {
        if self.account.is_none() {
            ui.label(
                RichText::new("Sign in to YouTube Music to see and edit your playlists.")
                    .color(MUTED),
            );
            return;
        }
        if self.account_playlists.is_none() && !self.account_playlists_loading {
            self.load_account_playlists();
        }
        ui.horizontal(|ui| {
            ui.label(heading("YouTube playlists", 20.0));
            ui.add_space(8.0);
            if icon_action(ui, !self.playlist_busy, Icon::Plus, "New playlist").clicked() {
                self.open_playlist_dialog(PlaylistDialog::Create {
                    name: String::new(),
                    privacy: "PRIVATE".into(),
                    seed: None,
                });
            }
            if icon_action(
                ui,
                !self.account_playlists_loading,
                Icon::Refresh,
                "Refresh playlists",
            )
            .clicked()
            {
                self.load_account_playlists();
            }
            if self.account_playlists_loading || self.playlist_busy {
                ui.spinner();
            }
        });
        ui.add_space(10.0);
        let Some(list) = self.account_playlists.clone() else {
            return;
        };
        if list.is_empty() {
            ui.label(RichText::new("No playlists in your library yet").color(MUTED));
            return;
        }
        let mut open = None;
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = Vec2::new(16.0, 16.0);
            for (i, p) in list.iter().enumerate() {
                ui.push_id(("account-playlist", i), |ui| {
                    let (rect, response) =
                        ui.allocate_exact_size(Vec2::new(160.0, 212.0), egui::Sense::click());
                    let art = Rect::from_min_size(rect.min, Vec2::splat(160.0));
                    draw_image(ui, &p.thumbnail, art, None);
                    if response.hovered() {
                        ui.painter()
                            .rect_filled(art, 8.0, Color32::from_black_alpha(40));
                    }
                    fit_text(
                        ui.painter(),
                        Pos2::new(rect.left(), art.bottom() + 16.0),
                        egui::Align2::LEFT_CENTER,
                        &p.title,
                        FontId::new(14.0, egui::FontFamily::Name("bold".into())),
                        WHITE,
                        rect.width(),
                    );
                    fit_text(
                        ui.painter(),
                        Pos2::new(rect.left(), art.bottom() + 36.0),
                        egui::Align2::LEFT_CENTER,
                        &p.subtitle,
                        FontId::proportional(12.5),
                        MUTED,
                        rect.width(),
                    );
                    if response.clicked() {
                        open = Some(p.clone());
                    }
                });
            }
        });
        if let Some(p) = open {
            self.open_item(p);
        }
    }
    /// The playlist dialogs: add a track, create, rename, describe and confirm-delete.
    fn playlist_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.playlist_dialog.take() else {
            return;
        };
        let busy = self.playlist_busy;
        let signed_in = self.account.is_some();
        let account = self.account_playlists.clone();
        let locals: Vec<(String, usize)> = self
            .library
            .playlists
            .iter()
            .map(|p| (p.name.clone(), p.tracks.len()))
            .collect();
        let error = self.playlist_error.clone();
        let title = match &dialog {
            PlaylistDialog::AddTo { .. } => "Add to playlist",
            PlaylistDialog::Create { .. } => "New playlist",
            PlaylistDialog::Rename { .. } | PlaylistDialog::RenameLocal { .. } => "Rename playlist",
            PlaylistDialog::Description { .. } => "Edit description",
            PlaylistDialog::Delete { .. } | PlaylistDialog::DeleteLocal { .. } => "Delete playlist",
        };
        let mut act = DialogAct::None;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(340.0);
                match &mut dialog {
                    PlaylistDialog::AddTo { track } => {
                        let who = if track.artist.is_empty() {
                            track.title.clone()
                        } else {
                            format!("{} — {}", track.title, track.artist)
                        };
                        ui.label(RichText::new(who).color(MUTED));
                    }
                    PlaylistDialog::Create { seed, .. } => {
                        let to_account =
                            create_target(signed_in, seed.as_ref()) == CreateTarget::Account;
                        ui.label(
                            RichText::new(if to_account {
                                "Saved to your YouTube Music account"
                            } else {
                                "Saved on this computer"
                            })
                            .color(MUTED),
                        );
                    }
                    PlaylistDialog::Rename { current, .. }
                    | PlaylistDialog::RenameLocal { current, .. } => {
                        ui.label(RichText::new(current.as_str()).color(MUTED));
                    }
                    PlaylistDialog::Delete { title, .. } => {
                        ui.label(format!(
                            "“{title}” will be removed from your YouTube Music account, on every device. The songs in it are not deleted."
                        ));
                    }
                    PlaylistDialog::DeleteLocal { title, .. } => {
                        ui.label(format!(
                            "“{title}” will be removed from this computer. The songs in it are not deleted."
                        ));
                    }
                    PlaylistDialog::Description { .. } => {}
                }
                if !error.is_empty() {
                    ui.add_space(4.0);
                    ui.label(RichText::new(&error).color(ERROR));
                }
                ui.add_space(6.0);
                match &mut dialog {
                    PlaylistDialog::AddTo { track } => {
                        if ui
                            .add_enabled(!busy, egui::Button::new("+  New playlist"))
                            .clicked()
                        {
                            act = DialogAct::New(track.clone());
                        }
                        let can_use_account = signed_in && is_youtube_track(track);
                        if signed_in && !can_use_account {
                            ui.add_space(4.0);
                            ui.label(
                                RichText::new("This track is not from YouTube Music, so it can only go in a playlist kept here.")
                                    .size(12.5)
                                    .color(MUTED),
                            );
                        }
                        let items: Vec<(String, String, String)> = if can_use_account {
                            account
                                .iter()
                                .flatten()
                                .filter_map(|p| {
                                    let id = p.browse_id.as_deref()?.strip_prefix("VL")?;
                                    Some((id.to_owned(), p.title.clone(), p.subtitle.clone()))
                                })
                                .collect()
                        } else {
                            vec![]
                        };
                        ui.add_space(4.0);
                        if items.is_empty() && locals.is_empty() {
                            if can_use_account && account.is_none() {
                                ui.spinner();
                            } else {
                                ui.label(
                                    RichText::new(if !signed_in {
                                        "No playlists on this computer yet. Sign in to see the ones on your account."
                                    } else if !can_use_account {
                                        "No playlists on this computer yet. Make one and this track goes straight into it."
                                    } else {
                                        "No playlists yet. Make one and this song goes straight into it."
                                    })
                                    .size(12.5)
                                    .color(MUTED),
                                );
                            }
                        } else {
                            egui::ScrollArea::vertical()
                                .max_height(320.0)
                                .show(ui, |ui| {
                                    let width = ui.available_width();
                                    let entry = |ui: &mut egui::Ui, name: &str, sub: &str| {
                                        let clicked = ui
                                            .add_enabled(
                                                !busy,
                                                egui::Button::new(truncate(name, 44))
                                                    .min_size(Vec2::new(width, 0.0)),
                                            )
                                            .clicked();
                                        ui.label(RichText::new(sub).size(12.0).color(MUTED));
                                        clicked
                                    };
                                    if !items.is_empty() {
                                        section_heading(ui, "On your account");
                                        for (id, name, sub) in &items {
                                            if entry(ui, name, sub) {
                                                act = DialogAct::PickAccount(
                                                    id.clone(),
                                                    track.clone(),
                                                );
                                            }
                                        }
                                    }
                                    if !locals.is_empty() {
                                        section_heading(ui, "On this computer");
                                        for (i, (name, count)) in locals.iter().enumerate() {
                                            if entry(ui, name, &format!("{count} songs")) {
                                                act = DialogAct::PickLocal(i, track.clone());
                                            }
                                        }
                                    }
                                });
                        }
                    }
                    PlaylistDialog::Create {
                        name,
                        privacy,
                        seed,
                    } => {
                        let r = ui.add(
                            egui::TextEdit::singleline(name).margin(TEXT_MARGIN)
                                .hint_text("Playlist name")
                                .desired_width(f32::INFINITY),
                        );
                        r.request_focus();
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            act = DialogAct::Submit;
                        }
                        if create_target(signed_in, seed.as_ref()) == CreateTarget::Account {
                            section_heading(ui, "Who can see it");
                            ui.horizontal(|ui| {
                                for (value, label) in services::PLAYLIST_PRIVACY {
                                    ui.radio_value(privacy, value.to_owned(), label);
                                }
                            });
                        }
                        if let Some(t) = seed {
                            ui.label(
                                RichText::new(format!("Starts with “{}”", t.title)).color(MUTED),
                            );
                        }
                    }
                    PlaylistDialog::Rename { name, .. }
                    | PlaylistDialog::RenameLocal { name, .. } => {
                        let r =
                            ui.add(egui::TextEdit::singleline(name).margin(TEXT_MARGIN).desired_width(f32::INFINITY));
                        r.request_focus();
                        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                            act = DialogAct::Submit;
                        }
                    }
                    PlaylistDialog::Description { text, .. } => {
                        ui.add(
                            egui::TextEdit::multiline(text).margin(TEXT_MARGIN)
                                .hint_text("Description")
                                .desired_rows(4)
                                .desired_width(f32::INFINITY),
                        )
                        .request_focus();
                    }
                    PlaylistDialog::Delete { .. } | PlaylistDialog::DeleteLocal { .. } => {}
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let confirm = match &dialog {
                        PlaylistDialog::AddTo { .. } => None,
                        PlaylistDialog::Create { name, .. } => {
                            Some(("Create playlist", !name.trim().is_empty()))
                        }
                        PlaylistDialog::Rename { name, current, .. }
                        | PlaylistDialog::RenameLocal { name, current, .. } => Some((
                            "Rename",
                            !name.trim().is_empty() && name.trim() != current.trim(),
                        )),
                        PlaylistDialog::Description { .. } => Some(("Save", true)),
                        PlaylistDialog::Delete { .. } | PlaylistDialog::DeleteLocal { .. } => {
                            Some(("Delete", true))
                        }
                    };
                    if let Some((label, enabled)) = confirm {
                        if ui
                            .add_enabled(enabled && !busy, egui::Button::new(label))
                            .clicked()
                        {
                            act = DialogAct::Submit;
                        }
                    }
                    if let PlaylistDialog::Create {
                        seed: Some(track), ..
                    } = &dialog
                    {
                        if ui.button("Back").clicked() {
                            act = DialogAct::Back(track.clone());
                        }
                    }
                    let close = if matches!(dialog, PlaylistDialog::AddTo { .. }) {
                        "Close"
                    } else {
                        "Cancel"
                    };
                    if ui.button(close).clicked() {
                        act = DialogAct::Cancel;
                    }
                    if busy {
                        ui.spinner();
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            act = DialogAct::Cancel;
        }
        match act {
            DialogAct::None => {}
            DialogAct::Cancel => {
                self.playlist_pending = false;
                self.playlist_error.clear();
                return;
            }
            DialogAct::Back(track) => {
                self.playlist_error.clear();
                dialog = PlaylistDialog::AddTo { track };
            }
            DialogAct::New(track) => {
                self.playlist_error.clear();
                dialog = PlaylistDialog::Create {
                    name: String::new(),
                    privacy: "PRIVATE".into(),
                    seed: Some(track),
                };
            }
            DialogAct::PickAccount(id, track) => {
                self.dialog_account_job(move || {
                    let set_id = services::add_to_playlist(&id, &track.id)?;
                    Ok(PlaylistEdit::Added { id, track, set_id })
                });
            }
            DialogAct::PickLocal(index, track) => {
                if let Some(name) = self.library.playlists.get(index).map(|p| p.name.clone()) {
                    if self.library.add_to_playlist(index, track) {
                        self.message = format!("Added to {name}");
                        self.save();
                    } else {
                        self.message = format!("Already in {name}");
                    }
                }
                return;
            }
            DialogAct::Submit => match self.submit_playlist_dialog(dialog) {
                Some(keep) => dialog = keep,
                None => return,
            },
        }
        self.playlist_dialog = Some(dialog);
    }
    /// The dialog's confirm action. `Some` keeps it open (a failure, or an account change that is
    /// still running); `None` closes it.
    fn submit_playlist_dialog(&mut self, dialog: PlaylistDialog) -> Option<PlaylistDialog> {
        match dialog {
            PlaylistDialog::AddTo { .. } => Some(dialog),
            PlaylistDialog::Create {
                name,
                privacy,
                seed,
            } => {
                if name.trim().is_empty() {
                    self.playlist_error = "A playlist needs a name".into();
                } else if create_target(self.account.is_some(), seed.as_ref())
                    == CreateTarget::Account
                {
                    let (n, p, s) = (name.clone(), privacy.clone(), seed.clone());
                    self.dialog_account_job(move || {
                        let seeds: Vec<String> = s.iter().map(|t| t.id.clone()).collect();
                        services::create_playlist(&n, &p, &seeds)?;
                        Ok(PlaylistEdit::Created {
                            title: n.trim().to_owned(),
                            seeded: !seeds.is_empty(),
                        })
                    });
                } else {
                    match self.library.create_playlist(&name, seed.clone()) {
                        Ok(index) => {
                            self.save();
                            self.message = format!("Created “{}”", name.trim());
                            self.navigate(Page::Playlist(index));
                            return None;
                        }
                        Err(e) => self.playlist_error = format!("{e:#}"),
                    }
                }
                Some(PlaylistDialog::Create {
                    name,
                    privacy,
                    seed,
                })
            }
            PlaylistDialog::Rename { id, name, current } => {
                let (i, n) = (id.clone(), name.clone());
                self.dialog_account_job(move || {
                    services::rename_playlist(&i, &n)?;
                    Ok(PlaylistEdit::Renamed {
                        id: i,
                        name: n.trim().to_owned(),
                    })
                });
                Some(PlaylistDialog::Rename { id, name, current })
            }
            PlaylistDialog::RenameLocal {
                index,
                name,
                current,
            } => match self.library.rename_playlist(index, &name) {
                Ok(()) => {
                    self.save();
                    self.message = format!("Renamed to “{}”", name.trim());
                    None
                }
                Err(e) => {
                    self.playlist_error = format!("{e:#}");
                    Some(PlaylistDialog::RenameLocal {
                        index,
                        name,
                        current,
                    })
                }
            },
            PlaylistDialog::Description { id, text } => {
                let (i, t) = (id.clone(), text.clone());
                self.dialog_account_job(move || {
                    services::set_playlist_description(&i, &t)?;
                    Ok(PlaylistEdit::Described {
                        id: i,
                        description: t,
                    })
                });
                Some(PlaylistDialog::Description { id, text })
            }
            PlaylistDialog::Delete { id, title } => {
                let i = id.clone();
                self.dialog_account_job(move || {
                    services::delete_playlist(&i)?;
                    Ok(PlaylistEdit::Deleted { id: i })
                });
                Some(PlaylistDialog::Delete { id, title })
            }
            PlaylistDialog::DeleteLocal { index, title } => {
                if self.library.delete_playlist(index).is_some() {
                    self.local_playlist_deleted();
                    self.save();
                    self.message = format!("Deleted “{title}”");
                }
                None
            }
        }
    }
    /// Tray menu actions, its labels, and close-to-tray.
    fn tray_step(&mut self, ctx: &egui::Context) {
        #[cfg(windows)]
        {
            let Some(tray) = self.tray.as_mut() else {
                return;
            };
            let playing = !self.status.paused && !self.status.ended;
            tray.update(
                self.current
                    .as_ref()
                    .map(|t| format!("{} · {}", t.title, t.artist)),
                playing,
            );
            for action in tray.poll() {
                match action {
                    crate::tray::TrayAction::Toggle => self.toggle(),
                    crate::tray::TrayAction::Next => self.next(false),
                    crate::tray::TrayAction::Previous => self.previous(),
                    crate::tray::TrayAction::Open => {
                        if let Some(hwnd) = self.hwnd {
                            crate::tray::restore_window(hwnd);
                        }
                    }
                    crate::tray::TrayAction::Settings => {
                        if let Some(hwnd) = self.hwnd {
                            crate::tray::restore_window(hwnd);
                        }
                        self.expanded = false;
                        self.navigate(Page::Settings);
                    }
                    crate::tray::TrayAction::Quit => {
                        self.quitting = true;
                        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
                    }
                }
            }
            if self.library.close_to_tray
                && !self.quitting
                && ctx.input(|i| i.viewport().close_requested())
            {
                // Playback carries on; the tray icon or its Open item brings the window back.
                if let Some(hwnd) = self.hwnd {
                    ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
                    crate::tray::hide_window(hwnd);
                }
            }
        }
        #[cfg(not(windows))]
        let _ = ctx;
    }
    /// Settings, grouped as upstream's `DesktopSettingsSection`s: titled groups of rows, each a
    /// title and explanation on the left and its control on the right, filtered by a search box.
    fn settings_view(&mut self, ui: &mut egui::Ui) {
        // Centre the 820 px column in the page: a gutter as wide as the space left beside it.
        let gutter = ((ui.available_width() - 820.0) / 2.0).max(0.0);
        ui.horizontal_top(|ui| {
            ui.add_space(gutter);
            ui.vertical(|ui| {
                ui.set_max_width(820.0);
                self.settings_body(ui);
            });
        });
    }

    fn settings_body(&mut self, ui: &mut egui::Ui) {
        self.load_lyrics_keys();
        self.load_audio_devices();
        ui.horizontal(|ui| {
            ui.label(heading("Settings", 26.0));
            ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                ui.add_space(14.0);
                egui::Frame::new()
                    .fill(Color32::from_rgb(40, 42, 38))
                    .stroke(Stroke::new(1.0_f32, Color32::from_gray(62)))
                    .corner_radius(8.0)
                    .inner_margin(egui::Margin::symmetric(10, 6))
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
                            widgets::paint(ui.painter(), Icon::Search, r, MUTED);
                            muted_hint(
                                ui,
                                egui::TextEdit::singleline(&mut self.settings_query)
                                    .hint_text(RichText::new("Search settings").color(MUTED))
                                    .frame(bare_text_frame())
                                    .desired_width(220.0),
                            );
                        });
                    });
            });
        });
        ui.add_space(12.0);
        let query = self.settings_query.trim().to_lowercase();
        let mut shown = false;

        // Playback
        let rows = [
            "Autoplay similar music",
            "Crossfade",
            "Playback speed",
            "Keep playing in the tray",
        ];
        if let Some(q) = group_query(&query, "Playback", &rows) {
            shown = true;
            settings_group(ui, "Playback", |ui| {
                if hit(&q, rows[0]) {
                    let mut on = self.library.autoplay;
                    if toggle_row(
                        ui,
                        rows[0],
                        "When the queue ends, keep playing songs like the last one",
                        &mut on,
                    ) {
                        self.library.autoplay = on;
                        self.save();
                    }
                }
                if hit(&q, rows[1]) {
                    setting_row(
                        ui,
                        rows[1],
                        "Blend the end of a song into the next. The next song is prepared while this one plays.",
                        |ui| {
                            let seconds = self.library.crossfade_seconds;
                            ui.label(
                                RichText::new(if seconds == 0 {
                                    "Off".to_owned()
                                } else {
                                    format!("{seconds} s")
                                })
                                .color(MUTED),
                            );
                            let mut value = seconds;
                            if ui
                                .add(egui::Slider::new(&mut value, 0..=12).show_value(false))
                                .changed()
                            {
                                self.library.crossfade_seconds = value;
                                self.save();
                            }
                        },
                    );
                }
                if hit(&q, rows[2]) {
                    setting_row(
                        ui,
                        rows[2],
                        "Applies to the song that is playing now",
                        |ui| {
                            if (self.library.speed - 1.0).abs() > 0.001
                                && icon_action(ui, true, Icon::Reset, "Reset to default").clicked()
                            {
                                self.library.speed = 1.0;
                                let _ = self.player.speed(1.0);
                                self.save();
                            }
                            ui.label(
                                RichText::new(format!("{:.2}×", self.library.speed)).color(MUTED),
                            );
                            if ui
                                .add(
                                    egui::Slider::new(&mut self.library.speed, 0.5..=2.0)
                                        .step_by(0.05)
                                        .show_value(false),
                                )
                                .changed()
                            {
                                let _ = self.player.speed(self.library.speed);
                                self.save();
                            }
                        },
                    );
                }
                #[cfg(windows)]
                if self.tray.is_some() && hit(&q, rows[3]) {
                    let mut on = self.library.close_to_tray;
                    if toggle_row(
                        ui,
                        "Keep playing in the tray",
                        "Closing the window leaves YTDugem playing in the notification area",
                        &mut on,
                    ) {
                        self.library.close_to_tray = on;
                        self.save();
                    }
                }
            });
        }

        // Audio output
        let rows = ["Output device", "WASAPI exclusive mode"];
        if let Some(q) = group_query(&query, "Audio output", &rows) {
            shown = true;
            settings_group(ui, "Audio output", |ui| {
                if hit(&q, rows[0]) {
                    let subtitle = if self.audio_devices_loading {
                        "Reading devices from mpv…"
                    } else {
                        "Applies when the next song starts"
                    };
                    setting_row(ui, rows[0], subtitle, |ui| {
                        if icon_action(
                            ui,
                            !self.audio_devices_loading,
                            Icon::Refresh,
                            "Refresh devices",
                        )
                        .clicked()
                        {
                            self.audio_devices_loaded = false;
                            self.load_audio_devices();
                        }
                        let selected = self.library.audio.output_device.clone();
                        egui::ComboBox::from_id_salt("audio-output-device")
                            .width(240.0)
                            .selected_text(if selected.is_empty() {
                                "System default".to_owned()
                            } else {
                                self.audio_devices
                                    .iter()
                                    .find(|(id, _)| *id == selected)
                                    .map_or("Unavailable device".to_owned(), |(_, n)| n.clone())
                            })
                            .show_ui(ui, |ui| {
                                if ui
                                    .selectable_label(selected.is_empty(), "System default")
                                    .clicked()
                                {
                                    self.library.audio.output_device.clear();
                                    self.save();
                                }
                                for (id, name) in self.audio_devices.clone() {
                                    if ui.selectable_label(selected == id, name).clicked() {
                                        self.library.audio.output_device = id;
                                        self.save();
                                    }
                                }
                            });
                    });
                }
                #[cfg(windows)]
                if hit(&q, rows[1]) {
                    let mut on = self.library.audio.wasapi_exclusive;
                    if toggle_row(
                        ui,
                        rows[1],
                        "Bypass the Windows mixer on a wasapi/ device; applies to the next song",
                        &mut on,
                    ) {
                        self.library.audio.wasapi_exclusive = on;
                        self.save();
                    }
                }
            });
        }

        // Equalizer
        let rows = ["Equalizer", "Preset", "Stereo balance"];
        if let Some(q) = group_query(&query, "Equalizer", &rows) {
            shown = true;
            settings_group(ui, "Equalizer", |ui| {
                if hit(&q, rows[0]) {
                    let mut on = self.library.audio.equalizer_enabled;
                    if toggle_row(
                        ui,
                        rows[0],
                        "Seven bands and balance, applied to the song that is playing",
                        &mut on,
                    ) {
                        self.library.audio.equalizer_enabled = on;
                        self.library.audio.sanitize();
                        self.save();
                        self.apply_audio_settings();
                    }
                }
                if hit(&q, rows[1]) {
                    setting_row(ui, rows[1], "", |ui| {
                        if icon_action(ui, true, Icon::Reset, "Reset to default").clicked() {
                            self.library.audio.equalizer_enabled = false;
                            self.library.audio.equalizer_bands = vec![0.0; 7];
                            self.library.audio.stereo_balance = 0.0;
                            self.save();
                            self.apply_audio_settings();
                        }
                        let current = store::audio_presets()
                            .iter()
                            .find(|(_, b)| self.library.audio.equalizer_bands.as_slice() == *b)
                            .map_or("Custom", |(name, _)| *name);
                        egui::ComboBox::from_id_salt("equalizer-preset")
                            .width(180.0)
                            .selected_text(current)
                            .show_ui(ui, |ui| {
                                for (name, bands) in store::audio_presets() {
                                    if ui.selectable_label(current == *name, *name).clicked() {
                                        self.library.audio.equalizer_bands = bands.to_vec();
                                        self.library.audio.equalizer_enabled = true;
                                        self.save();
                                        self.apply_audio_settings();
                                    }
                                }
                            });
                    });
                    self.equalizer_bands(ui);
                }
                if hit(&q, rows[2]) {
                    setting_row(ui, rows[2], "", |ui| {
                        let b = self.library.audio.stereo_balance;
                        ui.label(
                            RichText::new(if b.abs() < 0.01 {
                                "Centre".to_owned()
                            } else if b < 0.0 {
                                format!("L {:.0}%", -b * 100.0)
                            } else {
                                format!("R {:.0}%", b * 100.0)
                            })
                            .color(MUTED),
                        );
                        if ui
                            .add(
                                egui::Slider::new(
                                    &mut self.library.audio.stereo_balance,
                                    -1.0..=1.0,
                                )
                                .show_value(false),
                            )
                            .changed()
                        {
                            self.library.audio.sanitize();
                            self.save();
                            self.apply_audio_settings();
                        }
                    });
                }
            });
        }

        // Lyrics
        let rows = [
            "Lyrics lookup",
            "Translation language",
            "Providers",
            "API key",
        ];
        if let Some(q) = group_query(&query, "Lyrics", &rows) {
            shown = true;
            settings_group(ui, "Lyrics", |ui| {
                if hit(&q, rows[0]) {
                    let mut on = self.library.lyrics_enabled;
                    if toggle_row(
                        ui,
                        rows[0],
                        "Find timed lyrics for each song from the providers below",
                        &mut on,
                    ) {
                        self.library.lyrics_enabled = on;
                        self.save();
                        self.lyrics_settings_changed(true);
                    }
                }
                if hit(&q, rows[1]) {
                    setting_row(
                        ui,
                        rows[1],
                        "Used by the Translate button in the player",
                        |ui| {
                            let system = translate::system_language();
                            let name = |tag: &str| {
                                translate::LANGUAGES
                                    .iter()
                                    .find(|(t, _)| *t == tag)
                                    .map_or(tag.to_owned(), |(_, n)| (*n).to_owned())
                            };
                            let current = self.library.translation_language.clone();
                            let follows = format!("{} · follows Windows", name(&system));
                            let mut chosen = current.clone();
                            egui::ComboBox::from_id_salt("translation-language")
                                .width(240.0)
                                .selected_text(if current.is_empty() {
                                    follows.clone()
                                } else {
                                    name(&current)
                                })
                                .show_ui(ui, |ui| {
                                    ui.selectable_value(&mut chosen, String::new(), follows);
                                    for (tag, label) in translate::LANGUAGES {
                                        ui.selectable_value(&mut chosen, tag.to_owned(), label);
                                    }
                                });
                            if chosen != current {
                                self.library.translation_language = chosen;
                                self.save();
                                if self.lyrics_mode == LyricsMode::Translated {
                                    self.request_translation();
                                }
                            }
                        },
                    );
                }
                if hit(&q, rows[2]) {
                    setting_row(
                        ui,
                        rows[2],
                        "Tried from the top; untick one to skip it",
                        |ui| {
                            if icon_action(ui, true, Icon::Reset, "Reset to default").clicked() {
                                self.library.lyrics_order = store::lyrics_order_default();
                                self.library.lyrics_sources = store::lyrics_order_default();
                                self.library.lyrics_enabled = true;
                                self.save();
                                self.lyrics_settings_changed(true);
                            }
                        },
                    );
                    self.lyrics_provider_list(ui);
                }
                if hit(&q, rows[3]) {
                    self.lyrics_key_rows(ui);
                }
            });
        }

        // Downloads
        let rows = ["Download format", "Download folder"];
        if let Some(q) = group_query(&query, "Downloads", &rows) {
            shown = true;
            settings_group(ui, "Downloads", |ui| {
                if hit(&q, rows[0]) {
                    setting_row(
                        ui,
                        rows[0],
                        "Original keeps YouTube's audio as it is",
                        |ui| {
                            let current = services::DOWNLOAD_FORMATS
                                .iter()
                                .find(|(id, _)| *id == self.library.download_format)
                                .unwrap_or(&services::DOWNLOAD_FORMATS[0]);
                            let mut chosen = current.0.to_owned();
                            egui::ComboBox::from_id_salt("download-format")
                                .width(280.0)
                                .selected_text(current.1)
                                .show_ui(ui, |ui| {
                                    for (id, label) in services::DOWNLOAD_FORMATS {
                                        ui.selectable_value(&mut chosen, id.to_owned(), label);
                                    }
                                });
                            if chosen != self.library.download_format {
                                self.library.download_format = chosen;
                                self.save();
                            }
                        },
                    );
                }
                if hit(&q, rows[1]) {
                    let saved = self.library.download_dir.to_string_lossy() == self.download_path;
                    setting_row(
                        ui,
                        rows[1],
                        if saved { "" } else { "Not saved yet" },
                        |ui| {
                            if ui.add_enabled(!saved, egui::Button::new("Save")).clicked() {
                                self.library.download_dir = PathBuf::from(&self.download_path);
                                self.save();
                            }
                            ui.add(
                                egui::TextEdit::singleline(&mut self.download_path)
                                    .margin(TEXT_MARGIN)
                                    .desired_width(300.0),
                            );
                        },
                    );
                }
            });
        }

        // Integrations
        let rows = [
            "Discord Rich Presence",
            "Activity name",
            "Show YouTube Music button",
            "Show BitChord button",
            "System media controls",
        ];
        if let Some(q) = group_query(&query, "Integrations", &rows) {
            shown = true;
            settings_group(ui, "Integrations", |ui| {
                if hit(&q, rows[0]) && !DISCORD_READY {
                    setting_row(
                        ui,
                        rows[0],
                        "Show what you are listening to on your Discord profile",
                        |ui| {
                            ui.label(RichText::new("Coming soon").color(MUTED));
                        },
                    );
                } else if hit(&q, rows[0]) {
                    let status = if !self.library.discord_enabled {
                        "Show what you are listening to on your Discord profile".to_owned()
                    } else {
                        match self.discord.status() {
                            crate::discord::Status::Idle => {
                                "On · shown while a song plays".to_owned()
                            }
                            crate::discord::Status::Waiting => {
                                "On · waiting for Discord".to_owned()
                            }
                            crate::discord::Status::Connected => "On · connected".to_owned(),
                            crate::discord::Status::Error(e) => format!("On · {e}"),
                        }
                    };
                    let mut on = self.library.discord_enabled;
                    if toggle_row(ui, rows[0], &status, &mut on) {
                        self.library.discord_enabled = on;
                        self.save();
                        if !on {
                            self.discord.clear();
                        }
                    }
                }
                if DISCORD_READY && self.library.discord_enabled {
                    if hit(&q, rows[1]) {
                        setting_row(ui, rows[1], "Blank shows the song title", |ui| {
                            if ui
                                .add(
                                    egui::TextEdit::singleline(
                                        &mut self.library.discord_activity_name,
                                    )
                                    .margin(TEXT_MARGIN)
                                    .desired_width(240.0),
                                )
                                .changed()
                            {
                                self.save();
                            }
                        });
                    }
                    for i in 0..2 {
                        if hit(&q, rows[2 + i]) {
                            let mut on = self.library.discord_buttons[i];
                            if toggle_row(ui, rows[2 + i], "", &mut on) {
                                self.library.discord_buttons[i] = on;
                                self.save();
                            }
                        }
                    }
                }
                if hit(&q, rows[4]) {
                    setting_row(
                        ui,
                        rows[4],
                        "Media keys and the Windows volume flyout",
                        |ui| {
                            ui.label(
                                RichText::new(if self.integrations.available() {
                                    "Available"
                                } else {
                                    "Unavailable"
                                })
                                .color(MUTED),
                            );
                        },
                    );
                }
            });
        }

        if !shown {
            ui.add_space(30.0);
            ui.label(RichText::new("No settings match your search").color(MUTED));
        }
    }
    /// The seven equalizer bands as a graphic equalizer: vertical sliders side by side.
    fn equalizer_bands(&mut self, ui: &mut egui::Ui) {
        const BANDS: [&str; 7] = ["60", "150", "400", "1k", "2.5k", "6k", "14k"];
        ui.add_space(6.0);
        ui.horizontal(|ui| {
            ui.spacing_mut().slider_width = 120.0;
            let column = ((ui.available_width() - 20.0) / 7.0).clamp(48.0, 96.0);
            for (index, label) in BANDS.iter().enumerate() {
                ui.allocate_ui_with_layout(
                    Vec2::new(column, 170.0),
                    egui::Layout::top_down(Align::Center),
                    |ui| {
                        let mut gain = self.library.audio.equalizer_bands[index];
                        ui.label(RichText::new(format!("{gain:+.0}")).size(12.0).color(MUTED));
                        if ui
                            .add(
                                egui::Slider::new(&mut gain, -12.0..=12.0)
                                    .vertical()
                                    .show_value(false),
                            )
                            .on_hover_text(format!("{label} Hz · {gain:+.1} dB"))
                            .changed()
                        {
                            self.library.audio.equalizer_bands[index] = gain;
                            self.library.audio.equalizer_enabled = true;
                            self.save();
                            self.apply_audio_settings();
                        }
                        ui.label(RichText::new(*label).size(12.0).color(MUTED));
                    },
                );
            }
        });
        ui.add_space(6.0);
    }
    fn lyrics_provider_list(&mut self, ui: &mut egui::Ui) {
        let ordered = services::lyrics::ordered(&self.library.lyrics_order);
        let mut moved = None;
        egui::Frame::new()
            .fill(Color32::from_black_alpha(40))
            .corner_radius(8.0)
            .inner_margin(egui::Margin::symmetric(10, 6))
            .show(ui, |ui| {
                for (index, provider) in ordered.iter().enumerate() {
                    let mut enabled = self
                        .library
                        .lyrics_sources
                        .iter()
                        .any(|id| id == provider.id);
                    ui.horizontal(|ui| {
                        ui.set_min_height(30.0);
                        ui.label(
                            RichText::new(format!("{}", index + 1))
                                .size(12.0)
                                .color(MUTED),
                        );
                        if ui.checkbox(&mut enabled, provider.label).changed() {
                            if enabled {
                                self.library.lyrics_sources.push(provider.id.into());
                            } else {
                                self.library.lyrics_sources.retain(|id| id != provider.id);
                            }
                            self.save();
                            self.lyrics_settings_changed(true);
                        }
                        ui.add(
                            egui::Label::new(
                                RichText::new(provider.detail).size(12.5).color(MUTED),
                            )
                            .truncate(),
                        );
                        ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                            let down = index + 1 < ordered.len();
                            if ui
                                .add_enabled_ui(down, |u| {
                                    widgets::button(u, Icon::Collapse, 26.0, false, "Move down")
                                })
                                .inner
                                .clicked()
                            {
                                moved = Some((index, index + 1));
                            }
                            if ui
                                .add_enabled_ui(index > 0, |u| {
                                    widgets::button(u, Icon::Raise, 26.0, false, "Move up")
                                })
                                .inner
                                .clicked()
                            {
                                moved = Some((index, index - 1));
                            }
                        });
                    });
                }
            });
        if let Some((a, b)) = moved {
            let mut ids: Vec<_> = ordered.iter().map(|p| p.id.to_owned()).collect();
            ids.swap(a, b);
            self.library.lyrics_order = ids;
            self.save();
            self.lyrics_settings_changed(true);
        }
        ui.add_space(6.0);
    }
    fn lyrics_key_rows(&mut self, ui: &mut egui::Ui) {
        let key_ids: std::collections::BTreeSet<&str> = services::lyrics::PROVIDERS
            .iter()
            .filter_map(|provider| provider.key_id)
            .collect();
        for key in key_ids {
            let configured = self.lyric_keys.get(key).copied().unwrap_or(false);
            setting_row(
                ui,
                &format!("{key} API key"),
                if configured {
                    "Saved, protected by Windows"
                } else {
                    "Not set; providers that need it are skipped"
                },
                |ui| {
                    let input = self.lyric_key_inputs.entry(key.to_owned()).or_default();
                    let label = if input.trim().is_empty() && configured {
                        "Remove"
                    } else {
                        "Save"
                    };
                    let ready = !self.lyric_keys_busy && (configured || !input.trim().is_empty());
                    if ui.add_enabled(ready, egui::Button::new(label)).clicked() {
                        self.save_lyrics_key(key.to_owned());
                    }
                    let input = self.lyric_key_inputs.entry(key.to_owned()).or_default();
                    ui.add(
                        egui::TextEdit::singleline(input)
                            .margin(TEXT_MARGIN)
                            .password(true)
                            .hint_text(if configured {
                                "Paste to replace"
                            } else {
                                "Paste key"
                            })
                            .desired_width(220.0),
                    );
                },
            );
        }
    }
    /// Ctrl+K: open the search palette, or close it if it is already open.
    fn toggle_palette(&mut self) {
        if self.palette_open {
            self.palette_open = false;
            return;
        }
        self.palette_open = true;
        self.palette_focus = true;
        self.palette_query.clear();
        self.palette_suggestions.clear();
        self.palette_selected = None;
        self.palette_edited = None;
        self.palette_asked.clear();
        self.palette_loading = false;
        // Anything still in flight for an earlier query is now stale.
        self.palette_request += 1;
    }
    /// Search for `text` from the palette, as if typed into the search box.
    fn palette_submit(&mut self, text: String) {
        let text = text.trim().to_owned();
        if text.is_empty() {
            return;
        }
        self.palette_open = false;
        self.query = text;
        self.search();
    }
    /// Suggestions follow typing after a short pause, so a fast typist sends one request per
    /// pause rather than one per letter.
    fn palette_debounce(&mut self, ctx: &egui::Context) {
        const PAUSE: Duration = Duration::from_millis(250);
        let Some(edited) = self.palette_edited else {
            return;
        };
        let waited = edited.elapsed();
        if waited < PAUSE {
            ctx.request_repaint_after(PAUSE - waited);
            return;
        }
        self.palette_edited = None;
        let query = self.palette_query.trim().to_owned();
        if query == self.palette_asked {
            return;
        }
        self.palette_asked = query.clone();
        self.palette_request += 1;
        if query.is_empty() {
            self.palette_suggestions.clear();
            self.palette_loading = false;
            return;
        }
        self.palette_loading = true;
        let (id, tx) = (self.palette_request, self.tx.clone());
        std::thread::spawn(move || {
            let result = services::search_suggestions(&query).map_err(err);
            let _ = tx.send(Event::Suggestions(id, query, result));
        });
    }
    /// The palette itself: a centred modal holding only the search field and the suggestions
    /// under it. Up/Down choose a suggestion, Enter searches (the chosen one, else what is typed),
    /// and Escape or a click outside closes it.
    fn palette(&mut self, ctx: &egui::Context) {
        if !self.palette_open {
            return;
        }
        self.palette_debounce(ctx);
        let len = self.palette_suggestions.len();
        let (down, up, enter) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowDown),
                i.consume_key(egui::Modifiers::NONE, egui::Key::ArrowUp),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Enter),
            )
        });
        if down && len > 0 {
            self.palette_selected = Some(self.palette_selected.map_or(0, |i| (i + 1).min(len - 1)));
        }
        if up {
            self.palette_selected = self.palette_selected.and_then(|i| i.checked_sub(1));
        }
        let mut submit = None;
        if enter {
            submit = Some(
                self.palette_selected
                    .and_then(|i| self.palette_suggestions.get(i).cloned())
                    .unwrap_or_else(|| self.palette_query.clone()),
            );
        }
        let modal = egui::Modal::new(egui::Id::unique("global-search"))
            .backdrop_color(Color32::from_black_alpha(150))
            .show(ctx, |ui| {
                ui.set_width(560.0);
                egui::Frame::new()
                    .fill(Color32::from_rgb(30, 32, 28))
                    .stroke(Stroke::new(1.0_f32, Color32::from_gray(58)))
                    .corner_radius(14.0)
                    .inner_margin(egui::Margin::symmetric(16, 14))
                    .show(ui, |ui| {
                        let mut picked = None;
                        ui.horizontal(|ui| {
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::splat(20.0), egui::Sense::hover());
                            widgets::paint(ui.painter(), Icon::Search, r, MUTED);
                            let field = muted_hint(
                                ui,
                                egui::TextEdit::singleline(&mut self.palette_query)
                                    .id(egui::Id::unique("palette-input"))
                                    .hint_text(RichText::new("Search YouTube Music").size(18.0).color(MUTED))
                                    .font(FontId::proportional(18.0))
                                    .frame(bare_text_frame())
                                    .desired_width(f32::INFINITY),
                            );
                            if self.palette_focus {
                                field.request_focus();
                                self.palette_focus = false;
                            }
                            if field.changed() {
                                self.palette_edited = Some(Instant::now());
                                self.palette_selected = None;
                            }
                        });
                        ui.add_space(10.0);
                        if self.palette_query.trim().is_empty() {
                            ui.label(
                                RichText::new(
                                    "Type to search · ↑ ↓ to choose · Enter to search · Esc to close",
                                )
                                .color(MUTED)
                                .size(12.0),
                            );
                        } else if self.palette_loading && self.palette_suggestions.is_empty() {
                            ui.horizontal(|ui| {
                                ui.spinner();
                                ui.label(RichText::new("Finding suggestions…").color(MUTED));
                            });
                        }
                        for (i, suggestion) in self.palette_suggestions.iter().enumerate() {
                            let (rect, response) = ui.allocate_exact_size(
                                Vec2::new(ui.available_width(), 34.0),
                                egui::Sense::click(),
                            );
                            let lit = self.palette_selected == Some(i) || response.hovered();
                            if lit {
                                ui.painter()
                                    .rect_filled(rect, 8.0, widgets::white_alpha(14));
                                ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                            }
                            let icon = Rect::from_min_size(rect.min + Vec2::new(10.0, 9.0), Vec2::splat(16.0));
                            widgets::paint(ui.painter(), Icon::Search, icon, MUTED);
                            ui.painter().text(
                                Pos2::new(rect.left() + 38.0, rect.center().y),
                                egui::Align2::LEFT_CENTER,
                                truncate(suggestion, 60),
                                FontId::proportional(15.0),
                                if lit { WHITE } else { Color32::from_gray(200) },
                            );
                            if response.clicked() {
                                picked = Some(suggestion.clone());
                            }
                        }
                        picked
                    })
                    .inner
            });
        let close = modal.should_close();
        if submit.is_none() {
            submit = modal.inner;
        }
        if let Some(text) = submit {
            self.palette_submit(text);
        } else if close {
            self.palette_open = false;
        }
    }
    fn cancel_download(&mut self, key: &str) {
        if let Some(i) = self.downloads.iter().position(|d| d.track.key() == key) {
            // A running worker sees the flag, kills yt-dlp and reports back; either way the row
            // leaves now, as upstream: a cancelled download needs no reminder.
            self.downloads[i].cancel.store(true, Ordering::SeqCst);
            self.downloads.remove(i);
        }
        self.pump_downloads();
    }
    /// Opens the sign-in dialog (Meiro `openSignIn`).
    fn open_sign_in(&mut self) {
        if self.sign_in.open {
            return;
        }
        self.sign_in = SignIn {
            open: true,
            ..Default::default()
        };
    }
    /// Closes the dialog and stops what it started; a session that check was about to save is
    /// not kept (Meiro `dismissSignIn`).
    fn dismiss_sign_in(&mut self) {
        if self.sign_in.busy {
            self.sign_in.cancel.store(true, Ordering::SeqCst);
            auth::cancel_sign_in();
        }
        self.account_request += 1;
        self.sign_in = SignIn::default();
    }
    /// Starts one sign-in task; its outcome comes back as `Event::SignedIn`.
    fn start_sign_in(
        &mut self,
        job: impl FnOnce(&AtomicBool, u64) -> anyhow::Result<Account> + Send + 'static,
    ) {
        self.account_request += 1;
        let request = self.account_request;
        self.sign_in.busy = true;
        self.sign_in.err.clear();
        self.sign_in.import_failed.clear();
        self.sign_in.cancel = Arc::new(AtomicBool::new(false));
        let (cancel, tx) = (self.sign_in.cancel.clone(), self.tx.clone());
        let generation = auth::generation();
        std::thread::spawn(move || {
            let _ = tx.send(Event::SignedIn(
                request,
                job(&cancel, generation).map_err(err),
            ));
        });
    }
    /// The user signs in in a browser window of YTDugem's own (Meiro `windowSignIn`).
    fn window_sign_in(&mut self) {
        if self.sign_in.busy {
            return;
        }
        self.sign_in.in_window = true;
        self.start_sign_in(|cancel, generation| {
            let cookie = auth::window_sign_in(cancel)?;
            auth::sign_in(&cookie, &auth::Source::default(), generation)
        });
    }
    /// Tries the pasted Cookie header (Meiro `submitSignIn`).
    fn submit_sign_in(&mut self) {
        if self.sign_in.busy {
            return;
        }
        let cookie = match auth::pasted_cookie(&self.sign_in.cookie) {
            Ok(cookie) => cookie,
            Err(e) => {
                self.sign_in.err = e.to_string();
                return;
            }
        };
        self.start_sign_in(move |_, generation| {
            auth::sign_in(&cookie, &auth::Source::default(), generation)
        });
    }
    /// Reads a browser's accounts without choosing or saving one (Meiro `importSignIn`).
    fn import_sign_in(&mut self, browser: &'static str) {
        if self.sign_in.busy {
            return;
        }
        self.account_request += 1;
        let request = self.account_request;
        self.sign_in.busy = true;
        self.sign_in.err.clear();
        self.sign_in.import_failed.clear();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = auth::import_browser(browser).and_then(|cookie| {
                let accounts = auth::sign_in_accounts(&cookie)?;
                Ok((cookie, accounts))
            });
            let _ = tx.send(Event::BrowserImported(
                request,
                browser,
                result.map_err(err),
            ));
        });
    }
    /// Signs in as the account or channel picked from an imported browser session.
    fn pick_account(&mut self, channel: auth::Channel) {
        let cookie = self.sign_in.imported.clone();
        let source = auth::Source {
            account_index: channel.account_index,
            channel: channel.channel_id,
            browser: self.sign_in.source.to_owned(),
            email: channel.email,
        };
        self.start_sign_in(move |_, generation| auth::sign_in(&cookie, &source, generation));
    }
    fn signed_in(&mut self, request: u64, result: Result<Account, String>) {
        let ours = request == self.account_request;
        match result {
            Ok(account) => {
                self.sync_account();
                if ours {
                    self.sign_in = SignIn::default();
                }
                self.message = format!("Signed in as {}", account.name);
            }
            Err(e) if ours => {
                self.sign_in.busy = false;
                self.sign_in.in_window = false;
                if self.sign_in.accounts.is_none() {
                    // A failed import or paste points at pasting instead.
                    self.sign_in.import_failed = self.sign_in.source.to_owned();
                }
                self.sign_in.err = e;
            }
            Err(_) => {}
        }
    }
    /// Acts as another of the session's accounts or channels (Meiro `switchChannel`).
    fn switch_channel(&mut self, channel: auth::Channel) {
        self.account_request += 1;
        let request = self.account_request;
        let tx = self.tx.clone();
        self.message = format!("Switching to {}…", channel.label());
        std::thread::spawn(move || {
            let _ = tx.send(Event::SignedIn(
                request,
                auth::switch_channel(&channel).map_err(err),
            ));
        });
    }
    fn sign_out(&mut self) {
        self.dismiss_sign_in();
        match auth::logout() {
            Ok(()) => {
                self.sync_account();
                self.message = "Signed out".into();
            }
            Err(e) => self.message = format!("Sign out failed: {e:#}"),
        }
    }
    /// The account's picture in the top bar, which opens a menu of the account, its channels,
    /// the settings and the sign-in (Meiro `accountButton`).
    fn account_button(&mut self, ui: &mut egui::Ui) {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::click());
        let popup = ui.make_persistent_id("account-menu");
        let open = egui::Popup::is_id_open(ui.ctx(), popup);
        if response.hovered() || open {
            ui.painter()
                .circle_filled(rect.center(), 17.0, widgets::white_alpha(18));
        }
        let picture = Rect::from_center_size(rect.center(), Vec2::splat(28.0));
        match &self.account {
            Some(a) => avatar(ui, &a.thumbnail, &a.name, picture),
            None => {
                ui.painter()
                    .circle_filled(picture.center(), 14.0, widgets::white_alpha(22));
                widgets::paint(ui.painter(), Icon::User, picture.shrink(7.0), MUTED);
            }
        }
        let response = response.on_hover_text("Account");
        if response.clicked() {
            egui::Popup::toggle_id(ui.ctx(), popup);
        }
        let mut action = None;
        popup_below_widget(
            popup,
            &response,
            egui::PopupCloseBehavior::CloseOnClickOutside,
            |ui| {
                ui.set_min_width(260.0);
                if let Some(a) = &self.account {
                    ui.add_space(4.0);
                    ui.add(egui::Label::new(heading(&a.name, 15.0)).truncate());
                    if !a.email.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(&a.email).size(12.5).color(MUTED))
                                .truncate(),
                        );
                    }
                    ui.separator();
                    // Only when the cookies can act as more than one account or channel.
                    if self.channels.len() >= 2 {
                        ui.label(RichText::new("Account / channel").size(12.0).color(MUTED));
                        let has_selected = self
                            .channels
                            .iter()
                            .any(|c| c.selected && c.account_index == self.source.account_index);
                        for channel in &self.channels {
                            let selected = channel.account_index == self.source.account_index
                                && if has_selected {
                                    channel.selected
                                } else {
                                    !self.source.channel.is_empty()
                                        && channel.channel_id == self.source.channel
                                };
                            let text = format!(
                                "{}  {}",
                                if selected {
                                    egui_phosphor::regular::CHECK
                                } else {
                                    "   "
                                },
                                channel.label()
                            );
                            let row = ui.add_enabled(
                                !channel.disabled,
                                egui::Button::new(text).frame(false),
                            );
                            if row.clicked() && !selected {
                                action = Some(AccountAction::Switch(channel.clone()));
                            }
                        }
                        ui.separator();
                    }
                }
                if ui.add(egui::Button::new("Settings").frame(false)).clicked() {
                    action = Some(AccountAction::Settings);
                }
                let (label, act) = if self.account.is_some() {
                    ("Sign out", AccountAction::SignOut)
                } else {
                    ("Sign in", AccountAction::SignIn)
                };
                if ui.add(egui::Button::new(label).frame(false)).clicked() {
                    action = Some(act);
                }
            },
        );
        if let Some(action) = action {
            egui::Popup::close_id(ui.ctx(), popup);
            match action {
                AccountAction::Switch(channel) => self.switch_channel(channel),
                AccountAction::Settings => self.navigate(Page::Settings),
                AccountAction::SignOut => self.sign_out(),
                AccountAction::SignIn => self.open_sign_in(),
            }
        }
    }
    /// The sign-in dialog (Meiro `signInDialog`): a browser window first, and under Advanced an
    /// import from a browser the user already uses, or a pasted Cookie header.
    fn sign_in_dialog(&mut self, ctx: &egui::Context) {
        if !self.sign_in.open {
            return;
        }
        if self.sign_in.accounts.is_some() {
            self.account_picker(ctx);
            return;
        }
        let busy = self.sign_in.busy;
        let mut close = false;
        let modal = egui::Modal::new(egui::Id::unique("sign-in"))
            .backdrop_color(Color32::from_black_alpha(150))
            .show(ctx, |ui| {
                dialog_frame(ui, |ui| {
                    let (badge, _) =
                        ui.allocate_exact_size(Vec2::splat(56.0), egui::Sense::hover());
                    ui.painter()
                        .rect_filled(badge, 16.0, Color32::from_rgb(58, 64, 52));
                    widgets::paint(ui.painter(), Icon::User, badge.shrink(16.0), WHITE);
                    ui.add_space(12.0);
                    ui.label(heading("Sign in to YouTube Music", 22.0));
                    ui.add_space(6.0);
                    muted(ui, "YTDugem opens a separate browser window where you sign in to your Google account. YTDugem never sees your password. It keeps the session of that window, which gives it the same account access as a browser, and closes the window. Your own browser is not used.");
                    ui.add_space(12.0);
                    if ui
                        .add_enabled(
                            !busy,
                            egui::Button::new(
                                RichText::new("Sign in with a browser window")
                                    .size(15.0)
                                    .color(Color32::from_rgb(20, 21, 19)),
                            )
                            .fill(Color32::from_gray(236))
                            .corner_radius(20.0)
                            .min_size(Vec2::new(ui.available_width(), 44.0)),
                        )
                        .clicked()
                    {
                        self.window_sign_in();
                    }
                    if self.sign_in.in_window {
                        ui.add_space(6.0);
                        ui.label(
                            RichText::new(
                                "Finish signing in in the window that opened. YTDugem closes it for you.",
                            )
                            .color(ACCENT),
                        );
                    }
                    ui.add_space(8.0);
                    let chevron = if self.sign_in.advanced {
                        egui_phosphor::regular::CARET_UP
                    } else {
                        egui_phosphor::regular::CARET_DOWN
                    };
                    if ui
                        .add(egui::Button::new(format!("Advanced {chevron}")).frame(false))
                        .clicked()
                    {
                        self.sign_in.advanced = !self.sign_in.advanced;
                    }
                    if self.sign_in.advanced {
                        self.sign_in_advanced(ui, busy);
                    }
                    ui.add_space(8.0);
                    muted(ui, "YTDugem saves the session in its data folder, encrypted with a key that Windows Credential Manager keeps for your user. Sign out to remove the saved copy.");
                    if !self.sign_in.err.is_empty() {
                        ui.add_space(6.0);
                        ui.label(RichText::new(&self.sign_in.err).color(ERROR));
                    }
                    ui.add_space(12.0);
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        // Only the pasted header needs a button of its own: the window and an
                        // import start as soon as they are picked.
                        if self.sign_in.advanced
                            && ui
                                .add_enabled(
                                    !busy && !self.sign_in.cookie.trim().is_empty(),
                                    egui::Button::new("Sign in"),
                                )
                                .clicked()
                        {
                            self.submit_sign_in();
                        }
                        if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                            close = true;
                        }
                        if busy {
                            ui.spinner();
                        }
                    });
                });
            });
        if close || modal.should_close() {
            self.dismiss_sign_in();
        }
    }
    fn sign_in_advanced(&mut self, ui: &mut egui::Ui, busy: bool) {
        muted(ui, "Or use a browser you are already signed in with. This copy can stop working when that browser renews its cookies. Sign in at https://music.youtube.com there first, then pick it:");
        ui.add_space(6.0);
        let picker = ui.add_enabled(
            !busy,
            egui::Button::new(format!(
                "Import from a browser  {}",
                egui_phosphor::regular::CARET_DOWN
            )),
        );
        let popup = ui.make_persistent_id("import-browsers");
        if picker.clicked() {
            egui::Popup::toggle_id(ui.ctx(), popup);
        }
        let mut chosen = None;
        popup_below_widget(
            popup,
            &picker,
            egui::PopupCloseBehavior::CloseOnClick,
            |ui| {
                ui.set_min_width(200.0);
                let browsers = auth::browsers();
                if browsers.is_empty() {
                    ui.label(RichText::new("No supported browser was found").color(MUTED));
                }
                for browser in browsers {
                    if ui
                        .add(egui::Button::new(browser.label).frame(false))
                        .clicked()
                    {
                        chosen = Some(browser.label);
                    }
                }
            },
        );
        if let Some(label) = chosen {
            self.sign_in.source = label;
            self.import_sign_in(label);
        }
        if !self.sign_in.import_failed.is_empty() {
            ui.add_space(4.0);
            ui.label(
                RichText::new(format!(
                    "Importing from {} did not work. Paste the Cookie header instead.",
                    self.sign_in.import_failed
                ))
                .strong()
                .color(ACCENT),
            );
        }
        ui.add_space(6.0);
        muted(ui, "To paste it: open music.youtube.com while signed in, open your browser's developer tools (F12) and go to the Network tab, reload, pick a request to music.youtube.com, and copy the value of its Cookie request header.");
        ui.add_space(6.0);
        egui::Frame::new()
            .fill(Color32::from_rgb(40, 42, 38))
            .stroke(Stroke::new(1.0_f32, Color32::from_gray(62)))
            .corner_radius(10.0)
            .inner_margin(egui::Margin::symmetric(12, 8))
            .show(ui, |ui| {
                egui::ScrollArea::vertical()
                    .max_height(96.0)
                    .id_salt("cookie-box")
                    .show(ui, |ui| {
                        muted_hint(
                            ui,
                            egui::TextEdit::multiline(&mut self.sign_in.cookie)
                                .password(true)
                                .hint_text(RichText::new("Paste the Cookie header").color(MUTED))
                                .frame(bare_text_frame())
                                .desired_rows(3)
                                .desired_width(f32::INFINITY),
                        );
                    });
            });
    }
    /// Keeps an imported browser session private until the user chooses the Google account and
    /// channel to act as (Meiro `signInAccountPicker`).
    fn account_picker(&mut self, ctx: &egui::Context) {
        let busy = self.sign_in.busy;
        let mut picked = None;
        let (mut back, mut close) = (false, false);
        let accounts = self.sign_in.accounts.clone().unwrap_or_default();
        let modal = egui::Modal::new(egui::Id::unique("sign-in-accounts"))
            .backdrop_color(Color32::from_black_alpha(150))
            .show(ctx, |ui| {
                dialog_frame(ui, |ui| {
                    ui.label(heading("Choose your YouTube account", 22.0));
                    ui.add_space(6.0);
                    muted(ui, &format!(
                        "Choose the account or channel to use from {}. YTDugem remembers your choice.",
                        self.sign_in.source
                    ));
                    ui.add_space(10.0);
                    for (i, account) in accounts.iter().enumerate() {
                        let enabled = !busy && !account.disabled;
                        let (rect, row) = ui.allocate_exact_size(
                            Vec2::new(ui.available_width(), 64.0),
                            if enabled {
                                egui::Sense::click()
                            } else {
                                egui::Sense::hover()
                            },
                        );
                        let fill = if row.hovered() && enabled { 16 } else { 6 };
                        ui.painter()
                            .rect_filled(rect, 12.0, widgets::white_alpha(fill));
                        let avatar_rect = Rect::from_min_size(
                            rect.min + Vec2::new(12.0, 12.0),
                            Vec2::splat(40.0),
                        );
                        avatar(ui, &account.thumbnail, account.label(), avatar_rect);
                        let x = avatar_rect.right() + 12.0;
                        let width = rect.right() - x - 12.0;
                        let color = if enabled { WHITE } else { MUTED };
                        fit_text(
                            ui.painter(),
                            Pos2::new(x, rect.top() + 22.0),
                            egui::Align2::LEFT_CENTER,
                            account.label(),
                            FontId::new(15.0, egui::FontFamily::Name("bold".into())),
                            color,
                            width,
                        );
                        fit_text(
                            ui.painter(),
                            Pos2::new(x, rect.top() + 43.0),
                            egui::Align2::LEFT_CENTER,
                            account.detail(),
                            FontId::proportional(12.5),
                            MUTED,
                            width,
                        );
                        if row.clicked() {
                            picked = Some(i);
                        }
                        ui.add_space(6.0);
                    }
                    if !self.sign_in.err.is_empty() {
                        ui.label(RichText::new(&self.sign_in.err).color(ERROR));
                    }
                    ui.add_space(10.0);
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui.add(egui::Button::new("Cancel").frame(false)).clicked() {
                            close = true;
                        }
                        if ui
                            .add_enabled(!busy, egui::Button::new("Back").frame(false))
                            .clicked()
                        {
                            back = true;
                        }
                        if busy {
                            ui.spinner();
                        }
                    });
                });
            });
        if let Some(i) = picked {
            self.pick_account(accounts[i].clone());
        } else if back {
            self.sign_in.accounts = None;
            self.sign_in.imported.clear();
            self.sign_in.err.clear();
        } else if close || modal.should_close() {
            self.dismiss_sign_in();
        }
    }
    fn events(&mut self, ctx: &egui::Context) {
        while let Ok(event) = self.rx.try_recv() {
            match event {
                Event::Playing(id, t, r) if id == self.play_request => self.playing(t, r),
                Event::AccountPlaylists(id, r) if id == self.account_playlists_request => {
                    self.account_playlists_loading = false;
                    match r {
                        Ok(list) => self.account_playlists = Some(list),
                        Err(e) => {
                            self.account_playlists = Some(vec![]);
                            self.message = e;
                        }
                    }
                }
                Event::Suggestions(id, query, r)
                    if id == self.palette_request && query == self.palette_query.trim() =>
                {
                    self.palette_loading = false;
                    self.palette_suggestions = r.unwrap_or_default();
                    self.palette_selected = None;
                }
                Event::PlaylistEdited(r) => self.playlist_edited(r),
                Event::BulkTracks(action, origin, r) => {
                    self.bulk_loading = false;
                    match r {
                        Ok(tracks) => self.apply_bulk(action, tracks, &origin),
                        Err(e) => self.message = e,
                    }
                }
                Event::Translated(id, r) if id == self.translation_request => {
                    self.translating = false;
                    match r {
                        Ok(t) => {
                            if let Some(key) = self.current.as_ref().map(|t| t.key().to_owned()) {
                                self.translation = Some((key, self.translation_target(), t));
                            }
                        }
                        Err(e) => self.message = format!("Translation unavailable: {e}"),
                    }
                }
                Event::Romanized(id, key, r) if id == self.romanization_request => {
                    self.romanizing = false;
                    match r {
                        Ok(romanized) => self.romanization = Some((key, romanized)),
                        Err(e) => self.message = format!("Romanization unavailable: {e}"),
                    }
                }
                // Quiet on failure: the like button and the album/artist entries just stay off.
                Event::SongInfo(id, key, Ok(info)) if id == self.song_info_request => {
                    // A rating sent before the details arrived wins over the older reading.
                    let like = self
                        .song_info
                        .as_ref()
                        .filter(|(k, _)| *k == key)
                        .and_then(|(_, i)| i.like)
                        .or(info.like);
                    self.song_info = Some((key, services::SongInfo { like, ..info }));
                }
                Event::Rated(key, previous, Err(e)) => {
                    if let Some((_, info)) = self.song_info.as_mut().filter(|(k, _)| *k == key) {
                        info.like = Some(previous);
                    }
                    self.message = format!("Could not save the rating: {e}");
                }
                Event::Prepared(key, stream) => {
                    if self.preparing.as_deref() == Some(key.as_str()) {
                        self.preparing = None;
                    }
                    self.prepared = stream.map(|s| (key, s, Instant::now()));
                }
                Event::Results(id, r) if id == self.request => {
                    self.busy = false;
                    match r {
                        Ok(t) => self.results = t,
                        Err(e) => self.message = e,
                    }
                }
                Event::Home(id, r) if id == self.home_request => {
                    let first_page = !self.home_more;
                    if self.home_more {
                        self.home_more = false;
                    } else {
                        self.home_pending = self.home_pending.saturating_sub(1);
                    }
                    match r {
                        Ok(feed) => {
                            self.home_token = feed.continuation;
                            if first_page {
                                // Supplements may arrive first; Home's own shelves lead.
                                let supplements = std::mem::take(&mut self.home);
                                merge_shelves(&mut self.home, feed.shelves);
                                merge_shelves(&mut self.home, supplements);
                            } else {
                                merge_shelves(&mut self.home, feed.shelves);
                            }
                        }
                        Err(e) => {
                            if self.home.is_empty() {
                                self.message = e;
                            }
                        }
                    }
                }
                Event::Supplement(id, r) if id == self.home_request => {
                    self.home_pending = self.home_pending.saturating_sub(1);
                    if let Ok(s) = r {
                        merge_shelves(&mut self.home, s);
                    }
                }
                Event::Recents(id, r) if id == self.home_request => {
                    self.home_pending = self.home_pending.saturating_sub(1);
                    if let Ok(t) = r {
                        self.recents = t;
                    }
                }
                Event::ExploreShelves(id, feed, r) if id == self.explore.request_for(feed) => {
                    self.explore.shelves_loaded(feed, r);
                }
                Event::Moods(id, r) if id == self.explore.request => self.moods_loaded(r),
                Event::MoodArtwork(id, key, url) if id == self.explore.request => {
                    self.explore.artwork_loaded(&key, url);
                }
                Event::Collection(id, r) if id == self.collection_request => {
                    self.busy = false;
                    match r {
                        Ok(c) => {
                            if let Some(old) = self.collection.as_mut() {
                                old.tracks.extend(c.tracks);
                                old.set_ids.extend(c.set_ids);
                                old.continuation = c.continuation;
                            } else {
                                self.collection = Some(c);
                            }
                        }
                        Err(e) => self.message = e,
                    }
                }
                Event::Imported(r) => {
                    self.importing = false;
                    match r {
                        Ok(ts) => {
                            let n = ts.len();
                            merge_local(&mut self.library.local, ts);
                            self.message = format!("Imported {n} audio files");
                            self.navigate(Page::Local);
                            self.save();
                        }
                        Err(e) => self.message = e,
                    }
                }
                Event::DownloadProgress(key, f) => {
                    if let Some(d) = self.downloads.iter_mut().find(|d| d.track.key() == key) {
                        if matches!(d.state, DownloadState::Running(_)) {
                            d.state = DownloadState::Running(f);
                        }
                    }
                }
                Event::Downloaded(key, r) => {
                    let i = self.downloads.iter().position(|d| d.track.key() == key);
                    match (i, r) {
                        // Cancelled: the row is already gone.
                        (None, _) => {}
                        (Some(i), Ok(tracks)) => {
                            let d = self.downloads.remove(i);
                            self.message = format!("Downloaded {}", d.track.title);
                            merge_local(&mut self.library.local, tracks);
                            self.save();
                        }
                        (Some(i), Err(e)) => self.downloads[i].state = DownloadState::Failed(e),
                    }
                    self.pump_downloads();
                }
                Event::Lyrics(id, key, r)
                    if id == self.lyrics_request
                        && self.current.as_ref().is_some_and(|t| t.key() == key) =>
                {
                    match r {
                        Ok(found) => {
                            self.lyrics = found.lines;
                            self.lyrics_provider = found.provider.into();
                            self.lyrics_message.clear();
                            match self.lyrics_mode {
                                LyricsMode::Translated => self.request_translation(),
                                LyricsMode::Romanized => self.request_romanization(),
                                LyricsMode::Original => {}
                            }
                        }
                        // The providers' reasons are for whoever asks; the pane says it plainly.
                        Err(e) => {
                            self.lyrics_message = sheet_lyrics::no_lyrics_line().into();
                            self.lyrics_detail = e;
                        }
                    }
                }
                Event::Artwork(key, Ok((cover, back)))
                    if self.current.as_ref().is_some_and(|t| t.key() == key) =>
                {
                    self.cover = Some(ctx.load_texture(
                        format!("cover:{key}"),
                        cover,
                        // Mipmapped: the same 1200 px cover is also drawn at 52 px in the top bar.
                        egui::TextureOptions {
                            mipmap_mode: Some(egui::TextureFilter::Linear),
                            ..egui::TextureOptions::LINEAR
                        },
                    ));
                    self.backdrop = Some(ctx.load_texture(
                        format!("back:{key}"),
                        back,
                        egui::TextureOptions::LINEAR,
                    ));
                }
                Event::SignedIn(id, r) => self.signed_in(id, r),
                Event::BrowserImported(id, browser, r) if id == self.account_request => {
                    self.sign_in.busy = false;
                    match r {
                        Ok((cookie, accounts)) => {
                            self.sign_in.imported = cookie;
                            self.sign_in.source = browser;
                            self.sign_in.accounts = Some(accounts);
                        }
                        Err(e) => {
                            self.sign_in.import_failed = browser.to_owned();
                            self.sign_in.err = e;
                        }
                    }
                }
                Event::Renewed(r) => match r {
                    // The same account: only its channels were not known yet.
                    Ok(a) if self.account.as_ref() == Some(&a) => {
                        (self.channels, self.source) = auth::current_channels();
                    }
                    Ok(_) => self.sync_account(),
                    // YouTube retired the session and no browser could replace it.
                    Err((true, e)) => {
                        self.sync_account();
                        self.message = e;
                    }
                    // Offline or YouTube unreachable: keep the session and check next time.
                    Err((false, _)) => {}
                },
                Event::LyricsKeys(id, r) if id == self.lyric_keys_request => {
                    self.lyric_keys_busy = false;
                    match r {
                        Ok(keys) => {
                            self.lyric_keys = keys.into_iter().collect();
                            self.lyric_keys_loaded = true;
                        }
                        Err(e) => self.message = e,
                    }
                }
                Event::LyricsKeySaved(id, key, r) if id == self.lyric_keys_request => {
                    self.lyric_keys_busy = false;
                    match r {
                        Ok(configured) => {
                            self.lyric_keys.insert(key.clone(), configured);
                            self.lyric_key_inputs.remove(&key);
                            self.lyric_keys_loaded = true;
                            self.lyrics_settings_changed(true);
                        }
                        Err(e) => self.message = e,
                    }
                }
                Event::AudioDevices(result) => {
                    self.audio_devices_loading = false;
                    self.audio_devices_loaded = true;
                    match result {
                        Ok(devices) => self.audio_devices = devices,
                        Err(error) => {
                            self.message = format!("Could not enumerate mpv audio devices: {error}")
                        }
                    }
                }
                Event::Radio(id, key, r)
                    if id == self.radio_request
                        && self.current.as_ref().is_some_and(|t| t.key() == key) =>
                {
                    self.radio_pending = false;
                    match r {
                        Ok(mut ts) => {
                            ts.retain(|t| t.key() != key);
                            self.library.queue.extend(ts);
                            if self.status.ended && !self.library.queue.is_empty() {
                                self.next(false);
                            }
                            self.save();
                        }
                        Err(e) => self.message = e,
                    }
                }
                _ => {}
            }
        }
        if self.last_poll.elapsed() > Duration::from_millis(100) {
            self.last_poll = Instant::now();
            if let Some(s) = self.player.poll() {
                self.status = s;
                if let Some(error) = self.player.take_error() {
                    self.message = error;
                }
                if s.errored {
                    // A cached stream URL that failed must not be reused on the next try.
                    if let Some(t) = &self.current {
                        crate::player::forget_stream(&t.source);
                    }
                    self.stop();
                    self.message="Audio could not be opened. Check your connection or sign-in, then try again.".into();
                } else if s.ended {
                    self.next(true);
                }
            }
            self.step_fade();
            self.maybe_crossfade();
            self.prepare_next();
        }
        if self.sleep_until.is_some_and(|t| Instant::now() >= t) {
            self.sleep_until = None;
            self.stop();
            self.message = "Sleep timer finished".into();
        }
        if DISCORD_READY && self.library.discord_enabled {
            if let Some(track) = self
                .current
                .as_ref()
                .filter(|_| !self.status.paused && !self.status.ended)
            {
                let mut track = track.clone();
                if self.status.duration > 0.0 {
                    track.duration = self.status.duration;
                }
                self.discord.publish(
                    &track,
                    self.status.position,
                    self.library.speed,
                    &self.library.discord_activity_name,
                    self.library.discord_buttons,
                );
            } else {
                self.discord.clear();
            }
        }
        for event in self.integrations.poll() {
            match event {
                MediaEvent::Toggle => self.toggle(),
                MediaEvent::Play if self.status.paused => self.toggle(),
                MediaEvent::Pause if !self.status.paused => self.toggle(),
                MediaEvent::Next => self.next(false),
                MediaEvent::Previous => self.previous(),
                MediaEvent::Seek(s) => {
                    let r = self.player.seek(s);
                    self.report(r);
                }
                MediaEvent::SeekBy(s) => {
                    let r = self.player.seek((self.status.position + s).max(0.0));
                    self.report(r);
                }
                MediaEvent::Volume(v) if v.is_finite() => {
                    self.library.volume = (v * 100.0).clamp(0.0, 100.0);
                    let r = self.player.volume(self.library.volume);
                    self.report(r);
                    self.save();
                }
                MediaEvent::Stop => {
                    self.stop();
                    self.current = None;
                }
                _ => {}
            }
        }
        self.integrations
            .update(self.current.as_ref(), &self.status);
    }
}

fn err(e: anyhow::Error) -> String {
    format!("{e:#}")
}

impl YTDugem {
    fn sidebar(&mut self, ui: &mut egui::Ui, rect: Rect) {
        ui.painter().rect_filled(rect, 0.0, SIDE);
        ui.painter().line_segment(
            [rect.right_top(), rect.right_bottom()],
            Stroke::new(1.0_f32, Color32::from_gray(43)),
        );
        // The strip above the search box moves the window, like the top bar (the macOS-style
        // traffic lights that used to sit here duplicated the real window controls).
        let drag = ui.interact(
            Rect::from_min_size(rect.min, Vec2::new(rect.width(), 40.0)),
            ui.make_persistent_id("sidebar-drag"),
            egui::Sense::click_and_drag(),
        );
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let max = ui.ctx().input(|s| s.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
        }
        region(
            ui,
            Rect::from_min_size(
                rect.min + Vec2::new(14.0, 44.0),
                Vec2::new(rect.width() - 28.0, 42.0),
            ),
            |ui| {
                egui::Frame::new()
                    .fill(Color32::from_rgb(36, 37, 34))
                    .stroke(Stroke::new(1.0_f32, Color32::from_gray(63)))
                    .corner_radius(7.0)
                    .inner_margin(8.0)
                    .show(ui, |ui| {
                        ui.horizontal(|ui| {
                            let (r, _) =
                                ui.allocate_exact_size(Vec2::splat(18.0), egui::Sense::hover());
                            widgets::paint(ui.painter(), Icon::Search, r, MUTED);
                            let response = ui.add(
                                egui::TextEdit::singleline(&mut self.query)
                                    .id(egui::Id::unique("search_input"))
                                    .desired_width(rect.width() - 78.0)
                                    .hint_text("Search")
                                    .frame(bare_text_frame()),
                            );
                            if response.lost_focus()
                                && ui.input(|s| s.key_pressed(egui::Key::Enter))
                            {
                                self.search();
                            }
                        });
                    });
            },
        );
        let rows = [
            (Page::Home, Icon::Home, "Home"),
            (Page::Explore, Icon::Compass, "Explore"),
            (Page::Library, Icon::Library, "Library"),
            (Page::Search, Icon::Search, "Search"),
            (Page::History, Icon::History, "History"),
            (Page::Downloads, Icon::Download, "Downloads"),
            (Page::Local, Icon::Music, "Local Music"),
        ];
        let rule = |ui: &egui::Ui, y: &mut f32| {
            *y += 12.0;
            ui.painter().line_segment(
                [
                    Pos2::new(rect.left() + 14.0, *y),
                    Pos2::new(rect.right() - 14.0, *y),
                ],
                Stroke::new(1.0_f32, Color32::from_gray(42)),
            );
            *y += 12.0;
        };
        let mut y = rect.top() + 108.0;
        for (i, (page, icon, label)) in rows.into_iter().enumerate() {
            if i == 4 {
                rule(ui, &mut y);
            }
            let r = Rect::from_min_size(
                Pos2::new(rect.left() + 14.0, y),
                Vec2::new(rect.width() - 28.0, 38.0),
            );
            if nav_row(ui, r, ("nav", i), icon, label, self.page == page).clicked() {
                // Upstream `selectDestination` closes an open mood: Explore opens on its landing page.
                if page == Page::Explore {
                    self.explore.view = explore::View::Main;
                }
                self.navigate(page);
                if self.page == Page::Search {
                    ui.ctx()
                        .memory_mut(|m| m.request_focus(egui::Id::unique("search_input")));
                }
            }
            y += 40.0;
        }
        rule(ui, &mut y);
        let bottom = rect.bottom() - 67.0;
        if self.account.is_some()
            && self.account_playlists.is_none()
            && !self.account_playlists_loading
        {
            self.load_account_playlists();
        }
        if bottom > y + 25.0 {
            region(
                ui,
                Rect::from_min_max(
                    Pos2::new(rect.left() + 14.0, y + 5.0),
                    Pos2::new(rect.right() - 14.0, bottom),
                ),
                |ui| {
                    egui::ScrollArea::vertical()
                        .id_salt("sidebar-playlists")
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            let accounts = self.account_playlists.clone().unwrap_or_default();
                            let open_id = self
                                .collection
                                .as_ref()
                                .filter(|_| self.page == Page::Collection)
                                .and_then(|c| c.playlist_id.clone());
                            for (i, p) in accounts.iter().enumerate() {
                                let r = Rect::from_min_size(
                                    ui.cursor().min,
                                    Vec2::new(ui.available_width(), 35.0),
                                );
                                ui.allocate_space(r.size());
                                let active = p
                                    .browse_id
                                    .as_deref()
                                    .is_some_and(|b| open_id.as_deref() == b.strip_prefix("VL"));
                                if nav_thumb_row(
                                    ui,
                                    r,
                                    ("account-playlist-nav", i),
                                    &p.thumbnail,
                                    &p.title,
                                    active,
                                )
                                .clicked()
                                {
                                    self.open_item(p.clone());
                                }
                            }
                            // Local playlists have no artwork of their own; the first track's
                            // cover stands in for it.
                            let locals: Vec<(String, String)> = self
                                .library
                                .playlists
                                .iter()
                                .map(|p| {
                                    let thumb = p
                                        .tracks
                                        .iter()
                                        .map(|t| t.thumbnail.as_str())
                                        .find(|t| t.starts_with("https://"))
                                        .unwrap_or_default();
                                    (p.name.clone(), thumb.to_owned())
                                })
                                .collect();
                            for (i, (name, thumb)) in locals.iter().enumerate() {
                                let r = Rect::from_min_size(
                                    ui.cursor().min,
                                    Vec2::new(ui.available_width(), 35.0),
                                );
                                ui.allocate_space(r.size());
                                if nav_thumb_row(
                                    ui,
                                    r,
                                    ("playlist", i),
                                    thumb,
                                    name,
                                    self.page == Page::Playlist(i),
                                )
                                .clicked()
                                {
                                    self.navigate(Page::Playlist(i));
                                }
                            }
                            if accounts.is_empty() && locals.is_empty() {
                                ui.label(
                                    RichText::new("Your playlists will appear here")
                                        .size(12.5)
                                        .color(MUTED),
                                );
                            }
                            if ui.small_button("+ New playlist").clicked() {
                                self.open_playlist_dialog(PlaylistDialog::Create {
                                    name: String::new(),
                                    privacy: "PRIVATE".into(),
                                    seed: None,
                                });
                            }
                        });
                },
            );
        }
        let r = Rect::from_min_size(
            Pos2::new(rect.left() + 14.0, rect.bottom() - 53.0),
            Vec2::new(rect.width() - 28.0, 38.0),
        );
        if nav_row(
            ui,
            r,
            "settings",
            Icon::Settings,
            "Settings",
            self.page == Page::Settings,
        )
        .clicked()
        {
            self.navigate(Page::Settings);
        }
    }
    fn topbar(&mut self, ui: &mut egui::Ui, rect: Rect) {
        ui.painter().rect_filled(rect, 0.0, SIDE);
        ui.painter().line_segment(
            [rect.left_bottom(), rect.right_bottom()],
            Stroke::new(1.0_f32, Color32::from_gray(42)),
        );
        let drag = ui.interact(
            Rect::from_min_max(
                rect.min,
                Pos2::new(rect.right() - 3.0 * CAPTION.x, rect.top() + 20.0),
            ),
            ui.make_persistent_id("window-drag"),
            egui::Sense::click_and_drag(),
        );
        if drag.drag_started() {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::StartDrag);
        }
        if drag.double_clicked() {
            let m = ui.ctx().input(|s| s.viewport().maximized.unwrap_or(false));
            ui.ctx()
                .send_viewport_cmd(egui::ViewportCommand::Maximized(!m));
        }
        region(
            ui,
            Rect::from_min_size(rect.min + Vec2::new(10.0, 21.0), Vec2::new(285.0, 42.0)),
            |ui| {
                ui.with_layout(egui::Layout::left_to_right(Align::Center), |ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if widgets::button(
                        ui,
                        Icon::Sidebar,
                        32.0,
                        false,
                        if self.sidebar_open {
                            "Hide sidebar"
                        } else {
                            "Show sidebar"
                        },
                    )
                    .clicked()
                    {
                        self.sidebar_open = !self.sidebar_open;
                    }
                    if ui
                        .add_enabled_ui(!self.nav.is_empty(), |ui| {
                            widgets::button(ui, Icon::Back, 28.0, false, "Back")
                        })
                        .inner
                        .clicked()
                    {
                        if let Some(p) = self.nav.pop() {
                            self.page = p;
                        }
                    }
                    if widgets::button(ui, Icon::Shuffle, 30.0, self.shuffle, "Shuffle").clicked() {
                        self.shuffle = !self.shuffle;
                    }
                    if widgets::button(ui, Icon::Previous, 32.0, false, "Previous · Alt+Left")
                        .clicked()
                    {
                        self.previous();
                    }
                    if self.loading {
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::splat(34.0), egui::Sense::hover());
                        egui::Spinner::new()
                            .size(20.0)
                            .color(WHITE)
                            .paint_at(ui, Rect::from_center_size(r.center(), Vec2::splat(20.0)));
                    } else if widgets::button(
                        ui,
                        if self.status.paused || self.current.is_none() {
                            Icon::Play
                        } else {
                            Icon::Pause
                        },
                        34.0,
                        false,
                        "Play / pause · Space",
                    )
                    .clicked()
                    {
                        self.toggle();
                    }
                    if widgets::button(ui, Icon::Next, 32.0, false, "Next · Alt+Right").clicked() {
                        self.next(false);
                    }
                    if widgets::button(
                        ui,
                        Icon::Repeat,
                        30.0,
                        self.repeat > 0,
                        if self.repeat == 2 {
                            "Repeat one"
                        } else {
                            "Repeat"
                        },
                    )
                    .clicked()
                    {
                        self.repeat = (self.repeat + 1) % 3;
                    }
                });
            },
        );
        let tools_width = if rect.width() < 900.0 { 160.0 } else { 282.0 };
        let card_left = rect.left() + 300.0;
        let card_right = rect.right() - tools_width - 12.0;
        let card_width = (card_right - card_left).clamp(80.0, 650.0);
        let card = Rect::from_center_size(
            Pos2::new((card_left + card_right) / 2.0, rect.top() + 40.0),
            Vec2::new(card_width, 52.0),
        );
        ui.painter()
            .rect_filled(card, 5.0, Color32::from_rgb(47, 49, 51));
        ui.painter().rect_stroke(
            card,
            5.0,
            Stroke::new(1.0_f32, Color32::from_gray(66)),
            egui::StrokeKind::Inside,
        );
        let response = ui.interact(
            card,
            ui.make_persistent_id("open-player"),
            egui::Sense::click(),
        );
        response.clone().on_hover_text("Open Now Playing");
        if response.clicked() {
            self.expanded = true;
            self.tab = PlayerPane::Main;
        }
        if let Some(t) = self.current.clone() {
            let art = Rect::from_min_size(card.min, Vec2::splat(card.height()));
            draw_cover(ui, &t, art, Some(self.cover.as_ref()));
            if self.loading {
                self.now_playing_badge(ui, art);
            }
            fit_text(
                ui.painter(),
                Pos2::new(card.left() + 61.0, card.top() + 15.0),
                egui::Align2::LEFT_CENTER,
                &t.title,
                FontId::new(12.5, egui::FontFamily::Name("bold".into())),
                WHITE,
                card.width() - 70.0,
            );
            fit_text(
                ui.painter(),
                Pos2::new(card.left() + 61.0, card.top() + 33.0),
                egui::Align2::LEFT_CENTER,
                &t.artist,
                FontId::proportional(11.0),
                MUTED,
                card.width() - 70.0,
            );
            let progress = if self.duration() > 0.0 {
                (self.status.position / self.duration()).clamp(0.0, 1.0) as f32
            } else {
                0.0
            };
            ui.painter().line_segment(
                [
                    card.left_bottom(),
                    Pos2::new(card.left() + card.width() * progress, card.bottom()),
                ],
                Stroke::new(2.0_f32, WHITE),
            );
        } else {
            widgets::paint(
                ui.painter(),
                Icon::Logo,
                Rect::from_center_size(card.center(), Vec2::splat(20.0)),
                Color32::from_gray(105),
            );
        }
        region(
            ui,
            Rect::from_min_size(
                Pos2::new(rect.right() - tools_width, rect.top() + 26.0),
                Vec2::new(tools_width - 12.0, 40.0),
            ),
            |ui| {
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    if tools_width > 160.0 {
                        let (r, _) =
                            ui.allocate_exact_size(Vec2::splat(17.0), egui::Sense::hover());
                        widgets::paint(ui.painter(), Icon::Volume, r, MUTED);
                        self.volume_slider(ui, 76.0, "top-volume");
                    }
                    if widgets::button(
                        ui,
                        Icon::Headphones,
                        30.0,
                        false,
                        "Audio output and playback settings",
                    )
                    .clicked()
                    {
                        self.navigate(Page::Settings);
                    }
                    if widgets::button(
                        ui,
                        Icon::Lyrics,
                        30.0,
                        self.expanded && self.tab == PlayerPane::Lyrics,
                        "Lyrics",
                    )
                    .clicked()
                    {
                        self.expanded = true;
                        self.tab = PlayerPane::Lyrics;
                    }
                    if widgets::button(ui, Icon::Queue, 30.0, false, "Queue").clicked() {
                        self.expanded = true;
                        self.tab = PlayerPane::Queue;
                    }
                    self.account_button(ui);
                });
            },
        );
        window_controls(ui, rect);
    }
    /// Over the artwork of the current track: a spinner while its stream opens, then a small
    /// equalizer that moves while it plays.
    fn now_playing_badge(&self, ui: &egui::Ui, art: Rect) {
        let p = ui.painter();
        p.rect_filled(art, 8.0, Color32::from_black_alpha(140));
        let size = (art.width() * 0.42).min(26.0);
        let r = Rect::from_center_size(art.center(), Vec2::splat(size));
        if self.loading {
            egui::Spinner::new().size(size).color(WHITE).paint_at(ui, r);
            return;
        }
        let t = ui.input(|i| i.time) as f32;
        let playing = !self.status.paused && !self.status.ended;
        if playing {
            ui.ctx().request_repaint_after(Duration::from_millis(50));
        }
        let bar = r.width() / 5.0;
        for i in 0..3 {
            let level = if playing {
                0.35 + 0.65 * ((t * 7.0 + i as f32 * 1.9).sin() * 0.5 + 0.5)
            } else {
                0.3
            };
            let x = r.left() + bar * (i as f32 * 2.0);
            p.rect_filled(
                Rect::from_min_max(
                    Pos2::new(x, r.bottom() - r.height() * level),
                    Pos2::new(x + bar, r.bottom()),
                ),
                1.0,
                WHITE,
            );
        }
    }
    fn duration(&self) -> f64 {
        self.status
            .duration
            .max(self.current.as_ref().map_or(0.0, |t| t.duration))
    }
    fn volume_slider(&mut self, ui: &mut egui::Ui, width: f32, id: &str) {
        let old = self.library.volume;
        let r = widgets::slider(ui, id, &mut self.library.volume, 100.0, width, "Volume");
        if (old - self.library.volume).abs() > 0.01 && self.player.is_active() {
            let result = self.player.volume(self.library.volume);
            self.report(result);
        }
        if r.drag_stopped() || r.clicked() {
            self.save();
        }
    }
    fn seekbar(&mut self, ui: &mut egui::Ui, width: f32, id: &str) {
        let duration = self.duration();
        let mut position = self.status.position;
        let old = position;
        widgets::slider(
            ui,
            id,
            &mut position,
            duration,
            width,
            "Seek playback position",
        );
        if (position - old).abs() > 0.02 && self.player.is_active() {
            let r = self.player.seek(position);
            self.report(r);
        }
    }
    fn track_menu(&mut self, ui: &mut egui::Ui, track: &Track) {
        if ui.button("Play next").clicked() {
            self.library.queue.insert(0, track.clone());
            self.save();
            ui.close();
        }
        if ui.button("Add to queue").clicked() {
            self.library.queue.push(track.clone());
            self.save();
            ui.close();
        }
        self.playlist_menu(ui, track);
        if ui.button("Download audio").clicked() {
            self.download(track.clone());
            ui.close();
        }
        if self.page == Page::Downloads && ui.button("Delete from computer").clicked() {
            self.delete_download = Some(track.clone());
            ui.close();
        }
        self.playlist_row_menu(ui, track);
    }
    /// Confirm, then delete a downloaded file from disk (upstream has no equivalent: this is a
    /// YTDugem addition). The file must sit in the download folder.
    fn download_delete_dialog(&mut self, ctx: &egui::Context) {
        let Some(track) = self.delete_download.clone() else {
            return;
        };
        let mut close = false;
        let mut confirmed = false;
        egui::Window::new("Delete download")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(340.0);
                ui.label(format!(
                    "“{}” will be deleted from this computer, and removed from your local music and playlists.",
                    track.title
                ));
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    if ui.button("Delete").clicked() {
                        confirmed = true;
                    }
                    if ui.button("Cancel").clicked() {
                        close = true;
                    }
                });
            });
        if confirmed {
            self.delete_download_file(&track);
        }
        if confirmed || close {
            self.delete_download = None;
        }
    }
    fn delete_download_file(&mut self, track: &Track) {
        let path = match services::delete_download(&track.source, &self.library.download_dir) {
            Ok(path) => path,
            Err(e) => {
                self.message = format!("Could not delete {}: {e}", track.title);
                return;
            }
        };
        // Drop every entry for this file, including ones saved before path cleanup.
        let gone = |t: &Track| services::plain_path(Path::new(&t.source)) == path;
        self.library.local.retain(|t| !gone(t));
        self.library.queue.retain(|t| !gone(t));
        for p in &mut self.library.playlists {
            p.tracks.retain(|t| !gone(t));
        }
        self.message = format!("Deleted {}", track.title);
        self.save();
    }
    fn song_row(
        &mut self,
        ui: &mut egui::Ui,
        track: &Track,
        index: Option<usize>,
        width: f32,
    ) -> bool {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 68.0), egui::Sense::click());
        let active = self
            .current
            .as_ref()
            .is_some_and(|t| t.key() == track.key());
        if response.hovered() || active {
            ui.painter()
                .rect_filled(rect, 5.0, widgets::white_alpha(if active { 15 } else { 8 }));
        }
        let offset = if let Some(i) = index {
            ui.painter().text(
                Pos2::new(rect.left() + 16.0, rect.center().y),
                egui::Align2::CENTER_CENTER,
                format!("{}", i + 1),
                FontId::proportional(12.0),
                MUTED,
            );
            34.0
        } else {
            0.0
        };
        let art = Rect::from_min_size(rect.min + Vec2::new(offset, 5.0), Vec2::splat(56.0));
        draw_cover(ui, track, art, None);
        if active {
            self.now_playing_badge(ui, art);
        }
        let text_x = art.right() + 13.0;
        // Room for the "···" menu (and the duration, when numbered), plus a gap before it.
        let extra = if index.is_some() { 110.0 } else { 48.0 };
        let max = rect.right() - text_x - extra;
        fit_text(
            ui.painter(),
            Pos2::new(text_x, rect.top() + 23.0),
            egui::Align2::LEFT_CENTER,
            &track.title,
            FontId::new(15.0, egui::FontFamily::Name("bold".into())),
            WHITE,
            max,
        );
        fit_text(
            ui.painter(),
            Pos2::new(text_x, rect.top() + 46.0),
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
        if index.is_some() {
            ui.painter().text(
                Pos2::new(rect.right() - 65.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                clock(track.duration),
                FontId::proportional(12.0),
                MUTED,
            );
        }
        region(
            ui,
            Rect::from_min_size(
                Pos2::new(rect.right() - 35.0, rect.center().y - 16.0),
                Vec2::new(34.0, 32.0),
            ),
            |ui| {
                ui.visuals_mut().widgets.inactive.weak_bg_fill = Color32::TRANSPARENT;
                // The 34 px slot is narrower than the padded label; wrapping split the dots.
                ui.style_mut().wrap_mode = Some(egui::TextWrapMode::Extend);
                ui.spacing_mut().button_padding = Vec2::new(6.0, 4.0);
                ui.menu_button("···", |ui| self.track_menu(ui, track));
            },
        );
        response.context_menu(|ui| self.track_menu(ui, track));
        response.clicked()
    }
    fn track_list(&mut self, ui: &mut egui::Ui, tracks: Vec<Track>, origin: &str) {
        self.track_list_with(ui, tracks, origin, true);
    }
    /// The rows of a list; `header` adds Play all, Add all to queue and the count above them.
    fn track_list_with(
        &mut self,
        ui: &mut egui::Ui,
        tracks: Vec<Track>,
        origin: &str,
        header: bool,
    ) {
        if tracks.is_empty() {
            ui.add_space(28.0);
            ui.label(
                RichText::new(if self.busy {
                    "Loading music…"
                } else {
                    "No music here yet"
                })
                .color(MUTED),
            );
            return;
        }
        if header {
            ui.horizontal(|ui| {
                if icon_action(ui, true, Icon::Play, "Play all").clicked() {
                    self.start_tracks(tracks.clone(), 0, origin);
                }
                if icon_action(ui, true, Icon::AddToQueue, "Add all to queue").clicked() {
                    self.library.queue.extend(tracks.clone());
                    self.save();
                }
                ui.label(RichText::new(format!("{} songs", tracks.len())).color(MUTED));
            });
            ui.add_space(12.0);
        }
        for (i, t) in tracks.iter().enumerate() {
            ui.push_id((origin, i), |ui| {
                if self.song_row(ui, t, Some(i), ui.available_width()) {
                    self.start_tracks(tracks.clone(), i, origin);
                }
            });
        }
    }
    fn home_view(&mut self, ui: &mut egui::Ui) {
        let recent = if self.recents.is_empty() {
            self.library.history.clone()
        } else {
            self.recents.clone()
        };
        if !recent.is_empty() {
            ui.horizontal(|ui| {
                ui.label(heading("Recents", 24.0));
            });
            ui.add_space(12.0);
            let column = (ui.available_width() / 3.0).clamp(310.0, 510.0);
            egui::ScrollArea::horizontal()
                .id_salt("recent-shelf")
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for base in (0..recent.len().min(32)).step_by(4) {
                            ui.vertical(|ui| {
                                ui.set_min_width(column);
                                for i in base..(base + 4).min(recent.len()) {
                                    ui.push_id(("recent", i), |ui| {
                                        if self.song_row(ui, &recent[i], None, column) {
                                            self.start_tracks(recent.clone(), i, "Recents");
                                        }
                                    });
                                }
                            });
                        }
                    });
                });
            ui.add_space(32.0);
        }
        let shelves = self.home.clone();
        self.shelf_rows(ui, &shelves, "home");
        if self.home_pending > 0 || self.home_more {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(RichText::new("Loading YouTube Music…").color(MUTED));
            });
        } else if self.home.is_empty() {
            ui.label("Could not load Home. Check your connection or account.");
            if ui.button("Retry Home").clicked() {
                self.load_home();
            }
        }
        if self.home_token.is_some() && ui.button("Load more music").clicked() {
            self.more_home();
        }
    }
    /// Titled rows of cards (Home and Explore shelves); a click opens or plays the card.
    fn shelf_rows(&mut self, ui: &mut egui::Ui, shelves: &[HomeShelf], salt: &str) {
        for (shelf_i, shelf) in shelves.iter().enumerate() {
            ui.label(heading(&shelf.title, 24.0));
            if !shelf.subtitle.is_empty() {
                ui.label(RichText::new(&shelf.subtitle).color(MUTED));
            }
            ui.add_space(10.0);
            egui::ScrollArea::horizontal()
                .id_salt(("shelf", salt, shelf_i))
                .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for (i, item) in shelf.items.iter().enumerate() {
                            ui.push_id(("card", salt, shelf_i, i), |ui| {
                                ui.vertical(|ui| {
                                    ui.set_width(180.0);
                                    let (rect, response) = ui.allocate_exact_size(
                                        Vec2::splat(180.0),
                                        egui::Sense::click(),
                                    );
                                    draw_image(ui, &item.thumbnail, rect, None);
                                    if response.hovered() {
                                        ui.painter().rect_filled(
                                            rect,
                                            10.0,
                                            Color32::from_black_alpha(35),
                                        );
                                        let r = Rect::from_center_size(
                                            rect.right_bottom() - Vec2::splat(25.0),
                                            Vec2::splat(34.0),
                                        );
                                        ui.painter().circle_filled(
                                            r.center(),
                                            20.0,
                                            Color32::from_black_alpha(140),
                                        );
                                        widgets::paint(
                                            ui.painter(),
                                            Icon::Play,
                                            r.shrink(7.0),
                                            WHITE,
                                        );
                                    }
                                    if response.clicked() {
                                        self.open_item(item.clone());
                                    }
                                    if ui
                                        .add(
                                            egui::Label::new(heading(&item.title, 15.0))
                                                .truncate()
                                                .sense(egui::Sense::click()),
                                        )
                                        .clicked()
                                    {
                                        self.open_item(item.clone());
                                    }
                                    ui.add(
                                        egui::Label::new(
                                            RichText::new(&item.subtitle).size(14.0).color(MUTED),
                                        )
                                        .truncate(),
                                    );
                                });
                            });
                        }
                    });
                });
            ui.add_space(34.0);
        }
    }
    fn collection_view(&mut self, ui: &mut egui::Ui) {
        if let Some(c) = self.collection.clone() {
            let playlist_id = c.playlist_id.clone();
            ui.horizontal_top(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(155.0), egui::Sense::hover());
                draw_image(ui, &c.thumbnail, r, None);
                ui.vertical(|ui| {
                    ui.label(heading(&c.title, 31.0));
                    ui.label(RichText::new(&c.subtitle).color(MUTED));
                    let Some(id) = playlist_id.clone() else {
                        return;
                    };
                    if !c.description.is_empty() {
                        ui.set_max_width(ui.available_width().min(640.0));
                        ui.label(RichText::new(&c.description).color(MUTED));
                    }
                    ui.add_space(10.0);
                    let more = c.continuation.as_ref().map(|_| format!("VL{id}"));
                    // One row: what plays the playlist, a divider, then what manages it.
                    ui.horizontal_wrapped(|ui| {
                        self.bulk_buttons(ui, &c.title, &c.tracks, more);
                        if c.editable || c.saved.is_some() {
                            toolbar_divider(ui);
                        }
                        if c.editable {
                            if icon_action(ui, true, Icon::Edit, "Rename").clicked() {
                                self.open_playlist_dialog(PlaylistDialog::Rename {
                                    id: id.clone(),
                                    name: c.title.clone(),
                                    current: c.title.clone(),
                                });
                            }
                            if icon_action(ui, true, Icon::Description, "Edit description")
                                .clicked()
                            {
                                self.open_playlist_dialog(PlaylistDialog::Description {
                                    id: id.clone(),
                                    text: c.description.clone(),
                                });
                            }
                            let current = c.privacy.clone().unwrap_or_default();
                            let mut chosen = None;
                            ui.add_enabled_ui(!self.playlist_busy, |ui| {
                                egui::ComboBox::from_id_salt("playlist-privacy")
                                    .selected_text(if current.is_empty() {
                                        "Privacy"
                                    } else {
                                        privacy_label(&current)
                                    })
                                    .show_ui(ui, |ui| {
                                        for (value, label) in services::PLAYLIST_PRIVACY {
                                            if ui
                                                .selectable_label(current == value, label)
                                                .clicked()
                                                && current != value
                                            {
                                                chosen = Some(value.to_owned());
                                            }
                                        }
                                    });
                            });
                            if let Some(privacy) = chosen {
                                let id = id.clone();
                                self.edit_account_playlist(move || {
                                    services::set_playlist_privacy(&id, &privacy)?;
                                    Ok(PlaylistEdit::Privacy { id, privacy })
                                });
                            }
                            if icon_action(ui, true, Icon::Trash, "Delete playlist").clicked() {
                                self.open_playlist_dialog(PlaylistDialog::Delete {
                                    id: id.clone(),
                                    title: c.title.clone(),
                                });
                            }
                        } else if let Some(saved) = c.saved {
                            let (icon, label) = if saved {
                                (Icon::Check, "Remove from library")
                            } else {
                                (Icon::Plus, "Save to library")
                            };
                            if icon_action(ui, !self.playlist_busy, icon, label).clicked() {
                                let id = id.clone();
                                self.edit_account_playlist(move || {
                                    services::set_playlist_saved(&id, !saved)?;
                                    Ok(PlaylistEdit::Saved { id, saved: !saved })
                                });
                            }
                        }
                        if self.playlist_busy {
                            ui.spinner();
                        }
                    });
                });
            });
            ui.add_space(20.0);
            self.track_list_with(ui, c.tracks, &c.title, playlist_id.is_none());
            if c.continuation.is_some()
                && ui
                    .add_enabled(!self.busy, egui::Button::new("Load more tracks"))
                    .clicked()
            {
                self.more_collection();
            }
        } else {
            ui.spinner();
            ui.label("Loading collection…");
        }
    }
    fn content(&mut self, ui: &mut egui::Ui) {
        match self.page.clone() {
            Page::Home => self.home_view(ui),
            Page::Explore => self.explore_view(ui),
            Page::Collection => self.collection_view(ui),
            Page::Search => {
                ui.label(heading(
                    if self.submitted_query.is_empty() {
                        "Search"
                    } else {
                        &self.submitted_query
                    },
                    26.0,
                ));
                self.track_list(ui, self.results.clone(), "Search");
            }
            Page::Library => {
                ui.label(heading("Your library", 26.0));
                ui.add_space(14.0);
                self.account_playlists_view(ui);
                ui.add_space(24.0);
                ui.label(heading("On this computer", 20.0));
                ui.add_space(8.0);
                let locals: Vec<(String, usize)> = self
                    .library
                    .playlists
                    .iter()
                    .map(|p| (p.name.clone(), p.tracks.len()))
                    .collect();
                if locals.is_empty() {
                    ui.label(RichText::new("No playlists on this computer yet").color(MUTED));
                }
                for (i, (name, count)) in locals.iter().enumerate() {
                    let text = format!("{}  ·  {count} songs", truncate(name, 60));
                    if ui
                        .add(egui::Button::new(text).frame(false))
                        .on_hover_cursor(egui::CursorIcon::PointingHand)
                        .clicked()
                    {
                        self.navigate(Page::Playlist(i));
                    }
                }
            }
            Page::History => self.track_list(ui, self.library.history.clone(), "History"),
            Page::Downloads => {
                self.download_manager(ui);
                let tracks: Vec<_> = self
                    .library
                    .local
                    .iter()
                    .filter(|t| {
                        services::plain_path(Path::new(&t.source))
                            .starts_with(&self.library.download_dir)
                    })
                    .cloned()
                    .collect();
                self.track_list(ui, tracks, "Downloads");
            }
            Page::Local => {
                ui.label(heading("Local music", 26.0));
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.import_path)
                            .margin(TEXT_MARGIN)
                            .desired_width(500.0)
                            .hint_text("Audio file or folder path")
                            .id(egui::Id::unique("local-import")),
                    );
                    if ui.button("Import").clicked()
                        || r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter))
                    {
                        let p = PathBuf::from(self.import_path.trim());
                        if !p.as_os_str().is_empty() {
                            self.import(p);
                        }
                    }
                });
                ui.add_space(12.0);
                self.track_list(ui, self.library.local.clone(), "Local music");
            }
            Page::Playlist(i) => {
                let Some(p) = self.library.playlists.get(i).cloned() else {
                    self.page = Page::Library;
                    return;
                };
                ui.label(heading(&p.name, 26.0));
                ui.label(RichText::new(format!("{} songs", p.tracks.len())).color(MUTED));
                ui.add_space(10.0);
                ui.horizontal_wrapped(|ui| {
                    self.bulk_buttons(ui, &p.name, &p.tracks, None);
                    toolbar_divider(ui);
                    if icon_action(ui, true, Icon::Edit, "Rename").clicked() {
                        self.open_playlist_dialog(PlaylistDialog::RenameLocal {
                            index: i,
                            name: p.name.clone(),
                            current: p.name.clone(),
                        });
                    }
                    if icon_action(ui, true, Icon::Trash, "Delete playlist").clicked() {
                        self.open_playlist_dialog(PlaylistDialog::DeleteLocal {
                            index: i,
                            title: p.name.clone(),
                        });
                    }
                });
                ui.add_space(14.0);
                self.track_list_with(ui, p.tracks, &p.name, false);
            }
            Page::Settings => self.settings_view(ui),
        }
    }
}

/// The outgoing side of a crossfade. Dropping it stops its mpv.
struct Fade {
    old: Player,
    /// When the incoming track became audible; the blend waits for it.
    started: Option<Instant>,
    seconds: f32,
}

/// Equal-power gains (outgoing, incoming) at blend progress `t` in 0..=1.
pub(crate) fn fade_gains(t: f32) -> (f64, f64) {
    let angle = f64::from(t.clamp(0.0, 1.0)) * std::f64::consts::FRAC_PI_2;
    (angle.cos(), angle.sin())
}

#[cfg(test)]
mod fade_tests {
    use super::fade_gains;

    #[test]
    fn crossfade_keeps_constant_power() {
        assert_eq!(fade_gains(0.0), (1.0, 0.0));
        let (old, new) = fade_gains(1.0);
        assert!(old.abs() < 1e-9 && (new - 1.0).abs() < 1e-9);
        for t in [0.1, 0.25, 0.5, 0.9, 2.0, -1.0] {
            let (old, new) = fade_gains(t);
            assert!((old * old + new * new - 1.0).abs() < 1e-9);
        }
    }
}

/// Add imported `tracks` to `local`. A file already there (matched by path, ignoring the Windows
/// `\\?\` prefix older imports stored) is refreshed in place, keeping its artwork if the new
/// entry has none, so re-importing repairs stale titles and artists.
fn merge_local(local: &mut Vec<Track>, tracks: Vec<Track>) {
    let path = |t: &Track| services::plain_path(Path::new(&t.source));
    for mut t in tracks {
        match local.iter_mut().find(|s| path(s) == path(&t)) {
            Some(old) => {
                if t.thumbnail.is_empty() {
                    t.thumbnail = std::mem::take(&mut old.thumbnail);
                }
                *old = t;
            }
            None => local.push(t),
        }
    }
}

/// Upstream `downloadSummary`.
fn download_summary(items: &[DownloadItem]) -> String {
    let failed = items
        .iter()
        .filter(|d| matches!(d.state, DownloadState::Failed(_)))
        .count();
    let busy = failed < items.len();
    match items.len() {
        0 => "Nothing downloading".into(),
        _ if failed > 0 && !busy => match failed {
            1 => "1 download failed".into(),
            n => format!("{n} downloads failed"),
        },
        1 => "1 track".into(),
        n => format!("{n} tracks"),
    }
}

enum PlaylistDialog {
    /// Pick where `track` goes: an account playlist, a local one, or a new one.
    AddTo {
        track: Track,
    },
    Create {
        name: String,
        privacy: String,
        seed: Option<Track>,
    },
    Rename {
        id: String,
        name: String,
        current: String,
    },
    RenameLocal {
        index: usize,
        name: String,
        current: String,
    },
    Description {
        id: String,
        text: String,
    },
    Delete {
        id: String,
        title: String,
    },
    DeleteLocal {
        index: usize,
        title: String,
    },
}

/// What a click in the open playlist dialog asks for.
enum DialogAct {
    None,
    Submit,
    Cancel,
    Back(Track),
    New(Track),
    PickAccount(String, Track),
    PickLocal(usize, Track),
}

#[derive(Clone, Copy)]
enum PlaylistBulk {
    Play,
    Shuffle,
    Queue,
    Download,
}

/// A finished account-playlist change, applied to what is on screen without refetching.
enum PlaylistEdit {
    Created {
        title: String,
        seeded: bool,
    },
    Deleted {
        id: String,
    },
    Renamed {
        id: String,
        name: String,
    },
    Added {
        id: String,
        track: Track,
        set_id: Option<String>,
    },
    Removed {
        id: String,
        video_id: String,
    },
    Saved {
        id: String,
        saved: bool,
    },
    Privacy {
        id: String,
        privacy: String,
    },
    Described {
        id: String,
        description: String,
    },
    /// The whole playlist in its new order.
    Reordered(Box<Collection>),
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum CreateTarget {
    Account,
    Local,
}

/// YouTube can only hold tracks it knows: a video id and a web source, not a local file.
fn is_youtube_track(track: &Track) -> bool {
    !track.id.is_empty() && track.source.starts_with("https://")
}

/// A new playlist goes to the account when signed in and the seed (if any) is a YouTube track.
fn create_target(signed_in: bool, seed: Option<&Track>) -> CreateTarget {
    if signed_in && seed.is_none_or(is_youtube_track) {
        CreateTarget::Account
    } else {
        CreateTarget::Local
    }
}

/// Where an entry at `from` lands one step up or down; None at the end of the list.
fn step_target(len: usize, from: usize, up: bool) -> Option<usize> {
    if from >= len {
        return None;
    }
    if up {
        from.checked_sub(1)
    } else {
        (from + 1 < len).then_some(from + 1)
    }
}

/// `items` with the entry at `from` taken out and put back at `to`.
fn moved<T: Clone>(items: &[T], from: usize, to: usize) -> Vec<T> {
    let mut out = items.to_vec();
    if from < out.len() && to < out.len() {
        let item = out.remove(from);
        out.insert(to, item);
    }
    out
}

/// The playlist entry id of every track in order, when each one has one.
fn set_id_order(
    tracks: &[Track],
    set_ids: &std::collections::HashMap<String, String>,
) -> Option<Vec<String>> {
    tracks.iter().map(|t| set_ids.get(&t.id).cloned()).collect()
}

fn privacy_label(privacy: &str) -> &str {
    services::PLAYLIST_PRIVACY
        .iter()
        .find(|(value, _)| *value == privacy)
        .map_or(privacy, |(_, label)| *label)
}

fn section_heading(ui: &mut egui::Ui, text: &str) {
    ui.add_space(10.0);
    ui.label(RichText::new(text.to_uppercase()).size(11.0).color(MUTED));
}

#[cfg(test)]
mod playlist_tests {
    use super::*;

    fn track(id: &str, source: &str) -> Track {
        Track {
            id: id.into(),
            source: source.into(),
            ..Default::default()
        }
    }

    #[test]
    fn merge_local_refreshes_stale_imports() {
        let mut old = track(r"\\?\C:\Music\a.opus", r"\\?\C:\Music\a.opus");
        old.title = "a".into();
        old.thumbnail = "art".into();
        let mut local = vec![old];
        let mut fresh = track(r"C:\Music\a.opus", r"C:\Music\a.opus");
        fresh.title = "Real title".into();
        fresh.artist = "Artist".into();
        merge_local(
            &mut local,
            vec![fresh, track(r"C:\Music\b.opus", r"C:\Music\b.opus")],
        );
        assert_eq!(local.len(), 2);
        assert_eq!(local[0].source, r"C:\Music\a.opus");
        assert_eq!(
            (&*local[0].title, &*local[0].artist),
            ("Real title", "Artist")
        );
        assert_eq!(local[0].thumbnail, "art");
    }

    #[test]
    fn create_target_follows_sign_in_and_seed() {
        let web = track("abc", "https://music.youtube.com/watch?v=abc");
        let file = track("", "C:/music/a.mp3");
        assert_eq!(create_target(true, None), CreateTarget::Account);
        assert_eq!(create_target(true, Some(&web)), CreateTarget::Account);
        assert_eq!(create_target(true, Some(&file)), CreateTarget::Local);
        assert_eq!(create_target(false, None), CreateTarget::Local);
        assert_eq!(create_target(false, Some(&web)), CreateTarget::Local);
    }

    #[test]
    fn one_step_moves() {
        assert_eq!(step_target(3, 0, true), None);
        assert_eq!(step_target(3, 1, true), Some(0));
        assert_eq!(step_target(3, 1, false), Some(2));
        assert_eq!(step_target(3, 2, false), None);
        assert_eq!(step_target(3, 3, true), None);
        let ids = ["a", "b", "c"].map(String::from);
        assert_eq!(moved(&ids, 0, 1), ["b", "a", "c"].map(String::from));
        assert_eq!(moved(&ids, 2, 1), ["a", "c", "b"].map(String::from));
        assert_eq!(moved(&ids, 1, 9), ids);
    }

    #[test]
    fn reorder_needs_every_set_id() {
        let tracks = [track("a", ""), track("b", "")];
        let mut set_ids = std::collections::HashMap::new();
        set_ids.insert("a".to_owned(), "sa".to_owned());
        assert_eq!(set_id_order(&tracks, &set_ids), None);
        set_ids.insert("b".to_owned(), "sb".to_owned());
        assert_eq!(
            set_id_order(&tracks, &set_ids),
            Some(vec!["sa".to_owned(), "sb".to_owned()])
        );
    }

    #[test]
    fn privacy_labels() {
        assert_eq!(privacy_label("UNLISTED"), "Unlisted");
        assert_eq!(privacy_label("other"), "other");
    }
}

/// Transfers at once; upstream's `DesktopDownloadQueue.WORKERS`.
const DOWNLOAD_WORKERS: usize = 4;

struct DownloadItem {
    track: Track,
    state: DownloadState,
    cancel: Arc<AtomicBool>,
}

enum DownloadState {
    Queued,
    Running(f32),
    Failed(String),
}

/// The sign-in dialog, while it is open (Meiro `signInState`).
#[derive(Default)]
struct SignIn {
    open: bool,
    err: String,
    /// The pasted Cookie header.
    cookie: String,
    /// The browser a session could not be imported from, so the dialog points at pasting.
    import_failed: String,
    /// Set while the sign-in window waits for the user.
    in_window: bool,
    /// Shows the ways to sign in that copy a session from a browser the user keeps using.
    advanced: bool,
    /// The accounts an imported session can act as; the picker shows while this is set.
    accounts: Option<Vec<auth::Channel>>,
    /// The imported cookie, kept private until an account is chosen.
    imported: String,
    /// The browser it came from.
    source: &'static str,
    busy: bool,
    cancel: Arc<AtomicBool>,
}

enum AccountAction {
    Switch(auth::Channel),
    Settings,
    SignOut,
    SignIn,
}

/// The body of a sign-in dialog, as wide as Meiro's.
fn dialog_frame(ui: &mut egui::Ui, add: impl FnOnce(&mut egui::Ui)) {
    egui::Frame::new()
        .inner_margin(egui::Margin::symmetric(14, 12))
        .show(ui, |ui| {
            ui.set_width(520.0);
            ui.spacing_mut().item_spacing.y = 4.0;
            add(ui);
        });
}

/// A round picture, or the first letter of `name` on a disc when there is none.
fn avatar(ui: &egui::Ui, url: &str, name: &str, rect: Rect) {
    let radius = rect.width() / 2.0;
    if url.starts_with("https://") {
        egui::Image::new(url)
            .fit_to_exact_size(rect.size())
            .corner_radius(radius)
            .paint_at(ui, rect);
        return;
    }
    ui.painter()
        .circle_filled(rect.center(), radius, Color32::from_rgb(78, 88, 70));
    let letter: String = name
        .chars()
        .next()
        .into_iter()
        .flat_map(char::to_uppercase)
        .collect();
    ui.painter().text(
        rect.center(),
        egui::Align2::CENTER_CENTER,
        letter,
        FontId::new(radius, egui::FontFamily::Name("bold".into())),
        WHITE,
    );
}

/// Explanatory text in a dialog.
/// A list popup that hangs below `anchor` while `popup_id` is open in egui's memory (the caller
/// toggles it). It never flips above the anchor, and closes on Escape or per `close`.
fn popup_below_widget(
    popup_id: egui::Id,
    anchor: &egui::Response,
    close: egui::PopupCloseBehavior,
    add_contents: impl FnOnce(&mut egui::Ui),
) {
    egui::Popup::from_response(anchor)
        .id(popup_id)
        .open_memory(None)
        .align_alternatives(&[])
        .close_behavior(close)
        .layout(egui::Layout::top_down_justified(Align::LEFT))
        .show(add_contents);
}

fn muted(ui: &mut egui::Ui, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).color(MUTED).size(13.0)).wrap());
}

impl eframe::App for YTDugem {
    /// Runs before each `ui` and keeps ticking while the window is minimized or hidden in the
    /// tray (eframe then runs no UI pass), so playback events and tray actions are still handled.
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.events(ctx);
        if services::take_sign_in_rejected() {
            self.message = "Your YouTube sign-in has expired, so public music is shown. Open your account to sign in again.".into();
        }
        self.tray_step(ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
    }

    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let ctx = &ui.ctx().clone();
        let typing = ctx.egui_wants_keyboard_input();
        // While the palette is open it owns Escape (its modal closes on it).
        let palette_open = self.palette_open;
        let (search, local, quit, escape, palette) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::CTRL, egui::Key::F),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::L),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Q),
                !palette_open && i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::K),
            )
        });
        if palette {
            self.toggle_palette();
        }
        if search {
            ctx.memory_mut(|m| m.request_focus(egui::Id::unique("search_input")));
        }
        if local {
            self.page = Page::Local;
            ctx.memory_mut(|m| m.request_focus(egui::Id::unique("local-import")));
        }
        if quit {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        if escape {
            self.sheet_back(ctx);
            if self.sign_in.open {
                self.dismiss_sign_in();
            }
        }
        ctx.input_mut(|i| {
            if i.consume_key(egui::Modifiers::CTRL, egui::Key::Enter) {
                let tracks = match self.page {
                    Page::Local => self.library.local.clone(),
                    Page::Search => self.results.clone(),
                    Page::Collection => self
                        .collection
                        .as_ref()
                        .map(|c| c.tracks.clone())
                        .unwrap_or_default(),
                    _ => vec![],
                };
                if !tracks.is_empty() {
                    self.start_tracks(tracks, 0, "Selection");
                }
            }
            if i.key_pressed(egui::Key::F11) {
                self.expanded = !self.expanded;
            }
        });
        if self.expanded && !typing {
            ctx.input(|i| {
                for (key, pane) in [
                    (egui::Key::L, PlayerPane::Lyrics),
                    (egui::Key::Q, PlayerPane::Queue),
                ] {
                    if !i.modifiers.any() && i.key_pressed(key) {
                        self.tab = if self.tab == pane {
                            PlayerPane::Main
                        } else {
                            pane
                        };
                    }
                }
            });
        }
        if !typing {
            ctx.input_mut(|i| {
                if i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowLeft) {
                    self.previous();
                }
                if i.consume_key(egui::Modifiers::ALT, egui::Key::ArrowRight) {
                    self.next(false);
                }
                if i.key_pressed(egui::Key::Space) {
                    self.toggle();
                }
            });
        }
        // Upstream DesktopPlayerSheet: 360 ms up, 280 ms down, FastOutSlowIn.
        let shown = ctx.animate_bool_with_time_and_easing(
            egui::Id::unique("player-sheet-shown"),
            self.expanded,
            if self.expanded { 0.36 } else { 0.28 },
            egui::emath::easing::cubic_in_out,
        );
        // The page stays drawn while the sheet is dragged down, so it shows behind it.
        if shown < 1.0 || self.sheet_drag > 0.0 {
            if self.sidebar_open {
                egui::Panel::left("sidebar")
                    .exact_size(258.0)
                    .resizable(false)
                    .frame(egui::Frame::new().fill(SIDE).inner_margin(0.0))
                    .show(ui, |ui| {
                        self.sidebar(ui, ui.max_rect());
                    });
            }
            egui::Panel::top("topbar")
                .exact_size(78.0)
                .frame(egui::Frame::new().fill(SIDE).inner_margin(0.0))
                .show(ui, |ui| {
                    self.topbar(ui, ui.max_rect());
                });
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(BG)
                        .inner_margin(egui::Margin::same(28)),
                )
                .show(ui, |ui| {
                    // One scroll position per page: opening Settings from a scrolled Home page
                    // used to land halfway down it.
                    egui::ScrollArea::vertical()
                        .id_salt(("page", self.page.clone(), self.explore_scroll_key()))
                        .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
                        .show(ui, |ui| {
                            if !self.message.is_empty() {
                                ui.label(RichText::new(&self.message).color(MUTED));
                            }
                            self.content(ui);
                        });
                });
        }
        if shown > 0.0 {
            self.expanded_view(ctx, shown);
        }
        self.palette(ctx);
        self.sign_in_dialog(ctx);
        self.playlist_dialog(ctx);
        self.download_delete_dialog(ctx);
        self.screenshot_step(ctx);
    }
}

/// Translucent pill buttons and combo boxes for controls drawn over the player's artwork backdrop.
/// The query a group's rows are filtered by, or `None` to hide the group. A group whose own
/// name matches shows whole (upstream `SettingsGroup`).
fn group_query(query: &str, group: &str, rows: &[&str]) -> Option<String> {
    if query.is_empty() || group.to_lowercase().contains(query) {
        Some(String::new())
    } else if rows.iter().any(|r| hit(query, r)) {
        Some(query.to_owned())
    } else {
        None
    }
}

fn hit(query: &str, title: &str) -> bool {
    query.is_empty() || title.to_lowercase().contains(query)
}

fn settings_group(ui: &mut egui::Ui, title: &str, add: impl FnOnce(&mut egui::Ui)) {
    ui.label(
        RichText::new(title.to_uppercase())
            .size(11.5)
            .color(MUTED)
            .strong(),
    );
    ui.add_space(4.0);
    egui::Frame::new()
        .fill(Color32::from_rgb(35, 37, 33))
        // Clear of the page scrollbar, which sits on the content's right edge.
        .outer_margin(egui::Margin {
            right: 14,
            ..Default::default()
        })
        .stroke(Stroke::new(1.0_f32, Color32::from_rgb(45, 47, 43)))
        .corner_radius(12.0)
        .inner_margin(egui::Margin::symmetric(18, 4))
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = 0.0;
            add(ui);
        });
    ui.add_space(22.0);
}

/// Title and explanation on the left, `control` right-aligned (added right to left).
fn setting_row(
    ui: &mut egui::Ui,
    title: &str,
    subtitle: &str,
    control: impl FnOnce(&mut egui::Ui),
) -> egui::Response {
    // Rows after the first are divided by a hairline, as in upstream's settings list.
    let first = ui.min_rect().height() < 1.0;
    if !first {
        let x = ui.max_rect().x_range();
        let y = ui.cursor().top();
        ui.painter()
            .hline(x, y, Stroke::new(1.0_f32, Color32::from_rgb(46, 48, 44)));
    }
    ui.horizontal(|ui| {
        ui.set_min_height(62.0);
        let text = (ui.available_width() * 0.5).max(220.0);
        let label = ui
            .allocate_ui_with_layout(
                Vec2::new(text, 62.0),
                egui::Layout::top_down(Align::Min),
                |ui| {
                    ui.spacing_mut().item_spacing.y = 3.0;
                    // Centred by hand against the 62 px row; a wrapped subtitle grows the row.
                    ui.add_space(if subtitle.is_empty() { 21.0 } else { 12.0 });
                    let r = ui.add(
                        egui::Label::new(RichText::new(title).size(15.0).color(WHITE))
                            .sense(egui::Sense::click()),
                    );
                    if !subtitle.is_empty() {
                        ui.add(
                            egui::Label::new(RichText::new(subtitle).size(12.5).color(MUTED))
                                .wrap(),
                        );
                    }
                    r
                },
            )
            .inner;
        ui.with_layout(egui::Layout::right_to_left(Align::Center), control);
        label
    })
    .inner
}

/// A row with a switch; clicking the title toggles it too. Returns whether it changed.
fn toggle_row(ui: &mut egui::Ui, title: &str, subtitle: &str, on: &mut bool) -> bool {
    let mut changed = false;
    let label = setting_row(ui, title, subtitle, |ui| {
        changed |= toggle_switch(ui, on).clicked();
    });
    if label.clicked() {
        *on = !*on;
        changed = true;
    }
    if label.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    changed
}

fn toggle_switch(ui: &mut egui::Ui, on: &mut bool) -> egui::Response {
    let (rect, mut response) = ui.allocate_exact_size(Vec2::new(40.0, 22.0), egui::Sense::click());
    if response.clicked() {
        *on = !*on;
        response.mark_changed();
    }
    response.widget_info(|| egui::WidgetInfo::selected(egui::Role::CheckBox, true, *on, ""));
    let t = ui.ctx().animate_bool(response.id, *on);
    let track = if *on {
        Color32::from_gray(236)
    } else {
        widgets::white_alpha(if response.hovered() { 60 } else { 40 })
    };
    ui.painter().rect_filled(rect, 11.0, track);
    let knob = egui::pos2(
        egui::lerp((rect.left() + 11.0)..=(rect.right() - 11.0), t),
        rect.center().y,
    );
    ui.painter().circle_filled(
        knob,
        8.0,
        if *on {
            Color32::from_rgb(20, 21, 19)
        } else {
            Color32::from_gray(200)
        },
    );
    response
}

/// Size of one caption button.
const CAPTION: Vec2 = Vec2::new(44.0, 26.0);

/// Windows-style caption buttons at the top-right: flat, a light wash on hover and red behind
/// Close, 1 px glyphs, and the restore glyph while maximized.
fn window_controls(ui: &mut egui::Ui, bar: Rect) {
    let maximized = ui.ctx().input(|s| s.viewport().maximized.unwrap_or(false));
    let labels = [
        "Minimize",
        if maximized {
            "Restore down"
        } else {
            "Maximize"
        },
        "Close",
    ];
    for (i, label) in labels.into_iter().enumerate() {
        let r = Rect::from_min_size(
            Pos2::new(bar.right() - CAPTION.x * (3 - i) as f32, bar.top()),
            CAPTION,
        );
        let response = ui.interact(
            r,
            ui.make_persistent_id(("caption", i)),
            egui::Sense::click(),
        );
        response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
        let close = i == 2;
        let pressed = response.is_pointer_button_down_on();
        if response.hovered() {
            let fill = match (close, pressed) {
                (true, false) => Color32::from_rgb(196, 43, 28),
                (true, true) => Color32::from_rgb(160, 32, 22),
                (false, false) => widgets::white_alpha(20),
                (false, true) => widgets::white_alpha(32),
            };
            ui.painter().rect_filled(r, 0.0, fill);
        }
        let color = if response.hovered() {
            Color32::WHITE
        } else {
            Color32::from_gray(200)
        };
        let stroke = Stroke::new(1.0_f32, color);
        let c = r.center();
        let s = 5.0;
        let p = ui.painter();
        match i {
            0 => {
                p.hline((c.x - s)..=(c.x + s), c.y, stroke);
            }
            1 if maximized => {
                let front = Rect::from_center_size(c + Vec2::new(-1.0, 1.0), Vec2::splat(8.0));
                p.rect_stroke(front, 1.0, stroke, egui::StrokeKind::Middle);
                // The back window shows only above and to the right of the front one.
                let (t, rgt) = (front.top() - 2.0, front.right() + 2.0);
                p.line_segment(
                    [Pos2::new(front.left() + 2.0, t), Pos2::new(rgt, t)],
                    stroke,
                );
                p.line_segment(
                    [Pos2::new(rgt, t), Pos2::new(rgt, front.bottom() - 2.0)],
                    stroke,
                );
            }
            1 => {
                let square = Rect::from_center_size(c, Vec2::splat(2.0 * s));
                p.rect_stroke(square, 1.0, stroke, egui::StrokeKind::Middle);
            }
            _ => {
                p.line_segment([c - Vec2::splat(s), c + Vec2::splat(s)], stroke);
                p.line_segment([c + Vec2::new(s, -s), c + Vec2::new(-s, s)], stroke);
            }
        }
        if response.clicked() {
            let ctx = ui.ctx();
            match i {
                0 => ctx.send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                1 => ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(!maximized)),
                _ => ctx.send_viewport_cmd(egui::ViewportCommand::Close),
            }
        }
    }
}

fn soft_controls(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets;
    for (state, alpha) in [
        (&mut w.inactive, 16),
        (&mut w.hovered, 30),
        (&mut w.active, 40),
        (&mut w.open, 30),
    ] {
        state.weak_bg_fill = widgets::white_alpha(alpha);
        state.bg_fill = widgets::white_alpha(alpha);
        state.bg_stroke = Stroke::NONE;
        state.corner_radius = egui::CornerRadius::same(14);
        state.fg_stroke.color = WHITE;
    }
    ui.spacing_mut().button_padding = Vec2::new(12.0, 5.0);
    // A switched-on toggle (Translate, AutoPlay) reads as a solid light pill.
    let selection = &mut ui.visuals_mut().selection;
    selection.bg_fill = Color32::from_gray(236);
    selection.stroke.color = Color32::from_rgb(20, 21, 19);
}
fn region<R>(ui: &mut egui::Ui, rect: Rect, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(Align::LEFT)),
    );
    f(&mut child)
}
/// Paint `text` on one line, ending in "…" where it would pass `max_width`. Measured, not
/// counted: a CJK title is about twice as wide per character as a Latin one, so cutting at a
/// character count let it run over the row's menu button.
fn fit_text(
    painter: &egui::Painter,
    pos: Pos2,
    anchor: egui::Align2,
    text: &str,
    font: FontId,
    color: Color32,
    max_width: f32,
) -> Rect {
    let mut job = egui::text::LayoutJob::single_section(
        text.to_owned(),
        egui::TextFormat::simple(font, color),
    );
    job.wrap = egui::text::TextWrapping {
        max_width: max_width.max(8.0),
        max_rows: 1,
        break_anywhere: true,
        overflow_character: Some('…'),
    };
    let galley = painter.layout_job(job);
    let rect = anchor.anchor_size(pos, galley.size());
    painter.galley(rect.min, galley, color);
    rect
}
fn truncate(s: &str, max: usize) -> String {
    if s.chars().count() > max {
        let mut out: String = s.chars().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    } else {
        s.into()
    }
}
fn nav_row(
    ui: &mut egui::Ui,
    r: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    icon: Icon,
    label: &str,
    selected: bool,
) -> egui::Response {
    nav_row_with(ui, r, id, label, selected, |ui, color| {
        widgets::paint(
            ui.painter(),
            icon,
            Rect::from_min_size(r.min + Vec2::new(11.0, 10.0), Vec2::splat(18.0)),
            color,
        );
    })
}
/// A sidebar playlist row: the playlist's artwork stands where `nav_row` draws its icon.
fn nav_thumb_row(
    ui: &mut egui::Ui,
    r: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    thumbnail: &str,
    label: &str,
    selected: bool,
) -> egui::Response {
    nav_row_with(ui, r, id, label, selected, |ui, _| {
        let side = (r.height() - 8.0).min(28.0);
        let art =
            Rect::from_center_size(Pos2::new(r.left() + 20.0, r.center().y), Vec2::splat(side));
        draw_image(ui, thumbnail, art, None);
    })
}
fn nav_row_with(
    ui: &mut egui::Ui,
    r: Rect,
    id: impl std::hash::Hash + std::fmt::Debug,
    label: &str,
    selected: bool,
    leading: impl FnOnce(&mut egui::Ui, Color32),
) -> egui::Response {
    let response = ui.interact(r, ui.make_persistent_id(id), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, label));
    if selected || response.hovered() {
        ui.painter()
            .rect_filled(r, 6.0, widgets::white_alpha(if selected { 30 } else { 12 }));
    }
    let color = if selected { WHITE } else { MUTED };
    leading(ui, color);
    fit_text(
        ui.painter(),
        Pos2::new(r.left() + 43.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        label,
        FontId::proportional(15.0),
        color,
        r.right() - r.left() - 51.0,
    );
    response
}
/// A round icon button whose label is its tooltip; disabled buttons dim instead of vanishing.
fn icon_action(ui: &mut egui::Ui, enabled: bool, icon: Icon, label: &str) -> egui::Response {
    ui.add_enabled_ui(enabled, |ui| widgets::button(ui, icon, 36.0, false, label))
        .inner
}
fn toolbar_divider(ui: &mut egui::Ui) {
    ui.add_space(6.0);
    let (sep, _) = ui.allocate_exact_size(Vec2::new(1.0, 22.0), egui::Sense::hover());
    ui.painter().rect_filled(sep, 0.0, Color32::from_gray(63));
    ui.add_space(6.0);
}
fn draw_cover(
    ui: &mut egui::Ui,
    t: &Track,
    r: Rect,
    texture: Option<Option<&egui::TextureHandle>>,
) {
    draw_image(ui, &t.thumbnail, r, texture.flatten());
}
fn draw_image(ui: &mut egui::Ui, url: &str, r: Rect, texture: Option<&egui::TextureHandle>) {
    // Off-screen cards (long shelves, scrolled lists) would otherwise all download and decode
    // at once; egui 0.31 decodes on the GUI thread, so only load what is actually shown.
    if !ui.is_rect_visible(r) {
        return;
    }
    if let Some(tex) = texture {
        egui::Image::new(tex)
            .fit_to_exact_size(r.size())
            .corner_radius(8.0)
            .paint_at(ui, r);
    } else if url.starts_with("https://") {
        // A few shared buckets so rows and cards reuse one cached download each.
        let px = r.width().max(r.height()) * ui.ctx().pixels_per_point();
        let px = if px <= 120.0 {
            120
        } else if px <= 240.0 {
            240
        } else {
            544
        };
        egui::Image::new(artwork_at(url, px))
            .fit_to_exact_size(r.size())
            .corner_radius(8.0)
            .paint_at(ui, r);
    } else {
        ui.painter()
            .rect_filled(r, 8.0, Color32::from_rgb(51, 53, 48));
        widgets::paint(ui.painter(), Icon::Logo, r.shrink(r.width() * 0.3), MUTED);
    }
}

/// Upstream `artworkAt`: ask YouTube's image hosts for the size actually drawn instead of the
/// largest thumbnail, by rewriting `w<N>-h<N>` and, on Google image hosts, `=s<N>`.
fn artwork_at(url: &str, px: u32) -> String {
    fn digits(s: &str) -> usize {
        s.bytes().take_while(u8::is_ascii_digit).count()
    }
    let host = url
        .split("://")
        .nth(1)
        .unwrap_or("")
        .split(['/', ':', '?'])
        .next()
        .unwrap_or("")
        .to_ascii_lowercase();
    let google = ["ggpht.com", "googleusercontent.com"]
        .iter()
        .any(|d| host == *d || host.ends_with(&format!(".{d}")));
    let mut out = String::with_capacity(url.len());
    let mut rest = url;
    while !rest.is_empty() {
        if let Some(tail) = rest.strip_prefix('w') {
            let w = digits(tail);
            if w > 0 && tail[w..].starts_with("-h") {
                let h = digits(&tail[w + 2..]);
                if h > 0 {
                    out.push_str(&format!("w{px}-h{px}"));
                    rest = &tail[w + 2 + h..];
                    continue;
                }
            }
        }
        if google {
            if let Some(tail) = rest.strip_prefix("=s") {
                let s = digits(tail);
                if s > 0 {
                    out.push_str(&format!("=s{px}"));
                    rest = &tail[s..];
                    continue;
                }
            }
        }
        let c = rest.chars().next().unwrap();
        out.push(c);
        rest = &rest[c.len_utf8()..];
    }
    out
}

/// Inner margin of a text field. egui 0.37 derives it from `button_padding`, which this app sets
/// large for buttons; egui 0.31 used this fixed margin.
const TEXT_MARGIN: egui::Margin = egui::Margin::symmetric(4, 2);

/// Adds a text field whose hint text is muted. egui 0.37 colours every hint from the style (an
/// explicit colour on the hint is overridden), and the style makes it white, as 0.31 did for
/// hints that carried no colour of their own.
fn muted_hint(ui: &mut egui::Ui, edit: egui::TextEdit<'_>) -> egui::Response {
    ui.scope(|ui| {
        ui.visuals_mut().weak_text_color = Some(MUTED);
        ui.add(edit)
    })
    .inner
}

/// A text field that paints no background or outline of its own, with the egui 0.31 margin.
fn bare_text_frame() -> egui::Frame {
    egui::Frame::NONE.inner_margin(TEXT_MARGIN)
}

/// egui 0.32 to 0.37 changed several defaults (rounder, larger, slower widgets, a rubber-banding
/// trackpad scroll). The layout here was tuned against 0.31, so those values are kept.
fn legacy_look(style: &mut egui::Style) {
    use egui::style::{HandleShape, WidgetVisuals, Widgets};
    let gray = Color32::from_gray;
    let widget =
        |weak_bg: u8, bg: u8, bg_stroke: Stroke, fg_stroke: Stroke, radius: u8, expansion: f32| {
            WidgetVisuals {
                weak_bg_fill: gray(weak_bg),
                bg_fill: gray(bg),
                bg_stroke,
                fg_stroke,
                corner_radius: egui::CornerRadius::same(radius),
                expansion,
            }
        };
    style.animation_time = 1.0 / 12.0;
    style.spacing.interact_size = Vec2::new(40.0, 18.0);
    style.spacing.scroll.kinetic.rubber_band = false;
    style.visuals.handle_shape = HandleShape::Circle;
    // Hint text was drawn in the normal text colour, not the dimmer "weak" one.
    style.visuals.weak_text_color = Some(WHITE);
    style.visuals.widgets = Widgets {
        noninteractive: widget(
            27,
            27,
            Stroke::new(1.0_f32, gray(60)),
            Stroke::new(1.0_f32, gray(140)),
            2,
            0.0,
        ),
        inactive: widget(
            60,
            60,
            Stroke::NONE,
            Stroke::new(1.0_f32, gray(180)),
            2,
            0.0,
        ),
        hovered: widget(
            70,
            70,
            Stroke::new(1.0_f32, gray(150)),
            Stroke::new(1.5_f32, gray(240)),
            3,
            1.0,
        ),
        active: widget(
            55,
            55,
            Stroke::new(1.0_f32, Color32::WHITE),
            Stroke::new(2.0_f32, Color32::WHITE),
            2,
            1.0,
        ),
        open: widget(
            45,
            27,
            Stroke::new(1.0_f32, gray(60)),
            Stroke::new(1.0_f32, gray(210)),
            2,
            0.0,
        ),
    };
}

/// Downward shift, in em, that lines a fallback face's baseline up with the primary's.
/// epaint centers each face's row height inside the primary's, so faces whose
/// ascent/descent/leading balance differs (CJK vs Inter) otherwise sit on another baseline.
fn baseline_shift_em(primary: &[u8], fallback: &[u8]) -> Option<f32> {
    use skrifa::instance::{LocationRef, Size};
    use skrifa::MetadataProvider;
    let metrics = |bytes: &[u8]| {
        let m = skrifa::FontRef::from_index(bytes, 0)
            .ok()?
            .metrics(Size::unscaled(), LocationRef::default());
        let em = f32::from(m.units_per_em);
        (em > 0.0).then(|| (m.ascent / em, (m.ascent - m.descent + m.leading) / em))
    };
    let (ascent_p, height_p) = metrics(primary)?;
    let (ascent_f, height_f) = metrics(fallback)?;
    Some(ascent_p - (ascent_f + 0.5 * (height_p - height_f)))
}

fn font_definitions() -> egui::FontDefinitions {
    let inter_regular: &'static [u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(egui::FontData::from_static(inter_regular)),
    );
    fonts.font_data.insert(
        "inter-semibold".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-SemiBold.ttf"
        ))),
    );
    fonts
        .families
        .get_mut(&egui::FontFamily::Proportional)
        .unwrap()
        .insert(0, "inter".into());
    // Inter has no CJK glyphs; fall back to fonts Windows ships so Japanese/Chinese/Korean
    // titles render instead of empty boxes. One font per script, first one found wins.
    let groups: [&[&str]; 3] = [
        &[
            r"C:\Windows\Fonts\YuGothM.ttc",
            r"C:\Windows\Fonts\meiryo.ttc",
            r"C:\Windows\Fonts\msgothic.ttc",
            "/usr/share/fonts/opentype/noto/NotoSansCJK-Regular.ttc",
            "/usr/share/fonts/noto-cjk/NotoSansCJK-Regular.ttc",
        ],
        &[r"C:\Windows\Fonts\msyh.ttc", r"C:\Windows\Fonts\simsun.ttc"],
        &[r"C:\Windows\Fonts\malgun.ttf"],
    ];
    for (i, paths) in groups.iter().enumerate() {
        if let Some(bytes) = paths.iter().find_map(|p| std::fs::read(p).ok()) {
            let name = format!("cjk-{i}");
            let y_offset_factor = baseline_shift_em(inter_regular, &bytes).unwrap_or(0.0);
            let data = egui::FontData::from_owned(bytes).tweak(egui::FontTweak {
                y_offset_factor,
                ..Default::default()
            });
            fonts.font_data.insert(name.clone(), Arc::new(data));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push(name.clone());
            }
        }
    }
    widgets::install_icon_fonts(&mut fonts);
    let mut bold = vec!["inter-semibold".into()];
    bold.extend(fonts.families[&egui::FontFamily::Proportional].clone());
    fonts
        .families
        .insert(egui::FontFamily::Name("bold".into()), bold);
    fonts
}

fn configure_style(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    ctx.set_fonts(font_definitions());
    let mut style = (*ctx.global_style()).clone();
    style.visuals = egui::Visuals::dark();
    legacy_look(&mut style);
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = Color32::from_rgb(35, 36, 33);
    style.visuals.override_text_color = Some(WHITE);
    style.visuals.faint_bg_color = widgets::white_alpha(8);
    style.visuals.extreme_bg_color = Color32::from_rgb(31, 32, 29);
    style.visuals.selection.bg_fill = Color32::from_gray(65);
    style.visuals.widgets.inactive.bg_fill = Color32::from_rgb(43, 44, 40);
    style.visuals.widgets.inactive.weak_bg_fill = Color32::from_rgb(43, 44, 40);
    style.visuals.widgets.hovered.bg_fill = Color32::from_gray(64);
    style.visuals.widgets.active.bg_fill = Color32::from_gray(75);
    style.spacing.item_spacing = Vec2::new(10.0, 10.0);
    style.spacing.button_padding = Vec2::new(12.0, 8.0);
    style
        .text_styles
        .insert(egui::TextStyle::Body, FontId::proportional(15.0));
    style
        .text_styles
        .insert(egui::TextStyle::Button, FontId::proportional(15.0));
    ctx.set_global_style(style);
}

fn heading(text: impl Into<String>, size: f32) -> RichText {
    RichText::new(text)
        .font(FontId::new(size, egui::FontFamily::Name("bold".into())))
        .color(WHITE)
}
fn merge_shelves(existing: &mut Vec<HomeShelf>, incoming: Vec<HomeShelf>) {
    for shelf in incoming {
        if shelf.items.is_empty() {
            continue;
        }
        let keys: Vec<_> = shelf
            .items
            .iter()
            .filter_map(|i| i.browse_id.as_deref())
            .collect();
        let duplicate = existing.iter().any(|old| {
            old.title.eq_ignore_ascii_case(&shelf.title)
                || (!keys.is_empty()
                    && old
                        .items
                        .iter()
                        .filter_map(|i| i.browse_id.as_deref())
                        .filter(|k| keys.contains(k))
                        .count()
                        * 10
                        >= old.items.len().min(keys.len()) * 6)
        });
        if !duplicate {
            existing.push(shelf);
        }
    }
}

/// Where to fetch the player's artwork, best first. Library and search thumbnails are usually
/// 60–120 px (`=w120-h120`), which went blurry stretched over the Now Playing artwork, so the
/// image hosts are asked for a large copy; the stored URL stays as the fallback.
fn artwork_candidates(url: &str) -> Vec<String> {
    let mut out = vec![];
    let video = url
        .strip_prefix("https://i.ytimg.com/vi/")
        .and_then(|rest| rest.split_once('/'))
        .map(|(id, _)| id);
    if let Some(id) = video {
        out.push(format!("https://i.ytimg.com/vi/{id}/maxresdefault.jpg"));
    } else {
        out.push(artwork_at(url, 1200));
    }
    if !out.iter().any(|u| u == url) {
        out.push(url.to_owned());
    }
    out
}

fn load_artwork(url: &str) -> anyhow::Result<(egui::ColorImage, egui::ColorImage)> {
    use std::io::Read;
    let client = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?;
    let mut last = None;
    let mut bytes = vec![];
    for candidate in artwork_candidates(url) {
        match client
            .get(&candidate)
            .send()
            .and_then(|r| r.error_for_status())
        {
            Ok(response) => {
                bytes.clear();
                response.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
                last = None;
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    if let Some(e) = last {
        return Err(e.into());
    }
    anyhow::ensure!(bytes.len() <= 8 * 1024 * 1024, "Artwork is too large");
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let im = reader.decode()?;
    let cover = im
        .resize(1200, 1200, image::imageops::FilterType::Lanczos3)
        .into_rgba8();
    let blurred = image::imageops::blur(
        &im.resize_exact(96, 96, image::imageops::FilterType::Triangle)
            .into_rgba8(),
        9.0,
    );
    Ok((
        egui::ColorImage::from_rgba_unmultiplied(
            [cover.width() as usize, cover.height() as usize],
            &cover,
        ),
        egui::ColorImage::from_rgba_unmultiplied([96, 96], &blurred),
    ))
}

#[cfg(test)]
mod artwork_tests {
    use super::artwork_at;

    #[test]
    fn player_artwork_is_fetched_large_with_the_stored_url_as_fallback() {
        assert_eq!(
            super::artwork_candidates("https://yt3.googleusercontent.com/abc=w120-h120-l90-rj"),
            [
                "https://yt3.googleusercontent.com/abc=w1200-h1200-l90-rj",
                "https://yt3.googleusercontent.com/abc=w120-h120-l90-rj"
            ]
        );
        assert_eq!(
            super::artwork_candidates("https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg?sqp=x"),
            [
                "https://i.ytimg.com/vi/dQw4w9WgXcQ/maxresdefault.jpg",
                "https://i.ytimg.com/vi/dQw4w9WgXcQ/hqdefault.jpg?sqp=x"
            ]
        );
    }

    #[test]
    fn artwork_is_requested_at_the_drawn_size() {
        assert_eq!(
            artwork_at(
                "https://lh3.googleusercontent.com/abc=w544-h544-l90-rj",
                120
            ),
            "https://lh3.googleusercontent.com/abc=w120-h120-l90-rj"
        );
        assert_eq!(
            artwork_at(
                "https://yt3.ggpht.com/ytc/AIdro_x123=s88-c-k-c0x00ffffff-no-rj",
                240
            ),
            "https://yt3.ggpht.com/ytc/AIdro_x123=s240-c-k-c0x00ffffff-no-rj"
        );
        // `=s<N>` only means size on Google image hosts; video thumbnails are left alone.
        assert_eq!(
            artwork_at("https://i.ytimg.com/vi/abc/hqdefault.jpg?sqp=s12", 120),
            "https://i.ytimg.com/vi/abc/hqdefault.jpg?sqp=s12"
        );
        assert_eq!(
            artwork_at("https://example.test/w720-h720.jpg", 544),
            "https://example.test/w544-h544.jpg"
        );
    }
}

#[cfg(test)]
mod font_baseline_tests {
    use super::*;

    const INTER: &[u8] = include_bytes!("../assets/fonts/Inter-Regular.ttf");

    #[test]
    fn identical_faces_need_no_shift() {
        assert!(baseline_shift_em(INTER, INTER).unwrap().abs() < 1e-6);
        assert_eq!(baseline_shift_em(INTER, b"not a font"), None);
    }

    #[test]
    fn cjk_glyphs_share_the_latin_baseline() {
        let fonts = font_definitions();
        let mut factors: Vec<(String, f32)> = fonts
            .font_data
            .iter()
            .filter(|(name, _)| name.starts_with("cjk-"))
            .map(|(name, data)| (name.clone(), data.tweak.y_offset_factor))
            .collect();
        factors.sort_by(|a, b| a.0.cmp(&b.0));
        let Some(&(_, factor)) = factors.first() else {
            eprintln!("no CJK system font found; skipping");
            return;
        };
        eprintln!("cjk y_offset_factor values: {factors:?}");
        let size = 24.0;
        let ctx = egui::Context::default();
        ctx.set_fonts(fonts);
        let mut baselines = Vec::new();
        let mut output = ctx.run_ui(egui::RawInput::default(), |ui| {
            let galley = ui.painter().layout_no_wrap(
                "夕凪 Evening".into(),
                FontId::proportional(size),
                Color32::WHITE,
            );
            for glyph in &galley.rows[0].row.glyphs {
                // The tweak is applied at paint time, so add it to the layout baseline.
                let shift = if glyph.chr.is_ascii() {
                    0.0
                } else {
                    factor * size
                };
                baselines.push((glyph.chr, glyph.pos.y + shift));
            }
        });
        output.textures_delta.clear();
        eprintln!("effective baselines: {baselines:?}");
        let latin = baselines.iter().find(|(c, _)| *c == 'E').unwrap().1;
        for (chr, y) in baselines.iter().filter(|(c, _)| !c.is_whitespace()) {
            assert!(
                (y - latin).abs() < 1.5,
                "{chr:?} baseline {y} vs Latin {latin}"
            );
        }
    }
}
