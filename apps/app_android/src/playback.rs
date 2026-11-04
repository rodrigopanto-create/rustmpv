#[derive(Debug, Default, Clone)]
pub struct Playback {
    pub active: bool,
    pub loaded: bool,
    pub started: bool,
    pub paused: bool,
    pub buffering: bool,
    pub position: Option<f64>,
    pub duration: Option<f64>,
    pub audio_output: Option<String>,
    pub error: Option<String>,
    pub ended: bool,
    pub stopped: bool,
    pub diagnostic: Option<String>,
    pub stream_failure: bool,
    pub playlist_position: usize,
    pub playlist_count: usize,
    pub last_position: Option<f64>,
    pub cancel_generation: u64,
    pub recovering: bool,
    pub load_generation: u64,
    #[cfg(target_os = "android")]
    last_logged_position: Option<f64>,
}

impl Playback {
    pub fn service_flags(&self) -> i64 {
        if self.recovering {
            // Renewal is buffering, not a terminal error or a stopped service.
            1 | (i64::from(self.paused) << 1) | 8 | 32
        } else {
            i64::from(self.active)
                | (i64::from(self.paused) << 1)
                | (i64::from(self.loaded) << 2)
                | (i64::from(self.buffering) << 3)
                | (i64::from(self.error.is_some()) << 4)
                | (i64::from(self.can_navigate(false)) << 6)
                | (i64::from(self.can_navigate(true)) << 7)
        }
    }

    pub fn can_navigate(&self, next: bool) -> bool {
        self.active
            && self.loaded
            && !self.stopped
            && !self.ended
            && !self.recovering
            && self.error.is_none()
            && self.playlist_position < self.playlist_count
            && if next {
                self.playlist_position + 1 < self.playlist_count
            } else {
                self.playlist_position > 0
            }
    }

    pub fn is_playing(&self) -> bool {
        self.active
            && self.loaded
            && self.started
            && !self.paused
            && !self.stopped
            && !self.ended
            && self.error.is_none()
    }

    pub fn loading_state(&self) -> bool {
        self.active && self.error.is_none() && (!self.loaded || !self.started || self.buffering)
    }

    pub fn loading(&mut self) {
        let playlist_position = self.playlist_position;
        *self = Self {
            active: true,
            // mpv only re-emits observed properties when their values change.
            paused: self.paused,
            audio_output: self.audio_output.take(),
            playlist_position,
            playlist_count: self.playlist_count,
            cancel_generation: self.cancel_generation,
            load_generation: self.load_generation.wrapping_add(1),
            ..Self::default()
        };
    }

    pub fn status(&self) -> String {
        if let Some(error) = &self.error {
            match &self.diagnostic {
                Some(detail) => format!("Error de reproduccion: {error}. {detail}"),
                None => format!("Error de reproduccion: {error}"),
            }
        } else if self.stopped {
            "Reproduccion detenida".to_owned()
        } else if self.ended {
            "Reproduccion finalizada".to_owned()
        } else if !self.active {
            "Android listo".to_owned()
        } else if !self.loaded {
            "Cargando stream...".to_owned()
        } else if self.buffering {
            "Almacenando en buffer...".to_owned()
        } else if self.paused {
            "En pausa".to_owned()
        } else if self.started {
            "Reproduciendo".to_owned()
        } else {
            "Preparando reproduccion...".to_owned()
        }
    }

    pub fn fail(&mut self, message: String) {
        self.active = false;
        self.stream_failure = false;
        self.error = Some(message);
    }

    pub fn stop(&mut self) {
        self.active = false;
        self.loaded = false;
        self.started = false;
        self.stopped = true;
        self.ended = false;
        self.error = None;
        self.position = None;
        self.duration = None;
        self.stream_failure = false;
        self.recovering = false;
    }

    pub fn cancel(&mut self) {
        self.cancel_generation = self.cancel_generation.wrapping_add(1);
        self.stop();
    }

