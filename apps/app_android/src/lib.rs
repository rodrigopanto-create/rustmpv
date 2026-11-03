//! Aplicación Android independiente con GUI, JNI y FFI hacia libmpv.

#[cfg(target_os = "android")]
use core_logic::PlaybackMode;
#[cfg(any(target_os = "android", test))]
use eframe::egui;
#[cfg(target_os = "android")]
use jni::objects::{JObject, JString, JValue};
#[cfg(target_os = "android")]
use jni::{JavaVM, jni_sig, jni_str};
#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
use serde::Deserialize;
#[cfg(target_os = "android")]
use std::ffi::CString;
#[cfg(target_os = "android")]
use std::sync::mpsc::{self, Receiver};

#[cfg(all(target_os = "android", feature = "chaquopy"))]
mod chaquopy;
#[cfg(any(target_os = "android", test))]
mod controls;
#[cfg(target_os = "android")]
mod egl;
#[cfg(any(target_os = "android", test))]
mod history;
#[cfg(any(target_os = "android", test))]
mod lifecycle;
#[cfg(any(target_os = "android", test))]
mod playback;
#[cfg(any(all(target_os = "android", feature = "chaquopy"), test))]
mod prefetch;
#[cfg(all(target_os = "android", feature = "native-python"))]
mod python_ffi;
#[cfg(any(all(target_os = "android", feature = "chaquopy"), test))]
mod recovery;
#[cfg(target_os = "android")]
mod screen;
#[cfg(any(target_os = "android", feature = "yt-dlp-subprocess", test))]
mod stream_url;
#[cfg(target_os = "android")]
mod video;
#[cfg(any(target_os = "android", test))]
mod youtube;

#[cfg(any(feature = "yt-dlp-subprocess", test))]
mod yt_dlp_bundle;
#[cfg(any(feature = "yt-dlp-subprocess", test))]
mod yt_dlp_process;

#[cfg(any(
    all(feature = "chaquopy", feature = "native-python"),
    all(feature = "chaquopy", feature = "yt-dlp-subprocess"),
    all(feature = "native-python", feature = "yt-dlp-subprocess")
))]
compile_error!(
    "Selecciona un solo backend Python; usa --no-default-features para los alternativos"
);

// Símbolos mínimos de libmpv disponibles en las librerías oficiales empaquetadas.
#[cfg(target_os = "android")]
unsafe extern "C" {
    fn mpv_create() -> *mut std::ffi::c_void;
    fn mpv_initialize(context: *mut std::ffi::c_void) -> std::ffi::c_int;
    fn mpv_set_option_string(
        context: *mut std::ffi::c_void,
        name: *const std::ffi::c_char,
        value: *const std::ffi::c_char,
    ) -> std::ffi::c_int;
    fn mpv_command_async(
        context: *mut std::ffi::c_void,
        userdata: u64,
        args: *const *const std::ffi::c_char,
    ) -> std::ffi::c_int;
    fn mpv_terminate_destroy(context: *mut std::ffi::c_void);
}

#[cfg(target_os = "android")]
fn append_stream(context: usize, stream_url: &str, mode: PlaybackMode) -> Result<(), String> {
    let stream_url = stream_url::validate_stream_url(stream_url)?;
    let options = if mode == PlaybackMode::Video {
        "vid=auto"
    } else {
        "vid=no"
    };
    send_mpv_command(
        context,
        &["loadfile", &stream_url, "append-play", "-1", options],
    )
}

/// Estado de la interfaz Android.
#[cfg(target_os = "android")]
struct AndroidAppState {
    android_app: winit::platform::android::activity::AndroidApp,
    url: String,
    mode: PlaybackMode,
    status: String,
    #[cfg(feature = "chaquopy")]
    cookie_state_request: Option<Receiver<Result<String, String>>>,
    #[cfg(feature = "chaquopy")]
    cookie_import_request: Option<Receiver<Result<String, String>>>,
    #[cfg(feature = "chaquopy")]
    cookie_clear_request: Option<Receiver<Result<String, String>>>,
    #[cfg(feature = "chaquopy")]
    cookie_status_request: Option<Receiver<Result<String, String>>>,
    #[cfg(feature = "chaquopy")]
    cookies_available: bool,
    #[cfg(feature = "chaquopy")]
    cookie_import_pending: bool,
    #[cfg(feature = "chaquopy")]
    cookie_poll_at: std::time::Instant,
    #[cfg(feature = "chaquopy")]
    cookie_notice: Option<(String, bool)>,
    /// Fallo de yt-dlp visible solo mientras la URL siga siendo la misma.
    resolve_error: Option<(String, String)>,
    playback: std::sync::Arc<std::sync::Mutex<playback::Playback>>,
    mpv_context: usize,
    stream_receiver: Option<Receiver<Result<stream_url::MediaInfo, String>>>,
    video: Option<std::sync::Arc<std::sync::Mutex<video::Video>>>,
    video_error: Option<String>,
    playing_mode: PlaybackMode,
    seek_position: Option<f64>,
    screen_request: Option<Receiver<Result<(), String>>>,
    keep_awake_request: Option<Receiver<Result<(), String>>>,
    keep_awake: bool,
    service_request: Option<Receiver<Result<(), String>>>,
    download_request: Option<Receiver<Result<String, String>>>,
    last_media: Option<(String, PlaybackMode, stream_url::MediaInfo)>,
    cancelled_input: Option<String>,
    last_resolved_at: Option<std::time::Instant>,
    #[cfg(feature = "chaquopy")]
    python_warmup: Option<chaquopy::Warmup>,
    #[cfg(feature = "chaquopy")]
    startup_splash_started: std::time::Instant,
    #[cfg(feature = "chaquopy")]
    prefetch: prefetch::Prefetcher<stream_url::MediaInfo>,
    /// Mantiene en la cola del worker una precarga que Play adopto.
    #[cfg(feature = "chaquopy")]
    stream_token: Option<std::sync::Arc<()>>,
    history: Vec<history::Entry>,
    history_revision: u64,
    confirm_clear: bool,
    last_interaction: std::time::Instant,
    show_controls: bool,
    muted: bool,
    process_messages: std::collections::VecDeque<String>,
    resolution_started: Option<std::time::Instant>,
    youtube_session: bool,
    session_input: String,
    last_session: bool,
}

#[cfg(target_os = "android")]
impl AndroidAppState {
    fn apply_window_insets(&self, context: &egui::Context, frame: &mut egui::Frame) {
        if let Some(window) = self.android_app.native_window() {
            let content = self.android_app.content_rect();
            let margins = system_bar_margins(
                [content.left, content.top, content.right, content.bottom],
                [window.width(), window.height()],
                context.pixels_per_point(),
            );
            frame.inner_margin.left += margins.left;
            frame.inner_margin.top += margins.top;
            frame.inner_margin.right += margins.right;
            frame.inner_margin.bottom += margins.bottom;
            if let Some(bottom) = screen::visible_bottom() {
                frame.inner_margin.bottom = frame
                    .inner_margin
                    .bottom
                    .max((window.height() - bottom).max(0) as f32 / context.pixels_per_point());
            }
        }
    }

    fn update_session_choice(&mut self) {
        if self.session_input != self.url.trim() {
            #[cfg(feature = "chaquopy")]
            if self.prefetch.pending()
                || self
                    .prefetch
                    .ready()
                    .is_some_and(|media| media.playlist_pending)
                || self.stream_receiver.is_some()
                || lifecycle::queue_pending()
            {
                chaquopy::cancel_pending();
                lifecycle::invalidate_recovery();
                self.prefetch = prefetch::Prefetcher::default();
                self.stream_receiver = None;
                self.stream_token = None;
            }
            self.session_input = self.url.trim().to_owned();
            self.youtube_session = youtube::personalized(&self.session_input);
        }
    }

    fn resolving(&self) -> bool {
        self.stream_receiver.is_some() || self.playback.lock().expect("playback lock").recovering
    }

    fn process_pending(&self) -> bool {
        #[cfg(feature = "chaquopy")]
        if self.prefetch.pending()
            || self
                .prefetch
                .ready()
                .is_some_and(|media| media.playlist_pending)
            || lifecycle::queue_pending()
        {
            return true;
        }
        self.resolving()
    }

    fn poll_download(&mut self) {
        let result =
            self.download_request
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                        "Android no devolvio el resultado de la descarga".to_owned(),
                    )),
                    Err(mpsc::TryRecvError::Empty) => None,
                });
        if let Some(result) = result {
            self.download_request = None;
            match result {
                Ok(id) if id == "permission-required" => {
                    self.status =
                        "Concede el permiso de almacenamiento y vuelve a descargar".to_owned();
                    self.process_message(self.status.clone());
                }
                Ok(id) => {
                    log::info!("Descarga enviada a Android DownloadManager ({id})");
                    self.status = "Descarga iniciada; revisa la notificacion de Android".to_owned();
                    self.process_message(self.status.clone());
                }
                Err(error) => {
                    log::error!("Encolar descarga en Android: {error}");
                    self.status = format!("No se pudo iniciar la descarga: {error}");
                    self.process_message(self.status.clone());
                }
            }
        }
    }

    fn new(
        android_app: winit::platform::android::activity::AndroidApp,
        gl: Option<&eframe::glow::Context>,
    ) -> Self {
        let (mpv_context, status) = match initialize_mpv(PlaybackMode::Audio) {
            Ok(context) => (context, "Android listo".to_owned()),
            Err(error) => {
                log::error!("Inicializar libmpv: {error}");
                (0, error)
            }
        };
        let renderer = if mpv_context != 0 {
            gl.ok_or_else(|| "No hay contexto OpenGL para video".to_owned())
                .and_then(|gl| video::Video::new(gl, mpv_context))
        } else {
            Err("libmpv no esta inicializado".to_owned())
        };
        let (video, video_error) = match renderer {
            Ok(video) => (
                Some(std::sync::Arc::new(std::sync::Mutex::new(video))),
                None,
            ),
            Err(error) => {
                log::error!("{error}");
                (None, Some(error))
            }
        };
        history::init(android_app.internal_data_path().as_deref());
        let playback = lifecycle::register(mpv_context);
        #[cfg(feature = "chaquopy")]
        let startup_splash_started = std::time::Instant::now();
        #[cfg(feature = "chaquopy")]
        if let Err(error) = chaquopy::start_worker() {
            log::error!("No se pudo iniciar el worker Python: {error}");
        }
        #[cfg(feature = "chaquopy")]
        let cookie_state_request = Some(screen::has_supported_site_cookies(&android_app));
        Self {
            android_app,
            url: String::new(),
            mode: PlaybackMode::Audio,
            status,
            #[cfg(feature = "chaquopy")]
            cookie_state_request,
            #[cfg(feature = "chaquopy")]
            cookie_import_request: None,
            #[cfg(feature = "chaquopy")]
            cookie_clear_request: None,
            #[cfg(feature = "chaquopy")]
            cookie_status_request: None,
            #[cfg(feature = "chaquopy")]
            cookies_available: false,
            #[cfg(feature = "chaquopy")]
            cookie_import_pending: false,
            #[cfg(feature = "chaquopy")]
            cookie_poll_at: std::time::Instant::now(),
            #[cfg(feature = "chaquopy")]
            cookie_notice: None,
            resolve_error: None,
            playback,
            mpv_context,
            stream_receiver: None,
            video,
            video_error,
            playing_mode: PlaybackMode::Audio,
            seek_position: None,
            screen_request: None,
            keep_awake_request: None,
            keep_awake: false,
            service_request: None,
            download_request: None,
            last_media: None,
            cancelled_input: None,
            last_resolved_at: None,
            #[cfg(feature = "chaquopy")]
            python_warmup: None,
            #[cfg(feature = "chaquopy")]
            startup_splash_started,
            #[cfg(feature = "chaquopy")]
            prefetch: prefetch::Prefetcher::default(),
            #[cfg(feature = "chaquopy")]
            stream_token: None,
            history: Vec::new(),
            history_revision: 0,
            confirm_clear: false,
            last_interaction: std::time::Instant::now(),
            show_controls: true,
            muted: false,
            process_messages: std::collections::VecDeque::new(),
            resolution_started: None,
            youtube_session: false,
            session_input: String::new(),
            last_session: false,
        }
    }
}

