// Puente JNI entre Rust, MainActivity y el runtime Python de Chaquopy.

use android_activity::AndroidApp;
use jni::objects::{JObject, JString, JValue};
use jni::{JavaVM, jni_sig, jni_str};
use std::sync::OnceLock;
use std::sync::mpsc;
use std::thread;

static JAVA_VM: OnceLock<JavaVM> = OnceLock::new();

/// Guarda la VM Android para las llamadas que se hagan desde Rust.
pub fn initialize(app: AndroidApp) {
    let vm = unsafe { JavaVM::from_raw(app.vm_as_ptr() as *mut _) };
    let _ = JAVA_VM.set(vm);
}

/// Resuelve una URL de stream usando yt-dlp en Chaquopy.
pub fn extract_stream_url(url: String, audio_only: bool) -> Result<mpsc::Receiver<Result<String, String>>, String> {
    let vm = JAVA_VM
        .get()
        .cloned()
        .ok_or_else(|| "El puente JNI todavía no está inicializado".to_owned())?;
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = call_python(&vm, &url, audio_only);
        let _ = sender.send(result);
    });
    Ok(receiver)
}

/// Invoca el método estático de MainActivity que llama al módulo Python.
fn call_python(
    vm: &JavaVM,
    url: &str,
    audio_only: bool,
) -> Result<String, String> {
    vm.attach_current_thread(|env| {
        let class = env.find_class(jni_str!("com/visualrustmpv/app/MainActivity"))?;
        let url = JObject::from(env.new_string(url)?);
        let value = env.call_static_method(
            class,
            jni_str!("extractStreamUrl"),
            jni_sig!("(Ljava/lang/String;Z)Ljava/lang/String;"),
            &[
                JValue::Object(&url),
                JValue::Bool(audio_only),
            ],
        )?;
        let value = unsafe { JString::from_raw(env, value.l()?.into_raw()) };
        Ok::<_, jni::errors::Error>(value.to_string())
    })
    .map_err(|error| error.to_string())
}

