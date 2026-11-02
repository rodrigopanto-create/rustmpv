fn video_track(foreground: bool, video: bool, active: bool) -> Option<&'static str> {
    (video && active).then_some(if foreground { "auto" } else { "no" })
}

#[cfg(target_os = "android")]
mod native {
    use super::video_track;
    use crate::playback::Playback;
    use std::sync::{Arc, Mutex};

    struct State {
        context: usize,
        playback: Option<Arc<Mutex<Playback>>>,
        video: bool,
        foreground: bool,
        video_load: u64,
        #[cfg(feature = "chaquopy")]
        recovery: Option<crate::recovery::Session>,
    }

    static STATE: Mutex<State> = Mutex::new(State {
        context: 0,
        playback: None,
        video: false,
        foreground: true,
        video_load: 0,
        #[cfg(feature = "chaquopy")]
        recovery: None,
    });

    pub fn register(context: usize) -> Arc<Mutex<Playback>> {
        let playback = Arc::new(Mutex::new(Playback::default()));
        let mut state = STATE.lock().expect("playback lifecycle lock");
        state.context = context;
        state.playback = (context != 0).then(|| playback.clone());
        state.video = false;
        state.video_load = 0;
        #[cfg(feature = "chaquopy")]
        {
            state.recovery = None;
        }
        playback
    }

    #[cfg(feature = "chaquopy")]
    pub fn begin_media(
        input: &str,
        mode: core_logic::PlaybackMode,
        media: crate::stream_url::MediaInfo,
    ) {
        let mut state = STATE.lock().expect("playback lifecycle lock");
        state.recovery = crate::needs_resolver(input)
            .then(|| crate::recovery::Session::new(input.to_owned(), mode, media));
    }

    #[cfg(feature = "chaquopy")]
    pub fn invalidate_recovery() {
        let mut state = STATE.lock().expect("playback lifecycle lock");
        if state
            .recovery
            .as_ref()
            .is_some_and(|session| session.queue_pending())
        {
            crate::chaquopy::cancel_pending();
        }
        state.recovery = None;
        if let Some(playback) = &state.playback {
            playback.lock().expect("playback lock").recovering = false;
        }
    }

    #[cfg(feature = "chaquopy")]
    pub fn continue_queue(
        receiver: std::sync::mpsc::Receiver<Result<crate::stream_url::MediaInfo, String>>,
    ) {
        let mut state = STATE.lock().expect("playback lifecycle lock");
        if let Some(session) = &mut state.recovery {
            session.continue_queue(receiver);
        } else {
            log::error!("No se puede continuar la cola sin una sesion de reproduccion");
            crate::chaquopy::cancel_pending();
        }
    }

    #[cfg(feature = "chaquopy")]
    pub fn queue_pending() -> bool {
        STATE
            .lock()
            .expect("playback lifecycle lock")
            .recovery
            .as_ref()
            .is_some_and(|session| session.queue_pending())
    }

    #[cfg(feature = "chaquopy")]
    pub fn update_input(input: &str) {
        let mut state = STATE.lock().expect("playback lifecycle lock");
        if state
            .recovery
            .as_ref()
            .is_some_and(|session| session.input != input)
        {
            if state
                .recovery
                .as_ref()
                .is_some_and(|session| session.queue_pending())
            {
                crate::chaquopy::cancel_pending();
            }
            state.recovery = None;
            if let Some(playback) = &state.playback {
                playback.lock().expect("playback lock").recovering = false;
            }
        }
    }

    #[cfg(feature = "chaquopy")]
    pub fn take_recovery_updates() -> Option<(
        String,
        core_logic::PlaybackMode,
        Vec<crate::recovery::Update>,
    )> {
        let mut state = STATE.lock().expect("playback lifecycle lock");
        let session = state.recovery.as_mut()?;
        Some((session.input.clone(), session.mode, session.take_updates()))
    }

    pub fn poll() {
        let mut state = STATE.lock().expect("playback lifecycle lock");
        let Some(playback) = state.playback.clone() else {
            return;
        };
        let mut playback = playback.lock().expect("playback lock");
        poll_state(&mut state, &mut playback);
    }

