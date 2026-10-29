import importlib.util
import json
from pathlib import Path
import sys
import types
import unittest
from unittest.mock import MagicMock, patch


RUNNER = Path(__file__).resolve().parents[1] / "app" / "src" / "main" / "python" / "yt_dlp_runner.py"


class RunnerTests(unittest.TestCase):
    def setUp(self):
        dependency = types.ModuleType("yt_dlp")
        dependency.__path__ = []
        dependency.YoutubeDL = MagicMock()
        utils = types.ModuleType("yt_dlp.utils")
        utils.DownloadError = type("DownloadError", (Exception,), {})
        spec = importlib.util.spec_from_file_location("yt_dlp_runner", RUNNER)
        self.runner = importlib.util.module_from_spec(spec)
        with patch.dict(sys.modules, {"yt_dlp": dependency, "yt_dlp.utils": utils}):
            spec.loader.exec_module(self.runner)
        self.factory = dependency.YoutubeDL
        self.download_error = utils.DownloadError
        self.downloader = self.factory.return_value
        self.runtime_path = str(Path(__file__).resolve().parent / "libqjs.so")
        self.file_check = patch.object(self.runner.os.path, "isfile", return_value=True)
        self.file_check.start()
        self.addCleanup(self.file_check.stop)
        self.process = patch.object(self.runner.subprocess, "run")
        self.run = self.process.start()
        self.addCleanup(self.process.stop)
        self.run.return_value.stdout = "visualrustmpv-quickjs-ok\n"

    def resolve(self, url, audio_only):
        return self.media(url, audio_only)["url"]

    def media(self, url, audio_only, cache_dir=None):
        return json.loads(self.runner.extract_media(url, audio_only, self.runtime_path, cache_dir))

    def test_audio_and_video_formats_and_network_limits(self):
        for audio_only, expected in [
            (True, "bestaudio[ext=m4a]/bestaudio[ext=webm]/bestaudio/best"),
            (False, "best[ext=mp4][height<=720][protocol^=http]/best[ext=mp4][height<=720]"
                    "/best[height<=720]/best"),
        ]:
            with self.subTest(audio_only=audio_only):
                self.downloader.extract_info.return_value = {"url": "https://cdn.example/video"}
                self.assertEqual(self.resolve("https://youtu.be/12345678901", audio_only),
                                 "https://cdn.example/video")
                options = self.factory.call_args.args[0]
                self.assertEqual(options["format"], expected)
                self.assertEqual(options["socket_timeout"], 10)
                self.assertEqual(options["retries"], 1)
                self.assertEqual(options["extractor_retries"], 1)
                self.assertFalse(options["noplaylist"])
                self.assertEqual(options["playlistend"], 100)
                self.assertTrue(options["ignoreerrors"])
                self.assertTrue(options["skip_download"])
                self.assertEqual(options["js_runtimes"], {"quickjs": {"path": self.runtime_path}})
                self.assertEqual(options["remote_components"], set())
                self.assertIs(options["cachedir"], False)
                self.assertIs(options["cachedir"], False)
                self.downloader.extract_info.assert_called_with(
                    "https://youtu.be/12345678901", download=False)

    def test_requested_direct_stream(self):
        self.downloader.extract_info.return_value = {
            "requested_downloads": [{"url": "https://cdn.example/audio"}]}
        self.assertEqual(self.resolve("https://youtube.com/watch?v=12345678901", True),
                         "https://cdn.example/audio")

    def test_playlist_returns_bounded_stream_queue(self):
        self.downloader.extract_info.return_value = {
            "_type": "playlist",
            "entries": [
                {"url": f"https://cdn.example/{index}", "title": f"Track {index}"}
                for index in range(3)
            ],
        }
        media = self.media("https://youtube.com/playlist?list=PL123", False)
        self.assertEqual(media["url"], "https://cdn.example/0")
        self.assertEqual(
            media["playlist"],
            ["https://cdn.example/1", "https://cdn.example/2"],
        )
        self.assertEqual(self.downloader.extract_info.call_count, 1)

    def test_playlist_skips_unavailable_entries(self):
        self.downloader.extract_info.return_value = {
            "_type": "playlist",
            "entries": [
                None,
                {"url": "https://cdn.example/available", "title": "Available"},
            ],
        }
        media = self.media("https://youtube.com/playlist?list=PL123", False)
        self.assertEqual(media["url"], "https://cdn.example/available")
        self.assertEqual(media["playlist"], [])

    def test_playlist_skips_flat_entries_rejected_by_extractor(self):
        self.downloader.extract_info.side_effect = [
            {
                "_type": "playlist",
                "entries": [
                    {"_type": "url", "id": "private", "ie_key": "Youtube"},
                    {"_type": "url", "id": "public", "ie_key": "Youtube"},
                ],
            },
            self.download_error("private video"),
            {"url": "https://cdn.example/public", "title": "Public"},
        ]
        media = self.media("https://youtube.com/playlist?list=PL123", True)
        self.assertEqual(media["url"], "https://cdn.example/public")
        self.assertEqual(media["playlist"], [])

    def test_playlist_resolves_flat_entries_and_caps_at_100(self):
        self.downloader.extract_info.side_effect = [
            {
                "_type": "playlist",
                "entries": [
                    {"_type": "url", "id": f"video{index}", "ie_key": "Youtube"}
                    for index in range(105)
                ],
            },
            *[
                {"url": f"https://cdn.example/{index}", "title": f"Track {index}"}
                for index in range(100)
            ],
        ]
        media = self.media("https://youtube.com/playlist?list=PL123", True)
        self.assertEqual(media["url"], "https://cdn.example/0")
        self.assertEqual(len(media["playlist"]), 99)
        self.assertEqual(media["playlist"][-1], "https://cdn.example/99")
        self.assertEqual(self.downloader.extract_info.call_count, 101)
        self.assertEqual(
            self.downloader.extract_info.call_args_list[1].args[0],
            "https://www.youtube.com/watch?v=video0",
        )

    def test_social_multi_video_post_resolves_flat_entry_urls(self):
        self.downloader.extract_info.side_effect = [
            {
                "_type": "playlist",
                "entries": [
                    {"_type": "url", "id": "1", "ie_key": "Twitter",
                     "url": "https://x.com/user/status/1/video/1"},
                    {"_type": "url", "id": "2", "ie_key": "Twitter"},
                    {"url": "https://video.twimg.com/3.mp4", "title": "Inline"},
                ],
            },
            {"url": "https://video.twimg.com/1.mp4", "title": "First"},
        ]
        media = self.media("https://x.com/user/status/1", False)
        self.assertEqual(media["url"], "https://video.twimg.com/1.mp4")
        self.assertEqual(media["playlist"], ["https://video.twimg.com/3.mp4"])
        self.assertEqual(
            self.downloader.extract_info.call_args_list[1].args[0],
            "https://x.com/user/status/1/video/1",
        )
        self.assertEqual(self.downloader.extract_info.call_count, 2)

    def fail_with(self, message):
        def extract(url, download):
            self.factory.call_args.args[0]["logger"].error(message)
            return None
        self.downloader.extract_info.side_effect = extract

    def test_hidden_post_reports_login_requirement(self):
        for message in [
            "ERROR: [twitter] 1: No video could be found in this tweet",
            "ERROR: [Instagram] abc: Instagram sent an empty media response. "
            "Check if this post is accessible in your browser without being logged-in.",
        ]:
            with self.subTest(message=message):
                self.fail_with(message)
                with self.assertRaises(RuntimeError) as raised:
                    self.media("https://x.com/user/status/1", False)
                text = str(raised.exception)
                self.assertIn("requiere iniciar sesion", text)
                self.assertNotIn("Check if", text)

    def test_other_extraction_errors_keep_their_reason(self):
        self.fail_with("ERROR: [facebook] 1: Unable to download webpage: HTTP Error 500")
        with self.assertRaises(RuntimeError) as raised:
            self.media("https://www.facebook.com/user/videos/1/", False)
        self.assertEqual(
            str(raised.exception),
            "yt-dlp no pudo extraer el medio: [facebook] 1: "
            "Unable to download webpage: HTTP Error 500",
        )

    def test_previous_error_is_not_reported_for_a_later_failure(self):
        self.fail_with("ERROR: [twitter] 1: No video could be found in this tweet")
        with self.assertRaises(RuntimeError):
            self.media("https://x.com/user/status/1", False)
        self.downloader.extract_info.side_effect = None
        self.downloader.extract_info.return_value = None
        with self.assertRaises(RuntimeError) as raised:
            self.media("https://x.com/user/status/2", False)
        self.assertEqual(str(raised.exception), "yt-dlp no devolvio informacion de medios valida")

    def test_rejects_missing_invalid_or_multiple_streams(self):
        for info in [None, {}, {"_type": "playlist"}, {"url": "file:///tmp/x"},
                     {"url": "https://user:pass@host/video"}, {"url": "https://"},
                     {"url": "https://host/a\nb"}, {"url": 123},
                     {"requested_downloads": [{"url": "https://a.example"}, {"url": "https://b.example"}]}]:
            with self.subTest(info=info):
                self.downloader.extract_info.return_value = info
                with self.assertRaises(RuntimeError):
                    self.resolve("https://youtu.be/12345678901", False)

    def test_invalid_input_never_calls_extractor(self):
        with self.assertRaises(RuntimeError):
            self.resolve("file:///private", False)
        self.factory.assert_not_called()
        self.run.assert_not_called()

    def test_extractor_errors_are_not_hidden(self):
        self.downloader.extract_info.side_effect = RuntimeError("network timeout")
        with self.assertRaisesRegex(RuntimeError, "network timeout"):
            self.resolve("https://youtu.be/12345678901", True)

    def test_runtime_is_checked_once_with_timeout(self):
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/audio"}
        self.resolve("https://youtu.be/12345678901", True)
        self.resolve("https://youtu.be/12345678901", True)
        self.run.assert_called_once_with(
            [self.runtime_path, "--eval", 'console.log("visualrustmpv-quickjs-ok")'],
            capture_output=True, text=True, timeout=10, check=True)

    def test_runtime_failure_never_calls_extractor(self):
        for error in [OSError("Permission denied"),
                      self.runner.subprocess.TimeoutExpired("quickjs", 10),
                      self.runner.subprocess.CalledProcessError(1, "quickjs")]:
            with self.subTest(error=error):
                self.run.side_effect = error
                with self.assertRaisesRegex(RuntimeError, "No se pudo ejecutar QuickJS-ng"):
                    self.resolve("https://youtu.be/12345678901", True)
                self.factory.assert_not_called()

    def test_missing_runtime_never_calls_extractor(self):
        with patch.object(self.runner.os.path, "isfile", return_value=False):
            with self.assertRaisesRegex(RuntimeError, "No se encuentra QuickJS-ng"):
                self.resolve("https://youtu.be/12345678901", True)
        self.run.assert_not_called()
        self.factory.assert_not_called()

    def test_invalid_runtime_response_never_calls_extractor(self):
        self.run.return_value.stdout = "unexpected"
        with self.assertRaisesRegex(RuntimeError, "respuesta de comprobacion invalida"):
            self.resolve("https://youtu.be/12345678901", True)
        self.factory.assert_not_called()

    def test_warm_up_prepares_quickjs_and_both_downloaders(self):
        self.runner.warm_up(self.runtime_path, "/cache/yt-dlp")
        self.run.assert_called_once()
        formats = [call.args[0]["format"] for call in self.factory.call_args_list]
        self.assertEqual(formats, [self.runner._AUDIO_FORMAT, self.runner._VIDEO_FORMAT])
        self.assertEqual({call.args[0]["cachedir"] for call in self.factory.call_args_list},
                         {"/cache/yt-dlp"})
        self.downloader.get_info_extractor.assert_called_with("Youtube")
        self.downloader.extract_info.assert_not_called()

    def test_downloader_is_reused_after_warm_up(self):
        self.runner.warm_up(self.runtime_path, "/cache/yt-dlp")
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/audio"}
        for _ in range(3):
            self.media("https://youtu.be/12345678901", True, "/cache/yt-dlp")
        self.assertEqual(self.factory.call_count, 2)
        self.assertEqual(self.downloader.extract_info.call_count, 3)

    def test_download_errors_keep_cached_downloader(self):
        self.downloader.extract_info.side_effect = self.download_error("private video")
        for _ in range(2):
            with self.assertRaises(self.download_error):
                self.resolve("https://youtu.be/12345678901", True)
        self.factory.assert_called_once()
        self.downloader.close.assert_not_called()

    def test_unexpected_errors_discard_downloader(self):
        self.downloader.extract_info.side_effect = [RuntimeError("broken"),
                                                    {"url": "https://cdn.example/audio"}]
        with self.assertRaisesRegex(RuntimeError, "broken"):
            self.resolve("https://youtu.be/12345678901", True)
        self.downloader.close.assert_called_once()
        self.assertEqual(self.resolve("https://youtu.be/12345678901", True),
                         "https://cdn.example/audio")
        self.assertEqual(self.factory.call_count, 2)

    def make_player_cache(self, root, count):
        import os
        section = Path(root) / "challenge-solver"
        section.mkdir(parents=True)
        (section / "lib.json").write_text("{}")
        for index in range(count):
            player = section / f"player,3Ahttps,3A,2F,2Fyt,2F{index}.json"
            player.write_text("{}")
            os.utime(player, (1000 + index, 1000 + index))
        return section

    def test_player_cache_keeps_two_newest_players_and_scripts(self):
        import tempfile
        with tempfile.TemporaryDirectory() as root:
            section = self.make_player_cache(root, 4)
            self.runner._prune_player_cache(root)
            names = sorted(path.name for path in section.iterdir())
            self.assertEqual(names, ["lib.json", "player,3Ahttps,3A,2F,2Fyt,2F2.json",
                                     "player,3Ahttps,3A,2F,2Fyt,2F3.json"])

    def test_extraction_error_clears_cached_players(self):
        import tempfile
        self.downloader.extract_info.side_effect = self.download_error("n challenge failed")
        with tempfile.TemporaryDirectory() as root:
            section = self.make_player_cache(root, 2)
            with self.assertRaises(self.download_error):
                self.media("https://youtu.be/12345678901", True, root)
            self.assertEqual([path.name for path in section.iterdir()], ["lib.json"])

    def test_metadata_title_artist_and_thumbnail_order(self):
        page = "https://i.ytimg.com/vi/ID/hqdefault.jpg?sqp=abc"
        self.downloader.extract_info.return_value = {
            "url": "https://cdn.example/audio",
            "title": "  Big\nBuck\tBunny  ",
            "channel": "Blender",
            "uploader": "ignored",
            "thumbnail": "https://i.ytimg.com/vi_webp/ID/maxresdefault.webp",
            "thumbnails": [
                {"url": "https://i.ytimg.com/vi/ID/default.jpg", "width": 120},
                {"url": page, "width": 480},
                {"url": "https://i.ytimg.com/vi/ID/maxresdefault.jpg", "width": 1280},
                {"url": "https://i.ytimg.com/vi/ID/sddefault.jpg?x=1", "width": 640},
                {"url": "https://i.ytimg.com/vi/ID/hqdefault.jpg"},
                {"url": "http://insecure.example/x.jpg", "width": 600},
                {"url": 5},
            ],
        }
        media = self.media("https://youtu.be/12345678901", True)
        self.assertEqual(media["title"], "Big Buck Bunny")
        self.assertEqual(media["artist"], "Blender")
        self.assertEqual(media["thumbnails"], [
            "https://i.ytimg.com/vi/ID/sddefault.jpg?x=1",
            page,
            "https://i.ytimg.com/vi/ID/hqdefault.jpg",
        ])

    def test_metadata_falls_back_to_uploader_and_tolerates_missing_fields(self):
        self.downloader.extract_info.return_value = {
            "url": "https://cdn.example/audio", "uploader": "Uploader", "title": 7}
        media = self.media("https://youtu.be/12345678901", True)
        self.assertEqual((media["title"], media["artist"], media["thumbnails"]),
                         ("", "Uploader", []))

    def warm_player(self, player_url, cached):
        extractor = self.downloader.get_info_extractor.return_value
        extractor._download_player_url.return_value = player_url
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/audio"}
        with patch.object(self.runner, "_enable_player_cache", return_value=True), \
                patch.object(self.runner, "_player_cached", side_effect=cached) as check:
            result = self.runner.warm_player(self.runtime_path, "/cache/yt-dlp")
        return result, check

    def test_warm_player_skips_extraction_when_player_is_cached(self):
        result, check = self.warm_player("https://www.youtube.com/s/player/abc/base.js", [True])
        self.assertEqual(result, "al dia")
        check.assert_called_once_with(self.downloader, "https://www.youtube.com/s/player/abc/base.js")
        self.downloader.extract_info.assert_not_called()

    def test_warm_player_extracts_short_video_for_new_player(self):
        result, _ = self.warm_player("https://www.youtube.com/s/player/new/base.js", [False, True])
        self.assertEqual(result, "preparado")
        self.downloader.extract_info.assert_called_once_with(
            self.runner._PLAYER_WARMUP_URL, download=False)
        self.assertEqual(self.factory.call_args.args[0]["format"], self.runner._AUDIO_FORMAT)

    def test_warm_player_without_player_url_or_cache_does_nothing(self):
        result, check = self.warm_player(None, [])
        self.assertEqual(result, "sin respuesta de YouTube")
        check.assert_not_called()
        self.assertEqual(self.runner.warm_player(self.runtime_path, None), "sin cache")
        self.downloader.extract_info.assert_not_called()

    def updater(self, version="2026.11.2", pending=True, online=True):
        updater = MagicMock()
        updater.trial_pending.return_value = pending
        updater.active.return_value = version
        updater.online.return_value = online
        certifi = MagicMock()
        certifi.where.return_value = "ca.pem"
        modules = patch.dict(sys.modules, {"yt_dlp_update": updater, "certifi": certifi})
        modules.start()
        self.addCleanup(modules.stop)
        return updater

    def test_verify_update_confirms_after_real_extraction(self):
        updater = self.updater()
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/a"}
        self.assertEqual(self.runner.verify_update("root", self.runtime_path), "yt-dlp 2026.11.2 verificado")
        updater.confirm.assert_called_once_with("root")
        self.downloader.extract_info.assert_called_once()
        self.assertEqual(self.downloader.extract_info.call_args.args[0], self.runner._PLAYER_WARMUP_URL)

    def test_verify_update_rejects_failure_only_when_online(self):
        updater = self.updater()
        self.downloader.extract_info.side_effect = self.download_error("firma rota")
        message = self.runner.verify_update("root", self.runtime_path)
        self.assertIn("firma rota", message)
        updater.reject.assert_called_once_with("root", "2026.11.2")

        updater = self.updater(online=False)
        self.assertIn("sin red", self.runner.verify_update("root", self.runtime_path))
        updater.reject.assert_not_called()
        updater.confirm.assert_not_called()

    def test_verify_update_without_trial_does_nothing(self):
        self.updater(pending=False)
        self.assertEqual(self.runner.verify_update("root", self.runtime_path), "")
        self.downloader.extract_info.assert_not_called()

    def test_title_is_limited(self):
        self.downloader.extract_info.return_value = {
            "url": "https://cdn.example/audio", "title": "x" * 500}
        self.assertEqual(len(self.media("https://youtu.be/12345678901", True)["title"]), 200)


if __name__ == "__main__":
    unittest.main()
