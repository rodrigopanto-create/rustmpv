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
    }

    static STATE: Mutex<State> = Mutex::new(State {
        context: 0,
        playback: None,
        video: false,
        foreground: true,
    });

    pub fn register(context: usize) -> Arc<Mutex<Playback>> {
        let playback = Arc::new(Mutex::new(Playback::default()));
        let mut state = STATE.lock().expect("playback lifecycle lock");
        state.context = context;
        state.playback = (context != 0).then(|| playback.clone());
        state.video = false;
        playback
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
        let state = STATE.lock().expect("playback lifecycle lock");
        let Some(playback) = &state.playback else {
            return 0;
        };
        let mut playback = playback.lock().expect("playback lock");
        playback.poll(state.context);
        // Keeps resume positions current while the UI is not drawing (screen locked).
        crate::history::observe(&playback);
        i64::from(playback.active)
            | (i64::from(playback.paused) << 1)
            | (i64::from(playback.loaded) << 2)
            | (i64::from(playback.buffering) << 3)
            | (i64::from(playback.error.is_some()) << 4)
    }

    #[unsafe(no_mangle)]
    pub extern "system" fn Java_com_visualrustmpv_app_PlaybackService_nativePosition(
        _: *mut std::ffi::c_void,
        _: *mut std::ffi::c_void,
    ) -> i64 {
        let state = STATE.lock().expect("playback lifecycle lock");
        state.playback.as_ref().map_or(0, |playback| {
            (playback
                .lock()
                .expect("playback lock")
                .position
                .unwrap_or(0.0)
                * 1000.0) as i64
        })
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
        let Some(playback) = &state.playback else {
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
                if action == 2 && playback.error.is_none() {
                    playback.stop();
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

#[cfg(target_os = "android")]
pub use native::{register, set_video};

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
