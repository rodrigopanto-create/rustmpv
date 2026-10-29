"""Chaquopy entry point for the original yt-dlp Python package."""

from functools import cache
from itertools import islice
import json
import os
import subprocess
import time
from urllib.parse import urlsplit

from yt_dlp import YoutubeDL
from yt_dlp.utils import DownloadError


def _validate_stream_url(value: object) -> str:
    if not isinstance(value, str) or not value:
        raise RuntimeError("yt-dlp no devolvio una URL de stream reproducible")
    parsed = urlsplit(value)
    if (
        parsed.scheme not in ("http", "https")
        or not parsed.hostname
        or parsed.username is not None
        or parsed.password is not None
        or any(
            character.isspace() or ord(character) < 32 or ord(character) == 127
            for character in value
        )
    ):
        raise RuntimeError("yt-dlp devolvio una URL de stream no permitida")
    return value


@cache
def _check_js_runtime(runtime_path: str) -> None:
    if not os.path.isabs(runtime_path) or not os.path.isfile(runtime_path):
        raise RuntimeError("No se encuentra QuickJS-ng en nativeLibraryDir")
    try:
        result = subprocess.run(
            [runtime_path, "--eval", 'console.log("visualrustmpv-quickjs-ok")'],
            capture_output=True,
            text=True,
            timeout=10,
            check=True,
        )
    except (OSError, subprocess.SubprocessError) as error:
        raise RuntimeError(f"No se pudo ejecutar QuickJS-ng: {error}") from error
    if result.stdout.strip() != "visualrustmpv-quickjs-ok":
        raise RuntimeError("QuickJS-ng devolvio una respuesta de comprobacion invalida")
    print("[JavaScript] QuickJS-ng verificado en nativeLibraryDir", flush=True)


_AUDIO_FORMAT = "bestaudio[ext=m4a]/bestaudio[ext=webm]/bestaudio/best"
# X/Twitter, Instagram and Facebook may lack an audio-only format or a known
# height; progressive HTTP(S) MP4 keeps audio and video in one libmpv stream.
_VIDEO_FORMAT = (
    "best[ext=mp4][height<=720][protocol^=http]/best[ext=mp4][height<=720]"
    "/best[height<=720]/best"
)
_PLAYLIST_LIMIT = 100
_PLAYER_CACHE_SECTION = "challenge-solver"
# yt-dlp stores key "player:<url>" percent-encoded with "%" replaced by ",".
_PLAYER_CACHE_PREFIX = "player,3A"
_PLAYER_CACHE_LIMIT = 2

# Long-lived downloaders keyed by (audio_only, runtime_path, cache_dir). Their
# YouTube extractor keeps player code and solved n/sig challenges in memory.
_downloaders: dict[tuple[bool, str, str | None], YoutubeDL] = {}

# yt-dlp reports these when a post exists but is hidden from anonymous visitors.
_LOGIN_REQUIRED_HINTS = (
    "no video could be found",
    "empty media response",
    "login",
    "log in",
    "cookies",
    "private",
    "age-restricted",
    "age restricted",
    "sensitive",
)


class _ErrorLog:
    """yt-dlp logger; ignoreerrors hides failures as None, so keep the last one."""

    def __init__(self) -> None:
        self.last: str | None = None

    def debug(self, message: str) -> None:
        pass

    def info(self, message: str) -> None:
        pass

    def warning(self, message: str) -> None:
        print(f"[yt-dlp] {message}", flush=True)

    def error(self, message: str) -> None:
        self.last = message
        print(f"[yt-dlp] {message}", flush=True)


_errors = _ErrorLog()


def _failure_message(error: str | None) -> str:
    if not error:
        return "yt-dlp no devolvio informacion de medios valida"
    text = _clean_text(error.removeprefix("ERROR:"))
    # Keep the extractor's first sentence; the rest is generic yt-dlp advice.
    reason = text.split(". ")[0].rstrip(".")
    if any(hint in reason.lower() for hint in _LOGIN_REQUIRED_HINTS):
        return (
            "La publicacion no es publica o requiere iniciar sesion "
            f"(privada, sensible o con restriccion de edad): {reason}"
        )
    return f"yt-dlp no pudo extraer el medio: {reason}"


