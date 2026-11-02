use jni::objects::{JObject, JValue};
use jni::{JavaVM, jni_sig, jni_str};
use std::sync::atomic::{AtomicBool, AtomicI32, Ordering};
use std::sync::mpsc::{self, Receiver};
use winit::platform::android::activity::AndroidApp;

static FULLSCREEN: AtomicBool = AtomicBool::new(false);
static VISIBLE_BOTTOM: AtomicI32 = AtomicI32::new(0);

pub fn visible_bottom() -> Option<i32> {
    let bottom = VISIBLE_BOTTOM.load(Ordering::Acquire);
    (bottom > 0).then_some(bottom)
}

#[unsafe(no_mangle)]
pub extern "system" fn Java_com_visualrustmpv_app_MainActivity_visibleWindowBottom(
    _: *mut std::ffi::c_void,
    _: *mut std::ffi::c_void,
    bottom: i32,
) {
    VISIBLE_BOTTOM.store(bottom, Ordering::Release);
}

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

pub fn open_supported_site_cookies_file(app: &AndroidApp) -> Receiver<Result<String, String>> {
    cookie_action(app, CookieAction::Open)
}

pub fn has_supported_site_cookies(app: &AndroidApp) -> Receiver<Result<String, String>> {
    cookie_action(app, CookieAction::Has)
}

pub fn take_supported_site_cookies_status(app: &AndroidApp) -> Receiver<Result<String, String>> {
    cookie_action(app, CookieAction::TakeStatus)
}

pub fn clear_supported_site_cookies(app: &AndroidApp) -> Receiver<Result<String, String>> {
    cookie_action(app, CookieAction::Clear)
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

enum CookieAction {
    Open,
    Has,
    TakeStatus,
    Clear,
}

fn cookie_action(app: &AndroidApp, action: CookieAction) -> Receiver<Result<String, String>> {
    let (sender, receiver) = mpsc::channel();
    let activity = app.clone();
    app.run_on_java_main_thread(Box::new(move || {
        let android = ndk_context::android_context();
        let vm = unsafe { JavaVM::from_raw(android.vm() as *mut _) };
        let result = vm
            .attach_current_thread(|env| {
                let activity = unsafe { JObject::from_raw(env, activity.activity_as_ptr().cast()) };
                let method = match action {
                    CookieAction::Open => jni_str!("openSupportedSiteCookiesFile"),
                    CookieAction::Has => jni_str!("hasSupportedSiteCookies"),
                    CookieAction::TakeStatus => jni_str!("takeSupportedSiteCookiesStatus"),
                    CookieAction::Clear => jni_str!("clearSupportedSiteCookies"),
                };
                let result = (|| {
                    let value = env
                        .call_method(&activity, method, jni_sig!("()Ljava/lang/String;"), &[])?
                        .l()?;
                    if value.is_null() {
                        return Err(jni::errors::Error::NullPtr("cookies bridge"));
                    }
                    let value = unsafe { jni::objects::JString::from_raw(env, value.into_raw()) };
                    Ok::<_, jni::errors::Error>(value.to_string())
                })();
                if result.is_err() && env.exception_check() {
                    env.exception_describe();
                    env.exception_clear();
                }
                result
            })
            .map_err(|error: jni::errors::Error| error.to_string());
        if sender.send(result).is_err() {
            log::warn!("La interfaz ya no espera la operación de cookies");
        }
    }));
    receiver
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
