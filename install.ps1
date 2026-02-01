# roon-rd Windows Installer Script
# Downloads and installs the latest roon-rd release from GitHub
#
# USAGE:
#   Option 1: Right-click this file and select "Run with PowerShell"
#   Option 2: Run in PowerShell: .\install.ps1
#   Option 3: One-liner (bypasses execution policy):
#             irm https://raw.githubusercontent.com/jdrivas/roon-rd/master/install.ps1 | iex
#
# To install to Program Files (requires admin):
#   .\install.ps1 -SystemWide
#
# EXECUTION POLICY:
#   If you get a security error, you can either:
#   - Use the one-liner above (recommended)
#   - Run: Set-ExecutionPolicy RemoteSigned -Scope CurrentUser
#   - Right-click the file and select "Run with PowerShell"

param(
    [switch]$SystemWide = $false
)

$ErrorActionPreference = "Stop"

$repoOwner = "jdrivas"
$repoName = "roon-rd"
$binaryName = "roon-rd-windows-x64.exe"
$exeName = "roon-rd.exe"

# Determine install location
if ($SystemWide) {
    $installDir = "$env:ProgramFiles\roon-rd"
} else {
    $installDir = "$env:LOCALAPPDATA\roon-rd"
}

Write-Host ""
Write-Host "=== roon-rd Windows Installer ===" -ForegroundColor Cyan
Write-Host ""
Write-Host "Note: If you downloaded this script and got a security error," -ForegroundColor Gray
Write-Host "      try: irm https://raw.githubusercontent.com/jdrivas/roon-rd/master/install.ps1 | iex" -ForegroundColor Gray
Write-Host ""

# Step 1: Stop any running roon-rd processes
Write-Host "Checking for running roon-rd processes..." -ForegroundColor Yellow
$processes = Get-Process -Name "roon-rd" -ErrorAction SilentlyContinue
if ($processes) {
    Write-Host "  Stopping roon-rd server..." -ForegroundColor Yellow
    $processes | Stop-Process -Force
    Start-Sleep -Seconds 2
    Write-Host "  Server stopped." -ForegroundColor Green
} else {
    Write-Host "  No running processes found." -ForegroundColor Gray
}

# Step 2: Get latest release info from GitHub
Write-Host ""
Write-Host "Fetching latest release from GitHub..." -ForegroundColor Yellow
$releaseUrl = "https://api.github.com/repos/$repoOwner/$repoName/releases/latest"

try {
    $release = Invoke-RestMethod -Uri $releaseUrl -Headers @{ "User-Agent" = "roon-rd-installer" }
    $version = $release.tag_name
    Write-Host "  Latest version: $version" -ForegroundColor Green
} catch {
    Write-Host "  Error: Failed to fetch release info from GitHub" -ForegroundColor Red
    Write-Host "  $_" -ForegroundColor Red
    exit 1
}

# Find the Windows binary asset
$asset = $release.assets | Where-Object { $_.name -eq $binaryName }
if (-not $asset) {
    Write-Host "  Error: Windows binary not found in release" -ForegroundColor Red
    exit 1
}

$downloadUrl = $asset.browser_download_url

# Step 3: Create install directory
Write-Host ""
Write-Host "Installing to: $installDir" -ForegroundColor Yellow

if (-not (Test-Path $installDir)) {
    try {
        New-Item -ItemType Directory -Path $installDir -Force | Out-Null
        Write-Host "  Created directory." -ForegroundColor Green
    } catch {
        Write-Host "  Error: Failed to create directory. Try running as Administrator." -ForegroundColor Red
        exit 1
    }
}

# Step 4: Download the binary
$exePath = Join-Path $installDir $exeName
$tempPath = Join-Path $env:TEMP "roon-rd-download.exe"

Write-Host ""
Write-Host "Downloading $binaryName..." -ForegroundColor Yellow
try {
    Invoke-WebRequest -Uri $downloadUrl -OutFile $tempPath -UseBasicParsing
    Write-Host "  Download complete." -ForegroundColor Green
} catch {
    Write-Host "  Error: Failed to download binary" -ForegroundColor Red
    Write-Host "  $_" -ForegroundColor Red
    exit 1
}

# Step 5: Move to install location
Write-Host ""
Write-Host "Installing executable..." -ForegroundColor Yellow
try {
    Move-Item -Path $tempPath -Destination $exePath -Force
    Write-Host "  Installed to: $exePath" -ForegroundColor Green
} catch {
    Write-Host "  Error: Failed to install. The file may be in use." -ForegroundColor Red
    Write-Host "  $_" -ForegroundColor Red
    exit 1
}

# Step 6: Add to PATH if not already there
$pathScope = if ($SystemWide) { "Machine" } else { "User" }
$currentPath = [Environment]::GetEnvironmentVariable("Path", $pathScope)

if ($currentPath -notlike "*$installDir*") {
    Write-Host ""
    Write-Host "Adding to PATH..." -ForegroundColor Yellow
    try {
        $newPath = "$currentPath;$installDir"
        [Environment]::SetEnvironmentVariable("Path", $newPath, $pathScope)
        Write-Host "  Added $installDir to $pathScope PATH" -ForegroundColor Green
        Write-Host "  Note: Restart your terminal for PATH changes to take effect." -ForegroundColor Yellow
    } catch {
        Write-Host "  Warning: Could not add to PATH automatically." -ForegroundColor Yellow
        Write-Host "  You can add $installDir to your PATH manually." -ForegroundColor Yellow
    }
} else {
    Write-Host ""
    Write-Host "Already in PATH." -ForegroundColor Gray
}

# Done!
Write-Host ""
Write-Host "=== Installation Complete ===" -ForegroundColor Green
Write-Host ""
Write-Host "  Version:  $version" -ForegroundColor Cyan
Write-Host "  Location: $exePath" -ForegroundColor Cyan
Write-Host ""
Write-Host "To start the server, open a new terminal and run:" -ForegroundColor White
Write-Host "  roon-rd server" -ForegroundColor Yellow
Write-Host ""
Write-Host "Or run directly:" -ForegroundColor White
Write-Host "  & '$exePath' server" -ForegroundColor Yellow
Write-Host ""
