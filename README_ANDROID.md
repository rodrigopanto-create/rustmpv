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
On launch, a short splash screen covers the player controls while the Python
worker initializes; it disappears when yt-dlp is ready, with an eight-second
maximum so a slow warm-up cannot block the app. YouTube-player cache warming
continues in the background after the player screen appears.
The splash shares the player's system-bar insets and centers its content inside
the usable window, clear of the status and navigation bars.

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
before it touches the network. Prefetch failures are logged and displayed with
their reason; manual Play can retry normally.

### Player controls and resolution progress

Android uses a retro terminal layout: deep blue-black surfaces, phosphor-green
transport accents and cyan section labels. A shared source/mode input with
automatic access comes first, followed by observed player state, the bounded
process console, collapsed **ACCESO Y MOTOR** settings and history cards.
Cookie import/removal and yt-dlp updates remain in that settings section;
import errors remain visible even when it is collapsed.
The monospace console shows actual resolver events with distinct error/warning
colors, an independent scroll area and no decorative/fake output. A gentle cyan
border pulse (one cycle per 2.4 seconds, never an on/off flash) follows the active
stage: source/editing, process console, account access or engine maintenance,
then playback. Account access is highlighted only in account mode or after an
actual fallback notification; guest extraction skips it. Playback takes priority
over remaining queue extraction. Idle, stopped and failed flows return to source;
history does not pulse. Borders keep a fixed width so the layout never moves.
The pulse build passed 82 Android Rust tests and
`PASS: playlist_navigation,progressive_guest,music_session` on the OPPO.
Device frame samples confirmed that only the active border changed color;
inactive borders remained unchanged.
The theme is Android-only; Windows is unchanged. Normal text palette pairs are
tested at a minimum 4.5:1 contrast, with responsive 320-to-800-point layouts.

The speaker button switches libmpv sound ON/OFF without pausing or seeking;
the selection survives subsequent loads in the same app session. Narrow portrait
layouts put the seek bar below the transport buttons. The URL, resolver status,
video and history share a clipped scrollable area above the controls, with the
keyboard's visible window boundary reserved so history cannot paint over them.
**Descargar medio actual** sends the selected playlist item to Android's public
`Downloads` folder through `DownloadManager`; Android owns background progress
and its completion notification. The button is enabled only for recognized
single-file HTTP(S) formats, not HLS/DASH manifests.
**Anterior** and **Siguiente** move between ready playlist items without
restarting extraction or cancelling the progressive queue. They preserve Pause,
do not wrap at the ends, and stay disabled while switching/loading/recovering.
Siguiente becomes available when the worker appends another item. Android's
media notification, lock-screen session and headset skip commands use the same
navigation and boundary checks; individual-media playback has no skip actions.

Important resolver stages remain in Android logs and the bounded **Consola del
proceso** panel: guest preparation, site/format requests, JavaScript/PO Token work,
account fallback, success and final failure. Stage messages do not display
extractor URLs or cookie contents. An elapsed timer and an in-progress message
distinguish a slow JavaScript check from a completed failure.

**Cancelar proceso y volver al player** (or Stop) immediately releases the URL
editor, discards late results and invalidates queued resolver jobs. The embedded
worker checks cancellation during Python execution, including a stuck Python
loop, without turning cancellation into an account retry. Temporary account jars
are still closed/deleted in `finally`. A blocking native/network/subprocess call
can only unwind after returning or reaching its existing timeout; cancelling
never freezes the GUI while it waits. The same cancelled input is not
automatically prefetched again; another URL or manual Play starts a new request.

Verified on the OPPO CPH2127: cancelling a stuck extractor allowed another URL
to play on the same worker; four foreground recovery regressions passed. With
the portrait keyboard open, history stayed clipped above the controls. Sound
OFF/ON was toggled during public YouTube video playback without resetting its
clock; playback advanced to 15 seconds and ended normally. The related host
suites passed 60 Rust and 75 Python tests.

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

