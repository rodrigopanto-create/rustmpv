"""Updates yt-dlp from PyPI without reinstalling the APK.

Layout of the update directory (<filesDir>/yt-dlp-update):
  state.json   {"version", "apk", "status": "trial" | "ok", "launches"}
  checked      time of the last release query
  rejected     last version whose trial extraction failed
  v/<version>  yt_dlp plus the yt_dlp_ejs release it pins, from PyPI wheels

A download is only put on sys.path at the next start, before anything imports
yt_dlp: modules already loaded cannot be swapped safely. The new version stays on
trial until one real extraction succeeds. A failed trial, or more than
_MAX_TRIAL_LAUNCHES unconfirmed starts, falls back to the version in the APK.
"""

import hashlib
import io
import json
import os
import re
import shutil
import ssl
import sys
import tempfile
import time
import urllib.request
from urllib.parse import urlsplit
import zipfile

# GitHub answers with a redirect to the newest stable tag; no body is downloaded.
_LATEST_URL = "https://github.com/yt-dlp/yt-dlp/releases/latest"
_PYPI_URL = "https://pypi.org/pypi/{name}/{version}/json"
_ONLINE_URL = "https://www.youtube.com/generate_204"
_FILES_HOST = "files.pythonhosted.org"
_USER_AGENT = "VisualRustMPV-updater"
_CHECK_INTERVAL = 24 * 3600
_TIMEOUT = 20
_JSON_LIMIT = 4 * 1024 * 1024
_WHEEL_LIMIT = 16 * 1024 * 1024
_UNPACKED_LIMIT = 64 * 1024 * 1024
_MAX_TRIAL_LAUNCHES = 2
_VERSION = re.compile(r"\d{4}\.\d{1,2}\.\d{1,2}(?:\.\d+)?")
_EJS_VERSION = re.compile(r"\d+\.\d+\.\d+")
_PACKAGES = ("yt_dlp", "yt_dlp_ejs")

_active = ""


def _log(message: str) -> None:
    print(f"[yt-dlp-update] {message}", flush=True)


def version_tuple(value: object) -> tuple[int, ...] | None:
    """Stable yt-dlp versions only (YYYY.MM.DD[.N]); nightlies are ignored."""
    if not isinstance(value, str) or not _VERSION.fullmatch(value):
        return None
    return tuple(int(part) for part in value.split("."))


def active() -> str:
    return _active


def _path(root: str, *parts: str) -> str:
    return os.path.join(root, *parts)


def _read_text(root: str, name: str) -> str:
    try:
        with open(_path(root, name), encoding="utf-8") as file:
            return file.read().strip()
    except OSError:
        return ""


def _write_text(root: str, name: str, text: str) -> None:
    os.makedirs(root, exist_ok=True)
    temporary = _path(root, name + ".tmp")
    with open(temporary, "w", encoding="utf-8") as file:
        file.write(text)
    os.replace(temporary, _path(root, name))


def _read_state(root: str) -> dict:
    try:
        state = json.loads(_read_text(root, "state.json") or "{}")
    except ValueError:
        return {}
    return state if isinstance(state, dict) else {}


def _write_state(root: str, state: dict) -> None:
    _write_text(root, "state.json", json.dumps(state))


def _remove_versions(root: str, keep: str | None) -> None:
    versions = _path(root, "v")
    try:
        names = os.listdir(versions)
    except OSError:
        return
    for name in names:
        if name != keep:
            shutil.rmtree(_path(versions, name), ignore_errors=True)


def _clear(root: str) -> None:
    try:
        os.remove(_path(root, "state.json"))
    except OSError:
        pass
    _remove_versions(root, keep=None)


