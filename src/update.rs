//! Release check and in-place update of the portable ZIP.
//!
//! Upstream `DesktopUpdateChecker` asks GitHub for the latest release once per launch and opens
//! its installer in the browser. YTDugem ships a portable folder instead of an installer, so it
//! can also update that folder itself: download the release ZIP, verify its SHA-256, extract it
//! beside the app, swap the files and restart. Releases are made by `.github/workflows/release.yml`
//! (see `docs/RELEASING.md`); the asset names below must match it.

use anyhow::{bail, Context, Result};
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs,
    io::{Read, Write},
    path::{Path, PathBuf},
    process::Command,
    time::Duration,
};

pub const CURRENT: &str = env!("CARGO_PKG_VERSION");
/// GitHub's "latest" skips drafts and pre-releases, so only stable releases prompt anyone.
const LATEST: &str = "https://api.github.com/repos/TazakiN/YTDugem/releases/latest";
pub const PACKAGE: &str = "YTDugem-windows-x86_64.zip";
/// Download, extraction and the replaced files, inside the app folder (same volume, so the swap
/// is a rename). Removed at the next start.
const STAGING: &str = ".update";
/// Passed to the restarted app: wait for the old one to release the library lock.
pub const AFTER_UPDATE: &str = "--after-update";

#[derive(Clone, Debug, PartialEq)]
pub struct Release {
    pub version: String,
    /// The release page, for notes and a manual download.
    pub page: String,
    pub notes: String,
    /// The portable ZIP and its `.sha256` file, when the release carries both.
    pub package: Option<(String, String)>,
}

#[derive(Deserialize)]
struct GitHubRelease {
    tag_name: String,
    html_url: String,
    #[serde(default)]
    body: Option<String>,
    #[serde(default)]
    draft: bool,
    #[serde(default)]
    prerelease: bool,
    #[serde(default)]
    assets: Vec<GitHubAsset>,
}

#[derive(Deserialize)]
struct GitHubAsset {
    name: String,
    browser_download_url: String,
    #[serde(default)]
    state: String,
}

fn client(timeout: Duration) -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .timeout(timeout)
        .user_agent(concat!("YTDugem/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

/// The latest release when it is newer than this build; `None` when up to date or none exists.
pub fn check() -> Result<Option<Release>> {
    let response = client(Duration::from_secs(20))?
        .get(LATEST)
        .header("Accept", "application/vnd.github+json")
        .send()
        .context("Could not reach GitHub")?;
    if response.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(None);
    }
    let release: GitHubRelease = response.error_for_status()?.json()?;
    Ok(newer_release(release, CURRENT))
}

fn newer_release(release: GitHubRelease, current: &str) -> Option<Release> {
    if release.draft || release.prerelease {
        return None;
    }
    let version = release.tag_name.trim_start_matches('v').to_owned();
    if !is_newer(&version, current) {
        return None;
    }
    let asset = |name: &str| {
        release
            .assets
            .iter()
            .find(|a| {
                a.name.eq_ignore_ascii_case(name)
                    && a.state == "uploaded"
                    && a.browser_download_url.starts_with("https://github.com/")
            })
            .map(|a| a.browser_download_url.clone())
    };
    let package = asset(PACKAGE).zip(asset(&format!("{PACKAGE}.sha256")));
    Some(Release {
        version,
        page: release.html_url,
        notes: release.body.unwrap_or_default(),
        package,
    })
}

/// Numeric, part by part; a `-beta1` build counts as older than the release it leads up to
/// (upstream `DesktopUpdateChecker.isNewer`).
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |raw: &str| {
        let (base, pre) = match raw.split_once('-') {
            Some((base, _)) => (base, true),
            None => (raw, false),
        };
        let parts: Vec<u64> = base.split('.').map(|p| p.parse().unwrap_or(0)).collect();
        (parts, pre)
    };
    let (l, l_pre) = parse(latest);
    let (c, c_pre) = parse(current);
    for i in 0..l.len().max(c.len()) {
        let (a, b) = (l.get(i).unwrap_or(&0), c.get(i).unwrap_or(&0));
        if a != b {
            return a > b;
        }
    }
    c_pre && !l_pre
}

/// The folder of a portable install this build can replace: `YTDugem.exe` with its `tools`
/// beside it. A build run from `target/`, a renamed exe, or another OS is updated by hand.
pub fn portable_dir() -> Option<PathBuf> {
    if !cfg!(windows) {
        return None;
    }
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let named = exe.file_name()?.eq_ignore_ascii_case("YTDugem.exe");
    (named && dir.join("tools").join("mpv.exe").is_file()).then(|| dir.to_path_buf())
}

