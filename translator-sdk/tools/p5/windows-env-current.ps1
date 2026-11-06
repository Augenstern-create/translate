<#
  windows-env-current.ps1 -- P5.1 read-only environment capture (schema dsh-win-env/v1).

  Contract (docs/32 sec.0/sec.2, ADR-057):  ASCII-only, because this file is
    * READ ONLY. No New-Item / Set-Content / registry write / service change.
      The only processes started are getters (`where.exe`, `curl.exe -I`,
      `rustc -vV`, `git config --list`) and one ranged `curl` sample that
      writes to NUL -- nothing lands on disk.
    * Everything it reports must be reproducible by re-running it; no value is
      typed in by hand.
    * Paths come from the workspace constant at the top, so a different machine
      only needs that one line changed.

  Usage (Windows):  powershell -NoProfile -ExecutionPolicy Bypass -File windows-env-current.ps1
  Usage (Linux):    tools/p5/windows-env.sh home -o fixtures/p5/windows-env-current.json
#>

$ErrorActionPreference = 'Continue'
try {
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
    $OutputEncoding = [Console]::OutputEncoding
} catch { }

# ------------------------------------------------------------------ inputs ---
$WS = 'E:\augenstern\translate'   # the only writable root (docs/32 sec.0)
$PY = 'D:\Anaconda3\python.exe'   # fixed interpreter (docs/32 sec.0.3)
$ORT_DIR = 'D:\Programs\onnx'     # existing ORT, read-only for us

# NOTE: the second parameter must NOT be named $args -- that is an
# automatic variable, and the call silently degrades to running the tool with
# no arguments (cargo prints "Rust's package manager", python opens a REPL).
function First-Line([string]$exe, [string]$arg) {
    try {
        $out = & $exe $arg 2>&1 | Out-String
        $l = ($out -split "`r?`n") | Where-Object { $_.Trim() -ne '' } | Select-Object -First 1
        return "$l".Trim()
    } catch { return $null }
}

function Get-RustupHome {
    try {
        $show = & rustup show 2>&1 | Out-String
        $m = [regex]::Match($show, 'rustup home:\s*(\S+)')
        if ($m.Success) { return $m.Groups[1].Value }
    } catch { }
    return $null
}

# ---------------------------------------------------------------- workspace --
$wsExists = Test-Path $WS
$wsChildren = @()
$wsGit = $false
$wsFree = $null
if ($wsExists) {
    $wsChildren = @(Get-ChildItem $WS -Force -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name)
    $wsGit = Test-Path (Join-Path $WS '.git')
    $drive = (Get-Item $WS).PSDrive.Name
    $wsFree = [math]::Round((Get-PSDrive $drive).Free / 1GB, 1)
}

# ---------------------------------------------------------------- toolchain --
$rustcVv = ''
try { $rustcVv = (& rustc -vV 2>&1 | Out-String) } catch { }
function FromVv([string]$key) {
    $m = [regex]::Match($rustcVv, "(?m)^" + $key + ":\s*(.+?)\s*$")
    if ($m.Success) { return $m.Groups[1].Value } else { return $null }
}
$toolchain = [ordered]@{
    rustc_version   = FromVv 'release'
    rustc_host      = FromVv 'host'
    commit_hash     = FromVv 'commit-hash'
    commit_date     = FromVv 'commit-date'
    llvm_version    = FromVv 'LLVM version'
    cargo_version   = (First-Line 'cargo' '-V')
    rustc_exe       = ((Get-Command rustc -ErrorAction SilentlyContinue).Source)
    cargo_exe       = ((Get-Command cargo -ErrorAction SilentlyContinue).Source)
    rustup_home     = (Get-RustupHome)
    rustup_toolchains = $(try { @(& rustup toolchain list 2>&1) } catch { @() })
    targets         = $(try { @(& rustup target list --installed 2>&1) } catch { @() })
    components      = $(try { @(& rustup component list --installed 2>&1 | ForEach-Object { ($_ -split '-')[0] }) } catch { @() })
}

# ------------------------------------------------------------------- python --
$pythonInfo = [ordered]@{ path = $PY; exists = (Test-Path $PY); version = $null }
if ($pythonInfo.exists) { $pythonInfo.version = ((& $PY '--version' 2>&1 | Out-String).Trim()) }
$pythonInfo.naked_python = $(try { @(& where.exe python 2>&1) } catch { @() })
$pythonInfo.launcher = $(try { @(& py -0p 2>&1) } catch { @() })