Video uses libmpv's OpenGL render API in a separate EGL context with a 1 x 1
Pbuffer and textures shared with eframe/Glow. An egui callback composites the
offscreen RGBA8 texture into the in-app video rectangle using a shader.
Producer writes and consumer reads are synchronized with `glFinish` across
context switches. This isolates mpv's GL state from egui and fixes the black
video observed on the Redmi Note 8 Pro (Mali-G76).
Aspect ratio and Android system-bar margins are preserved. OpenGL ES 3 is
required for video. Decoding is currently software-only; hardware decoding
remains future work.
Audio loads use file-local `vid=no`; Video loads use `vid=auto`.
YouTube playlist URLs resolve up to 100 direct streams; the first starts
immediately and the remaining entries are queued in libmpv for sequential
playback. Unavailable entries are skipped; larger lists are capped at 100.
X/Twitter (`x.com`, `twitter.com`), Instagram (`instagram.com`, `instagr.am`)
and Facebook (`facebook.com`, `fb.watch`, `fb.com`) links are also resolved by
yt-dlp; other HTTP(S) hosts go straight to libmpv. Their supported URL formats
are handled by yt-dlp's extractors. Format selection falls back
to progressive MP4 with audio when no separate audio or known height exists,
and multi-video posts are queued like playlists. Resolve failures
show yt-dlp's real reason in red while the URL is unchanged; important progress
also appears in the process console. Checked on the Redmi
Note 8 Pro (Android 11): a public Instagram reel rendered; restricted tweets
and login-only reels showed the red error.

### Automatic access and per-site cookie fallback

Normal Android extraction chooses the likely access mode automatically.
Public YouTube videos/radios try without account credentials first; X/Twitter,
Instagram, Facebook and recognizable personal YouTube lists try saved cookies
first, then public access if unavailable or rejected. No session selector is
needed. A public fallback is announced visibly; recommendations can differ.
Previously saved
YouTube, X/Twitter, Instagram and Facebook cookies remain encrypted with Android
Keystore and excluded from backups/device transfers. If the first extraction
fails, the app makes **one** attempt with the alternate access mode, using a
fresh yt-dlp instance and account-compatible default YouTube clients with QuickJS.
Cookies are decrypted lazily to a private temporary file, removed after the
retry even on failure; account downloaders are closed and never enter the guest
cache. Missing cookies produce an actionable error; two failed attempts report
both reasons. Provider initialization failure can also fall back to cookies.
YouTube guest-first successes and background warm-up/update verification never
decrypt saved cookies. Invalid URLs and programming errors do not trigger login.
**Importar/Cambiar cookies de fallback** selects Netscape `cookies.txt` for these
four platforms; importing replaces cookies only for the platforms in that file,
preserving saved cookies for other platforms. **Eliminar cookies guardadas**
explicitly removes it. The UI indicates when the fallback uses your account.
Extractors may create their own guest-session cookies in memory.
Guest downloaders are isolated by platform; social extraction does not require
YouTube's Node/bgutil provider. Editing/pasting another URL or changing mode
cancels obsolete worker results, prefetch and recovery, clears old notices and
keeps the existing media playing until another source is loaded.
The automatic-access build passed the OPPO's five synthetic switching scenarios:
`cross_site_youtube,cross_site_twitter,cross_site_instagram,cross_site_facebook,cross_site_return`.
These use localhost audio and synthetic cookies, verify cookie merging/alias
isolation, and never certify real social-account permissions. The same APK
passed `guest,cookies,progressive_guest,music_session,progressive_session,playlist_navigation,music_radio_screenshot`
and a separate `resolve_cancel`. The radio case verifies real public YouTube;
restricted social URLs still require owner testing with valid site cookies.
The final APK also passed
`cross_site_youtube,cross_site_twitter,cross_site_instagram,cross_site_facebook,cross_site_return,playlist_navigation,music_radio_screenshot`
on the Redmi Note 8 Pro after installation preserving app data and cookies.
The separate test APK was removed afterward.
Other HTTP(S) hosts still go directly to libmpv.
Extraction does not rerun a partially successful playlist just for skipped entries.
Account cookies are sensitive and may expose the account to platform restrictions;
the fallback cannot guarantee access to private, age-gated or membership content.

#### YouTube and YouTube Music lists

