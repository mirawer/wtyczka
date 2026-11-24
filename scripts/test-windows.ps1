param(
    [string]$Executable = "",
    [switch]$ShowTray
)

$ErrorActionPreference = "Stop"
if ([Environment]::OSVersion.Platform -ne [PlatformID]::Win32NT) {
    throw "Run this test on Windows 10/11, not on Linux."
}
if (-not [Environment]::Is64BitOperatingSystem) {
    throw "The distributed executable requires a 64-bit Windows installation."
}
$Root = Split-Path -Parent $PSScriptRoot
if (-not $Executable) {
    foreach ($Candidate in @(
        (Join-Path $Root "bin/screen-answer.exe"),
        (Join-Path $Root "target/release/screen-answer.exe"),
        (Join-Path $Root "target/x86_64-pc-windows-msvc/release/screen-answer.exe")
    )) {
        if (Test-Path -LiteralPath $Candidate -PathType Leaf) { $Executable = $Candidate; break }
    }
}
if (-not $Executable -or -not (Test-Path -LiteralPath $Executable -PathType Leaf)) {
    throw "Missing screen-answer.exe. Pass -Executable with its path."
}
$Executable = (Resolve-Path -LiteralPath $Executable).Path

function Invoke-Checked {
    param([string]$File, [string[]]$Arguments)
    & $File @Arguments
    if ($LASTEXITCODE -ne 0) { throw "$File exited with code $LASTEXITCODE" }
}

Invoke-Checked $Executable @("--version")
Invoke-Checked $Executable @("--help")
# Unit/IPC tests run in CI (or `cargo test`), not in the end-user download.

if ($ShowTray) {
    Invoke-Checked $Executable @("doctor")
    Write-Host "Check the notification area (including its overflow drawer): no frame or tooltip."
    Write-Host "Synthetic digits: 1'2'3. No screenshot, API key, API request, or hotkey registration."
    Invoke-Checked $Executable @("--selection", "multiple", "--mode", "digits", "demo", "1'2'3")
    Write-Host "Synthetic dots: groups 1'2'5'6, separated by apostrophes."
    Invoke-Checked $Executable @("--selection", "multiple", "--mode", "dots", "demo", "1'2'5'6")
    Write-Host "Tray demos completed. Appearance must be confirmed visually on this Windows machine."
} else {
    Write-Host "Tray/hotkey/capture runtime behavior was NOT checked. Use -ShowTray for visual demos."
}
Write-Host "Completed requested Windows checks. No external API was used."
