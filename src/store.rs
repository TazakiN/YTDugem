use crate::model::Track;
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Clone, Default, Serialize, Deserialize)]
pub struct Playlist {
    pub name: String,
    pub tracks: Vec<Track>,
}

#[derive(Serialize, Deserialize)]
#[serde(default)]
pub struct Library {
    pub liked: Vec<Track>,
    pub local: Vec<Track>,
    pub history: Vec<Track>,
    pub playlists: Vec<Playlist>,
    pub queue: Vec<Track>,
    pub volume: f64,
    pub speed: f64,
    pub download_dir: PathBuf,
    pub autoplay: bool,
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
            download_dir: directories::UserDirs::new()
                .and_then(|d| d.audio_dir().map(Path::to_path_buf))
                .unwrap_or_else(|| PathBuf::from("."))
                .join("BitChord"),
        }
    }
}

pub fn data_path() -> PathBuf {
    if let Some(path) = std::env::var_os("BITCHORD_DATA_DIR") {
        return PathBuf::from(path).join("library.json");
    }
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
        .context("BitChord is already running, or its library cannot be locked")?;
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
        // Keep a last-known-good copy, including on Windows where rename cannot replace.
        if path.exists() {
            std::fs::copy(path, path.with_extension("json.bak"))?;
        }
        #[cfg(windows)]
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(temp, path).context("Cannot commit library")
    }

    pub fn like(&mut self, track: &Track) {
        if let Some(i) = self.liked.iter().position(|t| t.key() == track.key()) {
            self.liked.remove(i);
        } else {
            self.liked.push(track.clone());
        }
    }

    pub fn remember(&mut self, track: &Track) {
        self.history.retain(|t| t.key() != track.key());
        self.history.insert(0, track.clone());
        self.history.truncate(100);
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
        library.like(&t);
        library.like(&t);
        assert!(library.liked.is_empty());
        library.like(&t);
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
}
