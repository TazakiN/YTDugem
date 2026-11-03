use crate::{
    auth::{self, Account, SavedAccount},
    discord::Presence,
    integrations::{Integrations, MediaEvent},
    model::{clock, Track},
    player::{Player, PlayerStatus, Stream},
    services::translate::{self, Translation},
    services::{self, Collection, HomeFeed, HomeShelf, LyricLine, ShelfItem},
    store::{self, Library, Playlist},
    widgets::{self, Icon},
};
use eframe::egui::{self, Align, Color32, FontId, Pos2, Rect, RichText, Stroke, Vec2};
use std::{
    collections::BTreeMap,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc,
    },
    time::{Duration, Instant},
};

const BG: Color32 = Color32::from_rgb(29, 31, 27);
const SIDE: Color32 = Color32::from_rgb(21, 22, 19);
const MUTED: Color32 = Color32::from_rgb(150, 151, 147);
const WHITE: Color32 = Color32::from_rgb(244, 244, 242);

#[derive(Clone, PartialEq)]
enum Page {
    Home,
    Explore,
    Library,
    Search,
    History,
    Downloads,
    Local,
    Liked,
    Playlist(usize),
    Collection,
    Settings,
}
#[derive(Clone, Copy, PartialEq)]
enum PlayerTab {
    Controls,
    Lyrics,
    Queue,
}
// ponytail: low-volume worker events own their payloads; no shared mutable UI state.
#[allow(clippy::large_enum_variant)]
enum Event {
    Playing(u64, Track, Result<Player, String>),
    Prepared(String, Option<Stream>),
    AccountPlaylists(u64, Result<Vec<ShelfItem>, String>),
    Translated(u64, Result<Translation, String>),
    PlaylistEdited(Result<PlaylistEdit, String>),
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
    Account(u64, Result<Account, String>),
    SavedAccounts(u64, Result<Vec<SavedAccount>, String>),
    AccountRemoved(u64, Result<(), String>),
    LyricsKeys(u64, Result<Vec<(String, bool)>, String>),
    LyricsKeySaved(u64, String, Result<bool, String>),
    AudioDevices(Result<Vec<(String, String)>, String>),
    BrowserOpened(u64, Result<(), String>),
    Artwork(String, Result<(egui::ColorImage, egui::ColorImage), String>),
    Radio(u64, String, Result<Vec<Track>, String>),
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
    collection: Option<Collection>,
    collection_request: u64,
    tx: Sender<Event>,
    rx: Receiver<Event>,
    message: String,
    importing: bool,
    downloads: Vec<DownloadItem>,
    expanded: bool,
    tab: PlayerTab,
    shuffle: bool,
    repeat: u8,
    lyrics: Vec<LyricLine>,
    lyrics_message: String,
    lyrics_provider: String,
    lyrics_override: Option<String>,
    lyrics_request: u64,
    follow_lyrics: bool,
    active_lyric: Option<usize>,
    cover: Option<egui::TextureHandle>,
    backdrop: Option<egui::TextureHandle>,
    import_path: String,
    playlist_name: String,
    download_path: String,
    sleep_until: Option<Instant>,
    stop_after_track: bool,
    account: Option<Account>,
    account_open: bool,
    adding_account: bool,
    cookie_input: String,
    account_busy: bool,
    account_request: u64,
    saved_accounts: Vec<SavedAccount>,
    saved_accounts_request: u64,
    radio_request: u64,
    lyric_keys: BTreeMap<String, bool>,
    lyric_key_inputs: BTreeMap<String, String>,
    lyric_keys_loaded: bool,
    lyric_keys_busy: bool,
    audio_devices: Vec<(String, String)>,
    audio_devices_loading: bool,
    audio_devices_loaded: bool,
    lyric_keys_request: u64,
    login_cancel: Arc<AtomicBool>,
    last_poll: Instant,
    now_origin: String,
    radio_pending: bool,
    discord: Presence,
    #[cfg(windows)]
    tray: Option<crate::tray::Tray>,
    #[cfg(windows)]
    hwnd: Option<isize>,
    /// Set by Quit (Ctrl+Q, the tray): a close that must not be turned into hide-to-tray.
    quitting: bool,
    /// `--screenshot FILE`: save the window after `SCREENSHOT_FRAMES` frames, then exit.
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
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
    /// One account-playlist change at a time.
    playlist_busy: bool,
    /// The lyrics translated for (track key, language), when asked for.
    translation: Option<(String, String, Translation)>,
    /// The Translate toggle; stays on across songs, as upstream.
    show_translation: bool,
    translating: bool,
    translation_request: u64,
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
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot(Default::default()));
        }
        let image = ctx.input(|i| {
            i.raw.events.iter().find_map(|e| match e {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
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
                    "Saved sign-in could not be restored. Open your account to sign in again."
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
            collection: None,
            collection_request: 0,
            tx,
            rx,
            message,
            importing: false,
            downloads: vec![],
            expanded: false,
            tab: PlayerTab::Controls,
            shuffle: false,
            repeat: 0,
            lyrics: vec![],
            lyrics_message: String::new(),
            lyrics_provider: String::new(),
            lyrics_override: None,
            lyrics_request: 0,
            follow_lyrics: true,
            active_lyric: None,
            cover: None,
            backdrop: None,
            import_path: String::new(),
            playlist_name: String::new(),
            download_path,
            sleep_until: None,
            stop_after_track: false,
            account,
            account_open: false,
            adding_account: false,
            cookie_input: String::new(),
            account_busy: false,
            account_request: 0,
            saved_accounts: vec![],
            saved_accounts_request: 0,
            radio_request: 0,
            lyric_keys: BTreeMap::new(),
            lyric_key_inputs: BTreeMap::new(),
            lyric_keys_loaded: false,
            lyric_keys_busy: false,
            audio_devices: vec![],
            audio_devices_loading: false,
            audio_devices_loaded: false,
            lyric_keys_request: 0,
            login_cancel: Arc::new(AtomicBool::new(false)),
            last_poll: Instant::now(),
            now_origin: "Home".into(),
            radio_pending: false,
            discord: Presence::new(),
            #[cfg(windows)]
            tray: crate::tray::Tray::new(&cc.egui_ctx),
            #[cfg(windows)]
            hwnd,
            quitting: false,
            screenshot: None,
            screenshot_requested: false,
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
            playlist_busy: false,
            translation: None,
            show_translation: false,
            translating: false,
            translation_request: 0,
        };
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
        self.translation_request += 1;
        self.translation = None;
        self.translating = false;
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
        self.adding_account = false;
        self.load_home();
        self.request_lyrics(self.lyrics_override.clone());
    }
    fn open_accounts(&mut self) {
        self.account_open = true;
        self.saved_accounts_request += 1;
        let request = self.saved_accounts_request;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::SavedAccounts(
                request,
                auth::saved_accounts().map_err(err),
            ));
        });
    }
    fn select_account(&mut self, id: String) {
        if self.account_busy {
            return;
        }
        self.account_busy = true;
        self.account_request += 1;
        let request = self.account_request;
        self.login_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.login_cancel.clone();
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Account(
                request,
                auth::select_account(&id, &cancel).map_err(err),
            ));
        });
    }
    fn remove_account(&mut self, id: String) {
        if self.account_busy {
            return;
        }
        self.account_busy = true;
        self.account_request += 1;
        let request = self.account_request;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::AccountRemoved(
                request,
                auth::remove_account(&id).map_err(err),
            ));
        });
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
                    // Tags carry title and artist; the artwork comes from where it was found.
                    for t in &mut tracks {
                        if t.thumbnail.is_empty() {
                            t.thumbnail = track.thumbnail.clone();
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
                if failed
                    && ui
                        .button("Clear")
                        .on_hover_text("Remove failed downloads")
                        .clicked()
                {
                    self.downloads
                        .retain(|d| !matches!(d.state, DownloadState::Failed(_)));
                }
                if !self.downloads.is_empty() && ui.button("Cancel all").clicked() {
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
            ui.painter()
                .rect_filled(rect, 8.0, Color32::from_white_alpha(6));
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
                ui.painter()
                    .rect_filled(bar, 1.5, Color32::from_white_alpha(25));
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
    /// Run one account-playlist change off the GUI thread; its outcome arrives as `PlaylistEdit`.
    fn edit_account_playlist(
        &mut self,
        job: impl FnOnce() -> anyhow::Result<PlaylistEdit> + Send + 'static,
    ) {
        if self.playlist_busy {
            self.message = "Another playlist change is still being saved".into();
            return;
        }
        self.playlist_busy = true;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::PlaylistEdited(job().map_err(err)));
        });
    }
    fn playlist_edited(&mut self, result: Result<PlaylistEdit, String>) {
        self.playlist_busy = false;
        let edit = match result {
            Ok(edit) => edit,
            Err(e) => {
                self.message = e;
                return;
            }
        };
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
        }
    }
    /// "Add to YouTube playlist" and, inside an owned playlist, "Remove from this playlist".
    fn account_playlist_menu(&mut self, ui: &mut egui::Ui, track: &Track) {
        if self.account.is_none() || track.id.is_empty() || !track.source.starts_with("https://") {
            return;
        }
        if self.account_playlists.is_none() && !self.account_playlists_loading {
            self.load_account_playlists();
        }
        ui.menu_button("Add to YouTube playlist", |ui| {
            if ui.button("New playlist…").clicked() {
                self.playlist_dialog = Some(PlaylistDialog::Create {
                    name: String::new(),
                    privacy: "PRIVATE".into(),
                    seed: Some(track.clone()),
                });
                ui.close_menu();
            }
            ui.separator();
            match self.account_playlists.clone() {
                None => {
                    ui.spinner();
                }
                Some(list) if list.is_empty() => {
                    ui.label(RichText::new("No playlists yet").color(MUTED));
                }
                Some(list) => {
                    for p in list {
                        let Some(id) = p.browse_id.as_deref().and_then(|b| b.strip_prefix("VL"))
                        else {
                            continue;
                        };
                        if ui.button(truncate(&p.title, 40)).clicked() {
                            let (id, t) = (id.to_owned(), track.clone());
                            self.edit_account_playlist(move || {
                                let set_id = services::add_to_playlist(&id, &t.id)?;
                                Ok(PlaylistEdit::Added {
                                    id,
                                    track: t,
                                    set_id,
                                })
                            });
                            ui.close_menu();
                        }
                    }
                }
            }
        });
        let owned = self
            .collection
            .as_ref()
            .filter(|c| self.page == Page::Collection && c.editable)
            .and_then(|c| Some((c.playlist_id.clone()?, c.set_ids.get(&track.id)?.clone())));
        if let Some((id, set_id)) = owned {
            if ui.button("Remove from this playlist").clicked() {
                let video_id = track.id.clone();
                self.edit_account_playlist(move || {
                    services::remove_from_playlist(&id, &video_id, &set_id)?;
                    Ok(PlaylistEdit::Removed { id, video_id })
                });
                ui.close_menu();
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
            if ui
                .add_enabled(!self.playlist_busy, egui::Button::new("New playlist"))
                .clicked()
            {
                self.playlist_dialog = Some(PlaylistDialog::Create {
                    name: String::new(),
                    privacy: "PRIVATE".into(),
                    seed: None,
                });
            }
            if ui
                .add_enabled(
                    !self.account_playlists_loading,
                    egui::Button::new("Refresh"),
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
    /// Create / rename / confirm-delete for account playlists.
    fn playlist_dialog(&mut self, ctx: &egui::Context) {
        let Some(mut dialog) = self.playlist_dialog.take() else {
            return;
        };
        let title = match &dialog {
            PlaylistDialog::Create { .. } => "New playlist",
            PlaylistDialog::Rename { .. } => "Rename playlist",
            PlaylistDialog::Delete { .. } => "Delete playlist",
        };
        let mut keep = true;
        let mut submit = false;
        egui::Window::new(title)
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, Vec2::ZERO)
            .show(ctx, |ui| {
                ui.set_width(340.0);
                match &mut dialog {
                    PlaylistDialog::Create {
                        name,
                        privacy,
                        seed,
                    } => {
                        let r = ui.add(
                            egui::TextEdit::singleline(name)
                                .hint_text("Playlist name")
                                .desired_width(f32::INFINITY),
                        );
                        r.request_focus();
                        submit |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                        ui.horizontal(|ui| {
                            ui.label("Privacy");
                            for (value, label) in services::PLAYLIST_PRIVACY {
                                ui.radio_value(privacy, value.to_owned(), label);
                            }
                        });
                        if let Some(t) = seed {
                            ui.label(
                                RichText::new(format!("Starts with “{}”", t.title)).color(MUTED),
                            );
                        }
                    }
                    PlaylistDialog::Rename { name, .. } => {
                        let r = ui.add(
                            egui::TextEdit::singleline(name).desired_width(f32::INFINITY),
                        );
                        r.request_focus();
                        submit |= r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
                    }
                    PlaylistDialog::Delete { title, .. } => {
                        ui.label(format!(
                            "Delete “{title}” from your YouTube Music account? This cannot be undone."
                        ));
                    }
                }
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    let label = match dialog {
                        PlaylistDialog::Create { .. } => "Create",
                        PlaylistDialog::Rename { .. } => "Rename",
                        PlaylistDialog::Delete { .. } => "Delete",
                    };
                    submit |= ui.button(label).clicked();
                    if ui.button("Cancel").clicked() {
                        keep = false;
                    }
                });
            });
        if ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
            keep = false;
        }
        if submit {
            match dialog {
                PlaylistDialog::Create {
                    name,
                    privacy,
                    seed,
                } => {
                    if name.trim().is_empty() {
                        self.message = "A playlist needs a name".into();
                        self.playlist_dialog = Some(PlaylistDialog::Create {
                            name,
                            privacy,
                            seed,
                        });
                        return;
                    }
                    self.edit_account_playlist(move || {
                        let seeds: Vec<String> = seed.iter().map(|t| t.id.clone()).collect();
                        services::create_playlist(&name, &privacy, &seeds)?;
                        Ok(PlaylistEdit::Created {
                            title: name.trim().to_owned(),
                            seeded: !seeds.is_empty(),
                        })
                    });
                }
                PlaylistDialog::Rename { id, name } => self.edit_account_playlist(move || {
                    services::rename_playlist(&id, &name)?;
                    Ok(PlaylistEdit::Renamed {
                        id,
                        name: name.trim().to_owned(),
                    })
                }),
                PlaylistDialog::Delete { id, .. } => self.edit_account_playlist(move || {
                    services::delete_playlist(&id)?;
                    Ok(PlaylistEdit::Deleted { id })
                }),
            }
            return;
        }
        if keep {
            self.playlist_dialog = Some(dialog);
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
    fn translation_target(&self) -> String {
        if self.library.translation_language.is_empty() {
            translate::system_language()
        } else {
            self.library.translation_language.clone()
        }
    }
    /// Translate the lyrics on screen into the chosen language, unless already done for them.
    fn request_translation(&mut self) {
        let Some(key) = self.current.as_ref().map(|t| t.key().to_owned()) else {
            return;
        };
        let target = self.translation_target();
        let done = self
            .translation
            .as_ref()
            .is_some_and(|(k, lang, _)| *k == key && *lang == target);
        if done || self.lyrics.is_empty() || self.translating {
            return;
        }
        self.translation_request += 1;
        self.translating = true;
        let (id, lines, tx) = (
            self.translation_request,
            self.lyrics.clone(),
            self.tx.clone(),
        );
        std::thread::spawn(move || {
            let _ = tx.send(Event::Translated(
                id,
                translate::translate(&lines, &target).map_err(err),
            ));
        });
    }
    /// What the lyrics panel shows: the translation when it is on and ready, else the original.
    fn display_lyrics(&self) -> Vec<LyricLine> {
        let key = self.current.as_ref().map(Track::key);
        match &self.translation {
            Some((k, lang, Translation::Translated { lines, .. }))
                if self.show_translation
                    && Some(k.as_str()) == key
                    && *lang == self.translation_target() =>
            {
                lines.clone()
            }
            _ => self.lyrics.clone(),
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
    fn open_default_login(&mut self) {
        if self.account_busy {
            return;
        }
        self.account_busy = true;
        self.account_request += 1;
        self.message = "Opening your default browser…".into();
        let id = self.account_request;
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::BrowserOpened(
                id,
                auth::open_default_login().map_err(err),
            ));
        });
    }
    fn login(&mut self, browser: bool) {
        if self.account_busy {
            return;
        }
        self.account_busy = true;
        self.account_request += 1;
        let id = self.account_request;
        self.login_cancel = Arc::new(AtomicBool::new(false));
        let cancel = self.login_cancel.clone();
        let input = std::mem::take(&mut self.cookie_input);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let r = if browser {
                auth::browser_login(cancel)
            } else {
                auth::connect(&input, &cancel)
            };
            let _ = tx.send(Event::Account(id, r.map_err(err)));
        });
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
                Event::PlaylistEdited(r) => self.playlist_edited(r),
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
                            for t in ts {
                                if !self.library.local.iter().any(|s| s.key() == t.key()) {
                                    self.library.local.push(t);
                                }
                            }
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
                            for t in tracks {
                                if !self.library.local.iter().any(|s| s.key() == t.key()) {
                                    self.library.local.push(t);
                                }
                            }
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
                            if self.show_translation {
                                self.request_translation();
                            }
                        }
                        Err(e) => self.lyrics_message = e,
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
                Event::BrowserOpened(id, r) if id == self.account_request => {
                    self.account_busy = false;
                    self.message = match r {
                        Ok(()) => "The default browser only opened YouTube Music. Use Automatic sign-in for secure capture, or paste cookies below.".into(),
                        Err(e) => e,
                    };
                }
                Event::Account(id, r)
                    if accepts_account_result(id, self.account_request, r.is_ok()) =>
                {
                    if id == self.account_request {
                        self.account_busy = false;
                    }
                    match r {
                        Ok(_) => {
                            self.sync_account();
                            if id == self.account_request {
                                self.account_open = false;
                                self.message = "Signed in to YouTube Music".into();
                            }
                        }
                        Err(e) => self.message = e,
                    }
                }
                Event::SavedAccounts(id, r) if id == self.saved_accounts_request => match r {
                    Ok(accounts) => self.saved_accounts = accounts,
                    Err(e) => self.message = e,
                },
                Event::AccountRemoved(id, r)
                    if accepts_account_result(id, self.account_request, r.is_ok()) =>
                {
                    if id == self.account_request {
                        self.account_busy = false;
                    }
                    match r {
                        Ok(()) => {
                            self.sync_account();
                            if id == self.account_request {
                                self.open_accounts();
                                self.message = "Saved account removed".into();
                            }
                        }
                        Err(e) => self.message = e,
                    }
                }
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
        if self.library.discord_enabled {
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

fn accepts_account_result(request: u64, current: u64, succeeded: bool) -> bool {
    // A completed credential change survives dialog cancellation; read the current backend state.
    request == current || succeeded
}

#[cfg(test)]
mod account_tests {
    use super::accepts_account_result;

    #[test]
    fn committed_account_changes_are_reconciled_after_dialog_cancellation() {
        assert!(accepts_account_result(1, 2, true));
        assert!(!accepts_account_result(1, 2, false));
        assert!(accepts_account_result(2, 2, false));
    }
}

impl YTDugem {
    fn sidebar(&mut self, ui: &mut egui::Ui, rect: Rect) {
        ui.painter().rect_filled(rect, 0.0, SIDE);
        ui.painter().line_segment(
            [rect.right_top(), rect.right_bottom()],
            Stroke::new(1.0_f32, Color32::from_gray(43)),
        );
        for (i, color) in [
            Color32::from_rgb(255, 91, 84),
            Color32::from_rgb(255, 190, 51),
            Color32::from_rgb(40, 201, 65),
        ]
        .into_iter()
        .enumerate()
        {
            let r = Rect::from_center_size(
                Pos2::new(rect.left() + 25.0 + i as f32 * 24.0, rect.top() + 20.0),
                Vec2::splat(16.0),
            );
            let resp = ui.interact(
                r,
                ui.make_persistent_id(("window-dot", i)),
                egui::Sense::click(),
            );
            ui.painter().circle_filled(r.center(), 6.5, color);
            if resp.clicked() {
                match i {
                    0 => ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close),
                    1 => ui
                        .ctx()
                        .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                    _ => {
                        let max = ui.ctx().input(|s| s.viewport().maximized.unwrap_or(false));
                        ui.ctx()
                            .send_viewport_cmd(egui::ViewportCommand::Maximized(!max));
                    }
                }
            }
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
                                    .id(egui::Id::new("search_input"))
                                    .desired_width(rect.width() - 78.0)
                                    .hint_text("Search")
                                    .frame(false),
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
            (Page::Local, Icon::Library, "Local Music"),
            (Page::Liked, Icon::Queue, "Liked Music"),
        ];
        let mut y = rect.top() + 108.0;
        for (i, (page, icon, label)) in rows.into_iter().enumerate() {
            if i == 4 || i == 7 {
                y += 12.0;
                ui.painter().line_segment(
                    [
                        Pos2::new(rect.left() + 14.0, y),
                        Pos2::new(rect.right() - 14.0, y),
                    ],
                    Stroke::new(1.0_f32, Color32::from_gray(42)),
                );
                y += 12.0;
            }
            let r = Rect::from_min_size(
                Pos2::new(rect.left() + 14.0, y),
                Vec2::new(rect.width() - 28.0, 38.0),
            );
            if nav_row(ui, r, ("nav", i), icon, label, self.page == page).clicked() {
                self.navigate(page);
                if self.page == Page::Search {
                    ui.ctx()
                        .memory_mut(|m| m.request_focus(egui::Id::new("search_input")));
                }
            }
            y += 40.0;
        }
        let bottom = rect.bottom() - 67.0;
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
                        .show(ui, |ui| {
                            for (i, p) in self.library.playlists.iter().enumerate() {
                                let r = Rect::from_min_size(
                                    ui.cursor().min,
                                    Vec2::new(ui.available_width(), 35.0),
                                );
                                ui.allocate_space(r.size());
                                if nav_row(
                                    ui,
                                    r,
                                    ("playlist", i),
                                    Icon::Queue,
                                    &p.name,
                                    self.page == Page::Playlist(i),
                                )
                                .clicked()
                                {
                                    self.nav.push(self.page.clone());
                                    self.page = Page::Playlist(i);
                                    self.expanded = false;
                                }
                            }
                            if ui.small_button("+ New playlist").clicked() {
                                self.playlist_name = "New playlist".into();
                                self.library.playlists.push(Playlist {
                                    name: self.playlist_name.clone(),
                                    tracks: vec![],
                                });
                                self.navigate(Page::Playlist(self.library.playlists.len() - 1));
                                self.save();
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
            Rect::from_min_max(rect.min, Pos2::new(rect.right() - 106.0, rect.top() + 20.0)),
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
                    if widgets::button(ui, Icon::Logo, 42.0, false, "Home").clicked() {
                        self.navigate(Page::Home);
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
            self.tab = PlayerTab::Controls;
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
                        self.expanded && self.tab == PlayerTab::Lyrics,
                        "Lyrics",
                    )
                    .clicked()
                    {
                        self.expanded = true;
                        self.tab = PlayerTab::Lyrics;
                    }
                    if widgets::button(ui, Icon::Queue, 30.0, false, "Queue").clicked() {
                        self.expanded = true;
                        self.tab = PlayerTab::Queue;
                    }
                    if widgets::button(
                        ui,
                        Icon::User,
                        32.0,
                        self.account.is_some(),
                        "YouTube Music account",
                    )
                    .clicked()
                    {
                        self.open_accounts();
                    }
                });
            },
        );
        region(
            ui,
            Rect::from_min_size(
                Pos2::new(rect.right() - 103.0, rect.top()),
                Vec2::new(101.0, 23.0),
            ),
            |ui| {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    for icon in [Icon::Minimize, Icon::Maximize, Icon::Close] {
                        if widgets::button(ui, icon, 25.0, false, "Window control").clicked() {
                            match icon {
                                Icon::Close => {
                                    ui.ctx().send_viewport_cmd(egui::ViewportCommand::Close)
                                }
                                Icon::Minimize => ui
                                    .ctx()
                                    .send_viewport_cmd(egui::ViewportCommand::Minimized(true)),
                                _ => {
                                    let m =
                                        ui.ctx().input(|s| s.viewport().maximized.unwrap_or(false));
                                    ui.ctx()
                                        .send_viewport_cmd(egui::ViewportCommand::Maximized(!m));
                                }
                            }
                        }
                    }
                });
            },
        );
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
            ui.close_menu();
        }
        if ui.button("Add to queue").clicked() {
            self.library.queue.push(track.clone());
            self.save();
            ui.close_menu();
        }
        let liked = self.library.liked.iter().any(|t| t.key() == track.key());
        if ui
            .button(if liked {
                "Remove from Liked Music"
            } else {
                "Add to Liked Music"
            })
            .clicked()
        {
            self.library.like(track);
            self.save();
            ui.close_menu();
        }
        self.account_playlist_menu(ui, track);
        if ui.button("Download audio").clicked() {
            self.download(track.clone());
            ui.close_menu();
        }
        let mut changed = false;
        for playlist in &mut self.library.playlists {
            if ui.button(format!("Add to {}", playlist.name)).clicked() {
                if !playlist.tracks.iter().any(|t| t.key() == track.key()) {
                    playlist.tracks.push(track.clone());
                    changed = true;
                }
                ui.close_menu();
            }
        }
        if let Page::Playlist(i) = self.page {
            if ui.button("Remove from playlist").clicked() {
                self.library.playlists[i]
                    .tracks
                    .retain(|t| t.key() != track.key());
                changed = true;
                ui.close_menu();
            }
        }
        if changed {
            self.save();
        }
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
            ui.painter().rect_filled(
                rect,
                5.0,
                Color32::from_white_alpha(if active { 15 } else { 8 }),
            );
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
        ui.horizontal(|ui| {
            if ui.button("Play all").clicked() {
                self.start_tracks(tracks.clone(), 0, origin);
            }
            if ui.button("Add all to queue").clicked() {
                self.library.queue.extend(tracks.clone());
                self.save();
            }
            ui.label(RichText::new(format!("{} songs", tracks.len())).color(MUTED));
        });
        ui.add_space(12.0);
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
        for (shelf_i, shelf) in shelves.into_iter().enumerate() {
            ui.label(heading(&shelf.title, 24.0));
            if !shelf.subtitle.is_empty() {
                ui.label(RichText::new(&shelf.subtitle).color(MUTED));
            }
            ui.add_space(10.0);
            egui::ScrollArea::horizontal()
                .id_salt(("shelf", shelf_i))
                .show(ui, |ui| {
                    ui.horizontal_top(|ui| {
                        for (i, item) in shelf.items.iter().enumerate() {
                            ui.push_id(("card", shelf_i, i), |ui| {
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
    fn collection_view(&mut self, ui: &mut egui::Ui) {
        if let Some(c) = self.collection.clone() {
            ui.horizontal_top(|ui| {
                let (r, _) = ui.allocate_exact_size(Vec2::splat(155.0), egui::Sense::hover());
                draw_image(ui, &c.thumbnail, r, None);
                ui.vertical(|ui| {
                    ui.label(heading(&c.title, 31.0));
                    ui.label(RichText::new(&c.subtitle).color(MUTED));
                    if let Some(id) = c.playlist_id.clone().filter(|_| c.editable) {
                        ui.add_space(10.0);
                        ui.horizontal(|ui| {
                            if ui.button("Rename").clicked() {
                                self.playlist_dialog = Some(PlaylistDialog::Rename {
                                    id: id.clone(),
                                    name: c.title.clone(),
                                });
                            }
                            if ui.button("Delete playlist").clicked() {
                                self.playlist_dialog = Some(PlaylistDialog::Delete {
                                    id: id.clone(),
                                    title: c.title.clone(),
                                });
                            }
                            if self.playlist_busy {
                                ui.spinner();
                            }
                        });
                    }
                });
            });
            ui.add_space(20.0);
            self.track_list(ui, c.tracks, &c.title);
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
            Page::Home | Page::Explore => self.home_view(ui),
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
                ui.label(heading("Liked Music", 20.0));
                self.track_list(ui, self.library.liked.clone(), "Liked Music");
            }
            Page::History => self.track_list(ui, self.library.history.clone(), "History"),
            Page::Downloads => {
                self.download_manager(ui);
                let tracks: Vec<_> = self
                    .library
                    .local
                    .iter()
                    .filter(|t| Path::new(&t.source).starts_with(&self.library.download_dir))
                    .cloned()
                    .collect();
                self.track_list(ui, tracks, "Downloads");
            }
            Page::Local => {
                ui.label(heading("Local music", 26.0));
                ui.horizontal(|ui| {
                    let r = ui.add(
                        egui::TextEdit::singleline(&mut self.import_path)
                            .desired_width(500.0)
                            .hint_text("Audio file or folder path")
                            .id(egui::Id::new("local-import")),
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
            Page::Liked => self.track_list(ui, self.library.liked.clone(), "Liked Music"),
            Page::Playlist(i) => {
                if let Some(p) = self.library.playlists.get(i).cloned() {
                    ui.label(heading(&p.name, 26.0));
                    self.track_list(ui, p.tracks, "Playlist");
                }
            }
            Page::Settings => {
                self.load_lyrics_keys();
                self.load_audio_devices();
                ui.label(heading("Settings", 26.0));
                ui.add_space(16.0);
                if ui
                    .checkbox(&mut self.library.autoplay, "Autoplay similar music")
                    .changed()
                {
                    self.save();
                }
                if ui
                    .checkbox(
                        &mut self.library.discord_enabled,
                        "Enable Discord Rich Presence",
                    )
                    .changed()
                {
                    self.save();
                    if !self.library.discord_enabled {
                        self.discord.clear();
                    }
                }
                if self.library.discord_enabled {
                    ui.label(match self.discord.status() {
                        crate::discord::Status::Idle => {
                            "Discord: no active playback (Windows desktop only)".to_owned()
                        }
                        crate::discord::Status::Waiting => {
                            "Discord: waiting for acknowledgement".to_owned()
                        }
                        crate::discord::Status::Connected => "Discord: connected".to_owned(),
                        crate::discord::Status::Error(ref e) => format!("Discord: {e}"),
                    });
                    ui.horizontal(|ui| {
                        ui.label("Activity name");
                        if ui
                            .text_edit_singleline(&mut self.library.discord_activity_name)
                            .changed()
                        {
                            self.save();
                        }
                    });
                    for i in 0..2 {
                        if ui
                            .checkbox(
                                &mut self.library.discord_buttons[i],
                                if i == 0 {
                                    "Show YouTube Music button"
                                } else {
                                    "Show BitChord button"
                                },
                            )
                            .changed()
                        {
                            self.save();
                        }
                    }
                }
                if ui
                    .checkbox(&mut self.library.lyrics_enabled, "Enable lyrics lookup")
                    .changed()
                {
                    self.save();
                    self.lyrics_settings_changed(true);
                }
                ui.label(heading("Lyrics providers", 18.0));
                let ordered = services::lyrics::ordered(&self.library.lyrics_order);
                for (index, provider) in ordered.iter().enumerate() {
                    let mut enabled = self
                        .library
                        .lyrics_sources
                        .iter()
                        .any(|id| id == provider.id);
                    ui.horizontal(|ui| {
                        if ui.checkbox(&mut enabled, provider.label).changed() {
                            if enabled
                                && !self
                                    .library
                                    .lyrics_sources
                                    .iter()
                                    .any(|id| id == provider.id)
                            {
                                self.library.lyrics_sources.push(provider.id.into());
                            } else {
                                self.library.lyrics_sources.retain(|id| id != provider.id);
                            }
                            self.save();
                            self.lyrics_settings_changed(true);
                        }
                        ui.label(RichText::new(provider.detail).color(MUTED));
                        if ui.small_button("↑").clicked() && index > 0 {
                            let mut ids: Vec<_> = ordered.iter().map(|p| p.id.to_owned()).collect();
                            ids.swap(index, index - 1);
                            self.library.lyrics_order = ids;
                            self.save();
                            self.lyrics_settings_changed(true);
                        }
                        if ui.small_button("↓").clicked() && index + 1 < ordered.len() {
                            let mut ids: Vec<_> = ordered.iter().map(|p| p.id.to_owned()).collect();
                            ids.swap(index, index + 1);
                            self.library.lyrics_order = ids;
                            self.save();
                            self.lyrics_settings_changed(true);
                        }
                    });
                }
                let key_ids: Vec<String> = services::lyrics::PROVIDERS
                    .iter()
                    .filter_map(|provider| provider.key_id)
                    .map(str::to_owned)
                    .collect::<std::collections::BTreeSet<_>>()
                    .into_iter()
                    .collect();
                for key in key_ids {
                    ui.horizontal(|ui| {
                        ui.label(format!("{key} API key"));
                        let input = self.lyric_key_inputs.entry(key.clone()).or_default();
                        ui.add(
                            egui::TextEdit::singleline(input)
                                .password(true)
                                .hint_text("Paste key to replace"),
                        );
                        let configured = self.lyric_keys.get(&key).copied().unwrap_or(false);
                        ui.label(
                            RichText::new(if configured { "Configured" } else { "Not set" })
                                .color(MUTED),
                        );
                        let label = if input.trim().is_empty() && configured {
                            "Remove"
                        } else {
                            "Save"
                        };
                        if ui
                            .add_enabled(!self.lyric_keys_busy, egui::Button::new(label))
                            .clicked()
                        {
                            self.save_lyrics_key(key.clone());
                        }
                    });
                }
                if ui.button("Reset lyric providers").clicked() {
                    self.library.lyrics_order = store::lyrics_order_default();
                    self.library.lyrics_sources = store::lyrics_order_default();
                    self.library.lyrics_enabled = true;
                    self.save();
                    self.lyrics_settings_changed(true);
                }
                ui.separator();
                ui.label(heading("Audio output", 18.0));
                let selected = self.library.audio.output_device.clone();
                egui::ComboBox::from_id_salt("audio-output-device")
                    .selected_text(if selected.is_empty() {
                        "System default"
                    } else {
                        self.audio_devices
                            .iter()
                            .find(|(id, _)| id == &selected)
                            .map_or("Unavailable device", |(_, name)| name.as_str())
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
                            if ui.selectable_label(selected == id, name.clone()).clicked() {
                                self.library.audio.output_device = id;
                                self.save();
                            }
                        }
                    });
                if self.audio_devices_loading {
                    ui.spinner();
                    ui.label(RichText::new("Reading devices from mpv…").color(MUTED));
                } else if ui.button("Refresh audio devices").clicked() {
                    self.audio_devices_loaded = false;
                    self.load_audio_devices();
                }
                ui.label(
                    RichText::new("Output changes apply when the next track starts.").color(MUTED),
                );
                #[cfg(windows)]
                if ui
                    .checkbox(
                        &mut self.library.audio.wasapi_exclusive,
                        "Prefer WASAPI exclusive output",
                    )
                    .changed()
                {
                    self.save();
                    self.message =
                        "WASAPI exclusive preference applies when the next track starts.".into();
                }
                ui.separator();
                ui.horizontal(|ui| {
                    if ui
                        .checkbox(
                            &mut self.library.audio.equalizer_enabled,
                            "Enable equalizer",
                        )
                        .changed()
                    {
                        self.library.audio.sanitize();
                        self.save();
                        self.apply_audio_settings();
                    }
                    if ui.button("Reset EQ").clicked() {
                        self.library.audio.equalizer_enabled = false;
                        self.library.audio.equalizer_bands = vec![0.0; 7];
                        self.library.audio.stereo_balance = 0.0;
                        self.save();
                        self.apply_audio_settings();
                    }
                });
                let current_preset = store::audio_presets()
                    .iter()
                    .find(|(_, bands)| self.library.audio.equalizer_bands.as_slice() == bands)
                    .map_or("Custom", |(name, _)| *name);
                egui::ComboBox::from_id_salt("equalizer-preset")
                    .selected_text(current_preset)
                    .show_ui(ui, |ui| {
                        for (name, bands) in store::audio_presets() {
                            if ui
                                .selectable_label(current_preset == *name, *name)
                                .clicked()
                            {
                                self.library.audio.equalizer_bands = bands.to_vec();
                                self.library.audio.equalizer_enabled = true;
                                self.save();
                                self.apply_audio_settings();
                            }
                        }
                    });
                for (index, frequency) in [
                    "60 Hz", "150 Hz", "400 Hz", "1 kHz", "2.5 kHz", "6 kHz", "14 kHz",
                ]
                .iter()
                .enumerate()
                {
                    let mut gain = self.library.audio.equalizer_bands[index];
                    ui.horizontal(|ui| {
                        ui.label(*frequency);
                        if ui
                            .add(egui::Slider::new(&mut gain, -12.0..=12.0).suffix(" dB"))
                            .changed()
                        {
                            self.library.audio.equalizer_bands[index] = gain;
                            self.library.audio.equalizer_enabled = true;
                            self.save();
                            self.apply_audio_settings();
                        }
                    });
                }
                if ui
                    .add(
                        egui::Slider::new(&mut self.library.audio.stereo_balance, -1.0..=1.0)
                            .text("Stereo balance"),
                    )
                    .changed()
                {
                    self.library.audio.sanitize();
                    self.save();
                    self.apply_audio_settings();
                }
                ui.label(RichText::new("Bands and balance update the active mpv filter chain. Output device and WASAPI mode apply to the next track.").color(MUTED));
                ui.label(
                    RichText::new(format!(
                        "System media controls: {}",
                        if self.integrations.available() {
                            "available"
                        } else {
                            "unavailable"
                        }
                    ))
                    .color(MUTED),
                );
                ui.horizontal(|ui| {
                    ui.label("Playback speed");
                    if ui
                        .add(egui::Slider::new(&mut self.library.speed, 0.5..=2.0).step_by(0.05))
                        .changed()
                    {
                        let _ = self.player.speed(self.library.speed);
                        self.save();
                    }
                });
                #[cfg(windows)]
                if self.tray.is_some()
                    && ui
                        .checkbox(
                            &mut self.library.close_to_tray,
                            "Keep playing in the tray when the window is closed",
                        )
                        .changed()
                {
                    self.save();
                }
                ui.horizontal(|ui| {
                    ui.label("Crossfade");
                    let seconds = &mut self.library.crossfade_seconds;
                    let off = *seconds == 0;
                    let slider = egui::Slider::new(seconds, 0..=12)
                        .show_value(false)
                        .step_by(1.0);
                    if ui.add(slider).changed() {
                        self.save();
                    }
                    ui.label(if off {
                        "Off".to_owned()
                    } else {
                        format!("{}s", self.library.crossfade_seconds)
                    });
                });
                ui.label(
                    RichText::new(
                        "Blends the end of a song into the next one. The next song is prepared in the background while this one plays.",
                    )
                    .color(MUTED),
                );
                ui.horizontal(|ui| {
                    ui.label("Lyrics translation");
                    let system = translate::system_language();
                    let name = |tag: &str| {
                        translate::LANGUAGES
                            .iter()
                            .find(|(t, _)| *t == tag)
                            .map_or(tag.to_owned(), |(_, n)| (*n).to_owned())
                    };
                    let current = &self.library.translation_language;
                    let mut chosen = current.clone();
                    egui::ComboBox::from_id_salt("translation-language")
                        .selected_text(if current.is_empty() {
                            format!("{} · follows Windows", name(&system))
                        } else {
                            name(current)
                        })
                        .show_ui(ui, |ui| {
                            ui.selectable_value(
                                &mut chosen,
                                String::new(),
                                format!("{} · follows Windows", name(&system)),
                            );
                            for (tag, label) in translate::LANGUAGES {
                                ui.selectable_value(&mut chosen, tag.to_owned(), label);
                            }
                        });
                    if chosen != self.library.translation_language {
                        self.library.translation_language = chosen;
                        self.save();
                        if self.show_translation {
                            self.request_translation();
                        }
                    }
                });
                ui.horizontal(|ui| {
                    ui.label("Download format");
                    let current = services::DOWNLOAD_FORMATS
                        .iter()
                        .find(|(id, _)| *id == self.library.download_format)
                        .unwrap_or(&services::DOWNLOAD_FORMATS[0]);
                    let mut chosen = current.0.to_owned();
                    egui::ComboBox::from_id_salt("download-format")
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
                });
                ui.horizontal(|ui| {
                    ui.label("Download folder");
                    ui.text_edit_singleline(&mut self.download_path);
                    if ui.button("Save").clicked() {
                        self.library.download_dir = PathBuf::from(&self.download_path);
                        self.save();
                    }
                });
                if !self.message.is_empty() {
                    ui.label(RichText::new(&self.message).color(MUTED));
                }
            }
        }
    }
    fn account_dialog(&mut self, ctx: &egui::Context) {
        if !self.account_open {
            return;
        }
        let mut open = true;
        let mut cancel = false;
        egui::Window::new("YouTube Music account")
            .open(&mut open)
            .collapsible(false)
            .resizable(false)
            .show(ctx, |ui| {
                ui.label(heading("Saved accounts", 16.0));
                for saved in self.saved_accounts.clone() {
                    ui.horizontal(|ui| {
                        let label = format!("{}{}", saved.account.name, if saved.active { " · Active" } else { "" });
                        if ui.add_enabled(!saved.active && !self.account_busy, egui::Button::new(label)).clicked() {
                            self.select_account(saved.id.clone());
                        }
                        if !saved.account.email.is_empty() {
                            ui.label(RichText::new(&saved.account.email).color(MUTED));
                        }
                        if ui.add_enabled(!self.account_busy, egui::Button::new("Remove")).clicked() {
                            self.remove_account(saved.id);
                        }
                    });
                }
                if let Some(a) = self.account.clone() {
                    ui.label(heading(&a.name, 20.0));
                    if !a.email.is_empty() {
                        ui.label(RichText::new(&a.email).color(MUTED));
                    }
                    if !self.adding_account && ui.button("Add another account").clicked() {
                        self.adding_account = true;
                    }
                    if ui.button("Sign out").clicked() {
                        self.login_cancel.store(true, Ordering::SeqCst);
                        self.account_request += 1;
                        match auth::logout() {
                            Ok(()) => {
                                self.account = None;
                                self.adding_account = false;
                                self.invalidate_account_results();
                                self.load_home();
                                self.message = "Signed out".into();
                                self.open_accounts();
                            }
                            Err(e) => self.message = format!("Sign out failed: {e:#}"),
                        }
                    }
                }
                if self.account.is_none() || self.adding_account {
                    if self.account.is_some() && ui.button("Stop adding account").clicked() {
                        self.adding_account = false;
                    }
                    ui.label("Sign in to access your YouTube Music library and history.");
                    if ui
                        .add_enabled(!self.account_busy, egui::Button::new("Open default browser"))
                        .clicked()
                    {
                        self.open_default_login();
                    }
                    ui.label("Automatic sign-in opens an isolated browser window. Finish signing in there, then CLOSE that window to finish secure cookie capture. The default-browser option only opens the site.");
                    if ui
                        .add_enabled(!self.account_busy, egui::Button::new("Automatic sign-in (Chrome/Edge/Chromium)"))
                        .clicked()
                    {
                        self.login(true);
                    }
                    ui.separator();
                    ui.label("Or paste a Cookie header, Netscape cookies, or cookie JSON:");
                    egui::ScrollArea::vertical()
                        .max_height(120.0)
                        .id_salt("cookie-input-scroll")
                        .show(ui, |ui| {
                            ui.add(
                                egui::TextEdit::multiline(&mut self.cookie_input)
                                    .password(true)
                                    .desired_rows(4)
                                    .desired_width(420.0),
                            );
                        });
                    if ui.button("Cancel").clicked() {
                        self.login_cancel.store(true, Ordering::SeqCst);
                        self.account_request += 1;
                        self.account_busy = false;
                        cancel = true;
                    }
                    if ui
                        .add_enabled(
                            !self.account_busy && !self.cookie_input.trim().is_empty(),
                            egui::Button::new("Connect with cookies"),
                        )
                        .clicked()
                    {
                        self.login(false);
                    }
                }
                if self.account_busy {
                    ui.spinner();
                    ui.label("Connecting…");
                }
                if !self.message.is_empty() {
                    ui.label(RichText::new(&self.message).color(MUTED));
                }
            });
        if !open || cancel {
            self.login_cancel.store(true, Ordering::SeqCst);
            self.account_request += 1;
            self.account_busy = false;
            self.cookie_input.clear();
        }
        self.account_open = open && !cancel;
    }
    /// The Now Playing sheet. It rises over the window like upstream's `DesktopPlayerSheet`
    /// (`shown` runs 0 to 1 with the slide) and covers the page; the chevron or Escape lowers it.
    fn expanded_view(&mut self, ctx: &egui::Context, shown: f32) {
        let screen = ctx.screen_rect();
        let top = screen.top() + (1.0 - shown) * screen.height();
        egui::Area::new(egui::Id::new("player-sheet"))
            .order(egui::Order::Middle)
            .fixed_pos(Pos2::new(screen.left(), top))
            .movable(false)
            .constrain(false)
            .show(ctx, |ui| {
                // Claims the whole sheet, so clicks never reach the page underneath.
                let (rect, _) = ui.allocate_exact_size(screen.size(), egui::Sense::click());
                self.player_sheet(ui, rect);
            });
    }
    fn player_sheet(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let painter = ui.painter().clone();
        painter.rect_filled(rect, 0.0, Color32::from_rgb(18, 19, 17));
        if let Some(back) = &self.backdrop {
            painter.image(
                back.id(),
                rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        }
        painter.rect_filled(rect, 0.0, Color32::from_black_alpha(150));
        // Darker towards the bottom, where the controls sit.
        let mut shade = egui::Mesh::default();
        let clear = Color32::TRANSPARENT;
        let dark = Color32::from_black_alpha(120);
        shade.colored_vertex(Pos2::new(rect.left(), rect.center().y), clear);
        shade.colored_vertex(Pos2::new(rect.right(), rect.center().y), clear);
        shade.colored_vertex(rect.right_bottom(), dark);
        shade.colored_vertex(rect.left_bottom(), dark);
        shade.add_triangle(0, 1, 2);
        shade.add_triangle(0, 2, 3);
        painter.add(shade);

        const PAD: f32 = 36.0;
        const HEADER: f32 = 68.0;
        let body = Rect::from_min_max(
            Pos2::new(rect.left() + PAD, rect.top() + HEADER),
            Pos2::new(rect.right() - PAD, rect.bottom() - 28.0),
        );
        let wide = body.width() >= 960.0;
        if wide && self.tab == PlayerTab::Controls {
            // Side by side there is no artwork-only view; the column shows the lyrics.
            self.tab = PlayerTab::Lyrics;
        }
        self.sheet_header(
            ui,
            Rect::from_min_size(rect.min, Vec2::new(rect.width(), HEADER)),
            wide,
        );

        const CONTROLS: f32 = 236.0;
        if wide {
            let gap = (body.width() * 0.05).clamp(32.0, 72.0);
            let column = ((body.width() - gap) * 0.46).min(620.0);
            let art = (body.height() - CONTROLS - 28.0).min(column).max(160.0);
            let width = art.max(380.0).min(column);
            let block = art + 28.0 + CONTROLS;
            let y = body.top() + ((body.height() - block) / 2.0).max(0.0);
            let x = body.left() + (column - width) / 2.0;
            self.sheet_artwork(
                ui,
                Rect::from_min_size(Pos2::new(x + (width - art) / 2.0, y), Vec2::splat(art)),
            );
            self.sheet_controls(
                ui,
                Rect::from_min_size(Pos2::new(x, y + art + 28.0), Vec2::new(width, CONTROLS)),
            );
            let panel =
                Rect::from_min_max(Pos2::new(body.left() + column + gap, body.top()), body.max);
            self.sheet_panel(ui, panel);
        } else {
            let width = body.width().min(640.0);
            let x = body.center().x - width / 2.0;
            let controls = Rect::from_min_size(
                Pos2::new(x, body.bottom() - CONTROLS),
                Vec2::new(width, CONTROLS),
            );
            let upper =
                Rect::from_min_max(body.min, Pos2::new(body.right(), controls.top() - 24.0));
            if self.tab == PlayerTab::Controls {
                let art = upper.width().min(upper.height()).max(120.0);
                self.sheet_artwork(ui, Rect::from_center_size(upper.center(), Vec2::splat(art)));
            } else {
                self.sheet_panel(ui, upper);
            }
            self.sheet_controls(ui, controls);
        }
        // The page's status line is hidden behind the sheet; failures must still be seen.
        if !self.message.is_empty() {
            fit_text(
                &painter,
                Pos2::new(rect.center().x, rect.bottom() - 14.0),
                egui::Align2::CENTER_CENTER,
                &self.message,
                FontId::proportional(12.0),
                MUTED,
                rect.width() - 80.0,
            );
        }
    }
    fn sheet_header(&mut self, ui: &mut egui::Ui, rect: Rect, wide: bool) {
        let painter = ui.painter().clone();
        region(
            ui,
            Rect::from_min_size(rect.min + Vec2::new(24.0, 16.0), Vec2::splat(38.0)),
            |u| {
                if widgets::button(u, Icon::Collapse, 38.0, false, "Close player · Esc").clicked()
                {
                    self.expanded = false;
                }
            },
        );
        if self.current.is_some() {
            painter.text(
                Pos2::new(rect.center().x, rect.top() + 25.0),
                egui::Align2::CENTER_CENTER,
                "PLAYING FROM",
                FontId::proportional(11.0),
                MUTED,
            );
            // Between the collapse button and the tabs, which take up to ~340 px on the right.
            fit_text(
                &painter,
                Pos2::new(rect.center().x, rect.top() + 43.0),
                egui::Align2::CENTER_CENTER,
                &self.now_origin,
                FontId::new(14.0, egui::FontFamily::Name("bold".into())),
                WHITE,
                (rect.width() - 2.0 * 360.0).max(120.0),
            );
        }
        let tabs: &[(&str, PlayerTab)] = if wide {
            &[("Lyrics", PlayerTab::Lyrics), ("Up next", PlayerTab::Queue)]
        } else {
            &[
                ("Now playing", PlayerTab::Controls),
                ("Lyrics", PlayerTab::Lyrics),
                ("Up next", PlayerTab::Queue),
            ]
        };
        let item = 98.0;
        let total = item * tabs.len() as f32 + 8.0;
        let seg = Rect::from_min_size(
            Pos2::new(rect.right() - 36.0 - total, rect.top() + 17.0),
            Vec2::new(total, 36.0),
        );
        // Dark, not white, glass: the backdrop can be a bright cover, which washed the labels out.
        painter.rect_filled(seg, 18.0, Color32::from_black_alpha(110));
        for (i, (label, tab)) in tabs.iter().enumerate() {
            let r = Rect::from_min_size(
                seg.min + Vec2::new(4.0 + item * i as f32, 4.0),
                Vec2::new(item, 28.0),
            );
            let selected = self.tab == *tab;
            let response = ui.interact(r, ui.id().with(("sheet-tab", i)), egui::Sense::click());
            response.widget_info(|| {
                egui::WidgetInfo::selected(
                    egui::WidgetType::SelectableLabel,
                    true,
                    selected,
                    *label,
                )
            });
            if selected {
                painter.rect_filled(r, 14.0, Color32::from_gray(236));
            } else if response.hovered() {
                painter.rect_filled(r, 14.0, Color32::from_white_alpha(14));
            }
            painter.text(
                r.center(),
                egui::Align2::CENTER_CENTER,
                *label,
                FontId::new(13.0, egui::FontFamily::Name("bold".into())),
                if selected {
                    Color32::from_rgb(20, 21, 19)
                } else {
                    MUTED
                },
            );
            if response.clicked() {
                self.tab = *tab;
            }
        }
    }
    fn sheet_artwork(&self, ui: &mut egui::Ui, art: Rect) {
        let radius = (art.width() * 0.03).clamp(8.0, 16.0);
        ui.painter().add(
            egui::Shadow {
                offset: [0, 18],
                blur: 60,
                spread: 0,
                color: Color32::from_black_alpha(150),
            }
            .as_shape(art, radius),
        );
        match (&self.cover, &self.current) {
            (Some(tex), _) => {
                egui::Image::new(tex)
                    .fit_to_exact_size(art.size())
                    .corner_radius(radius)
                    .paint_at(ui, art);
            }
            // Optimistic: the list's thumbnail stands in until the full artwork arrives.
            (None, Some(t)) => draw_image(ui, &t.thumbnail, art, None),
            (None, None) => {
                ui.painter()
                    .rect_filled(art, radius, Color32::from_white_alpha(12));
                widgets::paint(
                    ui.painter(),
                    Icon::Logo,
                    art.shrink(art.width() * 0.36),
                    MUTED,
                );
            }
        }
    }
    fn sheet_controls(&mut self, ui: &mut egui::Ui, rect: Rect) {
        let width = rect.width();
        region(ui, rect, |u| {
            u.spacing_mut().item_spacing.y = 0.0;
            // Title, artist and like.
            u.allocate_ui_with_layout(
                Vec2::new(width, 58.0),
                egui::Layout::left_to_right(Align::Center),
                |u| {
                    u.vertical(|u| {
                        u.set_width(width - 48.0);
                        let (title, artist) = self
                            .current
                            .as_ref()
                            .map_or(("Nothing playing".to_owned(), String::new()), |t| {
                                (t.title.clone(), t.artist.clone())
                            });
                        u.add(egui::Label::new(heading(title, 24.0)).truncate());
                        u.add_space(4.0);
                        u.add(
                            egui::Label::new(RichText::new(artist).color(MUTED).size(16.0))
                                .truncate(),
                        );
                    });
                    u.with_layout(egui::Layout::right_to_left(Align::Center), |u| {
                        let liked = self
                            .current
                            .as_ref()
                            .is_some_and(|t| self.library.liked.iter().any(|l| l.key() == t.key()));
                        let like = u.add_enabled_ui(self.current.is_some(), |u| {
                            widgets::button(
                                u,
                                Icon::Heart,
                                40.0,
                                liked,
                                if liked {
                                    "Remove from Liked Music"
                                } else {
                                    "Add to Liked Music"
                                },
                            )
                        });
                        if like.inner.clicked() {
                            if let Some(t) = self.current.clone() {
                                self.library.like(&t);
                                self.save();
                            }
                        }
                    });
                },
            );
            u.add_space(14.0);
            u.add_enabled_ui(!self.loading && self.player.is_active(), |u| {
                self.seekbar(u, width, "sheet-seek");
            });
            u.allocate_ui_with_layout(
                Vec2::new(width, 18.0),
                egui::Layout::left_to_right(Align::Center),
                |u| {
                    let small = |s: String| RichText::new(s).size(12.0).color(MUTED);
                    u.label(small(if self.loading {
                        "Opening stream…".into()
                    } else {
                        clock(self.status.position)
                    }));
                    u.with_layout(egui::Layout::right_to_left(Align::Center), |u| {
                        let d = self.duration();
                        u.label(small(if self.loading || d <= 0.0 {
                            clock(d)
                        } else {
                            format!("-{}", clock((d - self.status.position).max(0.0)))
                        }));
                    });
                },
            );
            u.add_space(12.0);
            // Transport.
            u.allocate_ui_with_layout(
                Vec2::new(width, 72.0),
                egui::Layout::left_to_right(Align::Center),
                |u| {
                    let gap = (width * 0.05).clamp(10.0, 26.0);
                    u.spacing_mut().item_spacing.x = gap;
                    let row = 36.0 * 2.0 + 46.0 * 2.0 + 68.0 + gap * 4.0;
                    u.add_space(((width - row) / 2.0 - gap).max(0.0));
                    if widgets::button(u, Icon::Shuffle, 36.0, self.shuffle, "Shuffle").clicked() {
                        self.shuffle = !self.shuffle;
                    }
                    if widgets::button(u, Icon::Previous, 46.0, false, "Previous · Alt+Left")
                        .clicked()
                    {
                        self.previous();
                    }
                    if self.play_button(u, 68.0).clicked() {
                        self.toggle();
                    }
                    if widgets::button(u, Icon::Next, 46.0, false, "Next · Alt+Right").clicked() {
                        self.next(false);
                    }
                    let repeat = widgets::button(
                        u,
                        Icon::Repeat,
                        36.0,
                        self.repeat > 0,
                        match self.repeat {
                            0 => "Repeat off",
                            1 => "Repeat all",
                            _ => "Repeat one",
                        },
                    );
                    if self.repeat == 2 {
                        u.painter().text(
                            repeat.rect.right_top() + Vec2::new(-6.0, 7.0),
                            egui::Align2::CENTER_CENTER,
                            "1",
                            FontId::new(10.0, egui::FontFamily::Name("bold".into())),
                            WHITE,
                        );
                    }
                    if repeat.clicked() {
                        self.repeat = (self.repeat + 1) % 3;
                    }
                },
            );
            u.add_space(12.0);
            // Volume.
            u.allocate_ui_with_layout(
                Vec2::new(width, 28.0),
                egui::Layout::left_to_right(Align::Center),
                |u| {
                    let slider = (width * 0.55).clamp(140.0, 280.0);
                    u.add_space((width - slider - 24.0) / 2.0);
                    let (r, _) = u.allocate_exact_size(Vec2::splat(16.0), egui::Sense::hover());
                    widgets::paint(u.painter(), Icon::Volume, r, MUTED);
                    u.add_space(8.0);
                    self.volume_slider(u, slider, "sheet-volume");
                },
            );
        });
    }
    /// The filled play/pause disc; a spinner takes the icon's place while the stream opens.
    fn play_button(&self, ui: &mut egui::Ui, size: f32) -> egui::Response {
        let (rect, response) = ui.allocate_exact_size(Vec2::splat(size), egui::Sense::click());
        let playing = !self.status.paused && !self.status.ended;
        response.widget_info(|| {
            egui::WidgetInfo::labeled(
                egui::WidgetType::Button,
                !self.loading,
                if playing {
                    "Pause · Space"
                } else {
                    "Play · Space"
                },
            )
        });
        let fill = if response.hovered() {
            Color32::WHITE
        } else {
            Color32::from_gray(232)
        };
        let scale = if response.is_pointer_button_down_on() {
            0.94
        } else {
            1.0
        };
        ui.painter()
            .circle_filled(rect.center(), size * 0.5 * scale, fill);
        let icon = Rect::from_center_size(rect.center(), Vec2::splat(size * 0.36));
        let ink = Color32::from_rgb(20, 21, 19);
        if self.loading {
            egui::Spinner::new()
                .size(size * 0.4)
                .color(ink)
                .paint_at(ui, icon);
        } else if playing {
            widgets::paint(ui.painter(), Icon::Pause, icon, ink);
        } else {
            widgets::paint(
                ui.painter(),
                Icon::Play,
                icon.translate(Vec2::new(size * 0.03, 0.0)),
                ink,
            );
        }
        response
    }
    fn sheet_panel(&mut self, ui: &mut egui::Ui, rect: Rect) {
        ui.painter()
            .rect_filled(rect, 20.0, Color32::from_black_alpha(70));
        let inner = rect.shrink2(Vec2::new(24.0, 20.0));
        match self.tab {
            PlayerTab::Queue => self.queue_panel(ui, inner),
            _ => self.lyrics_panel(ui, inner),
        }
    }
    fn lyrics_panel(&mut self, ui: &mut egui::Ui, rect: Rect) {
        region(ui, rect, |panel| {
            soft_controls(panel);
            panel.allocate_ui_with_layout(
                Vec2::new(rect.width(), 30.0),
                egui::Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(heading("Lyrics", 18.0));
                    if !self.lyrics_provider.is_empty() {
                        ui.label(
                            RichText::new(format!("· {}", self.lyrics_provider))
                                .color(MUTED)
                                .size(13.0),
                        );
                    }
                    if !self.lyrics.is_empty() {
                        ui.add_space(6.0);
                        let on = self.show_translation;
                        if ui
                            .selectable_label(on, toggle_text("Translate", on))
                            .on_hover_text("Show the lyrics in your translation language")
                            .clicked()
                        {
                            self.show_translation = !on;
                            if self.show_translation {
                                self.request_translation();
                            }
                        }
                        if self.show_translation && self.translating {
                            ui.spinner();
                        } else if let Some((_, _, Translation::SameLanguage(_))) =
                            self.translation.as_ref().filter(|_| self.show_translation)
                        {
                            ui.label(
                                RichText::new("Already in your language")
                                    .color(MUTED)
                                    .size(12.0),
                            );
                        }
                    }
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if ui
                            .add_enabled(self.current.is_some(), egui::Button::new("Reload"))
                            .on_hover_text("Search the selected source again")
                            .clicked()
                        {
                            self.request_lyrics(self.lyrics_override.clone());
                        }
                        let selected = self.lyrics_override.as_deref().unwrap_or("auto");
                        egui::ComboBox::from_id_salt("lyrics-provider")
                            .width(150.0)
                            .selected_text(if selected == "auto" {
                                "Automatic"
                            } else {
                                services::lyrics::PROVIDERS
                                    .iter()
                                    .find(|p| p.id == selected)
                                    .map_or(selected, |p| p.label)
                            })
                            .show_ui(ui, |ui| {
                                ui.selectable_value(
                                    &mut self.lyrics_override,
                                    None,
                                    "Automatic (enabled order)",
                                );
                                for provider in services::lyrics::PROVIDERS {
                                    ui.selectable_value(
                                        &mut self.lyrics_override,
                                        Some(provider.id.to_owned()),
                                        provider.label,
                                    );
                                }
                            });
                    });
                },
            );
            panel.add_space(12.0);
            if self.lyrics.is_empty() {
                panel.add_space((rect.height() * 0.3).max(0.0));
                panel.vertical_centered(|ui| {
                    let waiting = self.lyrics_message.starts_with("Finding");
                    if waiting {
                        ui.add(egui::Spinner::new().size(22.0).color(MUTED));
                        ui.add_space(10.0);
                    }
                    ui.label(
                        RichText::new(if self.current.is_none() {
                            "Play a song to see its lyrics"
                        } else if self.lyrics_message.is_empty() {
                            "Lyrics not available"
                        } else {
                            &self.lyrics_message
                        })
                        .color(MUTED)
                        .size(15.0),
                    );
                    if !waiting && self.current.is_some() && self.library.lyrics_enabled {
                        ui.add_space(10.0);
                        if ui.button("Try again").clicked() {
                            self.request_lyrics(self.lyrics_override.clone());
                        }
                    }
                });
                return;
            }
            let position = self.status.position;
            let lines = self.display_lyrics();
            let mut active_idx = None;
            let mut seek_to = None;
            egui::ScrollArea::vertical()
                .id_salt("sheet-lyrics")
                .auto_shrink([false, false])
                .show(panel, |u| {
                    u.spacing_mut().item_spacing.y = 0.0;
                    // Room above the first and below the last line, so either can sit centred.
                    let margin = (rect.height() * 0.35).max(0.0);
                    u.add_space(margin * 0.4);
                    for (i, line) in lines.iter().enumerate() {
                        let active = position >= line.time
                            && lines.get(i + 1).is_none_or(|n| position < n.time);
                        let past = !active && position >= line.time;
                        if active {
                            active_idx = Some(i);
                        }
                        let dim = if past {
                            Color32::from_white_alpha(70)
                        } else {
                            Color32::from_white_alpha(110)
                        };
                        let size = if active { 28.0 } else { 24.0 };
                        let r = if line.words.is_empty() {
                            let r = u.add(
                                egui::Label::new(heading(&line.text, size).color(if active {
                                    WHITE
                                } else {
                                    dim
                                }))
                                .wrap()
                                .sense(egui::Sense::click()),
                            );
                            if r.clicked() {
                                seek_to = Some(line.time);
                            }
                            r
                        } else {
                            u.horizontal_wrapped(|u| {
                                // Words are stored trimmed; a word gap keeps "aku milikmu" from reading "akumilikmu".
                                u.spacing_mut().item_spacing.x = size * 0.26;
                                for word in &line.words {
                                    let sung = active && position >= word.start;
                                    let r = u.add(
                                        egui::Label::new(
                                            heading(&word.text, size).color(if sung {
                                                WHITE
                                            } else {
                                                dim
                                            }),
                                        )
                                        .sense(egui::Sense::click()),
                                    );
                                    if r.clicked() {
                                        seek_to = Some(word.start);
                                    }
                                }
                            })
                            .response
                        };
                        if r.hovered() {
                            u.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
                        }
                        if active && self.follow_lyrics && self.active_lyric != Some(i) {
                            r.scroll_to_me(Some(Align::Center));
                        }
                        u.add_space(18.0);
                    }
                    u.add_space(margin);
                });
            self.active_lyric = active_idx;
            if let Some(time) = seek_to {
                let result = self.player.seek(time);
                self.report(result);
            }
        });
    }
    fn queue_panel(&mut self, ui: &mut egui::Ui, rect: Rect) {
        region(ui, rect, |panel| {
            soft_controls(panel);
            panel.allocate_ui_with_layout(
                Vec2::new(rect.width(), 30.0),
                egui::Layout::left_to_right(Align::Center),
                |ui| {
                    ui.label(heading("Up next", 18.0));
                    ui.with_layout(egui::Layout::right_to_left(Align::Center), |ui| {
                        if !self.library.queue.is_empty() && ui.button("Clear").clicked() {
                            self.library.queue.clear();
                            self.save();
                        }
                        if ui
                            .selectable_label(
                                self.library.autoplay,
                                toggle_text("∞ AutoPlay", self.library.autoplay),
                            )
                            .on_hover_text("Keep playing similar songs when the queue ends")
                            .clicked()
                        {
                            self.library.autoplay = !self.library.autoplay;
                            self.save();
                        }
                    });
                },
            );
            panel.add_space(10.0);
            egui::ScrollArea::vertical()
                .id_salt("sheet-queue")
                .auto_shrink([false, false])
                .show(panel, |u| {
                    let width = u.available_width();
                    if let Some(t) = self.current.clone() {
                        u.label(RichText::new("NOW PLAYING").size(11.0).color(MUTED));
                        u.add_space(4.0);
                        if self.queue_row(u, &t, None, width).0 {
                            self.toggle();
                        }
                        u.add_space(14.0);
                    }
                    if self.library.queue.is_empty() {
                        u.add_space(24.0);
                        u.vertical_centered(|u| {
                            u.label(RichText::new("Your queue is empty").color(MUTED).size(15.0));
                            if self.library.autoplay {
                                u.label(
                                    RichText::new("AutoPlay will continue with similar songs")
                                        .color(MUTED)
                                        .size(12.0),
                                );
                            }
                        });
                        return;
                    }
                    u.label(
                        RichText::new(format!("NEXT · {} SONGS", self.library.queue.len()))
                            .size(11.0)
                            .color(MUTED),
                    );
                    u.add_space(4.0);
                    let mut action = None;
                    for i in 0..self.library.queue.len() {
                        let t = self.library.queue[i].clone();
                        u.push_id(("queue", i), |u| {
                            let (clicked, a) = self.queue_row(u, &t, Some(i), width);
                            if clicked {
                                action = Some((i, QueueAction::Play));
                            } else if let Some(a) = a {
                                action = Some((i, a));
                            }
                        });
                    }
                    if let Some((i, a)) = action {
                        let q = &mut self.library.queue;
                        match a {
                            QueueAction::Play => {
                                let t = q.remove(i);
                                if let Some(old) = self.current.clone() {
                                    self.previous.push(old);
                                }
                                self.play(t);
                            }
                            QueueAction::Up if i > 0 => q.swap(i, i - 1),
                            QueueAction::Down if i + 1 < q.len() => q.swap(i, i + 1),
                            QueueAction::Remove => {
                                q.remove(i);
                            }
                            _ => {}
                        }
                        self.save();
                    }
                });
        });
    }
    /// A compact queue row; `index` is `None` for the current track. Reorder and remove
    /// buttons appear on hover.
    fn queue_row(
        &mut self,
        ui: &mut egui::Ui,
        track: &Track,
        index: Option<usize>,
        width: f32,
    ) -> (bool, Option<QueueAction>) {
        let (rect, response) = ui.allocate_exact_size(Vec2::new(width, 58.0), egui::Sense::click());
        let current = index.is_none();
        let hovered = ui.rect_contains_pointer(rect);
        if hovered || current {
            ui.painter().rect_filled(
                rect,
                8.0,
                Color32::from_white_alpha(if current { 12 } else { 8 }),
            );
        }
        let art = Rect::from_min_size(rect.min + Vec2::new(7.0, 7.0), Vec2::splat(44.0));
        draw_cover(ui, track, art, None);
        if current {
            self.now_playing_badge(ui, art);
        }
        let buttons = if hovered && !current { 96.0 } else { 0.0 };
        let text_x = art.right() + 12.0;
        let max = rect.right() - text_x - buttons - 52.0;
        fit_text(
            ui.painter(),
            Pos2::new(text_x, rect.top() + 21.0),
            egui::Align2::LEFT_CENTER,
            &track.title,
            FontId::new(14.0, egui::FontFamily::Name("bold".into())),
            WHITE,
            max,
        );
        fit_text(
            ui.painter(),
            Pos2::new(text_x, rect.top() + 39.0),
            egui::Align2::LEFT_CENTER,
            if track.artist.is_empty() {
                "Local audio"
            } else {
                &track.artist
            },
            FontId::proportional(13.0),
            MUTED,
            max,
        );
        let mut action = None;
        if buttons > 0.0 {
            region(
                ui,
                Rect::from_min_size(
                    Pos2::new(rect.right() - buttons - 4.0, rect.center().y - 15.0),
                    Vec2::new(buttons, 30.0),
                ),
                |u| {
                    u.horizontal(|u| {
                        u.spacing_mut().item_spacing.x = 2.0;
                        if widgets::button(u, Icon::Collapse, 30.0, false, "Move down").clicked() {
                            action = Some(QueueAction::Down);
                        }
                        if widgets::button(u, Icon::Raise, 30.0, false, "Move up").clicked() {
                            action = Some(QueueAction::Up);
                        }
                        if widgets::button(u, Icon::Close, 30.0, false, "Remove from queue")
                            .clicked()
                        {
                            action = Some(QueueAction::Remove);
                        }
                    });
                },
            );
        } else if track.duration > 0.0 {
            ui.painter().text(
                Pos2::new(rect.right() - 12.0, rect.center().y),
                egui::Align2::RIGHT_CENTER,
                clock(track.duration),
                FontId::proportional(12.0),
                MUTED,
            );
        }
        response.context_menu(|ui| self.track_menu(ui, track));
        (response.clicked() && action.is_none(), action)
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
    Create {
        name: String,
        privacy: String,
        seed: Option<Track>,
    },
    Rename {
        id: String,
        name: String,
    },
    Delete {
        id: String,
        title: String,
    },
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

#[derive(Clone, Copy)]
enum QueueAction {
    Play,
    Up,
    Down,
    Remove,
}

impl eframe::App for YTDugem {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.events(ctx);
        if services::take_sign_in_rejected() {
            self.message = "Your YouTube sign-in has expired, so public music is shown. Open your account to sign in again.".into();
        }
        let typing = ctx.wants_keyboard_input();
        let (search, local, quit, escape) = ctx.input_mut(|i| {
            (
                i.consume_key(egui::Modifiers::CTRL, egui::Key::F),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::L),
                i.consume_key(egui::Modifiers::CTRL, egui::Key::Q),
                i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
            )
        });
        if search {
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("search_input")));
        }
        if local {
            self.page = Page::Local;
            ctx.memory_mut(|m| m.request_focus(egui::Id::new("local-import")));
        }
        if quit {
            self.quitting = true;
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
        self.tray_step(ctx);
        if escape {
            self.expanded = false;
            if self.account_open {
                self.login_cancel.store(true, Ordering::SeqCst);
                self.account_request += 1;
                self.account_busy = false;
                self.account_open = false;
                self.cookie_input.clear();
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
                if !i.modifiers.any() && i.key_pressed(egui::Key::L) {
                    self.tab = PlayerTab::Lyrics;
                }
                if !i.modifiers.any() && i.key_pressed(egui::Key::Q) {
                    self.tab = PlayerTab::Queue;
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
            egui::Id::new("player-sheet-shown"),
            self.expanded,
            if self.expanded { 0.36 } else { 0.28 },
            egui::emath::easing::cubic_in_out,
        );
        if shown < 1.0 {
            egui::SidePanel::left("sidebar")
                .exact_width(258.0)
                .resizable(false)
                .frame(egui::Frame::new().fill(SIDE).inner_margin(0.0))
                .show(ctx, |ui| {
                    self.sidebar(ui, ui.max_rect());
                });
            egui::TopBottomPanel::top("topbar")
                .exact_height(78.0)
                .frame(egui::Frame::new().fill(SIDE).inner_margin(0.0))
                .show(ctx, |ui| {
                    self.topbar(ui, ui.max_rect());
                });
            egui::CentralPanel::default()
                .frame(
                    egui::Frame::new()
                        .fill(BG)
                        .inner_margin(egui::Margin::same(28)),
                )
                .show(ctx, |ui| {
                    egui::ScrollArea::vertical().show(ui, |ui| {
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
        self.account_dialog(ctx);
        self.playlist_dialog(ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
        self.screenshot_step(ctx);
    }
}

/// Translucent pill buttons and combo boxes for controls drawn over the player's artwork backdrop.
/// Label for a soft toggle: dark on the light "on" pill, light otherwise.
fn toggle_text(text: &str, on: bool) -> RichText {
    RichText::new(text).color(if on {
        Color32::from_rgb(20, 21, 19)
    } else {
        WHITE
    })
}
fn soft_controls(ui: &mut egui::Ui) {
    let w = &mut ui.visuals_mut().widgets;
    for (state, alpha) in [
        (&mut w.inactive, 16),
        (&mut w.hovered, 30),
        (&mut w.active, 40),
        (&mut w.open, 30),
    ] {
        state.weak_bg_fill = Color32::from_white_alpha(alpha);
        state.bg_fill = Color32::from_white_alpha(alpha);
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
    id: impl std::hash::Hash,
    icon: Icon,
    label: &str,
    selected: bool,
) -> egui::Response {
    let response = ui.interact(r, ui.make_persistent_id(id), egui::Sense::click());
    response.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, label));
    if selected || response.hovered() {
        ui.painter().rect_filled(
            r,
            6.0,
            Color32::from_white_alpha(if selected { 30 } else { 12 }),
        );
    }
    let color = if selected { WHITE } else { MUTED };
    widgets::paint(
        ui.painter(),
        icon,
        Rect::from_min_size(r.min + Vec2::new(11.0, 10.0), Vec2::splat(18.0)),
        color,
    );
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

fn configure_style(ctx: &egui::Context) {
    egui_extras::install_image_loaders(ctx);
    let mut fonts = egui::FontDefinitions::default();
    fonts.font_data.insert(
        "inter".into(),
        Arc::new(egui::FontData::from_static(include_bytes!(
            "../assets/fonts/Inter-Regular.ttf"
        ))),
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
            fonts
                .font_data
                .insert(name.clone(), Arc::new(egui::FontData::from_owned(bytes)));
            for family in [egui::FontFamily::Proportional, egui::FontFamily::Monospace] {
                fonts.families.entry(family).or_default().push(name.clone());
            }
        }
    }
    let mut bold = vec!["inter-semibold".into()];
    bold.extend(fonts.families[&egui::FontFamily::Proportional].clone());
    fonts
        .families
        .insert(egui::FontFamily::Name("bold".into()), bold);
    ctx.set_fonts(fonts);
    let mut style = (*ctx.style()).clone();
    style.visuals = egui::Visuals::dark();
    style.visuals.panel_fill = BG;
    style.visuals.window_fill = Color32::from_rgb(35, 36, 33);
    style.visuals.override_text_color = Some(WHITE);
    style.visuals.faint_bg_color = Color32::from_white_alpha(8);
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
    ctx.set_style(style);
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
