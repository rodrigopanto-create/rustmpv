"""Chaquopy entry point for the original yt-dlp Python package."""

from functools import cache
from itertools import islice
import json
import os
import subprocess
import sys
import time
from typing import Callable, Protocol, TypeVar
from urllib.parse import parse_qsl, urlencode, urljoin, urlsplit, urlunsplit

from yt_dlp import YoutubeDL
from yt_dlp.utils import DownloadError
import anonymous_provider


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


def _youtube_host(host: str | None) -> bool:
    host = (host or "").lower().rstrip(".")
    return any(host == domain or host.endswith("." + domain)
               for domain in ("youtube.com", "youtube-nocookie.com", "youtu.be"))


def _normalize_youtube_url(url: str) -> str:
    for _ in range(3):
        parsed = urlsplit(url)
        if not _youtube_host(parsed.hostname):
            return url
        query = parse_qsl(parsed.query, keep_blank_values=True)
        if parsed.path == "/playlist" and not any(key == "v" for key, _ in query):
            playlist = next((value for key, value in query if key == "list"), "")
            for prefix in ("RDAMVM", "RD"):
                seed = playlist.removeprefix(prefix)
                if playlist.startswith(prefix) and len(seed) == 11 and all(
                    character.isascii() and (character.isalnum() or character in "-_")
                    for character in seed
                ):
                    return urlunsplit((
                        parsed.scheme, parsed.netloc, "/watch",
                        urlencode([("v", seed), *query]), parsed.fragment))
        if parsed.path in ("/attribution_link", "/redirect"):
            target = next((value for key, value in query if key in ("u", "q", "url")), "")
            target = urljoin("https://www.youtube.com/", target)
            if not _youtube_host(urlsplit(target).hostname):
                raise RuntimeError("El enlace compartido no apunta a YouTube")
            url = _validate_stream_url(target)
            continue
        parts = parsed.path.strip("/").split("/")
        short = (parsed.hostname or "").lower().rstrip(".") == "youtu.be"
        if short or (len(parts) == 2 and parts[0] in ("shorts", "live", "embed", "v")):
            video_id = parts[0] if short else parts[1]
            if video_id == "videoseries":
                query = [(key, value) for key, value in query if key != "v"]
                path = "/playlist"
            else:
                query = [(key, value) for key, value in query if key != "v"]
                query.insert(0, ("v", video_id))
                path = "/watch"
            host = "music.youtube.com" if parsed.hostname == "music.youtube.com" else "www.youtube.com"
            return urlunsplit(("https", host, path, urlencode(query), parsed.fragment))
        return url
    raise RuntimeError("Demasiadas redirecciones en el enlace compartido de YouTube")


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
_request: tuple["ExtractionBridge", int] | None = None
_streaming = False
_published = False
_account_downloader: YoutubeDL | None = None
_T = TypeVar("_T")


class ExtractionCancelled(BaseException):
    """Must not be swallowed by extractor errors or account fallback."""


class ExtractionBridge(Protocol):
    def isMediaExtractionCancelled(self, generation: int) -> bool: ...

    def reportMediaExtractionProgress(self, generation: int, message: str) -> None: ...

    def reportMediaExtractionItem(self, generation: int, media: str) -> None: ...


def _progress(message: str) -> None:
    print(f"[Proceso] {message}", flush=True)
    if _request is not None:
        bridge, generation = _request
        bridge.reportMediaExtractionProgress(generation, message)


def _interruptible(action: Callable[[], _T]) -> _T:
    if _request is None:
        return action()
    bridge, generation = _request
    previous = sys.gettrace()
    next_check = 0.0

    def check(frame, event, arg):
        nonlocal next_check
        now = time.monotonic()
        if now >= next_check:
            next_check = now + 0.1
            if bridge.isMediaExtractionCancelled(generation):
                raise ExtractionCancelled()
        return check

    try:
        sys.settrace(check)
        if bridge.isMediaExtractionCancelled(generation):
            raise ExtractionCancelled()
        return action()
    finally:
        sys.settrace(previous)


def extract_media_request(
    url: str, audio_only: bool, runtime_path: str, cache_dir: str | None,
    cookie_bridge, cookies_only: bool, refresh: bool, generation: int,
) -> str:
    global _request, _streaming, _published
    _request = (cookie_bridge, generation)
    _streaming = not refresh
    _published = False
    try:
        _progress("Preparando resolucion; las comprobaciones de JavaScript pueden tardar.")
        result = extract_media_with_fallback(
            url, audio_only, runtime_path, cache_dir, cookie_bridge, cookies_only, refresh)
        _progress("Enlace resuelto; cargando en el player.")
        return result
    except ExtractionCancelled as error:
        print("[Proceso] Resolucion cancelada por el usuario", flush=True)
        raise RuntimeError("Resolucion cancelada por el usuario") from error
    except Exception:
        _progress("No se pudo resolver el enlace. El proceso ha terminado.")
        raise
    finally:
        _request = None
        _streaming = False
        _published = False

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
        self.invalid_youtube_session = False

    def debug(self, message: str) -> None:
        if not message.startswith("[debug]"):
            self.info(message)

    def info(self, message: str) -> None:
        print(f"[yt-dlp] {message}", flush=True)
        lower = message.lower()
        # Show stages, never extractor URLs, signed query strings or credentials.
        if "po token" in lower:
            _progress("Generando o comprobando PO Token local...")
        elif any(word in lower for word in ("challenge", "javascript", "player")):
            _progress("Procesando el cliente y JavaScript del sitio...")
        elif any(word in lower for word in ("downloading", "extracting", "playlist")):
            _progress("Consultando el sitio y sus formatos de reproduccion...")

    def warning(self, message: str) -> None:
        if "youtube account cookies are no longer valid" in message.lower():
            self.invalid_youtube_session = True
        print(f"[yt-dlp] {message}", flush=True)
        _progress("El sitio dio un aviso; la resolucion sigue en curso.")

    def error(self, message: str) -> None:
        self.last = message
        print(f"[yt-dlp] {message}", flush=True)
        _progress("Fallo en este intento; comprobando si es posible el fallback.")


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