#[cfg(any(target_os = "android", test))]
fn system_bar_margins(content: [i32; 4], window: [i32; 2], pixels_per_point: f32) -> egui::Margin {
    let [left, top, right, bottom] = content;
    let [width, height] = window;
    // NativeActivity may not have received its initial content rectangle yet.
    if width <= 0 || height <= 0 || right <= left || bottom <= top {
        return egui::Margin::default();
    }
    egui::Margin {
        left: left.clamp(0, width) as f32 / pixels_per_point,
        top: top.clamp(0, height) as f32 / pixels_per_point,
        right: (width - right.clamp(0, width)) as f32 / pixels_per_point,
        bottom: (height - bottom.clamp(0, height)) as f32 / pixels_per_point,
    }
}

#[cfg(any(all(target_os = "android", feature = "chaquopy"), test))]
const STARTUP_SPLASH_MINIMUM: std::time::Duration = std::time::Duration::from_millis(900);
#[cfg(any(all(target_os = "android", feature = "chaquopy"), test))]
const STARTUP_SPLASH_MAXIMUM: std::time::Duration = std::time::Duration::from_secs(8);

#[cfg(any(all(target_os = "android", feature = "chaquopy"), test))]
fn startup_splash_visible(
    started: std::time::Instant,
    warmup_pending: bool,
    now: std::time::Instant,
) -> bool {
    let elapsed = now.saturating_duration_since(started);
    elapsed < STARTUP_SPLASH_MINIMUM || (warmup_pending && elapsed < STARTUP_SPLASH_MAXIMUM)
}

#[cfg(any(all(target_os = "android", feature = "chaquopy"), test))]
fn draw_startup_splash(context: &egui::Context, mut frame: egui::Frame) -> egui::Rect {
    frame.fill = egui::Color32::from_rgb(12, 18, 30);
    egui::CentralPanel::default()
        .frame(frame)
        .show(context, |ui| {
            let content = ui.available_rect_before_wrap();
            let block = egui::Rect::from_center_size(
                content.center(),
                egui::vec2(content.width(), content.height().min(150.0)),
            );
            ui.allocate_new_ui(egui::UiBuilder::new().max_rect(block), |ui| {
                ui.vertical_centered(|ui| {
                    ui.heading(
                        egui::RichText::new("VisualRustMPV")
                            .size(32.0)
                            .strong()
                            .color(egui::Color32::from_rgb(113, 190, 255)),
                    );
                    ui.add_space(16.0);
                    ui.add(egui::Spinner::new().size(28.0));
                    ui.add_space(12.0);
                    ui.label("Preparando el motor multimedia...");
                })
                .response
                .rect
            })
            .inner
        })
        .inner
}

