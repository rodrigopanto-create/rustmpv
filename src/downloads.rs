//! Inicialización y ejecución de yt-dlp y ffmpeg.

#[cfg(not(target_os = "android"))]
use crate::mpv::spawn_log_reader;
#[cfg(not(target_os = "android"))]
use std::fs;
#[cfg(not(target_os = "android"))]
use std::fs::File;
#[cfg(not(target_os = "android"))]
use std::path::Path;
#[cfg(not(target_os = "android"))]
use std::path::PathBuf;
#[cfg(not(target_os = "android"))]
use std::process::{Command, Stdio};
use std::sync::mpsc;
#[cfg(not(target_os = "android"))]
use std::thread;

/// Rutas resueltas de las herramientas de descarga.
pub struct DownloadTools {
    /// Ejecutable de yt-dlp.
    #[cfg(not(target_os = "android"))]
    pub ytdlp: PathBuf,
    /// Ejecutable de ffmpeg.
    #[cfg(not(target_os = "android"))]
    pub ffmpeg: PathBuf,
}

/// Resultado textual que recibe la GUI cuando termina una descarga.
pub type DownloadResult = Result<String, String>;

/// Busca, descarga o extrae las dos herramientas necesarias.
#[cfg(target_os = "android")]
pub fn locate_or_install() -> Result<DownloadTools, String> {
    Ok(DownloadTools {})
}

/// Busca, descarga o extrae las dos herramientas necesarias.
#[cfg(not(target_os = "android"))]
pub fn locate_or_install() -> Result<DownloadTools, String> {
    let online = internet_available();
    let ytdlp = locate_or_install_tool(
        "YT_DLP_PATH",
        "yt-dlp.exe",
        "https://github.com/yt-dlp/yt-dlp/releases/latest/download/yt-dlp.exe",
        online,
    )?;
    let ffmpeg = locate_or_install_tool(
        "FFMPEG_PATH",
        "ffmpeg.exe",
        "https://www.gyan.dev/ffmpeg/builds/ffmpeg-release-essentials.zip",
        online,
    )?;
    Ok(DownloadTools { ytdlp, ffmpeg })
}

/// Ejecuta una descarga sin bloquear la ventana de egui.
#[cfg(target_os = "android")]
pub fn spawn_download(
    _tools: &DownloadTools,
    url: String,
    audio_only: bool,
    log_sender: mpsc::Sender<String>,
) -> Result<(mpsc::Receiver<DownloadResult>, bool), String> {
    let output_dir = std::env::var("HOME")
        .map(|home| format!("{home}/downloads"))
        .unwrap_or_else(|_| "downloads".to_owned());
    let receiver = crate::android_bridge::spawn_download(url, output_dir, audio_only, log_sender)?;
    Ok((receiver, audio_only))
}

/// Ejecuta una descarga sin bloquear la ventana de egui.
#[cfg(not(target_os = "android"))]
pub fn spawn_download(
    tools: &DownloadTools,
    url: String,
    audio_only: bool,
    log_sender: mpsc::Sender<String>,
) -> Result<(mpsc::Receiver<DownloadResult>, bool), String> {
    fs::create_dir_all("downloads").map_err(|error| error.to_string())?;
    let output_template = "downloads/%(title)s.%(ext)s";
    let mut command = Command::new(&tools.ytdlp);
    command
        .arg("--newline")
        .arg("--no-playlist")
        .arg("--ffmpeg-location")
        .arg(&tools.ffmpeg)
        .arg("-o")
        .arg(output_template);
    if audio_only {
        command.args(["-x", "--audio-format", "mp3", "--audio-quality", "320K"]);
    } else {
        command.args([
            "-f",
            "bv*[height<=720]+ba/b[height<=720]",
            "--merge-output-format",
            "mp4",
        ]);
    }
    command
        .arg(url)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut process = command.spawn().map_err(|error| error.to_string())?;
    if let Some(stdout) = process.stdout.take() {
        spawn_log_reader(stdout, "yt-dlp", log_sender.clone());
    }
    if let Some(stderr) = process.stderr.take() {
        spawn_log_reader(stderr, "yt-dlp", log_sender);
    }
    let (sender, receiver) = mpsc::channel();
    thread::spawn(move || {
        let result = process
            .wait()
            .map_err(|error| error.to_string())
            .and_then(|status| {
                if status.success() {
                    Ok("Descarga completada en la carpeta downloads".to_owned())
                } else {
                    Err(format!("yt-dlp terminó con estado {status}"))
                }
            });
        let _ = sender.send(result);
    });
    Ok((receiver, audio_only))
}

/// Resuelve una herramienta usando variable de entorno, bin/, PATH o descarga.
#[cfg(not(target_os = "android"))]
fn locate_or_install_tool(
    variable: &str,
    local_name: &str,
    download_url: &str,
    online: bool,
) -> Result<PathBuf, String> {
    if let Some(path) = std::env::var_os(variable).map(PathBuf::from) {
        if path.is_file() {
            return Ok(path);
        }
        return Err(format!(
            "{variable} apunta a una ruta inexistente: {}",
            path.display()
        ));
    }
    let local_path = PathBuf::from("bin").join(local_name);
    if local_path.is_file() {
        return Ok(local_path);
    }
    if let Ok(path) = which::which(local_name) {
        return Ok(path);
    }
    if online {
        if let Ok(path) = download_tool(download_url, &local_path, local_name) {
            return Ok(path);
        }
    }
    embedded_tool(local_name).ok_or_else(|| {
        format!(
            "No se encontró {local_name}. Instálalo en PATH, colócalo en bin/, define {variable} o añade assets/{local_name}."
        )
    })
}

/// Descarga un EXE directo o extrae el ejecutable solicitado desde un ZIP.
#[cfg(not(target_os = "android"))]
fn download_tool(url: &str, destination: &Path, executable_name: &str) -> Result<PathBuf, String> {
    let response = reqwest::blocking::get(url).map_err(|error| error.to_string())?;
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
                    .map(|file| file.name().ends_with(executable_name))
                    .unwrap_or(false)
            })
            .ok_or_else(|| format!("El ZIP no contiene {executable_name}"))?;
        let mut file = archive.by_index(index).map_err(|error| error.to_string())?;
        let mut output = File::create(destination).map_err(|error| error.to_string())?;
        std::io::copy(&mut file, &mut output).map_err(|error| error.to_string())?;
    }
    Ok(destination.to_path_buf())
}

/// Comprueba conectividad antes de usar las URLs de descarga.
#[cfg(not(target_os = "android"))]
fn internet_available() -> bool {
    reqwest::blocking::Client::new()
        .head("https://github.com")
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .is_ok()
}

/// Extrae una herramienta embebida en el ejecutable.
#[cfg(all(feature = "embedded-tools", not(target_os = "android")))]
fn embedded_tool(executable_name: &str) -> Option<PathBuf> {
    let destination = PathBuf::from("bin").join(executable_name);
    fs::create_dir_all(destination.parent()?).ok()?;
    let bytes = match executable_name {
        "yt-dlp.exe" => include_bytes!("../assets/yt-dlp.exe").as_slice(),
        "ffmpeg.exe" => include_bytes!("../assets/ffmpeg.exe").as_slice(),
        _ => return None,
    };
    fs::write(&destination, bytes).ok()?;
    Some(destination)
}

/// Devuelve None cuando el fallback embebido está desactivado.
#[cfg(all(not(feature = "embedded-tools"), not(target_os = "android")))]
fn embedded_tool(_executable_name: &str) -> Option<PathBuf> {
    None
}
