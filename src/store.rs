use crate::model::Track;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Playlist {
    /// Stable identifier; libraries saved before ids existed get one on load.
    #[serde(default)]
    pub id: String,
    pub name: String,
    pub tracks: Vec<Track>,
}

fn new_playlist_id(playlists: &[Playlist]) -> String {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or_default();
    let mut index = playlists.len();
    loop {
        let id = format!("local-{nanos}-{index}");
        if !playlists.iter().any(|p| p.id == id) {
            return id;
        }
        index += 1;
    }
}

/// Give blank or duplicated playlist ids a fresh, unique value.
fn assign_playlist_ids(playlists: &mut [Playlist]) {
    for i in 0..playlists.len() {
        let duplicate = playlists[..i].iter().any(|p| p.id == playlists[i].id);
        if playlists[i].id.trim().is_empty() || duplicate {
            playlists[i].id = new_playlist_id(playlists);
        }
    }
}

fn clean_playlist_name(name: &str) -> Result<String> {
    let name = name.trim();
    if name.is_empty() {
        anyhow::bail!("A playlist needs a name");
    }
    Ok(name.to_owned())
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    /// No longer shown (YouTube Music's own Liked Music playlist replaces it), but still saved
    /// so songs liked in earlier versions are not erased from the library file.
    pub liked: Vec<Track>,
    pub local: Vec<Track>,
    pub history: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub queue: Vec<Track>,
    pub volume: f64,
    pub speed: f64,
    pub download_dir: PathBuf,
    pub autoplay: bool,
    #[serde(default = "lyrics_enabled_default")]
    pub lyrics_enabled: bool,
    #[serde(default = "lyrics_order_default")]
    pub lyrics_order: Vec<String>,
    #[serde(default = "lyrics_sources_default")]
    pub lyrics_sources: Vec<String>,
    #[serde(default)]
    pub lyrics_sources_seen: Vec<String>,
    #[serde(default)]
    pub audio: AudioSettings,
    #[serde(default)]
    pub discord_enabled: bool,
    #[serde(default)]
    pub discord_activity_name: String,
    #[serde(default = "discord_button_defaults")]
    pub discord_buttons: [bool; 2],
    /// Upstream `crossfade_seconds`: 0 (off) to 12, blended equal-power between tracks.
    #[serde(default)]
    pub crossfade_seconds: u8,
    /// Upstream `keepRunningWhenClosed`: closing the window hides it to the tray instead.
    #[serde(default)]
    pub close_to_tray: bool,
    /// Lyrics translation destination tag; blank follows the Windows display language.
    #[serde(default)]
    pub translation_language: String,
    /// One of `services::DOWNLOAD_FORMATS`; unknown values fall back to `best`.
    #[serde(default = "download_format_default")]
    pub download_format: String,
}

fn download_format_default() -> String {
    "best".into()
}

fn discord_button_defaults() -> [bool; 2] {
    [true, true]
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct AudioSettings {
    pub equalizer_enabled: bool,
    #[serde(deserialize_with = "deserialize_audio_bands")]
    pub equalizer_bands: Vec<f64>,
    #[serde(deserialize_with = "deserialize_audio_number")]
    pub stereo_balance: f64,
    pub output_device: String,
    pub wasapi_exclusive: bool,
}

fn audio_number(value: &serde_json::Value) -> f64 {
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|s| s.parse().ok()))
        .unwrap_or(0.0)
}

fn deserialize_audio_number<'de, D: serde::Deserializer<'de>>(d: D) -> Result<f64, D::Error> {
    serde_json::Value::deserialize(d).map(|value| audio_number(&value))
}

fn deserialize_audio_bands<'de, D: serde::Deserializer<'de>>(d: D) -> Result<Vec<f64>, D::Error> {
    serde_json::Value::deserialize(d).map(|value| {
        value
            .as_array()
            .into_iter()
            .flatten()
            .map(audio_number)
            .collect()
    })
}

impl Default for AudioSettings {
    fn default() -> Self {
        Self {
            equalizer_enabled: false,
            equalizer_bands: vec![0.0; 7],
            stereo_balance: 0.0,
            output_device: String::new(),
            wasapi_exclusive: false,
        }
    }
}