def activate(root: str, apk_stamp: str) -> str:
    """Put a downloaded yt-dlp ahead of the bundled one. Must run before importing yt_dlp."""
    global _active
    if _active or "yt_dlp" in sys.modules:
        return _active
    try:
        names = os.listdir(root)
    except OSError:
        names = []
    for name in names:
        if name.startswith("staging-"):
            shutil.rmtree(_path(root, name), ignore_errors=True)
    state = _read_state(root)
    version = state.get("version")
    if not state:
        _remove_versions(root, keep=None)
        return ""
    if state.get("apk") != apk_stamp or version_tuple(version) is None:
        _log("APK reinstalado: se descarta la actualizacion descargada")
        _clear(root)
        return ""
    if state.get("status") != "ok":
        launches = state.get("launches")
        launches = (launches if isinstance(launches, int) else 0) + 1
        if launches > _MAX_TRIAL_LAUNCHES:
            _log(f"{version} no se verifico tras {_MAX_TRIAL_LAUNCHES} arranques; "
                 "se usa la version incluida")
            _clear(root)
            return ""
        state["launches"] = launches
    path = _path(root, "v", version)
    if not all(os.path.isfile(_path(path, package, "__init__.py")) for package in _PACKAGES):
        _log(f"{version} incompleto en disco; se usa la version incluida")
        _clear(root)
        return ""
    _write_state(root, state)
    _remove_versions(root, keep=version)
    sys.path.insert(0, path)
    _active = version
    _log(f"Usando yt-dlp {version} ({state.get('status', 'trial')})")
    return version


def trial_pending(root: str) -> bool:
    state = _read_state(root)
    return bool(_active) and state.get("version") == _active and state.get("status") != "ok"


def confirm(root: str) -> None:
    state = _read_state(root)
    if state.get("version") == _active:
        state["status"] = "ok"
        state.pop("launches", None)
        _write_state(root, state)


def reject(root: str, version: str) -> None:
    """Never retry `version` automatically and use the bundled yt-dlp from the next start."""
    _write_text(root, "rejected", version)
    _clear(root)


def reject_active(root: str) -> bool:
    """Called when importing/warming yt-dlp failed; only a version on trial is blamed."""
    if not trial_pending(root):
        return False
    reject(root, _active)
    return True


def _open(url: str, context: ssl.SSLContext, method: str = "GET"):
    request = urllib.request.Request(url, method=method, headers={"User-Agent": _USER_AGENT})
    opener = urllib.request.build_opener(urllib.request.HTTPSHandler(context=context))
    return opener.open(request, timeout=_TIMEOUT)


def _read(url: str, context: ssl.SSLContext, limit: int) -> bytes:
    with _open(url, context) as response:
        data = response.read(limit + 1)
    if len(data) > limit:
        raise RuntimeError(f"Respuesta demasiado grande: {url}")
    return data


def online(cafile: str) -> bool:
    try:
        with _open(_ONLINE_URL, ssl.create_default_context(cafile=cafile), "HEAD") as response:
            return 200 <= response.status < 400
    except Exception:
        return False


def _latest_version(context: ssl.SSLContext) -> str:
    with _open(_LATEST_URL, context, "HEAD") as response:
        final = urlsplit(response.geturl())
    match = re.fullmatch(r"/yt-dlp/yt-dlp/releases/tag/([0-9.]+)", final.path)
    if final.scheme != "https" or final.hostname != "github.com" or not match \
            or version_tuple(match[1]) is None:
        raise RuntimeError(f"Respuesta inesperada al buscar la ultima version: {response.geturl()}")
    return match[1]


def _download_wheel(name: str, version: str, context: ssl.SSLContext) -> bytes:
    info = json.loads(_read(_PYPI_URL.format(name=name, version=version), context, _JSON_LIMIT))
    filename = f"{name.replace('-', '_')}-{version}-py3-none-any.whl"
    entry = next((entry for entry in info.get("urls") or []
                  if isinstance(entry, dict) and entry.get("filename") == filename
                  and entry.get("packagetype") == "bdist_wheel"), None)
    if entry is None:
        raise RuntimeError(f"PyPI no publica {filename}")
    url, size = entry.get("url"), entry.get("size")
    digest = (entry.get("digests") or {}).get("sha256")
    parsed = urlsplit(url) if isinstance(url, str) else None
    if parsed is None or parsed.scheme != "https" or parsed.hostname != _FILES_HOST \
            or not isinstance(size, int) or not 0 < size <= _WHEEL_LIMIT \
            or not isinstance(digest, str) or not re.fullmatch(r"[0-9a-f]{64}", digest):
        raise RuntimeError(f"Entrada de PyPI no valida para {filename}")
    data = _read(url, context, size)
    if len(data) != size or hashlib.sha256(data).hexdigest() != digest:
        raise RuntimeError(f"{filename}: tamano o SHA-256 no coinciden")
    return data