Paste individual watch/short/live/embed links, a shared `youtu.be` link with
`list=`, playlist URLs, or YouTube Music playlist/album (`browse/MP...`,
`browse/VL...`) URLs. Shared links are normalized without dropping their playlist
or index query. Album-to-playlist resolution is delegated to the bundled yt-dlp
YouTube extractor; YouTube Music is not a separate official playback API.
Seeded radio playlist links (`RD` or `RDAMVM` followed by an 11-character
video ID) use their equivalent `watch?v=...&list=...` route: YouTube can reject
the playlist-page route as "This playlist type is unviewable". The seed, list,
index and sharing parameters are preserved. Other personal list IDs are not
guessed or replaced.

The former **Usar mi sesion de YouTube / Music** selector is replaced by automatic
fallback. Seeded public radios (`RD` or `RDAMVM` plus an 11-character video ID)
start publicly. Other `RD...` personal mixes, Watch Later (`WL`), liked videos
(`LL`) and Music likes (`LM`) try account access first. Other private playlists
start publicly and can retry with cookies. Import current Netscape cookies from the intended account;
the app cannot infer a session from the YouTube/Music app installed on the phone.
Missing or rejected cookies allow one public attempt, with a visible notice
that personalization can differ. If both attempts fail, both reasons are shown.
Public playback is not evidence that account authentication or personalized
recommendations work. Account-first mode checks yt-dlp's YouTube authentication-cookie state before and after
extraction and rejects its expired/rotated-cookie warning. This detects missing
or lost authentication; it does not certify account entitlements or access.
If the playlist response already reports invalid authentication, it fails before
enumerating or resolving its tracks, rather than spending minutes on a lost session.
The UI reports this authentication rejection explicitly; it does not infer that
the export is old or promise that repeatedly importing it will restore access.

URL and format are part of the prefetch/cache identity. Once published, account playlist
streams retain account-only playback recovery, so a failed item cannot silently
switch to a guest mix. Only the matching site's jar is decrypted and it is
removed in `finally`, including cancellation.

Enumeration uses yt-dlp flat playlist entries and lazy paging, with at most
**100 entries per load** even for effectively endless radios. Each track is
resolved individually without its radio `list=` context, preventing recursive
playlist extraction. Progress shows the current item; the final summary reports
playable/skipped counts and the limit. Unavailable entries are skipped; an
entirely unavailable list fails visibly. Play starts with the first resolved
track; the existing dedicated Python worker continues resolving and appending
the remaining tracks without restarting playback or changing Pause/seek state.
Prefetch retains both the ready prefix and its live continuation when Play
adopts it. The shared playback service appends tracks even with the screen off.
If playback reaches the end of the ready queue first, it waits for the next
resolved item and resumes automatically. Stop cancels extraction and prevents
late results from restarting playback. A tail failure is reported explicitly
while retaining already queued tracks; extraction never switches account mode
after publishing a prefix. Account authentication is checked before each
published track. Playback-time stream renewal still resolves atomically so it
can identify the failed item before replacing a queue.
This is a bounded snapshot, not continuous generation of an infinite radio;
it needs no additional async runtime.

Playlist history is distinct from individual videos. Reopening a list resolves
it again without applying another track's saved seek time to the first track;
live stream renewal still matches the failed item by stable extractor ID.
No implementation can guarantee every URL: expired sessions, removed/private
items, regional or paid entitlements, DRM and upstream bot checks still apply.
Account-dependent results require owner testing with their own current cookies;
automated account tests use synthetic jars, not saved credentials.
On the OPPO CPH2127, the synthetic account queue used only cookies, played both
items and deleted its temporary jar. A real public one-item YouTube Music
playlist also resolved anonymously and advanced in libmpv. Host validation passed
69 Rust and 91 Python tests; actual personalized recommendations still need
verification with the owner's account.
Progressive guest/account playback, Stop during extraction and screen-off
continuation after an empty-ready-queue gap also passed on the OPPO. The public
Music check passed on retry after an initial local PO Token provider failure;
early queue delivery does not remove upstream extraction failures.

#### Recovery after a stream fails

