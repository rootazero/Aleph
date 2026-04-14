# One-line installer: irm https://raw.githubusercontent.com/rootazero/Aleph/main/install.ps1 | iex
# With version:       $env:ALEPH_VERSION="v0.2.10"; irm ... | iex
param(
    [switch]$SkipRuntime
)
$ErrorActionPreference = "Stop"

$Repo = "rootazero/Aleph"
$BinaryName = "aleph-server"
$Version = if ($env:ALEPH_VERSION) { $env:ALEPH_VERSION } else { "latest" }

# ── Detect architecture ─────────────────────────────────────────

$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ($Arch) {
    "X64"   { $ArchName = "x86_64" }
    "Arm64" { $ArchName = "aarch64" }
    default { Write-Error "Unsupported architecture: $Arch"; exit 1 }
}

$AssetName = "aleph-windows-$ArchName"

# ── Install directory ────────────────────────────────────────────

$InstallDir = "$env:LOCALAPPDATA\Aleph\bin"
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
}

# ── Check existing installation ──────────────────────────────────

$InstalledPath = Join-Path $InstallDir "$BinaryName.exe"
$IsUpgrade = Test-Path $InstalledPath
$CurrentVersion = "unknown"
if ($IsUpgrade) {
    try { $CurrentVersion = & $InstalledPath --version 2>$null } catch {}
    Write-Host "Existing installation found: $CurrentVersion"
    Write-Host "Upgrading..."
} else {
    Write-Host "Fresh install on windows/$ArchName"
}

# ── Fetch release info ──────────────────────────────────────────

if ($Version -eq "latest") {
    $ReleaseUrl = "https://api.github.com/repos/$Repo/releases/latest"
    Write-Host "Fetching latest release..."
} else {
    $ReleaseUrl = "https://api.github.com/repos/$Repo/releases/tags/$Version"
    Write-Host "Fetching release $Version..."
}

try {
    $Release = Invoke-RestMethod -Uri $ReleaseUrl -Headers @{ "User-Agent" = "Aleph-Installer" }
} catch {
    Write-Error "Failed to fetch release info. Check your network and that the release exists."
    exit 1
}

# Find .zip asset
$Asset = $Release.assets | Where-Object { $_.name -eq "$AssetName.zip" } | Select-Object -First 1
if (-not $Asset) {
    Write-Error "No binary found for $AssetName.zip in this release."
    Write-Host "Available assets:"
    $Release.assets | ForEach-Object { Write-Host "  $($_.name)" }
    exit 1
}

# ── Download and extract ─────────────────────────────────────────

$TmpDir = Join-Path $env:TEMP "aleph-install-$(Get-Random)"
New-Item -ItemType Directory -Path $TmpDir -Force | Out-Null

try {
    $ZipPath = Join-Path $TmpDir "$AssetName.zip"
    Write-Host "Downloading $AssetName..."
    Invoke-WebRequest -Uri $Asset.browser_download_url -OutFile $ZipPath -UseBasicParsing

    Expand-Archive -Path $ZipPath -DestinationPath $TmpDir -Force

    $ExePath = Join-Path $TmpDir "$BinaryName.exe"
    if (-not (Test-Path $ExePath)) {
        Write-Error "Could not find $BinaryName.exe in archive."
        exit 1
    }

    # Stop existing process before replacing binary
    Get-Process -Name $BinaryName -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue
    Start-Sleep -Seconds 1

    Copy-Item $ExePath $InstalledPath -Force
} finally {
    Remove-Item $TmpDir -Recurse -Force -ErrorAction SilentlyContinue
}

# ── Add to PATH ──────────────────────────────────────────────────

$UserPath = [Environment]::GetEnvironmentVariable("Path", "User")
if ($UserPath -notlike "*$InstallDir*") {
    [Environment]::SetEnvironmentVariable("Path", "$UserPath;$InstallDir", "User")
    $env:Path = "$env:Path;$InstallDir"
    Write-Host "Added $InstallDir to user PATH."
}

# Create config directory
$ConfigDir = Join-Path $env:USERPROFILE ".aleph"
if (-not (Test-Path $ConfigDir)) {
    New-Item -ItemType Directory -Path $ConfigDir -Force | Out-Null
}

# Verify installation
try {
    $InstalledVersion = & $InstalledPath --version 2>$null
} catch {
    $InstalledVersion = "unknown"
}

Write-Host ""
if ($IsUpgrade) {
    Write-Host "Aleph upgraded successfully! ($CurrentVersion -> $InstalledVersion)"
} else {
    Write-Host "Aleph installed successfully! ($InstalledVersion)"
}
Write-Host "  Server:  $InstalledPath"
Write-Host "  Config:  $ConfigDir"

# ── Auto-start (Windows Task Scheduler) ──────────────────────────

$TaskName = "AlephServer"

function Install-AlephService {
    # Remove existing task if present (handles upgrade)
    $ExistingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
    if ($ExistingTask) {
        # Stop running task before unregistering
        Stop-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    }

    $Action = New-ScheduledTaskAction -Execute $InstalledPath -Argument "start"
    $Trigger = New-ScheduledTaskTrigger -AtLogon
    $Settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable
    $Principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited

    Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null

    # Start it now
    Start-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue

    Write-Host ""
    Write-Host "Service installed (auto-start on login)."
    Write-Host "  Status:  Get-ScheduledTask -TaskName $TaskName"
    Write-Host "  Stop:    Stop-ScheduledTask -TaskName $TaskName"
    Write-Host "  Start:   Start-ScheduledTask -TaskName $TaskName"
    Write-Host "  Remove:  Unregister-ScheduledTask -TaskName $TaskName"
}

# ── Bootstrap runtime dependencies ───────────────────────────────

$RuntimeSkip = $SkipRuntime.IsPresent -or ($env:ALEPH_SKIP_RUNTIME -eq "1")

if ($RuntimeSkip) {
    Write-Host ""
    Write-Host "Skipping runtime bootstrap (-SkipRuntime or `$env:ALEPH_SKIP_RUNTIME=1)."
    Write-Host "Run 'aleph-server bootstrap-runtime' later, or use Panel -> Settings -> Runtime."
} else {
    Write-Host ""
    Write-Host "Bootstrapping runtime dependencies (fnm -> Node LTS -> uv -> @playwright/cli + Chromium)..."
    Write-Host "(Pass -SkipRuntime or set `$env:ALEPH_SKIP_RUNTIME='1' to skip.)"
    Write-Host ""
    $proc = Start-Process -FilePath $InstalledPath `
        -ArgumentList "bootstrap-runtime", "--best-effort" `
        -NoNewWindow -Wait -PassThru
    if ($proc.ExitCode -ne 0) {
        Write-Host ""
        Write-Host "Runtime bootstrap hit errors. Aleph will still install." -ForegroundColor Yellow
        Write-Host "   Fix and retry via: aleph-server bootstrap-runtime"
        Write-Host "   Or open Panel -> Settings -> Runtime for GUI."
    }
}

# On upgrade: always reinstall service (picks up new binary + start arg)
# On fresh install: ask in interactive mode, auto-install in pipe mode
if ($IsUpgrade) {
    Install-AlephService
} elseif ([Environment]::UserInteractive -and [Console]::IsInputRedirected -eq $false) {
    $Reply = Read-Host "Install as startup task (auto-start on login)? [Y/n]"
    if ($Reply -notmatch "^[Nn]$") {
        Install-AlephService
    }
} else {
    Install-AlephService
}