    fn poll_state(state: &mut State, playback: &mut Playback) {
        #[cfg(feature = "chaquopy")]
        if let Some(error) = state
            .recovery
            .as_ref()
            .and_then(|session| session.terminal_error())
        {
            if !playback.stopped {
                playback.fail(error.to_owned());
            }
            return;
        }
        playback.poll(state.context);
        crate::history::observe(playback);
        #[cfg(feature = "chaquopy")]
        if let Some(session) = &mut state.recovery {
            let context = state.context;
            let foreground = state.foreground;
            if playback.stream_failure && session.queue_pending() {
                crate::chaquopy::cancel_pending();
                session.discard_queue();
            }
            let mode = if foreground {
                session.mode
            } else {
                core_logic::PlaybackMode::Audio
            };
            let appended =
                session.poll_queue(playback, |url| crate::append_stream(context, url, mode));
            if appended && !playback.active && !playback.stopped && playback.error.is_none() {
                // Keep the service alive until mpv reports the appended START_FILE.
                playback.active = true;
                playback.ended = false;
                playback.loaded = false;
            }
            session.tick(
                playback,
                |input, mode, cookies_only| {
                    crate::chaquopy::retry_playback(
                        input,
                        mode == core_logic::PlaybackMode::Audio,
                        cookies_only,
                    )
                },
                || crate::send_mpv_command(context, &["stop"]),
                |media, mode, position, paused| {
                    crate::load_media(
                        context,
                        media,
                        if foreground {
                            mode
                        } else {
                            core_logic::PlaybackMode::Audio
                        },
                        position,
                        paused,
                    )
                },
            );
            playback.recovering = session
                .pending()
                .is_some_and(|request| !request.cancelled(playback));
            if playback.recovering {
                playback.paused = session.pending().expect("pending recovery").paused;
            }
            if session.queue_pending()
                && !playback.active
                && playback.error.is_none()
                && !playback.stopped
            {
                playback.recovering = true;
            }
        }
        if state.video && playback.active && state.video_load != playback.load_generation {
            if let Some(value) = video_track(state.foreground, true, true) {
                match crate::send_mpv_command(state.context, &["set", "vid", value]) {
                    Ok(()) => state.video_load = playback.load_generation,
                    Err(error) => {
                        log::error!("Video tras cambiar stream: {error}");
                        playback.fail(error);
                    }
                }
            }
        }
    }

    pub fn set_paused(paused: bool) {
        let state = STATE.lock().expect("playback lifecycle lock");
        #[cfg(feature = "chaquopy")]
        let mut state = state;
        if let Some(playback) = &state.playback {
            playback.lock().expect("playback lock").paused = paused;
        }
        #[cfg(feature = "chaquopy")]
        if let Some(session) = &mut state.recovery {
            session.set_paused(paused);
        }
    }

    pub fn cancel() {
        #[cfg(feature = "chaquopy")]
        crate::chaquopy::cancel_pending();
        let state = STATE.lock().expect("playback lifecycle lock");
        #[cfg(feature = "chaquopy")]
        let mut state = state;
        if let Some(playback) = &state.playback {
            playback.lock().expect("playback lock").cancel();
        }
        #[cfg(feature = "chaquopy")]
        if let Some(session) = &mut state.recovery {
            session.cancel();
        }
    }

