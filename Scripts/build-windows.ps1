# Build complete Windows application
# Usage: .\build-windows.ps1 [-Config Release|Debug]

param(
    [ValidateSet("Release", "Debug")]
    [string]$Config = "Release"
)

$ErrorActionPreference = "Stop"

$ScriptDir = Split-Path -Parent $MyInvocation.MyCommand.Path
$RootDir = Split-Path -Parent $ScriptDir
$CoreDir = Join-Path $RootDir "core"
$WindowsDir = Join-Path $RootDir "platforms/windows"

Write-Host "🪟 Building Windows app ($Config)..." -ForegroundColor Cyan

# Step 1: Build Rust core
Write-Host "📦 Building Rust core..." -ForegroundColor Yellow
Push-Location $CoreDir
try {
    if ($Config -eq "Debug") {
        cargo build --features cabi
        $LibPath = "target/debug/aethecore.dll"
    } else {
        cargo build --release --features cabi
        $LibPath = "target/release/aethecore.dll"
    }

    # Copy DLL
    $DestDir = Join-Path $WindowsDir "Aether/libs"
    if (-not (Test-Path $DestDir)) {
        New-Item -ItemType Directory -Path $DestDir -Force | Out-Null
    }
    Copy-Item $LibPath -Destination $DestDir -Force
    Write-Host "✅ Rust core built and copied" -ForegroundColor Green
}
finally {
    Pop-Location
}

# Step 2: Build .NET application
Write-Host "🏗️ Building .NET application..." -ForegroundColor Yellow
Push-Location $WindowsDir
try {
    dotnet build -c $Config
    Write-Host "✅ .NET build complete" -ForegroundColor Green
}
finally {
    Pop-Location
}

Write-Host "✅ Windows build complete!" -ForegroundColor Green