impl AudioSettings {
    pub fn sanitize(&mut self) {
        self.equalizer_bands = (0..7)
            .map(|i| {
                self.equalizer_bands
                    .get(i)
                    .copied()
                    .filter(|n| n.is_finite())
                    .unwrap_or(0.0)
                    .clamp(-12.0, 12.0)
            })
            .collect();
        self.stereo_balance = if self.stereo_balance.is_finite() {
            self.stereo_balance.clamp(-1.0, 1.0)
        } else {
            0.0
        };
        if self.output_device.len() > 1024 || self.output_device.contains(['\r', '\n', '\0']) {
            self.output_device.clear();
        }
    }
}

pub fn audio_presets() -> &'static [(&'static str, [f64; 7])] {
    &[
        ("Flat", [0., 0., 0., 0., 0., 0., 0.]),
        ("Acoustic", [3., 1.5, 0., 1.5, 2.5, 2., 1.]),
        ("Bass Boost", [6., 4., 1.5, 0., 0., 0., 0.]),
        ("Bass Cut", [-6., -4., -1.5, 0., 0., 0., 0.]),
        ("Vocal", [-3., -1.5, 1., 3.5, 3., 1., -1.]),
        ("Treble Boost", [0., 0., 0., 0., 1.5, 3.5, 5.]),
        ("Treble Cut", [0., 0., 0., 0., -1.5, -3.5, -5.]),
        ("Loudness", [6., 3.5, 0., -1.5, -1., 2., 5.]),
        ("Spoken Word", [-5., -2.5, 1.5, 4., 3.5, 1.5, -2.]),
        ("Electronic", [5., 3., -1., 0., 1., 3., 4.]),
        ("Rock", [4., 2.5, -1., -1.5, 1., 3., 3.5]),
        ("Hip-Hop", [6., 4., 0.5, -1., 0.5, 2., 2.5]),
        ("Jazz", [3., 1.5, 0., 1., 1.5, 2., 2.5]),
        ("Classical", [3., 2., 0., 0., 1., 2.5, 3.]),
        ("Small Speakers", [5., 4., 2., 0.5, 0., -1., -2.]),
        ("Late Night", [3., 1., 0., 1.5, 1., -1., -3.]),
    ]
}

pub fn lyrics_order_default() -> Vec<String> {
    [
        "lrc_red",
        "bini_lyrics",
        "betterlyrics",
        "betterlyrics_portato",
        "paxsenix",
        "paxsenix_spotify",
        "paxsenix_musixmatch",
        "lyrics_plus",
        "unison",
        "youtube_captions",
        "youtube_music",
        "kugou",
        "lrclib",
        "musixmatch",
        "genius",
    ]
    .map(str::to_owned)
    .to_vec()
}
fn lyrics_sources_default() -> Vec<String> {
    lyrics_order_default()
}
fn lyrics_enabled_default() -> bool {
    true
}

const LYRIC_KEY_IDS: &[&str] = &["paxsenix", "musixmatch"];

fn lyric_keys_path() -> Result<PathBuf> {
    let dir = data_path()
        .parent()
        .context("Library data directory unavailable")?
        .join("auth");
    std::fs::create_dir_all(&dir)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700))?;
    }
    Ok(dir.join("lyrics-keys.json"))
}

/// Read whether a protected lyrics-service key is configured without exposing it to the UI.
pub fn key_configured(key_id: &str) -> Result<bool> {
    let id = valid_key_id(key_id)?;
    Ok(read_lyric_keys()?.get(id).is_some_and(|v| !v.is_empty()))
}

/// Save or remove a provider key. Windows uses DPAPI; Unix stores it in an owner-only file.
pub fn save_key(key_id: &str, value: &str) -> Result<()> {
    save_key_at(&lyric_keys_path()?, key_id, value)
}

