#![windows_subsystem = "windows"]

//! Aplicación Windows: GUI egui y procesos externos de MPV/yt-dlp.

#[path = "../../../src/app.rs"]
mod app;
#[path = "../../../src/downloads.rs"]
mod downloads;
#[path = "../../../src/mpv.rs"]
mod mpv;
mod playlist;

/// Arranca la aplicación Windows.
fn main() -> eframe::Result {
    let options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([560.0, 560.0])
            .with_resizable(false),
        ..Default::default()
    };
    eframe::run_native(
        "VisualRustMPV",
        options,
        Box::new(|_creation_context| Ok(Box::new(app::MpvApp::new()))),
    )
}
