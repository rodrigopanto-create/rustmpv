"""Device-only HTTP recovery fixture. Never packaged in the application APK."""
import io
import json
import os
import tempfile
import threading
import time
import wave
import ssl
from urllib.error import HTTPError, URLError
from urllib.request import Request, urlopen
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

import yt_dlp_runner as runner

_original_extract = runner._extract_media
_original_fallback = runner.extract_media_with_fallback
_original_playlist_limit = runner._PLAYLIST_LIMIT
_lock = threading.Lock()
_state = {}

_audio = io.BytesIO()
with wave.open(_audio, "wb") as audio:
    audio.setnchannels(1)
    audio.setsampwidth(2)
    audio.setframerate(8000)
    audio.writeframes(b"\0\0" * 8000 * 30)
_wav = _audio.getvalue()
_short_audio = io.BytesIO()
with wave.open(_short_audio, "wb") as audio:
    audio.setnchannels(1)
    audio.setsampwidth(2)
    audio.setframerate(8000)
    audio.writeframes(b"\0\0" * 8000 * 5)
_short_wav = _short_audio.getvalue()


class Handler(BaseHTTPRequestHandler):
    def do_GET(self):
        denied = self.path == "/denied"
        content = _short_wav if self.path == "/short" else _wav
        with _lock:
            key = "denied" if denied else "served"
            _state[key] += 1
        self.send_response(403 if denied else 200)
        self.send_header("Content-Type", "audio/wav")
        self.send_header("Content-Length", str(0 if denied else len(content)))
        self.end_headers()
        if not denied:
            self.wfile.write(content)

    def log_message(self, *_args):
        pass


_server = ThreadingHTTPServer(("127.0.0.1", 0), Handler)
_thread = threading.Thread(target=_server.serve_forever, daemon=True)
_thread.start()
_base = f"http://127.0.0.1:{_server.server_port}"
_public_music = "https://music.youtube.com/playlist?list=PLt5yu3-wZAlQLfIN0MMgp0wVV6MP3bM4_"
_radio_urls = {
    "music_radio": "https://youtube.com/playlist?list=RDUsR08cY8k0A",
    "music_radio_second": "https://youtube.com/playlist?list=RD98WtmW-lfeE",
    "music_radio_screenshot": "https://youtube.com/playlist?list=RD3LXr89pMU2Q",
}


class Bridge:
    def prepareAnonymousExtraction(self):
        return ""

    def prepareSupportedSiteCookiesForFallback(self, _url):
        descriptor, path = tempfile.mkstemp(prefix="recovery-test-", suffix=".txt")
        with os.fdopen(descriptor, "w") as cookies:
            cookies.write("# Netscape HTTP Cookie File\n")
            cookies.write(".youtube.com\tTRUE\t/\tTRUE\t0\tLOGIN_INFO\tsynthetic\n")
            cookies.write(".youtube.com\tTRUE\t/\tTRUE\t0\tSAPISID\tsynthetic\n")
        with _lock:
            _state["cookie_file"] = path
        return path

    def setSupportedSiteCookiesFallbackStatus(self, _status):
        pass


def configure(scenario):
    runner._PLAYLIST_LIMIT = 2 if scenario.startswith("music_account") else _original_playlist_limit
    with _lock:
        _state.clear()
        _state.update(scenario=scenario, calls=[], denied=0, served=0, done=False)
    if scenario in ("music_session", "progressive_session"):
        return f"https://music.youtube.com/watch?v=RecoverTest1&probe={scenario}&list=RDMM"
    if scenario == "music_public":
        return _public_music
    if scenario.startswith("music_account"):
        return _radio_urls["music_radio_screenshot"]
    if scenario in _radio_urls:
        return _radio_urls[scenario]
    if scenario.startswith("music_compare"):
        return "https://www.youtube.com/watch?v=UsR08cY8k0A"
    if scenario.startswith("cross_site"):
        paths = {
            "cross_site_twitter": "https://x.com/test/status/1",
            "cross_site_instagram": "https://instagram.com/reel/test/",
            "cross_site_facebook": "https://facebook.com/test/videos/1/",
        }
        return paths.get(scenario, "https://youtube.com/watch?v=RecoverTest1") + (
            "&" if scenario not in paths else "?") + f"probe={scenario}"
    return f"https://www.youtube.com/watch?v=RecoverTest1&probe={scenario}"


