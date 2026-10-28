//! Estado y presentación de la ventana principal.

use crate::downloads::{self, DownloadTools};
use crate::mpv::{self, PlaybackInfo, PlaybackMode};
use crate::playlist::{self, PlaylistEntry};
use eframe::egui;
use std::collections::VecDeque;
use std::path::PathBuf;
use std::process::{Child, ExitStatus};
use std::sync::mpsc::{self, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Estado completo de la aplicación GUI.
pub struct MpvApp {
    /// URL o archivo que se está editando.
    url: String,
    /// Modo seleccionado para la próxima reproducción.
    mode: PlaybackMode,
    /// Texto de estado mostrado al usuario.
    status: String,
    /// Ruta resuelta de MPV.
    mpv_path: Option<PathBuf>,
    /// Resultado asíncrono de la inicialización de MPV.
    startup: Receiver<Result<PathBuf, String>>,
    /// Resultado asíncrono de la inicialización de yt-dlp y ffmpeg.
    tools_startup: Receiver<Result<DownloadTools, String>>,
    /// Ruta resuelta de yt-dlp y ffmpeg.
    tools: Option<DownloadTools>,
    /// Canal para recibir líneas de procesos externos.
    log_sender: mpsc::Sender<String>,
    /// Canal de entrada de líneas de log.
    log_receiver: Receiver<String>,
    /// Últimas líneas mostradas en la interfaz.
    logs: VecDeque<String>,
    /// Historial persistente de URLs.
    playlist: Vec<PlaylistEntry>,
    /// Metadatos consultados al reproductor.
    playback_info: PlaybackInfo,
    /// Momento de la última consulta IPC de metadatos.
    last_info_poll: Instant,
    /// Resultado de la descarga activa.
    download_receiver: Option<Receiver<downloads::DownloadResult>>,
    /// Evita iniciar dos descargas simultáneas.
    download_active: bool,
    /// Proceso MPV compartido con el código de limpieza.
    child: Arc<Mutex<Option<Child>>>,
}

impl MpvApp {
    /// Crea el estado inicial e inicia las búsquedas pesadas en segundo plano.
    pub fn new() -> Self {
        let (sender, startup) = mpsc::channel();
        let (tools_sender, tools_startup) = mpsc::channel();
        let (log_sender, log_receiver) = mpsc::channel();
        thread::spawn(move || {
            let _ = sender.send(mpv::locate_or_install());
        });
        thread::spawn(move || {
            let _ = tools_sender.send(downloads::locate_or_install());
        });
        Self {
            url: String::new(),
            mode: PlaybackMode::Audio,
            status: "Buscando MPV...".to_owned(),
            mpv_path: None,
            startup,
            tools_startup,
            tools: None,
            log_sender,
            log_receiver,
            logs: VecDeque::new(),
            playlist: playlist::load(),
            playback_info: PlaybackInfo::default(),
            last_info_poll: Instant::now(),
            download_receiver: None,
            download_active: false,
            child: Arc::new(Mutex::new(None)),
        }
    }

    /// Procesa las líneas recibidas sin bloquear el hilo de UI.
    fn poll_logs(&mut self) {
        for line in self.log_receiver.try_iter() {
            self.logs.push_back(line);
            if self.logs.len() > 200 {
                self.logs.pop_front();
            }
        }
    }

    /// Detecta si MPV terminó y actualiza el estado visible.
    fn poll_process(&mut self) {
        let result = {
            let mut child = self.child.lock().expect("child mutex poisoned");
            child
                .as_mut()
                .and_then(|process| process.try_wait().ok())
                .flatten()
        };
        if let Some(exit_status) = result {
            self.child.lock().expect("child mutex poisoned").take();
            self.record_process_exit(exit_status);
        }
    }

    /// Convierte el resultado de una descarga en un mensaje de interfaz.
    fn poll_download(&mut self) {
        let Some(receiver) = &self.download_receiver else {
            return;
        };
        match receiver.try_recv() {
            Ok(result) => {
                self.download_active = false;
                self.download_receiver = None;
                match result {
                    Ok(message) => self.status = message,
                    Err(error) => self.status = format!("Descarga fallida: {error}"),
                }
            }
            Err(mpsc::TryRecvError::Disconnected) => {
                self.download_active = false;
                self.download_receiver = None;
                self.status = "La descarga terminó inesperadamente".to_owned();
            }
            Err(mpsc::TryRecvError::Empty) => {}
        }
    }

    /// Recoge los resultados de inicialización de MPV y de las herramientas.
    fn poll_startup(&mut self) {
        if let Ok(result) = self.startup.try_recv() {
            match result {
                Ok(path) => {
                    self.status = format!("MPV listo: {}", path.display());
                    self.mpv_path = Some(path);
                }
                Err(error) => self.status = error,
            }
        }
        if let Ok(result) = self.tools_startup.try_recv() {
            match result {
                Ok(tools) => {
                    self.tools = Some(tools);
                    self.logs.push_back("yt-dlp y ffmpeg listos".to_owned());
                }
                Err(error) => self.logs.push_back(format!("Herramientas: {error}")),
            }
        }
    }

    /// Inicia MPV con la URL y el modo seleccionados.
    fn play(&mut self) {
        let Some(path) = self.mpv_path.clone() else {
            self.status = "MPV todavía no está listo".to_owned();
            return;
        };
        if self.url.trim().is_empty() {
            self.status = "Introduce una URL o archivo".to_owned();
            return;
        }
        self.add_to_history();
        self.stop_mpv();
        self.playback_info = PlaybackInfo::default();
        self.logs.clear();
        self.logs.push_back("Iniciando MPV...".to_owned());
        match mpv::spawn(path, self.url.trim(), self.mode, self.log_sender.clone()) {
            Ok(child) => {
                *self.child.lock().expect("child mutex poisoned") = Some(child);
                self.status = "Reproduciendo".to_owned();
            }
            Err(error) => self.status = format!("No se pudo iniciar MPV: {error}"),
        }
    }

    /// Mueve una URL al principio del historial y la conserva en disco.
    fn add_to_history(&mut self) {
        let url = self.url.trim().to_owned();
        if let Some(index) = self.playlist.iter().position(|entry| entry.url == url) {
            let favorite = self.playlist[index].favorite;
            let name = self.playlist[index].name.clone();
            self.playlist.remove(index);
            self.playlist.insert(
                0,
                PlaylistEntry {
                    url,
                    favorite,
                    name,
                },
            );
        } else {
            self.playlist.insert(
                0,
                PlaylistEntry {
                    url,
                    favorite: false,
                    name: String::new(),
                },
            );
        }
        self.playlist.truncate(100);
        playlist::save(&self.playlist);
    }

    /// Guarda en el historial el nombre resuelto por MPV.
    fn update_history_name(&mut self, name: &str) {
        let url = self.url.trim();
        if let Some(entry) = self.playlist.iter_mut().find(|entry| entry.url == url) {
            if entry.name != name {
                entry.name = name.to_owned();
                playlist::save(&self.playlist);
            }
        }
    }

    /// Alterna el estado de favorito de una fila.
    fn toggle_favorite(&mut self, index: usize) {
        if let Some(entry) = self.playlist.get_mut(index) {
            entry.favorite = !entry.favorite;
            playlist::save(&self.playlist);
        }
    }

    /// Elimina una fila del historial.
    fn remove_entry(&mut self, index: usize) {
        if index < self.playlist.len() {
            self.playlist.remove(index);
            playlist::save(&self.playlist);
        }
    }

    /// Detiene MPV antes de iniciar otra reproducción o cerrar la app.
    fn stop_mpv(&self) {
        let mut child = self.child.lock().expect("child mutex poisoned");
        if let Some(mut process) = child.take() {
            let _ = process.kill();
            let _ = process.wait();
        }
    }

    /// Consulta título, duración y posición mediante IPC.
    fn poll_playback_info(&mut self) {
        if self.child.lock().expect("child mutex poisoned").is_none()
            || self.last_info_poll.elapsed() < Duration::from_millis(700)
        {
            return;
        }
        self.last_info_poll = Instant::now();
        if let Ok(title) = mpv::query_property("media-title") {
            if let Some(name) = title
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned)
            {
                self.playback_info.title = name.clone();
                self.update_history_name(&name);
            }
        }
        if let Ok(duration) = mpv::query_property("duration") {
            self.playback_info.duration = duration.as_f64();
        }
        if let Ok(position) = mpv::query_property("time-pos") {
            self.playback_info.position = position.as_f64();
        }
    }

    /// Envía pausa o reanudación a MPV.
    fn toggle_pause(&mut self) {
        self.send_control(r#"{"command":["cycle","pause"]}"#, "Pausa/Reanudar enviado");
    }

    /// Ajusta el volumen en pasos de cinco por ciento.
    fn adjust_volume(&mut self, amount: i32) {
        self.send_control(
            &format!(r#"{{"command":["add","volume",{amount}]}}"#),
            &format!("Volumen {}5%", if amount > 0 { "+" } else { "-" }),
        );
    }

    /// Avanza o retrocede cinco segundos.
    fn seek(&mut self, seconds: i32) {
        self.send_control(
            &format!(r#"{{"command":["seek",{seconds},"relative"]}}"#),
            &format!("Avance de {seconds:+} segundos"),
        );
    }

    /// Centraliza el envío de controles y su mensaje de error.
    fn send_control(&mut self, command: &str, success: &str) {
        match mpv::send_command(command) {
            Ok(()) => {
                self.status = success.to_owned();
                self.logs.push_back(success.to_owned());
            }
            Err(error) => self.status = format!("MPV no está activo: {error}"),
        }
    }

    /// Inicia una descarga en segundo plano.
    fn start_download(&mut self, audio_only: bool) {
        if self.download_active {
            self.status = "Ya hay una descarga en curso".to_owned();
            return;
        }
        let url = self.url.trim().to_owned();
        if url.is_empty() {
            self.status = "Introduce una URL para descargar".to_owned();
            return;
        }
        let Some(tools) = &self.tools else {
            self.status = "yt-dlp y ffmpeg todavía no están listos".to_owned();
            return;
        };
        self.logs.push_back(
            if audio_only {
                "Iniciando descarga MP3 a 320 kbps..."
            } else {
                "Iniciando descarga de video MP4 hasta 720p..."
            }
            .to_owned(),
        );
        match downloads::spawn_download(tools, url, audio_only, self.log_sender.clone()) {
            Ok((receiver, _)) => {
                self.download_receiver = Some(receiver);
                self.download_active = true;
                self.status = "Descargando...".to_owned();
            }
            Err(error) => self.status = format!("No se pudo iniciar yt-dlp: {error}"),
        }
    }

    /// Registra la salida del proceso MPV.
    fn record_process_exit(&mut self, status: ExitStatus) {
        let message = format!("MPV finalizó con estado: {status}");
        self.status = message.clone();
        self.logs.push_back(message);
    }

    /// Dibuja la playlist con nombres resueltos y URL como tooltip.
    fn draw_playlist(&mut self, ui: &mut egui::Ui) {
        ui.label("Lista de reproducción");
        if self.playlist.is_empty() {
            ui.weak("Todavía no hay URLs en el historial.");
            return;
        }
        egui::ScrollArea::vertical()
            .id_salt("playlist-scroll")
            .max_height(125.0)
            .show(ui, |ui| {
                let entries: Vec<_> = self
                    .playlist
                    .iter()
                    .map(|entry| (entry.url.clone(), entry.favorite, entry.name.clone()))
                    .collect();
                for (index, (url, favorite, name)) in entries.into_iter().enumerate() {
                    ui.push_id(index, |ui| {
                        ui.horizontal(|ui| {
                            if ui.button(if favorite { "★" } else { "☆" }).clicked() {
                                self.toggle_favorite(index);
                            }
                            if ui
                                .add(egui::Button::new(
                                    egui::RichText::new(if name.is_empty() { &url } else { &name })
                                        .small(),
                                ))
                                .on_hover_text(&url)
                                .clicked()
                            {
                                self.url = url.clone();
                            }
                            if ui.small_button("×").on_hover_text("Eliminar").clicked() {
                                self.remove_entry(index);
                            }
                        });
                    });
                }
            });
    }

    /// Dibuja los metadatos actuales del medio.
    fn draw_playback_info(&self, ui: &mut egui::Ui) {
        let duration = self
            .playback_info
            .duration
            .map(mpv::format_time)
            .unwrap_or_else(|| "--:--".to_owned());
        let position = self
            .playback_info
            .position
            .map(mpv::format_time)
            .unwrap_or_else(|| "--:--".to_owned());
        let percentage = match (self.playback_info.position, self.playback_info.duration) {
            (Some(position), Some(duration)) if duration > 0.0 => {
                format!("{:.1}%", (position / duration * 100.0).clamp(0.0, 100.0))
            }
            _ => "--%".to_owned(),
        };
        ui.label(format!(
            "Nombre: {}",
            if self.playback_info.title.is_empty() {
                "--"
            } else {
                &self.playback_info.title
            }
        ));
        ui.label(format!("Duración: {position} / {duration} ({percentage})"));
    }
}

impl eframe::App for MpvApp {
    /// Ejecuta el ciclo de actualización y dibuja todos los controles.
    fn update(&mut self, context: &egui::Context, _frame: &mut eframe::Frame) {
        self.poll_startup();
        self.poll_logs();
        self.poll_process();
        self.poll_download();
        self.poll_playback_info();
        let keys = context.input(|input| {
            (
                input.key_pressed(egui::Key::Enter),
                input.key_pressed(egui::Key::Space),
                input.key_pressed(egui::Key::ArrowUp),
                input.key_pressed(egui::Key::ArrowDown),
                input.key_pressed(egui::Key::ArrowRight),
                input.key_pressed(egui::Key::ArrowLeft),
            )
        });
        match keys {
            (true, _, _, _, _, _) => self.play(),
            (_, true, _, _, _, _) => self.toggle_pause(),
            (_, _, true, _, _, _) => self.adjust_volume(5),
            (_, _, _, true, _, _) => self.adjust_volume(-5),
            (_, _, _, _, true, _) => self.seek(5),
            (_, _, _, _, _, true) => self.seek(-5),
            _ => {}
        }
        context.request_repaint_after(Duration::from_millis(250));
        egui::CentralPanel::default().show(context, |ui| {
            ui.set_max_width(560.0);
            ui.heading("VisualRustMPV");
            ui.label("Wrapper minimalista para MPV");
            ui.add_space(12.0);
            ui.label("URL, archivo o stream");
            ui.add(egui::TextEdit::singleline(&mut self.url).hint_text("https://..."));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                ui.label("Modo:");
                ui.selectable_value(&mut self.mode, PlaybackMode::Video, "Video");
                ui.selectable_value(&mut self.mode, PlaybackMode::Audio, "Solo Audio");
            });
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Play").clicked() {
                    self.play();
                }
                if ui.button("Pausa/Reanudar").clicked() {
                    self.toggle_pause();
                }
            });
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        !self.download_active,
                        egui::Button::new("Descargar MP4 720p"),
                    )
                    .clicked()
                {
                    self.start_download(false);
                }
                if ui
                    .add_enabled(
                        !self.download_active,
                        egui::Button::new("Descargar MP3 320k"),
                    )
                    .clicked()
                {
                    self.start_download(true);
                }
            });
            ui.add_space(12.0);
            ui.separator();
            ui.label(format!("Estado: {}", self.status));
            self.draw_playback_info(ui);
            ui.small("Enter: reproducir | Espacio: pausa | ↑/↓: volumen | ←/→: avanzar");
            self.draw_playlist(ui);
            ui.separator();
            ui.label("Registro de MPV");
            egui::ScrollArea::vertical()
                .id_salt("mpv-log-scroll")
                .max_height(280.0)
                .stick_to_bottom(true)
                .show(ui, |ui| {
                    for line in &self.logs {
                        ui.monospace(line);
                    }
                });
        });
    }
}

/// Libera MPV cuando la ventana principal se destruye.
impl Drop for MpvApp {
    fn drop(&mut self) {
        self.stop_mpv();
    }
}
