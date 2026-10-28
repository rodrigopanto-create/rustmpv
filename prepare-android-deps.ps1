# Valida las dependencias nativas Android antes de crear el APK.
$ErrorActionPreference = "Stop"
$abi = "arm64-v8a"
$abiDirectory = Join-Path $PSScriptRoot "assets\android\$abi"
$libmpv = Join-Path $abiDirectory "libmpv.so"

if (-not (Test-Path $libmpv)) {
    throw "Falta $libmpv. Obtén libmpv.so desde un build Android compatible de mpv-android/libmpv."
}

Write-Host "Dependencias nativas encontradas para $abi"
Write-Host "  $libmpv"
Write-Host "Ahora ejecuta: cargo apk build --lib"
