//! Modelos y persistencia de la lista de reproducción.

use serde::{Deserialize, Serialize};
use std::fs;

/// Archivo local donde se conserva el historial y los favoritos.
pub const PLAYLIST_FILE: &str = "visualrustmpv_playlist.json";

/// Una URL guardada junto con su nombre resuelto y estado de favorito.
#[derive(Clone, Debug, Deserialize, Serialize)]
pub struct PlaylistEntry {
    /// URL original introducida por el usuario.
    pub url: String,
    /// Indica si la entrada está marcada como favorita.
    pub favorite: bool,
    /// Nombre proporcionado por MPV; puede estar vacío para entradas antiguas.
    #[serde(default)]
    pub name: String,
}

/// Carga el historial desde disco o devuelve una lista vacía si aún no existe.
pub fn load() -> Vec<PlaylistEntry> {
    fs::read_to_string(PLAYLIST_FILE)
        .ok()
        .and_then(|contents| serde_json::from_str(&contents).ok())
        .unwrap_or_default()
}

/// Guarda el historial en formato JSON legible.
pub fn save(entries: &[PlaylistEntry]) {
    if let Ok(contents) = serde_json::to_string_pretty(entries) {
        let _ = fs::write(PLAYLIST_FILE, contents);
    }
}
