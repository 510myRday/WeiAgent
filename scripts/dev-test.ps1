$ErrorActionPreference = 'Stop'

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
& cargo test --manifest-path (Join-Path $projectRoot 'Cargo.toml')
if ($LASTEXITCODE -ne 0) {
    exit $LASTEXITCODE
}

& cargo check --manifest-path (Join-Path $projectRoot 'Cargo.toml')
exit $LASTEXITCODE