#[cfg(target_os = "android")]
impl eframe::App for AndroidAppState {
    /// Dibuja la interfaz Android y expone el formato directo elegido.
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.update_session_choice();
        #[cfg(feature = "chaquopy")]
        self.poll_cookie_auth(context);
        if screen::fullscreen()
            && self.screen_request.is_none()
            && context
                .input_mut(|input| input.consume_key(egui::Modifiers::NONE, egui::Key::Escape))
        {
            self.screen_request = Some(screen::request(&self.android_app, false));
        }
        if let Some(receiver) = &self.screen_request {
            match receiver.try_recv() {
                Ok(result) => {
                    self.screen_request = None;
                    if let Err(error) = result {
                        log::error!("Cambiar pantalla: {error}");
                        self.status = format!("No se pudo cambiar la pantalla: {error}");
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.screen_request = None;
                    log::error!("El cambio de pantalla no devolvio resultado");
                    self.status = "El cambio de pantalla no devolvio resultado".to_owned();
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        self.poll_stream();
        self.poll_download();
        #[cfg(feature = "chaquopy")]
        {
            lifecycle::update_input(self.url.trim());
        }
        lifecycle::poll();
        #[cfg(feature = "chaquopy")]
        {
            self.poll_recovery();
            self.poll_python_worker();
            self.poll_prefetch();
        }
        #[cfg(feature = "chaquopy")]
        {
            let now = std::time::Instant::now();
            let warmup_pending = matches!(chaquopy::warmup(), Some(chaquopy::Warmup::Pending));
            let splash_visible =
                startup_splash_visible(self.startup_splash_started, warmup_pending, now);
            if splash_visible {
                context.request_repaint_after(std::time::Duration::from_millis(80));
            } else if warmup_pending {
                context.request_repaint_after(std::time::Duration::from_millis(500));
            }
            if splash_visible {
                let mut splash_frame = egui::Frame::central_panel(&context.style());
                self.apply_window_insets(context, &mut splash_frame);
                draw_startup_splash(context, splash_frame);
                return;
            }
        }
        let previous = self.status.clone();
        let process_pending = self.process_pending();
        let playback = {
            let mut playback = self.playback.lock().expect("playback lock");
            history::observe(&playback);
            if let Some(video) = &self.video {
                let error = video.lock().expect("video renderer lock").error.clone();
                if let Some(error) = error.as_ref().filter(|_| self.video_error.is_none()) {
                    playback.fail(format!("Video: {error}"));
                    if let Err(error) = send_mpv_command(self.mpv_context, &["stop"]) {
                        log::error!("Detener video tras fallo de render: {error}");
                    }
                }
                self.video_error = error;
            }
            if !process_pending
                && playback.status() != previous
                && (playback.active
                    || playback.stopped
                    || playback.ended
                    || playback.error.is_some())
            {
                self.status = playback.status();
            }
            playback.clone()
        };
        if let Some(receiver) = &self.service_request {
            match receiver.try_recv() {
                Ok(result) => {
                    self.service_request = None;
                    if let Err(error) = result {
                        log::error!("Servicio multimedia: {error}");
                        self.run_player_command(&["stop"]);
                        self.playback
                            .lock()
                            .expect("playback lock")
                            .fail(error.clone());
                        self.status = format!("Servicio multimedia: {error}");
                    }
                }
                Err(mpsc::TryRecvError::Disconnected) => {
                    self.service_request = None;
                    log::error!("El servicio multimedia no devolvio resultado");
                    self.run_player_command(&["stop"]);
                    self.playback
                        .lock()
                        .expect("playback lock")
                        .fail("El servicio multimedia no devolvio resultado".to_owned());
                }
                Err(mpsc::TryRecvError::Empty) => {}
            }
        }
        if let Some(receiver) = &self.keep_awake_request {
            let result = match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => Some(Err(
                    "El cambio de pantalla encendida no devolvio resultado".to_owned(),
                )),
                Err(mpsc::TryRecvError::Empty) => None,
            };
            if let Some(result) = result {
                self.keep_awake_request = None;
                if let Err(error) = result {
                    log::error!("Mantener pantalla encendida: {error}");
                    self.status = format!("No se pudo cambiar la pantalla encendida: {error}");
                }
            }
        }
        let keep_awake = self.playing_mode == PlaybackMode::Video && playback.is_playing();
        if self.keep_awake_request.is_none() && keep_awake != self.keep_awake {
            self.keep_awake = keep_awake;
            self.keep_awake_request = Some(screen::keep_awake(&self.android_app, keep_awake));
        }
        let mut frame = egui::Frame::central_panel(&context.style());
        let now = std::time::Instant::now();
        let interacting = context.input(|input| {
            input.pointer.delta() != egui::Vec2::ZERO
                || input.pointer.any_down()
                || input.events.iter().any(|event| {
                    matches!(
                        event,
                        egui::Event::PointerButton { .. }
                            | egui::Event::Touch { .. }
                            | egui::Event::Key { .. }
                            | egui::Event::Text(_)
                            | egui::Event::MouseWheel { .. }
                            | egui::Event::WindowFocused(true)
                    )
                })
        });
        let playing_without_process = playback.is_playing() && !self.process_pending();
        controls::update_visibility(
            &mut self.last_interaction,
            &mut self.show_controls,
            now,
            playing_without_process,
            interacting,
        );
        if self.show_controls && playback.is_playing() {
            context.request_repaint_after(
                (controls::HIDE_AFTER + std::time::Duration::from_millis(1))
                    .saturating_sub(now.duration_since(self.last_interaction)),
            );
        }
        if screen::fullscreen() {
            frame.inner_margin = egui::Margin::default();
            frame.fill = egui::Color32::BLACK;
        }
        self.apply_window_insets(context, &mut frame);
        egui::CentralPanel::default()
            .frame(frame)
            .show(context, |ui| {
                if self.show_controls {
                    egui::TopBottomPanel::bottom("controls_panel")
                        .resizable(false)
                        .exact_height(controls::panel_height(ui.available_width()))
                        .frame(egui::Frame::none().inner_margin(egui::Margin::symmetric(0.0, 6.0)))
                        .show_inside(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if controls::stacked(ui.available_width()) {
                                ui.horizontal(|ui| self.draw_transport(ui));
                                ui.horizontal(|ui| self.draw_seek(ui));
                            } else {
                                ui.horizontal_centered(|ui| {
                                    self.draw_transport(ui);
                                    self.draw_seek(ui);
                                });
                            }
                        });
                }
                if screen::fullscreen() {
                    let (rect, _) =
                        ui.allocate_exact_size(ui.available_size(), egui::Sense::hover());
                    self.draw_video(ui, rect);
                    return;
                }
                egui::ScrollArea::vertical()
                    .id_salt("player_content")
                    .auto_shrink([false, false])
                    .show(ui, |ui| {
                ui.heading("VisualRustMPV Android");
                ui.add_enabled_ui(!self.resolving(), |ui| {
                    ui.horizontal(|ui| {
                        let url_edit = ui.add(
                            egui::TextEdit::singleline(&mut self.url).hint_text("URL del medio"),
                        );
                        if ui.button("📋 Pegar").clicked() {
                            // The native egui backend reads Android's clipboard for this focused editor.
                            url_edit.request_focus();
                            match read_android_clipboard() {
                                Ok(value) if !value.is_empty() => self.url = value,
                                Ok(_) => self.status = "El portapapeles está vacío".to_owned(),
                                Err(error) => {
                                    self.status =
                                        format!("No se pudo leer el portapapeles: {error}")
                                }
                            }
                        }
                    });
                });
                #[cfg(feature = "chaquopy")]
                {
                    ui.add_enabled_ui(!self.resolving(), |ui| {
                        ui.horizontal(|ui| {
                            if ui
                                .add_enabled(
                                    !self.cookie_import_pending
                                        && self.cookie_clear_request.is_none(),
                                    egui::Button::new(if self.cookies_available {
                                        "Cambiar cookies de fallback"
                                    } else {
                                        "Importar cookies de fallback"
                                    }),
                                )
                                .clicked()
                            {
                                self.cookie_import_pending = true;
                                self.cookie_poll_at = std::time::Instant::now()
                                    + std::time::Duration::from_millis(300);
                                self.cookie_import_request = Some(
                                    screen::open_supported_site_cookies_file(&self.android_app),
                                );
                                self.cookie_notice = Some((
                                    "Selecciona cookies.txt para YouTube, X/Twitter, Instagram o Facebook."
                                        .to_owned(),
                                    false,
                                ));
                                self.status = "Selecciona cookies.txt para YouTube, X/Twitter, Instagram o Facebook."
                                    .to_owned();
                            }
                            if self.cookies_available
                                && ui
                                    .add_enabled(
                                        !self.cookie_import_pending
                                            && self.cookie_clear_request.is_none(),
                                        egui::Button::new("Eliminar cookies guardadas"),
                                    )
                                    .clicked()
                            {
                                self.cookie_clear_request = Some(
                                    screen::clear_supported_site_cookies(&self.android_app),
                                );
                            }
                        });
                    });
                    if media_site(self.url.trim()) == Some(MediaSite::YouTube) {
                        let choice = ui.add_enabled(
                            !self.resolving(),
                            egui::Checkbox::new(
                                &mut self.youtube_session,
                                "Usar mi sesion de YouTube / Music",
                            ),
                        );
                        if choice.changed() {
                            chaquopy::cancel_pending();
                            lifecycle::invalidate_recovery();
                            self.prefetch = prefetch::Prefetcher::default();
                            self.stream_token = None;
                            self.resolve_error = None;
                            self.last_resolved_at = None;
                        }
                        if self.youtube_session {
                            ui.weak("Modo cuenta: exige una sesion aceptada por YouTube; no cambia a un mix anonimo si falla.");
                        } else {
                            ui.weak("Los radios publicos no necesitan cuenta. Activa la sesion solo si quieres usar tu cuenta y sus recomendaciones.");
                        }
                        ui.weak("Listas: hasta 100 elementos por carga; los no disponibles se indican y se omiten.");
                    }
                    ui.weak("En modo normal: primero sin cuenta (clientes automaticos + PO Token local); si falla, cookies.");
                    ui.weak("El fallback usa tu sesion. No oculta tu IP ni garantiza acceso a todos los enlaces.");
                    if self.cookies_available {
                        ui.weak("Cookies cifradas; solo para este sitio al usar sesion o fallback.");
                    }
                    if let Some((notice, error)) = &self.cookie_notice {
                        if *error {
                            ui.colored_label(egui::Color32::LIGHT_RED, notice);
                        } else {
                            ui.weak(notice);
                        }
                    }
                }
                ui.add_enabled_ui(!self.resolving(), |ui| {
                    ui.horizontal(|ui| {
                        ui.selectable_value(&mut self.mode, PlaybackMode::Video, "Video MP4");
                        ui.selectable_value(&mut self.mode, PlaybackMode::Audio, "Audio directo");
                        #[cfg(feature = "chaquopy")]
                        {
                            let enabled = chaquopy::update_check_available();
                            let button =
                                ui.add_enabled(enabled, egui::Button::new("Actualizar yt-dlp"));
                            if button.clicked() {
                                if let Err(error) = chaquopy::check_update_now() {
                                    log::error!("Buscar actualizacion de yt-dlp: {error}");
                                    self.status = format!(
                                        "No se pudo buscar actualizacion de yt-dlp: {error}"
                                    );
                                }
                            } else if !enabled {
                                button.on_hover_text("Comprobando actualizaciones...");
                            }
                        }
                    });
                });
                if self.process_pending()
                    || (playback.active
                        && playback.error.is_none()
                        && (!playback.loaded || !playback.started || playback.buffering))
                {
                    controls::loading_bar(ui);
                }
                ui.label(&self.status);
                if self.process_pending() {
                    if let Some(started) = self.resolution_started {
                        ui.weak(format!(
                            "Proceso en curso: {:.0} s. Espera el resultado o cancela para probar otra URL.",
                            started.elapsed().as_secs_f64()
                        ));
                    }
                    if ui.button("Cancelar proceso y volver al player").clicked() {
                        self.cancel_process();
                    }
                }
                if !self.process_messages.is_empty() {
                    egui::CollapsingHeader::new("Consola del proceso")
                        .default_open(self.process_pending())
                        .show(ui, |ui| {
                            for message in &self.process_messages {
                                ui.label(egui::RichText::new(message).monospace().small());
                            }
                        });
                }
                if let Some(error) = &self.video_error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Video: {error}"));
                }
                if let Some((_, error)) = self
                    .resolve_error
                    .as_ref()
                    .filter(|(input, _)| input == self.url.trim())
                {
                    ui.colored_label(egui::Color32::LIGHT_RED, error);
                } else if let Some(error) = &playback.error {
                    ui.colored_label(egui::Color32::LIGHT_RED, format!("Error: {error}"));
                }
                self.draw_download(ui, &playback);
                if self.playing_mode == PlaybackMode::Video {
                    let width = ui.available_width();
                    let height = (width * 9.0 / 16.0).min(ui.available_height()).max(1.0);
                    let (rect, _) =
                        ui.allocate_exact_size(egui::vec2(width, height), egui::Sense::hover());
                    self.draw_video(ui, rect);
                }
                self.draw_history(ui);
                    });
            });
        if self.mpv_context != 0 {
            let interval = if self.playing_mode == PlaybackMode::Video {
                16
            } else {
                50
            };
            context.request_repaint_after(std::time::Duration::from_millis(interval));
        }
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let (Some(video), Some(gl)) = (&self.video, gl) {
            video.lock().expect("video renderer lock").destroy(gl);
        }
    }
}