    pub fn set_video(video: bool) {
        STATE.lock().expect("playback lifecycle lock").video = video;
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_visualrustmpv_app_MainActivity_setPlaybackForeground(
        _: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
        foreground: u8,
    ) {
        let mut state = STATE.lock().expect("playback lifecycle lock");
        state.foreground = foreground != 0;
        if state.context == 0 || !state.video {
            return;
        }
        let Some(playback) = &state.playback else {
            return;
        };
        let mut playback = playback.lock().expect("playback lock");
        if let Some(value) = video_track(state.foreground, state.video, playback.active) {
            match crate::send_mpv_command(state.context, &["set", "vid", value]) {
                Ok(()) => log::info!("Segundo plano: vid={value}; audio y pausa conservados"),
                Err(error) => {
                    log::error!("Cambiar video por ciclo de vida: {error}");
                    playback.fail(error);
                }
            }
        }
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_visualrustmpv_app_PlaybackService_nativeTick(
        _: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i64 {
        let mut state = STATE.lock().expect("playback lifecycle lock");
        let Some(playback) = state.playback.clone() else {
            return 0;
        };
        let mut playback = playback.lock().expect("playback lock");
        poll_state(&mut state, &mut playback);
        playback.service_flags()
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_visualrustmpv_app_PlaybackService_nativePosition(
        _: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i64 {
        let state = STATE.lock().expect("playback lifecycle lock");
        let Some(playback) = &state.playback else {
            return 0;
        };
        let playback = playback.lock().expect("playback lock");
        let position = playback.position;
        #[cfg(feature = "chaquopy")]
        let position = state
            .recovery
            .as_ref()
            .and_then(|session| session.pending())
            .filter(|request| !request.cancelled(&playback))
            .map_or(position, |request| request.position);
        (position.unwrap_or(0.0) * 1000.0) as i64
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_visualrustmpv_app_PlaybackService_nativeDuration(
        _: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i64 {
        let state = STATE.lock().expect("playback lifecycle lock");
        state.playback.as_ref().map_or(0, |playback| {
            (playback
                .lock()
                .expect("playback lock")
                .duration
                .unwrap_or(0.0)
                * 1000.0) as i64
        })
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_visualrustmpv_app_PlaybackService_nativeControl(
        _: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
        action: i32,
    ) -> u8 {
        let state = STATE.lock().expect("playback lifecycle lock");
        #[cfg(feature = "chaquopy")]
        let mut state = state;
        let Some(playback) = state.playback.clone() else {
            log::error!("Control multimedia sin reproductor");
            return 0;
        };
        let mut playback = playback.lock().expect("playback lock");
        let command: &[&str] = match action {
            0 => &["set", "pause", "no"],
            1 => &["set", "pause", "yes"],
            2 => &["stop"],
            _ => {
                log::error!("Control multimedia desconocido: {action}");
                return 0;
            }
        };
        match crate::send_mpv_command(state.context, command) {
            Ok(()) => {
                if action == 2 {
                    #[cfg(feature = "chaquopy")]
                    crate::chaquopy::cancel_pending();
                    playback.cancel();
                    #[cfg(feature = "chaquopy")]
                    if let Some(session) = &mut state.recovery {
                        session.cancel();
                    }
                } else {
                    playback.paused = action == 1;
                    #[cfg(feature = "chaquopy")]
                    if let Some(session) = &mut state.recovery {
                        session.set_paused(action == 1);
                    }
                }
                1
            }
            Err(error) => {
                log::error!("Control multimedia del servicio: {error}");
                playback.fail(error);
                0
            }
        }
    }
}

#[cfg(all(target_os = "android", feature = "chaquopy"))]
pub use native::{
    begin_media, continue_queue, invalidate_recovery, queue_pending, take_recovery_updates,
    update_input,
};
#[cfg(target_os = "android")]
pub use native::{cancel, poll, register, set_paused, set_video};

#[cfg(test)]
mod tests {
    use super::video_track;
    use crate::playback::Playback;

    #[test]
    fn background_disables_only_video_track() {
        assert_eq!(video_track(false, true, true), Some("no"));
        assert_eq!(video_track(true, true, true), Some("auto"));
        assert_eq!(video_track(false, false, true), None);
        assert_eq!(video_track(true, false, true), None);
        assert_eq!(video_track(false, true, false), None);
    }

    #[test]
    fn background_track_change_preserves_audio_pause_and_position() {
        for paused in [false, true] {
            let playback = Playback {
                active: true,
                loaded: true,
                started: true,
                paused,
                position: Some(42.0),
                ..Playback::default()
            };
            assert_eq!(video_track(false, true, playback.active), Some("no"));
            assert_eq!(video_track(true, true, playback.active), Some("auto"));
            assert_eq!(playback.paused, paused);
            assert_eq!(playback.position, Some(42.0));
            assert!(playback.active);
        }
    }
}