fn save_key_at(path: &Path, key_id: &str, value: &str) -> Result<()> {
    let id = valid_key_id(key_id)?;
    if value.len() > 16 * 1024 || value.contains(['\r', '\n', '\0']) {
        anyhow::bail!("Invalid provider key");
    }
    let mut keys = read_lyric_keys_at(path)?;
    if value.trim().is_empty() {
        keys.remove(id);
    } else {
        keys.insert(id.to_owned(), value.trim().to_owned());
    }
    let bytes = serde_json::to_vec(&keys)?;
    #[cfg(windows)]
    let bytes = crate::auth::dpapi(true, &bytes)?;
    let temp = path.with_extension("tmp");
    {
        use std::io::Write;
        let mut file = std::fs::File::create(&temp)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            file.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        file.write_all(&bytes)?;
        file.sync_all()?;
    }
    std::fs::rename(&temp, path).context("Cannot commit provider key storage")
}

pub(crate) fn lyric_key(key_id: &str) -> Result<Option<String>> {
    Ok(read_lyric_keys()?.remove(valid_key_id(key_id)?))
}

fn valid_key_id(key_id: &str) -> Result<&'static str> {
    LYRIC_KEY_IDS
        .iter()
        .copied()
        .find(|id| *id == key_id)
        .context("Unknown lyrics provider key")
}

fn read_lyric_keys() -> Result<std::collections::HashMap<String, String>> {
    read_lyric_keys_at(&lyric_keys_path()?)
}

fn read_lyric_keys_at(path: &Path) -> Result<std::collections::HashMap<String, String>> {
    if !path.exists() {
        return Ok(Default::default());
    }
    if std::fs::metadata(path)?.len() > 1024 * 1024 {
        anyhow::bail!("Provider key storage is too large; original file preserved");
    }
    let bytes = std::fs::read(path).context("Cannot read provider key storage")?;
    #[cfg(windows)]
    let bytes = crate::auth::dpapi(false, &bytes).context("Cannot decrypt provider key storage")?;
    let keys: std::collections::HashMap<String, String> = serde_json::from_slice(&bytes)
        .context("Invalid provider key storage; original file preserved")?;
    if keys.keys().any(|id| !LYRIC_KEY_IDS.contains(&id.as_str())) {
        anyhow::bail!("Invalid provider key storage; original file preserved");
    }
    Ok(keys)
}

impl Default for Library {
    fn default() -> Self {
        Self {
            liked: vec![],
            local: vec![],
            history: vec![],
            playlists: vec![],
            queue: vec![],
            volume: 70.0,
            speed: 1.0,
            autoplay: true,
            lyrics_enabled: true,
            lyrics_order: lyrics_order_default(),
            lyrics_sources: lyrics_sources_default(),
            lyrics_sources_seen: lyrics_order_default(),
            audio: AudioSettings::default(),
            discord_enabled: false,
            discord_activity_name: String::new(),
            discord_buttons: discord_button_defaults(),
            crossfade_seconds: 0,
            close_to_tray: false,
            translation_language: String::new(),
            download_format: download_format_default(),
            download_dir: directories::UserDirs::new()
                .and_then(|d| d.audio_dir().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("."))
                .join("YTDugem"),
        }
    }
}

pub fn data_path() -> PathBuf {
    if let Some(path) = std::env::var_os("BITCHORD_DATA_DIR") {
        return PathBuf::from(path).join("library.json");
    }
    // ponytail: retain the legacy data location so renaming never loses a library or sign-in.
    directories::ProjectDirs::from("in", "BitChord", "BitChord")
        .expect("Home directory unavailable")
        .data_dir()
        .join("library.json")
}

pub fn lock(path: &Path) -> Result<std::fs::File> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let file = std::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .truncate(false)
        .open(path.with_extension("lock"))?;
    file.try_lock()
        .context("YTDugem is already running, or its library cannot be locked")?;
    Ok(file)
}

