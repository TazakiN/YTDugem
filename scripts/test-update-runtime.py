import importlib.util
from pathlib import Path
import unittest


SCRIPT = Path(__file__).with_name("update-runtime.py")
SPEC = importlib.util.spec_from_file_location("update_runtime", SCRIPT)
update = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(update)

MPV = {
    "repo": "shinchiro/mpv-winbuild-cmake",
    "tag": "20261021",
    "asset": "mpv-x86_64-20261021-git-e471496c56.7z",
    "asset_pattern": r"mpv-x86_64-\d{8}-git-[0-9a-f]+\.7z",
    "sha256": "0" * 64,
}


def release(tag, *names):
    return {"tag_name": tag, "assets": [{"name": name, "digest": f"sha256:{index:064x}"} for index, name in enumerate(names)]}


class PickUpdateTests(unittest.TestCase):
    def test_current_asset_is_not_an_update(self):
        latest = release("20261021", "mpv-x86_64-20261021-git-e471496c56.7z", "mpv-x86_64-v3-20261021-git-e471496c56.7z")
        self.assertIsNone(update.pick_update(MPV, latest))

    def test_new_tag_with_the_same_asset_name_is_an_update(self):
        entry = {"repo": "yt-dlp/yt-dlp", "tag": "2026.10.26", "asset": "yt-dlp.exe", "asset_pattern": r"yt-dlp\.exe"}
        found = update.pick_update(entry, release("2026.11.20", "yt-dlp.exe", "yt-dlp_x86.exe"))
        self.assertEqual(found["url"], "https://github.com/yt-dlp/yt-dlp/releases/download/2026.11.20/yt-dlp.exe")

    def test_newer_release_picks_baseline_build_and_builds_url(self):
        latest = release(
            "20261111",
            "mpv-dev-x86_64-20261111-git-abc1234.7z",
            "mpv-x86_64-v3-20261111-git-abc1234.7z",
            "mpv-x86_64-20261111-git-abc1234.7z",
        )
        self.assertEqual(
            update.pick_update(MPV, latest),
            {
                "tag": "20261111",
                "asset": "mpv-x86_64-20261111-git-abc1234.7z",
                "url": "https://github.com/shinchiro/mpv-winbuild-cmake/releases/download/20261111/mpv-x86_64-20261111-git-abc1234.7z",
                "digest": f"sha256:{2:064x}",
            },
        )

    def test_missing_digest_is_none(self):
        latest = {"tag_name": "20261111", "assets": [{"name": "mpv-x86_64-20261111-git-abc1234.7z"}]}
        self.assertIsNone(update.pick_update(MPV, latest)["digest"])

    def test_no_matching_asset_raises(self):
        with self.assertRaisesRegex(RuntimeError, "found none"):
            update.pick_update(MPV, release("20261111", "mpv-x86_64-v3-20261111-git-abc1234.7z"))

    def test_two_matching_assets_raise(self):
        latest = release("20261111", "mpv-x86_64-20261111-git-abc1234.7z", "mpv-x86_64-20261111-git-def5678.7z")
        with self.assertRaisesRegex(RuntimeError, "expected one asset"):
            update.pick_update(MPV, latest)

    def test_ffmpeg_pattern_skips_zip_and_full_builds(self):
        entry = {"repo": "GyanD/codexffmpeg", "tag": "9.0.2", "asset": "ffmpeg-9.0.2-essentials_build.7z", "asset_pattern": r"ffmpeg-[0-9.]+-essentials_build\.7z"}
        latest = release("9.1", "ffmpeg-9.1-essentials_build.zip", "ffmpeg-9.1-essentials_build.7z", "ffmpeg-9.1-full_build.7z")
        self.assertEqual(update.pick_update(entry, latest)["asset"], "ffmpeg-9.1-essentials_build.7z")


if __name__ == "__main__":
    unittest.main()
