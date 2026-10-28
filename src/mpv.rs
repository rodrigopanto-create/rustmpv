//! Localización, lanzamiento y comunicación IPC con MPV.

use serde_json::Value;
#[cfg(not(target_os = "android"))]
use std::fs;
use std::fs::OpenOptions;
use std::io::{BufRead, BufReader, Read, Write};
#[cfg(not(target_os = "android"))]
use std::path::Path;
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc;
use std::thread;

/// Named pipe que MPV expone para recibir comandos JSON.
pub const IPC_PIPE: &str = r"\\.\pipe\visualrustmpv";

/// Generador de identificadores numéricos requeridos por el protocolo IPC.
static NEXT_REQUEST_ID: AtomicU64 = AtomicU64::new(1);

/// Modo de reproducción seleccionado en la interfaz.
#[derive(Clone, Copy, PartialEq)]
pub enum PlaybackMode {
    /// Reproduce audio y video.
    Video,
    /// Desactiva el video y oculta la ventana de MPV.
    Audio,
}

/// Datos consultados periódicamente al reproductor.
#[derive(Default)]
pub struct PlaybackInfo {
    /// Título resuelto por MPV.
    pub title: String,
    /// Duración total en segundos.
    pub duration: Option<f64>,
    /// Posición actual en segundos.
    pub position: Option<f64>,
}

/// Busca MPV en PATH, en bin/ y finalmente en el binario embebido.
#[cfg(target_os = "android")]
pub fn locate_or_install() -> Result<PathBuf, String> {
    Err("MPV para Android requiere integrar libmpv.so mediante JNI; los assets mpv.exe de Windows no son compatibles".to_owned())
}

/// Busca MPV en PATH, en bin/ y finalmente en el binario embebido.
#[cfg(not(target_os = "android"))]
pub fn locate_or_install() -> Result<PathBuf, String> {
    if let Ok(path) = which::which("mpv.exe") {
        return Ok(path);
    }
    let local_path = PathBuf::from("bin/mpv.exe");
    if local_path.exists() {
        return Ok(local_path);
    }
    if !internet_available() {
        return embedded_mpv().ok_or_else(|| {
            "Sin conexión y no hay MPV instalado. Coloca assets/mpv.exe y compila con --features embedded-mpv.".to_owned()
        });
    }
    download_mpv(&local_path).or_else(|download_error| {
        embedded_mpv().ok_or_else(|| {
            format!("No se pudo descargar MPV ({download_error}) y no hay binario embebido")
        })
    })
}

/// Inicia MPV con el modo seleccionado y captura sus dos salidas de texto.
pub fn spawn(
    path: PathBuf,
    url: &str,
    mode: PlaybackMode,
    log_sender: mpsc::Sender<String>,
) -> Result<Child, String> {
    let mut command = Command::new(path);
    command.arg(format!("--input-ipc-server={IPC_PIPE}"));
    if mode == PlaybackMode::Audio {
        command.args(["--no-video", "--force-window=no"]);
    } else {
        command.arg("--force-window=yes");
    }
    command
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = command.spawn().map_err(|error| error.to_string())?;
    if let Some(stdout) = child.stdout.take() {
        spawn_log_reader(stdout, "MPV", log_sender.clone());
    }
    if let Some(stderr) = child.stderr.take() {
        spawn_log_reader(stderr, "MPV", log_sender);
    }
    Ok(child)
}

/// Envía un comando JSON al named pipe de MPV.
pub fn send_command(command: &str) -> std::io::Result<()> {
    let mut pipe = OpenOptions::new().write(true).open(IPC_PIPE)?;
    pipe.write_all(command.as_bytes())?;
    pipe.write_all(b"\n")?;
    pipe.flush()
}

