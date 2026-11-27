"""Packages the app with scripts/nsh_package.sh and checks what ships.

The macOS integration tests inspect the signed bundle and archive. The
Windows regression uses stub tools to exercise installer replacement.
"""

import json
import os
import plistlib
import subprocess
import sys
import tarfile
import tempfile
import unittest
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
SCRIPT = ROOT / "scripts" / "nsh_package.sh"
MANIFEST_SCRIPT = ROOT / "scripts" / "nsh_update_manifest.sh"
VERSION = (ROOT / "VERSION").read_text().strip()


def tiny_executable(path: Path) -> None:
    """A real Mach-O stand-in for aleph-server or AlephBridge, so the test
    skips their release builds."""
    subprocess.run(
        ["cc", "-x", "c", "-", "-o", str(path)],
        input=b"int main(void) { return 0; }\n",
        check=True,
    )


@unittest.skipIf(sys.platform == "win32", "the fixture supplies Unix executable stubs")
class WindowsInstaller(unittest.TestCase):
    def test_only_rebuilt_installer_is_left_at_the_release_path(self):
        with tempfile.TemporaryDirectory(prefix="aleph-inno-") as work:
            root = Path(work)
            tools = root / "tools"
            tools.mkdir()
            system = root / "System32"
            system.mkdir()
            out = root / "release output"
            server = root / "aleph-server.exe"
            server.write_text("server")
            stubs = {
                tools / "nsh": 'out=${@: -1}\nmkdir -p "$out/Aleph"\ntouch "$out/Aleph.iss"\necho stale > "$out/Aleph-setup.exe"',
                tools / "cygpath": 'printf "%s\\n" "$2"',
                system / "tar.exe": 'test -f Aleph/aleph-server.exe',
                tools / "ISCC.exe": '\n'.join([
                    'test "$MSYS2_ARG_CONV_EXCL" = /O',
                    'test "$1" = "/O$PWD"',
                    'test "$2" = Aleph.iss',
                    'test ! -e Aleph-setup.exe',
                    'cp Aleph/aleph-server.exe Aleph-setup.exe',
                ]),
            }
            for path, script in stubs.items():
                path.write_text(f"#!/usr/bin/env bash\nset -eu\n{script}\n")
                path.chmod(0o755)
            result = subprocess.run(
                ["bash", str(SCRIPT), "--target", "windows", "--out", str(out),
                 "--server", str(server)],
                env={**os.environ, "PATH": f"{tools}:{os.environ['PATH']}",
                     "SYSTEMROOT": str(root)},
                capture_output=True, text=True, check=False,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertEqual((out / "Aleph-setup.exe").read_bytes(), server.read_bytes())


@unittest.skipUnless(sys.platform == "darwin", "the macOS package is checked on macOS")
class MacosPackage(unittest.TestCase):
    @classmethod
    def setUpClass(cls):
        cls.work = tempfile.TemporaryDirectory(prefix="aleph-nsh-package-")
        work = Path(cls.work.name)
        cls.out = work / "out"
        server = work / "aleph-server"
        bridge = work / "AlephBridge"
        tiny_executable(server)
        tiny_executable(bridge)
        cls.result = subprocess.run(
            [
                "bash", str(SCRIPT), "--target", "macos", "--out", str(cls.out),
                "--server", str(server), "--bridge", str(bridge),
            ],
            cwd=ROOT,
            capture_output=True,
            text=True,
        )

    @classmethod
    def tearDownClass(cls):
        cls.work.cleanup()

    def setUp(self):
        self.assertEqual(self.result.returncode, 0, self.result.stdout + self.result.stderr)
        self.app = self.out / "Aleph.app"

    def test_bundle_carries_the_executable_helpers(self):
        # aleph-server finds AlephBridge beside its own executable.
        for helper in ("aleph-server", "AlephBridge"):
            path = self.app / "Contents" / "MacOS" / helper
            self.assertTrue(path.is_file(), helper)
            self.assertTrue(os.access(path, os.X_OK), helper)

    def test_bundle_carries_the_app_and_tray_icons(self):
        contents = self.app / "Contents"
        with (contents / "Info.plist").open("rb") as info:
            icon_name = plistlib.load(info)["CFBundleIconFile"]
        if not icon_name.endswith(".icns"):
            icon_name += ".icns"
        self.assertTrue((contents / "Resources" / icon_name).is_file())
        tray = contents / "Resources/nsh/assets/@aleph-nsh-shell/aleph-tray.png"
        self.assertEqual(
            tray.read_bytes(),
            (ROOT / "desktop/nsh-shell/assets/aleph-tray.png").read_bytes(),
        )

    def test_bundle_signature_verifies(self):
        verify = subprocess.run(
            ["codesign", "--verify", "--deep", "--strict", str(self.app)],
            capture_output=True,
            text=True,
        )
        self.assertEqual(verify.returncode, 0, verify.stderr)

    def archive(self) -> Path:
        machine = os.uname().machine
        arch = "aarch64" if machine == "arm64" else machine
        return self.out / f"Aleph-{VERSION}-macos-{arch}.tar.gz"

    def test_archive_holds_the_server_and_no_appledouble_files(self):
        with tarfile.open(self.archive()) as tar:
            names = tar.getnames()
        self.assertIn("Aleph.app/Contents/MacOS/aleph-server", names)
        self.assertIn("Aleph.app/Contents/MacOS/AlephBridge", names)
        self.assertFalse([name for name in names if Path(name).name.startswith("._")])

    def test_microphone_is_declared(self):
        plist = (self.app / "Contents" / "Info.plist").read_text()
        self.assertIn("NSMicrophoneUsageDescription", plist)
        entitlements = subprocess.run(
            ["codesign", "-d", "--entitlements", "-", "--xml", str(self.app)],
            capture_output=True,
            text=True,
        )
        self.assertIn("com.apple.security.device.audio-input", entitlements.stdout)

    def test_package_carries_the_update_configuration(self):
        config = self.app / "Contents" / "Resources" / "nsh" / "assets" / "nsh-updater.json"
        nsh_json = json.loads((ROOT / "desktop" / "nsh-shell" / "nsh.json").read_text())
        text = config.read_text()
        self.assertIn(nsh_json["updater"]["url"], text)
        self.assertIn(nsh_json["updater"]["publicKey"], text)

    @unittest.skipUnless(
        os.environ.get("ALEPH_UPDATE_KEY_FILE"),
        "set ALEPH_UPDATE_KEY_FILE to the update key's .secret: nsh signs only with "
        "the key of nsh.json's updater.publicKey",
    )
    def test_manifest_is_aleph_update_json_with_its_signature(self):
        with tempfile.TemporaryDirectory(prefix="aleph-nsh-manifest-") as work:
            out = Path(work) / "release"
            result = subprocess.run(
                ["bash", str(MANIFEST_SCRIPT), "--out", str(out),
                 "--key", os.environ["ALEPH_UPDATE_KEY_FILE"], str(self.archive())],
                cwd=ROOT,
                capture_output=True,
                text=True,
            )
            self.assertEqual(result.returncode, 0, result.stdout + result.stderr)
            self.assertTrue((out / "aleph-update.json").is_file())
            self.assertTrue((out / "latest.json.sig").is_file())
            self.assertFalse((out / "latest.json").exists())
            manifest = (out / "aleph-update.json").read_text()
            self.assertIn(VERSION, manifest)
            self.assertIn(self.archive().name, manifest)


if __name__ == "__main__":
    unittest.main()
