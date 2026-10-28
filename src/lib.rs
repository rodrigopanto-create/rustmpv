//! Biblioteca compartida para escritorio y Android.

#[cfg(target_os = "android")]
mod android_bridge;
pub mod app;
mod downloads;
mod mpv;
mod playlist;

/// Arranca la ventana de eframe en Windows, Linux o macOS.
#[cfg(not(target_os = "android"))]
pub fn run_desktop() -> eframe::Result {
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

/// Entry point que cargo-apk conecta con NativeActivity en Android.
#[cfg(target_os = "android")]
#[unsafe(no_mangle)]
pub fn android_main(android_app: winit::platform::android::activity::AndroidApp) {
    use winit::platform::android::EventLoopBuilderExtAndroid;

    android_bridge::initialize(android_app.clone());

    let mut options = eframe::NativeOptions {
        viewport: eframe::egui::ViewportBuilder::default().with_resizable(false),
        ..Default::default()
    };
    options.event_loop_builder = Some(Box::new(move |event_loop_builder| {
        event_loop_builder.with_android_app(android_app);
    }));
    let _ = eframe::run_native(
        "VisualRustMPV",
        options,
        Box::new(|_creation_context| Ok(Box::new(app::MpvApp::new()))),
    );
}
