use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

pub struct Bundle {
    pub executable: PathBuf,
    pub script: Option<PathBuf>,
    pub python_home: PathBuf,
}

fn write_asset(path: &Path, bytes: &[u8]) -> Result<(), String> {
    match fs::read(path) {
        Ok(existing) if existing == bytes => return Ok(()),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(format!("Leer {}: {error}", path.display())),
    }
    let parent = path
        .parent()
        .ok_or_else(|| "Asset sin directorio".to_owned())?;
    fs::create_dir_all(parent).map_err(|error| format!("Crear {}: {error}", parent.display()))?;
    let temporary = path.with_extension("installing");
    fs::write(&temporary, bytes).map_err(|error| format!("Extraer {}: {error}", path.display()))?;
    fs::rename(&temporary, path).map_err(|error| format!("Instalar {}: {error}", path.display()))
}

fn runtime_path(value: &str) -> Result<&Path, String> {
    let path = Path::new(value);
    if value.contains('\\')
        || !path
            .components()
            .all(|part| matches!(part, Component::Normal(_)))
        || value.is_empty()
        || path.extension().is_some_and(|ext| ext == "so")
    {
        return Err(format!(
            "Ruta no permitida en el manifiesto Python: {value}"
        ));
    }
    Ok(path)
}

pub fn prepare(
    files_dir: &Path,
    native_dir: &Path,
    mut read_asset: impl FnMut(&str) -> Result<Vec<u8>, String>,
) -> Result<Bundle, String> {
    let standalone = native_dir.join("libyt_dlp_exec.so");
    let python = native_dir.join("libpython_exec.so");
    if standalone.is_file() == python.is_file() {
        return Err("Empaqueta exactamente uno: libyt_dlp_exec.so o libpython_exec.so".to_owned());
    }
    let python_home = files_dir.join("python");
    if standalone.is_file() {
        log::info!("Usando yt-dlp standalone desde nativeLibraryDir");
        return Ok(Bundle {
            executable: standalone,
            script: None,
            python_home,
        });
    }

    let manifest = read_asset("python-runtime/manifest.txt")?;
    let manifest = std::str::from_utf8(&manifest)
        .map_err(|error| format!("Manifiesto Python no es UTF-8: {error}"))?;
    let entries: Vec<_> = manifest
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
        .collect();
    for required in [
        "lib/python313.zip",
        "site-packages/yt_dlp/__init__.py",
        "site-packages/certifi/cacert.pem",
    ] {
        if !entries.contains(&required) {
            return Err(format!("Falta {required} en el manifiesto Python"));
        }
    }
    for entry in entries {
        let path = runtime_path(entry)?;
        let bytes = read_asset(&format!("python-runtime/{entry}"))?;
        write_asset(&python_home.join(path), &bytes)?;
    }
    fs::create_dir_all(python_home.join("lib/python3.13"))
        .map_err(|error| format!("Preparar stdlib: {error}"))?;
    let script = files_dir.join("yt-dlp");
    write_asset(&script, &read_asset("yt-dlp")?)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&script, fs::Permissions::from_mode(0o700))
            .map_err(|error| format!("chmod yt-dlp: {error}"))?;
    }
    log::info!(
        "Script yt-dlp y runtime Python instalados en {}",
        files_dir.display()
    );
    Ok(Bundle {
        executable: python,
        script: Some(script),
        python_home,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_runtime_paths_outside_bundle() {
        for path in [
            "../secret",
            "/absolute",
            "a/../../b",
            "C:\\outside",
            "lib/module.so",
            "",
        ] {
            assert!(runtime_path(path).is_err(), "{path}");
        }
        assert!(runtime_path("site-packages/yt_dlp/__init__.py").is_ok());
    }

    #[test]
    fn installs_once_and_updates_assets() {
        let directory = std::env::temp_dir().join(format!("yt-dlp-install-{}", std::process::id()));
        fs::create_dir_all(&directory).unwrap();
        let path = directory.join("yt-dlp");
        write_asset(&path, b"version 1").unwrap();
        let modified = fs::metadata(&path).unwrap().modified().unwrap();
        write_asset(&path, b"version 1").unwrap();
        assert_eq!(fs::metadata(&path).unwrap().modified().unwrap(), modified);
        write_asset(&path, b"version 2").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"version 2");
        fs::remove_file(path).unwrap();
        fs::remove_dir(directory).unwrap();
    }

    #[test]
    fn prepares_python_bundle_and_reports_missing_assets() {
        let directory = std::env::temp_dir().join(format!("yt-dlp-bundle-{}", std::process::id()));
        let native = directory.join("native");
        let files = directory.join("files");
        fs::create_dir_all(&native).unwrap();
        let error = prepare(&files, &native, |_| unreachable!()).err().unwrap();
        assert!(error.contains("exactamente uno"));
        fs::write(
            native.join("libpython_exec.so"),
            b"test fixture, not an executable",
        )
        .unwrap();
        let manifest = b"lib/python313.zip\nsite-packages/yt_dlp/__init__.py\nsite-packages/certifi/cacert.pem\n";
        let bundle = prepare(&files, &native, |name| {
            Ok(match name {
                "python-runtime/manifest.txt" => manifest.to_vec(),
                "yt-dlp" => b"from yt_dlp import main; main()".to_vec(),
                _ => b"fixture".to_vec(),
            })
        })
        .unwrap();
        assert_eq!(bundle.executable, native.join("libpython_exec.so"));
        assert_eq!(bundle.script, Some(files.join("yt-dlp")));
        assert!(bundle.python_home.join("lib/python313.zip").is_file());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(files.join("yt-dlp"))
                    .unwrap()
                    .permissions()
                    .mode()
                    & 0o777,
                0o700
            );
        }
        assert!(
            prepare(&files, &native, |name| Err(format!(
                "Asset ausente: {name}"
            )))
            .is_err()
        );
        for path in [
            "files/python/lib/python313.zip",
            "files/python/site-packages/yt_dlp/__init__.py",
            "files/python/site-packages/certifi/cacert.pem",
            "files/yt-dlp",
            "native/libpython_exec.so",
        ] {
            fs::remove_file(directory.join(path)).unwrap();
        }
        for path in [
            "files/python/site-packages/yt_dlp",
            "files/python/site-packages/certifi",
            "files/python/site-packages",
            "files/python/lib/python3.13",
            "files/python/lib",
            "files/python",
            "files",
            "native",
        ] {
            fs::remove_dir(directory.join(path)).unwrap();
        }
        fs::remove_dir(directory).unwrap();
    }
}
