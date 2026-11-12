# Build the Windows release package (release binary + docs + SHA256SUMS).
#
# Usage (from translator-sdk): powershell -ExecutionPolicy Bypass -File scripts\package-release.ps1
# Output: augenstern-translate-windows-x64.zip and SHA256SUMS.txt under dist\
# NOTE: keep this file ASCII-only. Windows PowerShell 5.1 reads BOM-less files as ANSI,
#       so non-ASCII comments/strings here would corrupt parsing on stock Windows.
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$out = Join-Path $root "dist"
$name = "augenstern-translate-windows-x64"
$stage = Join-Path $env:TEMP ("release-stage-" + [guid]::NewGuid().ToString("N"))
$pkg = Join-Path $stage $name

Set-Location $root
Write-Host "==> cargo build --locked --release -p translator-cli"
cargo build --locked --release -p translator-cli
if ($LASTEXITCODE -ne 0) { throw "release build failed" }

New-Item -ItemType Directory -Force -Path $pkg | Out-Null
Copy-Item "target\release\translator-cli.exe" $pkg
foreach ($f in @("README.md", "LICENSE-MIT", "LICENSE-APACHE", "THIRD_PARTY_NOTICES.md", "translate.example.toml")) {
    if (Test-Path $f) { Copy-Item $f $pkg }
}
$releaseDoc = Join-Path (Split-Path -Parent $root) "docs\release.md"
if (Test-Path $releaseDoc) { Copy-Item $releaseDoc (Join-Path $pkg "RELEASE.md") }

New-Item -ItemType Directory -Force -Path $out | Out-Null
$zip = Join-Path $out "$name.zip"
if (Test-Path $zip) { Remove-Item $zip -Force }
Compress-Archive -Path $pkg -DestinationPath $zip
if (-not (Test-Path $zip)) { throw "zip archive not created" }

$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $name.zip" | Out-File -Encoding ascii (Join-Path $out "SHA256SUMS.txt")
Get-Content (Join-Path $out "SHA256SUMS.txt")
Remove-Item $stage -Recurse -Force
Write-Host "output: $out"
