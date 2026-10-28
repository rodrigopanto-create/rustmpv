use jni::objects::{JClass, JObject, JString, JValue};
use jni::{Env, JavaVM, jni_sig, jni_str};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex, OnceLock, Weak};

use crate::stream_url::MediaInfo;

struct BridgeError(String);

impl From<jni::errors::Error> for BridgeError {
    fn from(error: jni::errors::Error) -> Self {
        Self(error.to_string())
    }
}

fn java_error(env: &mut Env, error: jni::errors::Error) -> BridgeError {
    let Some(exception) = env.exception_occurred() else {
        return error.into();
    };
    env.exception_clear();
    let description = (|| {
        let value = env
            .call_method(
                &exception,
                jni_str!("toString"),
                jni_sig!("()Ljava/lang/String;"),
                &[],
            )?
            .l()?;
        let value = unsafe { JString::from_raw(env, value.into_raw()) };
        Ok::<_, jni::errors::Error>(value.to_string())
    })();
    match description {
        Ok(message) => BridgeError(message),
        Err(description_error) => {
            env.exception_describe();
            env.exception_clear();
            BridgeError(format!("{error}; leer excepcion Java: {description_error}"))
        }
    }
}

fn main_activity<'local>(env: &mut Env<'local>) -> jni::errors::Result<JClass<'local>> {
    let android = ndk_context::android_context();
    // ndk-context exposes Application, not Activity. Its loader sees app classes.
    let context = unsafe { JObject::from_raw(env, android.context() as *mut _) };
    let loader = env
        .call_method(
            &context,
            jni_str!("getClassLoader"),
            jni_sig!("()Ljava/lang/ClassLoader;"),
            &[],
        )?
        .l()?;
    let name = JObject::from(env.new_string("com.visualrustmpv.app.MainActivity")?);
    let class = env
        .call_method(
            &loader,
            jni_str!("loadClass"),
            jni_sig!("(Ljava/lang/String;)Ljava/lang/Class;"),
            &[JValue::Object(&name)],
        )?
        .l()?;
    Ok(unsafe { JClass::from_raw(env, class.into_raw()) })
}

pub fn ca_bundle_path() -> Result<String, String> {
    let android = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android.vm() as *mut _) };
    vm.attach_current_thread(|env| {
        let result = (|| {
            let class = main_activity(env)?;
            let value = env
                .call_static_method(
                    &class,
                    jni_str!("caBundlePath"),
                    jni_sig!("()Ljava/lang/String;"),
                    &[],
                )?
                .l()?;
            if value.is_null() {
                return Err(jni::errors::Error::NullPtr("caBundlePath"));
            }
            let value = unsafe { JString::from_raw(env, value.into_raw()) };
            Ok(value.to_string())
        })();
        result.map_err(|error| java_error(env, error))
    })
    .map_err(|error: BridgeError| format!("Certificados CA de Chaquopy: {}", error.0))
}

pub fn extract_stream_url(
    input: &str,
    audio_only: bool,
) -> Result<Receiver<Result<MediaInfo, String>>, String> {
    submit(input, audio_only, None)
}

/// Encola una resolucion anticipada. Si el `Arc` devuelto se suelta antes de
/// que el worker la empiece, se descarta sin tocar la red.
pub fn prefetch(
    input: &str,
    audio_only: bool,
) -> Result<(Receiver<Result<MediaInfo, String>>, Arc<()>), String> {
    let wanted = Arc::new(());
    let receiver = submit(input, audio_only, Some(Arc::downgrade(&wanted)))?;
    Ok((receiver, wanted))
}

fn submit(
    input: &str,
    audio_only: bool,
    wanted: Option<Weak<()>>,
) -> Result<Receiver<Result<MediaInfo, String>>, String> {
    let (reply, receiver) = mpsc::channel();
    worker()?
        .jobs
        .send(Job {
            input: input.to_owned(),
            audio_only,
            wanted,
            reply,
        })
        .map_err(|_| "El worker de Python ya no esta disponible".to_owned())?;
    Ok(receiver)
}

/// Estado de la precarga del interprete y de yt-dlp.
#[derive(Clone, Debug, PartialEq)]
pub enum Warmup {
    Pending,
    Ready(std::time::Duration, String),
    Failed(String),
}

struct Job {
    input: String,
    audio_only: bool,
    wanted: Option<Weak<()>>,
    reply: Sender<Result<MediaInfo, String>>,
}

struct Worker {
    jobs: Sender<Job>,
}

