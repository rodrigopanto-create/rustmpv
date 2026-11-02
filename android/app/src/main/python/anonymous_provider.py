"""Install and verify the APK's pinned, local bgutil script provider."""

import hashlib
import json
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import zipfile

_configuration: tuple[str, str] | None = None


def initialize(node_path: str, bundle_path: str, private_dir: str, ca_bundle: str) -> None:
    global _configuration
    if _configuration is not None:
        return
    node = Path(node_path)
    bundle = Path(bundle_path)
    root = Path(private_dir)
    if not node.is_absolute() or not node.is_file():
        raise RuntimeError("No se encuentra Node.js en nativeLibraryDir")
    if not Path(ca_bundle).is_file():
        raise RuntimeError("No se encuentra el certificado CA para Node.js")
    with bundle.open("rb") as source:
        digest = hashlib.file_digest(source, "sha256").hexdigest()
    destination = root / f"{digest}-v2"
    marker = destination / ".complete"
    root.mkdir(parents=True, exist_ok=True)
    if not marker.is_file():
        staging = Path(tempfile.mkdtemp(prefix="install-", dir=root))
        try:
            with zipfile.ZipFile(bundle) as archive:
                for entry in archive.infolist():
                    # Windows PowerShell's .NET ZIP writer uses backslashes.
                    name = entry.filename.replace("\\", "/")
                    target = (staging / name).resolve()
                    if not target.is_relative_to(staging.resolve()):
                        raise RuntimeError("Ruta no permitida en el proveedor PO")
                    if name.endswith("/"):
                        target.mkdir(parents=True, exist_ok=True)
                    else:
                        target.parent.mkdir(parents=True, exist_ok=True)
                        with archive.open(entry) as source, target.open("wb") as output:
                            shutil.copyfileobj(source, output)
            (staging / ".complete").write_text(digest, encoding="ascii")
            staging.rename(destination)
        finally:
            if staging.exists():
                shutil.rmtree(staging)

    server = destination / "server"
    manifest = json.loads((destination / "manifest.json").read_text(encoding="utf-8"))
    environment = {
        "OPENSSL_CONF": os.devnull,
        "NODE_EXTRA_CA_CERTS": ca_bundle,
        "XDG_CACHE_HOME": str(root / "cache"),
        "LD_LIBRARY_PATH": os.pathsep.join(filter(None, [
            str(destination / "runtime-libs"), os.environ.get("LD_LIBRARY_PATH"),
        ])),
    }
    process_environment = os.environ.copy()
    process_environment.update(environment)
    try:
        result = subprocess.run(
            [str(node), str(server / "build" / "generate_once.js"), "--version"],
            env=process_environment, capture_output=True, text=True, timeout=20, check=True,
        )
    except subprocess.CalledProcessError as error:
        raise RuntimeError(
            f"No se pudo iniciar el proveedor PO local: {error.stderr.strip()[:500]}"
        ) from error
    except (OSError, subprocess.SubprocessError) as error:
        raise RuntimeError(f"No se pudo iniciar el proveedor PO local: {error}") from error
    if result.stdout.strip() != manifest["provider_version"]:
        raise RuntimeError("Version del proveedor PO local incorrecta")

    # yt-dlp's namespace plugin finder needs real directories, not Chaquopy's IMY.
    plugin_path = str(destination / "plugin")
    if plugin_path not in sys.path:
        sys.path.insert(0, plugin_path)
    from yt_dlp.plugins import load_all_plugins
    load_all_plugins()
    from yt_dlp_plugins.extractor.getpot_bgutil_script import BgUtilScriptNodePTP
    if BgUtilScriptNodePTP.PROVIDER_VERSION != manifest["provider_version"]:
        raise RuntimeError("Version del plugin PO incorrecta")
    os.environ.update(environment)
    _configuration = (str(node), str(server))
    print("[PO] Node.js y bgutil local verificados; extraccion sin cuentas", flush=True)


def options() -> dict:
    if _configuration is None:
        raise RuntimeError("El proveedor PO local no esta inicializado")
    node, server = _configuration
    return {
        "js_runtimes": {"node": {"path": node}},
        "extractor_args": {
            "youtube": {"player_client": ["default"]},
            "youtubepot-bgutilscript": {"server_home": [server]},
        },
    }
