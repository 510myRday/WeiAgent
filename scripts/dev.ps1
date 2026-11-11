$ErrorActionPreference = 'Stop'

$projectRoot = (Resolve-Path (Join-Path $PSScriptRoot '..')).Path
& cargo run --manifest-path (Join-Path $projectRoot 'Cargo.toml') -- dev
exit $LASTEXITCODE
