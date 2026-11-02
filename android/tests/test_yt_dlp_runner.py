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
        provider = types.ModuleType("anonymous_provider")
        provider.options = MagicMock(return_value={
            "js_runtimes": {"node": {"path": "/native/libnode.so"}},
            "extractor_args": {
                "youtube": {"player_client": ["default"]},
                "youtubepot-bgutilscript": {"server_home": ["/private/server"]},
            },
        })
        self.provider = provider
        with patch.dict(sys.modules, {
            "yt_dlp": dependency, "yt_dlp.utils": utils, "anonymous_provider": provider,
        }):
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
                self.assertEqual(options["extract_flat"], "in_playlist")
                self.assertTrue(options["lazy_playlist"])
                self.assertTrue(options["ignoreerrors"])
                self.assertTrue(options["skip_download"])
                self.assertEqual(options["js_runtimes"], {
                    "quickjs": {"path": self.runtime_path},
                    "node": {"path": "/native/libnode.so"},
                })
                self.assertEqual(options["extractor_args"]["youtube"]["player_client"], ["default"])
                self.assertEqual(options["extractor_args"]["youtubepot-bgutilscript"],
                                 {"server_home": ["/private/server"]})
                self.assertEqual(options["remote_components"], set())
                self.assertIs(options["cachedir"], False)
                self.assertIs(options["cachedir"], False)
                self.downloader.extract_info.assert_called_with(
                    "https://www.youtube.com/watch?v=12345678901", download=False)

    def test_requested_direct_stream(self):
        self.downloader.extract_info.return_value = {
            "requested_downloads": [{
                "url": "https://cdn.example/audio",
                "ext": "webm",
                "protocol": "https",
            }]}
        media = self.media("https://youtube.com/watch?v=12345678901", True)
        self.assertEqual(media["url"], "https://cdn.example/audio")
        self.assertEqual(media["extension"], "webm")
        self.assertEqual(media["protocol"], "https")

    def test_all_sites_use_guest_downloader_without_account_credentials(self):
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/video"}
        urls = [
            "https://youtube.com/watch?v=12345678901",
            "https://x.com/user/status/1",
            "https://www.instagram.com/reel/abc/",
            "https://www.facebook.com/user/videos/1/",
            "https://public.example/video",
        ]
        for url in urls:
            with self.subTest(url=url):
                result = self.runner.extract_media(
                    url,
                    True,
                    self.runtime_path,
                )
                self.assertEqual(json.loads(result)["url"], "https://cdn.example/video")
                options = self.factory.call_args.args[0]
                for credential in ("cookiefile", "cookiesfrombrowser", "username", "password"):
                    self.assertNotIn(credential, options)
                self.assertFalse(options["usenetrc"])
        self.assertEqual(self.downloader.extract_info.call_count, len(urls))
        self.assertEqual(self.factory.call_count, 1)
        self.downloader.close.assert_not_called()

    def test_saved_cookie_argument_is_not_accepted(self):
        with self.assertRaises(TypeError):
            self.runner.extract_media("https://youtu.be/12345678901", True, self.runtime_path,
                                      cookie_file="/private/saved-cookies.txt")

    def test_missing_provider_is_an_explicit_error(self):
        self.provider.options.side_effect = RuntimeError("El proveedor PO local no esta inicializado")
        with self.assertRaisesRegex(RuntimeError, "proveedor PO"):
            self.resolve("https://youtu.be/12345678901", True)
        self.factory.assert_not_called()

    def fallback_bridge(self, cookie_file="/private/site-cookies.txt"):
        bridge = MagicMock()
        bridge.prepareAnonymousExtraction.return_value = ""
        bridge.prepareSupportedSiteCookiesForFallback.return_value = cookie_file
        return bridge

    def fallback(self, bridge):
        return self.runner.extract_media_with_fallback(
            "https://youtu.be/12345678901", True, self.runtime_path, None, bridge)

    def test_cancel_interrupts_a_stuck_extractor_and_allows_next_request(self):
        bridge = self.fallback_bridge()
        cancel = False
        bridge.isMediaExtractionCancelled.side_effect = lambda generation: cancel

        def stuck(*args, **kwargs):
            nonlocal cancel
            cancel = True
            while True:
                pass

        self.downloader.extract_info.side_effect = stuck
        previous = sys.gettrace()
        with self.assertRaisesRegex(RuntimeError, "cancelada por el usuario"):
            self.runner.extract_media_request(
                "https://youtu.be/12345678901", True, self.runtime_path, None,
                bridge, False, False, 1)
        self.assertIs(sys.gettrace(), previous)
        self.assertIsNone(self.runner._request)
        bridge.prepareSupportedSiteCookiesForFallback.assert_not_called()
        self.downloader.close.assert_called_once()
        self.downloader.extract_info.side_effect = None
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/next"}
        bridge.isMediaExtractionCancelled.side_effect = None
        bridge.isMediaExtractionCancelled.return_value = False
        result = self.runner.extract_media_request(
            "https://youtu.be/abcdefghijk", True, self.runtime_path, None,
            bridge, False, False, 2)
        self.assertEqual(json.loads(result)["url"], "https://cdn.example/next")

    def test_cancelled_account_extractor_still_closes_and_deletes_cookie_file(self):
        bridge = self.fallback_bridge()
        cancel = False
        bridge.isMediaExtractionCancelled.side_effect = lambda generation: cancel

        def stuck(*args, **kwargs):
            nonlocal cancel
            cancel = True
            while True:
                pass

        self.downloader.extract_info.side_effect = stuck
        with patch.object(self.runner.os, "remove") as remove:
            with self.assertRaisesRegex(RuntimeError, "cancelada"):
                self.runner.extract_media_request(
                    "https://youtu.be/12345678901", True, self.runtime_path, None,
                    bridge, True, True, 3)
        self.downloader.close.assert_called_once()
        remove.assert_called_once_with("/private/site-cookies.txt")

    def test_progress_reports_stages_without_forwarding_sensitive_extractor_text(self):
        bridge = self.fallback_bridge()
        bridge.isMediaExtractionCancelled.return_value = False

        def extract(*args, **kwargs):
            self.runner._errors.debug("[youtube] Downloading player https://host/?secret=value")
            self.runner._errors.warning("temporary error")
            return {"url": "https://cdn.example/video"}

        self.downloader.extract_info.side_effect = extract
        self.runner.extract_media_request(
            "https://youtu.be/12345678901", True, self.runtime_path, None,
            bridge, False, False, 4)
        messages = [call.args[1] for call in bridge.reportMediaExtractionProgress.call_args_list]
        self.assertTrue(any("JavaScript" in message for message in messages))
        self.assertTrue(any("sigue en curso" in message for message in messages))
        self.assertIn("Enlace resuelto; cargando en el player.", messages)
        self.assertFalse(any("secret" in message or "https://" in message for message in messages))

    def test_final_resolution_failure_is_reported_before_returning_error(self):
        bridge = self.fallback_bridge(cookie_file="")
        bridge.isMediaExtractionCancelled.return_value = False
        self.downloader.extract_info.return_value = None
        with self.assertRaisesRegex(RuntimeError, "No hay cookies"):
            self.runner.extract_media_request(
                "https://youtu.be/12345678901", True, self.runtime_path, None,
                bridge, False, False, 5)
        bridge.reportMediaExtractionProgress.assert_called_with(
            5, "No se pudo resolver el enlace. El proceso ha terminado.")

    def test_successful_guest_never_reads_cookies(self):
        bridge = self.fallback_bridge()
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/public"}
        self.assertEqual(json.loads(self.fallback(bridge))["url"], "https://cdn.example/public")
        bridge.prepareSupportedSiteCookiesForFallback.assert_not_called()
        bridge.setSupportedSiteCookiesFallbackStatus.assert_not_called()
        self.downloader.close.assert_not_called()

    def test_guest_failure_retries_once_with_isolated_account_downloader(self):
        bridge = self.fallback_bridge()
        authenticated = MagicMock()
        self.factory.side_effect = [self.downloader, authenticated]
        self.downloader.extract_info.side_effect = self.download_error("login required")
        authenticated.extract_info.return_value = {"url": "https://cdn.example/account"}
        with patch.object(self.runner.os, "remove") as remove:
            self.assertEqual(json.loads(self.fallback(bridge))["url"], "https://cdn.example/account")
        self.assertEqual(self.factory.call_count, 2)
        guest_options, account_options = [call.args[0] for call in self.factory.call_args_list]
        self.assertNotIn("cookiefile", guest_options)
        self.assertEqual(account_options["cookiefile"], "/private/site-cookies.txt")
        self.assertNotIn("extractor_args", account_options)
        bridge.setSupportedSiteCookiesFallbackStatus.assert_called_once_with("fallback")
        authenticated.extract_info.assert_called_once()
        authenticated.close.assert_called_once()
        self.downloader.close.assert_not_called()
        remove.assert_called_once_with("/private/site-cookies.txt")
        self.assertEqual(list(self.runner._downloaders.values()), [self.downloader])

    def test_both_failures_are_reported_and_cookie_file_is_cleaned(self):
        bridge = self.fallback_bridge()
        self.downloader.extract_info.side_effect = [
            self.download_error("guest blocked"), self.download_error("cookies expired")]
        with patch.object(self.runner.os, "remove") as remove:
            with self.assertRaisesRegex(RuntimeError, "anonimo: guest blocked.*cookies: cookies expired"):
                self.fallback(bridge)
        self.assertEqual(self.downloader.extract_info.call_count, 2)
        self.downloader.close.assert_called_once()
        remove.assert_called_once()

    def test_empty_extractor_result_triggers_cookie_fallback(self):
        bridge = self.fallback_bridge()
        self.downloader.extract_info.side_effect = [None, {"url": "https://cdn.example/account"}]
        with patch.object(self.runner.os, "remove"):
            self.assertEqual(json.loads(self.fallback(bridge))["url"], "https://cdn.example/account")
        bridge.prepareSupportedSiteCookiesForFallback.assert_called_once()

    def test_no_matching_cookies_reports_guest_failure_without_second_attempt(self):
        bridge = self.fallback_bridge("")
        self.downloader.extract_info.side_effect = self.download_error("guest blocked")
        with self.assertRaisesRegex(RuntimeError, "guest blocked.*No hay cookies"):
            self.fallback(bridge)
        self.assertEqual(self.factory.call_count, 1)
        bridge.setSupportedSiteCookiesFallbackStatus.assert_not_called()

    def test_provider_initialization_failure_still_allows_account_fallback(self):
        bridge = self.fallback_bridge()
        bridge.prepareAnonymousExtraction.return_value = "Provider unavailable"
        self.provider.options.side_effect = RuntimeError("Provider unavailable")
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/account"}
        with patch.object(self.runner.os, "remove"):
            self.assertEqual(json.loads(self.fallback(bridge))["url"], "https://cdn.example/account")
        self.provider.options.assert_not_called()
        self.assertEqual(self.factory.call_count, 1)
        self.downloader.close.assert_called_once()
        self.assertEqual(self.runner._downloaders, {})

    def test_programming_error_does_not_use_account_credentials(self):
        bridge = self.fallback_bridge()
        self.downloader.extract_info.side_effect = TypeError("bug")
        with self.assertRaisesRegex(TypeError, "bug"):
            self.fallback(bridge)
        bridge.prepareSupportedSiteCookiesForFallback.assert_not_called()

    def test_account_constructor_failure_still_removes_decrypted_cookies(self):
        bridge = self.fallback_bridge()
        bridge.prepareAnonymousExtraction.return_value = "Provider unavailable"
        self.factory.side_effect = RuntimeError("constructor failed")
        with patch.object(self.runner.os, "remove") as remove:
            with self.assertRaisesRegex(RuntimeError, "constructor failed"):
                self.fallback(bridge)
        remove.assert_called_once()

    def test_invalid_input_never_attempts_provider_or_cookies(self):
        bridge = self.fallback_bridge()
        with self.assertRaises(RuntimeError):
            self.runner.extract_media_with_fallback(
                "file:///private/video", True, self.runtime_path, None, bridge)
        bridge.prepareAnonymousExtraction.assert_not_called()
        bridge.prepareSupportedSiteCookiesForFallback.assert_not_called()

    def test_temporary_cookie_cleanup_failure_is_not_hidden(self):
        bridge = self.fallback_bridge()
        bridge.prepareAnonymousExtraction.return_value = "Provider unavailable"
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/account"}
        with patch.object(self.runner.os, "remove", side_effect=OSError("denied")):
            with self.assertRaisesRegex(RuntimeError, "archivo temporal"):
                self.fallback(bridge)
        self.downloader.close.assert_called_once()

    def test_playback_cookie_retry_skips_guest_and_reports_account_provenance(self):
        bridge = self.fallback_bridge()
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/account", "id": "a"}
        with patch.object(self.runner.os, "remove") as remove:
            media = json.loads(self.runner.extract_media_with_fallback(
                "https://youtu.be/12345678901", True, self.runtime_path, None, bridge,
                cookies_only=True, refresh=True,
            ))
        bridge.prepareAnonymousExtraction.assert_not_called()
        self.assertTrue(media["used_cookies"])
        self.assertEqual(media["entry_id"], "a")
        self.assertEqual(self.factory.call_count, 1)
        self.assertEqual(self.runner._downloaders, {})
        remove.assert_called_once()

    def test_playback_refresh_discards_old_guest_before_resolving_again(self):
        bridge = self.fallback_bridge()
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/old"}
        self.fallback(bridge)
        fresh = MagicMock()
        fresh.extract_info.return_value = {"url": "https://cdn.example/new"}
        self.factory.return_value = fresh
        media = json.loads(self.runner.extract_media_with_fallback(
            "https://youtu.be/12345678901", True, self.runtime_path, None, bridge, refresh=True,
        ))
        self.assertEqual(media["url"], "https://cdn.example/new")
        self.assertFalse(media.get("used_cookies", False))
        self.downloader.close.assert_called_once()
        bridge.prepareSupportedSiteCookiesForFallback.assert_not_called()

    def test_playlist_identity_tracks_successful_entries_without_stream_url_tokens(self):
        self.downloader.extract_info.return_value = {
            "_type": "playlist",
            "entries": [
                {"id": "a", "url": "https://cdn.example/a?signature=secret"},
                None,
                {"id": "b", "url": "https://cdn.example/b?signature=secret"},
            ],
        }
        media = self.media("https://youtube.com/playlist?list=PL123", True)
        self.assertEqual(media["entry_ids"], ["a", "b"])

    def test_playlist_returns_bounded_stream_queue(self):
        self.downloader.extract_info.return_value = {
            "_type": "playlist",
            "entries": [
                {
                    "url": f"https://cdn.example/{index}",
                    "title": f"Track {index}",
                    "ext": "mp4",
                    "protocol": "https",
                }
                for index in range(3)
            ],
        }
        media = self.media("https://youtube.com/playlist?list=PL123", False)
        self.assertEqual(media["url"], "https://cdn.example/0")
        self.assertEqual(
            media["playlist"],
            ["https://cdn.example/1", "https://cdn.example/2"],
        )
        self.assertEqual(media["playlist_extensions"], ["mp4", "mp4"])
        self.assertEqual(media["playlist_protocols"], ["https", "https"])
        self.assertEqual(self.downloader.extract_info.call_count, 1)

    def test_youtube_shared_links_preserve_playlist_and_music_context(self):
        cases = [
            ("https://youtu.be/abcdefghijk?list=RDMM&index=4",
             "https://www.youtube.com/watch?v=abcdefghijk&list=RDMM&index=4"),
            ("https://music.youtube.com/shorts/abcdefghijk?list=RDAMVMabcdefghijk",
             "https://music.youtube.com/watch?v=abcdefghijk&list=RDAMVMabcdefghijk"),
            ("https://www.youtube.com/embed/videoseries?list=PL123",
             "https://www.youtube.com/playlist?list=PL123"),
            ("https://youtube.com/live/abcdefghijk?list=PL123",
             "https://www.youtube.com/watch?v=abcdefghijk&list=PL123"),
            ("https://youtube.com/attribution_link?u=%2Fwatch%3Fv%3Dabcdefghijk%26list%3DPL123",
             "https://www.youtube.com/watch?v=abcdefghijk&list=PL123"),
            ("https://music.youtube.com/browse/MPREb123",
             "https://music.youtube.com/browse/MPREb123"),
            ("https://music.youtube.com/browse/VLPL123",
             "https://music.youtube.com/browse/VLPL123"),
        ]
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/a"}
        for original, normalized in cases:
            with self.subTest(original=original):
                self.media(original, True)
                self.downloader.extract_info.assert_called_with(normalized, download=False)
        with self.assertRaisesRegex(RuntimeError, "no apunta a YouTube"):
            self.media("https://youtube.com/redirect?q=https://other.example/", True)

    def test_seeded_radio_playlist_uses_watch_route_without_losing_context(self):
        for prefix in ("RD", "RDAMVM"):
            original = (
                f"https://music.youtube.com/playlist?list={prefix}UsR08cY8k0A"
                "&playnext=1&index=3&si=shared")
            self.assertEqual(self.runner._normalize_youtube_url(original), (
                f"https://music.youtube.com/watch?v=UsR08cY8k0A&list={prefix}UsR08cY8k0A"
                "&playnext=1&index=3&si=shared"))
        for list_id in ("RDMM", "RDTMAK123", "RDCLAK123", "RDabcdefghij!", "PLUsR08cY8k0A"):
            url = f"https://youtube.com/playlist?list={list_id}"
            self.assertEqual(self.runner._normalize_youtube_url(url), url)

    def test_personal_playlist_uses_account_from_start_without_guest_provider(self):
        bridge = self.fallback_bridge()
        self.downloader.extract_info.return_value = {
            "_type": "playlist",
            "entries": [{"id": "abcdefghijk", "url": "https://cdn.example/account"}],
        }
        with patch.object(self.runner.os, "remove") as remove:
            media = json.loads(self.runner.extract_media_with_fallback(
                "https://music.youtube.com/watch?v=abcdefghijk&list=RDMM",
                True, self.runtime_path, None, bridge, cookies_only=True))
        bridge.prepareAnonymousExtraction.assert_not_called()
        bridge.setSupportedSiteCookiesFallbackStatus.assert_called_once_with("session")
        self.assertTrue(media["used_cookies"])
        self.assertIn("1 reproducibles", media["playlist_notice"])
        self.assertEqual(self.factory.call_args.args[0]["cookiefile"], "/private/site-cookies.txt")
        self.downloader.close.assert_called_once()
        remove.assert_called_once()

    def test_personal_playlist_without_cookies_never_substitutes_anonymous_mix(self):
        bridge = self.fallback_bridge("")
        with self.assertRaisesRegex(RuntimeError, "No se sustituye una lista personal"):
            self.runner.extract_media_with_fallback(
                "https://music.youtube.com/playlist?list=RDMM",
                True, self.runtime_path, None, bridge, cookies_only=True)
        bridge.prepareAnonymousExtraction.assert_not_called()
        self.factory.assert_not_called()

    def test_personal_playlist_rejects_cookie_jar_without_youtube_authentication(self):
        bridge = self.fallback_bridge()
        self.downloader.get_info_extractor.return_value.is_authenticated = False
        self.downloader.extract_info.return_value = {"url": "https://cdn.example/guest"}
        with patch.object(self.runner.os, "remove") as remove:
            with self.assertRaisesRegex(RuntimeError, "sesion de YouTube"):
                self.runner.extract_media_with_fallback(
                    "https://music.youtube.com/playlist?list=RDMM",
                    True, self.runtime_path, None, bridge, cookies_only=True)
        self.downloader.extract_info.assert_not_called()
        bridge.prepareAnonymousExtraction.assert_not_called()
        self.downloader.close.assert_called_once()
        remove.assert_called_once()

    def test_personal_playlist_rejects_session_lost_during_extraction(self):
        bridge = self.fallback_bridge()
        extractor = self.downloader.get_info_extractor.return_value
        extractor.is_authenticated = True

        def expired(*_args, **_kwargs):
            extractor.is_authenticated = False
            return {"url": "https://cdn.example/guest"}

        self.downloader.extract_info.side_effect = expired
        with patch.object(self.runner.os, "remove") as remove:
            with self.assertRaisesRegex(RuntimeError, "sesion de YouTube"):
                self.runner.extract_media_with_fallback(
                    "https://music.youtube.com/playlist?list=RDMM",
                    True, self.runtime_path, None, bridge, cookies_only=True)
        bridge.prepareAnonymousExtraction.assert_not_called()
        self.downloader.close.assert_called_once()
        remove.assert_called_once()

    def test_personal_playlist_rejects_rotated_cookie_warning_even_with_auth_cookies(self):
        bridge = self.fallback_bridge()
        self.downloader.get_info_extractor.return_value.is_authenticated = True

        def rotated(*_args, **_kwargs):
            self.runner._errors.warning(
                "[youtube:tab] The provided YouTube account cookies are no longer valid. "
                "They have likely been rotated in the browser as a security measure.")
            return {"url": "https://cdn.example/guest"}

        self.downloader.extract_info.side_effect = rotated
        with patch.object(self.runner.os, "remove") as remove:
            with self.assertRaisesRegex(RuntimeError, "sesion de YouTube"):
                self.runner.extract_media_with_fallback(
                    "https://music.youtube.com/playlist?list=RDMM",
                    True, self.runtime_path, None, bridge, cookies_only=True)
        bridge.prepareAnonymousExtraction.assert_not_called()
        self.downloader.close.assert_called_once()
        remove.assert_called_once()

    def test_personal_playlist_failure_does_not_fall_back_to_guest(self):
        bridge = self.fallback_bridge()
        self.downloader.extract_info.side_effect = self.download_error("expired session")
        with patch.object(self.runner.os, "remove") as remove:
            with self.assertRaisesRegex(self.download_error, "expired session"):
                self.runner.extract_media_with_fallback(
                    "https://music.youtube.com/playlist?list=RDMM",
                    True, self.runtime_path, None, bridge, cookies_only=True)
        bridge.prepareAnonymousExtraction.assert_not_called()
        self.factory.assert_called_once()
        self.downloader.close.assert_called_once()
        remove.assert_called_once()

    def test_flat_radio_entry_is_resolved_without_recursing_into_its_playlist(self):
        self.downloader.extract_info.side_effect = [
            {"_type": "playlist", "entries": [
                {"_type": "url", "id": "abcdefghijk", "ie_key": "Youtube",
                 "url": "https://music.youtube.com/watch?v=abcdefghijk&list=RDMM"},
            ]},
            {"id": "abcdefghijk", "url": "https://cdn.example/a"},
        ]
        self.media("https://music.youtube.com/playlist?list=RDMM", True)
        self.downloader.extract_info.assert_called_with(
            "https://www.youtube.com/watch?v=abcdefghijk", download=False)

    def test_request_publishes_first_track_before_resolving_second_and_marks_final_complete(self):
        bridge = self.fallback_bridge()
        bridge.isMediaExtractionCancelled.return_value = False
        emitted = []
        bridge.reportMediaExtractionItem.side_effect = (
            lambda generation, payload: emitted.append((generation, json.loads(payload))))

        def extract(url, **_kwargs):
            if "list=" in url:
                return {"_type": "playlist", "entries": [
                    {"_type": "url", "id": "abcdefghijk"},
                    {"_type": "url", "id": "lmnopqrstuv"},
                ]}
            if "lmnopqrstuv" in url:
                self.assertEqual(len(emitted), 1)
                self.assertEqual(emitted[0][1]["url"], "https://cdn.example/abcdefghijk")
            return {
                "id": url.split("v=")[1],
                "url": "https://cdn.example/" + url.split("v=")[1],
                "ext": "m4a",
                "protocol": "https",
            }

        self.downloader.extract_info.side_effect = extract
        final = json.loads(self.runner.extract_media_request(
            "https://music.youtube.com/playlist?list=PL123", True,
            self.runtime_path, None, bridge, False, False, 42))
        self.assertEqual([generation for generation, _ in emitted], [42, 42])
        self.assertTrue(all(media["playlist_pending"] for _, media in emitted))
        self.assertEqual(len(emitted[-1][1]["playlist"]), 1)
        self.assertEqual(emitted[-1][1]["playlist_extensions"], ["m4a"])
        self.assertEqual(emitted[-1][1]["playlist_protocols"], ["https"])
        self.assertFalse(final.get("playlist_pending", False))
        self.assertEqual(final["entry_ids"], ["abcdefghijk", "lmnopqrstuv"])
        self.assertEqual(final["playlist_extensions"], ["m4a"])
        self.assertEqual(final["playlist_protocols"], ["https"])
        self.assertFalse(self.runner._streaming)

    def test_partial_guest_queue_never_switches_to_account_after_publication_error(self):
        bridge = self.fallback_bridge()
        bridge.isMediaExtractionCancelled.return_value = False
        bridge.reportMediaExtractionItem.side_effect = [None, RuntimeError("queue transport failed")]
        self.downloader.extract_info.return_value = {"_type": "playlist", "entries": [
            {"id": "a", "url": "https://cdn.example/a"},
            {"id": "b", "url": "https://cdn.example/b"},
        ]}
        with self.assertRaisesRegex(RuntimeError, "No se cambia de sesion"):
            self.runner.extract_media_request(
                "https://youtube.com/playlist?list=PL123", True,
                self.runtime_path, None, bridge, False, False, 42)
        bridge.prepareSupportedSiteCookiesForFallback.assert_not_called()

    def test_account_queue_checks_authentication_before_each_published_track(self):
        bridge = self.fallback_bridge()
        bridge.isMediaExtractionCancelled.return_value = False
        extractor = self.downloader.get_info_extractor.return_value
        extractor.is_authenticated = True
        emitted = []

        def publish(_generation, payload):
            emitted.append(json.loads(payload))
            extractor.is_authenticated = False

        bridge.reportMediaExtractionItem.side_effect = publish
        self.downloader.extract_info.return_value = {"_type": "playlist", "entries": [
            {"id": "a", "url": "https://cdn.example/a"},
            {"id": "b", "url": "https://cdn.example/b"},
        ]}
        with patch.object(self.runner.os, "remove") as remove:
            with self.assertRaisesRegex(RuntimeError, "sesion de YouTube"):
                self.runner.extract_media_request(
                    "https://music.youtube.com/playlist?list=RDMM", True,
                    self.runtime_path, None, bridge, True, False, 42)
        self.assertEqual(len(emitted), 1)
        self.assertTrue(emitted[0]["used_cookies"])
        bridge.prepareAnonymousExtraction.assert_not_called()
        remove.assert_called_once()
        self.assertIsNone(self.runner._account_downloader)

    def test_expired_account_is_rejected_before_enumerating_or_resolving_tracks(self):
        bridge = self.fallback_bridge()
        bridge.isMediaExtractionCancelled.return_value = False
        self.downloader.get_info_extractor.return_value.is_authenticated = True

        def entries():
            raise AssertionError("Invalid account must fail before enumerating its radio")
            yield

        def expired(*_args, **_kwargs):
            self.runner._errors.warning(
                "The provided YouTube account cookies are no longer valid.")
            return {"_type": "playlist", "entries": entries()}

        self.downloader.extract_info.side_effect = expired
        with patch.object(self.runner.os, "remove") as remove:
            with self.assertRaisesRegex(RuntimeError, "sesion de YouTube"):
                self.runner.extract_media_request(
                    "https://youtube.com/playlist?list=RDUsR08cY8k0A", True,
                    self.runtime_path, None, bridge, True, False, 42)
        self.assertEqual(self.downloader.extract_info.call_count, 1)
        bridge.reportMediaExtractionItem.assert_not_called()
        bridge.prepareAnonymousExtraction.assert_not_called()
        remove.assert_called_once()

    def test_playback_recovery_remains_atomic_without_partial_publication(self):
        bridge = self.fallback_bridge()
        bridge.isMediaExtractionCancelled.return_value = False
        self.downloader.extract_info.return_value = {"_type": "playlist", "entries": [
            {"id": "a", "url": "https://cdn.example/a"},
            {"id": "b", "url": "https://cdn.example/b"},
        ]}
        result = json.loads(self.runner.extract_media_request(
            "https://youtube.com/playlist?list=PL123", True,
            self.runtime_path, None, bridge, False, True, 42))
        bridge.reportMediaExtractionItem.assert_not_called()
        self.assertEqual(len(result["playlist"]), 1)

    def test_cancel_during_playlist_discards_the_guest_before_the_next_url(self):
        bridge = self.fallback_bridge()
        bridge.isMediaExtractionCancelled.return_value = False
        bridge.reportMediaExtractionItem.side_effect = [None, self.runner.ExtractionCancelled()]
        self.downloader.extract_info.return_value = {"_type": "playlist", "entries": [
            {"id": "a", "url": "https://cdn.example/a"},
            {"id": "b", "url": "https://cdn.example/b"},
        ]}
        with self.assertRaisesRegex(RuntimeError, "cancelada"):
            self.runner.extract_media_request(
                "https://youtube.com/playlist?list=PL123", True,
                self.runtime_path, None, bridge, False, False, 42)
        self.assertEqual(self.runner._downloaders, {})
        self.downloader.close.assert_called_once()
        fresh = MagicMock()
        fresh.extract_info.return_value = {"url": "https://cdn.example/next"}
        self.factory.return_value = fresh
        self.assertEqual(self.media("https://youtu.be/abcdefghijk", True)["url"],
                         "https://cdn.example/next")

    def test_unbounded_mix_is_only_consumed_up_to_100_entries(self):
        consumed = []

        def entries():
            for index in range(100):
                consumed.append(index)
                yield {"id": str(index), "url": f"https://cdn.example/{index}"}
            raise AssertionError("Must not enumerate an endless mix beyond the limit")

        self.downloader.extract_info.return_value = {"_type": "playlist", "entries": entries()}
        media = self.media("https://music.youtube.com/playlist?list=RDMM", True)
        self.assertEqual(len(consumed), 100)
        self.assertEqual(len(media["playlist"]), 99)
        self.assertEqual(len(media["entry_ids"]), 100)
        self.assertIn("100 reproducibles", media["playlist_notice"])
        self.assertIn("Limite: 100", media["playlist_notice"])

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