def _options(
    audio_only: bool, runtime_path: str, cache_dir: str | None,
    cookie_file: str | None = None,
) -> dict:
    options = {
        "logger": _errors,
        "noplaylist": False,
        "playlistend": _PLAYLIST_LIMIT,
        "extract_flat": "in_playlist",
        "lazy_playlist": True,
        "quiet": True,
        "skip_download": True,
        "ignoreerrors": True,
        "socket_timeout": 10,
        "retries": 1,
        "extractor_retries": 1,
        "js_runtimes": {"quickjs": {"path": runtime_path}},
        "remote_components": set(),
        "cachedir": cache_dir if cache_dir else False,
        "usenetrc": False,
        "format": _AUDIO_FORMAT if audio_only else _VIDEO_FORMAT,
    }
    if cookie_file:
        options["cookiefile"] = cookie_file
        return options
    provider_options = anonymous_provider.options()
    options["js_runtimes"].update(provider_options["js_runtimes"])
    options["extractor_args"] = provider_options["extractor_args"]
    return options


def _downloader(
    audio_only: bool,
    runtime_path: str,
    cache_dir: str | None,
) -> YoutubeDL:
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


def _discard_downloader(
    audio_only: bool,
    runtime_path: str,
    cache_dir: str | None,
) -> None:
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
    url: str,
    audio_only: bool,
    runtime_path: str,
    cache_dir: str | None = None,
) -> str:
    """Resolve anonymously; never load saved account cookies or credentials."""
    _validate_stream_url(url)
    try:
        return _extract_media(_normalize_youtube_url(url), audio_only, runtime_path, cache_dir)
    except ExtractionCancelled:
        _discard_downloader(audio_only, runtime_path, cache_dir)
        raise


class CookieFallbackBridge(Protocol):
    def prepareAnonymousExtraction(self) -> str: ...

    def prepareSupportedSiteCookiesForFallback(self, url: str) -> str: ...

    def setSupportedSiteCookiesFallbackStatus(self, status: str) -> None: ...


def extract_media_with_fallback(
    url: str,
    audio_only: bool,
    runtime_path: str,
    cache_dir: str | None,
    cookie_bridge: CookieFallbackBridge,
    cookies_only: bool = False,
    refresh: bool = False,
) -> str:
    """Try the guest first; decrypt account cookies only after extraction fails."""
    _validate_stream_url(url)
    url = _normalize_youtube_url(url)
    if cookies_only:
        _progress("Usando la sesion guardada para conservar la lista o renovar su stream...")
        return _account_media(url, audio_only, runtime_path, cache_dir, cookie_bridge)
    try:
        if refresh:
            _discard_downloader(audio_only, runtime_path, cache_dir)
        _progress("Preparando el intento anonimo y el proveedor local...")
        provider_error = _interruptible(cookie_bridge.prepareAnonymousExtraction)
        if provider_error:
            raise RuntimeError(provider_error)
        _progress("Resolviendo sin cuenta...")
        return _interruptible(lambda: extract_media(url, audio_only, runtime_path, cache_dir))
    except (DownloadError, RuntimeError) as anonymous_error:
        if _published:
            raise RuntimeError(
                "La cola quedo incompleta; se conservan los elementos ya resueltos. "
                f"No se cambia de sesion: {_clean_text(str(anonymous_error))}"
            ) from anonymous_error
        print(f"[yt-dlp] Intento anonimo fallo: {_clean_text(str(anonymous_error))}", flush=True)
        _progress("El intento anonimo fallo; comprobando cookies de este sitio...")
        try:
            return _account_media(
                url, audio_only, runtime_path, cache_dir, cookie_bridge, fallback=True)
        except (DownloadError, RuntimeError) as cookie_error:
            raise RuntimeError(
                f"Intento anonimo: {_clean_text(str(anonymous_error))}. "
                f"Intento con cookies: {_clean_text(str(cookie_error))}"
            ) from cookie_error