def extract(url, audio_only, runtime_path, cache_dir, downloader=None):
    if "v=RecoverTest1&probe=" not in url and "probe=cross_site" not in url:
        return _original_extract(url, audio_only, runtime_path, cache_dir, downloader)
    with _lock:
        _state["calls"].append("guest" if downloader is None else "cookies")
        scenario = _state["scenario"]
        count = len(_state["calls"])
    if "progressive" in scenario:
        items = [{"url": _base + "/short", "title": "Playback recovery test", "entry_id": "short"}]
        runner._publish_playlist(items, 0, True)
        deadline = time.monotonic() + 30
        try:
            while True:
                with _lock:
                    released = _state.get("release", False)
                if released:
                    break
                if time.monotonic() >= deadline:
                    raise RuntimeError("Progressive probe never observed early playback")
                time.sleep(0.05)
            # The first item ends before the next arrives: the service must survive the gap.
            time.sleep(6)
            items.append({"url": _base + "/audio", "entry_id": "RecoverTest1"})
            runner._publish_playlist(items, 0, True)
            result = dict(items[0], playlist=[items[1]["url"]],
                          entry_ids=["short", "RecoverTest1"],
                          playlist_notice="Lista: 2 reproducibles, 0 omitidos. Limite: 100 por carga.")
            with _lock:
                _state["done"] = True
            return json.dumps(result)
        except runner.ExtractionCancelled:
            with _lock:
                _state["cancelled"] = True
            raise
    if scenario == "resolve_cancel":
        try:
            while True:
                pass
        finally:
            with _lock:
                _state["cancelled"] = True
    if (scenario.endswith("cancel") or scenario.endswith("pause")) and count == 2:
        # Stop arrives while the worker is still producing its refresh result.
        time.sleep(3)
    denied = scenario.endswith("exhaust") or count == 1 or (scenario.endswith("cookies") and count == 2)
    payload = {
        "url": _base + ("/denied" if denied else "/audio"),
        "title": "Playback recovery test",
        "entry_id": "RecoverTest1",
    }
    if scenario.startswith("cross_site"):
        payload["url"] = _base + "/audio"
    if scenario.startswith("background") and count == 1:
        # A working first item starts the service; the second fails only after screen-off.
        payload.update(url=_base + "/short", playlist=[_base + "/denied"],
                       entry_ids=["short", "RecoverTest1"])
    if scenario == "music_session":
        payload.update(url=_base + "/short", playlist=[_base + "/audio"],
                       entry_ids=["short", "RecoverTest1"],
                       playlist_notice="Lista: 2 reproducibles, 1 omitidos. Limite: 100 por carga.")
    if scenario == "playlist_navigation":
        payload.update(url=_base + "/audio", playlist=[_base + "/audio", _base + "/audio"],
                       entry_ids=["RecoverTest1", "second", "third"],
                       playlist_notice="Lista: 3 reproducibles, 0 omitidos. Limite: 100 por carga.")
    result = json.dumps(payload)
    with _lock:
        _state["done"] = True
    return result


def fallback(url, audio_only, runtime_path, cache_dir, cookie_bridge,
             cookies_only=False, refresh=False):
    with _lock:
        scenario = _state.get("scenario", "")
    if scenario.startswith("music_account"):
        # Explicit opt-in diagnostic stays strict independently of automatic UI policy.
        cookies_only = True

        class AccountBridge:
            def prepareSupportedSiteCookiesForFallback(self, source):
                path = cookie_bridge.prepareSupportedSiteCookiesForFallback(source)
                with _lock:
                    _state["cookie_file"] = path
                return path

            def setSupportedSiteCookiesFallbackStatus(self, status):
                cookie_bridge.setSupportedSiteCookiesFallbackStatus(status)

        with _lock:
            _state["calls"].append("cookies")
        try:
            if scenario == "music_account_auth":
                check_account_authentication(AccountBridge())
            result = _original_fallback(
                url, audio_only, runtime_path, cache_dir, AccountBridge(), True, refresh)
        except (runner.DownloadError, RuntimeError):
            with _lock:
                _state["account_failed"] = True
                _state["authentication_rejected"] = runner._errors.invalid_youtube_session
            raise
        media = json.loads(result)
        if not media.get("used_cookies") or not media.get("playlist_notice"):
            raise RuntimeError("Account probe did not resolve an account playlist")
        with _lock:
            _state["entries"] = len(media["entry_ids"])
            _state["done"] = True
        return result
    if scenario.startswith("music_compare"):
        if cookies_only:
            raise RuntimeError("Comparison probe never opens saved cookies")
        error = runner._interruptible(cookie_bridge.prepareAnonymousExtraction)
        if error:
            raise RuntimeError(error)
        options = runner._options(audio_only, runtime_path, cache_dir)
        if scenario.endswith("default"):
            options["extractor_args"]["youtube"].pop("player_client")
        else:
            options["extractor_args"]["youtube"]["player_client"] = ["mweb"]
        if scenario.endswith("opus"):
            options["format"] = "bestaudio/best"
        with runner.YoutubeDL(options) as downloader:
            info = downloader.extract_info(url, download=False)
            if not isinstance(info, dict):
                raise RuntimeError("Comparison extractor returned no media")
            media = runner._media_payload(info)
            import certifi
            context = ssl.create_default_context(cafile=certifi.where())
            statuses = {}
            for name, headers in (
                ("default", {"Range": "bytes=0-0"}),
                ("extractor", {**(info.get("http_headers") or {}), "Range": "bytes=0-0"}),
                ("open_range", {"Range": "bytes=0-"}),
                ("no_range", {}),
                ("mpv_agent", {"User-Agent": "libmpv", "Range": "bytes=0-"}),
                ("extractor_open", {**(info.get("http_headers") or {}), "Range": "bytes=0-"}),
            ):
                try:
                    request = Request(media["url"], headers=headers)
                    with urlopen(request, timeout=15, context=context) as response:
                        statuses[name] = response.status
                        response.read(1)
                except HTTPError as error:
                    statuses[name] = error.code
                except URLError as error:
                    statuses[name] = str(error.reason)[:100]
            with _lock:
                _state["calls"].append("guest")
                _state["http_statuses"] = statuses
                _state["format"] = info.get("format_id")
                _state["chunk_size"] = info.get("downloader_options", {}).get("http_chunk_size")
                _state["done"] = True
            print("[TransportProbe] " + json.dumps({
                "format": info.get("format_id"), "statuses": statuses,
                "chunk_size": info.get("downloader_options", {}).get("http_chunk_size"),
            }), flush=True)
            return json.dumps(media)
    with _lock:
        public = (
            (_state.get("scenario") == "music_public" and url == _public_music)
            or url == _radio_urls.get(_state.get("scenario")))
    if public:
        if cookies_only:
            raise RuntimeError("Public Music probe must not use saved account cookies")
        error = runner._interruptible(cookie_bridge.prepareAnonymousExtraction)
        if error:
            raise RuntimeError(error)
        if scenario in _radio_urls:
            with _lock:
                _state["calls"].append("guest")
        result = runner._interruptible(
            lambda: _original_extract(
                runner._normalize_youtube_url(url), audio_only, runtime_path, cache_dir))
        media = json.loads(result)
        if not media.get("playlist_notice") or not media.get("entry_ids"):
            raise RuntimeError("Public Music probe did not resolve a playlist")
        with _lock:
            if scenario not in _radio_urls:
                _state["calls"].append("guest")
            _state["entries"] = len(media["entry_ids"])
            _state["done"] = True
        return result
    if "v=RecoverTest1&probe=" in url or "probe=cross_site" in url:
        cookie_bridge = Bridge()
    return _original_fallback(url, audio_only, runtime_path, cache_dir, cookie_bridge,
                              cookies_only, refresh)


