param(
    [string]$OutputPath = "dist\installer\latest.json",
    [string]$InstallerFileName,
    [string]$Version
)

$ErrorActionPreference = "Stop"

$env:LANG = "en_US.UTF-8"
$env:LC_ALL = "en_US.UTF-8"
$env:PYTHONIOENCODING = "utf-8"

if ([string]::IsNullOrWhiteSpace($InstallerFileName)) {
    throw "InstallerFileName is required."
}

if ([string]::IsNullOrWhiteSpace($Version)) {
    throw "Version is required."
}

$projectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$targetPath = Join-Path $projectRoot $OutputPath
$targetDir = Split-Path -Parent $targetPath

New-Item -ItemType Directory -Force -Path $targetDir | Out-Null

$manifest = [ordered]@{
    product = "Zapret Hub"
    version = $Version
    installer = $InstallerFileName
    published_at = (Get-Date).ToString("yyyy-MM-ddTHH:mm:ssK")
    notes = "Installer update package"
}

$manifest | ConvertTo-Json | Set-Content -Path $targetPath -Encoding utf8

Write-Host "Update manifest written to: $targetPath"