def _account_media(
    url: str, audio_only: bool, runtime_path: str, cache_dir: str | None,
    cookie_bridge: CookieFallbackBridge, fallback: bool = False,
) -> str:
    global _account_downloader
    cookie_file = cookie_bridge.prepareSupportedSiteCookiesForFallback(url)
    if not cookie_file:
        raise RuntimeError(
            "No hay cookies guardadas para este sitio; importa cookies.txt de tu cuenta. "
            "No se sustituye una lista personal por recomendaciones anonimas.")
    downloader = None
    try:
        cookie_bridge.setSupportedSiteCookiesFallbackStatus("fallback" if fallback else "session")
        _progress("Resolviendo con la sesion guardada; comprobando acceso a la lista...")
        downloader = YoutubeDL(_options(audio_only, runtime_path, cache_dir, cookie_file))
        require_youtube_session = not fallback and _youtube_host(urlsplit(url).hostname)
        _errors.invalid_youtube_session = False
        if require_youtube_session and not downloader.get_info_extractor("Youtube").is_authenticated:
            raise RuntimeError(
                "Las cookies no contienen una sesion de YouTube activa; "
                "exporta e importa cookies.txt de nuevo.")
        _account_downloader = downloader
        result = json.loads(_interruptible(
            lambda: _extract_media(url, audio_only, runtime_path, cache_dir, downloader)))
        if require_youtube_session and (
            _errors.invalid_youtube_session
            or not downloader.get_info_extractor("Youtube").is_authenticated
        ):
            raise RuntimeError(
                "La sesion de YouTube dejo de ser valida durante la consulta; "
                "exporta e importa cookies.txt de nuevo. No se usara una lista anonima.")
        result["used_cookies"] = True
        return json.dumps(result)
    finally:
        _account_downloader = None
        if downloader is not None:
            try:
                downloader.close()
            except Exception as error:
                print(f"[yt-dlp] Cerrar instancia con cookies: {error}", flush=True)
        try:
            os.remove(cookie_file)
        except OSError as error:
            print(f"[yt-dlp] No se pudo eliminar el archivo temporal de cookies: {error}", flush=True)
            raise RuntimeError("No se pudo eliminar el archivo temporal de cookies") from error


def _check_account_playlist_session(youtube: bool) -> None:
    if _account_downloader is not None and youtube and (
        _errors.invalid_youtube_session
        or not _account_downloader.get_info_extractor("Youtube").is_authenticated
    ):
        raise RuntimeError(
            "La sesion de YouTube no es valida; importa cookies actuales. "
            "No se usara una lista anonima.")


def _publish_playlist(media: list[dict], skipped: int, youtube: bool) -> None:
    global _published
    if not _streaming or _request is None:
        return
    _check_account_playlist_session(youtube)
    result = dict(media[0])
    result.update(
        playlist=[item["url"] for item in media[1:]],
        entry_ids=[item["entry_id"] for item in media],
        used_cookies=_account_downloader is not None,
        playlist_pending=True,
        playlist_notice=(
            f"Cola: {len(media)} listos, {skipped} omitidos; "
            f"resolviendo el resto (limite {_PLAYLIST_LIMIT})."),
    )
    bridge, generation = _request
    bridge.reportMediaExtractionItem(generation, json.dumps(result))
    _published = True


def _extract_media(
    url: str,
    audio_only: bool,
    runtime_path: str,
    cache_dir: str | None,
    downloader: YoutubeDL | None = None,
) -> str:
    """Resolve media with a reusable guest downloader."""
    _check_js_runtime(runtime_path)
    started = time.monotonic()
    guest = downloader is None
    if downloader is None:
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
        if guest:
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
        _check_account_playlist_session(_youtube_host(urlsplit(url).hostname))
        entries = info.get("entries")
        if entries is None:
            raise RuntimeError("yt-dlp devolvio una lista sin elementos")
        _progress(f"Leyendo lista: hasta {_PLAYLIST_LIMIT} elementos por carga...")
        selected = list(islice(entries, _PLAYLIST_LIMIT))
        if not selected:
            raise RuntimeError("La lista no contiene elementos reproducibles")
        media = []
        skipped = 0
        for index, entry in enumerate(selected, 1):
            _progress(f"Resolviendo elemento {index}/{len(selected)} de la lista...")
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
                    video_id = entry.get("id")
                    if _youtube_host(urlsplit(url).hostname) and isinstance(video_id, str) and video_id:
                        # Resolve only the track, not its watch?list=... radio again.
                        entry_url = f"https://www.youtube.com/watch?{urlencode({'v': video_id})}"
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
                continue
            _publish_playlist(media, skipped, _youtube_host(urlsplit(url).hostname))
        if not media:
            raise RuntimeError(f"La lista no contiene elementos reproducibles: {_failure_message(_errors.last)}")
        result = media[0]
        result["playlist"] = [item["url"] for item in media[1:]]
        result["entry_ids"] = [item["entry_id"] for item in media]
        result["playlist_notice"] = (
            f"Lista: {len(media)} reproducibles, {skipped} omitidos. "
            f"Limite: {_PLAYLIST_LIMIT} por carga."
        )
        _progress(result["playlist_notice"])
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
        "entry_id": _clean_text(info.get("id") or info.get("webpage_url")),
    }