For these four platforms, a later libmpv loading failure (including HTTP 403)
invalidates the cached URL and prefetch and resolves the original input again:
first a fresh guest attempt, then a cookie-only attempt if playback still fails.
Guest renewal retains extraction's anonymous-first/account fallback. A stream
already resolved with account cookies gets only one account refresh.
The budget is at most two playback renewals per manual Play, including playlists;
it does not reset for every queue item, so failures cannot create a retry loop.
Missing cookies or exhausted retries remain visible rather than silently
restarting playback. Direct HTTP(S) URLs, TLS validation failures and audio/video
output errors do not trigger account recovery.

Renewal preserves the last playback position and pause state. Refreshed playlists
match the failed item by extractor identity and drop the already-played prefix;
an absent or unidentifiable item produces an error instead of playing another
entry. Stop (including the media notification) cancels a pending result, and a
changed input prevents it from loading.
Stopping also prevents automatic prefetch of that same loaded input; a new input
can prefetch normally, and manual Play can resolve it again.

The GUI and `PlaybackService` share one recovery session and the existing serial
Python worker. The service can detect failures, resolve and load the renewed
stream while the screen is locked; returning to the GUI does not start another
retry or load the stream twice. During renewal the notification shows
`Renovando enlace...`; the service retains its wake lock and does not stop at
the transient loading error. Notification Pause/Resume changes the state restored
by the pending load, and Stop cancels it. Video remains disabled in the background
and is restored in the foreground, including after queued items change.
Terminal failures stop the queue and remain available when the app is reopened.
The standalone provider itself still needs no listening HTTP server.

Device regression tests in
[PlaybackRecoveryInstrumentation.java](android/app/src/androidTest/java/com/visualrustmpv/app/PlaybackRecoveryInstrumentation.java)
use a localhost HTTP 403/WAV fixture and synthetic cookies, never the saved jar.
On the Redmi Note 8 Pro, guest renewal and cookie retry each reached loaded
libmpv playback with at least 2 seconds of progress. Stop discarded the pending
refresh without another extraction, and three rejected streams exhausted the
budget without a fourth request. Temporary cookies and test history are cleaned.
All nine foreground/screen-off scenarios passed on the installed background-recovery
build: guest renewal, cookie retry, Stop and exhaustion in both states, plus
notification Pause/Resume during screen-off renewal. The screen-off fixture first
plays a short queue item, then rejects the next item; the service independently
loads its replacement before the observer checks it. Mid-item seek restoration
is covered by host tests, not this device fixture.
The fixture belongs only to the separate test APK, not the installable app.

With JDK/SDK configured and an unlocked phone permitting USB installation:

```powershell
.\android\gradlew.bat -p android :app:assembleDebugAndroidTest --no-daemon --max-workers=2
adb install -r android\app\build\outputs\apk\debug\app-debug.apk
adb install -r android\app\build\outputs\apk\androidTest\debug\app-debug-androidTest.apk
adb shell am instrument -w -e pasteX 610 -e pasteY 315 -e playX 145 -e playY 1365 com.visualrustmpv.app.test/com.visualrustmpv.app.PlaybackRecoveryInstrumentation
adb uninstall com.visualrustmpv.app.test
```

