package com.visualrustmpv.app;

import android.app.NativeActivity;
import android.content.ClipboardManager;
import android.content.Context;
import android.content.Intent;
import android.content.pm.ActivityInfo;
import android.graphics.Rect;
import android.net.Uri;
import android.os.Build;
import android.security.keystore.KeyGenParameterSpec;
import android.security.keystore.KeyProperties;
import android.util.AtomicFile;
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
import java.io.ByteArrayOutputStream;
import java.io.File;
import java.io.FileNotFoundException;
import java.io.FileInputStream;
import java.io.FileOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.nio.charset.StandardCharsets;
import java.security.GeneralSecurityException;
import java.security.KeyStore;
import java.util.Arrays;
import java.util.Locale;
import javax.crypto.Cipher;
import javax.crypto.KeyGenerator;
import javax.crypto.SecretKey;
import javax.crypto.spec.GCMParameterSpec;

/** NativeActivity plus a small bridge for the embedded yt-dlp runtime. */
public final class MainActivity extends NativeActivity {
    private static final int REQUEST_YOUTUBE_COOKIES = 1401;
    private static final int MAX_COOKIE_FILE_BYTES = 1024 * 1024;
    private static final String COOKIE_KEY_ALIAS = "visualrustmpv-youtube-cookies-v1";

    static {
        // NativeActivity's dlopen alone does not register Java native-method lookup.
        System.loadLibrary("app_android");
    }

    private static MainActivity instance;
    private boolean playerFullscreen;
    private boolean playerKeepScreenOn;
    private boolean playbackForeground;
    private OnBackInvokedCallback fullscreenBack;
    private volatile String youtubeCookiesStatus = "";
    private static boolean anonymousProviderInitialized;

    @Override
    public void onCreate(android.os.Bundle state) {
        super.onCreate(state);
        instance = this;
        cleanupTemporaryYoutubeCookies();
        screenModeChanged(false);
        getWindow().getDecorView().getViewTreeObserver().addOnGlobalLayoutListener(() -> {
            Rect visible = new Rect();
            View decor = getWindow().getDecorView();
            decor.getWindowVisibleDisplayFrame(visible);
            int[] origin = new int[2];
            decor.getLocationOnScreen(origin);
            visibleWindowBottom(visible.bottom - origin[1]);
        });
    }

