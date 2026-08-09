$ErrorActionPreference = 'Stop'

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
$targetDir = Join-Path $projectRoot 'target'
$cacheDir = Join-Path $projectRoot '.btls-prebuilt\windows-x64-msvc-debug'
$cacheInclude = Join-Path $cacheDir 'include'
$cacheLibDir = Join-Path $cacheDir 'Debug'

function Test-BtlsCache {
    return (
        (Test-Path (Join-Path $cacheInclude 'openssl\x509v3.h')) -and
        (Test-Path (Join-Path $cacheLibDir 'crypto.lib')) -and
        (Test-Path (Join-Path $cacheLibDir 'ssl.lib'))
    )
}

$oldBoringBsslPath = $env:BORING_BSSL_PATH

if (-not (Test-BtlsCache)) {
    Write-Host 'Preparing the one-time btls-sys native cache...'

    Remove-Item Env:BORING_BSSL_PATH -ErrorAction SilentlyContinue
    & cargo build --manifest-path (Join-Path $projectRoot 'Cargo.toml')
    if ($LASTEXITCODE -ne 0) {
        if ($null -eq $oldBoringBsslPath) {
            Remove-Item Env:BORING_BSSL_PATH -ErrorAction SilentlyContinue
        } else {
            $env:BORING_BSSL_PATH = $oldBoringBsslPath
        }
        exit $LASTEXITCODE
    }

    $nativeOut = Get-ChildItem (Join-Path $targetDir 'debug\build') -Directory -Filter 'btls-sys-*' |
        ForEach-Object { Join-Path $_.FullName 'out' } |
        Where-Object {
            (Test-Path (Join-Path $_ 'boringssl\include\openssl\x509v3.h')) -and
            (Test-Path (Join-Path $_ 'build\Debug\crypto.lib')) -and
            (Test-Path (Join-Path $_ 'build\Debug\ssl.lib'))
        } |
        Sort-Object { (Get-Item $_).LastWriteTime } -Descending |
        Select-Object -First 1

    if ($null -eq $nativeOut) {
        throw 'btls-sys finished without a discoverable Windows native output directory.'
    }

    New-Item -ItemType Directory -Force $cacheInclude, $cacheLibDir | Out-Null
    Copy-Item (Join-Path $nativeOut 'boringssl\include\*') $cacheInclude -Recurse -Force
    Copy-Item (Join-Path $nativeOut 'build\Debug\crypto.lib') $cacheLibDir -Force
    Copy-Item (Join-Path $nativeOut 'build\Debug\ssl.lib') $cacheLibDir -Force
    Write-Host "btls-sys cache ready: $cacheDir"
}

$env:BORING_BSSL_PATH = $cacheDir
& cargo run --manifest-path (Join-Path $projectRoot 'Cargo.toml') -- dev
exit $LASTEXITCODE