#[cfg(target_os = "android")]
impl AndroidAppState {
    #[cfg(feature = "chaquopy")]
    fn poll_cookie_auth(&mut self, context: &egui::Context) {
        let state =
            self.cookie_state_request
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(Err("La consulta de cookies se desconecto".to_owned()))
                    }
                    Err(mpsc::TryRecvError::Empty) => None,
                });
        if let Some(result) = state {
            self.cookie_state_request = None;
            match result {
                Ok(value) if value == "available" => self.cookies_available = true,
                Ok(value) if value == "empty" => self.cookies_available = false,
                Ok(value) => {
                    log::error!("Estado de cookies inesperado: {value}");
                    self.cookie_notice =
                        Some((format!("Estado de cookies inesperado: {value}"), true));
                    self.status = format!("Estado de cookies inesperado: {value}");
                }
                Err(error) => {
                    log::error!("Consultar cookies guardadas: {error}");
                    self.cookie_notice = Some((
                        format!("No se pudieron consultar las cookies guardadas: {error}"),
                        true,
                    ));
                    self.status =
                        format!("No se pudieron consultar las cookies guardadas: {error}");
                }
            }
        }

        let import =
            self.cookie_import_request
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(Err("El selector de archivos se desconecto".to_owned()))
                    }
                    Err(mpsc::TryRecvError::Empty) => None,
                });
        if let Some(result) = import {
            self.cookie_import_request = None;
            match result {
                Ok(value) if value == "opened" => {}
                Ok(value) => {
                    self.cookie_import_pending = false;
                    log::error!("Respuesta inesperada al abrir cookies: {value}");
                    self.cookie_notice = Some((
                        format!("No se pudo abrir el selector de cookies: {value}"),
                        true,
                    ));
                    self.status = format!("No se pudo abrir el selector de cookies: {value}");
                }
                Err(error) => {
                    self.cookie_import_pending = false;
                    log::error!("Abrir selector de cookies: {error}");
                    self.cookie_notice = Some((
                        format!("No se pudo abrir el selector de archivos: {error}"),
                        true,
                    ));
                    self.status = format!("No se pudo abrir el selector de archivos: {error}");
                }
            }
        }

        let clear =
            self.cookie_clear_request
                .as_ref()
                .and_then(|receiver| match receiver.try_recv() {
                    Ok(result) => Some(result),
                    Err(mpsc::TryRecvError::Disconnected) => {
                        Some(Err("La eliminacion de cookies se desconecto".to_owned()))
                    }
                    Err(mpsc::TryRecvError::Empty) => None,
                });
        if let Some(result) = clear {
            self.cookie_clear_request = None;
            match result {
                Ok(value) if value == "deleted" => {
                    self.cookies_available = false;
                    self.cookie_notice = Some(("Cookies guardadas eliminadas".to_owned(), false));
                    self.status = "Cookies guardadas eliminadas".to_owned();
                }
                Ok(value) => {
                    log::error!("Respuesta inesperada al eliminar cookies: {value}");
                    self.cookie_notice = Some((
                        format!("No se pudieron eliminar las cookies: {value}"),
                        true,
                    ));
                    self.status = format!("No se pudieron eliminar las cookies: {value}");
                }
                Err(error) => {
                    log::error!("Eliminar cookies guardadas: {error}");
                    self.cookie_notice = Some((
                        format!("No se pudieron eliminar las cookies: {error}"),
                        true,
                    ));
                    self.status = format!("No se pudieron eliminar las cookies: {error}");
                }
            }
        }

        let cookie_status = self
            .cookie_status_request
            .as_ref()
            .and_then(|receiver| match receiver.try_recv() {
                Ok(result) => Some(result),
                Err(mpsc::TryRecvError::Disconnected) => {
                    Some(Err("La importacion de cookies se desconecto".to_owned()))
                }
                Err(mpsc::TryRecvError::Empty) => None,
            });
        if let Some(result) = cookie_status {
            self.cookie_status_request = None;
            match result {
                Ok(status) if status.is_empty() => {}
                Ok(status) => {
                    self.cookie_import_pending = false;
                    match status.as_str() {
                        "fallback" => {
                            self.cookie_notice = Some((
                                "El intento anonimo fallo; se reintenta usando tu sesion guardada."
                                    .to_owned(),
                                false,
                            ));
                        }
                        "session" => {
                            self.cookie_notice = Some((
                                "Resolviendo la lista con tu sesion de YouTube / Music.".to_owned(),
                                false,
                            ));
                        }
                        "imported" => {
                            self.cookies_available = true;
                            self.cookie_notice = Some((
                                "Cookies protegidas; se usaran solo si falla el intento anonimo."
                                    .to_owned(),
                                false,
                            ));
                            self.status =
                                "Cookies protegidas; se usaran solo si falla el intento anonimo."
                                    .to_owned();
                        }
                        "invalid" => {
                            self.cookie_notice = Some((
                                "Archivo invalido: requiere cookies de sitios compatibles en formato Netscape."
                                    .to_owned(),
                                true,
                            ));
                            self.status = "Archivo invalido: requiere cookies de sitios compatibles en formato Netscape."
                                .to_owned();
                        }
                        "error" => {
                            self.cookie_notice =
                                Some(("No se pudieron guardar las cookies".to_owned(), true));
                            self.status = "No se pudieron guardar las cookies".to_owned();
                        }
                        "cancelled" => {
                            self.cookie_notice =
                                Some(("Importacion de cookies cancelada".to_owned(), false));
                            self.status = "Importacion de cookies cancelada".to_owned();
                        }
                        "deleted" => {
                            self.cookies_available = false;
                            self.cookie_notice =
                                Some(("Cookies guardadas eliminadas".to_owned(), false));
                            self.status = "Cookies guardadas eliminadas".to_owned();
                        }
                        _ => {
                            log::error!("Estado de importacion inesperado: {status}");
                            self.cookie_notice =
                                Some((format!("Estado de importacion inesperado: {status}"), true));
                            self.status = format!("Estado de importacion inesperado: {status}");
                        }
                    }
                }
                Err(error) => {
                    self.cookie_import_pending = false;
                    log::error!("Consultar resultado de cookies: {error}");
                    self.cookie_notice = Some((
                        format!("No se pudo leer el resultado de cookies: {error}"),
                        true,
                    ));
                    self.status = format!("No se pudo leer el resultado de cookies: {error}");
                }
            }
        }

        if self.cookie_status_request.is_none() && std::time::Instant::now() >= self.cookie_poll_at
        {
            self.cookie_status_request = Some(screen::take_supported_site_cookies_status(
                &self.android_app,
            ));
            self.cookie_poll_at = std::time::Instant::now() + std::time::Duration::from_millis(300);
            context.request_repaint_after(std::time::Duration::from_millis(300));
        }
    }

    fn draw_download(&mut self, ui: &mut egui::Ui, playback: &playback::Playback) {
        let Some((referer, audio_only, target)) =
            self.last_media.as_ref().map(|(input, mode, media)| {
                (
                    input.clone(),
                    *mode == PlaybackMode::Audio,
                    media.download_target(playback.playlist_position),
                )
            })
        else {
            return;
        };
        let unavailable = target.as_ref().err().cloned();
        let enabled =
            playback.active && playback.loaded && self.download_request.is_none() && target.is_ok();
        let button = ui.add_enabled(enabled, egui::Button::new("Descargar medio actual"));
        let clicked = button.clicked();
        if let Some(reason) = unavailable {
            button.on_hover_text(&reason);
            ui.weak(reason);
        } else {
            button.on_hover_text("Guardar el stream actual en Descargas de Android");
        }
        if clicked && let Ok(target) = target {
            self.status = format!("Preparando descarga de {}...", target.title);
            self.download_request = Some(screen::download_media(
                &self.android_app,
                target.url,
                target.title,
                target.extension,
                referer,
                audio_only,
            ));
        }
    }

    fn draw_transport(&mut self, ui: &mut egui::Ui) {
        use controls::{Icon, button};
        let playback = self.playback.lock().expect("playback lock").clone();
        let pending = self.process_pending();
        let can_play = !self.resolving();
        #[cfg(feature = "chaquopy")]
        let can_play = can_play && (!lifecycle::queue_pending() || playback.active);
        if button(
            ui,
            Icon::Previous,
            "Anterior en la lista",
            playback.can_navigate(false),
        )
        .clicked()
        {
            self.navigate_playlist(false);
        }
        if button(ui, Icon::Play, "Reproducir / Reanudar", can_play).clicked() {
            let same_media = self
                .last_media
                .as_ref()
                .is_some_and(|(input, mode, _)| input == self.url.trim() && *mode == self.mode)
                && self.last_session == self.youtube_session;
            if same_media && playback.active && playback.loaded {
                self.run_player_command(&["set", "pause", "no"]);
            } else {
                self.resolve_and_play();
            }
        }
        if button(
            ui,
            Icon::Pause,
            "Pausar",
            ((playback.active && playback.loaded) || playback.recovering) && !playback.paused,
        )
        .clicked()
        {
            self.run_player_command(&["set", "pause", "yes"]);
        }
        if button(
            ui,
            Icon::Stop,
            "Detener / Cancelar",
            playback.active || pending,
        )
        .clicked()
        {
            self.cancel_process();
        }
        if button(
            ui,
            Icon::Next,
            "Siguiente en la lista",
            playback.can_navigate(true),
        )
        .clicked()
        {
            self.navigate_playlist(true);
        }
        if button(
            ui,
            if self.muted { Icon::Muted } else { Icon::Sound },
            if self.muted {
                "Sonido OFF: activar"
            } else {
                "Sonido ON: silenciar"
            },
            self.mpv_context != 0,
        )
        .clicked()
        {
            let muted = !self.muted;
            if self.run_player_command(&["set", "mute", if muted { "yes" } else { "no" }]) {
                self.muted = muted;
                self.process_message(if muted { "Sonido OFF" } else { "Sonido ON" }.to_owned());
            }
        }
        let fullscreen = screen::fullscreen();
        if button(
            ui,
            if fullscreen {
                Icon::Restore
            } else {
                Icon::Fullscreen
            },
            if fullscreen {
                "Salir de pantalla completa"
            } else {
                "Pantalla completa horizontal"
            },
            self.screen_request.is_none()
                && (fullscreen
                    || (self.playing_mode == PlaybackMode::Video
                        && playback.loaded
                        && self.video_error.is_none())),
        )
        .clicked()
        {
            self.screen_request = Some(screen::request(&self.android_app, !fullscreen));
        }
    }

    fn draw_seek(&mut self, ui: &mut egui::Ui) {
        let playback = self.playback.lock().expect("playback lock").clone();
        let duration = playback
            .duration
            .filter(|value| value.is_finite() && *value > 0.0);
        let timer = format!(
            "{} / {}",
            controls::clock(playback.position.unwrap_or(0.0)),
            duration
                .map(controls::clock)
                .unwrap_or_else(|| "--:--".to_owned())
        );
        let mut position = self
            .seek_position
            .unwrap_or(playback.position.unwrap_or(0.0));
        let slider = controls::seek(
            ui,
            &mut position,
            duration,
            &timer,
            playback.active && playback.loaded && duration.is_some(),
        );
        if slider.dragged() {
            self.seek_position = Some(position);
        }
        if slider.drag_stopped() || (slider.changed() && !slider.dragged()) {
            self.seek_position = None;
            self.run_player_command(&["seek", &position.to_string(), "absolute"]);
        }
    }

    fn draw_history(&mut self, ui: &mut egui::Ui) {
        let revision = history::revision();
        if revision != self.history_revision {
            self.history = history::entries();
            self.history_revision = revision;
        }
        if self.history.is_empty() {
            return;
        }
        ui.separator();
        let mut play = None;
        let mut remove = None;
        ui.horizontal(|ui| {
            ui.strong("Historial");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if self.confirm_clear {
                    if ui.small_button("Cancelar").clicked() {
                        self.confirm_clear = false;
                    }
                    if ui.small_button("Sí, borrar").clicked() {
                        self.confirm_clear = false;
                        history::clear();
                    }
                } else if ui.small_button("Borrar todo").clicked() {
                    self.confirm_clear = true;
                }
            });
        });
        let enabled = !self.resolving();
        for entry in &self.history {
            ui.horizontal(|ui| {
                if ui.small_button("🗙").on_hover_text("Quitar").clicked() {
                    remove = Some(entry.key.clone());
                }
                ui.vertical(|ui| {
                    let title = ui.add_enabled(
                        enabled,
                        egui::Label::new(entry.label())
                            .truncate()
                            .sense(egui::Sense::click()),
                    );
                    let duration = entry.duration.filter(|value| value.is_finite());
                    let progress = if entry.finished() {
                        "visto".to_owned()
                    } else if let (Some(position), Some(duration)) =
                        (entry.resume_position(), duration)
                    {
                        format!(
                            "continuar en {} / {}",
                            controls::clock(position),
                            controls::clock(duration)
                        )
                    } else {
                        duration.map(controls::clock).unwrap_or_default()
                    };
                    let mode = if entry.audio_only { "Audio" } else { "Video" };
                    let details = [mode, entry.artist.as_str(), progress.as_str()]
                        .into_iter()
                        .filter(|part| !part.is_empty())
                        .collect::<Vec<_>>()
                        .join(" · ");
                    ui.add(
                        egui::Label::new(egui::RichText::new(details).weak().small()).truncate(),
                    );
                    if title.clicked() {
                        play = Some((entry.input.clone(), entry.audio_only));
                    }
                });
            });
        }
        if let Some(key) = remove {
            history::remove(&key);
        }
        if let Some((input, audio_only)) = play {
            self.url = input;
            self.mode = if audio_only {
                PlaybackMode::Audio
            } else {
                PlaybackMode::Video
            };
            self.resolve_and_play();
        }
    }

    fn draw_video(&self, ui: &egui::Ui, rect: egui::Rect) {
        ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        if let Some(video) = &self.video {
            let video = video.clone();
            ui.painter().add(egui::PaintCallback {
                rect,
                callback: std::sync::Arc::new(eframe::egui_glow::CallbackFn::new(
                    move |info, painter| {
                        video
                            .lock()
                            .expect("video renderer lock")
                            .paint(info, painter)
                    },
                )),
            });
        }
    }

    fn navigate_playlist(&mut self, next: bool) {
        match lifecycle::navigate_playlist(next) {
            Ok(()) => self.seek_position = None,
            Err(error) => {
                log::error!("Cambiar elemento de la lista: {error}");
                self.status = error.clone();
                self.process_message(error);
            }
        }
    }

    fn run_player_command(&mut self, args: &[&str]) -> bool {
        if let Err(error) = send_mpv_command(self.mpv_context, args) {
            log::error!("Control libmpv: {error}");
            self.playback
                .lock()
                .expect("playback lock")
                .fail(error.clone());
            self.status = error;
            return false;
        }
        if let ["set", "pause", value] = args {
            lifecycle::set_paused(*value == "yes");
        }
        true
    }

    fn process_message(&mut self, message: String) {
        if self.process_messages.back() == Some(&message) {
            return;
        }
        log::info!("Player: {message}");
        if self.process_messages.len() == 12 {
            self.process_messages.pop_front();
        }
        self.process_messages.push_back(message);
    }

    fn cancel_process(&mut self) {
        // Release the UI immediately; a cancelled worker can never load its late result.
        self.stream_receiver = None;
        self.resolution_started = None;
        self.cancelled_input = Some(self.url.trim().to_owned());
        self.resolve_error = None;
        #[cfg(feature = "chaquopy")]
        {
            self.prefetch = prefetch::Prefetcher::default();
            self.stream_token = None;
        }
        lifecycle::cancel();
        self.seek_position = None;
        if self.run_player_command(&["stop"]) {
            self.status = "Proceso cancelado. Puedes introducir otra URL.".to_owned();
        }
        self.process_message(self.status.clone());
        if screen::fullscreen() && self.screen_request.is_none() {
            self.screen_request = Some(screen::request(&self.android_app, false));
        }
    }

    fn load_resolved_stream(&mut self, media: stream_url::MediaInfo) {
        if !media.playlist_notice.is_empty() {
            self.process_message(media.playlist_notice.clone());
        }
        let queued = media.playlist.clone();
        let mut playback = self.playback.lock().expect("playback lock");
        playback.poll(self.mpv_context);
        // Records the previous item's last position before switching.
        history::observe(&playback);
        let resume = history::start(
            self.url.trim(),
            self.mode == PlaybackMode::Audio,
            &media.title,
            &media.artist,
        );
        let loaded = load_media(self.mpv_context, &media, self.mode, resume, false);
        match loaded {
            Ok(()) => {
                playback.playlist_position = 0;
                playback.loading();
                self.playing_mode = self.mode;
                self.seek_position = None;
                self.status = match resume {
                    Some(position) => format!("Reanudando en {}", controls::clock(position)),
                    None if queued.is_empty() => playback.status(),
                    None => format!("Lista en cola: {} elementos", queued.len() + 1),
                };
                if !queued.is_empty() {
                    log::info!("Lista encolada: {} elementos", queued.len() + 1);
                }
                drop(playback);
                #[cfg(feature = "chaquopy")]
                lifecycle::begin_media(self.url.trim(), self.mode, media.clone());
                lifecycle::set_video(self.mode == PlaybackMode::Video);
                self.service_request =
                    Some(screen::start_playback_service(&self.android_app, &media));
                self.last_media = Some((self.url.trim().to_owned(), self.mode, media));
                self.last_session = self.youtube_session;
            }
            Err(error) => {
                log::error!("Cargar stream: {error}");
                playback.fail(error);
                self.status = playback.status();
            }
        }
    }

    #[cfg(feature = "chaquopy")]
    fn poll_recovery(&mut self) {
        let Some((input, mode, updates)) = lifecycle::take_recovery_updates() else {
            return;
        };
        for update in updates {
            self.last_resolved_at = None;
            self.prefetch = prefetch::Prefetcher::default();
            self.stream_token = None;
            match update {
                recovery::Update::Started(notice) => {
                    self.resolution_started = Some(std::time::Instant::now());
                    self.process_message(notice.clone());
                    self.cookie_notice = Some((notice.clone(), false));
                    self.status = notice;
                }
                recovery::Update::Loaded(media) => {
                    self.last_resolved_at = Some(std::time::Instant::now());
                    self.last_media = Some((input.clone(), mode, media));
                    self.seek_position = None;
                }
                recovery::Update::Extended(media) => {
                    if !media.playlist_pending {
                        self.last_resolved_at = Some(std::time::Instant::now());
                    }
                    self.process_message(media.playlist_notice.clone());
                    self.last_media = Some((input.clone(), mode, media));
                }
                recovery::Update::QueueFailed(error) => {
                    self.process_message(error.clone());
                    self.resolve_error = Some((input.clone(), error));
                }
                recovery::Update::Failed(error) => {
                    self.resolve_error = Some((input.clone(), error));
                }
                recovery::Update::Cancelled => {}
            }
        }
    }

    /// Resuelve YouTube, X/Twitter, Instagram o Facebook en segundo plano o acepta una URL directa.
    fn resolve_and_play(&mut self) {
        self.update_session_choice();
        if self.url.trim().is_empty() {
            self.status = "Introduce una URL".to_owned();
            return;
        }
        let input = self.url.trim().to_owned();
        self.cancelled_input = None;
        self.resolution_started = Some(std::time::Instant::now());
        self.process_message("Encolando resolucion del enlace...".to_owned());
        #[cfg(feature = "chaquopy")]
        {
            lifecycle::invalidate_recovery();
        }
        self.resolve_error = None;
        if self.mode == PlaybackMode::Video {
            if let Some(error) = &self.video_error {
                self.status = format!("Video no disponible: {error}");
                return;
            }
        }
        let audio_only = self.mode == PlaybackMode::Audio;
        if let Some((source, mode, media)) = &self.last_media {
            if source == &input && *mode == self.mode && self.last_session == self.youtube_session {
                if self.playback.lock().expect("playback lock").error.is_none()
                    && self
                        .last_resolved_at
                        .is_some_and(|time| time.elapsed() < std::time::Duration::from_secs(300))
                {
                    let media = media.clone();
                    self.load_resolved_stream(media);
                    return;
                }
            }
        }
        if is_http_url(&input) && !needs_resolver(&input) {
            self.stream_receiver = None;
            self.last_resolved_at = Some(std::time::Instant::now());
            self.load_resolved_stream(stream_url::MediaInfo::direct(input));
            return;
        }

        #[cfg(feature = "chaquopy")]
        {
            let key = prefetch::Key {
                input: input.clone(),
                audio_only,
                session: self.youtube_session,
            };
            match self.prefetch.take(&key, std::time::Instant::now()) {
                prefetch::Taken::Ready(media, resolved_at, receiver, _token) => {
                    log::info!("Play usa el enlace precargado");
                    self.stream_receiver = None;
                    let pending = media.playlist_pending;
                    self.last_resolved_at = (!pending).then_some(resolved_at);
                    self.load_resolved_stream(media);
                    if pending {
                        lifecycle::continue_queue(receiver);
                    }
                    return;
                }
                prefetch::Taken::Pending(receiver, token) => {
                    log::info!("Play espera la precarga en curso");
                    self.stream_receiver = Some(receiver);
                    self.stream_token = Some(token);
                    self.status = "Resolviendo con yt-dlp (Chaquopy)...".to_owned();
                    return;
                }
                prefetch::Taken::None => {}
            }
            match chaquopy::extract_stream_url(&input, audio_only, self.youtube_session) {
                Ok(receiver) => {
                    self.stream_receiver = Some(receiver);
                    self.status = "Resolviendo con yt-dlp (Chaquopy)...".to_owned();
                }
                Err(error) => {
                    log::error!("No se pudo iniciar Chaquopy: {error}");
                    self.resolve_error = Some((input.clone(), user_facing_error(&error)));
                    self.status = format!(
                        "No se pudo iniciar la resolucion: {}",
                        user_facing_error(&error)
                    );
                    self.process_message(self.status.clone());
                }
            }
        }
        #[cfg(feature = "yt-dlp-subprocess")]
        {
            self.start_subprocess_resolution(&input, audio_only);
        }
        #[cfg(feature = "native-python")]
        {
            self.start_native_python_resolution(&input);
        }
        #[cfg(not(any(
            feature = "chaquopy",
            feature = "native-python",
            feature = "yt-dlp-subprocess"
        )))]
        {
            let (sender, receiver) = mpsc::channel();
            self.stream_receiver = Some(receiver);
            self.status = "Resolviendo enlace...".to_owned();
            std::thread::spawn(move || {
                log::info!("Iniciando hilo de resolución de URL...");
                let result = resolve_stream_url(&input, audio_only);
                let _ = sender.send(result.map(stream_url::MediaInfo::direct));
            });
        }
    }

    #[cfg(feature = "yt-dlp-subprocess")]
    fn start_subprocess_resolution(&mut self, input: &str, audio_only: bool) {
        let input = input.to_owned();
        let android_app = self.android_app.clone();
        let (sender, receiver) = mpsc::channel();
        self.stream_receiver = Some(receiver);
        self.status = "Resolviendo con yt-dlp (subproceso)...".to_owned();
        std::thread::spawn(move || {
            let result = (|| {
                let url = if media_site(&input) == Some(MediaSite::YouTube)
                    && !is_youtube_playlist_url(&input)
                {
                    canonical_youtube_url(&input)?.1
                } else {
                    input.clone()
                };
                let files_dir = android_app
                    .internal_data_path()
                    .ok_or_else(|| "Android no proporciono filesDir".to_owned())?;
                let native_dir = android_native_library_dir()?;
                let manager = android_app.asset_manager();
                let bundle = yt_dlp_bundle::prepare(&files_dir, &native_dir, |name| {
                    use std::io::Read;
                    let name = CString::new(name).map_err(|error| error.to_string())?;
                    let mut asset = manager
                        .open(&name)
                        .ok_or_else(|| format!("Asset ausente: {}", name.to_string_lossy()))?;
                    let mut bytes = Vec::new();
                    asset
                        .read_to_end(&mut bytes)
                        .map_err(|error| error.to_string())?;
                    Ok(bytes)
                })?;
                yt_dlp_process::resolve(
                    &bundle.executable,
                    bundle.script.as_deref(),
                    &bundle.python_home,
                    &url,
                    audio_only,
                )
            })();
            if let Err(error) = &result {
                log::error!("Resolver yt-dlp: {error}");
            }
            if sender
                .send(result.and_then(stream_url::MediaInfo::from_urls))
                .is_err()
            {
                log::info!("La interfaz ya no espera el resultado de yt-dlp");
            }
        });
    }

    /// Ejecuta yt-dlp mediante la C-API de Python en un hilo dedicado.
    #[cfg(feature = "native-python")]
    fn start_native_python_resolution(&mut self, input: &str) {
        let input = input.to_owned();
        let (sender, receiver) = mpsc::channel();
        self.stream_receiver = Some(receiver);
        self.status = "Resolviendo con yt-dlp nativo...".to_owned();
        std::thread::spawn(move || {
            let result = python_ffi::initialize("/data/data/com.visualrustmpv.app/files/python")
                .and_then(|_| python_ffi::resolve(&input));
            if let Ok(ref stream_url) = result {
                log::info!(
                    "URL resuelta exitosamente con yt-dlp nativo: {}",
                    stream_url
                );
            }
            let _ = sender.send(result.map(stream_url::MediaInfo::direct));
        });
    }

    /// Refleja en el estado inicial cuando el worker Python termina la precarga.
    #[cfg(feature = "chaquopy")]
    fn poll_python_worker(&mut self) {
        for message in chaquopy::take_progress() {
            if self.process_pending() {
                self.status = message.clone();
            }
            self.process_message(message);
        }
        if self.stream_receiver.is_none() {
            if let Some(notice) = chaquopy::take_notice() {
                self.status = notice;
            }
        }
        let state = chaquopy::warmup();
        if state == self.python_warmup {
            return;
        }
        let idle =
            self.stream_receiver.is_none() && !self.playback.lock().expect("playback lock").active;
        let initial = self.status.starts_with("Android listo");
        match &state {
            Some(chaquopy::Warmup::Pending) if initial => {
                self.status = "Android listo · precargando yt-dlp...".to_owned();
            }
            Some(chaquopy::Warmup::Ready(time, version)) if initial => {
                self.status = format!(
                    "Android listo · yt-dlp {version} precargado en {:.1} s",
                    time.as_secs_f64()
                );
            }
            Some(chaquopy::Warmup::Failed(error)) if idle => {
                self.status = format!("Precarga de yt-dlp fallida: {error}");
            }
            _ => {}
        }
        self.python_warmup = state;
    }

    /// Resuelve YouTube mientras el usuario aun no pulsa Play.
    #[cfg(feature = "chaquopy")]
    fn poll_prefetch(&mut self) {
        if lifecycle::queue_pending() {
            return;
        }
        if !self.resolving() {
            self.stream_token = None;
        } else {
            // Play ya esta esperando; la URL y el modo no pueden cambiar.
            return;
        }
        let now = std::time::Instant::now();
        let input = self.url.trim();
        let audio_only = self.mode == PlaybackMode::Audio;
        let already_loaded = self
            .last_media
            .as_ref()
            .is_some_and(|(source, mode, _)| source == input && *mode == self.mode)
            && self.last_session == self.youtube_session
            && (self.playback.lock().expect("playback lock").stopped
                || self
                    .last_resolved_at
                    .is_some_and(|time| time.elapsed() < prefetch::TTL));
        let blocked = self
            .resolve_error
            .as_ref()
            .is_some_and(|(source, _)| source == input)
            || self.cancelled_input.as_deref() == Some(input);
        let eligible = needs_resolver(input)
            && !already_loaded
            && !blocked
            && (audio_only || self.video_error.is_none());
        let key = eligible.then(|| prefetch::Key {
            input: input.to_owned(),
            audio_only,
            session: self.youtube_session,
        });
        if let Some(key) = self.prefetch.observe(key, now) {
            self.resolution_started = Some(now);
            self.process_message(
                "Precargando el enlace; puedes pulsar Play o cancelar.".to_owned(),
            );
            log::info!("Precargando enlace antes de Play");
            match chaquopy::prefetch(&key.input, key.audio_only, key.session) {
                Ok((receiver, token)) => self.prefetch.start(key, receiver, token),
                Err(error) => {
                    log::error!("No se pudo encolar la precarga: {error}");
                    self.prefetch.fail(key);
                }
            }
        }
        match self.prefetch.poll(now) {
            Some(Ok(())) => {
                log::info!("Enlace precargado; Play sera inmediato");
                let idle = !self.playback.lock().expect("playback lock").active;
                if idle {
                    self.status = "Enlace listo · pulsa Reproducir".to_owned();
                }
            }
            Some(Err(error)) => {
                log::error!("Precarga fallida (Play reintentara): {error}");
                self.resolve_error = Some((self.url.trim().to_owned(), user_facing_error(&error)));
                self.status = format!(
                    "No se pudo resolver la precarga: {}",
                    user_facing_error(&error)
                );
                self.process_message(self.status.clone());
            }
            None => {}
        }
    }

    /// Consume el resultado del worker y carga el stream en libmpv.
    fn poll_stream(&mut self) {
        let Some(receiver) = &self.stream_receiver else {
            return;
        };
        let result = receiver.try_recv();
        match result {
            Ok(Ok(media)) => {
                #[cfg(feature = "chaquopy")]
                let continuation = media
                    .playlist_pending
                    .then(|| self.stream_receiver.take().expect("stream receiver"));
                self.stream_receiver = None;
                self.last_resolved_at = (!media.playlist_pending).then(std::time::Instant::now);
                self.load_resolved_stream(media);
                #[cfg(feature = "chaquopy")]
                if let Some(receiver) = continuation {
                    lifecycle::continue_queue(receiver);
                }
            }
            Ok(Err(error)) => {
                log::error!("No se pudo resolver el enlace: {error}");
                self.stream_receiver = None;
                self.status = format!("No se pudo resolver el enlace: {error}");
                self.resolve_error = Some((self.url.trim().to_owned(), user_facing_error(&error)));
                self.process_message(format!(
                    "No se pudo resolver el enlace: {}",
                    user_facing_error(&error)
                ));
            }
            Err(mpsc::TryRecvError::Empty) => {}
            Err(mpsc::TryRecvError::Disconnected) => {
                self.stream_receiver = None;
                self.status = "yt-dlp terminó inesperadamente".to_owned();
                log::error!("{}", self.status);
                self.resolve_error = Some((self.url.trim().to_owned(), self.status.clone()));
                self.process_message(self.status.clone());
            }
        }
    }
}