def _pinned_ejs(wheel: bytes, version: str) -> str:
    with zipfile.ZipFile(io.BytesIO(wheel)) as archive:
        metadata = archive.read(f"yt_dlp-{version}.dist-info/METADATA").decode("utf-8")
    pins = set(re.findall(r"^Requires-Dist: yt-dlp-ejs==([0-9.]+)\s*;", metadata, re.MULTILINE))
    if len(pins) != 1 or not _EJS_VERSION.fullmatch(next(iter(pins))):
        raise RuntimeError(f"yt-dlp {version} no fija una version unica de yt-dlp-ejs")
    return pins.pop()


def _extract(wheel: bytes, target: str, package: str) -> None:
    """Unpack only `package/` from a wheel, rejecting unsafe paths and zip bombs."""
    total = 0
    with zipfile.ZipFile(io.BytesIO(wheel)) as archive:
        for member in archive.infolist():
            name = member.filename
            if member.is_dir() or not name.startswith(package + "/"):
                continue
            parts = name.split("/")
            if "\\" in name or ":" in name or any(part in ("", ".", "..") for part in parts):
                raise RuntimeError(f"Ruta no permitida en el wheel: {name}")
            total += member.file_size
            if total > _UNPACKED_LIMIT:
                raise RuntimeError(f"{package} descomprimido excede el limite")
            path = os.path.join(target, *parts)
            os.makedirs(os.path.dirname(path), exist_ok=True)
            with archive.open(member) as source, open(path, "wb") as destination:
                shutil.copyfileobj(source, destination)
    if not os.path.isfile(os.path.join(target, package, "__init__.py")):
        raise RuntimeError(f"El wheel no contiene el paquete {package}")


def check(root: str, apk_stamp: str, running_version: str, cafile: str,
          now: float | None = None, force: bool = False) -> str:
    """Download a newer stable yt-dlp for the next start. Returns a user-facing
    message when something was downloaded, otherwise an empty string."""
    now = time.time() if now is None else now
    try:
        last = float(_read_text(root, "checked"))
    except ValueError:
        last = None
    if not force and last is not None and 0 <= now - last < _CHECK_INTERVAL:
        _log("Consultado hace menos de 24 h")
        return ""
    # Recorded before any network access so a failing check is not retried in a loop.
    _write_text(root, "checked", repr(now))
    context = ssl.create_default_context(cafile=cafile)
    latest = _latest_version(context)
    if version_tuple(latest) <= (version_tuple(running_version) or ()):
        _log(f"Al dia ({running_version}; ultima {latest})")
        return f"yt-dlp {running_version} esta al dia" if force else ""
    state = _read_state(root)
    if state.get("apk") == apk_stamp and state.get("version") == latest \
            and os.path.isdir(_path(root, "v", latest)):
        _log(f"{latest} ya descargado; pendiente de reinicio")
        return f"yt-dlp {latest} ya descargado; reinicia la app para usarlo" if force else ""
    if _read_text(root, "rejected") == latest:
        _log(f"{latest} omitido: fallo su verificacion")
        return f"yt-dlp {latest} se descarto porque fallo su verificacion" if force else ""
    started = time.monotonic()
    wheel = _download_wheel("yt-dlp", latest, context)
    ejs_version = _pinned_ejs(wheel, latest)
    ejs_wheel = _download_wheel("yt-dlp-ejs", ejs_version, context)
    staging = tempfile.mkdtemp(prefix="staging-", dir=root)
    try:
        _extract(wheel, staging, "yt_dlp")
        _extract(ejs_wheel, staging, "yt_dlp_ejs")
        target = _path(root, "v", latest)
        shutil.rmtree(target, ignore_errors=True)
        os.makedirs(_path(root, "v"), exist_ok=True)
        os.replace(staging, target)
    except BaseException:
        shutil.rmtree(staging, ignore_errors=True)
        raise
    _write_state(root, {"version": latest, "apk": apk_stamp, "status": "trial", "launches": 0})
    _log(f"Descargado {latest} (ejs {ejs_version}) en {time.monotonic() - started:.2f} s")
    return f"yt-dlp {latest} descargado; se aplicara al reiniciar la app"
