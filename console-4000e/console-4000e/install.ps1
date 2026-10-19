# Installs the built 4000 E Console plugin into the standard system-wide VST3/CLAP folders on
# Windows. Run this from the project root AFTER `cargo nih-plug bundle console-4000e --release`
# has produced target\bundled\console-4000e.vst3 (and .clap).
#
# Usage (from a terminal in this folder):
#   powershell -ExecutionPolicy Bypass -File install.ps1

$ErrorActionPreference = "Stop"

$root = $PSScriptRoot
# Works both from the project folder (after a local build) and from an unzipped GitHub artifact.
$bundled = Join-Path $root "target\bundled"
if (-not (Test-Path (Join-Path $bundled "console-4000e.vst3"))) { $bundled = $root }
$vst3Src = Join-Path $bundled "console-4000e.vst3"
$clapSrc = Join-Path $bundled "console-4000e.clap"

$vst3Dest = "C:\Program Files\Common Files\VST3"
$clapDest = "C:\Program Files\Common Files\CLAP"

if (-not (Test-Path $vst3Src)) {
    Write-Host "Can't find $vst3Src" -ForegroundColor Red
    Write-Host "Run 'cargo nih-plug bundle console-4000e --release' first." -ForegroundColor Yellow
    exit 1
}

function Install-Plugin($src, $destDir, $name) {
    if (-not (Test-Path $destDir)) {
        try {
            New-Item -ItemType Directory -Path $destDir -Force | Out-Null
        } catch {
            Write-Host "Could not create $destDir (try running this script as Administrator)." -ForegroundColor Red
            return
        }
    }
    $dest = Join-Path $destDir (Split-Path $src -Leaf)
    try {
        Copy-Item -Path $src -Destination $dest -Recurse -Force
        Write-Host "Installed $name to $dest" -ForegroundColor Green
    } catch {
        Write-Host "Could not copy to $dest - try running this script as Administrator." -ForegroundColor Red
        Write-Host $_.Exception.Message -ForegroundColor Red
    }
}

Install-Plugin $vst3Src $vst3Dest "VST3"

if (Test-Path $clapSrc) {
    Install-Plugin $clapSrc $clapDest "CLAP"
} else {
    Write-Host "No .clap bundle found, skipping (VST3 install is enough for most DAWs)." -ForegroundColor Yellow
}

Write-Host ""
Write-Host "Done. Rescan plugins in your DAW (or restart it) and look for '4000 E Console'." -ForegroundColor Cyan
