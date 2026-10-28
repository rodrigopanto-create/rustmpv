//! Adaptador de persistencia Windows sobre core_logic.

pub use core_logic::PlaylistEntry;

/// Archivo de historial usado por la aplicación Windows.
pub const PLAYLIST_FILE: &str = "visualrustmpv_playlist.json";

/// Carga las entradas del historial compartido.
pub fn load() -> Vec<PlaylistEntry> {
    core_logic::Playlist::load(PLAYLIST_FILE).entries
}

/// Guarda las entradas del historial compartido.
pub fn save(entries: &[PlaylistEntry]) {
    let playlist = core_logic::Playlist {
        entries: entries.to_vec(),
    };
    let _ = playlist.save(PLAYLIST_FILE);
}