# --------------------------------------------------------------------- msvc --
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vcVer = $null
if (Test-Path $vswhere) {
    $vcVer = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationVersion 2>$null | Out-String).Trim()
    if ($vcVer -eq '') { $vcVer = $null }
}
$sdkLib = @()
foreach ($r in @("${env:ProgramFiles(x86)}\Windows Kits\10\Lib", "${env:ProgramFiles}\Windows Kits\10\Lib")) {
    if (Test-Path $r) { $sdkLib += @(Get-ChildItem $r -Directory -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name) }
}
$msvc = [ordered]@{
    vswhere          = $(if (Test-Path $vswhere) { $vswhere } else { $null })
    vc_tools_version = $vcVer
    sdk_lib_versions = $sdkLib
    cl_on_path       = ((Get-Command cl.exe   -ErrorAction SilentlyContinue).Source)
    link_on_path     = ((Get-Command link.exe -ErrorAction SilentlyContinue).Source)
}

# ---------------------------------------------------------------------- git --
$git = [ordered]@{
    version         = (First-Line 'git' '--version')
    exe             = ((Get-Command git -ErrorAction SilentlyContinue).Source)
    system_config   = $(try { @(& git config --system --list 2>&1) } catch { @() })
    global_config   = $(try { @(& git config --global --list 2>&1) } catch { @() })
    workspace_local = $(if ($wsExists -and $wsGit) { @(& git -C $WS config --local --list 2>&1) } else { @() })
}

# ---------------------------------------------------------------------- ort --
$ort = [ordered]@{ dir = $ORT_DIR; exists = (Test-Path $ORT_DIR); version = $null; lib_files = @() }
if ($ort.exists) {
    $verFile = Join-Path $ORT_DIR 'VERSION_NUMBER'
    if (Test-Path $verFile) { $ort.version = (Get-Content $verFile -Raw).Trim() }
    $libDir = Join-Path $ORT_DIR 'lib'
    if (Test-Path $libDir) {
        $ort.lib_files = @(Get-ChildItem $libDir -ErrorAction SilentlyContinue |
            Where-Object { $_.Name -like '*onnxruntime*' } |
            ForEach-Object { "$($_.Name) ($([math]::Round($_.Length/1MB,1)) MB)" })
    }
}

# ------------------------------------------------------------------ network --
# HEAD only (no body), so nothing is downloaded. The ranged sample is capped at
# 256 KiB / 15 s and writes to NUL, purely to distinguish "slow" from "blocked".
$targets = @(
    'https://static.rust-lang.org/dist/channel-rust-stable.toml',
    'https://index.crates.io/config.json',
    'https://cdn.pyke.io/0/pyke:ort-rs/ms@1.31.0/x86_64-pc-windows-msvc+directml.tar.lzma2'
)
$net = @()
foreach ($u in $targets) {
    $direct = & curl.exe -sS -I -o NUL -w '%{http_code}' --max-time 12 --noproxy '*' $u 2>&1
    $proxied = & curl.exe -sS -I -o NUL -w '%{http_code}' --max-time 12 $u 2>&1
    $net += [ordered]@{ url = $u; direct_code = "$direct".Trim(); via_env_proxy_code = "$proxied".Trim() }
}
$sample = & curl.exe -sS --noproxy '*' -r 0-262143 -o NUL -w '%{http_code} %{size_download} %{speed_download} %{time_total}' --max-time 20 $targets[2] 2>&1
$pykeSample = [ordered]@{ direct_range_0_256KiB = "$sample".Trim() }

# ------------------------------------------------------------------- policy --
# Everything a build would write outside the workspace unless redirected.
$writeRisk = [ordered]@{
    cargo_home_default   = (Join-Path $env:USERPROFILE '.cargo')
    cargo_home_env       = [Environment]::GetEnvironmentVariable('CARGO_HOME')
    rustup_home_default  = (Join-Path $env:USERPROFILE '.rustup')
    temp_default         = $env:TEMP
    anaconda_dir         = 'D:\Anaconda3'
    note_git_global      = 'git config --global/--system counts as an out-of-workspace write; approval first (docs/32 sec.2)'
}

$result = [ordered]@{
    schema       = 'dsh-win-env/v1'
    collected_at_utc = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    computer     = [ordered]@{
        name = $env:COMPUTERNAME
        user = "$env:USERDOMAIN\$env:USERNAME"
        ps_version = $PSVersionTable.PSVersion.ToString()
        elevated = $(try {
            $id = [Security.Principal.WindowsIdentity]::GetCurrent()
            (New-Object Security.Principal.WindowsPrincipal($id)).IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
        } catch { $null })
    }
    workspace    = [ordered]@{
        path = $WS
        exists = $wsExists
        empty = ($wsChildren.Count -eq 0)
        children = $wsChildren
        git_present = $wsGit
        free_gib = $wsFree
    }
    toolchain    = $toolchain
    python       = $pythonInfo
    msvc         = $msvc
    git          = $git
    ort          = $ort
    network      = $net
    pyke_sample  = $pykeSample
    write_risk   = $writeRisk
}

$json = $result | ConvertTo-Json -Depth 8
Write-Output 'DSH-ENV-BEGIN'
Write-Output $json
Write-Output 'DSH-ENV-END'
