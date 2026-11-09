# Releasing YTDugem

YTDugem uses one branch, `main`. A **release** is a tagged commit on `main`. There are no
long-lived `stable` or `nightly` branches:

| Channel | Where it comes from | Who gets it |
|---|---|---|
| Stable | Tag `vX.Y.Z` → GitHub release | Everyone. The in-app update check offers it. |
| Pre-release (beta) | Tag `vX.Y.Z-beta1` → GitHub pre-release | Only people who download it by hand. The update check ignores it. |
| Development build | Every push to `main` → `YTDugem-windows-x86_64` artifact of the **Rust desktop** workflow | Testers. Artifacts expire; they are not releases. |

## One version, from one place

- `Cargo.toml` `version` is the app's version. The app reads it at build time
  (`env!("CARGO_PKG_VERSION")`) and shows it under **Settings → About**.
- The tag must be `v` + that version. The release workflow fails if they differ.
- Use [SemVer](https://semver.org): `0.2.0`, `0.2.1`, `1.0.0`; pre-releases add a dash suffix,
  `0.3.0-beta1`. The update check compares numbers part by part and treats `0.3.0-beta1` as older
  than `0.3.0` (`src/update.rs` `is_newer`).

## Cutting a release

1. Make sure `main` is green (the **Rust desktop** workflow) and run the Windows checks from
   `AGENTS.md` on the packaged ZIP if runtime, IPC, playback or packaging changed.
2. Bump the version on a branch and merge it:

   ```sh
   # edit Cargo.toml: version = "0.2.0"
   cargo check            # updates Cargo.lock
   git commit -am "chore: release 0.2.0"
   ```

3. Tag the merged commit on `main` and push the tag:

   ```sh
   git switch main && git pull
   git tag -a v0.2.0 -m "YTDugem 0.2.0"
   git push origin v0.2.0
   ```

4. `.github/workflows/release.yml` runs on the tag. It checks the version, then runs fmt, clippy,
   the tests, the release build, the packager and the offline playback smoke test. It then
   publishes a GitHub release with generated notes and these files:
   - `YTDugem-windows-x86_64-setup.exe`: the installer
   - `YTDugem-windows-x86_64.zip`: the portable package
   - a `.sha256` for each (`<hash>  <file>`, as `sha256sum` prints it)

   It also builds the installer and test-installs it silently: install, start, uninstall.
5. Edit the release notes on GitHub if you want. The update dialog shows them.

If the workflow fails, fix the problem on `main`, delete the tag (`git push origin :v0.2.0`,
`git tag -d v0.2.0`), and tag again. Do not reuse a version that has already been published.

## Last.fm API key

Last.fm scrobbling needs a Last.fm API account (key and shared secret, from
https://www.last.fm/api/account/create). They are **never committed**. The app reads them at build
time with `option_env!("BITCHORD_LASTFM_API_KEY")` and `option_env!("BITCHORD_LASTFM_API_SECRET")`,
and falls back to the same environment variables at run time.

Release builds need them as repository secrets `BITCHORD_LASTFM_API_KEY` and
`BITCHORD_LASTFM_API_SECRET` (Settings → Secrets and variables → Actions). The release build step in
`release.yml` and `check.yml` passes them to `cargo build --release`. When they are missing (forks,
pull requests from forks) the build still succeeds; that build shows "This build has no Last.fm API
key" and only ListenBrainz works. A key compiled into a distributed `.exe` can be extracted from it,
as with upstream's Android builds, so use a key dedicated to YTDugem.

## The installer

`packaging/windows/ytdugem.iss` (Inno Setup 6 or later) is built by
`scripts/build-installer.ps1`. That script extracts the portable ZIP and packages exactly those files,
so build the ZIP first. Locally:

```powershell
cargo build --release --locked
python scripts/package-windows.py
powershell -NoProfile -File scripts/build-installer.ps1 -Test
```

- It installs per user into `%LOCALAPPDATA%\Programs\YTDugem`. There is no admin prompt and no
  folder choice. The folder stays writable, so the in-app updater works on installed copies too.
- **Never change `AppId`.** Upgrades, the uninstaller and `src/update.rs` (`INSTALL_KEY`, which
  updates the version shown in Apps & features after an in-app update) all depend on it.
- Running a newer setup over an installed copy upgrades it in place.
- The installer is unsigned, so Windows SmartScreen warns on first run until the app gains
  reputation or the files are code-signed.

## Rules the update check depends on

`src/update.rs` reads `https://api.github.com/repos/TazakiN/YTDugem/releases/latest`:

- GitHub's "latest" skips drafts and pre-releases. A release prompts users only once it is
  published as a normal release.
- The asset names above are matched exactly. Renaming them in `release.yml` or
  `scripts/package-windows.py` makes the dialog fall back to the release page.
- The ZIP must keep its single top folder `YTDugem/` with `YTDugem.exe`, `tools/mpv.exe` and
  `tools/yt-dlp.exe` inside. The updater refuses a package without them.
- The `.sha256` is uploaded to the same release, so it guards against a damaged or truncated
  download, not against a compromised GitHub account. Signing releases (Authenticode or minisign)
  would be the next step if that matters.

## Testing the update path

To exercise a real update, run an older build against the published release:

1. Extract the ZIP of the previous release (or package a build with a lower `version`) into a
   folder you can write to, and start `YTDugem.exe`.
2. The **Update available** dialog appears. Choose **Update now**, wait for the download, then
   **Restart now**.
3. The app reopens with "YTDugem was updated to X" and **Settings → About** shows the new version.
   The `.update` folder is removed at that start.
