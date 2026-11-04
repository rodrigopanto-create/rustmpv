package com.visualrustmpv.app;

import android.app.Activity;
import android.app.Instrumentation;
import android.app.NotificationManager;
import android.app.KeyguardManager;
import android.content.ClipData;
import android.content.ClipboardManager;
import android.content.Intent;
import android.os.Bundle;
import android.os.ParcelFileDescriptor;
import android.os.SystemClock;
import android.os.PowerManager;
import android.service.notification.StatusBarNotification;
import android.util.Log;
import com.chaquo.python.PyObject;
import com.chaquo.python.Python;
import java.io.ByteArrayOutputStream;
import java.io.InputStream;
import java.lang.reflect.Method;
import java.util.Arrays;
import org.json.JSONObject;

/** Exercises JNI/libmpv with synthetic recovery fixtures and opt-in public media. */
public final class PlaybackRecoveryInstrumentation extends Instrumentation {
    private Bundle arguments;
    private PyObject probe;
    private Method tick;
    private Method position;
    private Method control;
    private Activity activity;

    @Override public void onCreate(Bundle arguments) {
        this.arguments = arguments;
        start();
    }

    @Override public void onStart() {
        Bundle result = new Bundle();
        int code = Activity.RESULT_CANCELED;
        try {
            activity = startActivitySync(new Intent(getTargetContext(), MainActivity.class)
                    .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK));
            // The startup splash may remain visible for eight seconds on a cold worker.
            SystemClock.sleep(12000);
            tick = nativeMethod("nativeTick");
            position = nativeMethod("nativePosition");
            control = nativeMethod("nativeControl", int.class);
            String source;
            try (InputStream input = getContext().getAssets().open("playback_recovery_probe.py")) {
                ByteArrayOutputStream bytes = new ByteArrayOutputStream();
                byte[] buffer = new byte[4096];
                for (int count; (count = input.read(buffer)) != -1;) {
                    bytes.write(buffer, 0, count);
                }
                source = bytes.toString("UTF-8");
            }
            probe = Python.getInstance().getModule("types").callAttr("ModuleType", "_recovery_probe");
            Python.getInstance().getModule("builtins").callAttr(
                    "exec", source, probe.get("__dict__"));
            String selected = arguments.getString("scenarios");
            boolean matched = false;
            for (String scenario : new String[] {"guest", "cookies", "cancel", "exhaust",
                    "background_guest", "background_cookies", "background_pause",
                    "background_cancel", "background_exhaust", "resolve_cancel", "music_session",
                    "music_public", "progressive_guest", "progressive_session",
                    "progressive_cancel", "background_progressive",
                    "music_compare_m4a", "music_compare_opus", "music_compare_default",
                    "music_radio", "music_radio_second", "music_radio_screenshot",
                    "music_account", "music_account_auth", "playlist_navigation",
                    "cross_site_youtube", "cross_site_twitter", "cross_site_instagram",
                    "cross_site_facebook", "cross_site_return"}) {
                if ((scenario.equals("resolve_cancel") || scenario.startsWith("music_")
                        || scenario.contains("progressive") || scenario.equals("playlist_navigation")
                        || scenario.startsWith("cross_site"))
                        && selected == null) continue;
                if (selected != null && !Arrays.asList(selected.split(",")).contains(scenario)) {
                    continue;
                }
                matched = true;
                runScenario(scenario);
            }
            if (!matched) throw new AssertionError("No matching recovery scenarios");
            result.putString("stream", selected == null
                    ? "PASS: foreground and screen-off renewal, cookie retry, pause, Stop, bounded exhaustion"
                    : "PASS: " + selected);
            code = Activity.RESULT_OK;
        } catch (Exception | AssertionError error) {
            Log.e("RecoveryTest", "Device recovery test failed", error);
            result.putString("stream", "FAIL: " + error);
        } finally {
            try {
                shell("input keyevent KEYCODE_WAKEUP");
                if (control != null) control.invoke(null, 2);
                if (tick != null) tick.invoke(null);
                if (probe != null) {
                    probe.callAttr("close");
                    probe.callAttr("clean_history", getTargetContext().getFilesDir().getAbsolutePath());
                }
            } catch (Exception error) {
                Log.e("RecoveryTest", "Fixture cleanup failed", error);
                result.putString("stream", "FAIL: fixture cleanup: " + error);
                code = Activity.RESULT_CANCELED;
            }
            finish(code, result);
        }
    }

    private Method nativeMethod(String name, Class<?>... parameters) throws Exception {
        Method method = PlaybackService.class.getDeclaredMethod(name, parameters);
        method.setAccessible(true);
        return method;
    }

    private void tap(String prefix) throws Exception {
        int x = Integer.parseInt(arguments.getString(prefix + "X"));
        int y = Integer.parseInt(arguments.getString(prefix + "Y"));
        shell("input tap " + x + " " + y);
    }

    private void shell(String command) throws Exception {
        try (ParcelFileDescriptor descriptor =
                getUiAutomation().executeShellCommand(command);
             InputStream input = new ParcelFileDescriptor.AutoCloseInputStream(descriptor)) {
            while (input.read() != -1) { }
        }
    }

    private JSONObject state() throws Exception {
        return new JSONObject(probe.callAttr("state").toString());
    }

    private void notificationAction(int index) throws Exception {
        StatusBarNotification[] notifications = getTargetContext()
                .getSystemService(NotificationManager.class).getActiveNotifications();
        for (StatusBarNotification notification : notifications) {
            if (notification.getNotification().actions != null
                    && notification.getNotification().actions.length > index) {
                notification.getNotification().actions[index].actionIntent.send();
                return;
            }
        }
        throw new AssertionError("Playback notification disappeared during renewal");
    }

    private void runScenario(String scenario) throws Exception {
        boolean background = scenario.startsWith("background");
        shell("input keyevent KEYCODE_WAKEUP");
        shell("am start -n com.visualrustmpv.app/.MainActivity");
        KeyguardManager keyguard = getTargetContext().getSystemService(KeyguardManager.class);
        if (keyguard.isKeyguardLocked()) {
            // Uses Android's normal dismissal flow; secure locks still require the owner.
            runOnMainSync(() -> keyguard.requestDismissKeyguard(activity, null));
            Log.i("RecoveryTest", "Unlock the phone to continue: " + scenario);
            long unlockDeadline = SystemClock.elapsedRealtime() + 120000;
            while (keyguard.isKeyguardLocked() && SystemClock.elapsedRealtime() < unlockDeadline) {
                SystemClock.sleep(100);
            }
            if (keyguard.isKeyguardLocked()) {
                throw new AssertionError("Unlock the phone before the next device scenario");
            }
        }
        SystemClock.sleep(1500);
        control.invoke(null, 2);
        if (scenario.startsWith("cross_site")) verifySiteCookieMerge();
        String url = probe.callAttr("configure", scenario).toString();
        runOnMainSync(() -> getTargetContext().getSystemService(ClipboardManager.class)
                .setPrimaryClip(ClipData.newPlainText("Recovery test", url)));
        tap("paste");
        SystemClock.sleep(500);
        shell("input keyevent KEYCODE_BACK");
        SystemClock.sleep(700);
        tap("play");
        if (scenario.equals("playlist_navigation")) {
            runPlaylistNavigation();
            return;
        }
        if (scenario.contains("progressive")) {
            runProgressiveScenario(scenario);
            return;
        }
        boolean stopped = false;
        boolean screenOff = false;
        boolean paused = false;
        // A cold worker can still be preparing the real YouTube player.
        long deadline = SystemClock.elapsedRealtime() + 120000;
        while (SystemClock.elapsedRealtime() < deadline) {
            JSONObject state = state();
            if (state.optBoolean("account_failed")) {
                throw new AssertionError("Account extraction failed: " + state);
            }
            int calls = state.getJSONArray("calls").length();
            // While screen-off, only the service ticker may drive extraction/loading.
            long flags = screenOff ? 0 : (long) tick.invoke(null);
            long milliseconds = (long) position.invoke(null);
            if (background && !screenOff && state.getInt("served") >= 1 && milliseconds >= 300) {
                shell("input keyevent KEYCODE_SLEEP");
                SystemClock.sleep(500);
                if (getTargetContext().getSystemService(PowerManager.class).isInteractive()) {
                    throw new AssertionError("Screen did not turn off");
                }
                screenOff = true;
                continue;
            }
            if (scenario.endsWith("cancel") && calls == 2 && !stopped) {
                if (background) notificationAction(1);
                else if (!(boolean) control.invoke(null, 2)) throw new AssertionError("Stop failed");
                stopped = true;
                SystemClock.sleep(5000);
                continue;
            }
            if (scenario.equals("resolve_cancel") && calls == 1) {
                tap("cancel");
                long cancelledDeadline = SystemClock.elapsedRealtime() + 5000;
                while (!state().optBoolean("cancelled")
                        && SystemClock.elapsedRealtime() < cancelledDeadline) {
                    SystemClock.sleep(100);
                }
                if (!state().optBoolean("cancelled")) {
                    throw new AssertionError("Cancel button did not interrupt stuck extraction");
                }
                // Paste/Play on a new input verifies the editor and serial worker are reusable.
                runScenario("guest");
                Log.i("RecoveryTest", "PASS resolve_cancel: stuck extractor cancelled; next URL played");
                return;
            }
            if (scenario.endsWith("pause") && calls == 2 && !paused) {
                notificationAction(0);
                paused = true;
                continue;
            }
            if (background && state.getInt("served") >= 2 && !scenario.endsWith("cancel")) {
                // The second stream was already loaded by the service, not this probe.
                flags = (long) tick.invoke(null);
                if (scenario.endsWith("pause") && paused && (flags & 7) == 7) {
                    long before = (long) position.invoke(null);
                    SystemClock.sleep(1000);
                    if ((long) position.invoke(null) > before + 200) {
                        throw new AssertionError("Recovered stream ignored Pause");
                    }
                    notificationAction(0);
                    paused = false;
                    SystemClock.sleep(2500);
                    milliseconds = (long) position.invoke(null);
                    if (milliseconds < before + 2000) {
                        throw new AssertionError("Notification Resume failed after renewal");
                    }
                }
            }
            if (background && scenario.endsWith("exhaust") && calls == 3) {
                SystemClock.sleep(1000);
                flags = (long) tick.invoke(null);
            }
            boolean complete = scenario.endsWith("cancel") ? stopped
                    : scenario.endsWith("exhaust") ? calls == 3 && (flags & 16) != 0
                    : scenario.equals("music_session")
                            ? state.getInt("served") >= 2 && (flags & 5) == 5 && milliseconds >= 2000
                    : scenario.startsWith("music_account")
                            ? state.getBoolean("done") && (flags & 5) == 5 && milliseconds >= 2000
                    : (flags & 5) == 5 && milliseconds >= 2000;
            if (complete) {
                SystemClock.sleep(1500);
                state = state();
                if (scenario.startsWith("music_radio") && state.getBoolean("done")) {
                    throw new AssertionError("Radio playback waited for the complete queue");
                }
                String expected = scenario.startsWith("cross_site")
                        ? (scenario.endsWith("youtube") || scenario.endsWith("return")
                                ? "[\"guest\"]" : "[\"cookies\"]")
                        : (scenario.equals("music_public") || scenario.startsWith("music_compare")
                        || scenario.startsWith("music_radio")) ? "[\"guest\"]"
                        : (scenario.equals("music_session") || scenario.startsWith("music_account"))
                                ? "[\"cookies\"]"
                        : scenario.endsWith("cookies") || scenario.endsWith("exhaust")
                                ? "[\"guest\",\"guest\",\"cookies\"]" : "[\"guest\",\"guest\"]";
                if (!state.getJSONArray("calls").toString().equals(expected)
                        || !state.getBoolean("cookie_deleted")) {
                    throw new AssertionError(scenario + ": wrong retry order/cleanup: " + state);
                }
                if (scenario.endsWith("cancel") && (long) tick.invoke(null) != 0) {
                    throw new AssertionError("Cancelled retry restarted playback");
                }
                if (!scenario.endsWith("cancel") && !scenario.startsWith("music_")
                        && !scenario.startsWith("cross_site")
                        && state.getInt("denied") < 1) {
                    throw new AssertionError("No actual HTTP 403 was consumed");
                }

                if (background && (!screenOff || getTargetContext()
                        .getSystemService(PowerManager.class).isInteractive())) {
                    throw new AssertionError("Background recovery was not verified screen-off");
                }
                Log.i("RecoveryTest", "PASS " + scenario + ": " + state + "; position=" + milliseconds);
                return;
            }
            SystemClock.sleep(100);
        }
        throw new AssertionError(scenario + ": recovery timeout: " + state());
    }

    private void verifySiteCookieMerge() {
        String youtube = ".youtube.com\tTRUE\t/\tTRUE\t0\tTEST\tyoutube\n";
        String oldTwitter = ".x.com\tTRUE\t/\tTRUE\t0\tTEST\told\n";
        String newTwitter = ".twitter.com\tTRUE\t/\tTRUE\t0\tTEST\tnew\n";
        String instagram = "#HttpOnly_.instagram.com\tTRUE\t/\tTRUE\t0\tTEST\tinstagram\n";
        byte[] stored = (youtube + oldTwitter).getBytes(java.nio.charset.StandardCharsets.UTF_8);
        byte[] imported = (newTwitter + instagram).getBytes(java.nio.charset.StandardCharsets.UTF_8);
        String merged = new String(SiteCookies.merge(stored, imported),
                java.nio.charset.StandardCharsets.UTF_8);
        if (!merged.contains(youtube) || !merged.contains(newTwitter)
                || !merged.contains(instagram) || merged.contains(oldTwitter)) {
            throw new AssertionError("Cookie merge did not preserve other sites or replace aliases");
        }
        if (SiteCookies.root("instagram.com.evil.example") != null
                || !"facebook.com".equals(SiteCookies.root("fb.watch"))) {
            throw new AssertionError("Cookie domain grouping is not isolated");
        }
    }

    private void awaitNavigation(int expected, boolean paused) throws Exception {
        long deadline = SystemClock.elapsedRealtime() + 15000;
        while (SystemClock.elapsedRealtime() < deadline) {
            long flags = (long) tick.invoke(null);
            if ((flags & 16) != 0) throw new AssertionError("Navigation caused a playback error");
            if ((flags & 197) == (expected | 5) && ((flags & 2) != 0) == paused
                    && (paused || (long) position.invoke(null) >= 300)) return;
            SystemClock.sleep(100);
        }
        throw new AssertionError("Navigation state timeout: " + tick.invoke(null));
    }

    private void runPlaylistNavigation() throws Exception {
        awaitNavigation(128, false);
        if ((boolean) control.invoke(null, 3)) {
            throw new AssertionError("Previous was accepted at the start of the playlist");
        }
        if (!(boolean) control.invoke(null, 1)) throw new AssertionError("Pause failed");
        tap("next");
        awaitNavigation(192, true);
        // Verify the notification uses the same navigation while paused.
        StatusBarNotification[] notifications = getTargetContext()
                .getSystemService(NotificationManager.class).getActiveNotifications();
        boolean sent = false;
        for (StatusBarNotification notification : notifications) {
            if (notification.getNotification().actions == null) continue;
            for (android.app.Notification.Action action : notification.getNotification().actions) {
                if ("Siguiente".contentEquals(action.title)) {
                    action.actionIntent.send();
                    sent = true;
                    break;
                }
            }
        }
        if (!sent) throw new AssertionError("Notification has no Next action");
        awaitNavigation(64, true);
        if ((boolean) control.invoke(null, 4)) {
            throw new AssertionError("Next was accepted at the end of the playlist");
        }
        tap("previous");
        awaitNavigation(192, true);
        tap("previous");
        awaitNavigation(128, true);
        if (!(boolean) control.invoke(null, 0)) throw new AssertionError("Resume failed");
        awaitNavigation(128, false);
        if (state().getJSONArray("calls").length() != 1) {
            throw new AssertionError("Navigation restarted extraction");
        }
        Log.i("RecoveryTest", "PASS playlist_navigation: UI previous/next, notification next, "
                + "boundaries, Pause/Resume, one extraction");
    }

    private void runProgressiveScenario(String scenario) throws Exception {
        boolean released = false;
        boolean background = scenario.startsWith("background");
        long deadline = SystemClock.elapsedRealtime() + 120000;
        while (SystemClock.elapsedRealtime() < deadline) {
            JSONObject state = state();
            if (!background || !released) tick.invoke(null);
            long milliseconds = (long) position.invoke(null);
            if (!released && milliseconds >= 1000 && state.getInt("served") >= 1) {
                if (state.getBoolean("done")) {
                    throw new AssertionError("Playback waited for the entire playlist");
                }
                if (scenario.endsWith("cancel")) {
                    control.invoke(null, 2);
                    SystemClock.sleep(3000);
                    state = state();
                    if (!state.optBoolean("cancelled") || state.getInt("served") != 1
                            || (long) tick.invoke(null) != 0 || !state.getBoolean("cookie_deleted")) {
                        throw new AssertionError("Stop did not cancel progressive resolution: " + state);
                    }
                    Log.i("RecoveryTest", "PASS " + scenario + ": early playback then Stop; " + state);
                    return;
                }
                if (background) {
                    shell("input keyevent KEYCODE_SLEEP");
                    SystemClock.sleep(1000);
                }
                probe.callAttr("release");
                released = true;
            }
            if (released && state.getBoolean("done") && state.getInt("served") >= 2
                    && milliseconds >= 1000) {
                String expected = scenario.equals("progressive_session") ? "[\"cookies\"]" : "[\"guest\"]";
                if (!state.getJSONArray("calls").toString().equals(expected)
                        || !state.getBoolean("cookie_deleted")) {
                    throw new AssertionError("Wrong progressive session/cleanup: " + state);
                }
                if (background && getTargetContext().getSystemService(PowerManager.class).isInteractive()) {
                    throw new AssertionError("Progressive background queue was not screen-off");
                }
                Log.i("RecoveryTest", "PASS " + scenario
                        + ": played before resolution finished; second item after EOF gap; " + state);
                shell("input keyevent KEYCODE_WAKEUP");
                return;
            }
            SystemClock.sleep(100);
        }
        throw new AssertionError(scenario + ": progressive timeout: " + state());
    }
}