    private static native void setPlaybackForeground(boolean foreground);
    private static native void screenModeChanged(boolean fullscreen);
    private static native void visibleWindowBottom(int bottom);

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
        return extractMediaRequest(url, audioOnly, false, false);
    }

    public static String retryMediaPlayback(String url, boolean audioOnly, boolean cookiesOnly) {
        return extractMediaRequest(url, audioOnly, cookiesOnly, true);
    }

    public static native boolean isMediaExtractionCancelled(long generation);
    public static native void reportMediaExtractionProgress(long generation, String message);
    public static native void reportMediaExtractionItem(long generation, String media);

    public static String extractMediaCancellable(String url, boolean audioOnly,
            boolean cookiesOnly, boolean refresh, long generation) {
        try {
            return Python.getInstance().getModule("yt_dlp_runner").callAttr(
                    "extract_media_request", url, audioOnly, quickJsPath(), ytDlpCacheDir(),
                    instance, cookiesOnly, refresh, generation).toString();
        } catch (PyException error) {
            Log.e("VisualRustMPV", "Chaquopy/yt-dlp request failed", error);
            throw error;
        }
    }

    private static String extractMediaRequest(
            String url, boolean audioOnly, boolean cookiesOnly, boolean refresh) {
        try {
            return Python.getInstance().getModule("yt_dlp_runner").callAttr(
                    "extract_media_with_fallback",
                    url,
                    audioOnly,
                    quickJsPath(),
                    ytDlpCacheDir(),
                    instance,
                    cookiesOnly,
                    refresh).toString();
        } catch (PyException error) {
            Log.e("VisualRustMPV", "Chaquopy/yt-dlp extraction failed", error);
            throw error;
        }
    }

    public String openSupportedSiteCookiesFile() {
        Intent intent = new Intent(Intent.ACTION_OPEN_DOCUMENT)
                .addCategory(Intent.CATEGORY_OPENABLE)
                .setType("*/*")
                .putExtra(Intent.EXTRA_MIME_TYPES,
                        new String[] {"text/plain", "application/octet-stream"})
                .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION);
        startActivityForResult(intent, REQUEST_YOUTUBE_COOKIES);
        return "opened";
    }

    public String hasSupportedSiteCookies() throws IOException {
        try (FileInputStream ignored = youtubeCookiesStore().openRead()) {
            return "available";
        } catch (FileNotFoundException error) {
            return "empty";
        }
    }

    public synchronized String takeSupportedSiteCookiesStatus() {
        String status = youtubeCookiesStatus;
        youtubeCookiesStatus = "";
        return status;
    }

    public synchronized void setSupportedSiteCookiesFallbackStatus(String status) {
        youtubeCookiesStatus = status;
    }

    public String prepareAnonymousExtraction() {
        try {
            ytDlpRunner();
            return "";
        } catch (IllegalStateException | PyException error) {
            Log.e("VisualRustMPV", "Preparar intento anonimo; se comprobara el fallback", error);
            return "No se pudo preparar el proveedor PO anonimo: " + error.getMessage();
        }
    }

    /** Worker-thread callback; never called by the successful anonymous attempt. */
    public String prepareSupportedSiteCookiesForFallback(String url)
            throws GeneralSecurityException, IOException {
        String host = Uri.parse(url).getHost();
        if (host == null) {
            throw new IOException("La URL no tiene un sitio valido para cookies");
        }
        String root = supportedCookieRoot(host.equalsIgnoreCase("youtu.be") ? "youtube.com" : host);
        if (root == null) {
            return "";
        }
        byte[] encrypted;
        try (FileInputStream stored = youtubeCookiesStore().openRead();
                ByteArrayOutputStream output = new ByteArrayOutputStream()) {
            byte[] buffer = new byte[8192];
            int count;
            while ((count = stored.read(buffer)) != -1) {
                if (output.size() + count > MAX_COOKIE_FILE_BYTES + 64) {
                    throw new IOException("El almacenamiento de cookies supera el limite");
                }
                output.write(buffer, 0, count);
            }
            encrypted = output.toByteArray();
        } catch (FileNotFoundException error) {
            return "";
        }
        byte[] contents = null;
        byte[] filtered = null;
        try {
            if (encrypted.length < 30 || encrypted[0] != 1 || encrypted[1] != 12) {
                throw new IOException("El almacenamiento de cookies no tiene un formato valido");
            }
            Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
            cipher.init(Cipher.DECRYPT_MODE, youtubeCookiesKey(),
                    new GCMParameterSpec(128, Arrays.copyOfRange(encrypted, 2, 14)));
            cipher.updateAAD(COOKIE_KEY_ALIAS.getBytes(StandardCharsets.US_ASCII));
            contents = cipher.doFinal(encrypted, 14, encrypted.length - 14);
            StringBuilder cookies = new StringBuilder("# Netscape HTTP Cookie File\n");
            boolean found = false;
            for (String line : new String(contents, StandardCharsets.UTF_8).split("\\r?\\n")) {
                String[] fields = line.split("\\t", -1);
                if (fields.length != 7) {
                    continue;
                }
                String domain = fields[0].startsWith("#HttpOnly_")
                        ? fields[0].substring("#HttpOnly_".length()) : fields[0];
                if (root.equals(supportedCookieRoot(domain))) {
                    cookies.append(line).append('\n');
                    found = true;
                }
            }
            if (!found) {
                return "";
            }
            filtered = cookies.toString().getBytes(StandardCharsets.UTF_8);
            File temporary = File.createTempFile("supported-site-cookies-", ".txt", getCacheDir());
            try (FileOutputStream output = new FileOutputStream(temporary)) {
                output.write(filtered);
                output.getFD().sync();
            } catch (IOException error) {
                if (!temporary.delete()) {
                    Log.e("VisualRustMPV", "No se pudo limpiar el archivo temporal de cookies incompleto");
                }
                throw error;
            }
            return temporary.getAbsolutePath();
        } finally {
            Arrays.fill(encrypted, (byte) 0);
            if (contents != null) Arrays.fill(contents, (byte) 0);
            if (filtered != null) Arrays.fill(filtered, (byte) 0);
        }
    }

    public String clearSupportedSiteCookies() {
        AtomicFile store = youtubeCookiesStore();
        store.delete();
        if (store.getBaseFile().exists()) {
            throw new IllegalStateException("No se pudo eliminar el archivo de cookies");
        }
        youtubeCookiesStatus = "deleted";
        return "deleted";
    }

    @Override
    protected void onActivityResult(int requestCode, int resultCode, Intent data) {
        super.onActivityResult(requestCode, resultCode, data);
        if (requestCode != REQUEST_YOUTUBE_COOKIES) {
            return;
        }
        if (resultCode != RESULT_OK || data == null || data.getData() == null) {
            youtubeCookiesStatus = "cancelled";
            return;
        }
        byte[] contents = null;
        byte[] supportedSiteCookies = null;
        try {
            contents = readYoutubeCookieFile(data.getData());
            supportedSiteCookies = supportedSiteCookiesOnly(contents);
            encryptAndStoreYoutubeCookies(supportedSiteCookies);
            youtubeCookiesStatus = "imported";
        } catch (InvalidCookieFileException error) {
            youtubeCookiesStatus = "invalid";
            Log.w("VisualRustMPV", "Archivo de cookies no valido: " + error.getMessage());
        } catch (Exception error) {
            youtubeCookiesStatus = "error";
            Log.e("VisualRustMPV", "No se pudieron importar las cookies", error);
        } finally {
            if (contents != null) {
                Arrays.fill(contents, (byte) 0);
            }
            if (supportedSiteCookies != null) {
                Arrays.fill(supportedSiteCookies, (byte) 0);
            }
        }
    }

    private byte[] readYoutubeCookieFile(Uri uri) throws IOException {
        InputStream input = getContentResolver().openInputStream(uri);
        if (input == null) {
            throw new IOException("El proveedor de archivos no abrio el documento");
        }
        try (InputStream stream = input; ByteArrayOutputStream output = new ByteArrayOutputStream()) {
            byte[] buffer = new byte[8192];
            int total = 0;
            int count;
            while ((count = stream.read(buffer)) != -1) {
                total += count;
                if (total > MAX_COOKIE_FILE_BYTES) {
                    throw new InvalidCookieFileException("El archivo supera 1 MiB");
                }
                output.write(buffer, 0, count);
            }
            return output.toByteArray();
        }
    }

    private byte[] supportedSiteCookiesOnly(byte[] contents) throws InvalidCookieFileException {
        String text = new String(contents, StandardCharsets.UTF_8);
        boolean hasSupportedCookie = false;
        StringBuilder filtered = new StringBuilder();
        for (String line : text.split("\\r?\\n")) {
            if (line.isEmpty() || (line.startsWith("#") && !line.startsWith("#HttpOnly_"))) {
                if (!line.isEmpty()) {
                    filtered.append(line).append('\n');
                }
                continue;
            }
            String[] fields = line.split("\\t", -1);
            if (fields.length != 7) {
                throw new InvalidCookieFileException("No tiene el formato Netscape");
            }
            String domain = fields[0].startsWith("#HttpOnly_")
                    ? fields[0].substring("#HttpOnly_".length())
                    : fields[0];
            if (isSupportedCookieDomain(domain)) {
                hasSupportedCookie = true;
                filtered.append(line).append('\n');
            }
        }
        if (!hasSupportedCookie) {
            throw new InvalidCookieFileException("No contiene cookies de los sitios compatibles");
        }
        return filtered.toString().getBytes(StandardCharsets.UTF_8);
    }

    private boolean isSupportedCookieDomain(String domain) {
        return supportedCookieRoot(domain) != null;
    }

    private String supportedCookieRoot(String domain) {
        String normalized = (domain.startsWith(".") ? domain.substring(1) : domain)
                .toLowerCase(Locale.ROOT);
        for (String root : new String[] {
                "youtube.com", "youtube-nocookie.com",
                "x.com", "twitter.com",
                "instagram.com", "instagr.am",
                "facebook.com", "fb.com", "fb.watch"
        }) {
            if (normalized.equals(root) || normalized.endsWith("." + root)) {
                if (root.equals("youtube-nocookie.com")) return "youtube.com";
                if (root.equals("x.com")) return "twitter.com";
                if (root.equals("instagr.am")) return "instagram.com";
                if (root.equals("fb.com") || root.equals("fb.watch")) return "facebook.com";
                return root;
            }
        }
        return null;
    }

    private void encryptAndStoreYoutubeCookies(byte[] contents)
            throws GeneralSecurityException, IOException {
        Cipher cipher = Cipher.getInstance("AES/GCM/NoPadding");
        cipher.init(Cipher.ENCRYPT_MODE, youtubeCookiesKey());
        cipher.updateAAD(COOKIE_KEY_ALIAS.getBytes(StandardCharsets.US_ASCII));
        byte[] encrypted = cipher.doFinal(contents);
        byte[] iv = cipher.getIV();
        try (ByteArrayOutputStream output = new ByteArrayOutputStream()) {
            output.write(1);
            output.write(iv.length);
            output.write(iv);
            output.write(encrypted);
            AtomicFile store = youtubeCookiesStore();
            FileOutputStream stream = store.startWrite();
            try {
                stream.write(output.toByteArray());
                store.finishWrite(stream);
            } catch (IOException error) {
                store.failWrite(stream);
                throw error;
            }
        } finally {
            Arrays.fill(encrypted, (byte) 0);
        }
    }

    private SecretKey youtubeCookiesKey() throws GeneralSecurityException, IOException {
        KeyStore store = KeyStore.getInstance("AndroidKeyStore");
        store.load(null);
        if (!store.containsAlias(COOKIE_KEY_ALIAS)) {
            KeyGenerator generator = KeyGenerator.getInstance(
                    KeyProperties.KEY_ALGORITHM_AES, "AndroidKeyStore");
            generator.init(new KeyGenParameterSpec.Builder(
                    COOKIE_KEY_ALIAS,
                    KeyProperties.PURPOSE_ENCRYPT | KeyProperties.PURPOSE_DECRYPT)
                    .setBlockModes(KeyProperties.BLOCK_MODE_GCM)
                    .setEncryptionPaddings(KeyProperties.ENCRYPTION_PADDING_NONE)
                    .build());
            generator.generateKey();
        }
        return (SecretKey) store.getKey(COOKIE_KEY_ALIAS, null);
    }

    private AtomicFile youtubeCookiesStore() {
        return new AtomicFile(new File(getFilesDir(), "youtube-cookies.enc"));
    }

    private void cleanupTemporaryYoutubeCookies() {
        File[] temporaryFiles = getCacheDir().listFiles((directory, name) ->
                (name.startsWith("youtube-cookies-") || name.startsWith("supported-site-cookies-"))
                        && name.endsWith(".txt"));
        if (temporaryFiles == null) {
            Log.w("VisualRustMPV", "No se pudieron buscar archivos temporales de cookies");
            return;
        }
        for (File file : temporaryFiles) {
            if (!file.delete()) {
                Log.w("VisualRustMPV", "No se pudo eliminar un archivo temporal anterior de cookies");
            }
        }
    }

    private static final class InvalidCookieFileException extends IOException {
        InvalidCookieFileException(String message) {
            super(message);
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

    /** Downloads a newer yt-dlp for the next start; "" when nothing changed unless forced. */
    public static String checkYtDlpUpdate(boolean force) {
        return ytDlpRunner().callAttr("check_update", ytDlpUpdateDir(), apkStamp(), force).toString();
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

    private static synchronized PyObject ytDlpRunner() {
        if (!anonymousProviderInitialized) {
            AtomicFile bundle = new AtomicFile(new File(instance.getCacheDir(), "anonymous-provider.zip"));
            try (InputStream input = instance.getAssets().open("anonymous-provider.zip")) {
                FileOutputStream output = bundle.startWrite();
                try {
                    byte[] buffer = new byte[32768];
                    int count;
                    while ((count = input.read(buffer)) != -1) {
                        output.write(buffer, 0, count);
                    }
                    bundle.finishWrite(output);
                } catch (IOException error) {
                    bundle.failWrite(output);
                    throw error;
                }
                Python.getInstance().getModule("anonymous_provider").callAttr(
                        "initialize",
                        new File(instance.getApplicationInfo().nativeLibraryDir, "libnode.so").getAbsolutePath(),
                        bundle.getBaseFile().getAbsolutePath(),
                        new File(instance.getFilesDir(), "anonymous-provider").getAbsolutePath(),
                        caBundlePath());
                anonymousProviderInitialized = true;
            } catch (IOException | PyException error) {
                Log.e("VisualRustMPV", "Inicializar proveedor PO anonimo", error);
                throw new IllegalStateException("No se pudo iniciar el proveedor PO local", error);
            } finally {
                bundle.delete();
            }
        }
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
