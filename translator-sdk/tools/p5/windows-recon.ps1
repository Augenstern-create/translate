<#
  windows-recon.ps1 -- read-only environment probe for the P5 Windows
  verification matrix (see docs/30).

  Design rules:
    * READ ONLY.  No installs, no registry writes, no service changes.
      Every cmdlet used here is a getter; the only process it starts is
      `where.exe`/`cmd /c chcp`-style queries and `rustc/cargo --version`.
    * PowerShell 5.1 compatible (no `??`, no ternary, no `-AsHashtable`).
    * ASCII only in this file, so the script itself cannot be mangled by a
      console code page when it is pasted into a remote session.
    * Emits one JSON object wrapped in DSH-RECON-BEGIN / DSH-RECON-END
      markers.  The markers let the caller strip any banner/MOTD that the
      SSH server prepends to stdout.

  Usage (on the Windows box, no admin required -- elevation is *reported*):
      powershell -NoProfile -ExecutionPolicy Bypass -File windows-recon.ps1
  or, if script execution is blocked by policy:
      powershell -NoProfile -Command "iex (Get-Content .\windows-recon.ps1 -Raw)"
#>

$ErrorActionPreference = 'Continue'
$ProgressPreference = 'SilentlyContinue'

# Process-local only (no machine/registry change): make stdout UTF-8 so that a
# localized OS caption ("Microsoft Windows 11 家庭版") survives the SSH pipe.
try {
    [Console]::OutputEncoding = New-Object System.Text.UTF8Encoding $false
    $OutputEncoding = [Console]::OutputEncoding
} catch { }

function Test-Cmd([string]$name) {
    $c = Get-Command $name -ErrorAction SilentlyContinue
    if ($null -ne $c) { return $true } else { return $false }
}

function Get-FirstLine([string]$exe, [string]$exeArgs) {
    try {
        $out = & $exe $exeArgs 2>&1 | Out-String
        $first = ($out -split "`r?`n") | Where-Object { $_.Trim() -ne '' } | Select-Object -First 1
        return "$first".Trim()
    } catch {
        return "ERROR: " + $_.Exception.Message
    }
}

# ---------------------------------------------------------------- OS / CPU --
$os   = Get-CimInstance Win32_OperatingSystem
$cs   = Get-CimInstance Win32_ComputerSystem
$cpu  = Get-CimInstance Win32_Processor | Select-Object -First 1

$displayVersion = $null
try {
    $displayVersion = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -Name DisplayVersion -ErrorAction Stop).DisplayVersion
} catch { $displayVersion = $null }

$productName = $null
try {
    $productName = (Get-ItemProperty 'HKLM:\SOFTWARE\Microsoft\Windows NT\CurrentVersion' -Name ProductName -ErrorAction Stop).ProductName
} catch { $productName = $null }

$longPaths = $null
try {
    $longPaths = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\FileSystem' -Name LongPathsEnabled -ErrorAction Stop).LongPathsEnabled
} catch { $longPaths = $null }

$utf8Ansi = $null
try {
    $utf8Ansi = (Get-ItemProperty 'HKLM:\SYSTEM\CurrentControlSet\Control\Nls\CodePage' -Name ACP -ErrorAction Stop).ACP
} catch { $utf8Ansi = $null }

# ------------------------------------------------------------------ volumes --
$volumes = @()
try {
    $volumes = Get-CimInstance Win32_LogicalDisk -Filter 'DriveType=3' | ForEach-Object {
        [ordered]@{
            drive      = $_.DeviceID
            fs         = $_.FileSystem
            size_gib   = [math]::Round($_.Size / 1GB, 1)
            free_gib   = [math]::Round($_.FreeSpace / 1GB, 1)
        }
    }
} catch { }

# ------------------------------------------------------------------- tools ---
$vswhere = Join-Path ${env:ProgramFiles(x86)} 'Microsoft Visual Studio\Installer\vswhere.exe'
$vsInstalls = @()
if (Test-Path $vswhere) {
    try {
        $vsInstalls = & $vswhere -all -prerelease -format json 2>$null | Out-String | ConvertFrom-Json
        $vsInstalls = $vsInstalls | ForEach-Object {
            [ordered]@{
                displayName = $_.displayName
                version     = $_.installationVersion
                path        = $_.installationPath
            }
        }
    } catch { $vsInstalls = @() }
}

