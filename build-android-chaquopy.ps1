param(
    [ValidateSet("Debug", "Release")]
    [string]$Variant = "Debug"
)

# Prepara mpv-android y QuickJS-ng y ensambla el APK Gradle con Chaquopy.
$ErrorActionPreference = "Stop"
Set-Location $PSScriptRoot

$localTools = Join-Path $env:LOCALAPPDATA "VisualRustMPV\build-tools"
$gradle = Join-Path $PSScriptRoot "android\gradlew.bat"
if (-not (Test-Path $gradle)) {
    $localGradle = Join-Path $localTools "gradle-8.9\bin"
    if (Test-Path (Join-Path $localGradle "gradle.bat")) {
        $gradle = Join-Path $localGradle "gradle.bat"
    } elseif (Get-Command gradle -ErrorAction SilentlyContinue) {
        $gradle = (Get-Command gradle).Source
    } else {
        throw "No se encuentra Gradle Wrapper ni Gradle 8.9 en PATH o $localGradle."
    }
}
$jdk = $env:JAVA_HOME
if (-not $jdk -or -not (Test-Path "$jdk\bin\java.exe")) {
    $localJdk = Get-ChildItem $localTools -Directory -Filter "jdk-17*" -ErrorAction SilentlyContinue |
        Sort-Object Name -Descending | Select-Object -First 1
    if ($localJdk) {
        $jdk = $localJdk.FullName
    }
}
if ($jdk -and (Test-Path "$jdk\bin\java.exe")) {
    $env:JAVA_HOME = $jdk
    $env:PATH = "$jdk\bin;$env:PATH"
}
if (-not (Get-Command java -ErrorAction SilentlyContinue)) {
    throw "Se requiere JDK 17 o 21 en JAVA_HOME/PATH o en $localTools."
}
if (-not $env:CHAQUOPY_BUILD_PYTHON) {
    $hostPython = Join-Path $localTools "Python313\python.exe"
    if (Test-Path $hostPython) {
        $env:CHAQUOPY_BUILD_PYTHON = $hostPython
    }
}
$sdk = $env:ANDROID_HOME
if (-not $sdk) { $sdk = $env:ANDROID_SDK_ROOT }
if (-not $sdk -or -not (Test-Path (Join-Path $sdk "platforms\android-36\android.jar"))) {
    throw "Se requiere Android SDK Platform 36 en ANDROID_HOME o ANDROID_SDK_ROOT."
}
foreach ($directory in @("assets\android\arm64-v8a", "android\app\src\main\jniLibs\arm64-v8a")) {
    foreach ($name in @("libpython_exec.so", "libyt_dlp_exec.so", "libpython3.13.so")) {
        if (Test-Path (Join-Path $directory $name)) {
            throw "Runtime Python alternativo encontrado en $directory\$name. El build Chaquopy usa un solo runtime."
        }
    }
}

$releaseUrl = "https://github.com/mpv-android/mpv-android/releases/download/2026-08-11/app-default-arm64-v8a-release.apk"
$releaseSha256 = "671a1b3f66ee3e598b15be6b3142df2288db01e0ab08fd667851687a15867e05"
$apk = Join-Path $env:TEMP "mpv-android-arm64-v8a.zip"
$nativeDir = Join-Path $PSScriptRoot "assets\android\arm64-v8a"

if (-not (Test-Path $apk)) {
    Invoke-WebRequest -Uri $releaseUrl -OutFile $apk
}
$actualSha256 = (Get-FileHash $apk -Algorithm SHA256).Hash.ToLowerInvariant()
if ($actualSha256 -ne $releaseSha256) {
    throw "SHA-256 de mpv-android invalido: $actualSha256"
}

New-Item -ItemType Directory -Force $nativeDir | Out-Null

# APK es un ZIP, pero Expand-Archive no maneja bien entradas duplicadas.
Add-Type -AssemblyName System.IO.Compression.FileSystem
$archive = [System.IO.Compression.ZipFile]::OpenRead($apk)
try {
    foreach ($entry in $archive.Entries) {
        if ($entry.FullName -like "lib/arm64-v8a/*.so") {
            $destination = Join-Path $nativeDir (Split-Path $entry.FullName -Leaf)
            $sourceStream = $entry.Open()
            $targetStream = [System.IO.File]::Create($destination)
            try {
                $sourceStream.CopyTo($targetStream)
            }
            finally {
                $targetStream.Dispose()
                $sourceStream.Dispose()
            }
        }
    }
}
finally {
    $archive.Dispose()
}