Build the app first with the main build script. The tap coordinates above are for
the tested retro-terminal OPPO 720 x 1600 portrait layout; adjust them for another
screen/density and use `adb -s SERIAL` when multiple devices are connected.
Keep the phone nearby: screen-off cases wake it between scenarios and wait up to
120 seconds for the owner to unlock it through Android's normal lock screen.
The tests do not disable or bypass a secure lock.
To run only the five screen-off cases, add
`-e scenarios background_guest,background_cookies,background_pause,background_cancel,background_exhaust`
to the instrumentation command; for foreground only, use
`-e scenarios guest,cookies,cancel,exhaust`. Selected runs print `PASS:` followed
by the selected names. Inspect this output, not just adb's exit code.
The focused stuck-extractor test uses `-e scenarios resolve_cancel` plus
`-e cancelX 320 -e cancelY 1365` for the OPPO's Stop button. It starts an infinite
Python loop, cancels via the actual player control, then pastes and plays a
different URL using the same serial worker. It is opt-in rather than part of the
default suite because it requires the additional Cancel coordinates.
Playlist checks are also opt-in: `-e scenarios music_session,music_public`.
The first uses synthetic authentication-shaped cookies and requires both queue
items to play; the second resolves a real public YouTube Music playlist, requires
playlist metadata and playback progress, and never opens saved account cookies.
The public case needs network access. Each scenario allows up to 120 seconds,
including cold-worker/player preparation.
Progressive checks use
`-e scenarios progressive_guest,progressive_session,progressive_cancel,background_progressive`.
The extractor publishes one short track, then waits for instrumentation to
observe at least one second of real libmpv playback before resolving the next.
The next track arrives after the first reaches EOF; foreground and screen-off
cases must resume it automatically. Stop must interrupt the unfinished queue
without serving the second item. These checks use only localhost streams and
synthetic cookies.
Playlist navigation is opt-in with `-e scenarios playlist_navigation` plus
`-e nextX ... -e nextY ... -e previousX ... -e previousY ...`. The localhost
three-item fixture checks both UI buttons, notification Next, disabled end
boundaries, Pause/Resume and that navigation never restarts extraction.
The existing Play coordinate now targets the second button, after Anterior.
With the retro-terminal UI on the OPPO, use `nextX=410`, `previousX=55` and
`nextY=previousY=1365`. The final APK printed
`PASS: playlist_navigation,guest,progressive_guest,music_radio_screenshot`
and `PASS: resolve_cancel`. All 80 Android Rust host tests passed, including
contrast checks and themed source/console/transport layouts at narrow and wide
widths. Portrait inspection confirmed the complete seek row, keyboard-safe
transport controls and live process console.
The same retro-terminal APK was installed on the Redmi Note 8 Pro without
clearing app data. For its 1080 x 2340 portrait layout, use `pasteX=930`,
`pasteY=410`, `playX=200`, `playY=2007`, `previousX=80`, `nextX=565`,
`previousY=nextY=2007`, and `cancelX=440`, `cancelY=2007`.
It printed `PASS: playlist_navigation,guest,progressive_guest,music_radio_screenshot,music_session,progressive_session`,
`PASS: resolve_cancel`, and `PASS: background_progressive`.
Individual runner logs also confirmed `cookies,cancel,exhaust,background_pause`.
The subsequent progressive screen-off case initially stopped at the normal
device lock; it passed separately after the owner unlocked the phone.
Run `resolve_cancel` in a separate instrumentation invocation: it internally
repeats `guest`, and including an earlier `guest` in the same process can reuse
the resolved fixture URL and invalidate the expected extraction-call count.
These synthetic session/fallback checks do not validate saved account cookies.
Verified on the Redmi Note 8 Pro: `PASS: playlist_navigation` covered both UI
buttons, notification Next, playlist boundaries and Pause/Resume with a single
extraction. `guest,progressive_guest,music_radio_screenshot` also printed `PASS:`
on the same APK. Host checks passed 76 Android Rust and 94 Python tests.
Real Redmi radio checks use `-e scenarios music_radio,music_radio_second`,
or `-e scenarios music_radio_screenshot` for the latest Termux comparison
(`RD3LXr89pMU2Q`). These seeded radios must default to guest mode without
toggling the account selector. Both earlier screenshot
mixes, `RDUsR08cY8k0A` and `RD98WtmW-lfeE`, were verified with advancing libmpv
playback while the remaining queue was still resolving. These checks never
open saved cookies; they do not certify personalized recommendations. Stored
YouTube cookies on the Redmi produced yt-dlp's rotated-cookie warning.
`-e scenarios music_account` explicitly runs a strict account-only diagnostic
for the latest screenshot radio, independent of the automatic UI policy.
Unlike the synthetic scenarios, this opt-in test opens the owner's saved cookies
and requires their authorization. It limits extraction to two entries and never
retries anonymously; authentication
rejection fails the test and reports only a boolean, not cookie values. Playback
alone does not certify that the returned recommendations match the owner's account.
`music_account_auth` first checks the same saved jar with a normal yt-dlp
YouTube library-page request, without the app's extraction options, on the same
serial worker. It reports only authentication-cookie presence before/after and
the server's `LOGGED_IN` boolean, then attempts account playback only if accepted.
On the OPPO, the latest screenshot radio passed public playback with this build.
The owner then imported newly exported cookies: both strict account playback and
the baseline check failed. The baseline had authentication cookies before the
request, but YouTube returned `LOGGED_IN=false` and removed authentication.
The temporary jars were deleted. Account playback/personalization is therefore
**not verified or fixed** by the public-radio correction.
The updated APK was also tested on the Redmi Note 8 Pro on 2026-11-02:
`music_radio_screenshot,music_session,progressive_session` printed `PASS:`.
The exact screenshot radio started publicly while its queue was still resolving,
with playback reaching 2000 ms. Both real-cookie cases (`music_account` and
`music_account_auth`) failed: the baseline had authentication before the request,
but returned `LOGGED_IN=false` and lost authentication afterward. Temporary jars
were deleted; the test APK was removed without clearing the app's saved cookies.
For the earlier, pre-redesign Redmi 1080 x 2340 layout, tests used
`pasteX=915`, `pasteY=190`,
`playX=200`, `playY=2045`, `sessionX=42`, `sessionY=306`.
Navigation used `previousX=80`, `nextX=565`, and `previousY=nextY=2045`.
These historical coordinates must be remeasured for the retro-terminal UI.
The same Redmi build also passed `music_public` and nine selected recovery,
progressive-queue, cancellation and screen-off scenarios.
Success prints `PASS: foreground and screen-off renewal, cookie retry, pause, Stop, bounded exhaustion`.
Normal public YouTube video playback was also verified in the final app, with
rendered frames, advancing time and normal EOF after removing the test APK.