def _options(audio_only: bool, runtime_path: str, cache_dir: str | None) -> dict:
    return {
        "logger": _errors,
        "noplaylist": False,
        "playlistend": _PLAYLIST_LIMIT,
        "quiet": True,
        "skip_download": True,
        "ignoreerrors": True,
        "socket_timeout": 10,
        "retries": 1,
        "extractor_retries": 1,
        "js_runtimes": {"quickjs": {"path": runtime_path}},
        "remote_components": set(),
        "cachedir": cache_dir if cache_dir else False,
        "format": _AUDIO_FORMAT if audio_only else _VIDEO_FORMAT,
    }


def _downloader(audio_only: bool, runtime_path: str, cache_dir: str | None) -> YoutubeDL:
    key = (audio_only, runtime_path, cache_dir)
    downloader = _downloaders.get(key)
    if downloader is None:
        if cache_dir:
            _enable_player_cache()
        downloader = YoutubeDL(_options(audio_only, runtime_path, cache_dir))
        downloader.get_info_extractor("Youtube")
        _downloaders[key] = downloader
    return downloader


@cache
def _enable_player_cache() -> bool:
    # yt-dlp-ejs preprocesses the whole YouTube player in QuickJS on every call
    # (~30 s on a phone). Its preprocessed-player cache is off upstream only because
    # files are large and never rotated; _prune_player_cache handles rotation here.
    try:
        from yt_dlp.extractor.youtube.jsc._builtin.ejs import EJSBaseJCP
    except ImportError as error:
        print(f"[yt-dlp] Cache del player no disponible: {error}", flush=True)
        return False
    EJSBaseJCP._ENABLE_PREPROCESSED_PLAYER_CACHE = True
    return True


def _prune_player_cache(cache_dir: str | None, keep: int = _PLAYER_CACHE_LIMIT) -> None:
    if not cache_dir:
        return
    section = os.path.join(cache_dir, _PLAYER_CACHE_SECTION)
    try:
        players = [entry for entry in os.scandir(section)
                   if entry.is_file() and entry.name.startswith(_PLAYER_CACHE_PREFIX)]
    except OSError:
        return
    players.sort(key=lambda entry: entry.stat().st_mtime, reverse=True)
    for entry in players[keep:]:
        try:
            os.remove(entry.path)
        except OSError as error:
            print(f"[yt-dlp] No se pudo borrar {entry.name}: {error}", flush=True)


def _discard_downloader(audio_only: bool, runtime_path: str, cache_dir: str | None) -> None:
    downloader = _downloaders.pop((audio_only, runtime_path, cache_dir), None)
    if downloader is not None:
        try:
            downloader.close()
        except Exception as error:
            print(f"[yt-dlp] Cerrar instancia descartada: {error}", flush=True)


def warm_up(runtime_path: str, cache_dir: str | None = None) -> float:
    """Prepare QuickJS and both persistent downloaders before the first Play."""
    started = time.monotonic()
    _check_js_runtime(runtime_path)
    for audio_only in (True, False):
        _downloader(audio_only, runtime_path, cache_dir)
    _prune_player_cache(cache_dir)
    elapsed = time.monotonic() - started
    print(f"[yt-dlp] Worker precargado en {elapsed:.2f} s", flush=True)
    return elapsed


# Short, long-lived public video used only to make yt-dlp preprocess a new player.
_PLAYER_WARMUP_URL = "https://www.youtube.com/watch?v=jNQXAC9IVRw"


def _player_cached(downloader: YoutubeDL, player_url: str) -> bool:
    cache_file = downloader.cache._get_cache_fn(
        _PLAYER_CACHE_SECTION, f"player:{player_url}", "json")
    return os.path.isfile(cache_file)