/// Downloads and verifies the package into `dir/.update`, then extracts it. Returns the extracted
/// `YTDugem` folder. Nothing outside `.update` is touched.
pub fn download(package: &(String, String), dir: &Path, progress: impl Fn(f32)) -> Result<PathBuf> {
    let staging = dir.join(STAGING);
    let _ = fs::remove_dir_all(&staging);
    fs::create_dir_all(&staging).with_context(|| {
        format!(
            "Cannot write to {}; download the update from its release page instead",
            dir.display()
        )
    })?;
    let client = client(Duration::from_secs(30 * 60))?;
    let sums = client.get(&package.1).send()?.error_for_status()?.text()?;
    let expected = parse_checksum(&sums).context("The release's checksum file is unreadable")?;
    let zip = staging.join(PACKAGE);
    let mut response = client.get(&package.0).send()?.error_for_status()?;
    let total = response.content_length().filter(|&n| n > 0);
    let mut file = fs::File::create(&zip)?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; 64 * 1024];
    let (mut done, mut shown) = (0u64, 0u64);
    loop {
        let n = response.read(&mut buffer)?;
        if n == 0 {
            break;
        }
        hasher.update(&buffer[..n]);
        file.write_all(&buffer[..n])?;
        done += n as u64;
        if let Some(total) = total {
            let percent = done * 100 / total;
            if percent != shown {
                shown = percent;
                progress(done as f32 / total as f32);
            }
        }
    }
    file.sync_all()?;
    drop(file);
    let actual: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();
    if actual != expected {
        bail!("The downloaded update is damaged (SHA-256 mismatch); nothing was changed");
    }
    let extracted = staging.join("new");
    fs::create_dir_all(&extracted)?;
    extract(&zip, &extracted)?;
    let _ = fs::remove_file(&zip);
    let root = extracted.join("YTDugem");
    check_package(&root)?;
    Ok(root)
}

/// The first word of a `sha256sum`-style line, when it is a SHA-256 in hex.
fn parse_checksum(text: &str) -> Option<String> {
    let word = text.split_whitespace().next()?.to_ascii_lowercase();
    (word.len() == 64 && word.bytes().all(|b| b.is_ascii_hexdigit())).then_some(word)
}

fn check_package(root: &Path) -> Result<()> {
    for required in ["YTDugem.exe", "tools/mpv.exe", "tools/yt-dlp.exe"] {
        if !root.join(required).is_file() {
            bail!("The update package is missing {required}; nothing was changed");
        }
    }
    Ok(())
}