YouTube uses yt-dlp's automatic default client selection instead of forcing
`mweb`. On the Redmi, forcing `mweb` produced HTTP 403 while the same video
played with the default clients. The bundled bgutil **2.0.2** provider remains
available to generate content-bound PO Tokens when requested, through a local
Node.js **26.12.1** child process. Everything is bundled in the APK; no PC server,
listening HTTP port,
external token service or manually supplied token is required. Internet access
to YouTube is still necessary. Forcing `android` alone is not a reliable
replacement: that client can also require a PO Token. This does not hide your IP,
replace login, unlock private/membership/age-gated media, or guarantee that every
URL or network avoids YouTube's bot checks. The second, account-cookie attempt
is explicitly announced; final failures remain visible.

[prepare-anonymous-provider.ps1](./prepare-anonymous-provider.ps1), invoked by
[build-android-chaquopy.ps1](./build-android-chaquopy.ps1), verifies pinned download
SHA-256 digests, compiles upstream TypeScript with locked npm dependencies and
disabled install scripts, and packages the script/plugin and required isolated
native libraries. Node runs from `nativeLibraryDir`; provider files are unpacked
once to private app storage. TLS verification remains enabled using Chaquopy's
certifi CA bundle. QuickJS-ng remains available for yt-dlp-ejs. The upstream
optional canvas integration has no native Android addon; actual PO generation
was verified without it on the Redmi Note 8 Pro.
The installed APK was also verified on that phone: bgutil generated a nonempty,
content-bound token automatically, and the public `jNQXAC9IVRw` YouTube video
loaded and played through libmpv in both Audio and Video MP4 modes with rendered
frames and advancing playback time. Previously saved encrypted cookies remained
available for explicit deletion, but were not used during those successful guest
extractions. Cookie fallback ordering, error handling and cleanup are covered
with synthetic mocked credentials; real account access requires valid user cookies.

The APK includes upstream bgutil source, production dependency JS source and
license notices under `assets/sources`, `assets/licenses` and the provider ZIP.
Bgutil is **GPL-3.0-only**; the repository's existing MIT notices are retained.
Redistribution of the combined APK must satisfy the applicable GPLv3 source and
license obligations, including corresponding application/build source for the
distributed revision, not just the provider archive. Do not represent the whole
APK as MIT-only. Node and its required native libraries retain their own notices.
See the [official PO Token guide](https://github.com/yt-dlp/yt-dlp/wiki/PO-Token-Guide).

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
