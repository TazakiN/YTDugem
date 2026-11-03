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


if __name__ == "__main__":
    unittest.main()