static WORKER: OnceLock<Result<Worker, String>> = OnceLock::new();
static WARMUP: Mutex<Option<Warmup>> = Mutex::new(None);

fn worker() -> Result<&'static Worker, String> {
    WORKER
        .get_or_init(spawn_worker)
        .as_ref()
        .map_err(Clone::clone)
}

/// Arranca, una sola vez por proceso, el hilo que mantiene Python y yt-dlp en memoria.
pub fn start_worker() -> Result<(), String> {
    worker().map(|_| ())
}

pub fn warmup() -> Option<Warmup> {
    WARMUP.lock().expect("warmup lock").clone()
}

fn set_warmup(state: Warmup) {
    *WARMUP.lock().expect("warmup lock") = Some(state);
}

static NOTICE: Mutex<Option<String>> = Mutex::new(None);

/// Mensaje pendiente sobre actualizaciones de yt-dlp para mostrar en la UI.
pub fn take_notice() -> Option<String> {
    NOTICE.lock().expect("notice lock").take()
}

fn notify(message: String) {
    log::info!("{message}");
    *NOTICE.lock().expect("notice lock") = Some(message);
}

fn call_string(
    vm: &JavaVM,
    class: &jni::refs::Global<JClass<'static>>,
    name: &jni::strings::JNIStr,
) -> Result<String, String> {
    vm.attach_current_thread(|env| {
        let result = (|| {
            let value = env
                .call_static_method(class, name, jni_sig!("()Ljava/lang/String;"), &[])?
                .l()?;
            if value.is_null() {
                return Err(jni::errors::Error::NullPtr("String estatico"));
            }
            let value = unsafe { JString::from_raw(env, value.into_raw()) };
            Ok(value.to_string())
        })();
        result.map_err(|error| java_error(env, error))
    })
    .map_err(|error: BridgeError| error.0)
}

