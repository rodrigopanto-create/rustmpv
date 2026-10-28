use jni::objects::{JObject, JValue};
use jni::{JavaVM, jni_sig, jni_str};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};
use winit::platform::android::activity::AndroidApp;

static FULLSCREEN: AtomicBool = AtomicBool::new(false);

pub fn fullscreen() -> bool {
    FULLSCREEN.load(Ordering::Acquire)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_visualrustmpv_app_MainActivity_screenModeChanged(
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
    fullscreen: u8,
) {
    FULLSCREEN.store(fullscreen != 0, Ordering::Release);
}

pub fn request(app: &AndroidApp, fullscreen: bool) -> Receiver<Result<(), String>> {
    request_flag(app, fullscreen, Action::Fullscreen)
}

pub fn keep_awake(app: &AndroidApp, enabled: bool) -> Receiver<Result<(), String>> {
    request_flag(app, enabled, Action::KeepAwake)
}

pub fn start_playback_service(
    app: &AndroidApp,
    media: &crate::stream_url::MediaInfo,
) -> Receiver<Result<(), String>> {
    let (sender, receiver) = mpsc::channel();
    let activity = app.clone();
    let title = media.title.clone();
    let artist = media.artist.clone();
    // Validated URLs contain no whitespace, so a newline is an unambiguous separator.
    let thumbnails = media.thumbnails.join("\n");
    app.run_on_java_main_thread(Box::new(move || {
        let android = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(android.vm() as *mut _) };
        let result = vm
            .attach_current_thread(|env| {
                let result = (|| {
                    let activity =
                        unsafe { JObject::from_raw(env, activity.activity_as_ptr().cast()) };
                    let title = JObject::from(env.new_string(&title)?);
                    let artist = JObject::from(env.new_string(&artist)?);
                    let thumbnails = JObject::from(env.new_string(&thumbnails)?);
                    env.call_method(
                        &activity,
                        jni_str!("startPlaybackService"),
                        jni_sig!("(Ljava/lang/String;Ljava/lang/String;Ljava/lang/String;)V"),
                        &[
                            JValue::Object(&title),
                            JValue::Object(&artist),
                            JValue::Object(&thumbnails),
                        ],
                    )
                    .map(|_| ())
                })();
                if result.is_err() && env.exception_check() {
                    env.exception_describe();
                    env.exception_clear();
                }
                result
            })
            .map_err(|error: jni::errors::Error| error.to_string());
        if sender.send(result).is_err() {
            log::warn!("La interfaz ya no espera el servicio multimedia");
        }
    }));
    receiver
}

enum Action {
    Fullscreen,
    KeepAwake,
}

fn request_flag(app: &AndroidApp, enabled: bool, action: Action) -> Receiver<Result<(), String>> {
    let (sender, receiver) = mpsc::channel();
    let app = app.clone();
    let activity = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let android = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(android.vm() as *mut _) };
        let result = vm
            .attach_current_thread(|env| {
                let activity = unsafe { JObject::from_raw(env, activity.activity_as_ptr().cast()) };
                let result = env.call_method(
                    &activity,
                    match action {
                        Action::KeepAwake => jni_str!("setPlayerKeepScreenOn"),
                        Action::Fullscreen => jni_str!("setPlayerFullscreen"),
                    },
                    jni_sig!("(Z)V"),
                    &[JValue::Bool(enabled)],
                );
                if result.is_err() && env.exception_check() {
                    env.exception_describe();
                    env.exception_clear();
                }
                result.map(|_| ())
            })
            .map_err(|error: jni::errors::Error| error.to_string());
        if sender.send(result).is_err() {
            log::warn!("La interfaz ya no espera el cambio de ventana");
        }
    }));
    receiver
}
