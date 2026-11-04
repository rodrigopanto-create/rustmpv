# Guía del proyecto

## Regla de oro

- RustMPV es un **wrapper en Rust (principalmente) de mpv, yt-dlp y ffmpeg**: conserva su potencia y comportamiento de Termux, PowerShell y consolas Linux/Unix, con una interfaz amigable para el usuario común.
- Prioriza un flujo automático, claro y minimalista: una caja de URL para los sitios compatibles, fallback explícito cuando haga falta y errores comprensibles. La estética retrofuturista/ciberpunk es estilo visual, no complejidad ni simulación de procesos.

## Arquitectura

- El workspace contiene `app_windows` y `app_android`; ambos comparten modelos y lógica de dominio desde `crates/core_logic`.
- Windows usa `eframe` y los módulos compartidos de `src/`; Android usa la interfaz Rust/egui dentro de `NativeActivity`, `libmpv` por FFI y `MainActivity` como puente JNI/Chaquopy. `PlaybackService` mantiene la reproducción multimedia en segundo plano.
- El backend Android predeterminado es Chaquopy con Python 3.13 y un único worker persistente de `yt-dlp`. QuickJS y el proveedor local Node.js/bgutil resuelven los desafíos de YouTube. No agregues otro runtime de Python ni un proceso `yt-dlp` al APK sin cambiar y validar explícitamente la arquitectura.
- Las cookies de cuenta de Android se almacenan cifradas con Android Keystore y solo se descifran temporalmente para el fallback. Nunca registres cookies, credenciales ni URLs sensibles.

Consulta [README_ANDROID.md](README_ANDROID.md) y [apps/app_android/README.md](apps/app_android/README.md) para el flujo de build, runtime, dependencias y pruebas del dispositivo.

## Build y validación

Desde la raíz del repositorio, usa PowerShell:

```powershell
cargo fmt --all -- --check
cargo test --workspace
py -3.13 -m unittest discover -s android\tests -v
.\build-android-chaquopy.ps1
```

El build Android requiere JDK, Android SDK/NDK y Python 3.13; el script prepara los componentes nativos y valida el APK. Para cambios de reproducción Android, sigue el procedimiento de instrumentación en `README_ANDROID.md`: este runner personalizado se ejecuta con `adb shell am instrument`; comprueba su salida `PASS:` y no dependas solo del resultado de Gradle o del código de salida de ADB.

## Convenciones

- Mantén el trabajo de red, extracción y acceso a Python fuera del hilo de UI; reutiliza el worker, canales y mecanismos de cancelación existentes.
- Propaga errores de extracción y reproducción a la UI y los logs sin fallback silencioso. Evita registrar secretos o contenido sensible.
- Añade o actualiza pruebas junto con cambios de comportamiento y conserva la documentación Android cuando cambie el flujo de build o runtime.