impl Library {
    pub fn load(path: &Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let bytes = std::fs::read(path).context("Cannot read library")?;
        let mut value: Self =
            serde_json::from_slice(&bytes).context("Invalid library; original file preserved")?;
        // Enable sources first offered by this build once, like the desktop settings store.
        let offered = lyrics_order_default();
        for id in offered {
            if !value.lyrics_sources_seen.contains(&id) {
                if !value.lyrics_sources.contains(&id) {
                    value.lyrics_sources.push(id.clone());
                }
                value.lyrics_sources_seen.push(id);
            }
        }
        value.volume = if value.volume.is_finite() {
            value.volume.clamp(0.0, 100.0)
        } else {
            70.0
        };
        value.speed = if value.speed.is_finite() {
            value.speed.clamp(0.5, 2.0)
        } else {
            1.0
        };
        value.audio.sanitize();
        assign_playlist_ids(&mut value.playlists);
        Ok(value)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let temp = path.with_extension("json.tmp");
        let mut file = std::fs::File::create(&temp)?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec_pretty(self)?)?;
        file.sync_all()?;
        // Keep a last-known-good copy; rename replaces files atomically on Windows too.
        if path.exists() {
            std::fs::copy(path, path.with_extension("json.bak"))?;
        }
        std::fs::rename(temp, path).context("Cannot commit library")
    }

    pub fn remember(&mut self, track: &Track) {
        self.history.retain(|t| t.key() != track.key());
        self.history.insert(0, track.clone());
        self.history.truncate(100);
    }

    /// Create a playlist, optionally seeded with one track, and return its index.
    pub fn create_playlist(&mut self, name: &str, seed: Option<Track>) -> Result<usize> {
        let name = clean_playlist_name(name)?;
        let id = new_playlist_id(&self.playlists);
        self.playlists.push(Playlist {
            id,
            name,
            tracks: seed.into_iter().collect(),
        });
        Ok(self.playlists.len() - 1)
    }

    pub fn rename_playlist(&mut self, index: usize, name: &str) -> Result<()> {
        let name = clean_playlist_name(name)?;
        let playlist = self
            .playlists
            .get_mut(index)
            .context("Playlist not found")?;
        playlist.name = name;
        Ok(())
    }

    pub fn delete_playlist(&mut self, index: usize) -> Option<Playlist> {
        (index < self.playlists.len()).then(|| self.playlists.remove(index))
    }

    /// Returns false for an invalid index or when a track with the same key is already present.
    pub fn add_to_playlist(&mut self, index: usize, track: Track) -> bool {
        let Some(playlist) = self.playlists.get_mut(index) else {
            return false;
        };
        if playlist.tracks.iter().any(|t| t.key() == track.key()) {
            return false;
        }
        playlist.tracks.push(track);
        true
    }

    pub fn remove_from_playlist(&mut self, index: usize, key: &str) -> bool {
        let Some(playlist) = self.playlists.get_mut(index) else {
            return false;
        };
        match playlist.tracks.iter().position(|t| t.key() == key) {
            Some(position) => {
                playlist.tracks.remove(position);
                true
            }
            None => false,
        }
    }

    /// Moves the track at `from` so it ends up at `to`; false if either position is out of range.
    pub fn move_in_playlist(&mut self, index: usize, from: usize, to: usize) -> bool {
        let Some(playlist) = self.playlists.get_mut(index) else {
            return false;
        };
        let len = playlist.tracks.len();
        if from >= len || to >= len {
            return false;
        }
        let track = playlist.tracks.remove(from);
        playlist.tracks.insert(to, track);
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn library_roundtrip_and_corruption_is_not_silently_discarded() {
        let dir = std::env::temp_dir().join(format!("bitchord-store-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.json");
        let guard = lock(&path).unwrap();
        assert!(
            lock(&path).is_err(),
            "two app instances must not overwrite each other's library"
        );
        let t = Track {
            id: "abc".into(),
            title: "A song".into(),
            ..Default::default()
        };
        let mut library = Library::default();
        library.liked.push(t.clone());
        library.remember(&t);
        library.remember(&t);
        library.save(&path).unwrap();
        let read = Library::load(&path).unwrap();
        assert_eq!(read.liked, vec![t]);
        assert_eq!(read.history.len(), 1);
        library.save(&path).unwrap();
        assert!(path.with_extension("json.bak").exists());
        std::fs::write(&path, "broken").unwrap();
        assert!(Library::load(&path).is_err());
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "broken");
        drop(guard);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn lyrics_preferences_roundtrip_and_old_libraries_get_defaults() {
        let old: Library = serde_json::from_str(r#"{"volume":70.0}"#).unwrap();
        assert!(old.lyrics_enabled);
        assert_eq!(old.lyrics_order, lyrics_order_default());
        assert!(!old.discord_enabled);
        assert_eq!(old.discord_buttons, [true, true]);
        let dir =
            std::env::temp_dir().join(format!("bitchord-lyrics-migration-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.json");
        std::fs::write(
            &path,
            r#"{"lyrics_sources":["betterlyrics","youtube_music","lrclib","unison"]}"#,
        )
        .unwrap();
        let prior = Library::load(&path).unwrap();
        assert!(prior.lyrics_sources.contains(&"lrc_red".to_owned()));
        assert_eq!(prior.lyrics_sources_seen, lyrics_order_default());
        std::fs::remove_dir_all(dir).unwrap();
        let mut configured = old;
        configured.lyrics_enabled = false;
        configured.lyrics_order.reverse();
        configured.lyrics_sources = vec!["lrclib".into()];
        configured.discord_enabled = true;
        configured.discord_activity_name = "My music".into();
        configured.discord_buttons = [false, true];
        let restored: Library =
            serde_json::from_slice(&serde_json::to_vec(&configured).unwrap()).unwrap();
        assert!(!restored.lyrics_enabled);
        assert_eq!(restored.lyrics_order, configured.lyrics_order);
        assert_eq!(restored.lyrics_sources, ["lrclib"]);
        assert!(restored.discord_enabled);
        assert_eq!(restored.discord_activity_name, "My music");
        assert_eq!(restored.discord_buttons, [false, true]);
    }

    #[test]
    fn audio_settings_are_backward_compatible_sanitized_and_persistent() {
        let mut old: Library = serde_json::from_str(r#"{"volume":70.0}"#).unwrap();
        assert_eq!(old.audio.equalizer_bands, vec![0.0; 7]);
        old.audio.equalizer_enabled = true;
        old.audio.equalizer_bands = vec![14.0, f64::NAN, -20.0];
        old.audio.stereo_balance = f64::INFINITY;
        old.audio.output_device = "wasapi/{device}".into();
        old.audio.sanitize();
        assert_eq!(
            old.audio.equalizer_bands,
            [12.0, 0.0, -12.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(old.audio.stereo_balance, 0.0);
        let restored: Library = serde_json::from_slice(&serde_json::to_vec(&old).unwrap()).unwrap();
        assert!(restored.audio.equalizer_enabled);
        assert_eq!(restored.audio.output_device, "wasapi/{device}");
        assert_eq!(restored.audio.equalizer_bands, old.audio.equalizer_bands);
        assert_eq!(audio_presets().len(), 16);
        let path = std::env::temp_dir().join(format!(
            "bitchord-audio-sanitize-{}.json",
            std::process::id()
        ));
        std::fs::write(
            &path,
            r#"{"audio":{"equalizer_bands":["NaN",99,-99],"stereo_balance":"Infinity"}}"#,
        )
        .unwrap();
        let malformed = Library::load(&path).unwrap();
        assert_eq!(
            malformed.audio.equalizer_bands,
            [0.0, 12.0, -12.0, 0.0, 0.0, 0.0, 0.0]
        );
        assert_eq!(malformed.audio.stereo_balance, 0.0);
        std::fs::remove_file(path).unwrap();
    }

    fn test_track(id: &str) -> Track {
        Track {
            id: id.into(),
            title: format!("Track {id}"),
            ..Default::default()
        }
    }

    #[test]
    fn local_playlist_edits_validate_names_indexes_and_duplicates() {
        let mut library = Library::default();
        assert!(library.create_playlist("   ", None).is_err());
        assert!(library.create_playlist("", None).is_err());
        let first = library
            .create_playlist("  Mix  ", Some(test_track("a")))
            .unwrap();
        let second = library.create_playlist("Other", None).unwrap();
        assert_eq!((first, second), (0, 1));
        assert_eq!(library.playlists[0].name, "Mix");
        assert_eq!(library.playlists[0].tracks.len(), 1);
        assert_ne!(library.playlists[0].id, library.playlists[1].id);
        assert!(!library.playlists[0].id.is_empty());

        assert!(library.rename_playlist(0, "  Renamed ").is_ok());
        assert_eq!(library.playlists[0].name, "Renamed");
        assert!(library.rename_playlist(0, " \t ").is_err());
        assert_eq!(library.playlists[0].name, "Renamed");
        assert!(library.rename_playlist(9, "Nope").is_err());

        assert!(library.add_to_playlist(1, test_track("b")));
        assert!(library.add_to_playlist(1, test_track("c")));
        assert!(
            !library.add_to_playlist(1, test_track("b")),
            "dedupe by key"
        );
        assert!(!library.add_to_playlist(9, test_track("d")));
        assert_eq!(library.playlists[1].tracks.len(), 2);

        assert!(library.move_in_playlist(1, 0, 1));
        let keys: Vec<&str> = library.playlists[1]
            .tracks
            .iter()
            .map(|t| t.key())
            .collect();
        assert_eq!(keys, ["c", "b"]);
        assert!(!library.move_in_playlist(1, 0, 2), "target out of range");
        assert!(!library.move_in_playlist(1, 2, 0), "source out of range");
        assert!(!library.move_in_playlist(9, 0, 0));

        assert!(library.remove_from_playlist(1, "c"));
        assert!(!library.remove_from_playlist(1, "c"));
        assert!(!library.remove_from_playlist(9, "b"));
        assert_eq!(library.playlists[1].tracks, vec![test_track("b")]);

        assert!(library.delete_playlist(9).is_none());
        let removed = library.delete_playlist(0).unwrap();
        assert_eq!(removed.name, "Renamed");
        assert_eq!(library.playlists.len(), 1);
        assert_eq!(library.playlists[0].name, "Other");
    }

    #[test]
    fn old_playlists_without_ids_load_with_unique_ids() {
        let dir =
            std::env::temp_dir().join(format!("bitchord-playlist-ids-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("library.json");
        std::fs::write(
            &path,
            r#"{"playlists":[{"name":"Old","tracks":[]},{"name":"Older","tracks":[]},{"id":"keep","name":"Kept","tracks":[]},{"id":"keep","name":"Dup","tracks":[]}]}"#,
        )
        .unwrap();
        let loaded = Library::load(&path).unwrap();
        std::fs::remove_dir_all(dir).unwrap();
        let ids: Vec<&str> = loaded.playlists.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(loaded.playlists[0].name, "Old");
        assert_eq!(ids[2], "keep");
        assert!(ids.iter().all(|id| !id.is_empty()));
        let unique: std::collections::HashSet<&str> = ids.iter().copied().collect();
        assert_eq!(unique.len(), ids.len(), "ids must be unique: {ids:?}");
    }

    #[test]
    fn lyric_keys_are_private_and_corruption_is_not_overwritten() {
        let dir = std::env::temp_dir().join(format!("bitchord-keys-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("lyrics-keys.json");
        save_key_at(&path, "paxsenix", "secret-key-123").unwrap();
        assert_eq!(
            read_lyric_keys_at(&path).unwrap()["paxsenix"],
            "secret-key-123"
        );
        #[cfg(windows)]
        {
            assert!(!std::fs::read(&path)
                .unwrap()
                .windows(14)
                .any(|w| w == b"secret-key-123"));
            let mut permissions = std::fs::metadata(&path).unwrap().permissions();
            permissions.set_readonly(true);
            std::fs::set_permissions(&path, permissions.clone()).unwrap();
            let failed_commit = save_key_at(&path, "paxsenix", "replacement");
            permissions.set_readonly(false);
            std::fs::set_permissions(&path, permissions).unwrap();
            assert!(failed_commit.is_err());
            assert_eq!(
                read_lyric_keys_at(&path).unwrap()["paxsenix"],
                "secret-key-123"
            );
        }
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
        save_key_at(&path, "paxsenix", "").unwrap();
        assert!(!read_lyric_keys_at(&path).unwrap().contains_key("paxsenix"));
        std::fs::write(&path, b"corrupt").unwrap();
        assert!(save_key_at(&path, "paxsenix", "replacement").is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"corrupt");
        std::fs::remove_dir_all(dir).unwrap();
    }
}
