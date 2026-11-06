#!/usr/bin/env python3
"""Real native-window smoke check. Requires Xvfb, xdotool, ffmpeg on PATH."""
import json
import os
import select
import secrets
from pathlib import Path
import subprocess
import tempfile
import wave

ROOT = Path(__file__).resolve().parents[1]
OUT = Path(os.environ.get("BITCHORD_TEST_OUTPUT") or Path(tempfile.gettempdir()) / "ytdugem-smoke")
OUT.mkdir(parents=True, exist_ok=True)


def check(*args, **kwargs):
    return subprocess.check_output(args, **kwargs).decode().strip()


def screenshot(name, env, size="1320x850"):
    # Software-rendered CI needs time to finish artwork uploads.
    check("ffmpeg", "-hide_banner", "-loglevel", "error", "-y", "-f", "x11grab",
          "-video_size", size, "-framerate", "10", "-i", env["DISPLAY"],
          "-vf", "select=gte(n\\,50)", "-frames:v", "1", "-threads", "1", str(OUT / name), env=env)


with tempfile.TemporaryDirectory(prefix="bitchord-gui-") as data:
    audio = Path(data) / "Smoke tone.wav"
    with wave.open(str(audio), "wb") as stream:
        stream.setnchannels(1)
        stream.setsampwidth(2)
        stream.setframerate(44100)
        stream.writeframes(b"\0\0" * 44100 * 20)
    read_fd, write_fd = os.pipe()
    xlog = (OUT / "xvfb.log").open("w")
    auth = Path(data) / "Xauthority"
    cookie = secrets.token_hex(16)
    check("xauth", "-f", str(auth), "add", ":99", ".", cookie)
    # Use the current display when requested; only our own window receives input.
    existing = os.environ.get("BITCHORD_TEST_DISPLAY")
    xvfb = None if existing else subprocess.Popen([os.environ.get("BITCHORD_XVFB", "Xvfb"), "-displayfd", str(write_fd), "-screen", "0", "1920x1080x24", "-nolisten", "unix", "-nolisten", "local", "-listen", "tcp", "-auth", str(auth)], pass_fds=(write_fd,), stderr=xlog)
    os.close(write_fd)
    if not existing and not select.select([read_fd], [], [], 10)[0]:
        xvfb.terminate()
        xvfb.wait(timeout=5)
        raise RuntimeError("Xvfb did not become ready; inspect " + str(OUT / "xvfb.log"))
    number = "" if existing else os.read(read_fd, 32).decode().strip()
    if not existing and not number: raise RuntimeError("Xvfb failed; inspect xvfb.log")
    display = existing or ("localhost:" + number)
    if not existing: check("xauth", "-f", str(auth), "add", display, ".", cookie)
    os.close(read_fd)
    env = {**os.environ, "DISPLAY": display, "WINIT_UNIX_BACKEND": "x11", "BITCHORD_DATA_DIR": data,
           "BITCHORD_AUDIO_OUTPUT": "null", "LIBGL_ALWAYS_SOFTWARE": "1"}
    env.pop("WAYLAND_DISPLAY", None)
    if not existing: env["XAUTHORITY"] = str(auth)
    log = (OUT / "desktop.log").open("w")
    binary = os.environ.get("BITCHORD_TEST_BINARY", str(ROOT / "target/debug/ytdugem"))
    app = subprocess.Popen([binary], env=env, stdout=log, stderr=log)
    window = None
    try:
        window = check("xdotool", "search", "--sync", "--onlyvisible", "--name", "^YTDugem$", env=env, timeout=30).splitlines()[0]
        geometry = check("xdotool", "getwindowgeometry", "--shell", window, env=env)
        print("Native window:\n" + geometry)
        check("xdotool", "windowfocus", "--sync", window, env=env)
        screenshot("01-home.png", env)
        assert app.poll() is None, "Desktop exited unexpectedly"
        check("xdotool", "key", "--clearmodifiers", "ctrl+l", env=env)
        screenshot("02-local.png", env)
        # Local music import path input.
        check("xdotool", "type", "--clearmodifiers", str(audio), env=env)
        check("xdotool", "key", "Return", env=env)
        screenshot("03-import.png", env)
        saved = Path(data) / "library.json"
        assert saved.exists(), "Import did not save library; inspect screenshots"
        library = json.loads(saved.read_text())
        assert len(library["local"]) == 1, library
        print("Imported and persisted:", library["local"][0]["title"])
        check("xdotool", "key", "--clearmodifiers", "ctrl+Return", env=env)
        screenshot("04-playback.png", env)
        library = json.loads(saved.read_text())
        assert len(library["history"]) == 1, "Local playback did not start"
        print("Started native audio playback:", library["history"][0]["title"])
        # Search uses the actual YouTube Music service.
        check("xdotool", "key", "--clearmodifiers", "ctrl+f", env=env)
        screenshot("05-search-focus.png", env)
        check("xdotool", "type", "--clearmodifiers", "For Revenge Jakarta Hari Ini", env=env)
        check("xdotool", "key", "Return", env=env)
        screenshot("05-search.png", env)
        check("xdotool", "key", "--clearmodifiers", "ctrl+Return", env=env)
        screenshot("06-online-playback.png", env)
        library = json.loads(saved.read_text())
        assert any(t["source"].startswith("https://") for t in library["history"]), "Search result did not start playback"
        check("xdotool", "windowsize", "--sync", window, "1920", "1028", env=env)
        check("xdotool", "key", "F11", env=env)
        screenshot("07-expanded-player.png", env, "1920x1028")
        check("xdotool", "key", "l", env=env)
        screenshot("07-expanded-lyrics.png", env, "1920x1028")
        check("xdotool", "key", "q", env=env)
        screenshot("07-expanded-queue.png", env, "1920x1028")
        check("xdotool", "key", "Escape", env=env)
        check("xdotool", "mousemove", "--window", window, "90", "127", "click", "1", env=env)
        screenshot("08-home-wide.png", env, "1920x1028")
        check("xdotool", "mousemove", "--window", window, "420", "295", "click", "1", env=env)
        screenshot("08-home-selected-playback.png", env, "1920x1028")
        assert json.loads(saved.read_text())["history"][0]["source"] == str(audio), "Home song selection did not immediately start playback"
        check("xdotool", "windowsize", "--sync", window, "960", "640", env=env)
        screenshot("09-home-narrow.png", env, "960x640")
        assert app.poll() is None
        print("PASS: native startup, keyboard navigation, local import, persistence, local/online playback, search and expanded player")
        print("Screenshots:", OUT)
    finally:
        if window and app.poll() is None:
            subprocess.run(["xdotool", "key", "--clearmodifiers", "ctrl+q"], env=env, check=False)
        try:
            app.wait(timeout=5)
        except subprocess.TimeoutExpired:
            app.terminate()
            app.wait(timeout=10)
        assert app.returncode == 0, "Desktop failed to shut down cleanly; inspect desktop.log"
        if xvfb:
            xvfb.terminate()
            xvfb.wait(timeout=10)
        log.close()
