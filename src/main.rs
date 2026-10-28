#![cfg_attr(windows, windows_subsystem = "windows")]

//! Punto de entrada mínimo de VisualRustMPV.

/// Configura la ventana nativa y arranca la aplicación egui.
#[cfg(not(target_os = "android"))]
fn main() -> eframe::Result {
    visual_rust_mpv::run_desktop()
}
