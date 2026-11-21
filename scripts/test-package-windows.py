import importlib.util
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch


SCRIPT = Path(__file__).with_name("package-windows.py")
SPEC = importlib.util.spec_from_file_location("package_windows", SCRIPT)
package = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(package)


class PackageWindowsTests(unittest.TestCase):
    def test_native_windows_binary_beats_stale_cross_build(self):
        with tempfile.TemporaryDirectory() as directory:
            root = Path(directory)
            native = root / "target/release/ytdugem.exe"
            cross = root / "target/x86_64-pc-windows-gnu/release/ytdugem.exe"
            native.parent.mkdir(parents=True)
            cross.parent.mkdir(parents=True)
            native.touch()
            cross.touch()
            self.assertEqual(package.windows_binary(root, True), native)
            self.assertEqual(package.windows_binary(root, False), cross)

    def test_tar_fallback_uses_libarchive_syntax(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "fixture.7z"
            destination = Path(directory) / "out"
            destination.mkdir()
            with patch.dict("os.environ", {}, clear=True), patch.object(package.shutil, "which", side_effect=lambda name: "tar.exe" if name == "tar" else None), patch.object(package.subprocess, "run") as run:
                package.extract_7z(archive, destination)
            run.assert_called_once_with(["tar.exe", "-xf", str(archive), "-C", str(destination)], check=True)

    def test_start_here_sources_follow_the_pins(self):
        tools = {
            "mpv": {"repo": "shinchiro/mpv-winbuild-cmake", "tag": "20261021", "asset": "mpv-x86_64-20261021-git-e471496c56.7z"},
            "ffmpeg": {"repo": "GyanD/codexffmpeg", "tag": "9.0.2", "asset": "ffmpeg-9.0.2-essentials_build.7z"},
            "yt-dlp": {"repo": "yt-dlp/yt-dlp", "tag": "2026.10.26", "asset": "yt-dlp.exe"},
            "deno": {"repo": "denoland/deno", "tag": "v2.9.8", "asset": "deno-x86_64-pc-windows-msvc.zip"},
        }
        self.assertEqual(
            package.source_lines(tools),
            "mpv/FFmpeg build recipes and upstream source: https://github.com/shinchiro/mpv-winbuild-cmake/tree/20261021\n"
            "mpv source: https://github.com/mpv-player/mpv/tree/e471496c56\n"
            "FFmpeg source: https://github.com/FFmpeg/FFmpeg/tree/n9.0.2\n"
            "FFmpeg Windows build: https://github.com/GyanD/codexffmpeg/releases/tag/9.0.2\n"
            "yt-dlp source/license: https://github.com/yt-dlp/yt-dlp/tree/2026.10.26\n"
            "Deno source/license: https://github.com/denoland/deno/tree/v2.9.8\n",
        )
        self.assertEqual(
            package.asset_url(tools["mpv"]),
            "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261021/mpv-x86_64-20261021-git-e471496c56.7z",
        )

    def test_committed_table_is_valid(self):
        tools = package.load_tools()
        self.assertEqual(list(tools), ["mpv", "ffmpeg", "yt-dlp", "deno"])
        for entry in tools.values():
            self.assertRegex(entry["sha256"], "^[0-9a-f]{64}$")
            self.assertRegex(entry["asset"], f"^{entry['asset_pattern']}$")
        package.source_lines(tools)


if __name__ == "__main__":
    unittest.main()
