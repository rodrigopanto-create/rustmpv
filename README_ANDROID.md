# Android

The default Android build uses Gradle, Chaquopy and the Rust `app_android`
library for `aarch64-linux-android`. Run from the repository root, not `.git`.

```powershell
.\build-android-chaquopy.ps1
```

## Distribution APK

The launcher has an adaptive icon (and a vector fallback for Android 7.0/7.1),
including a monochrome Android 13 themed icon. The application label is
`VisualRustMPV`; Android 12+ also uses its launcher icon for the system splash.

Create a signed arm64 APK with:

```powershell
.\build-android-release.ps1
```

The first run generates a local PKCS#12 release key outside the repository at
`%LOCALAPPDATA%\VisualRustMPV\signing\`. Its password is encrypted with Windows
DPAPI for the current Windows account. The script builds the `release` variant,
verifies APK signing and 16 KiB native-library alignment, then copies the
installable artifact to `dist/VisualRustMPV-v0.1.0-arm64-v8a.apk` and prints
its SHA-256. It does not install over the development build.

**Back up `visualrustmpv-release.p12` securely.** Every future release must use
this same key to upgrade installations without removing app data. The DPAPI
password file only decrypts under the same Windows account; keep it alongside
the key backup or make a separate secure backup of the password. Never commit
the keystore or password.

## Native binaries

The Windows files in `assets/` (`mpv.exe`, `yt-dlp.exe` and `ffmpeg.exe`) are never used by Android.

The official `mpv-android` arm64-v8a release is extracted into `assets/android/arm64-v8a/`:

- `libmpv.so`
- `libplayer.so`
- `libav*.so` and `libsw*.so`
- `libc++_shared.so`

The source release and SHA-256 are pinned in [build-android-chaquopy.ps1](build-android-chaquopy.ps1).

## yt-dlp with Chaquopy

The Gradle module in `android/` embeds Python 3.13 and installs the original `yt-dlp` package. The runner is:

```text
android/app/src/main/python/yt_dlp_runner.py
```

Rust calls `MainActivity.extractMedia` through JNI from one persistent
`yt-dlp-worker` thread. Chaquopy owns the only Python runtime; no standalone Python executable, manually
extracted stdlib, or Python C-API library is needed. The validated direct URL
returns through `mpsc` to libmpv's `loadfile`. Python/Java exceptions are reported
in the UI and logs, without silently falling back to remote resolvers.

### Persistent Python worker

At app startup Rust spawns a single process-wide thread (`OnceLock`), attached
to the JVM for its whole life. It first calls `MainActivity.warmUpYtDlp`
(`yt_dlp_runner.warm_up`): import yt-dlp, QuickJS check, and one reusable
`YoutubeDL` per mode with the YouTube extractor already instantiated. The UI
shows `yt-dlp precargado en N s` (about 2 s on the API 31 test phone). Play then
queues a job on a channel; jobs run serially and reply on a per-request channel,
so the UI never blocks and Stop still discards a pending result.

Measured on device, about 29 of the ~33 s per extraction were QuickJS
preprocessing the ~2.5 MB YouTube player, repeated on every call because
yt-dlp-ejs disables its preprocessed-player cache upstream (large files, no
rotation). The runner enables it with `cachedir=<cacheDir>/yt-dlp`, keeps only
the two newest players (~4.3 MB each) and clears them after any extraction error.
Result: first extraction for a new player ~33 s, later ones ~4.5–5.5 s, also
after restarting the app.

After warm-up the worker calls `warmUpYouTubePlayer` (`warm_player`): it asks
`iframe_api` for the current player id (a few KB) and, only if that player is
not cached, extracts one short public video so the ~33 s preprocessing happens
in the background at startup instead of on Play (measured 0.4 s when current,
34.6 s for a new player). Once a YouTube URL stays unchanged for 700 ms, the UI
also queues a prefetch for the current URL and mode. A finished prefetch stays
valid for 5 minutes and Play sends it to `loadfile` immediately (measured 1 ms).
Pressing Play while a prefetch is in flight reuses that request instead of
starting another one. A queued prefetch whose URL/mode changed is dropped
before it touches the network. Prefetch failures are only logged, and Play
then retries normally.

### yt-dlp updates without reinstalling

`yt_dlp_update.py` lets yt-dlp follow YouTube changes between APK releases.
After the worker is warm, a separate `yt-dlp-update` thread checks, at most
once every 24 h, where GitHub's `releases/latest` redirects. If that stable
version is newer, it downloads the `py3-none-any` wheels for yt-dlp and for the
exact `yt-dlp-ejs` version that release pins. Wheels come only from
`files.pythonhosted.org`, with size and SHA-256 checked against PyPI. Only the
`yt_dlp/` and `yt_dlp_ejs/` packages are extracted, with path and size limits,
into `files/yt-dlp-update/v/<version>`.

On the next start, `MainActivity.warmUpYtDlp` calls `activate()` before
anything imports `yt_dlp`, so the download goes first on `sys.path`. Modules
are never swapped in a running process. The new version starts on trial.
After the player warm-up, one real extraction confirms it. If that extraction
fails while online, or the version fails to import, the version is rejected
and never downloaded again. The bundled version is used from the next start.
Failures while offline only postpone verification, for at most two unverified
starts. Installing a new APK changes `lastUpdateTime`, which discards any
downloaded version. The UI now has an `Actualizar yt-dlp` button beside
`Video MP4` / `Audio directo`; it triggers a forced check immediately (ignores
the 24-hour limit) and disables itself while checking. YouTube's player cache is
per yt-dlp version, so the first
start on a new version prepares the player again (~41 s, in the background).

Verified on the device by bundling 2026.10.21:
- First start: downloaded 2026.10.26 + ejs 0.8.0 in 2.3 s.
- Second start: activated it on trial, and a 6.8 s extraction verified it.
- Reinstalling the APK discarded it.

YouTube JavaScript challenges use **QuickJS-ng 0.12.1**, compiled from
SHA-256-pinned source with the Android NDK, and the bundled
**yt-dlp-ejs 0.8.0** solver paired with **yt-dlp 2026.10.26**.
The Android PIE executable is packaged as `lib/arm64-v8a/libqjs.so`;
Java passes its installed `nativeLibraryDir` path to Python. It is not
executed from writable `files/` and does not add another Python runtime.
The runner checks actual JavaScript execution with a 10-second startup
timeout and explicitly configures yt-dlp's `quickjs` runtime. Remote EJS
component downloads are not enabled. The source license is included in
APK assets. The Windows build requires `ANDROID_NDK_HOME` and `tar`;
run the full script after Gradle `clean` to prepare QuickJS and its license.

Playback status comes from nonblocking libmpv events, not successful command
submission. The GUI no longer prints the debug-style status line or active
audio output line; those details remain in logs. Native HTTPS uses certifi's CA
bundle from the same Chaquopy runtime with TLS verification enabled.
OpenSL ES audio was verified on the connected API 31 device with a progressing
playback clock and an active Android AudioFlinger track; the user also confirmed
audible output in the audio-only build.

Video now uses libmpv's OpenGL render API inside an egui/Glow callback.
An offscreen RGBA8 framebuffer is copied into the in-app video rectangle,
preserving aspect ratio and the Android system-bar margins. OpenGL ES 3 is
required for video. Decoding is currently software-only; hardware decoding
remains future work.
Audio loads use file-local `vid=no`; Video loads use `vid=auto`.
Pause/resume, seeking and loading use asynchronous mpv commands to avoid
blocking the render thread. Audio continues on lock/background in both Audio
and Video modes. Lifecycle callbacks disable only the video track while hidden,
then restore it on return, preserving position and manual pause.
Rendered YouTube frames, pause, seeking and surface recreation on return from
background were checked on the API 31 phone. The user confirmed human A/V
lip-sync as well as audible output.

Vector Play, Pause and Stop controls share a position slider and media clock.
Play resumes the current media; Stop followed by Play restarts from zero,
reusing the direct URL for up to five minutes in memory unless playback failed.
Stop during extraction discards the eventual result without killing Python or
allowing overlapping workers.

Fullscreen hides system bars and switches to sensor landscape; the persistent
restore button returns to portrait. Activity and mpv are retained, preserving
position and pause. Immersive landscape and portrait restoration were verified
on API 31. Android Back is mapped to Escape through a platform-scoped local
patch of egui-winit 0.29.1 in `vendor/egui-winit`, so the app can leave fullscreen
without recreating playback. Desktop key mapping is unchanged. The API 33+
predictive Back callback and physical rotation
between landscape sides still need device validation.

Video playback sets the window's `FLAG_KEEP_SCREEN_ON` on the Java UI thread.
Pause, Stop, EOF, errors, audio-only mode and backgrounding release it.
Buffering during active video keeps it set. No global screen timeout changes
are needed.

`PlaybackService` is a `mediaPlayback` foreground service sharing the existing
Rust/libmpv player. Its MediaSession exposes position, duration and Play/Pause/Stop
through a media notification and lock-screen controls. It polls native events
while the GUI is suspended, and owns a partial CPU wake lock only while active
and unpaused. Pause releases the lock; Stop, EOF and errors also remove the
service and notification. Audio focus interruptions and headphone disconnection
pause playback. Each load passes title, channel and thumbnail candidates to the
service; it downloads the first valid HTTPS image in the background (8 s timeout,
4 MB limit, scaled to 512 px) and publishes it as MediaSession artwork and the
notification large icon. Direct URLs show the file name and host without artwork.
The manifest declares foreground media service and wake-lock
permissions; no battery-optimization exemption is requested.
Removing the task from recents stops playback. Force-stop/process death does
not automatically restart it (`START_NOT_STICKY`).

Network operations use a 10-second socket timeout and limited retries. These
are not a hard total extraction deadline: in-process Python cannot safely be
killed. The UI remains responsive and does not start overlapping resolutions.
See [app_android](apps/app_android/README.md) for tests and details.

The included Gradle Wrapper pins Gradle 8.9 with a SHA-256 checksum. The script
requires JDK 17 or 21, SDK 36, the NDK and host Python 3.13 for packaging.
Set `CHAQUOPY_BUILD_PYTHON` to an explicit host interpreter if needed. This host
Python is not another runtime in the APK. The script also discovers the local
tools under `%LOCALAPPDATA%\VisualRustMPV\build-tools`.
For a restricted PowerShell session, use
`powershell.exe -NoProfile -ExecutionPolicy RemoteSigned -File .\build-android-chaquopy.ps1`
without changing machine-wide policy. It explicitly builds
`app_android` with `chaquopy`; the manifest loads `libapp_android.so`.
`cargo-apk` alone does not include the Java/Python pieces required by Chaquopy.

## History and resume

`apps/app_android/src/history.rs` keeps the last 50 items in
`files/history.json`: title, channel, mode, position and duration. A YouTube
video uses its ID as the key, so `watch?v=`, `youtu.be`, `shorts` and `live`
links share one resume position. Progress is saved after every 5 s of playback
change, once when paused, and on stop or end. It is saved from
`PlaybackService.nativeTick` too, so it keeps working with the screen locked.
Writes go to a temporary file followed by a rename. A corrupt file starts an
empty history.

Playing an item again passes `start=<s>` to `loadfile` and shows
`Reanudando en m:ss`. There is no resume:
- before 15 s;
- within 15 s or 5 % of the end, where the item counts as watched;
- for items without a finite duration (live streams).

The list below the player can be tapped to replay with the saved mode; each item
has a remove button, and "Borrar todo" asks for confirmation.

Verified on the device with Big Buck Bunny:
- Stopping at 44.6 s and force-stopping the app still showed
  `continuar en 0:44 / 10:34`; tapping the item resumed at `time-pos=44.64s`.
- With the screen off, the file kept advancing (96.9 s) up to when the process
  was killed.

## Optional yt-dlp subprocess backend

The `yt-dlp-subprocess` feature in [app_android](apps/app_android/README.md)
resolves streams in a worker using `std::process::Command`, captures both output
pipes, enforces a 45-second process timeout, and passes a single validated HTTP(S)
URL to libmpv. It requires `--no-default-features --features yt-dlp-subprocess`
and is an alternative to Chaquopy/`native-python`, not an additional runtime
for the default APK. The Gradle module excludes its script/runtime assets.
See the app README for the asset manifest and NDK executable requirements.

On Android 10+ with this app's target SDK, `chmod +x` does not permit executing
native binaries from writable `files/`. Package an Android PIE executable as
`libpython_exec.so` (with the Python assets) or `libyt_dlp_exec.so` (standalone)
in `jniLibs`/the native runtime directory. The package installer extracts it to
`nativeLibraryDir`; only scripts/data are extracted to `files/`.

The remaining Android-specific requirement is FFmpeg post-processing. The extracted mpv libraries include FFmpeg shared libraries, but yt-dlp's Python postprocessor expects an executable or a dedicated FFmpeg JNI bridge for both MP4 merging and MP3 conversion.

ABI-specific files belong in:

```text
assets/android/arm64-v8a/
assets/android/armeabi-v7a/
assets/android/x86_64/
```

Build the Chaquopy APK with:

```powershell
.\build-android-chaquopy.ps1
```

This downloads and verifies the official mpv-android APK and QuickJS-ng source,
prepares the native binaries, builds the Rust `cdylib`, and invokes Gradle/Chaquopy.