/// Windows' own `tar` (bsdtar) reads ZIP and refuses paths that leave the destination.
fn extract(zip: &Path, destination: &Path) -> Result<()> {
    let mut command = Command::new(system_tool("tar.exe"));
    command.arg("-xf").arg(zip).arg("-C").arg(destination);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let output = command
        .output()
        .context("Could not start tar to unpack the update")?;
    if !output.status.success() {
        bail!(
            "Could not unpack the update: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(())
}

/// A program from `System32` by full path: `PATH` starts with the bundled tools folder.
fn system_tool(name: &str) -> PathBuf {
    let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
    PathBuf::from(root).join("System32").join(name)
}

/// Moves each top-level item of the extracted package into `dir`, keeping what it replaces in
/// `.update/old` until the next start. Windows lets a running exe be renamed, not overwritten;
/// a folder with a file in use (a tool still running) cannot be moved at all. On any failure
/// everything moved so far goes back, so the app folder is left as it was.
pub fn apply(dir: &Path, staged: &Path) -> Result<()> {
    check_package(staged)?;
    let old = dir.join(STAGING).join("old");
    let _ = fs::remove_dir_all(&old);
    fs::create_dir_all(&old)?;
    let mut names: Vec<OsString> = fs::read_dir(staged)?
        .map(|entry| entry.map(|e| e.file_name()))
        .collect::<std::io::Result<_>>()?;
    names.sort();
    swap(dir, staged, &old, names)
}

fn swap(dir: &Path, staged: &Path, old: &Path, names: Vec<OsString>) -> Result<()> {
    let mut moved: Vec<(OsString, bool)> = vec![];
    for name in names {
        let current = dir.join(&name);
        let had = current.symlink_metadata().is_ok();
        let step = || -> std::io::Result<()> {
            if had {
                fs::rename(&current, old.join(&name))?;
            }
            if let Err(e) = fs::rename(staged.join(&name), &current) {
                if had {
                    let _ = fs::rename(old.join(&name), &current);
                }
                return Err(e);
            }
            Ok(())
        };
        if let Err(e) = step() {
            for (name, had) in moved.iter().rev() {
                let _ = fs::rename(dir.join(name), staged.join(name));
                if *had {
                    let _ = fs::rename(old.join(name), dir.join(name));
                }
            }
            return Err(e).with_context(|| {
                format!(
                    "Could not replace {}; the app was left unchanged",
                    name.to_string_lossy()
                )
            });
        }
        moved.push((name, had));
    }
    Ok(())
}

/// The installer's uninstall entry (`packaging/windows/ytdugem.iss` AppId, plus Inno's `_is1`).
const INSTALL_KEY: &str = r"HKCU\Software\Microsoft\Windows\CurrentVersion\Uninstall\{C8AB3870-DAD2-4919-849F-1B729CD32A70}_is1";

/// In a copy the setup installed (its uninstaller sits beside the exe), keeps the version shown in
/// Apps & features in step with the files an in-app update swapped in. Best effort.
pub fn record_installed_version(dir: &Path, version: &str) {
    if !cfg!(windows) || version.is_empty() || !dir.join("unins000.exe").is_file() {
        return;
    }
    let mut command = Command::new(system_tool("reg.exe"));
    command.args([
        "add",
        INSTALL_KEY,
        "/v",
        "DisplayVersion",
        "/t",
        "REG_SZ",
        "/d",
        version,
        "/f",
    ]);
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let _ = command.output();
}

/// Starts the updated app, which waits for this one to exit.
pub fn restart(dir: &Path) -> Result<()> {
    Command::new(dir.join("YTDugem.exe"))
        .arg(AFTER_UPDATE)
        .current_dir(dir)
        .spawn()
        .context("Updated, but could not start YTDugem again; open it yourself")?;
    Ok(())
}

/// Removes what the last update left behind. The old exe may still be exiting, so retry briefly.
pub fn cleanup() {
    let Some(staging) = portable_dir().map(|d| d.join(STAGING)) else {
        return;
    };
    if !staging.exists() {
        return;
    }
    std::thread::spawn(move || {
        for _ in 0..20 {
            if fs::remove_dir_all(&staging).is_ok() || !staging.exists() {
                return;
            }
            std::thread::sleep(Duration::from_millis(500));
        }
    });
}

/// Opens a GitHub page in the default browser.
pub fn open_page(url: &str) -> Result<()> {
    if !url.starts_with("https://github.com/") {
        bail!("Refusing to open {url}");
    }
    open_browser(url)
}

/// Opens an already validated `https://` URL in the default browser, as one argument.
pub(crate) fn open_browser(url: &str) -> Result<()> {
    if !url.starts_with("https://") || url.contains(char::is_whitespace) {
        bail!("Refusing to open {url}");
    }
    #[cfg(windows)]
    let mut command = {
        let root = std::env::var_os("SystemRoot").unwrap_or_else(|| "C:\\Windows".into());
        let mut c = Command::new(PathBuf::from(root).join("explorer.exe"));
        c.arg(url);
        c
    };
    #[cfg(not(windows))]
    let mut command = {
        let mut c = Command::new("xdg-open");
        c.arg(url);
        c
    };
    command.spawn().context("Could not open the browser")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn compares_versions_like_upstream() {
        assert!(is_newer("0.2.0", "0.1.0"));
        assert!(is_newer("0.10.0", "0.9.0"));
        assert!(is_newer("1.0", "0.9.9"));
        assert!(is_newer("0.2.0", "0.2.0-beta1"));
        assert!(!is_newer("0.2.0-beta1", "0.2.0"));
        assert!(!is_newer("0.1.0", "0.1.0"));
        assert!(!is_newer("0.1.0", "0.2.0"));
        assert!(!is_newer("1.0.0", "1.0"));
    }

    fn release(tag: &str, assets: &[(&str, &str)]) -> GitHubRelease {
        GitHubRelease {
            tag_name: tag.into(),
            html_url: "https://github.com/TazakiN/YTDugem/releases/tag/x".into(),
            body: Some("notes".into()),
            draft: false,
            prerelease: false,
            assets: assets
                .iter()
                .map(|(name, state)| GitHubAsset {
                    name: (*name).into(),
                    browser_download_url: format!("https://github.com/dl/{name}"),
                    state: (*state).into(),
                })
                .collect(),
        }
    }

    #[test]
    fn picks_the_portable_zip_and_its_checksum() {
        let both = [
            (PACKAGE, "uploaded"),
            ("YTDugem-windows-x86_64.zip.sha256", "uploaded"),
        ];
        let found = newer_release(release("v0.2.0", &both), "0.1.0").unwrap();
        assert_eq!(found.version, "0.2.0");
        assert_eq!(
            found.package,
            Some((
                format!("https://github.com/dl/{PACKAGE}"),
                format!("https://github.com/dl/{PACKAGE}.sha256")
            ))
        );
        // Without a checksum, or mid-upload, only the release page is offered.
        let no_sum = newer_release(release("v0.2.0", &[(PACKAGE, "uploaded")]), "0.1.0");
        assert_eq!(no_sum.unwrap().package, None);
        let partial = [
            (PACKAGE, "new"),
            ("YTDugem-windows-x86_64.zip.sha256", "uploaded"),
        ];
        assert_eq!(
            newer_release(release("v0.2.0", &partial), "0.1.0")
                .unwrap()
                .package,
            None
        );
        assert_eq!(newer_release(release("v0.1.0", &both), "0.1.0"), None);
        let mut pre = release("v0.3.0-beta1", &both);
        pre.prerelease = true;
        assert_eq!(newer_release(pre, "0.1.0"), None);
    }

    #[test]
    fn reads_sha256sum_lines() {
        let hash = "AB".repeat(32);
        assert_eq!(
            parse_checksum(&format!("{hash}  {PACKAGE}\n")),
            Some("ab".repeat(32))
        );
        assert_eq!(parse_checksum("not-a-hash file"), None);
        assert_eq!(parse_checksum(""), None);
    }

    fn package(root: &Path, marker: &str) {
        fs::create_dir_all(root.join("tools")).unwrap();
        for file in ["YTDugem.exe", "tools/mpv.exe", "tools/yt-dlp.exe"] {
            fs::write(root.join(file), marker).unwrap();
        }
    }

    #[test]
    fn swaps_the_app_folder_and_keeps_the_old_one() {
        let root = std::env::temp_dir().join(format!("ytdugem-update-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let app = root.join("app");
        let staged = root.join("staged");
        package(&app, "old");
        fs::write(app.join("user-note.txt"), "kept").unwrap();
        package(&staged, "new");
        fs::write(staged.join("NOTICE"), "new").unwrap();
        apply(&app, &staged).unwrap();
        assert_eq!(fs::read_to_string(app.join("YTDugem.exe")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(app.join("tools/mpv.exe")).unwrap(),
            "new"
        );
        assert_eq!(fs::read_to_string(app.join("NOTICE")).unwrap(), "new");
        assert_eq!(
            fs::read_to_string(app.join("user-note.txt")).unwrap(),
            "kept"
        );
        let old = app.join(STAGING).join("old");
        assert_eq!(fs::read_to_string(old.join("YTDugem.exe")).unwrap(), "old");
        fs::remove_dir_all(&root).unwrap();
    }

    #[test]
    fn a_failed_swap_restores_the_app_folder() {
        let root = std::env::temp_dir().join(format!("ytdugem-rollback-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        let app = root.join("app");
        let staged = root.join("staged");
        let old = root.join("old");
        package(&app, "old");
        package(&staged, "new");
        fs::create_dir_all(&old).unwrap();
        // `missing` fails after the exe and tools have moved, like a tool still in use.
        let names = ["YTDugem.exe", "tools", "missing"]
            .map(OsString::from)
            .to_vec();
        assert!(swap(&app, &staged, &old, names).is_err());
        assert_eq!(fs::read_to_string(app.join("YTDugem.exe")).unwrap(), "old");
        assert_eq!(
            fs::read_to_string(app.join("tools/mpv.exe")).unwrap(),
            "old"
        );
        assert_eq!(
            fs::read_to_string(staged.join("YTDugem.exe")).unwrap(),
            "new"
        );
        assert_eq!(
            fs::read_to_string(staged.join("tools/mpv.exe")).unwrap(),
            "new"
        );
        assert!(fs::read_dir(&old).unwrap().next().is_none());
        fs::remove_dir_all(&root).unwrap();
    }
}
