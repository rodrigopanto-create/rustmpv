# Construye el APK debug para arm64-v8a.
$ErrorActionPreference = "Stop"
cargo apk build --target aarch64-linux-android

# El APK se genera dentro de target/android-artifacts.
Write-Host "APK generado en target/android-artifacts"
