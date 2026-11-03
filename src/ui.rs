use crate::{
    auth::{self, Account, SavedAccount},
    discord::Presence,
    integrations::{Integrations, MediaEvent},
    model::{clock, Track},
    player::{Player, PlayerStatus},
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
    Results(u64, Result<Vec<Track>, String>),
    Home(u64, Result<HomeFeed, String>),
    Supplement(u64, Result<Vec<HomeShelf>, String>),
    Recents(u64, Result<Vec<Track>, String>),
    Collection(u64, Result<Collection, String>),
    Imported(Result<Vec<Track>, String>),
    Lyrics(u64, String, Result<services::lyrics::Lyrics, String>),
    Downloaded(Result<PathBuf, String>),
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
    downloading: bool,
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
    /// `--screenshot FILE`: save the window after `SCREENSHOT_FRAMES` frames, then exit.
    screenshot: Option<PathBuf>,
    screenshot_requested: bool,
    frames: u32,
}

const SCREENSHOT_FRAMES: u32 = 120;

impl YTDugem {
    pub fn with_screenshot(mut self, path: Option<PathBuf>) -> Self {
        self.screenshot = path;
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
        let integrations = {
            use raw_window_handle::{HasWindowHandle, RawWindowHandle};
            let hwnd = cc.window_handle().ok().and_then(|h| match h.as_raw() {
                RawWindowHandle::Win32(w) => Some(w.hwnd.get() as *mut std::ffi::c_void),
                _ => None,
            });
            Integrations::for_window(hwnd)
        };
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
            downloading: false,
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
            screenshot: None,
            screenshot_requested: false,
            frames: 0,
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
        self.lyrics_request += 1;
        let id = self.play_request;
        self.loading = true;
        self.player.stop();
        self.status = PlayerStatus {
            paused: true,
            ..Default::default()
        };
        let (volume, speed, audio) = (
            self.library.volume,
            self.library.speed,
            self.library.audio.clone(),
        );
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let mut p = Player::new();
            let r = p
                .load_with_audio(&t.source, volume, speed, &audio)
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
            Ok(p) => {
                self.player = p;
                self.library.remember(&t);
                self.status = PlayerStatus::default();
                self.lyrics.clear();
                self.active_lyric = None;
                self.lyrics_override = None;
                self.lyrics_request += 1;
                self.cover = None;
                self.backdrop = None;
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
                self.current = Some(t);
                self.discord.clear();
                if self.library.lyrics_enabled {
                    self.request_lyrics(None);
                } else {
                    self.lyrics_message = "Lyrics are disabled in Settings".into();
                }
                self.save();
            }
            Err(e) => self.message = format!("Playback failed: {e}"),
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
        let i = if self.shuffle {
            rand::random_range(0..self.library.queue.len())
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
    fn toggle(&mut self) {
        if self.loading {
            return;
        }
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
        self.player.stop();
        self.status.paused = true;
        self.status.ended = true;
        self.discord.clear();
    }
    fn download(&mut self, t: Track) {
        if self.downloading {
            self.message = "Another download is in progress".into();
            return;
        }
        self.downloading = true;
        let tx = self.tx.clone();
        let dir = self.library.download_dir.clone();
        std::thread::spawn(move || {
            let _ = tx.send(Event::Downloaded(services::download(&t, &dir).map_err(err)));
        });
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
                Event::Downloaded(r) => {
                    self.downloading = false;
                    match r {
                        Ok(p) => {
                            self.message = format!("Saved {}", p.display());
                            self.import(p);
                        }
                        Err(e) => self.message = e,
                    }
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
                        egui::TextureOptions::LINEAR,
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
                    if widgets::button(
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
            ui.painter().text(
                Pos2::new(card.left() + 61.0, card.top() + 15.0),
                egui::Align2::LEFT_CENTER,
                truncate(&t.title, (card_width / 8.0 - 9.0).max(8.0) as usize),
                FontId::new(12.5, egui::FontFamily::Name("bold".into())),
                WHITE,
            );
            ui.painter().text(
                Pos2::new(card.left() + 61.0, card.top() + 33.0),
                egui::Align2::LEFT_CENTER,
                truncate(&t.artist, (card_width / 8.0 - 9.0).max(8.0) as usize),
                FontId::proportional(11.0),
                MUTED,
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
        let text_x = art.right() + 13.0;
        let extra = if index.is_some() { 100.0 } else { 36.0 };
        let max = ((rect.right() - text_x - extra) / 7.6).max(8.0) as usize;
        ui.painter().text(
            Pos2::new(text_x, rect.top() + 23.0),
            egui::Align2::LEFT_CENTER,
            truncate(&track.title, max),
            FontId::new(15.0, egui::FontFamily::Name("bold".into())),
            WHITE,
        );
        ui.painter().text(
            Pos2::new(text_x, rect.top() + 46.0),
            egui::Align2::LEFT_CENTER,
            truncate(
                if track.artist.is_empty() {
                    "Local audio"
                } else {
                    &track.artist
                },
                max,
            ),
            FontId::proportional(14.0),
            MUTED,
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
                                            egui::Button::new(heading(
                                                truncate(&item.title, 22),
                                                15.0,
                                            ))
                                            .frame(false),
                                        )
                                        .clicked()
                                    {
                                        self.open_item(item.clone());
                                    }
                                    ui.label(
                                        RichText::new(truncate(&item.subtitle, 25))
                                            .size(14.0)
                                            .color(MUTED),
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
                self.track_list(ui, self.library.liked.clone(), "Liked Music");
            }
            Page::History => self.track_list(ui, self.library.history.clone(), "History"),
            Page::Downloads => {
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
    fn expanded_view(&mut self, ctx: &egui::Context) {
        let screen = ctx.input(|i| i.screen_rect());
        let painter = ctx.layer_painter(egui::LayerId::background());
        if let Some(back) = &self.backdrop {
            painter.image(
                back.id(),
                screen,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else {
            painter.rect_filled(screen, 0.0, Color32::from_rgb(27, 18, 17));
        }
        painter.rect_filled(screen, 0.0, Color32::from_black_alpha(132));
        painter.rect_filled(
            Rect::from_min_max(Pos2::new(0.0, screen.bottom() * 0.76), screen.max),
            0.0,
            Color32::from_black_alpha(95),
        );
        let width = (screen.width() - 96.0).min(1320.0);
        let gap = width * 0.06;
        let side = ((width - gap) / 2.0).min(screen.height() - 80.0).max(180.0);
        let left = screen.center().x - width / 2.0;
        let top = (screen.height() - side) * 0.35;
        let art_rect = Rect::from_min_size(Pos2::new(left, top), Vec2::splat(side));
        if let Some(tex) = &self.cover {
            painter.image(
                tex.id(),
                art_rect,
                Rect::from_min_max(Pos2::ZERO, Pos2::new(1.0, 1.0)),
                Color32::WHITE,
            );
        } else if let Some(t) = &self.current {
            painter.rect_filled(art_rect, 8.0, Color32::from_gray(35));
            painter.text(
                art_rect.center(),
                egui::Align2::CENTER_CENTER,
                &t.title,
                FontId::proportional(22.0),
                WHITE,
            );
        }
        let right_x = left + side + gap;
        let right = Rect::from_min_max(
            Pos2::new(right_x, 50.0),
            Pos2::new(right_x + side, screen.bottom() - 50.0),
        );
        egui::CentralPanel::default()
            .frame(egui::Frame::new().fill(Color32::TRANSPARENT))
            .show(ctx, |ui| {
                let handle = Rect::from_center_size(
                    Pos2::new(screen.center().x, 14.0),
                    Vec2::new(48.0, 6.0),
                );
                let response = ui.interact(
                    handle.expand(12.0),
                    ui.make_persistent_id("collapse-player"),
                    egui::Sense::click(),
                );
                painter.rect_filled(handle, 3.0, Color32::from_gray(205));
                if response.clicked() {
                    self.expanded = false;
                }
                let icon_y = screen.bottom() - 105.0;
                for (x, icon, tab) in [
                    (left + 90.0, Icon::Lyrics, PlayerTab::Lyrics),
                    (left + side * 0.48, Icon::Headphones, PlayerTab::Controls),
                    (left + side - 90.0, Icon::Queue, PlayerTab::Queue),
                ] {
                    region(
                        ui,
                        Rect::from_center_size(Pos2::new(x, icon_y), Vec2::splat(46.0)),
                        |u| {
                            if widgets::button(
                                u,
                                icon,
                                42.0,
                                self.tab == tab,
                                match tab {
                                    PlayerTab::Lyrics => "Lyrics (L)",
                                    PlayerTab::Queue => "Queue (Q)",
                                    _ => "Playback settings",
                                },
                            )
                            .clicked()
                            {
                                self.tab = tab;
                            }
                        },
                    );
                }
                let mut panel = ui.new_child(
                    egui::UiBuilder::new()
                        .max_rect(right)
                        .layout(egui::Layout::top_down(Align::LEFT)),
                );
                match self.tab {
                    PlayerTab::Controls => {
                        region(
                            ui,
                            Rect::from_min_size(
                                Pos2::new(
                                    right.right() - 48.0,
                                    right.top() + right.height() * 0.30 + 32.0,
                                ),
                                Vec2::splat(44.0),
                            ),
                            |u| {
                                let liked = self.current.as_ref().is_some_and(|t| {
                                    self.library.liked.iter().any(|l| l.id == t.id)
                                });
                                if widgets::button(u, Icon::Heart, 42.0, liked, "Like").clicked() {
                                    if let Some(t) = self.current.clone() {
                                        self.library.like(&t);
                                        self.save();
                                    }
                                }
                            },
                        );
                        panel.add_space(right.height() * 0.30);
                        if let Some(t) = self.current.clone() {
                            panel.label(
                                RichText::new(format!("Playing from {}", self.now_origin))
                                    .strong()
                                    .size(14.0),
                            );
                            panel.add_space(12.0);
                            panel.label(heading(&t.title, 23.0));
                            panel.label(RichText::new(&t.artist).color(MUTED).size(17.0));
                        } else {
                            panel.label(heading("Nothing playing", 24.0));
                        }
                        panel.add_space(22.0);
                        if let Some(line) = self
                            .lyrics
                            .iter()
                            .rev()
                            .find(|line| line.time <= self.status.position)
                        {
                            if panel.button(&line.text).clicked() {
                                self.tab = PlayerTab::Lyrics;
                            }
                        }
                        self.seekbar(&mut panel, right.width(), "expanded-seek");
                        panel.horizontal(|u| {
                            u.label(clock(self.status.position));
                            u.with_layout(egui::Layout::right_to_left(Align::Center), |u| {
                                u.label(format!(
                                    "-{}",
                                    clock((self.duration() - self.status.position).max(0.0))
                                ));
                            });
                        });
                        panel.add_space(28.0);
                        panel.allocate_ui_with_layout(
                            Vec2::new(right.width(), 64.0),
                            egui::Layout::left_to_right(Align::Center),
                            |u| {
                                u.add_space((right.width() * 0.10).max(8.0));
                                if widgets::button(u, Icon::Previous, 52.0, false, "Previous")
                                    .clicked()
                                {
                                    self.previous();
                                }
                                u.add_space((right.width() * 0.12).max(8.0));
                                if widgets::button(
                                    u,
                                    if self.status.paused {
                                        Icon::Play
                                    } else {
                                        Icon::Pause
                                    },
                                    64.0,
                                    false,
                                    "Play / pause",
                                )
                                .clicked()
                                {
                                    self.toggle();
                                }
                                u.add_space((right.width() * 0.12).max(8.0));
                                if widgets::button(u, Icon::Next, 52.0, false, "Next").clicked() {
                                    self.next(false);
                                }
                            },
                        );
                        panel.add_space(24.0);
                        self.volume_slider(&mut panel, right.width(), "expanded-volume");
                    }
                    PlayerTab::Lyrics => {
                        panel.horizontal(|ui| {
                            ui.label("Lyrics source");
                            let selected = self.lyrics_override.as_deref().unwrap_or("auto");
                            egui::ComboBox::from_id_salt("lyrics-provider")
                                .selected_text(if selected == "auto" {
                                    "Automatic (enabled order)"
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
                            if ui.button("Load").clicked() {
                                self.request_lyrics(self.lyrics_override.clone());
                            }
                            if !self.lyrics_provider.is_empty() {
                                ui.label(
                                    RichText::new(format!("From {}", self.lyrics_provider))
                                        .color(MUTED),
                                );
                            }
                        });
                        egui::ScrollArea::vertical()
                            .max_height(right.height())
                            .stick_to_bottom(false)
                            .show(&mut panel, |u| {
                                let mut active_idx = None;
                                let mut seek_to = None;
                                for (i, line) in self.lyrics.iter().enumerate() {
                                    let active = self.status.position >= line.time
                                        && self
                                            .lyrics
                                            .get(i + 1)
                                            .is_none_or(|n| self.status.position < n.time);
                                    if active {
                                        active_idx = Some(i);
                                    }
                                    let r = if line.words.is_empty() {
                                        let r = u.add(
                                            egui::Label::new(
                                                heading(
                                                    &line.text,
                                                    if active { 38.0 } else { 28.0 },
                                                )
                                                .color(if active {
                                                    WHITE
                                                } else {
                                                    Color32::from_gray(145)
                                                }),
                                            )
                                            .sense(egui::Sense::click()),
                                        );
                                        if r.clicked() {
                                            seek_to = Some(line.time);
                                        }
                                        r
                                    } else {
                                        u.horizontal_wrapped(|u| {
                                            for word in &line.words {
                                                let word_active = self.status.position
                                                    >= word.start
                                                    && self.status.position < word.end;
                                                let r = u.add(
                                                    egui::Label::new(
                                                        heading(
                                                            &word.text,
                                                            if word_active {
                                                                38.0
                                                            } else if active {
                                                                30.0
                                                            } else {
                                                                24.0
                                                            },
                                                        )
                                                        .color(if word_active {
                                                            WHITE
                                                        } else {
                                                            Color32::from_gray(145)
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
                                    if active && self.follow_lyrics && self.active_lyric != Some(i)
                                    {
                                        r.scroll_to_me(Some(egui::Align::Center));
                                    }
                                    u.add_space(15.0);
                                }
                                self.active_lyric = active_idx;
                                if let Some(time) = seek_to {
                                    let result = self.player.seek(time);
                                    self.report(result);
                                }
                                if self.lyrics.is_empty() {
                                    u.label(
                                        RichText::new(if self.lyrics_message.is_empty() {
                                            "Lyrics not available"
                                        } else {
                                            &self.lyrics_message
                                        })
                                        .color(MUTED),
                                    );
                                    if u.button("Retry lyrics").clicked() {
                                        self.request_lyrics(self.lyrics_override.clone());
                                    }
                                }
                            });
                    }
                    PlayerTab::Queue => {
                        panel.label(heading("Queue", 24.0));
                        panel.add_space(18.0);
                        egui::ScrollArea::vertical().show(&mut panel, |u| {
                            if let Some(t) = self.current.clone() {
                                u.label(RichText::new("Now playing").color(MUTED));
                                u.horizontal(|u| {
                                    if self.song_row(
                                        u,
                                        &t,
                                        None,
                                        (u.available_width() - 38.0).max(80.0),
                                    ) {
                                        self.toggle();
                                    }
                                    if widgets::button(u, Icon::Close, 28.0, false, "Remove")
                                        .clicked()
                                    {
                                        self.stop();
                                        self.current = None;
                                    }
                                });
                            }
                            if !self.library.queue.is_empty() {
                                u.horizontal(|u| {
                                    if u.selectable_label(self.library.autoplay, "∞  AutoPlay")
                                        .clicked()
                                    {
                                        self.library.autoplay = !self.library.autoplay;
                                        self.save();
                                    }
                                    if widgets::button(
                                        u,
                                        Icon::Shuffle,
                                        30.0,
                                        self.shuffle,
                                        "Shuffle",
                                    )
                                    .clicked()
                                    {
                                        self.shuffle = !self.shuffle;
                                    }
                                    if widgets::button(
                                        u,
                                        Icon::Repeat,
                                        30.0,
                                        self.repeat > 0,
                                        "Repeat",
                                    )
                                    .clicked()
                                    {
                                        self.repeat = (self.repeat + 1) % 3;
                                    }
                                });
                            }
                            for i in 0..self.library.queue.len() {
                                let t = self.library.queue[i].clone();
                                let mut changed = false;
                                u.horizontal(|u| {
                                    if self.song_row(
                                        u,
                                        &t,
                                        None,
                                        (u.available_width() - 112.0).max(80.0),
                                    ) {
                                        self.library.queue.remove(i);
                                        self.play(t);
                                        changed = true;
                                    }
                                    if widgets::button(u, Icon::Back, 28.0, false, "Move earlier")
                                        .clicked()
                                        && i > 0
                                        && !changed
                                    {
                                        self.library.queue.swap(i, i - 1);
                                        self.save();
                                    }
                                    if widgets::button(u, Icon::Next, 28.0, false, "Move later")
                                        .clicked()
                                        && i + 1 < self.library.queue.len()
                                        && !changed
                                    {
                                        self.library.queue.swap(i, i + 1);
                                        self.save();
                                    }
                                    if widgets::button(u, Icon::Close, 28.0, false, "Remove")
                                        .clicked()
                                        && !changed
                                    {
                                        self.library.queue.remove(i);
                                        self.save();
                                        changed = true;
                                    }
                                });
                                if changed {
                                    break;
                                }
                            }
                            if self.library.queue.is_empty() {
                                u.label(RichText::new("Queue is empty").color(MUTED));
                            }
                        });
                    }
                }
            });
    }
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
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
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
        if self.expanded {
            self.expanded_view(ctx);
        } else {
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
        self.account_dialog(ctx);
        ctx.request_repaint_after(Duration::from_millis(100));
        self.screenshot_step(ctx);
    }
}

fn region<R>(ui: &mut egui::Ui, rect: Rect, f: impl FnOnce(&mut egui::Ui) -> R) -> R {
    let mut child = ui.new_child(
        egui::UiBuilder::new()
            .max_rect(rect)
            .layout(egui::Layout::top_down(Align::LEFT)),
    );
    f(&mut child)
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
    ui.painter().text(
        Pos2::new(r.left() + 43.0, r.center().y),
        egui::Align2::LEFT_CENTER,
        truncate(label, 23),
        FontId::proportional(15.0),
        color,
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
    if let Some(tex) = texture {
        egui::Image::new(tex)
            .fit_to_exact_size(r.size())
            .corner_radius(8.0)
            .paint_at(ui, r);
    } else if url.starts_with("https://") {
        egui::Image::new(url)
            .fit_to_exact_size(r.size())
            .corner_radius(8.0)
            .paint_at(ui, r);
    } else {
        ui.painter()
            .rect_filled(r, 8.0, Color32::from_rgb(51, 53, 48));
        widgets::paint(ui.painter(), Icon::Logo, r.shrink(r.width() * 0.3), MUTED);
    }
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

fn load_artwork(url: &str) -> anyhow::Result<(egui::ColorImage, egui::ColorImage)> {
    use std::io::Read;
    let response = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(15))
        .build()?
        .get(url)
        .send()?
        .error_for_status()?;
    let mut bytes = vec![];
    response.take(8 * 1024 * 1024 + 1).read_to_end(&mut bytes)?;
    anyhow::ensure!(bytes.len() <= 8 * 1024 * 1024, "Artwork is too large");
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(64 * 1024 * 1024);
    reader.limits(limits);
    let im = reader.decode()?;
    let cover = im
        .resize(1024, 1024, image::imageops::FilterType::Triangle)
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
