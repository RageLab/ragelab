param(
    [string]$OutDir = ""
)

$ErrorActionPreference = "Stop"
$root = Split-Path -Parent $PSScriptRoot
if ([string]::IsNullOrWhiteSpace($OutDir)) {
    $OutDir = Join-Path $root "target\ragelab-wasm-pkg"
} elseif (-not [System.IO.Path]::IsPathRooted($OutDir)) {
    $OutDir = Join-Path $root $OutDir
}

$cargo = (Get-Command cargo -ErrorAction Stop).Source
$bindgen = (Get-Command wasm-bindgen -ErrorAction Stop).Source
$target = "wasm32-unknown-unknown"
$wasmInput = Join-Path $root "target\$target\release\ragelab_wasm.wasm"
$packageTemplate = Join-Path $root "crates\ragelab-wasm\package.json"
$cargoHome = if ([string]::IsNullOrWhiteSpace($env:CARGO_HOME)) {
    Join-Path $env:USERPROFILE ".cargo"
} else {
    $env:CARGO_HOME
}
$existingRustFlags = $env:RUSTFLAGS
$remapFlags = "--remap-path-prefix=$root=. --remap-path-prefix=$cargoHome=.cargo"
$env:RUSTFLAGS = if ([string]::IsNullOrWhiteSpace($existingRustFlags)) {
    $remapFlags
} else {
    "$existingRustFlags $remapFlags"
}

& $cargo build -p ragelab-wasm --target $target --release
if ($LASTEXITCODE -ne 0) { throw "cargo wasm release build failed with exit code $LASTEXITCODE" }

if (Test-Path $OutDir) { Remove-Item -Recurse -Force $OutDir }
New-Item -ItemType Directory -Force -Path $OutDir | Out-Null

& $bindgen $wasmInput --target web --out-dir $OutDir --out-name ragelab_wasm --remove-name-section --remove-producers-section
if ($LASTEXITCODE -ne 0) { throw "wasm-bindgen failed with exit code $LASTEXITCODE" }

Copy-Item $packageTemplate (Join-Path $OutDir "package.json")

function Get-Sha256Hex([string]$Path) {
    $sha256 = [System.Security.Cryptography.SHA256]::Create()
    $stream = [System.IO.File]::OpenRead($Path)
    try {
        $hash = $sha256.ComputeHash($stream)
    } finally {
        $stream.Dispose()
        $sha256.Dispose()
    }
    return ([System.BitConverter]::ToString($hash) -replace "-", "").ToLowerInvariant()
}

$files = Get-ChildItem -File $OutDir | Where-Object { $_.Name -ne "manifest.json" } | Sort-Object Name | ForEach-Object {
    [ordered]@{
        name = $_.Name
        bytes = $_.Length
        sha256 = Get-Sha256Hex $_.FullName
    }
}

$manifest = [ordered]@{
    schema = "ragelab.wasm.package"
    schemaVersion = 1
    target = $target
    bindgenTarget = "web"
    crate = "ragelab-wasm"
    package = "@ragelab/wasm"
    files = @($files)
}
$manifest | ConvertTo-Json -Depth 6 | Set-Content -Encoding utf8 (Join-Path $OutDir "manifest.json")

Write-Output $OutDir
