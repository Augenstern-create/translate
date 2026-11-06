<#
  windows-build-env.ps1 -- one place that points every build knob into the
  workspace, so a build cannot silently write outside it (ADR-057).

  Read docs/35 sec.1.4 for why ORT_CACHE_DIR matters: ort-sys caches its
  prebuilt ONNX Runtime under %LOCALAPPDATA%\ort.pyke.io by default, and that
  is outside the workspace. `ORT_CACHE_DIR` is honoured first
  (ort-sys src/internal/dirs.rs:133), verified on 2026-11-07.

  Usage:
      . .\tools\p5\windows-build-env.ps1          # dot-source: sets $env:* in THIS shell
      . .\tools\p5\windows-build-env.ps1 -Root 'E:\augenstern\translate'

  Pure environment assignment (process-scoped). It does NOT write files,
  change the registry, or persist anything.
#>
param(
    [string]$Root = 'E:\augenstern\translate'
)

$ErrorActionPreference = 'Continue'

if (-not (Test-Path $Root)) {
    Write-Warning "workspace not found: $Root"
}

$build = Join-Path $Root '.build'

$env:CARGO_HOME         = Join-Path $Root  '.cargo'
$env:CARGO_TARGET_DIR   = Join-Path $build 'target'
$env:TEMP               = Join-Path $build 'temp'
$env:TMP                = $env:TEMP
$env:TMPDIR             = $env:TEMP
$env:PIP_CACHE_DIR      = Join-Path $build 'python-cache'
$env:PYTHONPYCACHEPREFIX = Join-Path $build 'python-cache\pyc'
$env:HF_HOME            = Join-Path $build 'model-cache'
$env:PYTHONNOUSERSITE   = '1'
# ONNX Runtime prebuilt binaries: keep the 360 MB cache inside the workspace.
$env:ORT_CACHE_DIR      = Join-Path $build 'ort-cache'

Write-Output '--- workspace-scoped build environment ---'
foreach ($n in 'CARGO_HOME', 'CARGO_TARGET_DIR', 'TEMP', 'ORT_CACHE_DIR', 'HF_HOME') {
    Write-Output ("{0,-16} = {1}" -f $n, [Environment]::GetEnvironmentVariable($n))
}
Write-Output ("RUSTUP_HOME      = " + $env:USERPROFILE + '\.rustup  (READ-ONLY, not redirected)')
Write-Output ("python           = D:\Anaconda3\python.exe")
Write-Output 'note: proxy, if needed, is set per command, e.g.'
Write-Output '      $env:HTTP_PROXY=''http://127.0.0.1:7890''; $env:HTTPS_PROXY=$env:HTTP_PROXY'