    fn record_error(&mut self, message: &str) {
        let lower = message.to_ascii_lowercase();
        let detail =
            if lower.contains("certificate") || lower.contains("tls") || lower.contains("ssl") {
                Some("Fallo al verificar el certificado TLS")
            } else if lower.contains("401") || lower.contains("403") {
                Some("El servidor rechazo el stream (HTTP 403)")
            } else if lower.contains("404") || lower.contains("410") {
                Some("El stream no existe (HTTP 404)")
            } else if lower.contains("timed out") || lower.contains("timeout") {
                Some("Se agoto el tiempo de conexion")
            } else if lower.contains("connection refused")
                || lower.contains("couldn't connect")
                || lower.contains("could not connect")
            {
                Some("No se pudo conectar al servidor")
            } else if lower.contains("resolve host") || lower.contains("name resolution") {
                Some("No se pudo resolver el nombre del servidor")
            } else if lower.contains("audio output") && lower.contains("failed") {
                Some("No se pudo iniciar la salida de audio")
            } else {
                None
            };
        if let Some(detail) = detail {
            self.diagnostic = Some(detail.to_owned());
        }
    }

    pub fn fail_stream(&mut self, message: String) {
        self.fail(message);
        self.stream_failure = !self
            .diagnostic
            .as_deref()
            .is_some_and(|detail| detail.contains("TLS") || detail.contains("audio"));
    }
}

#[cfg(target_os = "android")]
mod native {
    use super::Playback;
    use std::ffi::{CStr, CString, c_char, c_int, c_void};

    #[repr(C)]
    struct Event {
        id: c_int,
        error: c_int,
        userdata: u64,
        data: *mut c_void,
    }
    #[repr(C)]
    struct Property {
        name: *const c_char,
        format: c_int,
        data: *mut c_void,
    }
    #[repr(C)]
    struct EndFile {
        reason: c_int,
        error: c_int,
        entry: i64,
        insert: i64,
        count: c_int,
    }
    #[repr(C)]
    struct LogMessage {
        prefix: *const c_char,
        level: *const c_char,
        text: *const c_char,
        log_level: c_int,
    }

    unsafe extern "C" {
        fn mpv_wait_event(context: *mut c_void, timeout: f64) -> *const Event;
        fn mpv_observe_property(
            context: *mut c_void,
            userdata: u64,
            name: *const c_char,
            format: c_int,
        ) -> c_int;
        fn mpv_request_log_messages(context: *mut c_void, level: *const c_char) -> c_int;
        fn mpv_error_string(error: c_int) -> *const c_char;
        fn mpv_get_property(
            context: *mut c_void,
            name: *const c_char,
            format: c_int,
            data: *mut c_void,
        ) -> c_int;
    }

    pub fn error_text(code: c_int) -> String {
        unsafe {
            CStr::from_ptr(mpv_error_string(code))
                .to_string_lossy()
                .into_owned()
        }
    }

    pub fn subscribe(context: usize) -> Result<(), String> {
        let context = context as *mut c_void;
        for (index, (name, format)) in [
            (c"time-pos", 5),
            (c"duration", 5),
            (c"pause", 3),
            (c"paused-for-cache", 3),
            (c"current-ao", 1),
            (c"playlist-pos", 4),
            (c"playlist-count", 4),
        ]
        .into_iter()
        .enumerate()
        {
            let result =
                unsafe { mpv_observe_property(context, index as u64 + 1, name.as_ptr(), format) };
            if result < 0 {
                return Err(format!(
                    "Observar {}: {}",
                    name.to_string_lossy(),
                    error_text(result)
                ));
            }
        }
        let result = unsafe { mpv_request_log_messages(context, c"warn".as_ptr()) };
        if result < 0 {
            return Err(format!("Activar logs mpv: {}", error_text(result)));
        }
        Ok(())
    }

    fn failed_playlist_index(context: usize, entry: i64) -> Result<usize, String> {
        let context = context as *mut c_void;
        let mut count: i64 = 0;
        let result = unsafe {
            mpv_get_property(
                context,
                c"playlist/count".as_ptr(),
                4,
                (&mut count as *mut i64).cast(),
            )
        };
        if result < 0 {
            return Err(format!("Leer lista de libmpv: {}", error_text(result)));
        }
        for index in 0..count.min(crate::stream_url::PLAYLIST_LIMIT as i64) {
            let name = CString::new(format!("playlist/{index}/id")).expect("numeric playlist path");
            let mut id: i64 = 0;
            let result = unsafe {
                mpv_get_property(context, name.as_ptr(), 4, (&mut id as *mut i64).cast())
            };
            if result < 0 {
                return Err(format!("Leer elemento de libmpv: {}", error_text(result)));
            }
            if id == entry {
                return Ok(index as usize);
            }
        }
        Err("El elemento que fallo ya no figura en la lista de libmpv".to_owned())
    }

