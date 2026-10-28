use crate::stream_url::validate_stream_url;
use std::io::{self, Read};
#[cfg(target_os = "android")]
use std::path::Path;
use std::process::{Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

const OUTPUT_LIMIT: usize = 256 * 1024;
#[cfg(target_os = "android")]
pub const RESOLVE_TIMEOUT: Duration = Duration::from_secs(45);

fn capture(mut pipe: impl Read) -> io::Result<(Vec<u8>, bool)> {
    let mut output = Vec::new();
    let mut buffer = [0; 8192];
    let mut truncated = false;
    loop {
        let count = pipe.read(&mut buffer)?;
        if count == 0 {
            return Ok((output, truncated));
        }
        let keep = count.min(OUTPUT_LIMIT - output.len());
        output.extend_from_slice(&buffer[..keep]);
        truncated |= keep < count;
    }
}

pub fn run(mut command: Command, timeout: Duration) -> Result<String, String> {
    command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(target_os = "android")]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Iniciar yt-dlp: {error}"))?;
    // Both pipes must be drained concurrently, even after the capture limit is reached.
    let stdout = child.stdout.take().expect("piped stdout");
    let stderr = child.stderr.take().expect("piped stderr");
    let out = thread::spawn(move || capture(stdout));
    let err = thread::spawn(move || capture(stderr));
    let start = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Ok(status),
            Ok(None) if start.elapsed() < timeout => thread::sleep(Duration::from_millis(25)),
            Ok(None) => {
                break Err(format!(
                    "yt-dlp excedio el timeout de {} s",
                    timeout.as_secs()
                ));
            }
            Err(error) => break Err(format!("Esperar yt-dlp: {error}")),
        }
    };
    #[cfg(target_os = "android")]
    {
        // JS runtimes spawned by yt-dlp may inherit pipes. Close the whole process group.
        let result = unsafe { libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL) };
        if result != 0 && io::Error::last_os_error().raw_os_error() != Some(libc::ESRCH) {
            log::error!(
                "No se pudo terminar el grupo yt-dlp: {}",
                io::Error::last_os_error()
            );
        }
    }
    if status.is_err() {
        // Always reap the child: killing alone leaves a zombie on Android.
        if let Err(error) = child.kill() {
            log::error!("No se pudo terminar yt-dlp: {error}");
        }
        if let Err(error) = child.wait() {
            log::error!("No se pudo recolectar yt-dlp: {error}");
        }
    }
    let (stdout, out_truncated) = out
        .join()
        .map_err(|_| "Fallo el lector de stdout".to_owned())?
        .map_err(|error| format!("Leer stdout: {error}"))?;
    let (stderr, err_truncated) = err
        .join()
        .map_err(|_| "Fallo el lector de stderr".to_owned())?
        .map_err(|error| format!("Leer stderr: {error}"))?;
    let stderr = String::from_utf8_lossy(&stderr);
    if !stderr.trim().is_empty() {
        log::info!("yt-dlp stderr: {stderr}");
    }
    let status = status.map_err(|error| format!("{error}; stderr: {stderr}"))?;
    if !status.success() {
        return Err(format!("yt-dlp termino con {status}; stderr: {stderr}"));
    }
    if out_truncated || err_truncated {
        return Err("La salida de yt-dlp excedio el limite de captura".to_owned());
    }
    let output =
        String::from_utf8(stdout).map_err(|error| format!("stdout no es UTF-8: {error}"))?;
    validate_stream_url(&output)
}

#[cfg(target_os = "android")]
pub fn resolve(
    executable: &Path,
    script: Option<&Path>,
    python_home: &Path,
    input: &str,
    audio_only: bool,
) -> Result<String, String> {
    let mut command = Command::new(executable);
    if let Some(directory) = executable.parent() {
        command.env("LD_LIBRARY_PATH", directory);
    }
    if let Some(script) = script {
        command
            .arg(script)
            .env("PYTHONHOME", python_home)
            .env("PYTHONPATH", python_home.join("site-packages"))
            .env("PYTHONNOUSERSITE", "1")
            .env("PYTHONDONTWRITEBYTECODE", "1");
    }
    command.args([
        "--ignore-config",
        "--no-playlist",
        "--no-progress",
        "--socket-timeout",
        "10",
        "--retries",
        "1",
        "--extractor-retries",
        "1",
        "-g",
        "-f",
        if audio_only { "bestaudio" } else { "best" },
        "--",
        input,
    ]);
    log::info!("Iniciando yt-dlp en subproceso");
    let result = run(command, RESOLVE_TIMEOUT);
    match &result {
        Ok(_) => log::info!("yt-dlp resolvio una URL HTTP(S)"),
        Err(error) => log::error!("Error del subproceso yt-dlp: {error}"),
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn capture_is_bounded_and_drains_input() {
        let input = vec![b'x'; OUTPUT_LIMIT + 500];
        let mut reader = &input[..];
        let (output, truncated) = capture(&mut reader).unwrap();
        assert_eq!(output.len(), OUTPUT_LIMIT);
        assert!(truncated);
        assert!(reader.is_empty());
    }

    #[cfg(windows)]
    fn shell(script: &str) -> Command {
        let mut command = Command::new("powershell.exe");
        command.args(["-NoProfile", "-NonInteractive", "-Command", script]);
        command
    }

    #[cfg(unix)]
    fn shell(script: &str) -> Command {
        let mut command = Command::new("sh");
        command.args(["-c", script]);
        command
    }

    #[test]
    fn subprocess_success_and_failure() {
        #[cfg(windows)]
        let (success, failure) = (
            "[Console]::Out.WriteLine('https://cdn.example/video')",
            "[Console]::Error.WriteLine('extractor failed'); exit 3",
        );
        #[cfg(unix)]
        let (success, failure) = (
            "printf 'https://cdn.example/video\\n'",
            "printf 'extractor failed' >&2; exit 3",
        );
        assert!(run(shell(success), Duration::from_secs(10)).is_ok());
        let error = run(shell(failure), Duration::from_secs(10)).unwrap_err();
        assert!(error.contains("extractor failed"), "{error}");
    }

    #[test]
    fn subprocess_timeout() {
        #[cfg(windows)]
        let script = "Start-Sleep -Seconds 30";
        #[cfg(unix)]
        let script = "exec sleep 30";
        let start = Instant::now();
        let error = run(shell(script), Duration::from_millis(200)).unwrap_err();
        assert!(error.contains("timeout"), "{error}");
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn drains_full_stderr_without_blocking_stdout() {
        #[cfg(windows)]
        let script = "[Console]::Error.Write(('x' * 300000)); [Console]::Out.WriteLine('https://cdn.example/video')";
        #[cfg(unix)]
        let script = "head -c 300000 /dev/zero >&2; printf 'https://cdn.example/video\\n'";
        let error = run(shell(script), Duration::from_secs(10)).unwrap_err();
        assert!(error.contains("limite de captura"), "{error}");
    }
}
