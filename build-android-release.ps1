$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$signingDir = Join-Path $env:LOCALAPPDATA "VisualRustMPV\signing"
$keystore = Join-Path $signingDir "visualrustmpv-release.p12"
$encryptedPasswordFile = Join-Path $signingDir "keystore-password.dpapi"
$keyAlias = "visualrustmpv"
New-Item -ItemType Directory -Force $signingDir | Out-Null

$keytool = if ($env:JAVA_HOME) { Join-Path $env:JAVA_HOME "bin\keytool.exe" } else { $null }
if (-not $keytool -or -not (Test-Path $keytool)) {
    $keytool = (Get-Command keytool.exe -ErrorAction SilentlyContinue).Source
}
if (-not $keytool) {
    $localJdk = Get-ChildItem (Join-Path $env:LOCALAPPDATA "VisualRustMPV\build-tools") `
        -Filter keytool.exe -Recurse -ErrorAction SilentlyContinue | Select-Object -First 1
    if ($localJdk) { $keytool = $localJdk.FullName }
}
if (-not $keytool -or -not (Test-Path $keytool)) {
    throw "No se encuentra keytool.exe. Instala JDK 17 o ejecuta build-android-chaquopy.ps1 primero."
}

$hasKeystore = Test-Path $keystore
$hasPassword = Test-Path $encryptedPasswordFile
if ($hasKeystore -ne $hasPassword) {
    throw "El keystore y su secreto DPAPI deben existir juntos en $signingDir. No se generara una identidad nueva automaticamente."
}

if (-not $hasKeystore) {
    $random = [byte[]]::new(32)
    $rng = [System.Security.Cryptography.RandomNumberGenerator]::Create()
    try { $rng.GetBytes($random) } finally { $rng.Dispose() }
    $password = [Convert]::ToBase64String($random).TrimEnd('=').Replace('+', 'A').Replace('/', 'B')
    [Array]::Clear($random, 0, $random.Length)
    $securePassword = ConvertTo-SecureString $password -AsPlainText -Force
    ConvertFrom-SecureString $securePassword | Set-Content -Path $encryptedPasswordFile -Encoding ASCII
    $env:VISUALRUSTMPV_KEYSTORE_PASSWORD = $password
    $env:VISUALRUSTMPV_KEY_PASSWORD = $password
    $env:VISUALRUSTMPV_KEYSTORE = $keystore
    $env:VISUALRUSTMPV_KEY_ALIAS = $keyAlias
    $keytoolArgs = @(
        "-genkeypair", "-v", "-keystore", $keystore, "-storetype", "PKCS12",
        "-storepass:env", "VISUALRUSTMPV_KEYSTORE_PASSWORD",
        "-keypass:env", "VISUALRUSTMPV_KEY_PASSWORD",
        "-alias", $keyAlias, "-keyalg", "RSA", "-keysize", "3072",
        "-validity", "10000", "-dname", "CN=VisualRustMPV"
    )
    try {
        & $keytool @keytoolArgs
    }
    finally {
        Remove-Item Env:VISUALRUSTMPV_KEYSTORE_PASSWORD -ErrorAction SilentlyContinue
        Remove-Item Env:VISUALRUSTMPV_KEY_PASSWORD -ErrorAction SilentlyContinue
    }
    if ($LASTEXITCODE -ne 0) {
        Remove-Item $encryptedPasswordFile -ErrorAction SilentlyContinue
        Remove-Item $keystore -ErrorAction SilentlyContinue
        throw "No se pudo crear el keystore de release."
    }
    $password = $null
    $securePassword.Dispose()
}

$encryptedPassword = (Get-Content $encryptedPasswordFile -Raw).Trim()
$securePassword = ConvertTo-SecureString $encryptedPassword
$passwordPointer = [Runtime.InteropServices.Marshal]::SecureStringToBSTR($securePassword)
try {
    $password = [Runtime.InteropServices.Marshal]::PtrToStringBSTR($passwordPointer)
    $env:VISUALRUSTMPV_KEYSTORE = $keystore
    $env:VISUALRUSTMPV_KEYSTORE_PASSWORD = $password
    $env:VISUALRUSTMPV_KEY_ALIAS = $keyAlias
    $env:VISUALRUSTMPV_KEY_PASSWORD = $password

    & (Join-Path $PSScriptRoot "build-android-chaquopy.ps1") -Variant Release
    if ($LASTEXITCODE -ne 0) { throw "Fallo la compilacion del APK de release." }
}
finally {
    Remove-Item Env:VISUALRUSTMPV_KEYSTORE -ErrorAction SilentlyContinue
    Remove-Item Env:VISUALRUSTMPV_KEYSTORE_PASSWORD -ErrorAction SilentlyContinue
    Remove-Item Env:VISUALRUSTMPV_KEY_ALIAS -ErrorAction SilentlyContinue
    Remove-Item Env:VISUALRUSTMPV_KEY_PASSWORD -ErrorAction SilentlyContinue
    [Runtime.InteropServices.Marshal]::ZeroFreeBSTR($passwordPointer)
}

$sdk = $env:ANDROID_HOME
if (-not $sdk) { $sdk = $env:ANDROID_SDK_ROOT }
if (-not $sdk) { throw "No se encuentra Android SDK para verificar la firma." }
$buildTools = Get-ChildItem (Join-Path $sdk "build-tools") -Directory |
    Sort-Object { [version]($_.Name -replace '-.*$', '') } -Descending | Select-Object -First 1
if (-not $buildTools) { throw "No se encuentran Android SDK Build Tools." }
$apksigner = Join-Path $buildTools.FullName "apksigner.bat"
$zipalign = Join-Path $buildTools.FullName "zipalign.exe"
$apk = Join-Path $PSScriptRoot "android\app\build\outputs\apk\release\app-release.apk"
if (-not (Test-Path $apksigner)) { throw "No se encuentra apksigner: $apksigner" }
if (-not (Test-Path $apk)) { throw "No se genero el APK: $apk" }

& $apksigner verify --verbose --print-certs $apk
if ($LASTEXITCODE -ne 0) { throw "La firma del APK no se valido." }
if (Test-Path $zipalign) {
    & $zipalign -c -P 16 -v 4 $apk
    if ($LASTEXITCODE -ne 0) { throw "El APK no cumple alineacion de 16 KiB." }
}

$versionName = (Select-String -Path "android\app\build.gradle" `
    -Pattern '^\s*versionName\s+"([^"]+)"').Matches.Groups[1].Value
if (-not $versionName) { throw "No se pudo leer versionName de build.gradle." }
$dist = Join-Path $PSScriptRoot "dist"
New-Item -ItemType Directory -Force $dist | Out-Null
$distributionApk = Join-Path $dist "VisualRustMPV-v$versionName-arm64-v8a.apk"
Copy-Item $apk $distributionApk -Force
$sha256 = (Get-FileHash $distributionApk -Algorithm SHA256).Hash.ToLowerInvariant()
Write-Host ""
Write-Host "APK firmado de distribucion: $distributionApk"
Write-Host "SHA-256: $sha256"
Write-Warning "Conserva una copia segura de $keystore; la necesitas para actualizar esta app. El secreto esta cifrado con DPAPI para esta cuenta de Windows."
