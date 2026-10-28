import hashlib
import importlib.util
import io
import json
import os
from pathlib import Path
import sys
import tempfile
import unittest
from unittest.mock import MagicMock, patch
import zipfile


UPDATER = Path(__file__).resolve().parents[1] / "app" / "src" / "main" / "python" / "yt_dlp_update.py"
VERSION = "2026.11.2"
EJS = "0.9.0"
STAMP = "1.0/123"


def wheel(files: dict[str, str]) -> bytes:
    buffer = io.BytesIO()
    with zipfile.ZipFile(buffer, "w") as archive:
        for name, text in files.items():
            archive.writestr(name, text)
    return buffer.getvalue()


def yt_dlp_wheel(version=VERSION, pins=(EJS,), extra=None) -> bytes:
    metadata = "Name: yt-dlp\n" + "".join(
        f'Requires-Dist: yt-dlp-ejs=={pin} ; extra == "default"\n' for pin in pins)
    files = {
        "yt_dlp/__init__.py": "",
        "yt_dlp/version.py": f"__version__ = {version!r}\n",
        f"yt_dlp-{version}.dist-info/METADATA": metadata,
        "yt_dlp_ejs/__init__.py": "ignored",
    }
    files.update(extra or {})
    return wheel(files)


EJS_WHEEL = wheel({"yt_dlp_ejs/__init__.py": "", f"yt_dlp_ejs-{EJS}.dist-info/METADATA": ""})


def pypi_entry(filename: str, data: bytes) -> dict:
    return {
        "filename": filename,
        "packagetype": "bdist_wheel",
        "url": f"https://files.pythonhosted.org/packages/{filename}",
        "size": len(data),
        "digests": {"sha256": hashlib.sha256(data).hexdigest()},
    }


class FakeResponse(io.BytesIO):
    def __init__(self, data=b"", url="", status=200):
        super().__init__(data)
        self._url = url
        self.status = status

    def geturl(self):
        return self._url


class UpdaterTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("yt_dlp_update", UPDATER)
        self.updater = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.updater)
        self.updater._log = lambda message: None
        temporary = tempfile.TemporaryDirectory()
        self.addCleanup(temporary.cleanup)
        self.root = os.path.join(temporary.name, "yt-dlp-update")
        self.modules = patch.dict(sys.modules)
        self.modules.start()
        self.addCleanup(self.modules.stop)
        sys.modules.pop("yt_dlp", None)
        self.sys_path = list(sys.path)
        self.addCleanup(lambda: sys.path.__setitem__(slice(None), self.sys_path))
        context = patch.object(self.updater.ssl, "create_default_context", return_value=MagicMock())
        context.start()
        self.addCleanup(context.stop)

    def serve(self, latest=VERSION, main=None, ejs=EJS_WHEEL, main_entry=None):
        main = yt_dlp_wheel(latest) if main is None else main
        main_name = f"yt_dlp-{latest}-py3-none-any.whl"
        ejs_name = f"yt_dlp_ejs-{EJS}-py3-none-any.whl"
        responses = {
            self.updater._PYPI_URL.format(name="yt-dlp", version=latest):
                json.dumps({"urls": [main_entry or pypi_entry(main_name, main)]}).encode(),
            self.updater._PYPI_URL.format(name="yt-dlp-ejs", version=EJS):
                json.dumps({"urls": [pypi_entry(ejs_name, ejs)]}).encode(),
            f"https://files.pythonhosted.org/packages/{main_name}": main,
            f"https://files.pythonhosted.org/packages/{ejs_name}": ejs,
        }
        self.requests = []

        def fake_open(url, context, method="GET"):
            self.requests.append((method, url))
            if url == self.updater._LATEST_URL:
                return FakeResponse(url=f"https://github.com/yt-dlp/yt-dlp/releases/tag/{latest}")
            return FakeResponse(responses[url], url)

        opener = patch.object(self.updater, "_open", side_effect=fake_open)
        opener.start()
        self.addCleanup(opener.stop)

    def check(self, running="2026.10.26", now=1000.0, force=False):
        return self.updater.check(self.root, STAMP, running, "ca.pem", now=now, force=force)

    def state(self):
        with open(os.path.join(self.root, "state.json"), encoding="utf-8") as file:
            return json.load(file)

    def test_version_tuple_accepts_only_stable_releases(self):
        self.assertEqual(self.updater.version_tuple("2026.10.26"), (2026, 10, 26))
        self.assertEqual(self.updater.version_tuple("2026.10.26.1"), (2026, 10, 26, 1))
        self.assertIsNone(self.updater.version_tuple("2026.10.26.dev0"))
        self.assertIsNone(self.updater.version_tuple(None))

    def test_check_downloads_and_extracts_both_packages(self):
        self.serve()
        message = self.check()
        self.assertIn(VERSION, message)
        self.assertEqual(self.state(), {"version": VERSION, "apk": STAMP, "status": "trial", "launches": 0})
        target = os.path.join(self.root, "v", VERSION)
        self.assertTrue(os.path.isfile(os.path.join(target, "yt_dlp", "version.py")))
        with open(os.path.join(target, "yt_dlp_ejs", "__init__.py"), encoding="utf-8") as file:
            self.assertEqual(file.read(), "", "ejs must come from its own wheel")
        self.assertFalse(os.path.exists(os.path.join(target, f"yt_dlp-{VERSION}.dist-info")))
        self.assertFalse([name for name in os.listdir(self.root) if name.startswith("staging-")])

    def test_check_is_rate_limited_unless_forced(self):
        self.serve()
        self.assertEqual(self.check(running=VERSION), "")
        self.assertEqual(self.check(running=VERSION, now=1000.0 + 3600), "")
        self.assertEqual(len(self.requests), 1)
        self.check(running=VERSION, now=1000.0 + 3600, force=True)
        self.assertEqual(len(self.requests), 2)
        self.check(running=VERSION, now=1000.0 + 25 * 3600)
        self.assertEqual(len(self.requests), 3)

    def test_check_skips_when_up_to_date_or_rejected(self):
        self.serve()
        self.assertEqual(self.check(running=VERSION), "")
        os.makedirs(self.root, exist_ok=True)
        with open(os.path.join(self.root, "rejected"), "w", encoding="utf-8") as file:
            file.write(VERSION)
        self.assertIn("se descarto", self.check(force=True))
        self.assertEqual([method for method, _ in self.requests], ["HEAD", "HEAD"])

    def test_check_rejects_hash_mismatch(self):
        main = yt_dlp_wheel()
        entry = pypi_entry(f"yt_dlp-{VERSION}-py3-none-any.whl", main)
        entry["digests"]["sha256"] = "0" * 64
        self.serve(main=main, main_entry=entry)
        with self.assertRaisesRegex(RuntimeError, "SHA-256"):
            self.check()
        self.assertFalse(os.path.exists(os.path.join(self.root, "state.json")))

    def test_check_rejects_foreign_download_host(self):
        main = yt_dlp_wheel()
        entry = pypi_entry(f"yt_dlp-{VERSION}-py3-none-any.whl", main)
        entry["url"] = "https://example.com/yt_dlp.whl"
        self.serve(main=main, main_entry=entry)
        with self.assertRaisesRegex(RuntimeError, "no valida"):
            self.check()

    def test_check_rejects_unsafe_paths_and_cleans_staging(self):
        self.serve(main=yt_dlp_wheel(extra={"yt_dlp/../../evil.py": "x"}))
        with self.assertRaisesRegex(RuntimeError, "Ruta no permitida"):
            self.check()
        self.assertFalse([name for name in os.listdir(self.root) if name.startswith("staging-")])
        self.assertFalse(os.path.exists(os.path.join(self.root, "v")))

    def test_pinned_ejs_requires_single_pin(self):
        self.assertEqual(self.updater._pinned_ejs(yt_dlp_wheel(), VERSION), EJS)
        with self.assertRaisesRegex(RuntimeError, "unica"):
            self.updater._pinned_ejs(yt_dlp_wheel(pins=()), VERSION)
        with self.assertRaisesRegex(RuntimeError, "unica"):
            self.updater._pinned_ejs(yt_dlp_wheel(pins=("0.8.0", "0.9.0")), VERSION)

    def test_latest_version_rejects_unexpected_redirect(self):
        response = FakeResponse(url="https://github.com/yt-dlp/yt-dlp/releases")
        with patch.object(self.updater, "_open", return_value=response):
            with self.assertRaisesRegex(RuntimeError, "inesperada"):
                self.updater._latest_version(MagicMock())

    def download(self):
        self.serve()
        self.check()
        return os.path.join(self.root, "v", VERSION)

    def test_activate_puts_update_first_and_counts_trial_launches(self):
        path = self.download()
        self.assertEqual(self.updater.activate(self.root, STAMP), VERSION)
        self.assertEqual(sys.path[0], path)
        self.assertEqual(self.state()["launches"], 1)
        self.assertTrue(self.updater.trial_pending(self.root))
        self.updater.confirm(self.root)
        self.assertFalse(self.updater.trial_pending(self.root))
        self.assertEqual(self.state(), {"version": VERSION, "apk": STAMP, "status": "ok"})

    def test_activate_discards_update_after_apk_reinstall(self):
        self.download()
        self.assertEqual(self.updater.activate(self.root, "1.0/999"), "")
        self.assertFalse(os.path.exists(os.path.join(self.root, "state.json")))
        self.assertEqual(os.listdir(os.path.join(self.root, "v")), [])

    def test_activate_gives_up_after_unverified_launches(self):
        self.download()
        for _ in range(self.updater._MAX_TRIAL_LAUNCHES):
            self.assertEqual(self.updater.activate(self.root, STAMP), VERSION)
            self.updater._active = ""
        self.assertEqual(self.updater.activate(self.root, STAMP), "")
        self.assertFalse(os.path.exists(os.path.join(self.root, "state.json")))

    def test_activate_ignores_incomplete_download(self):
        path = self.download()
        os.remove(os.path.join(path, "yt_dlp_ejs", "__init__.py"))
        self.assertEqual(self.updater.activate(self.root, STAMP), "")
        self.assertNotIn(path, sys.path)

    def test_activate_is_skipped_once_yt_dlp_was_imported(self):
        self.download()
        sys.modules["yt_dlp"] = MagicMock()
        self.assertEqual(self.updater.activate(self.root, STAMP), "")

    def test_reject_active_only_blames_versions_on_trial(self):
        self.download()
        self.updater.activate(self.root, STAMP)
        self.updater.confirm(self.root)
        self.assertFalse(self.updater.reject_active(self.root))
        self.assertEqual(self.state()["status"], "ok")

        self.updater._active = ""
        sys.path[:] = self.sys_path
        self.download_again()
        self.updater.activate(self.root, STAMP)
        self.assertTrue(self.updater.reject_active(self.root))
        with open(os.path.join(self.root, "rejected"), encoding="utf-8") as file:
            self.assertEqual(file.read(), VERSION)
        self.assertIn("se descarto", self.check(force=True))

    def download_again(self):
        state = self.state()
        state.update(status="trial", launches=0)
        self.updater._write_state(self.root, state)

    def test_online_reports_network_errors_as_offline(self):
        with patch.object(self.updater, "_open", side_effect=OSError("down")):
            self.assertFalse(self.updater.online("ca.pem"))
        with patch.object(self.updater, "_open", return_value=FakeResponse(status=204)):
            self.assertTrue(self.updater.online("ca.pem"))


if __name__ == "__main__":
    unittest.main()