/// Consulta una propiedad de MPV y devuelve el valor de data.
pub fn query_property(property: &str) -> std::io::Result<Value> {
    let mut pipe = OpenOptions::new().read(true).write(true).open(IPC_PIPE)?;
    let request = serde_json::json!({
        "command": ["get_property", property],
        "request_id": NEXT_REQUEST_ID.fetch_add(1, Ordering::Relaxed),
    });
    pipe.write_all(request.to_string().as_bytes())?;
    pipe.write_all(b"\n")?;
    pipe.flush()?;
    let mut response = String::new();
    BufReader::new(pipe).read_line(&mut response)?;
    let response: Value = serde_json::from_str(&response)
        .map_err(|error| std::io::Error::new(std::io::ErrorKind::InvalidData, error))?;
    response.get("data").cloned().ok_or_else(|| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, "MPV no devolvió datos")
    })
}

/// Convierte segundos a un reloj legible para la interfaz.
pub fn format_time(seconds: f64) -> String {
    let total_seconds = seconds.max(0.0) as u64;
    let hours = total_seconds / 3600;
    let minutes = (total_seconds % 3600) / 60;
    let seconds = total_seconds % 60;
    if hours > 0 {
        format!("{hours:02}:{minutes:02}:{seconds:02}")
    } else {
        format!("{minutes:02}:{seconds:02}")
    }
}

/// Lee una salida de proceso sin bloquear el hilo de la interfaz.
pub fn spawn_log_reader<R>(reader: R, source: &str, sender: mpsc::Sender<String>)
where
    R: Read + Send + 'static,
{
    let source = source.to_owned();
    thread::spawn(move || {
        for line in BufReader::new(reader).lines().map_while(Result::ok) {
            let _ = sender.send(format!("[{source}] {line}"));
        }
    });
}

/// Comprueba si hay conectividad antes de intentar una descarga.
#[cfg(not(target_os = "android"))]
fn internet_available() -> bool {
    reqwest::blocking::Client::new()
        .head("https://github.com")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .is_ok()
}

/// Descarga MPV desde la URL configurada en MPV_DOWNLOAD_URL.
#[cfg(not(target_os = "android"))]
fn download_mpv(destination: &Path) -> Result<PathBuf, String> {
    let url = std::env::var("MPV_DOWNLOAD_URL")
        .map_err(|_| "Define MPV_DOWNLOAD_URL con un ZIP o EXE de MPV para Windows".to_owned())?;
    let response = reqwest::blocking::get(&url).map_err(|error| error.to_string())?;
    if !response.status().is_success() {
        return Err(format!("HTTP {}", response.status()));
    }
    let bytes = response.bytes().map_err(|error| error.to_string())?;
    fs::create_dir_all(destination.parent().unwrap_or_else(|| Path::new("bin")))
        .map_err(|error| error.to_string())?;
    if url.to_ascii_lowercase().ends_with(".exe") {
        fs::write(destination, bytes).map_err(|error| error.to_string())?;
    } else {
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(bytes))
            .map_err(|error| format!("ZIP inválido: {error}"))?;
        let index = (0..archive.len())
            .find(|&index| {
                archive
                    .by_index(index)
                    .map(|file| file.name().ends_with("mpv.exe"))
                    .unwrap_or(false)
            })
            .ok_or_else(|| "El ZIP no contiene mpv.exe".to_owned())?;
        let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
        let mut output = fs::File::create(destination).map_err(|error| error.to_string())?;
        std::io::copy(&mut file, &mut output).map_err(|error| error.to_string())?;
    }
    Ok(destination.to_path_buf())
}

/// Extrae MPV y su DLL auxiliar desde los assets compilados.
#[cfg(all(feature = "embedded-mpv", not(target_os = "android")))]
fn embedded_mpv() -> Option<PathBuf> {
    let destination = PathBuf::from("bin/mpv.exe");
    let directory = destination.parent()?;
    fs::create_dir_all(directory).ok()?;
    fs::write(&destination, include_bytes!("../assets/mpv.exe")).ok()?;
    fs::write(
        directory.join("d3dcompiler_43.dll"),
        include_bytes!("../assets/d3dcompiler_43.dll"),
    )
    .ok()?;
    Some(destination)
}

/// Desactiva el fallback si la feature correspondiente no fue compilada.
#[cfg(all(not(feature = "embedded-mpv"), not(target_os = "android")))]
fn embedded_mpv() -> Option<PathBuf> {
    None
}
