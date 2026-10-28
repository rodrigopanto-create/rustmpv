//! Configura el enlace nativo según el target seleccionado por Cargo.

fn main() {
    let target = std::env::var("TARGET").expect("TARGET no está definido");
    let manifest_dir = std::env::var_os("CARGO_MANIFEST_DIR").expect("CARGO_MANIFEST_DIR");

    if target == "aarch64-linux-android" {
        let lib_dir = std::path::PathBuf::from(&manifest_dir)
            .join("../../assets/android/arm64-v8a")
            .canonicalize()
            .expect("No se encuentra assets/android/arm64-v8a");
        println!(
            "cargo:rerun-if-changed={}",
            lib_dir.join("libmpv.so").display()
        );
        println!("cargo:rustc-link-search=native={}", lib_dir.display());
        println!("cargo:rustc-link-lib=dylib=mpv");
        if std::env::var_os("CARGO_FEATURE_NATIVE_PYTHON").is_some() {
            let python_dir = std::path::PathBuf::from(&manifest_dir)
                .join("../../assets/android/arm64-v8a")
                .canonicalize()
                .expect("No se encuentra el directorio de librerías Android");
            println!(
                "cargo:rerun-if-changed={}",
                python_dir.join("libpython3.13.so").display()
            );
            println!("cargo:rustc-link-search=native={}", python_dir.display());
            println!("cargo:rustc-link-lib=dylib=python3.13");
        }
    } else if target == "x86_64-pc-windows-msvc" {
        // Optional local desktop check. Android builds never inspect this path.
        if let Some(path) = std::env::var_os("APP_ANDROID_WINDOWS_MPV_LIB") {
            let lib_path = std::path::PathBuf::from(path);
            if let Some(parent) = lib_path.parent() {
                println!("cargo:rerun-if-changed={}", lib_path.display());
                println!("cargo:rustc-link-search=native={}", parent.display());
                println!("cargo:rustc-link-lib=dylib=mpv");
            }
        }
    }
}
