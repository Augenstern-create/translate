# 打发布包（Windows）：release 二进制 + 必要文件 + SHA256SUMS。
#
# 用法（在 translator-sdk 目录）：powershell -ExecutionPolicy Bypass -File scripts\package-release.ps1
# 产物：augenstern-translate-windows-x64.zip 与 SHA256SUMS.txt
$ErrorActionPreference = "Stop"

$root = Split-Path -Parent (Split-Path -Parent $MyInvocation.MyCommand.Path)
$out = Join-Path $root "dist"
$name = "augenstern-translate-windows-x64"
$stage = Join-Path $env:TEMP ("release-stage-" + [guid]::NewGuid().ToString("N"))
$pkg = Join-Path $stage $name

Set-Location $root
Write-Host "==> 构建 release（--locked）"
cargo build --locked --release -p translator-cli

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
Compress-Archive -Path (Join-Path $stage $name) -DestinationPath $zip

$hash = (Get-FileHash $zip -Algorithm SHA256).Hash.ToLower()
"$hash  $name.zip" | Out-File -Encoding ascii (Join-Path $out "SHA256SUMS.txt")
Get-Content (Join-Path $out "SHA256SUMS.txt")
Remove-Item $stage -Recurse -Force
Write-Host "产物目录：$out"