/// Respuesta de una instancia Piped para el mismo video.
#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
#[derive(Debug, Deserialize)]
struct PipedStreams {
    #[serde(rename = "audioStreams", default)]
    audio_streams: Vec<PipedAudioStream>,
    #[serde(rename = "hls", default)]
    hls: String,
    #[serde(rename = "streamList", default)]
    stream_list: Vec<PipedVideoStream>,
}

/// Stream de audio publicado por Piped.
#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
#[derive(Debug, Deserialize)]
struct PipedAudioStream {
    url: String,
    #[serde(default)]
    bitrate: Option<u64>,
}

/// Stream de video publicado por Piped.
#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
#[derive(Debug, Deserialize)]
struct PipedVideoStream {
    #[serde(default)]
    url: Option<String>,
}

/// Respuesta de una instancia Invidious.
#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
#[derive(Debug, Deserialize)]
struct InvidiousStreams {
    #[serde(rename = "formatStreams", default)]
    format_streams: Vec<InvidiousStream>,
    #[serde(rename = "adaptiveFormats", default)]
    adaptive_formats: Vec<InvidiousStream>,
    #[serde(rename = "hlsUrl", default)]
    hls_url: String,
}

/// Formato directo de audio o video publicado por Invidious.
#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
#[derive(Debug, Deserialize)]
struct InvidiousStream {
    url: String,
    #[serde(default)]
    mime: String,
    #[serde(default)]
    container: String,
    #[serde(rename = "qualityLabel", default)]
    quality_label: String,
    #[serde(default)]
    height: Option<u32>,
}