def warm_player(runtime_path: str, cache_dir: str | None = None) -> str:
    """Preprocess YouTube's current player once, so a rotation never delays Play.

    Asking iframe_api for the player id costs a few KB. Only when that player is
    missing from the disk cache does this run one full extraction (~30 s in QuickJS).
    """
    if not cache_dir or not _enable_player_cache():
        return "sin cache"
    started = time.monotonic()
    downloader = _downloader(True, runtime_path, cache_dir)
    player_url = downloader.get_info_extractor("Youtube")._download_player_url("player-warmup")
    if not player_url:
        return "sin respuesta de YouTube"
    if _player_cached(downloader, player_url):
        result = "al dia"
    else:
        extract_media(_PLAYER_WARMUP_URL, True, runtime_path, cache_dir)
        result = "preparado" if _player_cached(downloader, player_url) else "otra variante"
    print(f"[yt-dlp] Player {result} en {time.monotonic() - started:.2f} s: {player_url}",
          flush=True)
    return result


_THUMBNAIL_LIMIT = 3
_TEXT_LIMIT = 200


def version_label() -> str:
    from yt_dlp.version import __version__
    import yt_dlp_update
    return f"{__version__} (actualizado)" if yt_dlp_update.active() else __version__


def verify_update(update_root: str, runtime_path: str, cache_dir: str | None = None) -> str:
    """Confirm or reject a freshly downloaded yt-dlp with one real extraction."""
    import certifi
    import yt_dlp_update
    if not yt_dlp_update.trial_pending(update_root):
        return ""
    version = yt_dlp_update.active()
    try:
        extract_media(_PLAYER_WARMUP_URL, True, runtime_path, cache_dir)
    except Exception as error:
        if not yt_dlp_update.online(certifi.where()):
            return f"yt-dlp {version}: verificacion pospuesta (sin red)"
        yt_dlp_update.reject(update_root, version)
        return (f"yt-dlp {version} fallo ({_clean_text(str(error))}); "
                "al reiniciar se usara la version incluida")
    yt_dlp_update.confirm(update_root)
    return f"yt-dlp {version} verificado"


def check_update(update_root: str, apk_stamp: str, force: bool = False) -> str:
    import certifi
    import yt_dlp_update
    from yt_dlp.version import __version__
    return yt_dlp_update.check(update_root, apk_stamp, __version__, certifi.where(), force=force)


def _clean_text(value: object) -> str:
    if not isinstance(value, str):
        return ""
    text = "".join(" " if ord(c) < 32 or ord(c) == 127 else c for c in value)
    return " ".join(text.split())[:_TEXT_LIMIT]


def _thumbnail_candidates(info: dict) -> list[str]:
    """Order artwork so verified page thumbnails come before guessed CDN names."""
    thumbnails = [t for t in info.get("thumbnails") or []
                  if isinstance(t, dict) and isinstance(t.get("url"), str)]
    # Entries with a width come from the YouTube page itself; guessed names such as
    # maxresdefault may 404. A notification never needs more than ~720 px.
    sized = sorted((t for t in thumbnails
                    if isinstance(t.get("width"), int) and 240 <= t["width"] <= 720),
                   key=lambda t: t["width"], reverse=True)
    ordered = [t["url"] for t in sized]
    ordered += [t["url"] for t in thumbnails if t["url"].split("?")[0].endswith("/hqdefault.jpg")]
    if isinstance(info.get("thumbnail"), str):
        ordered.append(info["thumbnail"])
    result: list[str] = []
    for candidate in ordered:
        try:
            valid = _validate_stream_url(candidate)
        except RuntimeError:
            continue
        if urlsplit(valid).scheme == "https" and valid not in result:
            result.append(valid)
        if len(result) == _THUMBNAIL_LIMIT:
            break
    return result


