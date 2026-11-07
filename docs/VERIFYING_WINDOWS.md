# Windows verification

## Automated checks

Discord Rich Presence has payload tests and actual Windows named-pipe fake-server tests. These use unique test-only endpoints, not a running Discord client or personal account. Run `cargo test --locked discord:: -- --nocapture` to verify handshake, nonce acknowledgements, ping/PONG, activity/clear, timeout, reconnect, malformed frames, coalescing and shutdown. Real Discord profile rendering remains an interactive check.

From the project root, using Rust with MSVC and Python. `cargo ci`, `cargo xtask package`, and `cargo xtask smoke [--online] [--real-audio]` wrap the same commands:

```powershell
cargo fmt --check
cargo clippy --locked -- -D warnings
cargo test --locked
cargo build --release --locked
python scripts/test-package-windows.py
python scripts/package-windows.py
powershell -NoProfile -File scripts/windows-smoke.ps1
powershell -NoProfile -File scripts/windows-ui-smoke.ps1
powershell -NoProfile -File scripts/windows-playback-smoke.ps1
powershell -NoProfile -File scripts/windows-playback-smoke.ps1 -Online -RealAudio
```

If `python` is the Windows Store alias, use the full path to your installed Python executable. Packaging accepts 7zip (`SEVENZIP` overrides its location), or native Windows `tar` with 7z support. Runtime hashes must match; never bypass a mismatch.

- Startup smoke checks the real visible native window and clean exit.
- UI smoke uses an isolated library and generated local WAV. It checks import/persisted playback history, keyboard sequences, resize survival, and clean exit. Screenshots and JSON reports are saved under ignored `dist/ui-test-results/`. Tab/search screenshots require visual inspection; sending a key is not a semantic UI assertion. The script refuses input unless its own app window is foreground.
- Playback smoke extracts the actual ZIP into a fresh directory containing spaces, restricts `PATH`, and uses the application's real playback engine. Reports go to ignored `dist/playback-test-results/`.
- Default/null output proves decoding and playback/control state, not speakers. `-RealAudio` must report a non-null backend. Only a listener can confirm that sound was heard and assess quality.
- Report the exact source/package checkpoint tested. Rebuild after changing source: the previous ZIP does not contain later edits.

## Discord presence: interactive verification

Discord Rich Presence is disabled in the app (`DISCORD_READY` in `src/ui.rs`) until YTDugem has its own Discord application. Run these steps only after enabling it.

1. Open the **Discord desktop client** on Windows; browser-only Discord has no local IPC pipe. No Discord token is needed or requested.
2. In YTDugem Settings, enable **Discord Rich Presence** (off by default), then play a song. After an acknowledgement, the status should read **Discord: connected**. If Discord is absent it stays waiting; rejected/timed-out requests show an error and retry.
3. Inspect your Discord profile: title, artist, artwork when public HTTPS artwork is available, album tooltip, and the countdown. Test a seek, speed change, next track, and the optional activity name/button settings. Ordinary edits are coalesced to at most one update per two seconds. Discord may control or ignore the application display name, external images or buttons; your own profile may not show your own buttons.
4. Pause, stop, or disable presence: activity should clear. Resume should restore it. Close YTDugem: it attempts a bounded clear and disconnects. Restart Discord during playback: reconnection occurs automatically (up to about 20 seconds without another playback change).
5. Turn off Discord activity privacy/display settings to confirm they can hide activity independently of successful IPC. A connected acknowledgement proves delivery, not that Discord elected to display the activity.

No account-token/gateway fallback is implemented. This local integration uses upstream's application ID and default Listening activity; advanced wording/status/type/custom-button-label controls remain outside this implementation. Local filesystem paths are not used for artwork or listen-button URLs.

## Account flow: interactive user verification required

Automated tests must never operate on a personal browser profile or assume a real Google account. To verify manually:

1. Run the complete portable package. Open its account dialog.
2. Choose automatic sign-in. Verify that a separate browser profile opens, without your usual tabs or cookies.
3. Complete Google sign-in yourself. **Close that isolated browser window** after YouTube Music confirms sign-in. The app then briefly opens that profile headlessly to capture and validate the session.
4. Confirm that the app shows the correct account and that personalized Home/library requests work. A page opening in the default browser alone does **not** connect an account.
5. Close and reopen the app to verify saved-session restoration. Exercise cancellation and sign-out, checking that errors are shown rather than reported as success.
6. If account switching is available in the tested build, add a second account, switch, restart, and remove only one account. Confirm that the device-local library survives and the other account's private feed does not remain visible.

Manual Cookie input is validated against the Music page's signed-in configuration first, like upstream desktop, not solely by whether an account-menu request succeeds. Session index, page/account scope, visitor data and client version come from that page and are used on authenticated requests. Account-menu name/email/photo are optional once a signed-in scope with stable identity is confirmed; a temporary metadata failure can show the generic **YouTube Music** account name. No live sign-in is implied by offline tests: `cargo test --locked auth::tests -- --nocapture` uses dummy values and an isolated loopback HTTP server.

Paste a freshly copied raw Cookie value (optionally prefixed `Cookie:`), without Markdown or manual changes. Unknown names are preserved but never repaired into authentication cookies. JSON/Netscape imports remain supported. Distinguish parsing errors, a signed-out/consent page, and HTTP errors; report only the error text, never your cookie. Treat any cookie shared in chat as exposed and revoke the affected session before testing a replacement privately in the app.

Google may reject browser sign-in or expire a session. Record that limitation. Manual cookie input, if used, must stay in the app's masked field; never paste cookies into chat, issues, screenshots, or logs. Authentication credentials are stored separately from `library.json` using Windows DPAPI.

## Lyrics verification

Use known tracks with accurate title, artist, album, and duration. For each provider actually offered in the tested build:

1. Test explicit provider selection, not only automatic fallback. Confirm the displayed provenance matches the provider that returned the result.
2. Test plain text, line timing, and word timing only where supported. Check click-to-seek and current-line/word highlighting against playback.
3. Change enabled providers and order, restart, and confirm persistence. Disable all providers/global lookup and verify no new automatic request is made.
4. Retry and switch tracks/providers quickly; an older result must not overwrite the newest request.
5. Distinguish missing lyrics (normal miss) from service outage/rate limit (for example, HTTP 503/429). A fallback success does not prove an earlier provider worked.
6. Enter any required API key only in the app's masked setup control. Do not use production secrets in test fixtures or persist them in the ordinary library file.

Live service availability changes independently of parser tests. A failed live provider test must be reported even when every offline fixture passes.

Opt-in keyless smoke (no credentials): `cargo test --locked live_keyless_lyrics_provider_smoke -- --ignored --nocapture`. It prints each provider's actual result and requires at least one success; that does not turn the other printed misses/errors into passes.
