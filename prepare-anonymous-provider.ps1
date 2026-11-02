$ErrorActionPreference = "Stop"
$tools = Join-Path $env:LOCALAPPDATA "VisualRustMPV\build-tools"
$build = Join-Path $PSScriptRoot "android\app\build\anonymousProvider"
$assets = Join-Path $PSScriptRoot "android\app\build\generated\anonymousProviderAssets"
$native = Join-Path $PSScriptRoot "android\app\src\main\jniLibs\arm64-v8a\libnode.so"
New-Item -ItemType Directory -Force $tools, $build, $assets, (Split-Path $native) | Out-Null
Add-Type -AssemblyName System.IO.Compression.FileSystem

function Get-VerifiedFile([string]$Url, [string]$Hash, [string]$Path) {
    if (-not (Test-Path $Path)) {
        & curl.exe -fL --retry 2 --connect-timeout 30 --max-time 600 $Url -o "$Path.partial"
        if ($LASTEXITCODE -ne 0) { throw "No se pudo descargar $Url" }
        Move-Item "$Path.partial" $Path -Force
    }
    if ((Get-FileHash $Path -Algorithm SHA256).Hash.ToLowerInvariant() -ne $Hash) {
        throw "SHA-256 invalido: $Path"
    }
}

$nodeVersion = "26.12.1"
$providerVersion = "2.0.2"
$nodeHash = "57d6928e5ae7f715b6ac2ba9b15795fba1901d114c9125844bfbfb9d35b53ac6"
$providerHash = "df68ee6210e4960757bceae298927fb2ebd4d5cd9d390f9ff3a8287b0d2d1054"
$nodeApk = Join-Path $tools "nodejs-$nodeVersion-arm64-v8a.apk"
$sourceZip = Join-Path $tools "bgutil-$providerVersion.zip"
Get-VerifiedFile "https://github.com/deniscerri/ytdlnis-packages/releases/download/nodejs-$nodeVersion/app-nodejs-arm64-v8a-release.apk" $nodeHash $nodeApk
Get-VerifiedFile "https://github.com/Brainicism/bgutil-ytdlp-pot-provider/archive/refs/tags/$providerVersion.zip" $providerHash $sourceZip

$id = "$nodeHash|$providerHash|script-node|v1"
$stamp = Join-Path $build "build-id.txt"
$bundleZip = Join-Path $assets "anonymous-provider.zip"
if ((Test-Path $stamp) -and (Get-Content $stamp -Raw) -eq $id -and
    (Test-Path $bundleZip) -and (Test-Path $native) -and
    (Test-Path (Join-Path $assets "licenses\bgutil-GPL-3.0.txt")) -and
    (Test-Path (Join-Path $assets "licenses\nodejs.txt")) -and
    (Test-Path (Join-Path $assets "licenses\libffi.txt")) -and
    (Test-Path (Join-Path $assets "licenses\libcxx.txt")) -and
    (Test-Path (Join-Path $assets "sources\bgutil-$providerVersion.zip"))) {
    Write-Host "Proveedor anonimo Node.js/bgutil al dia."
    return
}