def extract_media(
    url: str, audio_only: bool, runtime_path: str, cache_dir: str | None = None
) -> str:
    """Resolve the direct stream plus lock-screen metadata as a JSON object."""
    _validate_stream_url(url)
    _check_js_runtime(runtime_path)
    started = time.monotonic()
    downloader = _downloader(audio_only, runtime_path, cache_dir)
    _errors.last = None
    try:
        info = downloader.extract_info(url, download=False)
    except DownloadError:
        # A stale preprocessed player must not break every later extraction.
        _prune_player_cache(cache_dir, keep=0)
        raise
    except BaseException:
        # Never reuse an instance whose extractor may have been left half-updated.
        _discard_downloader(audio_only, runtime_path, cache_dir)
        _prune_player_cache(cache_dir, keep=0)
        raise
    _prune_player_cache(cache_dir)
    print(
        f"[yt-dlp] Extraccion {'audio' if audio_only else 'video'} en "
        f"{time.monotonic() - started:.2f} s",
        flush=True,
    )
    if not isinstance(info, dict):
        raise RuntimeError(_failure_message(_errors.last))
    if info.get("_type") in ("playlist", "multi_video"):
        entries = info.get("entries")
        if entries is None:
            raise RuntimeError("yt-dlp devolvio una lista sin elementos")
        selected = list(islice(entries, _PLAYLIST_LIMIT + 1))
        if not selected:
            raise RuntimeError("La lista no contiene elementos reproducibles")
        if len(selected) > _PLAYLIST_LIMIT:
            print(f"[yt-dlp] Lista limitada a {_PLAYLIST_LIMIT} elementos", flush=True)
            selected = selected[:_PLAYLIST_LIMIT]
        media = []
        skipped = 0
        for index, entry in enumerate(selected, 1):
            if entry is None:
                skipped += 1
                continue
            if not isinstance(entry, dict):
                raise RuntimeError(f"El elemento {index} de la lista no es valido")
            try:
                if entry.get("_type") in ("url", "url_transparent"):
                    entry_url = next(
                        (
                            candidate
                            for candidate in (entry.get("webpage_url"), entry.get("url"))
                            if isinstance(candidate, str)
                            and candidate.startswith(("http://", "https://"))
                        ),
                        None,
                    )
                    if entry_url is None:
                        video_id = entry.get("id")
                        if (
                            entry.get("ie_key") not in (None, "Youtube")
                            or not isinstance(video_id, str)
                            or not video_id
                        ):
                            raise RuntimeError(
                                f"No se pudo resolver el elemento {index} de la lista"
                            )
                        entry_url = f"https://www.youtube.com/watch?v={video_id}"
                    entry = downloader.extract_info(entry_url, download=False)
            except (DownloadError, RuntimeError) as error:
                skipped += 1
                print(f"[yt-dlp] Se omite elemento {index}: {error}", flush=True)
                continue
            try:
                media.append(_media_payload(entry))
            except RuntimeError as error:
                skipped += 1
                print(f"[yt-dlp] Se omite elemento {index}: {error}", flush=True)
        if not media:
            raise RuntimeError("La lista no contiene elementos reproducibles")
        result = media[0]
        result["playlist"] = [item["url"] for item in media[1:]]
        print(
            f"[yt-dlp] Lista resuelta: {len(media)} elementos; "
            f"{skipped} omitidos",
            flush=True,
        )
        return json.dumps(result)
    return json.dumps(_media_payload(info))


def _media_payload(info: object) -> dict:
    if not isinstance(info, dict) or info.get("_type") in ("playlist", "multi_video", "url", "url_transparent"):
        raise RuntimeError("yt-dlp no devolvio un medio individual reproducible")
    requested = info.get("requested_downloads") or []
    if len(requested) > 1:
        raise RuntimeError("yt-dlp devolvio varios streams; se requiere un formato directo")
    stream_url = requested[0].get("url") if requested else info.get("url")
    return {
        "url": _validate_stream_url(stream_url),
        "title": _clean_text(info.get("title")),
        "artist": _clean_text(info.get("channel") or info.get("uploader")),
        "thumbnails": _thumbnail_candidates(info),
    }
