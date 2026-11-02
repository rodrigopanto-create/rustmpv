import importlib.util
import json
import os
from pathlib import Path
import sys
import tempfile
import types
import unittest
from unittest.mock import MagicMock, patch
import zipfile


SOURCE = Path(__file__).resolve().parents[1] / "app" / "src" / "main" / "python" / "anonymous_provider.py"


class AnonymousProviderTests(unittest.TestCase):
    def setUp(self):
        spec = importlib.util.spec_from_file_location("anonymous_provider", SOURCE)
        self.provider = importlib.util.module_from_spec(spec)
        spec.loader.exec_module(self.provider)
        directory = tempfile.TemporaryDirectory()
        self.addCleanup(directory.cleanup)
        self.root = Path(directory.name)
        self.node = self.root / "libnode.so"
        self.ca = self.root / "cacert.pem"
        self.node.touch()
        self.ca.touch()
        self.bundle = self.root / "bundle.zip"
        with zipfile.ZipFile(self.bundle, "w") as archive:
            archive.writestr("manifest.json", json.dumps({"provider_version": "2.0.2"}))
            archive.writestr("server/build/generate_once.js", "// upstream")
        self.process = MagicMock()
        self.process.return_value.stdout = "2.0.2\n"
        self.loader = types.ModuleType("yt_dlp.plugins")
        self.loader.load_all_plugins = MagicMock()
        self.plugin = types.ModuleType("yt_dlp_plugins.extractor.getpot_bgutil_script")
        self.plugin.BgUtilScriptNodePTP = types.SimpleNamespace(PROVIDER_VERSION="2.0.2")

    def initialize(self):
        with patch.dict(os.environ), patch.object(sys, "path", sys.path.copy()), \
                patch.object(self.provider.subprocess, "run", self.process), \
                patch.dict(sys.modules, {
                    "yt_dlp.plugins": self.loader,
                    "yt_dlp_plugins.extractor.getpot_bgutil_script": self.plugin,
                }):
            self.provider.initialize(str(self.node), str(self.bundle),
                                     str(self.root / "private"), str(self.ca))
            return os.environ.copy()

    def test_initialization_verifies_script_loads_plugin_and_preserves_tls(self):
        environment = self.initialize()
        self.assertEqual(environment["NODE_EXTRA_CA_CERTS"], str(self.ca))
        self.assertEqual(environment["OPENSSL_CONF"], os.devnull)
        self.assertNotIn("NODE_TLS_REJECT_UNAUTHORIZED", environment)
        self.assertTrue(environment["LD_LIBRARY_PATH"].startswith(str(self.root / "private")))
        self.loader.load_all_plugins.assert_called_once()
        self.assertTrue(self.process.call_args.kwargs["check"])
        self.assertEqual(self.provider.options()["extractor_args"]["youtube"],
                         {"player_client": ["default"]})
        self.assertEqual(self.provider.options()["js_runtimes"]["node"]["path"], str(self.node))
        self.initialize()
        self.process.assert_called_once()

    def test_uninitialized_provider_fails(self):
        with self.assertRaisesRegex(RuntimeError, "inicializado"):
            self.provider.options()

    def test_missing_node_fails_without_subprocess(self):
        self.node.unlink()
        with self.assertRaisesRegex(RuntimeError, "Node.js"):
            self.initialize()
        self.process.assert_not_called()

    def test_bad_version_fails_and_does_not_publish_configuration(self):
        self.process.return_value.stdout = "wrong\n"
        with self.assertRaisesRegex(RuntimeError, "Version"):
            self.initialize()
        self.assertIsNone(self.provider._configuration)

    def test_zip_traversal_is_rejected_and_staging_is_removed(self):
        with zipfile.ZipFile(self.bundle, "a") as archive:
            archive.writestr("../escaped.txt", "not allowed")
        with self.assertRaisesRegex(RuntimeError, "Ruta no permitida"):
            self.initialize()
        self.assertFalse((self.root / "escaped.txt").exists())
        self.assertEqual(list((self.root / "private").iterdir()), [])

    def test_windows_zip_paths_are_extracted_as_directories(self):
        with zipfile.ZipFile(self.bundle, "a") as archive:
            archive.writestr("runtime-libs\\libz_node.so.1", "native")
        environment = self.initialize()
        libraries = Path(environment["LD_LIBRARY_PATH"].split(os.pathsep)[0])
        self.assertEqual((libraries / "libz_node.so.1").read_text(), "native")

    def test_windows_zip_traversal_is_rejected(self):
        with zipfile.ZipFile(self.bundle, "a") as archive:
            archive.writestr("..\\escaped.txt", "not allowed")
        with self.assertRaisesRegex(RuntimeError, "Ruta no permitida"):
            self.initialize()
        self.assertFalse((self.root / "escaped.txt").exists())


if __name__ == "__main__":
    unittest.main()
