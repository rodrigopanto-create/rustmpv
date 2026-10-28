//! Lógica compartida entre las aplicaciones Windows y Android.

use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

/// Modo de reproducción común a ambas interfaces.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum PlaybackMode {
    /// Reproduce video y audio.
    Video,
    /// Solicita solamente audio.
    Audio,
}

/// Estado mínimo compartido del reproductor.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlaybackState {
    /// Título resuelto del medio.
    pub title: String,
    /// Duración en segundos.
    pub duration: Option<f64>,
    /// Posición actual en segundos.
    pub position: Option<f64>,
    /// Indica si está pausado.
    pub paused: bool,
}

/// Elemento persistente del historial y favoritos.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct PlaylistEntry {
    /// URL original.
    pub url: String,
    /// Nombre resuelto por el backend.
    #[serde(default)]
    pub name: String,
    /// Marca de favorito.
    #[serde(default)]
    pub favorite: bool,
}

/// Historial que comparten las dos aplicaciones.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Playlist {
    /// Entradas ordenadas desde la más reciente.
    pub entries: Vec<PlaylistEntry>,
}

impl Playlist {
    /// Carga una playlist existente o crea una vacía.
    pub fn load(path: impl AsRef<Path>) -> Self {
        fs::read_to_string(path)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    /// Guarda la playlist como JSON.
    pub fn save(&self, path: impl AsRef<Path>) -> Result<(), String> {
        let text = serde_json::to_string_pretty(self).map_err(|error| error.to_string())?;
        fs::write(path, text).map_err(|error| error.to_string())
    }

    /// Añade una URL al principio conservando nombre y favorito existentes.
    pub fn touch(&mut self, url: impl Into<String>) {
        let url = url.into();
        if let Some(index) = self.entries.iter().position(|entry| entry.url == url) {
            let entry = self.entries.remove(index);
            self.entries.insert(0, entry);
        } else {
            self.entries.insert(
                0,
                PlaylistEntry {
                    url,
                    ..Default::default()
                },
            );
        }
        self.entries.truncate(100);
    }

    /// Actualiza el nombre resuelto de una URL.
    pub fn set_name(&mut self, url: &str, name: impl Into<String>) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| entry.url == url) {
            entry.name = name.into();
        }
    }

    /// Alterna el favorito de una entrada.
    pub fn toggle_favorite(&mut self, index: usize) {
        if let Some(entry) = self.entries.get_mut(index) {
            entry.favorite = !entry.favorite;
        }
    }

    /// Elimina una entrada por índice.
    pub fn remove(&mut self, index: usize) {
        if index < self.entries.len() {
            self.entries.remove(index);
        }
    }
}

/// Construye el selector Android para video sin depender de FFmpeg.
pub fn android_video_format() -> &'static str {
    "best[ext=mp4][height<=720]"
}

/// Construye el selector Android para audio directo sin postprocesado externo.
pub fn android_audio_format() -> &'static str {
    "bestaudio[ext=m4a]/bestaudio[ext=webm]"
}