$hostNode = Join-Path $tools "node-v22.16.0-win-x64\node.exe"
if (-not (Test-Path $hostNode)) {
    $hostZip = Join-Path $tools "node-v22.16.0-win-x64.zip"
    Get-VerifiedFile "https://nodejs.org/dist/v22.16.0/node-v22.16.0-win-x64.zip" `
        "21c2d9735c80b8f86dab19305aa6a9f6f59bbc808f68de3eef09d5832e3bfbbd" $hostZip
    [System.IO.Compression.ZipFile]::ExtractToDirectory($hostZip, $tools)
}
$env:PATH = "$(Split-Path $hostNode);$env:PATH"

$source = Join-Path $build "bgutil-ytdlp-pot-provider-$providerVersion"
if (-not (Test-Path $source)) {
    [System.IO.Compression.ZipFile]::ExtractToDirectory($sourceZip, $build)
}
$server = Join-Path $source "server"
Push-Location $server
try {
    & (Join-Path (Split-Path $hostNode) "npm.cmd") ci --ignore-scripts --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw "Fallo npm ci del proveedor PO." }
    & $hostNode "node_modules\typescript\bin\tsc"
    if ($LASTEXITCODE -ne 0) { throw "Fallo TypeScript del proveedor PO." }
    & (Join-Path (Split-Path $hostNode) "npm.cmd") prune --omit=dev --ignore-scripts --no-audit --no-fund
    if ($LASTEXITCODE -ne 0) { throw "Fallo el empaquetado de dependencias del proveedor PO." }
    & $hostNode "build\generate_once.js" --version
    if ($LASTEXITCODE -ne 0) { throw "No se pudo iniciar el proveedor PO." }
} finally {
    Pop-Location
}

$content = Join-Path $build "bundle"
$runtimeLibs = Join-Path $content "runtime-libs"
New-Item -ItemType Directory -Force $runtimeLibs | Out-Null
$runtimeZip = Join-Path $build "node-runtime.zip"
$apk = [System.IO.Compression.ZipFile]::OpenRead($nodeApk)
try {
    foreach ($item in @(
        @("lib/arm64-v8a/libnode.so", $native),
        @("lib/arm64-v8a/libnode.zip.so", $runtimeZip)
    )) {
        $entry = $apk.GetEntry($item[0])
        if (-not $entry) { throw "Falta $($item[0]) en el runtime Node." }
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile($entry, $item[1], $true)
    }
} finally { $apk.Dispose() }

# Only Node's dependency closure; do not ship unrelated readline/Tcl/npm.
$libraries = @(
    "libz_node.so.1", "libcares_node.so", "libsqlite3_node.so.3.54.1",
    "libffi_node.so", "libcrypto_node.so.3", "libssl_node.so.3",
    "libicui18n_node.so.78", "libicuuc_node.so.78", "libicudata_node.so.78",
    "libc++_shared_node.so"
)
$archive = [System.IO.Compression.ZipFile]::OpenRead($runtimeZip)
try {
    foreach ($name in $libraries) {
        $entry = $archive.GetEntry("usr/lib/$name")
        if (-not $entry -or $entry.Length -lt 4096) {
            throw "Falta la biblioteca ELF real $name de Node."
        }
        [System.IO.Compression.ZipFileExtensions]::ExtractToFile(
            $entry, (Join-Path $runtimeLibs $name), $true)
    }
} finally { $archive.Dispose() }

$bundleServer = Join-Path $content "server"
$plugin = Join-Path $content "plugin\yt_dlp_plugins\extractor"
New-Item -ItemType Directory -Force $bundleServer, $plugin | Out-Null
foreach ($name in @("build", "node_modules")) {
    Copy-Item (Join-Path $server $name) $bundleServer -Recurse -Force
}
Copy-Item (Join-Path $server "package.json") $bundleServer -Force
foreach ($name in @("getpot_bgutil.py", "getpot_bgutil_script.py")) {
    Copy-Item (Join-Path $source "plugin\yt_dlp_plugins\extractor\$name") $plugin -Force
}
@{
    node_version = $nodeVersion
    node_apk_sha256 = $nodeHash
    provider_version = $providerVersion
    provider_source_sha256 = $providerHash
} | ConvertTo-Json | Set-Content (Join-Path $content "manifest.json") -Encoding ASCII

$licenses = Join-Path $assets "licenses"
$sources = Join-Path $assets "sources"
New-Item -ItemType Directory -Force $licenses, $sources | Out-Null
Copy-Item (Join-Path $source "LICENSE") (Join-Path $licenses "bgutil-GPL-3.0.txt") -Force
Copy-Item $sourceZip (Join-Path $sources "bgutil-$providerVersion.zip") -Force
Get-VerifiedFile "https://raw.githubusercontent.com/nodejs/node/v$nodeVersion/LICENSE" `
    "8e603aa87c70d36e497004f0dec63111511cb61b7d65ace12e10ea791599286c" `
    (Join-Path $licenses "nodejs.txt")
Get-VerifiedFile "https://raw.githubusercontent.com/libffi/libffi/v3.4.6/LICENSE" `
    "67894089811f93fca47a76f85e017da6f8582d4ba0905963c6e0f1ad6df7a195" `
    (Join-Path $licenses "libffi.txt")
Get-VerifiedFile "https://raw.githubusercontent.com/llvm/llvm-project/llvmorg-21.1.0/libcxx/LICENSE.TXT" `
    "539dd7aed86e8a4f12cbdd0e6c50c189c7d74847e4fecc64ce2c6ee3a01da38b" `
    (Join-Path $licenses "libcxx.txt")
# Dependencies retain their own licenses and JS source inside the bundle.
if (Test-Path $bundleZip) { Remove-Item $bundleZip }
[System.IO.Compression.ZipFile]::CreateFromDirectory(
    $content, $bundleZip, [System.IO.Compression.CompressionLevel]::Optimal, $false)
Set-Content $stamp $id -NoNewline -Encoding ASCII
Write-Host "Proveedor PO local preparado: Node.js $nodeVersion, bgutil $providerVersion."