# Android solo permite ejecutar este binario desde nativeLibraryDir, no desde files/.
$quickJsVersion = "0.12.1"
$quickJsSha256 = "c2c54b76ca2f52ffea49658a61c5111449cfe0f94e62510bd3bd7a12e2e18884"
$quickJsArchive = Join-Path $localTools "quickjs-v$quickJsVersion.tar.gz"
$quickJsBuild = Join-Path $PSScriptRoot "android\app\build\quickjs"
$quickJsSource = Join-Path $quickJsBuild "quickjs-$quickJsVersion"
$quickJsOutput = Join-Path $PSScriptRoot "android\app\src\main\jniLibs\arm64-v8a\libqjs.so"
$ndk = $env:ANDROID_NDK_HOME
if (-not $ndk) { throw "Se requiere ANDROID_NDK_HOME para compilar QuickJS-ng." }
$llvm = Join-Path $ndk "toolchains\llvm\prebuilt\windows-x86_64\bin"
$clang = Join-Path $llvm "aarch64-linux-android24-clang.cmd"
if (-not (Test-Path $clang)) { throw "No se encuentra el compilador NDK: $clang" }
New-Item -ItemType Directory -Force $localTools, $quickJsBuild | Out-Null
if (-not (Test-Path $quickJsArchive)) {
    Invoke-WebRequest "https://github.com/quickjs-ng/quickjs/archive/refs/tags/v$quickJsVersion.tar.gz" -OutFile $quickJsArchive
}
if ((Get-FileHash $quickJsArchive -Algorithm SHA256).Hash.ToLowerInvariant() -ne $quickJsSha256) {
    throw "SHA-256 del codigo fuente QuickJS-ng invalido."
}
& tar -xzf $quickJsArchive -C $quickJsBuild
if ($LASTEXITCODE -ne 0) { throw "Fallo la extraccion de QuickJS-ng." }
$quickJsStamp = Join-Path $quickJsBuild "build-id.txt"
$quickJsBuildId = "$quickJsSha256|$ndk|android24|O2|pie|16k|strip|v1"
if (-not (Test-Path $quickJsOutput) -or -not (Test-Path $quickJsStamp) -or
    (Get-Content $quickJsStamp -Raw) -ne $quickJsBuildId) {
    New-Item -ItemType Directory -Force (Split-Path $quickJsOutput) | Out-Null
    Push-Location $quickJsSource
    try {
        & $clang -std=gnu11 -O2 -D_GNU_SOURCE -fPIE -pie `
            "-Wl,-z,max-page-size=16384" "-Wl,--export-dynamic" `
            -o $quickJsOutput qjs.c quickjs.c quickjs-libc.c dtoa.c libregexp.c libunicode.c `
            gen\repl.c gen\standalone.c -lm -ldl
        if ($LASTEXITCODE -ne 0) { throw "Fallo la compilacion NDK de QuickJS-ng." }
        & (Join-Path $llvm "llvm-strip.exe") --strip-unneeded $quickJsOutput
        if ($LASTEXITCODE -ne 0) { throw "Fallo strip de QuickJS-ng." }
        Set-Content -Path $quickJsStamp -Value $quickJsBuildId -NoNewline
    }
    finally {
        Pop-Location
    }
}
$quickJsLicenses = Join-Path $PSScriptRoot "android\app\build\generated\quickjsAssets\licenses"
New-Item -ItemType Directory -Force $quickJsLicenses | Out-Null
Copy-Item (Join-Path $quickJsSource "LICENSE") (Join-Path $quickJsLicenses "quickjs-ng.txt") -Force

cargo ndk -t arm64-v8a --platform 24 -o "android\app\src\main\jniLibs" build -p app_android --release --lib --no-default-features --features chaquopy
if ($LASTEXITCODE -ne 0) { throw "Fallo la compilacion Rust para Chaquopy." }
& $gradle -p android ":app:assemble$Variant" --console=plain --no-daemon --max-workers=2
if ($LASTEXITCODE -ne 0) { throw "Fallo el empaquetado Gradle/Chaquopy." }
$variantPath = $Variant.ToLowerInvariant()
$outputApk = Join-Path $PSScriptRoot "android\app\build\outputs\apk\$variantPath\app-$variantPath.apk"
$archive = [System.IO.Compression.ZipFile]::OpenRead($outputApk)
try {
    $entries = $archive.Entries.FullName
    $pythonRuntimes = @($entries | Where-Object { $_ -match '^lib/[^/]+/libpython[^/]*\.so$' })
    if ($pythonRuntimes.Count -ne 1 -or $pythonRuntimes[0] -ne "lib/arm64-v8a/libpython3.13.so") {
        throw "El APK no contiene exactamente el runtime Python 3.13 de Chaquopy."
    }
    $alternateFiles = @($entries | Where-Object {
        $_ -eq "assets/yt-dlp" -or $_ -like "assets/python-runtime/*" -or
        $_ -match '/lib(python_exec|yt_dlp_exec)\.so$'
    })
    if ($alternateFiles.Count -gt 0) {
        throw "El APK contiene archivos de un backend Python alternativo: $($alternateFiles -join ', ')"
    }
    foreach ($required in @("lib/arm64-v8a/libqjs.so", "assets/licenses/quickjs-ng.txt")) {
        if ($entries -notcontains $required) { throw "Falta $required en el APK." }
    }
    $requirementsEntry = $archive.GetEntry("assets/chaquopy/requirements-common.imy")
    if (-not $requirementsEntry) { throw "Faltan las dependencias Python en el APK." }
    $requirementsData = New-Object System.IO.MemoryStream
    try {
        $sourceStream = $requirementsEntry.Open()
        try { $sourceStream.CopyTo($requirementsData) } finally { $sourceStream.Dispose() }
        $requirementsData.Position = 0
        $requirementsArchive = New-Object System.IO.Compression.ZipArchive($requirementsData)
        try {
            foreach ($script in @("core.min.js", "lib.min.js")) {
                if (-not $requirementsArchive.GetEntry("yt_dlp_ejs/yt/solver/$script")) {
                    throw "Falta el solver EJS $script en el APK."
                }
            }
        }
        finally { $requirementsArchive.Dispose() }
    }
    finally { $requirementsData.Dispose() }
}
finally {
    $archive.Dispose()
}
Write-Host "APK: $outputApk"
Write-Host "Verificado: un solo runtime Python, sin assets Python de subproceso; QuickJS-ng y EJS incluidos."