$sdkRoots = @()
foreach ($r in @("${env:ProgramFiles(x86)}\Windows Kits\10\Lib", "${env:ProgramFiles}\Windows Kits\10\Lib")) {
    if (Test-Path $r) {
        $sdkRoots += @(Get-ChildItem $r -Directory -ErrorAction SilentlyContinue | Select-Object -ExpandProperty Name)
    }
}

# `link.exe` is normally NOT on PATH (it lives in the VS install dir); the
# authoritative "is the MSVC x64 toolset installed" probe is vswhere with the
# component requirement, which is what the `cc` crate and rustc do too.
$vcToolsVersion = $null
$vcToolsPath = $null
if (Test-Path $vswhere) {
    try {
        $vcToolsVersion = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationVersion 2>$null | Out-String).Trim()
        $vcToolsPath    = (& $vswhere -latest -products * -requires Microsoft.VisualStudio.Component.VC.Tools.x86.x64 -property installationPath 2>$null | Out-String).Trim()
        if ($vcToolsVersion -eq '') { $vcToolsVersion = $null }
        if ($vcToolsPath -eq '')    { $vcToolsPath = $null }
    } catch { }
}

$linkExe = Get-Command link.exe -ErrorAction SilentlyContinue
$clExe   = Get-Command cl.exe   -ErrorAction SilentlyContinue

# ------------------------------------------------------------------- rust ----
$rust = [ordered]@{ cargo = $null; rustc = $null; rustup = $null; targets = @(); components = @(); default_toolchain = $null }
if (Test-Cmd 'cargo')  { $rust.cargo  = (Get-Command cargo).Source;  $rust.cargo_version  = Get-FirstLine 'cargo'  '--version' }
if (Test-Cmd 'rustc')  { $rust.rustc  = (Get-Command rustc).Source;  $rust.rustc_version  = Get-FirstLine 'rustc'  '--version' }
if (Test-Cmd 'rustup') {
    $rust.rustup = (Get-Command rustup).Source
    try {
        $t = & rustup target list --installed 2>&1 | Out-String
        $rust.targets = @($t -split "`r?`n" | Where-Object { $_.Trim() -ne '' })
    } catch { }
    try {
        $c = & rustup component list --installed 2>&1 | Out-String
        $rust.components = @($c -split "`r?`n" | Where-Object { $_.Trim() -ne '' })
    } catch { }
    try { $rust.default_toolchain = (Get-FirstLine 'rustup' 'show active-toolchain') } catch { }
    # `rustc --version` does not print the host triple; the MSVC-vs-GNU decision
    # depends on it, so pull it out of `rustc -vV`.
    try {
        $vv = (& rustc -vV 2>&1 | Out-String)
        $m = [regex]::Match($vv, '(?m)^host:\s*(\S+)')
        if ($m.Success) { $rust.host = $m.Groups[1].Value }
    } catch { }
}

# ------------------------------------------------------------------- ssh -----
$sshdSvc = $null
try {
    $s = Get-Service -Name sshd -ErrorAction Stop
    $sshdSvc = [ordered]@{ status = "$($s.Status)"; startType = "$($s.StartType)" }
} catch { $sshdSvc = $null }

$listening22 = @()
try {
    $listening22 = Get-NetTCPConnection -LocalPort 22 -State Listen -ErrorAction Stop |
        ForEach-Object { "$($_.LocalAddress):22" }
} catch { }

# -------------------------------------------------------------- defender -----
$defender = [ordered]@{ realtime = $null; exclusions_path = @(); exclusions_process = @() }
try {
    $mp = Get-MpPreference -ErrorAction Stop
    $defender.realtime = $true
    $defender.exclusions_path    = @($mp.ExclusionPath)
    $defender.exclusions_process = @($mp.ExclusionProcess)
} catch { }
try {
    $st = Get-MpComputerStatus -ErrorAction Stop
    $defender.realtime = "$($st.RealTimeProtectionEnabled)"
} catch { }

