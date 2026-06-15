# Aleph server installer (Windows) — downloads the standalone aleph-server.exe.
# Usage: irm https://github.com/rootazero/Aleph/releases/latest/download/install.ps1 | iex
#
# Windows analogue of scripts/install.sh. Mirrors its behaviour: resolve the
# release asset for this host, download it, drop it where the App's daemon also
# lives ($LOCALAPPDATA\Aleph), and print start + LAN guidance.
$ErrorActionPreference = 'Stop'
$ProgressPreference = 'SilentlyContinue'  # Invoke-WebRequest is glacial with the progress bar on.
[Net.ServicePointManager]::SecurityProtocol = [Net.SecurityProtocolType]::Tls12  # PS 5.1 default is TLS 1.0.

$repo = if ($env:ALEPH_REPO) { $env:ALEPH_REPO } else { 'rootazero/Aleph' }
$version = if ($env:ALEPH_VERSION) { $env:ALEPH_VERSION } else { 'latest' }

# Asset names mirror the host triples the release workflow uploads
# (desktop/shell/binaries/aleph-server-<triple>.exe). Only x86_64 ships a
# Windows server binary today; arm64 must build from source.
$arch = $env:PROCESSOR_ARCHITECTURE
if ($arch -ne 'AMD64') {
    Write-Error "Unsupported architecture: $arch (only x86_64/AMD64 ships a prebuilt server; build from source)"
}
$asset = 'aleph-server-x86_64-pc-windows-msvc.exe'

$url = if ($version -eq 'latest') {
    "https://github.com/$repo/releases/latest/download/$asset"
} else {
    "https://github.com/$repo/releases/download/$version/$asset"
}

$destDir = Join-Path $env:LOCALAPPDATA 'Aleph'
$destExe = Join-Path $destDir 'aleph-server.exe'
New-Item -ItemType Directory -Force -Path $destDir | Out-Null

# Windows cannot overwrite a running .exe — stop any daemon already on this path first.
if (Test-Path $destExe) {
    try { & $destExe stop 2>$null } catch {}
}

Write-Host "Downloading $asset -> $destExe"
Invoke-WebRequest -Uri $url -OutFile $destExe

# Put the install dir on the user PATH (idempotent) so `aleph-server` is callable.
$userPath = [Environment]::GetEnvironmentVariable('Path', 'User')
if (-not ($userPath -split ';' | Where-Object { $_ -eq $destDir })) {
    $newPath = if ([string]::IsNullOrEmpty($userPath)) { $destDir } else { "$userPath;$destDir" }
    [Environment]::SetEnvironmentVariable('Path', $newPath, 'User')
    $env:Path = "$env:Path;$destDir"
    Write-Host "Added $destDir to your user PATH (restart the terminal to pick it up everywhere)."
}

Write-Host "Installed. Start it with:  aleph-server start"
Write-Host 'LAN access: set [gateway] host = "0.0.0.0" in ~/.aleph/config.toml (trusts your whole LAN).'
