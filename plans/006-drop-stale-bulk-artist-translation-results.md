# Plan 006: Late playlist, artist and translation results are dropped when they no longer apply

> **Executor instructions**: Follow this plan step by step. Run every
> verification command and confirm the expected result before moving to the
> next step. If anything in the "STOP conditions" section occurs, stop and
> report — do not improvise. When done, update the status row for this plan
> in `plans/README.md` — unless a reviewer dispatched you and told you they
> maintain the index.
>
> **Drift check (run first)**: `git diff --stat c6cbd32..HEAD -- src/ui.rs src/ui/artist.rs src/ui/sheet_lyrics.rs`
> Plans 004/005 add code to `src/ui.rs` (tests, save fields) — that is expected. Any change to
> the excerpts below is a STOP condition.

## Status

- **Priority**: P2
- **Effort**: S
- **Risk**: LOW
- **Depends on**: none (if plans 004/005 are in flight in the same checkout, run this after them to avoid conflicts in `src/ui.rs`)
- **Category**: bug
- **Planned at**: commit `c6cbd32`, 2026-11-21

## Why this matters

YTDugem does network work on background threads and sends results back as `Event`s, which `fn events` applies on the GUI thread. The project rule (docs/PROJECT_GUIDELINES.md) is: "Discard stale results after track/account/request changes." Three result kinds break it:

1. **Playlist bulk actions** (`Play` / `Shuffle` / `Queue` / `Download` all on a long playlist) carry no request id. If the user clicks Play on a big playlist and then plays another song, the late result **replaces the queue and the song now playing**. After an account switch, a private playlist from the old account still plays or downloads.
2. **Artist pages** fetched under account A are cached after switching to account B (showing A's subscription state and personalized shelves for the rest of the session).
3. **Lyrics translation**: changing the target language while a translation is running labels the old-language result as the new language, and it is never re-translated for that song.

Each fix is a small guard following patterns already used elsewhere in the file (request counters, `account_epoch`).

## Current state

Files:
- `src/ui.rs` — `enum Event` (~100-200), `bulk` (~1781-1799), `apply_bulk` (~1800-1828), `invalidate_account_results` (~1167-1199), the translation-language combo (~2928-2941), `fn events` (from ~3981; its last arm is a catch-all `_ => {}` at ~4339, so guarded arms that don't match are silently dropped).
- `src/ui/artist.rs` — `load_artist` / `artist_loaded` (~538-554), `Artists::reset` (~43-48).
- `src/ui/sheet_lyrics.rs` — `translation_target` (~160) and `request_translation` (~168-195).

Bulk today (`src/ui.rs`):

```rust
    BulkTracks(PlaylistBulk, String, Result<Vec<Track>, String>),            // enum Event, ~163
...
    fn bulk(&mut self, action: PlaylistBulk, origin: &str, tracks: Vec<Track>, browse_id: Option<String>) {
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
...
                Event::BulkTracks(action, origin, r) => {                          // fn events, ~4010
                    self.bulk_loading = false;
                    match r {
                        Ok(tracks) => self.apply_bulk(action, tracks, &origin),
                        Err(e) => self.message = e,
                    }
                }
```

`enum PlaylistBulk { Play, Shuffle, Queue, Download }` is `#[derive(Clone, Copy)]` (no `PartialEq`; use `matches!`). `fn play` increments `self.play_request` on every playback start (`src/ui.rs:817-818`), so a changed `play_request` means the user started something else.

Account switch (`src/ui.rs` ~1167): `invalidate_account_results` bumps `self.account_epoch` and many `*_request` counters, calls `self.artists.reset()`, but does **not** touch `bulk_loading` or any bulk counter.

Artist today:

```rust
    Artist(String, Result<services::artist::ArtistPage, String>),               // enum Event, ~196
...
                Event::Artist(id, r) => self.artist_loaded(id, r),                 // fn events, ~4124
...
    // src/ui/artist.rs
    fn load_artist(&mut self, browse_id: String) {
        self.artists.pages.insert(browse_id.clone(), Loadable::Loading);
        let tx = self.tx.clone();
        std::thread::spawn(move || {
            let result = services::artist::artist_page(&browse_id).map_err(err);
            let _ = tx.send(Event::Artist(browse_id, result));
        });
    }
    pub(super) fn artist_loaded(&mut self, browse_id: String, result: Result<ArtistPage, String>) {
        let state = match result { Ok(page) => Loadable::Ready(page), Err(e) => Loadable::Failed(e) };
        self.artists.pages.insert(browse_id, state);
    }
```

After `artists.reset()` clears `pages`, `artist_view` sees `None` for the open page and calls `load_artist` again (`src/ui/artist.rs` ~742), so dropping a stale result never leaves a page stuck.

Translation today (`src/ui.rs` ~2935):

```rust
                            if chosen != current {
                                self.library.translation_language = chosen;
                                self.save();
                                if self.lyrics_mode == LyricsMode::Translated {
                                    self.request_translation();
                                }
                            }
```

`request_translation` returns early while `self.translating` is true, so `translation_request` isn't bumped and the in-flight result (for the old language) is accepted by `Event::Translated(id, r) if id == self.translation_request` and stored with `self.translation_target()` — the *new* language.

Pattern to copy: request-id guards in `fn events`, e.g. `Event::Moods(id, r) if id == self.explore.request => self.moods_loaded(r),` and epoch capture `let (epoch, tx) = (self.account_epoch, self.tx.clone());` (`src/ui.rs:998`).

## Commands you will need

| Purpose | Command | Expected on success |
|---|---|---|
| Format check | `cargo fmt --all --check` | exit 0 |
| Lint | `cargo clippy --workspace --locked -- -D warnings` | exit 0 |
| New tests | `cargo test --locked stale_result_tests` | all pass |
| Full suite | `cargo test --workspace --locked` | all pass |

If `cargo test` fails only in `ui_tests` snapshot image comparisons, rerun with `CI=1` and mention it.

## Scope

**In scope**:
- `src/ui.rs` — `Event::BulkTracks` / `Event::Artist` shapes, `bulk`, the two event arms, `invalidate_account_results`, a new `bulk_request: u64` field (+ `with_parts` init), the translation-language change, new `mod stale_result_tests`
- `src/ui/artist.rs` — `load_artist`, `artist_loaded`
- `plans/README.md` (status row)

**Out of scope**:
- Other events (`ArtistGrid`, `ArtistLookup`, `ArtistQueue` already have request counters bumped by `Artists::reset`).
- Splitting `src/ui.rs` or centralizing request counters — a separate, larger refactor.
- `src/ui/sheet_lyrics.rs` — no change needed there.

## Git workflow

- Branch: `advisor/006-drop-stale-results`
- One commit per fix is fine, e.g. `fix(ui): ignore a playlist's tracks that arrive after other playback started`, `fix(artist): drop artist pages fetched under another account`, `fix(lyrics): drop a translation for the previous language`.
- No `Co-Authored-By` or "Generated with" trailers.
- Do NOT push or open a PR unless instructed.

## Steps

### Step 1: Guard bulk results

1. Add a field `bulk_request: u64` to `struct YTDugem` next to `bulk_loading` (doc comment: "Tags a playlist fetch for a bulk action; bumped to drop it"), init `0` in `with_parts`.
2. Change the variant to `BulkTracks(u64, u64, PlaylistBulk, String, Result<Vec<Track>, String>)` with a doc comment: "(request, `play_request` when asked, action, origin, tracks)".
3. In `bulk`, before spawning: `self.bulk_request += 1; let (request, played) = (self.bulk_request, self.play_request);` and send `Event::BulkTracks(request, played, action, origin, r)`.
4. Replace the arm:

```rust
                Event::BulkTracks(id, played, action, origin, r) if id == self.bulk_request => {
                    self.bulk_loading = false;
                    match r {
                        // The user started something else while the playlist loaded.
                        Ok(_) if matches!(action, PlaylistBulk::Play | PlaylistBulk::Shuffle)
                            && played != self.play_request => {}
                        Ok(tracks) => self.apply_bulk(action, tracks, &origin),
                        Err(e) => self.message = e,
                    }
                }
```

5. In `invalidate_account_results`, add `self.bulk_request += 1; self.bulk_loading = false;`.

**Verify**: `cargo clippy --workspace --locked -- -D warnings` → exit 0.

### Step 2: Guard artist pages by account epoch

1. Change the variant to `Artist(u64, String, Result<services::artist::ArtistPage, String>)` (doc: "(account epoch when asked, browse id, page)").
2. In `load_artist`, capture `let epoch = self.account_epoch;` and send `Event::Artist(epoch, browse_id, result)`.
3. `artist_loaded(&mut self, epoch: u64, browse_id: String, result: …)`: first line `if epoch != self.account_epoch { return; }` with a comment "Fetched under another account; the open page asks again."
4. Event arm: `Event::Artist(epoch, id, r) => self.artist_loaded(epoch, id, r),`.

`account_epoch` is a private field of `YTDugem` in `src/ui.rs`; `src/ui/artist.rs` is a child module (`use super::*`), so it can read it.

**Verify**: `cargo clippy --workspace --locked -- -D warnings` → exit 0.

### Step 3: Drop a translation for the previous language

Extract the body of the `if chosen != current { … }` block into a method on `YTDugem` in `src/ui.rs`:

```rust
    /// A translation still running is for the previous language: drop it, then ask again.
    fn set_translation_language(&mut self, language: String) {
        self.library.translation_language = language;
        self.save();
        self.translation_request += 1;
        self.translating = false;
        if self.lyrics_mode == LyricsMode::Translated {
            self.request_translation();
        }
    }
```

and call `self.set_translation_language(chosen);` in the combo.

**Verify**: `cargo clippy --workspace --locked -- -D warnings` → exit 0.

### Step 4: Tests

Add `#[cfg(test)] mod stale_result_tests { use super::*; … }` at the end of `src/ui.rs`. Build the app read-only so nothing is written (copy the `app()` helper idea from `src/ui/ui_tests.rs:76-95`):

```rust
    fn app() -> YTDugem {
        let path = std::env::temp_dir().join("ytdugem-stale-tests-no-data").join("library.json");
        let stats = stats_page::Stats::load(&path);
        YTDugem::with_parts(Library::default(), path, true, String::new(), None, stats, Integrations::new())
    }
    fn deliver(app: &mut YTDugem, event: Event) {
        app.tx.send(event).unwrap();
        app.events(&egui::Context::default());
    }
```

Tests (do **not** deliver a non-stale `Play`/`Shuffle`: that would start mpv):
1. `a_bulk_result_from_an_old_request_is_ignored` — `app.bulk_request = 2;` deliver `BulkTracks(1, app.play_request, PlaylistBulk::Queue, "x".into(), Ok(vec![Track::default()]))` → `app.library.queue.is_empty()`. Deliver the same with id `2` → `app.library.queue.len() == 1`.
2. `a_bulk_play_after_other_playback_is_ignored` — `app.bulk_request = 1; app.bulk_loading = true; let played = app.play_request; app.play_request += 1;` deliver `BulkTracks(1, played, PlaylistBulk::Play, "x".into(), Ok(vec![Track::default()]))` → `app.current.is_none()`, `app.library.queue.is_empty()`, `!app.bulk_loading`.
3. `an_account_switch_drops_bulk_and_artist_results` — record `let req = app.bulk_request; let epoch = app.account_epoch;` call `app.invalidate_account_results();` deliver `BulkTracks(req, app.play_request, PlaylistBulk::Queue, …, Ok(vec![Track::default()]))` → queue empty; deliver `Event::Artist(epoch, "UCold".into(), Err("x".into()))` → `!app.artists.pages.contains_key("UCold")`; deliver `Event::Artist(app.account_epoch, "UCnew".into(), Err("x".into()))` → `app.artists.pages.contains_key("UCnew")`.
4. `changing_the_language_drops_a_running_translation` — `app.translation_request = 5; app.translating = true;` call `app.set_translation_language("de".into());` → `app.translation_request == 6`, `!app.translating`; deliver `Event::Translated(5, Err("late".into()))` → `app.message.is_empty()` (a stale result would have set "Translation unavailable: late").

`Artists::pages` is private to `src/ui/artist.rs` (`pages: HashMap<String, Loadable<ArtistPage>>,` at ~line 26), so the test module in `src/ui.rs` can't read it. Change it to `pub(super) pages: …` — that is in scope and the only visibility change allowed.

**Verify**: `cargo test --locked stale_result_tests` → 4 passed. `cargo test --workspace --locked` → all pass.

## Test plan

- 4 new unit tests in `mod stale_result_tests` (no network, no mpv, read-only library).
- Existing ui and snapshot tests unchanged.

## Done criteria

- [ ] `cargo fmt --all --check` exits 0
- [ ] `cargo clippy --workspace --locked -- -D warnings` exits 0
- [ ] `cargo test --workspace --locked` exits 0, including 4 new tests
- [ ] `grep -n "Event::BulkTracks(" src/ui.rs` shows the send with two ids and a guarded arm (`if id == self.bulk_request`)
- [ ] `grep -n "bulk_request += 1" src/ui.rs` shows two lines (in `bulk` and `invalidate_account_results`)
- [ ] `git status` shows only in-scope files modified

## STOP conditions

- `fn events` no longer ends with a catch-all `_ => {}` arm (a guarded arm would then fail to compile or panic).
- `play()` no longer increments `play_request` on every playback start.
- Calling `app.events(&egui::Context::default())` in a test tries to start mpv or touch the network — report it rather than mocking.
- The excerpts above differ from the live code beyond what plans 004/005 added.

## Maintenance notes

- Every new background result type must carry a request id or `account_epoch` and be guarded in `fn events`; `invalidate_account_results` must bump it if it depends on the account. Consider this when reviewing new `Event` variants.
- `Queue` and `Download` bulk results still apply after the user plays something else (intentionally — they don't hijack playback); only an account switch drops them.
- Deferred: grouping request counters per feature (part of the `ui.rs` split finding).