# ------------------------------------------------------------------- net -----
$ipv4 = @()
try {
    $ipv4 = Get-NetIPAddress -AddressFamily IPv4 -ErrorAction Stop |
        Where-Object { $_.IPAddress -ne '127.0.0.1' } |
        ForEach-Object { "$($_.IPAddress)/$($_.PrefixLength) ($($_.InterfaceAlias))" }
} catch { }

# ------------------------------------------------------------- elevation -----
$isAdmin = $false
try {
    $id = [Security.Principal.WindowsIdentity]::GetCurrent()
    $pr = New-Object Security.Principal.WindowsPrincipal($id)
    $isAdmin = $pr.IsInRole([Security.Principal.WindowsBuiltInRole]::Administrator)
} catch { }

$wsl = $false
if (Test-Cmd 'wsl') {
    try { $null = & wsl --status 2>&1; $wsl = $true } catch { $wsl = $false }
}

$result = [ordered]@{
    schema = 'dsh-win-recon/v1'
    collected_at_utc = (Get-Date).ToUniversalTime().ToString('yyyy-MM-ddTHH:mm:ssZ')
    os = [ordered]@{
        product_name   = $productName
        caption        = $os.Caption
        version        = $os.Version
        build          = $os.BuildNumber
        display_version = $displayVersion
        architecture   = $os.OSArchitecture
        install_date   = "$($os.InstallDate)"
        last_boot      = "$($os.LastBootUpTime)"
    }
    host = [ordered]@{
        computer_name = $env:COMPUTERNAME
        user          = "$env:USERDOMAIN\$env:USERNAME"
        elevated      = $isAdmin
        ps_version    = $PSVersionTable.PSVersion.ToString()
        clr           = $PSVersionTable.CLRVersion
        culture       = (Get-Culture).Name
        ui_culture    = (Get-UICulture).Name
        system_locale = (Get-WinSystemLocale).Name
        acp           = $utf8Ansi
        long_paths_enabled = $longPaths
    }
    cpu = [ordered]@{
        name   = $cpu.Name
        cores  = $cpu.NumberOfCores
        logical = $cpu.NumberOfLogicalProcessors
        max_mhz = $cpu.MaxClockSpeed
        virt_firmware_enabled = $cpu.VirtualizationFirmwareEnabled
        slat = $cpu.SecondLevelAddressTranslationExtensions
    }
    memory = [ordered]@{
        total_gib = [math]::Round($cs.TotalPhysicalMemory / 1GB, 1)
        free_gib  = [math]::Round($os.FreePhysicalMemory / 1MB, 1)
    }
    volumes = $volumes
    tools = [ordered]@{
        git    = (Get-FirstLine 'git' '--version')
        curl   = (Get-FirstLine 'curl' '--version')
        python = (Get-FirstLine 'python' '--version')
        cmake  = (Get-FirstLine 'cmake' '--version')
        node   = (Get-FirstLine 'node' '--version')
        tar    = (Get-FirstLine 'tar' '--version')
        link_exe = $(if ($null -ne $linkExe) { $linkExe.Source } else { $null })
        cl_exe   = $(if ($null -ne $clExe)   { $clExe.Source }   else { $null })
    }
    vs = [ordered]@{
        vswhere          = $(if (Test-Path $vswhere) { $vswhere } else { $null })
        installs         = @($vsInstalls)
        vc_tools_version = $vcToolsVersion
        vc_tools_path    = $vcToolsPath
    }
    windows_sdk_lib_versions = $sdkRoots
    rust = $rust
    ssh = [ordered]@{
        sshd_service   = $sshdSvc
        listening_22   = $listening22
        ssh_client     = (Get-FirstLine 'ssh' '-V')
    }
    defender = $defender
    features = [ordered]@{
        wsl_present = $wsl
        docker      = (Test-Cmd 'docker')
        choco       = (Test-Cmd 'choco')
        winget      = (Test-Cmd 'winget')
    }
    network = [ordered]@{ ipv4 = $ipv4 }
}

$json = $result | ConvertTo-Json -Depth 6

Write-Output 'DSH-RECON-BEGIN'
Write-Output $json
Write-Output 'DSH-RECON-END'