    impl Playback {
        pub fn poll(&mut self, context: usize) {
            if context == 0 || (self.stream_failure && self.error.is_some()) {
                return;
            }
            // Event payloads belong to mpv and must be consumed before the next wait.
            for _ in 0..128 {
                let event = unsafe { &*mpv_wait_event(context as *mut c_void, 0.0) };
                match event.id {
                    0 => break,
                    1 => self.fail("libmpv se cerro".to_owned()),
                    2 if !event.data.is_null() => {
                        let message = unsafe { &*event.data.cast::<LogMessage>() };
                        let text = unsafe { CStr::from_ptr(message.text) }.to_string_lossy();
                        // Do not log signed media URLs or authentication query parameters.
                        let text = text
                            .split_whitespace()
                            .map(|word| if word.contains("://") { "[URL]" } else { word })
                            .collect::<Vec<_>>()
                            .join(" ");
                        if message.log_level <= 20 {
                            log::error!("libmpv: {text}");
                            self.record_error(&text);
                        } else {
                            log::warn!("libmpv: {text}");
                        }
                    }
                    5 if event.error < 0 => {
                        let message = error_text(event.error);
                        log::error!("libmpv comando asincrono: {message}");
                        self.fail(format!("Comando libmpv: {message}"));
                    }
                    6 => {
                        self.loading();
                        log::info!("libmpv START_FILE: cargando");
                    }
                    7 if !event.data.is_null() => {
                        let end = unsafe { &*event.data.cast::<EndFile>() };
                        if end.reason == 4 || end.error < 0 {
                            let message = error_text(end.error);
                            log::error!("libmpv END_FILE: {message} ({})", end.error);
                            if end.error == -13 {
                                // playlist-pos may already point to the next item when events are polled.
                                self.playlist_position =
                                    match failed_playlist_index(context, end.entry) {
                                        Ok(index) => index,
                                        Err(error) => {
                                            log::error!("{error}");
                                            usize::MAX
                                        }
                                    };
                                self.fail_stream(message);
                            } else {
                                self.fail(message);
                            }
                            // Let the UI recover before mpv skips to the next queued item.
                            break;
                        } else if end.reason == 2 {
                            self.stop();
                            log::info!("libmpv END_FILE: detenido");
                        } else if end.reason != 5 {
                            self.active = false;
                            self.ended = true;
                            log::info!("libmpv END_FILE: motivo {}", end.reason);
                        }
                    }
                    8 => {
                        self.loaded = true;
                        log::info!("libmpv FILE_LOADED");
                    }
                    21 if self.active => {
                        self.started = true;
                        log::info!("libmpv PLAYBACK_RESTART");
                    }
                    22 if !event.data.is_null() => {
                        let property = unsafe { &*event.data.cast::<Property>() };
                        let available = !property.data.is_null();
                        match (event.userdata, property.format, available) {
                            (1, 5, true) if self.active => {
                                let value = unsafe { *property.data.cast::<f64>() };
                                if value.is_finite() {
                                    if self
                                        .last_logged_position
                                        .is_none_or(|old| value >= old + 5.0)
                                    {
                                        log::info!("libmpv time-pos={value:.2}s");
                                        self.last_logged_position = Some(value);
                                    }
                                    self.position = Some(value);
                                    self.last_position = Some(value);
                                }
                            }
                            (2, 5, true) => {
                                self.duration = Some(unsafe { *property.data.cast::<f64>() })
                            }
                            (3, 3, true) => {
                                self.paused = unsafe { *property.data.cast::<c_int>() } != 0
                            }
                            (4, 3, true) => {
                                self.buffering = unsafe { *property.data.cast::<c_int>() } != 0
                            }
                            (5, 1, true) => {
                                let value = unsafe { *property.data.cast::<*const c_char>() };
                                if !value.is_null() {
                                    let name = unsafe { CStr::from_ptr(value) }
                                        .to_string_lossy()
                                        .into_owned();
                                    log::info!("libmpv salida de audio: {name}");
                                    self.audio_output = Some(name);
                                }
                            }
                            (6, 4, true) => {
                                let value = unsafe { *property.data.cast::<i64>() };
                                if value >= 0 {
                                    self.playlist_position = value as usize;
                                }
                            }
                            (7, 4, true) => {
                                self.playlist_count =
                                    unsafe { *property.data.cast::<i64>() }.max(0) as usize;
                            }
                            (1, 0, _) => self.position = None,
                            (2, 0, _) => self.duration = None,
                            (5, 0, _) => self.audio_output = None,
                            (6, 0, _) => self.playlist_position = 0,
                            (7, 0, _) => self.playlist_count = 0,
                            _ => {}
                        }
                    }
                    24 => {
                        log::error!("libmpv: desbordamiento de la cola de eventos");
                        self.fail("Se perdieron eventos de libmpv".to_owned());
                    }
                    _ => {}
                }
            }
        }
    }
}