#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
const RESOLVER_USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64)";

/// Extrae el ID de un enlace YouTube estándar o youtu.be.
#[cfg(all(
    target_os = "android",
    not(any(feature = "chaquopy", feature = "native-python"))
))]
fn youtube_video_id(input: &str) -> Option<String> {
    let url = reqwest::Url::parse(input).ok()?;
    let host = url.host_str()?.to_ascii_lowercase();
    if host == "youtu.be" {
        return url.path_segments()?.next().map(str::to_owned);
    }
    if host.ends_with("youtube.com") || host.ends_with("youtube-nocookie.com") {
        if url.path() == "/watch" {
            return url
                .query_pairs()
                .find(|(key, _)| key == "v")
                .map(|(_, value)| value.into_owned());
        }
        return url
            .path_segments()?
            .skip(1)
            .find(|segment| segment.len() == 11)
            .map(str::to_owned);
    }
    None
}

/// Convierte cualquier enlace YouTube admitido a la URL canónica de watch.
#[cfg(all(
    target_os = "android",
    not(any(feature = "chaquopy", feature = "native-python"))
))]
fn canonical_youtube_url(input: &str) -> Result<(String, String), String> {
    let video_id = youtube_video_id(input).ok_or_else(|| "La URL no es YouTube".to_owned())?;
    if video_id.len() != 11 {
        return Err("El ID de YouTube no es válido".to_owned());
    }
    Ok((
        video_id.clone(),
        format!("https://www.youtube.com/watch?v={video_id}"),
    ))
}

