# yt-dlp nativo con Python C-API

La feature `native-python` enlaza Python embebido directamente desde Rust:

Es una alternativa avanzada, no el backend predeterminado. Para mantener
un solo Python, utilizar Chaquopy como se explica en [README.md](README.md).
No empaquetar este runtime en el APK Gradle de Chaquopy.

```powershell
cargo apk build --manifest-path apps/app_android/Cargo.toml --lib --no-default-features --features native-python
```

## Artefactos obligatorios arm64-v8a

Coloca en `assets/android/arm64-v8a/`:

```text
libpython3.13.so
```

Debe ser un build Android NDK arm64-v8a de Python 3.13. No sirve `python.dll`, `python3.dll` ni una librería compilada para Windows/Linux.

Extrae además el runtime en la ruta privada que usa el código:

```text
/data/data/com.visualrustmpv.app/files/python/
├── python313.zip        # stdlib comprimida
├── yt_dlp/
├── yt_dlp-*.dist-info/
└── dependencias puras Python de yt-dlp
```

La forma recomendada de crear ese contenido es compilar Python para Android con el mismo NDK/ABI, instalar `yt-dlp` en un staging directory y copiar la stdlib comprimida junto a los módulos Python. El instalador de la app debe extraer esos assets una sola vez a `files/python` antes de pulsar Play.

## Flujo C-API

1. `Py_Initialize` inicia el intérprete.
2. `PyRun_SimpleString` inserta `files/python` en `sys.path`.
3. Rust importa `yt_dlp`.
4. Construye `YoutubeDL({'format': 'best', 'quiet': True})`.
5. Ejecuta `extract_info(url, download=False)` con kwargs.
6. Lee `info['url']`.
7. El canal `mpsc` devuelve la URL a egui y `libmpv` recibe `loadfile`.

Sin `libpython3.13.so` y los módulos Python extraídos, `native-python` no puede enlazar ni resolver yt-dlp. El build predeterminado usa Chaquopy; `--no-default-features` conserva el backend HTTP anterior.
