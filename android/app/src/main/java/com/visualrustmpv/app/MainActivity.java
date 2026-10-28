package com.visualrustmpv.app;

import android.app.NativeActivity;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.os.Build;
import android.view.View;
import android.view.WindowInsets;
import android.view.WindowInsetsController;
import android.view.WindowManager;
import android.window.OnBackInvokedCallback;
import android.window.OnBackInvokedDispatcher;
import android.util.Log;
import com.chaquo.python.PyException;
import com.chaquo.python.PyObject;
import com.chaquo.python.Python;
import java.io.File;

/** NativeActivity plus a small bridge for the embedded yt-dlp runtime. */
public final class MainActivity extends NativeActivity {
    static {
        // NativeActivity's dlopen alone does not register Java native-method lookup.
        System.loadLibrary("app_android");
    }

    private static MainActivity instance;
    private boolean playerFullscreen;
    private boolean playerKeepScreenOn;
    private boolean playbackForeground;
    private OnBackInvokedCallback fullscreenBack;

    @Override
    public void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        instance = this;
        screenModeChanged(false);
    }

    private static native void setPlaybackForeground(boolean foreground);
    private static native void screenModeChanged(boolean fullscreen);

    /** Starts or updates the media service with lock-screen metadata for the loaded stream. */
    public void startPlaybackService(String title, String artist, String thumbnails) {
        Intent intent = new Intent(this, PlaybackService.class)
                .putExtra(PlaybackService.EXTRA_TITLE, title)
                .putExtra(PlaybackService.EXTRA_ARTIST, artist)
                .putExtra(PlaybackService.EXTRA_THUMBNAILS, thumbnails);
        if (Build.VERSION.SDK_INT >= 26) {
            startForegroundService(intent);
        } else {
            startService(intent);
        }
    }

    public void setPlayerKeepScreenOn(boolean keepScreenOn) {
        playerKeepScreenOn = keepScreenOn;
        applyKeepScreenOn();
    }

    private void applyKeepScreenOn() {
        boolean enabled = playerKeepScreenOn && playbackForeground;
        if (enabled) {
            getWindow().addFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        } else {
            getWindow().clearFlags(WindowManager.LayoutParams.FLAG_KEEP_SCREEN_ON);
        }
        Log.i("VisualRustMPV", "Pantalla encendida durante video: " + enabled);
    }

    /** Called on the Android UI thread; rotating keeps the NativeActivity and mpv alive. */
    public void setPlayerFullscreen(boolean fullscreen) {
        applySystemBars(fullscreen);
        setRequestedOrientation(fullscreen
                ? ActivityInfo.SCREEN_ORIENTATION_SENSOR_LANDSCAPE
                : ActivityInfo.SCREEN_ORIENTATION_PORTRAIT);
        playerFullscreen = fullscreen;
        if (Build.VERSION.SDK_INT >= 33) {
            if (fullscreen && fullscreenBack == null) {
                fullscreenBack = () -> setPlayerFullscreen(false);
                getOnBackInvokedDispatcher().registerOnBackInvokedCallback(
                        OnBackInvokedDispatcher.PRIORITY_DEFAULT, fullscreenBack);
            } else if (!fullscreen && fullscreenBack != null) {
                getOnBackInvokedDispatcher().unregisterOnBackInvokedCallback(fullscreenBack);
                fullscreenBack = null;
            }
        }
        screenModeChanged(fullscreen);
    }

    @SuppressWarnings("deprecation")
    private void applySystemBars(boolean fullscreen) {
        if (Build.VERSION.SDK_INT >= 30) {
            WindowInsetsController controller = getWindow().getInsetsController();
            if (controller == null) {
                throw new IllegalStateException("WindowInsetsController no disponible");
            }
            controller.setSystemBarsBehavior(
                    WindowInsetsController.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE);
            if (fullscreen) {
                controller.hide(WindowInsets.Type.systemBars());
            } else {
                controller.show(WindowInsets.Type.systemBars());
            }
        } else {
            getWindow().getDecorView().setSystemUiVisibility(fullscreen
                    ? View.SYSTEM_UI_FLAG_FULLSCREEN
                            | View.SYSTEM_UI_FLAG_HIDE_NAVIGATION
                            | View.SYSTEM_UI_FLAG_IMMERSIVE_STICKY
                    : View.SYSTEM_UI_FLAG_VISIBLE);
        }
    }

    @Override
    public void onWindowFocusChanged(boolean hasFocus) {
        super.onWindowFocusChanged(hasFocus);
        if (hasFocus && playerFullscreen) {
            applySystemBars(true);
        }
    }

    @Override
    @SuppressWarnings("deprecation")
    public void onBackPressed() {
        if (playerFullscreen) {
            setPlayerFullscreen(false);
        } else {
            super.onBackPressed();
        }
    }

    @Override
    protected void onPause() {
        playbackForeground = false;
        applyKeepScreenOn();
        setPlaybackForeground(false);
        super.onPause();
    }

    @Override
    protected void onResume() {
        super.onResume();
        playbackForeground = true;
        applyKeepScreenOn();
        setPlaybackForeground(true);
    }

    /** Resolves the direct stream and lock-screen metadata through the persistent yt-dlp worker. */
    public static String extractMedia(String url, boolean audioOnly) {
        try {
            return ytDlpRunner().callAttr(
                    "extract_media", url, audioOnly, quickJsPath(), ytDlpCacheDir()).toString();
        } catch (PyException error) {
            Log.e("VisualRustMPV", "Chaquopy/yt-dlp extraction failed", error);
            throw error;
        }
    }

    /** Imports yt-dlp, checks QuickJS-ng and creates the reusable downloaders. Returns seconds. */
    public static double warmUpYtDlp() {
        try {
            // Must run before anything imports yt_dlp so a downloaded update can take precedence.
            String update = Python.getInstance().getModule("yt_dlp_update")
                    .callAttr("activate", ytDlpUpdateDir(), apkStamp()).toString();
            if (!update.isEmpty()) {
                Log.i("VisualRustMPV", "yt-dlp actualizado activo: " + update);
            }
        } catch (PyException error) {
            Log.e("VisualRustMPV", "Activar actualizacion de yt-dlp", error);
        }
        try {
            return ytDlpRunner().callAttr("warm_up", quickJsPath(), ytDlpCacheDir()).toDouble();
        } catch (PyException error) {
            Log.e("VisualRustMPV", "Chaquopy/yt-dlp warm-up failed", error);
            rejectYtDlpUpdate();
            throw error;
        }
    }

    /** Version label shown in the UI, e.g. "2026.10.26" or "2026.11.02 (actualizado)". */
    public static String ytDlpVersion() {
        return ytDlpRunner().callAttr("version_label").toString();
    }

    /** Confirms or rejects a downloaded yt-dlp with one real extraction; "" if none is on trial. */
    public static String verifyYtDlpUpdate() {
        return ytDlpRunner().callAttr(
                "verify_update", ytDlpUpdateDir(), quickJsPath(), ytDlpCacheDir()).toString();
    }

    /** Downloads a newer yt-dlp for the next start; "" when nothing changed. */
    public static String checkYtDlpUpdate() {
        return ytDlpRunner().callAttr("check_update", ytDlpUpdateDir(), apkStamp()).toString();
    }

    private static void rejectYtDlpUpdate() {
        try {
            boolean rejected = Python.getInstance().getModule("yt_dlp_update")
                    .callAttr("reject_active", ytDlpUpdateDir()).toBoolean();
            if (rejected) {
                Log.w("VisualRustMPV", "yt-dlp actualizado descartado; se usara el incluido al reiniciar");
            }
        } catch (PyException error) {
            Log.e("VisualRustMPV", "Descartar actualizacion de yt-dlp", error);
        }
    }

    private static String ytDlpUpdateDir() {
        if (instance == null) {
            throw new IllegalStateException("Activity no disponible para actualizar yt-dlp");
        }
        return new File(instance.getFilesDir(), "yt-dlp-update").getAbsolutePath();
    }

    /** Changes on every APK install, so a reinstall always reverts to the bundled yt-dlp. */
    private static String apkStamp() {
        try {
            android.content.pm.PackageInfo info =
                    instance.getPackageManager().getPackageInfo(instance.getPackageName(), 0);
            return info.versionName + "/" + info.lastUpdateTime;
        } catch (android.content.pm.PackageManager.NameNotFoundException error) {
            throw new IllegalStateException(error);
        }
    }

    public static String warmUpYouTubePlayer() {
        try {
            return ytDlpRunner().callAttr("warm_player", quickJsPath(), ytDlpCacheDir()).toString();
        } catch (PyException error) {
            Log.e("VisualRustMPV", "YouTube player warm-up failed", error);
            throw error;
        }
    }

    private static PyObject ytDlpRunner() {
        return Python.getInstance().getModule("yt_dlp_runner");
    }

    private static String quickJsPath() {
        if (instance == null) {
            throw new IllegalStateException("Activity no disponible para localizar QuickJS-ng");
        }
        return new File(instance.getApplicationInfo().nativeLibraryDir, "libqjs.so").getAbsolutePath();
    }

    private static String ytDlpCacheDir() {
        if (instance == null) {
            throw new IllegalStateException("Activity no disponible para la cache de yt-dlp");
        }
        return new File(instance.getCacheDir(), "yt-dlp").getAbsolutePath();
    }

    /** Returns the CA bundle extracted by Chaquopy for native HTTPS clients. */
    public static String caBundlePath() {
        return Python.getInstance().getModule("certifi").callAttr("where").toString();
    }

    /** Reads plain text from Android's system clipboard. */
    public static String readClipboard() {
        if (instance == null) return "";
        ClipboardManager clipboard = (ClipboardManager) instance.getSystemService(Context.CLIPBOARD_SERVICE);
        if (clipboard == null || !clipboard.hasPrimaryClip()) return "";
        CharSequence text = clipboard.getPrimaryClip().getItemAt(0).coerceToText(instance);
        return text == null ? "" : text.toString();
    }
}
