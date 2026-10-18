# Fast inner loop. UI files (ui/) are served live: save a file and the window reloads - no Rust build.
# Rust changes rebuild incrementally in debug mode (seconds instead of minutes of LTO).
#   .\scripts\dev.ps1                # debug recorder + app with live UI
#   .\scripts\dev.ps1 -NoRecorder    # app only (talks to an already running recorder)
#   .\scripts\dev.ps1 -Inspect       # also opens WebView2's debug port 9222 on localhost, so the UI can be driven and
#                                    # screenshotted from outside (CDP). Off by default: any local process could use it.
# Note: a plain `cargo build -p buffer` copies ffmpeg.exe next to the debug exe; it fails while the debug recorder
# is running from there (os error 32). Use this script (or stop the recorder first).
# The installer is only for releases (build-installer.ps1 / publish.ps1).
param([switch]$NoRecorder, [switch]$Inspect)
$ErrorActionPreference = "Stop"
if ($Inspect) {
    $env:WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS = "--remote-debugging-port=9222"
    Write-Host "Inspect mode: UI debug port http://127.0.0.1:9222 is open (dev only)." -ForegroundColor Yellow
}
Set-Location (Split-Path -Parent $PSScriptRoot)
if (-not $NoRecorder) {
    # cargo must replace the executable during the build. Stop a previous debug
    # or installed recorder first, otherwise Windows keeps the file locked.
    $existingRecorder = Get-Process buffer-recorder -ErrorAction SilentlyContinue
    if ($existingRecorder) {
        $existingRecorder | Stop-Process -Force
        $existingRecorder | Wait-Process -Timeout 10 -ErrorAction SilentlyContinue
    }
    cargo build -p ml-recorder
    if ($LASTEXITCODE -ne 0) { throw "recorder build failed" }
    # The recorder prefers the ffmpeg next to it. Without it, it takes whatever ffmpeg is on PATH, and an older one
    # (7.x) hangs while opening the three audio inputs: every session ends "ffmpeg did not open its audio inputs".
    if (-not (Test-Path "vendor\ffmpeg\ffmpeg.exe")) { & "$PSScriptRoot\fetch-ffmpeg.ps1" }
    $debugFfmpeg = "target\debug\ffmpeg.exe"
    if (-not (Test-Path $debugFfmpeg) -or
        (Get-FileHash $debugFfmpeg).Hash -ne (Get-FileHash "vendor\ffmpeg\ffmpeg.exe").Hash) {
        Copy-Item "vendor\ffmpeg\ffmpeg.exe" $debugFfmpeg -Force
    }
    # Only one recorder may run (single-instance lock): replace an installed/old one with the debug build.
    Start-Process (Resolve-Path "target\debug\buffer-recorder.exe") -ArgumentList "run" -WindowStyle Hidden
}
# A fresh clone or worktree has no node_modules, and `npx tauri` then fails with "could not determine executable".
if (-not (Test-Path "node_modules\.bin\tauri.cmd")) {
    npm ci
    if ($LASTEXITCODE -ne 0) { throw "npm ci failed" }
}
npx tauri dev --config src-tauri/tauri.dev.conf.json