#[cfg(target_os = "android")]
pub use native::{error_text, subscribe};

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loading_indicator_uses_observed_playback_state() {
        let mut playback = Playback::default();
        assert!(!playback.loading_state());
        playback.loading();
        assert!(playback.loading_state());
        playback.loaded = true;
        assert!(playback.loading_state());
        playback.started = true;
        assert!(!playback.loading_state());
        playback.buffering = true;
        assert!(playback.loading_state());
        playback.fail("sin conexion".to_owned());
        assert!(!playback.loading_state());
    }

    #[test]
    fn playlist_navigation_respects_boundaries_and_ready_entries() {
        let mut playback = Playback {
            active: true,
            loaded: true,
            playlist_count: 1,
            ..Playback::default()
        };
        assert!(!playback.can_navigate(false));
        assert!(!playback.can_navigate(true));
        playback.playlist_count = 3;
        assert!(!playback.can_navigate(false));
        assert!(playback.can_navigate(true));
        assert_eq!(playback.service_flags() & 192, 128);
        playback.playlist_position = 1;
        assert!(playback.can_navigate(false));
        assert!(playback.can_navigate(true));
        assert_eq!(playback.service_flags() & 192, 192);
        playback.playlist_position = 2;
        assert!(playback.can_navigate(false));
        assert!(!playback.can_navigate(true));
        playback.playlist_position = usize::MAX;
        assert!(!playback.can_navigate(false));
        assert!(!playback.can_navigate(true));
    }

    #[test]
    fn navigation_is_disabled_during_loading_recovery_and_after_stop_or_error() {
        let ready = Playback {
            active: true,
            loaded: true,
            paused: true,
            playlist_position: 1,
            playlist_count: 3,
            ..Playback::default()
        };
        assert!(ready.can_navigate(false));
        assert!(ready.can_navigate(true));
        for state in 0..5 {
            let mut playback = ready.clone();
            match state {
                0 => playback.loading(),
                1 => playback.recovering = true,
                2 => playback.cancel(),
                3 => playback.fail("test".to_owned()),
                _ => playback.ended = true,
            }
            assert!(!playback.can_navigate(false));
            assert!(!playback.can_navigate(true));
            assert_eq!(playback.service_flags() & 192, 0);
            assert_eq!(playback.playlist_count, 3);
        }
    }

    #[test]
    fn pending_renewal_retains_service_until_stop_or_terminal_failure() {
        let mut playback = Playback {
            recovering: true,
            ..Playback::default()
        };
        assert_eq!(playback.service_flags(), 41);
        playback.paused = true;
        assert_eq!(playback.service_flags(), 43);
        playback.cancel();
        assert_eq!(playback.service_flags() & 33, 0);
        playback.fail("renewal failed".into());
        assert_ne!(playback.service_flags() & 16, 0);
        playback.loading();
        assert_eq!(playback.service_flags() & 33, 1);
        assert_eq!(playback.load_generation, 1);
        playback.loading();
        assert_eq!(playback.load_generation, 2);
    }

    #[test]
    fn screen_stays_awake_only_during_playback() {
        let mut state = Playback::default();
        assert!(!state.is_playing());
        state.loading();
        assert!(!state.is_playing());
        state.loaded = true;
        assert!(!state.is_playing());
        state.started = true;
        assert!(state.is_playing());
        state.buffering = true;
        assert!(state.is_playing());
        state.paused = true;
        assert!(!state.is_playing());
        state.paused = false;
        state.fail("stream failed".to_owned());
        assert!(!state.is_playing());
        state.loading();
        state.loaded = true;
        state.started = true;
        state.ended = true;
        assert!(!state.is_playing());
        state.stop();
        assert!(!state.is_playing());
    }

    #[test]
    fn accepted_load_is_not_playback() {
        let mut state = Playback::default();
        state.loading();
        assert_eq!(state.status(), "Cargando stream...");
        state.loaded = true;
        assert_eq!(state.status(), "Preparando reproduccion...");
        state.started = true;
        assert_eq!(state.status(), "Reproduciendo");
        state.buffering = true;
        assert_eq!(state.status(), "Almacenando en buffer...");
        state.buffering = false;
        state.paused = true;
        assert_eq!(state.status(), "En pausa");
    }

    #[test]
    fn loading_preserves_the_observed_playlist_position() {
        let mut state = Playback {
            playlist_position: 2,
            ..Playback::default()
        };
        state.loading();
        assert_eq!(state.playlist_position, 2);
    }

    #[test]
    fn stop_is_distinct_from_eof_and_play_resets_it() {
        let mut state = Playback::default();
        state.loading();
        state.loaded = true;
        state.started = true;
        state.duration = Some(19.0);
        state.position = Some(8.0);
        state.stop();
        assert_eq!(state.status(), "Reproduccion detenida");
        assert!(!state.active);
        assert!(!state.loaded);
        assert!(!state.ended);
        assert!(state.position.is_none());
        assert!(state.duration.is_none());
        state.loading();
        assert_eq!(state.status(), "Cargando stream...");
        assert!(!state.stopped);
    }

    #[test]
    fn replacing_media_preserves_unchanged_observed_properties() {
        let mut state = Playback {
            paused: true,
            audio_output: Some("opensles".to_owned()),
            position: Some(13.2),
            duration: Some(18.9),
            ..Playback::default()
        };
        state.loading();
        assert!(state.paused);
        assert_eq!(state.audio_output.as_deref(), Some("opensles"));
        assert!(state.position.is_none());
        assert!(state.duration.is_none());
        state.loaded = true;
        state.started = true;
        assert_eq!(state.status(), "En pausa");
    }

    #[test]
    fn error_survives_late_property_changes_and_resets_on_new_load() {
        let mut state = Playback::default();
        state.loading();
        state.fail("loading failed".to_owned());
        state.buffering = true;
        assert_eq!(state.status(), "Error de reproduccion: loading failed");
        assert!(!state.active);
        state.loading();
        assert!(state.error.is_none());
        assert!(state.position.is_none());
        assert!(state.duration.is_none());
        assert!(state.audio_output.is_none());
    }

    #[test]
    fn eof_is_not_reported_as_playing() {
        let state = Playback {
            ended: true,
            started: true,
            ..Playback::default()
        };
        assert_eq!(state.status(), "Reproduccion finalizada");
    }

    #[test]
    fn diagnostics_are_visible_only_on_failure_and_reset_on_retry() {
        let mut state = Playback::default();
        state.loading();
        state.record_error("TLS certificate verification failed.");
        assert_eq!(state.status(), "Cargando stream...");
        state.fail("loading failed".to_owned());
        assert!(state.status().contains("certificado TLS"));
        state.loading();
        assert!(state.diagnostic.is_none());
        state.record_error("error: Could not connect to server");
        state.fail("loading failed".to_owned());
        assert!(state.status().contains("conectar al servidor"));
    }

    #[test]
    fn stream_failures_preserve_resume_state_but_tls_and_audio_do_not_retry() {
        let mut state = Playback::default();
        state.loading();
        state.record_error("HTTP error 403 Forbidden");
        state.last_position = Some(34.0);
        state.playlist_position = 2;
        state.fail_stream("loading failed".to_owned());
        assert!(state.stream_failure);
        assert_eq!(state.last_position, Some(34.0));
        assert_eq!(state.playlist_position, 2);
        state.loading();
        state.record_error("TLS certificate verification failed");
        state.fail_stream("loading failed".to_owned());
        assert!(!state.stream_failure);
        state.loading();
        state.record_error("SSL handshake failed");
        state.fail_stream("loading failed".to_owned());
        assert!(!state.stream_failure);
        state.loading();
        state.record_error("audio output failed");
        state.fail_stream("loading failed".to_owned());
        assert!(!state.stream_failure);
        state.loading();
        state.fail("command failed".to_owned());
        assert!(!state.stream_failure);
        state.stop();
        assert!(!state.stream_failure);
    }
}
