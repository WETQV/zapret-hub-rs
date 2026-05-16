param(
    [string]$BundlePath = "C:\Users\mejik\Downloads\zapret-discord-youtube-1.9.8c"
)

$ErrorActionPreference = "Stop"

$env:LANG = "en_US.UTF-8"
$env:LC_ALL = "en_US.UTF-8"
$env:PYTHONIOENCODING = "utf-8"

$projectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$cargoToml = Join-Path $projectRoot "Cargo.toml"
$isccPath = "C:\Program Files (x86)\Inno Setup 6\ISCC.exe"
$installerScript = Join-Path $projectRoot "installer\zapret-hub.iss"
$stageScript = Join-Path $projectRoot "packaging\stage-release.ps1"
$manifestScript = Join-Path $projectRoot "packaging\generate-update-manifest.ps1"

if (-not (Test-Path $isccPath)) {
    throw "Inno Setup compiler not found: $isccPath"
}

$versionLine = Select-String -Path $cargoToml -Pattern '^version\s*=\s*"(.+)"$' | Select-Object -First 1
if (-not $versionLine) {
    throw "Could not read package version from Cargo.toml"
}

$version = $versionLine.Matches[0].Groups[1].Value

& $stageScript -BundlePath $BundlePath

New-Item -ItemType Directory -Force -Path (Join-Path $projectRoot "dist\installer") | Out-Null

& $isccPath `
    "/DAppVersion=$version" `
    "/DSourceDir=$projectRoot\dist\stage" `
    $installerScript

$installerFile = "zapret-hub-setup-$version.exe"

& $manifestScript `
    -OutputPath "dist\installer\latest.json" `
    -InstallerFileName $installerFile `
    -Version $version

Write-Host "Installer created in: $(Join-Path $projectRoot 'dist\installer')"