/// Resuelve YouTube rotando Invidious y Piped mediante GET.
#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
fn resolve_stream_url(input: &str, audio_only: bool) -> Result<String, String> {
    let (video_id, _) = canonical_youtube_url(input)?;
    let client = reqwest::blocking::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .connect_timeout(std::time::Duration::from_secs(5))
        .user_agent(RESOLVER_USER_AGENT)
        .build()
        .map_err(|error| error.to_string())?;
    log::info!("Cliente HTTP blocking construido para resolución de {video_id}");
    let mut failures = Vec::new();
    let endpoints = [
        (
            "Invidious",
            format!("https://inv.nadeko.net/api/v1/videos/{video_id}"),
        ),
        (
            "Invidious",
            format!("https://invidious.nerdvpn.de/api/v1/videos/{video_id}"),
        ),
        (
            "Piped",
            format!("https://pipedapi.privacy.com.de/streams/{video_id}"),
        ),
    ];
    for (provider, endpoint) in endpoints {
        match client.get(&endpoint).send() {
            Ok(response) if response.status().as_u16() == 200 => {
                log::info!("Respuesta de {provider} {endpoint}: status = 200");
                let stream = if provider == "Invidious" {
                    match response.json::<InvidiousStreams>() {
                        Ok(streams) => select_invidious_stream(streams, audio_only),
                        Err(error) => {
                            log::warn!("JSON inválido de Invidious {endpoint}: {error}");
                            None
                        }
                    }
                } else {
                    match response.json::<PipedStreams>() {
                        Ok(streams) => select_piped_stream(streams, audio_only),
                        Err(error) => {
                            log::warn!("JSON inválido de Piped {endpoint}: {error}");
                            None
                        }
                    }
                };
                if let Some(url) = stream {
                    log::info!("URL resuelta con éxito: {}", url);
                    return Ok(url);
                }
                failures.push(format!("{provider} {endpoint}: sin stream compatible"));
            }
            Ok(response) => {
                log::warn!(
                    "Respuesta HTTP de {provider} {endpoint}: {}",
                    response.status()
                );
                failures.push(format!("{provider} HTTP {}", response.status()));
            }
            Err(error) => {
                log::warn!("Error de red/DNS/TLS en {provider} {endpoint}: {error}");
                failures.push(format!("{provider}: {error}"));
            }
        }
    }
    Err(format!(
        "No se pudo resolver el stream ({})",
        failures.join("; ")
    ))
}

/// Elige un único stream Piped sin depender de FFmpeg.
#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
fn select_piped_stream(streams: PipedStreams, audio_only: bool) -> Option<String> {
    if audio_only {
        streams
            .audio_streams
            .into_iter()
            .filter(|s| !s.url.is_empty())
            .max_by_key(|s| s.bitrate.unwrap_or(0))
            .map(|s| s.url)
    } else if is_http_url(&streams.hls) {
        Some(streams.hls)
    } else {
        streams
            .stream_list
            .into_iter()
            .find_map(|stream| stream.url)
    }
}

/// Elige el mejor formato directo de Invidious sin depender de FFmpeg.
#[cfg(all(
    target_os = "android",
    not(any(
        feature = "chaquopy",
        feature = "native-python",
        feature = "yt-dlp-subprocess"
    ))
))]
fn select_invidious_stream(streams: InvidiousStreams, audio_only: bool) -> Option<String> {
    let hls_url = streams.hls_url;
    let mut formats = streams.format_streams;
    formats.extend(streams.adaptive_formats);
    formats
        .into_iter()
        .filter(|stream| {
            if stream.url.is_empty() {
                return false;
            }
            if audio_only {
                stream.mime.starts_with("audio/") || stream.container == "m4a"
            } else {
                stream.mime.starts_with("video/")
                    && stream.container == "mp4"
                    && stream.height.unwrap_or(0) <= 720
            }
        })
        .max_by_key(|stream| {
            stream.height.unwrap_or_else(|| {
                stream
                    .quality_label
                    .trim_end_matches('p')
                    .parse::<u32>()
                    .unwrap_or(0)
            })
        })
        .map(|stream| stream.url)
        .or_else(|| (!audio_only && is_http_url(&hls_url)).then_some(hls_url))
}

/// Comprueba que la entrada sea una URL HTTP(S).
#[cfg(target_os = "android")]
fn is_http_url(input: &str) -> bool {
    input.starts_with("http://") || input.starts_with("https://")
}

/// Quita los prefijos del puente Java/Python para mostrar solo la causa.
#[cfg(any(test, target_os = "android"))]
fn user_facing_error(error: &str) -> String {
    let reason = error
        .rsplit_once("RuntimeError: ")
        .map_or(error, |(_, reason)| reason);
    format!("No se pudo reproducir: {}", reason.trim())
}

/// Sitios cuyas paginas deben pasar por yt-dlp antes de llegar a libmpv.
#[cfg(any(test, target_os = "android"))]
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum MediaSite {
    YouTube,
    Twitter,
    Instagram,
    Facebook,
}

#[cfg(any(test, target_os = "android"))]
fn media_site(input: &str) -> Option<MediaSite> {
    let url = url::Url::parse(input).ok()?;
    if !matches!(url.scheme(), "http" | "https") {
        return None;
    }
    let host = url.host_str()?.to_ascii_lowercase();
    let host = host.trim_end_matches('.');
    let on = |domain: &str| {
        host == domain
            || host
                .strip_suffix(domain)
                .is_some_and(|prefix| prefix.ends_with('.'))
    };
    if ["youtu.be", "youtube.com", "youtube-nocookie.com"]
        .into_iter()
        .any(on)
    {
        Some(MediaSite::YouTube)
    } else if ["twitter.com", "x.com"].into_iter().any(on) {
        Some(MediaSite::Twitter)
    } else if ["instagram.com", "instagr.am"].into_iter().any(on) {
        Some(MediaSite::Instagram)
    } else if ["facebook.com", "fb.watch", "fb.com"].into_iter().any(on) {
        Some(MediaSite::Facebook)
    } else {
        None
    }
}

/// Limita yt-dlp a los sitios compatibles; el resto se entrega directo a libmpv.
#[cfg(target_os = "android")]
fn needs_resolver(input: &str) -> bool {
    media_site(input).is_some()
}

#[cfg(any(test, all(target_os = "android", feature = "yt-dlp-subprocess")))]
fn is_youtube_playlist_url(input: &str) -> bool {
    youtube::collection(input)
}

/// Inicializa la instancia nativa de libmpv enlazada en el APK.
#[cfg(target_os = "android")]
fn initialize_mpv(mode: PlaybackMode) -> Result<usize, String> {
    let context = unsafe { mpv_create() };
    if context.is_null() {
        return Err("No se pudo crear libmpv".to_owned());
    }
    let result = (|| {
        #[cfg(feature = "chaquopy")]
        {
            let path = chaquopy::ca_bundle_path()?;
            if !std::path::Path::new(&path).is_file() {
                return Err("El bundle CA de Chaquopy no existe".to_owned());
            }
            let path = CString::new(path).map_err(|error| error.to_string())?;
            let result =
                unsafe { mpv_set_option_string(context, c"tls-ca-file".as_ptr(), path.as_ptr()) };
            if result < 0 {
                return Err(format!("Configurar CA: {}", playback::error_text(result)));
            }
            log::info!("libmpv: bundle CA configurado; verificacion TLS activada");
        }
        for (name, value) in [
            // OpenSL ES does not require registering a JVM with libmpv/FFmpeg.
            (c"ao", c"opensles"),
            (c"vo", c"libmpv"),
            // Software decoding avoids Android MediaCodec's separate JVM/surface requirements.
            (c"hwdec", c"no"),
            (c"gpu-hwdec-interop", c"no"),
            (c"osd-level", c"0"),
            (c"osd-bar", c"no"),
            (
                c"vid",
                if mode == PlaybackMode::Audio {
                    c"no"
                } else {
                    c"auto"
                },
            ),
            (c"ytdl", c"no"),
            (c"network-timeout", c"15"),
            (c"tls-verify", c"yes"),
        ] {
            let result = unsafe { mpv_set_option_string(context, name.as_ptr(), value.as_ptr()) };
            if result < 0 {
                return Err(format!(
                    "Opcion {}: {}",
                    name.to_string_lossy(),
                    playback::error_text(result)
                ));
            }
        }
        let result = unsafe { mpv_initialize(context) };
        if result < 0 {
            return Err(format!(
                "Inicializar libmpv: {}",
                playback::error_text(result)
            ));
        }
        playback::subscribe(context as usize)?;
        Ok(context as usize)
    })();
    if result.is_err() {
        unsafe { mpv_terminate_destroy(context) };
    }
    result
}

/// Lee el primer elemento de texto del portapapeles usando el Context Android.
#[cfg(target_os = "android")]
fn read_android_clipboard() -> Result<String, String> {
    let android = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android.vm() as *mut _) };
    vm.attach_current_thread(|env| {
        let context = unsafe { JObject::from_raw(env, android.context() as *mut _) };
        let service = JObject::from(env.new_string("clipboard")?);
        let manager = env
            .call_method(
                &context,
                jni_str!("getSystemService"),
                jni_sig!("(Ljava/lang/String;)Ljava/lang/Object;"),
                &[JValue::Object(&service)],
            )?
            .l()?;
        if manager.is_null() {
            return Ok::<_, jni::errors::Error>(String::new());
        }
        if !env
            .call_method(&manager, jni_str!("hasPrimaryClip"), jni_sig!("()Z"), &[])?
            .z()?
        {
            return Ok(String::new());
        }
        let clip = env
            .call_method(
                &manager,
                jni_str!("getPrimaryClip"),
                jni_sig!("()Landroid/content/ClipData;"),
                &[],
            )?
            .l()?;
        let item = env
            .call_method(
                &clip,
                jni_str!("getItemAt"),
                jni_sig!("(I)Landroid/content/ClipData$Item;"),
                &[JValue::Int(0)],
            )?
            .l()?;
        let text = env
            .call_method(
                &item,
                jni_str!("coerceToText"),
                jni_sig!("(Landroid/content/Context;)Ljava/lang/CharSequence;"),
                &[JValue::Object(&context)],
            )?
            .l()?;
        if text.is_null() {
            return Ok(String::new());
        }
        let value = env
            .call_method(
                &text,
                jni_str!("toString"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        let value = unsafe { JString::from_raw(env, value.into_raw()) };
        Ok(value.to_string())
    })
    .map_err(|error| error.to_string())
}

/// Obtiene la ruta instalada por PackageManager, sin asumir /data/app.
#[cfg(all(target_os = "android", feature = "yt-dlp-subprocess"))]
fn android_native_library_dir() -> Result<std::path::PathBuf, String> {
    let android = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android.vm() as *mut _) };
    vm.attach_current_thread(|env| {
        let context = unsafe { JObject::from_raw(env, android.context() as *mut _) };
        let info = env
            .call_method(
                &context,
                jni_str!("getApplicationInfo"),
                jni_sig!("()Landroid/content/pm/ApplicationInfo;"),
                &[],
            )?
            .l()?;
        let directory = env
            .get_field(
                &info,
                jni_str!("nativeLibraryDir"),
                jni_sig!("Ljava/lang/String;"),
            )?
            .l()?;
        let directory = unsafe { JString::from_raw(env, directory.into_raw()) };
        Ok::<_, jni::errors::Error>(std::path::PathBuf::from(directory.to_string()))
    })
    .map_err(|error| format!("Obtener nativeLibraryDir: {error}"))
}