static UPDATER: OnceLock<(JavaVM, jni::refs::Global<JClass<'static>>)> = OnceLock::new();
static CHECKING: AtomicBool = AtomicBool::new(false);

/// Busqueda manual: ignora el limite de 24 h y siempre deja un aviso con el resultado.
pub fn check_update_now() -> Result<(), String> {
    spawn_update_check(true)
}

pub fn update_check_available() -> bool {
    UPDATER.get().is_some() && !CHECKING.load(Ordering::Acquire)
}

/// Busca una version nueva de yt-dlp sin bloquear el worker; se aplica al reiniciar.
fn spawn_update_check(force: bool) -> Result<(), String> {
    let (vm, class) = UPDATER
        .get()
        .ok_or_else(|| "yt-dlp aun se esta preparando".to_owned())?;
    if CHECKING.swap(true, Ordering::AcqRel) {
        return Err("Ya se esta buscando una actualizacion".to_owned());
    }
    let spawned = std::thread::Builder::new()
        .name("yt-dlp-update".to_owned())
        .spawn(move || {
            let result = vm
                .attach_current_thread(|env| {
                    let result = (|| {
                        let value = env
                            .call_static_method(
                                class,
                                jni_str!("checkYtDlpUpdate"),
                                jni_sig!("(Z)Ljava/lang/String;"),
                                &[JValue::Bool(force)],
                            )?
                            .l()?;
                        if value.is_null() {
                            return Err(jni::errors::Error::NullPtr("checkYtDlpUpdate"));
                        }
                        let value = unsafe { JString::from_raw(env, value.into_raw()) };
                        Ok(value.to_string())
                    })();
                    result.map_err(|error| java_error(env, error))
                })
                .map_err(|error: BridgeError| error.0);
            match result {
                Ok(message) if !message.is_empty() => notify(message),
                Ok(_) => {}
                Err(error) => {
                    log::error!("Buscar actualizacion de yt-dlp: {error}");
                    if force {
                        notify(format!(
                            "No se pudo buscar actualizacion de yt-dlp: {error}"
                        ));
                    }
                }
            }
            CHECKING.store(false, Ordering::Release);
        });
    spawned.map(|_| ()).map_err(|error| {
        CHECKING.store(false, Ordering::Release);
        format!("Hilo de actualizacion de yt-dlp: {error}")
    })
}
fn spawn_worker() -> Result<Worker, String> {
    let android = ndk_context::android_context();
    let vm = unsafe { JavaVM::from_raw(android.vm() as *mut _) };
    let class = vm
        .attach_current_thread(|env| {
            let result = (|| {
                let class = main_activity(env)?;
                env.new_global_ref(class)
            })();
            result.map_err(|error| java_error(env, error))
        })
        .map_err(|error: BridgeError| format!("Inicializar puente Chaquopy: {}", error.0))?;
    let (jobs, queue) = mpsc::channel::<Job>();
    set_warmup(Warmup::Pending);
    std::thread::Builder::new()
        .name("yt-dlp-worker".to_owned())
        .spawn(move || {
            // attach_current_thread keeps this thread attached for its whole life and
            // opens a fresh local-reference frame for every call below.
            let started = std::time::Instant::now();
            log::info!("Worker Python: precargando yt-dlp");
            let warmed = vm
                .attach_current_thread(|env| {
                    let result = env
                        .call_static_method(&class, jni_str!("warmUpYtDlp"), jni_sig!("()D"), &[])
                        .and_then(|value| value.d());
                    result.map_err(|error| java_error(env, error))
                })
                .map_err(|error: BridgeError| error.0);
            let ready = match warmed {
                Ok(python_seconds) => {
                    let elapsed = started.elapsed();
                    let version = call_string(&vm, &class, jni_str!("ytDlpVersion"))
                        .unwrap_or_else(|error| {
                            log::error!("Leer version de yt-dlp: {error}");
                            "?".to_owned()
                        });
                    log::info!(
                        "Worker Python listo en {:.2} s (yt-dlp {version}, {python_seconds:.2} s)",
                        elapsed.as_secs_f64()
                    );
                    set_warmup(Warmup::Ready(elapsed, version));
                    true
                }
                Err(error) => {
                    log::error!("Precarga de yt-dlp: {error}");
                    set_warmup(Warmup::Failed(error));
                    false
                }
            };
            let started = std::time::Instant::now();
            let player = call_string(&vm, &class, jni_str!("warmUpYouTubePlayer"));
            let elapsed = started.elapsed().as_secs_f64();
            match player {
                Ok(state) => log::info!("Player de YouTube {state} ({elapsed:.2} s)"),
                Err(error) => {
                    log::error!("Calentar player de YouTube tras {elapsed:.2} s: {error}")
                }
            }
            if ready {
                match call_string(&vm, &class, jni_str!("verifyYtDlpUpdate")) {
                    Ok(message) if !message.is_empty() => notify(message),
                    Ok(_) => {}
                    Err(error) => log::error!("Verificar actualizacion de yt-dlp: {error}"),
                }
                match vm.attach_current_thread(|env| env.new_global_ref(&class)) {
                    Ok(updater_class) => {
                        let _ = UPDATER.set((vm.clone(), updater_class));
                        if let Err(error) = spawn_update_check(false) {
                            log::error!("{error}");
                        }
                    }
                    Err(error) => log::error!("Referencia para actualizar yt-dlp: {error}"),
                }
            }
            for job in queue {
                if job
                    .wanted
                    .as_ref()
                    .is_some_and(|wanted| wanted.upgrade().is_none())
                {
                    log::info!("Precarga descartada antes de empezar");
                    continue;
                }
                let started = std::time::Instant::now();
                log::info!("Worker Python: resolviendo con yt-dlp precargado");
                let result = vm
                    .attach_current_thread(|env| {
                        let result = (|| {
                            let url = JObject::from(env.new_string(&job.input)?);
                            let value = env
                                .call_static_method(
                                    &class,
                                    jni_str!("extractMedia"),
                                    jni_sig!("(Ljava/lang/String;Z)Ljava/lang/String;"),
                                    &[JValue::Object(&url), JValue::Bool(job.audio_only)],
                                )?
                                .l()?;
                            if value.is_null() {
                                return Err(jni::errors::Error::NullPtr("extractMedia"));
                            }
                            let value = unsafe { JString::from_raw(env, value.into_raw()) };
                            Ok(value.to_string())
                        })();
                        result.map_err(|error| java_error(env, error))
                    })
                    .map_err(|error: BridgeError| format!("Chaquopy/yt-dlp: {}", error.0))
                    .and_then(|value| MediaInfo::from_json(&value));
                let elapsed = started.elapsed().as_secs_f64();
                match &result {
                    Ok(media) => log::info!(
                        "Chaquopy resolvio una URL HTTP(S) en {elapsed:.2} s ({} miniaturas)",
                        media.thumbnails.len()
                    ),
                    Err(error) => log::error!("Error de Chaquopy tras {elapsed:.2} s: {error}"),
                }
                if job.reply.send(result).is_err() {
                    log::info!("La interfaz ya no espera el resultado de Chaquopy");
                }
            }
            log::info!("Worker Python finalizado");
        })
        .map_err(|error| format!("Crear worker Python: {error}"))?;
    Ok(Worker { jobs })
}