def check_account_authentication(bridge):
    """Compare with a normal yt-dlp webpage request, outside the app resolver."""
    runner._errors.invalid_youtube_session = False
    path = bridge.prepareSupportedSiteCookiesForFallback("https://www.youtube.com/")
    if not path:
        raise RuntimeError("Account check has no saved YouTube cookies")
    try:
        with runner.YoutubeDL({
            "cookiefile": path, "quiet": True, "logger": runner._errors, "socket_timeout": 10,
        }) as downloader:
            extractor = downloader.get_info_extractor("YoutubeTab")
            extractor.initialize()
            before = extractor.is_authenticated
            webpage = extractor._download_webpage(
                "https://www.youtube.com/feed/library", "account-check")
            configuration = extractor.extract_ytcfg("account-check", webpage)
            logged_in = configuration.get("LOGGED_IN")
            after = extractor.is_authenticated
        with _lock:
            _state.update(
                auth_cookies_before_request=before,
                server_logged_in=logged_in,
                auth_cookies_after_request=after,
            )
        if not (before and logged_in is True and after):
            raise RuntimeError("YouTube did not confirm authentication in the baseline yt-dlp request")
    finally:
        os.remove(path)


def state():
    with _lock:
        result = dict(_state)
    result["cookie_deleted"] = not os.path.exists(result.get("cookie_file", ""))
    return json.dumps(result)


def release():
    with _lock:
        if _state["done"]:
            raise RuntimeError("Extraction finished before early playback was observed")
        _state["release"] = True


def close():
    runner._extract_media = _original_extract
    runner.extract_media_with_fallback = _original_fallback
    runner._PLAYLIST_LIMIT = _original_playlist_limit
    _server.shutdown()
    _server.server_close()
    _thread.join(timeout=5)
    with _lock:
        path = _state.get("cookie_file")
    if path and os.path.exists(path):
        os.remove(path)


def clean_history(files_dir):
    path = os.path.join(files_dir, "history.json")
    if not os.path.exists(path):
        return
    with open(path, encoding="utf-8") as source:
        history = json.load(source)
    history["entries"] = [
        entry for entry in history["entries"]
        if not (entry.get("title") == "Playback recovery test"
                and ("v=RecoverTest1&probe=" in entry.get("input", "")
                     or "probe=cross_site" in entry.get("input", "")))
    ]
    temporary = path + ".recovery-test"
    with open(temporary, "w", encoding="utf-8") as destination:
        json.dump(history, destination)
    os.replace(temporary, path)


runner._extract_media = extract
runner.extract_media_with_fallback = fallback
