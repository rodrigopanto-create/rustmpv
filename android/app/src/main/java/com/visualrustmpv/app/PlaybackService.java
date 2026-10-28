package com.visualrustmpv.app;

import android.app.Notification;
import android.app.NotificationChannel;
import android.app.NotificationManager;
import android.app.PendingIntent;
import android.app.Service;
import android.content.BroadcastReceiver;
import android.content.Context;
import android.content.Intent;
import android.content.IntentFilter;
import android.content.pm.ServiceInfo;
import android.graphics.Bitmap;
import android.graphics.BitmapFactory;
import android.media.AudioAttributes;
import android.media.AudioFocusRequest;
import android.media.AudioManager;
import android.media.MediaMetadata;
import android.media.session.MediaSession;
import android.media.session.PlaybackState;
import android.os.Build;
import android.os.Handler;
import android.os.IBinder;
import android.os.Looper;
import android.os.PowerManager;
import android.util.Log;
import android.widget.Toast;

import java.io.ByteArrayOutputStream;
import java.io.IOException;
import java.io.InputStream;
import java.net.HttpURLConnection;
import java.net.URL;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;

/** Keeps the existing Rust/mpv player alive; does not create another player. */
public final class PlaybackService extends Service {
    private static final String CHANNEL = "playback";
    private static final int NOTIFICATION = 1;
    private static final String PLAY = "com.visualrustmpv.app.PLAY";
    private static final String PAUSE = "com.visualrustmpv.app.PAUSE";
    private static final String STOP = "com.visualrustmpv.app.STOP";
    static final String EXTRA_TITLE = "com.visualrustmpv.app.TITLE";
    static final String EXTRA_ARTIST = "com.visualrustmpv.app.ARTIST";
    static final String EXTRA_THUMBNAILS = "com.visualrustmpv.app.THUMBNAILS";
    private static final String DEFAULT_TITLE = "VisualRustMPV";
    private static final int ARTWORK_MAX_BYTES = 4 * 1024 * 1024;
    private static final int ARTWORK_MAX_SIDE = 512;
    private static final int ARTWORK_TIMEOUT_MS = 8000;
    private final Handler handler = new Handler(Looper.getMainLooper());
    private final ExecutorService artworkLoader = Executors.newSingleThreadExecutor();
    private String title = DEFAULT_TITLE;
    private String artist = "";
    private Bitmap artwork;
    private int artworkGeneration;
    private MediaSession session;
    private AudioManager audio;
    private AudioFocusRequest focusRequest;
    private PowerManager.WakeLock wakeLock;
    private boolean hasFocus;
    private boolean focusRequested;
    private boolean resumeOnFocus;
    private boolean foreground;
    private long previousFlags = -1;
    private long previousDuration = -1;

    static {
        System.loadLibrary("app_android");
    }

    private static native long nativeTick();
    private static native long nativePosition();
    private static native long nativeDuration();
    private static native boolean nativeControl(int action);

    private final AudioManager.OnAudioFocusChangeListener focusListener = change -> {
        if (change == AudioManager.AUDIOFOCUS_GAIN) {
            hasFocus = true;
            if (resumeOnFocus) {
                resumeOnFocus = false;
                control(0);
            }
        } else {
            hasFocus = false;
            long flags = nativeTick();
            resumeOnFocus = change != AudioManager.AUDIOFOCUS_LOSS
                    && (flags & 1) != 0 && (flags & 2) == 0;
            if (!nativeControl(1)) {
                reportError("No se pudo pausar al perder el foco de audio");
            }
            refresh();
        }
    };

    private final BroadcastReceiver noisy = new BroadcastReceiver() {
        @Override
        public void onReceive(Context context, Intent intent) {
            if (AudioManager.ACTION_AUDIO_BECOMING_NOISY.equals(intent.getAction())) {
                control(1);
            }
        }
    };

    private final Runnable ticker = new Runnable() {
        @Override
        public void run() {
            refresh();
            handler.postDelayed(this, 250);
        }
    };

