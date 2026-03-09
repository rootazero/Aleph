# One-line installer: irm https://raw.githubusercontent.com/rootazero/Aleph/main/install.ps1 | iex
# With version:       $env:ALEPH_VERSION="v0.2.0"; irm ... | iex
$ErrorActionPreference = "Stop"

$Repo = "rootazero/Aleph"
$BinaryName = "aleph"
$Version = if ($env:ALEPH_VERSION) { $env:ALEPH_VERSION } else { "latest" }

# ── Detect architecture ─────────────────────────────────────────

$Arch = [System.Runtime.InteropServices.RuntimeInformation]::OSArchitecture
switch ($Arch) {
    "X64"   { $ArchName = "x86_64" }
    "Arm64" { $ArchName = "aarch64" }
    default { Write-Error "Unsupported architecture: $Arch"; exit 1 }
}

$AssetName = "$BinaryName-windows-$ArchName"
Write-Host "Detected platform: windows/$ArchName"

# ── Install directory ────────────────────────────────────────────

$InstallDir = "$env:LOCALAPPDATA\Aleph\bin"
if (-not (Test-Path $InstallDir)) {
    New-Item -ItemType Directory -Path $InstallDir -Force | Out-Null
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

    # Stop existing process if running
    Get-Process -Name $BinaryName -ErrorAction SilentlyContinue | Stop-Process -Force -ErrorAction SilentlyContinue

    Copy-Item $ExePath (Join-Path $InstallDir "$BinaryName.exe") -Force
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
$InstalledPath = Join-Path $InstallDir "$BinaryName.exe"
try {
    $InstalledVersion = & $InstalledPath --version 2>$null
} catch {
    $InstalledVersion = "unknown"
}

Write-Host ""
Write-Host "Aleph installed successfully! ($InstalledVersion)"
Write-Host "  Binary:  $InstalledPath"
Write-Host "  Config:  $ConfigDir"
Write-Host ""
Write-Host "Run:  aleph"

# ── Auto-start (Windows Task Scheduler) ──────────────────────────

$TaskName = "AlephServer"

# Check if task already exists
$ExistingTask = Get-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue

$InstallTask = $true
if ([Environment]::UserInteractive -and [Console]::IsInputRedirected -eq $false) {
    $Reply = Read-Host "Install as startup task (auto-start on login)? [Y/n]"
    if ($Reply -match "^[Nn]$") {
        $InstallTask = $false
    }
}

if ($InstallTask) {
    # Remove existing task if present
    if ($ExistingTask) {
        Unregister-ScheduledTask -TaskName $TaskName -Confirm:$false -ErrorAction SilentlyContinue
    }

    $Action = New-ScheduledTaskAction -Execute $InstalledPath
    $Trigger = New-ScheduledTaskTrigger -AtLogon
    $Settings = New-ScheduledTaskSettingsSet -AllowStartIfOnBatteries -DontStopIfGoingOnBatteries -StartWhenAvailable
    $Principal = New-ScheduledTaskPrincipal -UserId $env:USERNAME -LogonType Interactive -RunLevel Limited

    Register-ScheduledTask -TaskName $TaskName -Action $Action -Trigger $Trigger -Settings $Settings -Principal $Principal -Force | Out-Null

    # Start it now
    Start-ScheduledTask -TaskName $TaskName -ErrorAction SilentlyContinue

    Write-Host "Startup task installed (auto-start on login)."
    Write-Host "  Manage:  Get-ScheduledTask -TaskName $TaskName"
}