/// Envía la URL directa a libmpv usando el comando loadfile.
#[cfg(target_os = "android")]
fn load_stream(
    context: usize,
    stream_url: &str,
    mode: PlaybackMode,
    start: Option<f64>,
    paused: bool,
) -> Result<(), String> {
    let stream_url = stream_url::validate_stream_url(stream_url)?;
    let mut options = if mode == PlaybackMode::Video {
        "vid=auto".to_owned()
    } else {
        "vid=no".to_owned()
    };
    if let Some(start) = start {
        options.push_str(&format!(",start={start:.3}"));
    }
    send_mpv_command(
        context,
        &["set", "pause", if paused { "yes" } else { "no" }],
    )?;
    send_mpv_command(
        context,
        &["loadfile", &stream_url, "replace", "-1", &options],
    )?;
    log::info!(
        "URL directa enviada a libmpv mediante loadfile ({mode:?}, inicio {})",
        start.map_or("0".to_owned(), |start| format!("{start:.1} s"))
    );
    Ok(())
}

#[cfg(target_os = "android")]
fn load_media(
    context: usize,
    media: &stream_url::MediaInfo,
    mode: PlaybackMode,
    start: Option<f64>,
    paused: bool,
) -> Result<(), String> {
    load_stream(context, &media.url, mode, start, paused)?;
    for url in &media.playlist {
        append_stream(context, url, mode)?;
    }
    Ok(())
}

#[cfg(target_os = "android")]
fn send_mpv_command(context: usize, args: &[&str]) -> Result<(), String> {
    if context == 0 {
        return Err("libmpv no está inicializado".to_owned());
    }
    let values = args
        .iter()
        .map(|arg| CString::new(*arg))
        .collect::<Result<Vec<_>, _>>()
        .map_err(|error| error.to_string())?;
    let mut pointers: Vec<_> = values.iter().map(|arg| arg.as_ptr()).collect();
    pointers.push(std::ptr::null());
    // The render thread must never wait for mpv's core.
    let result = unsafe { mpv_command_async(context as *mut _, 0, pointers.as_ptr()) };
    if result < 0 {
        Err(format!(
            "libmpv rechazo el comando: {}",
            playback::error_text(result)
        ))
    } else {
        Ok(())
    }
}

/// Libera la instancia nativa al cerrar la aplicación Android.
#[cfg(target_os = "android")]
impl Drop for AndroidAppState {
    fn drop(&mut self) {
        lifecycle::register(0);
        if self.mpv_context != 0 {
            unsafe { mpv_terminate_destroy(self.mpv_context as *mut _) };
        }
    }
}

/// Entry point NativeActivity que cargo-apk conecta con Android.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    android_logger::init_once(
        android_logger::Config::default().with_max_level(log::LevelFilter::Info),
    );
    log::info!("VisualRustMPV Android iniciado");
    let mut options = eframe::NativeOptions::default();
    options.renderer = eframe::Renderer::Glow;
    let app_handle = android_app.clone();
    options.event_loop_builder = Some(Box::new(move |builder| {
        builder.with_android_app(android_app);
    }));
    let _ = eframe::run_native(
        "VisualRustMPV",
        options,
        Box::new(move |creation| {
            Ok(Box::new(AndroidAppState::new(
                app_handle,
                creation.gl.as_deref(),
            )))
        }),
    );
}

/// Símbolo alternativo para que el paquete pueda inspeccionarse en desktop.
#[cfg(not(target_os = "android"))]
pub fn desktop_placeholder() -> &'static str {
    "app_android requiere target aarch64-linux-android"
}

#[cfg(test)]
mod layout_tests {
    use super::{
        MediaSite, STARTUP_SPLASH_MAXIMUM, STARTUP_SPLASH_MINIMUM, draw_startup_splash,
        is_youtube_playlist_url, media_site, startup_splash_visible, system_bar_margins,
        user_facing_error,
    };
    use eframe::egui;

    #[test]
    fn shows_only_the_python_error_reason() {
        assert_eq!(
            user_facing_error(
                "Chaquopy/yt-dlp: com.chaquo.python.PyException: RuntimeError: \
                 La publicacion no es publica"
            ),
            "No se pudo reproducir: La publicacion no es publica"
        );
        assert_eq!(
            user_facing_error("sin red"),
            "No se pudo reproducir: sin red"
        );
    }

    #[test]
    fn reserves_system_bars_in_logical_points() {
        let margins = system_bar_margins([0, 48, 720, 1504], [720, 1600], 2.0);
        assert_eq!(margins.top, 24.0);
        assert_eq!(margins.bottom, 48.0);
        assert_eq!(margins.left, 0.0);
        assert_eq!(margins.right, 0.0);
    }

    #[test]
    fn splash_content_is_centered_inside_system_bars() {
        for (window, content, scale) in [
            ([1080, 2340], [0, 90, 1080, 2208], 3.0),
            ([2340, 1080], [90, 0, 2208, 1020], 3.0),
            ([720, 1600], [0, 48, 720, 1504], 2.0),
        ] {
            let context = egui::Context::default();
            let screen = egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(window[0] as f32 / scale, window[1] as f32 / scale),
            );
            let margins = system_bar_margins(content, window, scale);
            let safe_area = egui::Rect::from_min_max(
                screen.min + egui::vec2(margins.left, margins.top),
                screen.max - egui::vec2(margins.right, margins.bottom),
            );
            let _ = context.run(
                egui::RawInput {
                    screen_rect: Some(screen),
                    ..Default::default()
                },
                |context| {
                    let mut frame = egui::Frame::central_panel(&context.style());
                    frame.inner_margin.left += margins.left;
                    frame.inner_margin.top += margins.top;
                    frame.inner_margin.right += margins.right;
                    frame.inner_margin.bottom += margins.bottom;
                    let block = draw_startup_splash(context, frame);
                    assert!(safe_area.contains_rect(block), "{block:?} in {safe_area:?}");
                    assert!((block.center().x - safe_area.center().x).abs() < 1.0);
                    assert!((block.center().y - safe_area.center().y).abs() < 30.0);
                },
            );
        }
    }

    #[test]
    fn already_fitted_window_has_no_extra_margin() {
        assert_eq!(
            system_bar_margins([0, 0, 720, 1456], [720, 1456], 2.0),
            egui::Margin::default()
        );
    }

    #[test]
    fn handles_side_insets_and_density_changes() {
        let content = [90, 0, 1440, 680];
        let window = [1600, 720];
        let margins = system_bar_margins(content, window, 2.0);
        assert_eq!(margins.left, 45.0);
        assert_eq!(margins.right, 80.0);
        assert_eq!(margins.bottom, 20.0);
        assert_eq!(system_bar_margins(content, window, 1.0).left, 90.0);
    }

    #[test]
    fn tolerates_initial_or_stale_content_rectangles() {
        assert_eq!(
            system_bar_margins([0; 4], [720, 1600], 2.0),
            egui::Margin::default()
        );
        assert_eq!(
            system_bar_margins([-10, -20, 800, 1700], [720, 1600], 2.0),
            egui::Margin::default()
        );
    }

    #[test]
    fn detects_only_youtube_playlist_urls() {
        assert!(is_youtube_playlist_url(
            "https://www.youtube.com/playlist?list=PL123"
        ));
        assert!(is_youtube_playlist_url(
            "https://youtu.be/abcdefghijk?list=PL123"
        ));
        assert!(!is_youtube_playlist_url(
            "https://youtube.com/watch?v=abcdefghijk"
        ));
        assert!(!is_youtube_playlist_url(
            "https://youtube.com.evil.example/playlist?list=PL123"
        ));
        assert!(!is_youtube_playlist_url(
            "https://example.com/playlist?list=PL123"
        ));
    }

    #[test]
    fn routes_social_video_sites_to_yt_dlp() {
        for (url, site) in [
            ("https://youtu.be/abcdefghijk", MediaSite::YouTube),
            (
                "https://m.youtube.com/watch?v=abcdefghijk",
                MediaSite::YouTube,
            ),
            ("https://x.com/user/status/1", MediaSite::Twitter),
            (
                "https://mobile.twitter.com/user/status/1",
                MediaSite::Twitter,
            ),
            ("https://www.instagram.com/reel/abc/", MediaSite::Instagram),
            ("https://instagr.am/p/abc/", MediaSite::Instagram),
            (
                "https://www.facebook.com/user/videos/1/",
                MediaSite::Facebook,
            ),
            ("https://m.facebook.com/watch/?v=1", MediaSite::Facebook),
            ("https://fb.watch/abc/", MediaSite::Facebook),
            ("HTTPS://WWW.X.COM./user/status/1", MediaSite::Twitter),
        ] {
            assert_eq!(media_site(url), Some(site), "{url}");
        }
    }

    #[test]
    fn startup_splash_waits_for_warmup_but_has_a_bounded_duration() {
        let started = std::time::Instant::now();
        assert!(startup_splash_visible(
            started,
            false,
            started + STARTUP_SPLASH_MINIMUM / 2
        ));
        assert!(startup_splash_visible(
            started,
            true,
            started + STARTUP_SPLASH_MINIMUM
        ));
        assert!(!startup_splash_visible(
            started,
            false,
            started + STARTUP_SPLASH_MINIMUM
        ));
        assert!(!startup_splash_visible(
            started,
            true,
            started + STARTUP_SPLASH_MAXIMUM
        ));
    }

    #[test]
    fn leaves_other_urls_for_direct_playback() {
        for url in [
            "https://example.com/video.mp4",
            "https://notx.com/user/status/1",
            "https://facebook.com.evil.example/videos/1",
            "https://evilinstagram.com/reel/abc/",
            "ftp://x.com/video.mp4",
            "x.com/user/status/1",
        ] {
            assert_eq!(media_site(url), None, "{url}");
        }
    }
}