    @Override
    public void onCreate() {
        super.onCreate();
        audio = (AudioManager) getSystemService(AUDIO_SERVICE);
        PowerManager power = (PowerManager) getSystemService(POWER_SERVICE);
        wakeLock = power.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "VisualRustMPV:playback");
        session = new MediaSession(this, "VisualRustMPV");
        session.setFlags(MediaSession.FLAG_HANDLES_MEDIA_BUTTONS
                | MediaSession.FLAG_HANDLES_TRANSPORT_CONTROLS);
        session.setSessionActivity(openPlayer());
        session.setCallback(new MediaSession.Callback() {
            @Override
            public void onPlay() { control(0); }
            @Override
            public void onPause() { control(1); }
            @Override
            public void onStop() { control(2); }
        }, handler);
        session.setActive(true);
        if (Build.VERSION.SDK_INT >= 26) {
            NotificationChannel channel = new NotificationChannel(
                    CHANNEL, "Reproduccion multimedia", NotificationManager.IMPORTANCE_LOW);
            channel.setSound(null, null);
            getSystemService(NotificationManager.class).createNotificationChannel(channel);
            AudioAttributes attributes = new AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_MEDIA)
                    .setContentType(AudioAttributes.CONTENT_TYPE_MOVIE).build();
            focusRequest = new AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
                    .setAudioAttributes(attributes)
                    .setOnAudioFocusChangeListener(focusListener, handler).build();
        }
        IntentFilter filter = new IntentFilter(AudioManager.ACTION_AUDIO_BECOMING_NOISY);
        if (Build.VERSION.SDK_INT >= 33) {
            registerReceiver(noisy, filter, Context.RECEIVER_NOT_EXPORTED);
        } else {
            registerReceiver(noisy, filter);
        }
    }

    @Override
    public int onStartCommand(Intent intent, int flags, int startId) {
        if (intent != null && intent.hasExtra(EXTRA_TITLE)) {
            applyMedia(intent);
        }
        if (!foreground) {
            Notification notification = notification(nativeTick());
            if (Build.VERSION.SDK_INT >= 29) {
                startForeground(NOTIFICATION, notification,
                        ServiceInfo.FOREGROUND_SERVICE_TYPE_MEDIA_PLAYBACK);
            } else {
                startForeground(NOTIFICATION, notification);
            }
            foreground = true;
        }
        String action = intent == null ? null : intent.getAction();
        if (STOP.equals(action)) {
            control(2);
        } else if (PAUSE.equals(action)) {
            control(1);
        } else if (PLAY.equals(action)) {
            control(0);
        }
        handler.removeCallbacks(ticker);
        handler.post(ticker);
        return START_NOT_STICKY;
    }

    private void applyMedia(Intent intent) {
        String newTitle = intent.getStringExtra(EXTRA_TITLE);
        title = newTitle == null || newTitle.isEmpty() ? DEFAULT_TITLE : newTitle;
        String newArtist = intent.getStringExtra(EXTRA_ARTIST);
        artist = newArtist == null ? "" : newArtist;
        artwork = null;
        int generation = ++artworkGeneration;
        // Force the next refresh() to republish metadata and the notification.
        previousDuration = -1;
        previousFlags = -1;
        String thumbnails = intent.getStringExtra(EXTRA_THUMBNAILS);
        if (thumbnails == null || thumbnails.isEmpty()) return;
        String[] candidates = thumbnails.split("\n");
        artworkLoader.execute(() -> {
            Bitmap loaded = null;
            for (String candidate : candidates) {
                loaded = downloadArtwork(candidate);
                if (loaded != null) break;
            }
            Bitmap result = loaded;
            handler.post(() -> {
                if (generation != artworkGeneration) return;
                if (result == null) {
                    Log.w("VisualRustMPV", "No se pudo cargar ninguna miniatura");
                    return;
                }
                artwork = result;
                Log.i("VisualRustMPV", "Miniatura cargada " + result.getWidth() + "x" + result.getHeight());
                publishMetadata(nativeDuration());
                getSystemService(NotificationManager.class).notify(NOTIFICATION, notification(nativeTick()));
            });
        });
    }

    private static Bitmap downloadArtwork(String address) {
        HttpURLConnection connection = null;
        try {
            URL url = new URL(address);
            if (!"https".equals(url.getProtocol())) return null;
            connection = (HttpURLConnection) url.openConnection();
            connection.setConnectTimeout(ARTWORK_TIMEOUT_MS);
            connection.setReadTimeout(ARTWORK_TIMEOUT_MS);
            if (connection.getResponseCode() != HttpURLConnection.HTTP_OK
                    || !"https".equals(connection.getURL().getProtocol())
                    || connection.getContentLengthLong() > ARTWORK_MAX_BYTES) {
                return null;
            }
            ByteArrayOutputStream bytes = new ByteArrayOutputStream();
            try (InputStream input = connection.getInputStream()) {
                byte[] buffer = new byte[16 * 1024];
                int read;
                while ((read = input.read(buffer)) != -1) {
                    if (bytes.size() + read > ARTWORK_MAX_BYTES) return null;
                    bytes.write(buffer, 0, read);
                }
            }
            byte[] data = bytes.toByteArray();
            BitmapFactory.Options options = new BitmapFactory.Options();
            options.inJustDecodeBounds = true;
            BitmapFactory.decodeByteArray(data, 0, data.length, options);
            if (options.outWidth <= 0 || options.outHeight <= 0) return null;
            int sample = 1;
            while (Math.max(options.outWidth, options.outHeight) / (sample * 2) >= ARTWORK_MAX_SIDE) {
                sample *= 2;
            }
            options = new BitmapFactory.Options();
            options.inSampleSize = sample;
            return BitmapFactory.decodeByteArray(data, 0, data.length, options);
        } catch (IOException | RuntimeException error) {
            Log.w("VisualRustMPV", "Miniatura no disponible: " + error);
            return null;
        } finally {
            if (connection != null) connection.disconnect();
        }
    }

    private void publishMetadata(long duration) {
        MediaMetadata.Builder metadata = new MediaMetadata.Builder()
                .putString(MediaMetadata.METADATA_KEY_TITLE, title)
                .putString(MediaMetadata.METADATA_KEY_DISPLAY_TITLE, title)
                .putString(MediaMetadata.METADATA_KEY_ARTIST, artist)
                .putString(MediaMetadata.METADATA_KEY_DISPLAY_SUBTITLE, artist)
                .putLong(MediaMetadata.METADATA_KEY_DURATION, duration);
        if (artwork != null) {
            metadata.putBitmap(MediaMetadata.METADATA_KEY_ART, artwork)
                    .putBitmap(MediaMetadata.METADATA_KEY_ALBUM_ART, artwork)
                    .putBitmap(MediaMetadata.METADATA_KEY_DISPLAY_ICON, artwork);
        }
        session.setMetadata(metadata.build());
    }

    private void control(int action) {
        resumeOnFocus = false;
        if (action == 0 && !acquireFocus()) {
            reportError("Otra aplicacion no permite obtener el foco de audio");
            return;
        }
        if (!nativeControl(action)) {
            reportError("No se pudo enviar el control al reproductor");
        }
        refresh();
    }

    @SuppressWarnings("deprecation")
    private boolean acquireFocus() {
        if (hasFocus) return true;
        int result = Build.VERSION.SDK_INT >= 26
                ? audio.requestAudioFocus(focusRequest)
                : audio.requestAudioFocus(focusListener, AudioManager.STREAM_MUSIC,
                        AudioManager.AUDIOFOCUS_GAIN);
        hasFocus = result == AudioManager.AUDIOFOCUS_REQUEST_GRANTED;
        focusRequested = hasFocus;
        return hasFocus;
    }

    @SuppressWarnings("deprecation")
    private void releaseFocus() {
        if (focusRequested) {
            if (Build.VERSION.SDK_INT >= 26) {
                audio.abandonAudioFocusRequest(focusRequest);
            } else {
                audio.abandonAudioFocus(focusListener);
            }
            hasFocus = false;
            focusRequested = false;
        }
    }

    private void refresh() {
        long flags = nativeTick();
        boolean active = (flags & 1) != 0;
        boolean paused = (flags & 2) != 0;
        if (active && !paused && !hasFocus && !resumeOnFocus && !acquireFocus()) {
            nativeControl(1);
            reportError("No se obtuvo el foco de audio; reproduccion pausada");
            return;
        }
        if (active && !paused) {
            if (!wakeLock.isHeld()) wakeLock.acquire();
        } else {
            if (wakeLock.isHeld()) wakeLock.release();
            if (!resumeOnFocus) releaseFocus();
        }
        int state = !active ? PlaybackState.STATE_STOPPED
                : paused ? PlaybackState.STATE_PAUSED
                : (flags & 4) == 0 || (flags & 8) != 0 ? PlaybackState.STATE_BUFFERING
                : PlaybackState.STATE_PLAYING;
        session.setPlaybackState(new PlaybackState.Builder()
                .setActions(PlaybackState.ACTION_PLAY | PlaybackState.ACTION_PAUSE
                        | PlaybackState.ACTION_PLAY_PAUSE | PlaybackState.ACTION_STOP)
                .setState(state, nativePosition(), state == PlaybackState.STATE_PLAYING ? 1f : 0f)
                .build());
        long duration = nativeDuration();
        if (duration != previousDuration) {
            publishMetadata(duration);
            previousDuration = duration;
        }
        if (flags != previousFlags) {
            getSystemService(NotificationManager.class).notify(NOTIFICATION, notification(flags));
            Log.i("VisualRustMPV", "Servicio multimedia flags=" + flags
                    + " posicion=" + nativePosition() + "ms");
            previousFlags = flags;
        }
        if (!active) {
            if ((flags & 16) != 0) reportError("Error de reproduccion; revisa el estado en la app");
            stopSelf();
        }
    }

    private PendingIntent openPlayer() {
        Intent intent = new Intent(this, MainActivity.class)
                .addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP | Intent.FLAG_ACTIVITY_CLEAR_TOP);
        return PendingIntent.getActivity(this, 0, intent,
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    private PendingIntent action(String action, int code) {
        return PendingIntent.getService(this, code,
                new Intent(this, PlaybackService.class).setAction(action),
                PendingIntent.FLAG_UPDATE_CURRENT | PendingIntent.FLAG_IMMUTABLE);
    }

    @SuppressWarnings("deprecation")
    private Notification notification(long flags) {
        boolean paused = (flags & 2) != 0;
        Notification.Builder builder = Build.VERSION.SDK_INT >= 26
                ? new Notification.Builder(this, CHANNEL) : new Notification.Builder(this);
        String state = paused ? "En pausa" : "Reproduciendo";
        return builder.setContentTitle(title)
                .setContentText(artist.isEmpty() ? state : artist)
                .setSubText(state)
                .setLargeIcon(artwork)
                .setSmallIcon(android.R.drawable.ic_media_play)
                .setContentIntent(openPlayer())
                .setVisibility(Notification.VISIBILITY_PUBLIC)
                .setOnlyAlertOnce(true)
                .setOngoing(!paused)
                .setDeleteIntent(action(STOP, 3))
                .addAction(paused ? android.R.drawable.ic_media_play : android.R.drawable.ic_media_pause,
                        paused ? "Reanudar" : "Pausar", action(paused ? PLAY : PAUSE, 1))
                .addAction(android.R.drawable.ic_menu_close_clear_cancel, "Detener", action(STOP, 2))
                .setStyle(new Notification.MediaStyle().setMediaSession(session.getSessionToken())
                        .setShowActionsInCompactView(0, 1))
                .build();
    }

    private void reportError(String message) {
        Log.e("VisualRustMPV", message);
        Toast.makeText(this, message, Toast.LENGTH_LONG).show();
    }

    @Override
    public void onTaskRemoved(Intent rootIntent) {
        nativeControl(2);
        stopSelf();
    }

    @Override
    public void onDestroy() {
        handler.removeCallbacks(ticker);
        artworkGeneration++;
        artworkLoader.shutdownNow();
        if (nativeTick() != 0) nativeControl(2);
        if (wakeLock.isHeld()) wakeLock.release();
        resumeOnFocus = false;
        releaseFocus();
        unregisterReceiver(noisy);
        session.setActive(false);
        session.release();
        stopForeground(true);
        super.onDestroy();
    }

    @Override
    public IBinder onBind(Intent intent) {
        return null;
    }
}
