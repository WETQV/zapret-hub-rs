param(
    [string]$BundlePath,
    [string]$BundleTag = "1.10.0",
    [string]$TelegramProxyTag = "v1.9.1"
)

$ErrorActionPreference = "Stop"

$env:LANG = "en_US.UTF-8"
$env:LC_ALL = "en_US.UTF-8"
$env:PYTHONIOENCODING = "utf-8"

$projectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$cargoToml = Join-Path $projectRoot "Cargo.toml"
$installerScript = Join-Path $projectRoot "installer\zapret-hub.iss"
$stageScript = Join-Path $projectRoot "packaging\stage-release.ps1"
$manifestScript = Join-Path $projectRoot "packaging\generate-update-manifest.ps1"

function Resolve-InnoSetupCompiler {
    $candidates = @(
        (Join-Path $env:ProgramFiles "Inno Setup 7\ISCC.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 7\ISCC.exe"),
        (Join-Path $env:ProgramFiles "Inno Setup 6\ISCC.exe"),
        (Join-Path ${env:ProgramFiles(x86)} "Inno Setup 6\ISCC.exe")
    )

    foreach ($candidate in $candidates) {
        if (Test-Path -LiteralPath $candidate) {
            return $candidate
        }
    }

    throw "Inno Setup compiler not found. Checked: $($candidates -join '; ')"
}

$isccPath = Resolve-InnoSetupCompiler

if (-not (Test-Path $isccPath)) {
    throw "Inno Setup compiler not found: $isccPath"
}

$versionLine = Select-String -Path $cargoToml -Pattern '^version\s*=\s*"(.+)"$' | Select-Object -First 1
if (-not $versionLine) {
    throw "Could not read package version from Cargo.toml"
}

$version = $versionLine.Matches[0].Groups[1].Value

$stageArgs = @{}
if ($PSBoundParameters.ContainsKey("BundlePath")) {
    $stageArgs.BundlePath = $BundlePath
}
$stageArgs.BundleTag = $BundleTag
$stageArgs.TelegramProxyTag = $TelegramProxyTag

& $stageScript @stageArgs

New-Item -ItemType Directory -Force -Path (Join-Path $projectRoot "dist\installer") | Out-Null

& $isccPath `
    "/DAppVersion=$version" `
    "/DSourceDir=$projectRoot\dist\stage" `
    $installerScript
if ($LASTEXITCODE -ne 0) {
    throw "Inno Setup compiler failed with exit code $LASTEXITCODE"
}

$installerFile = "zapret-hub-setup-$version.exe"

& $manifestScript `
    -OutputPath "dist\installer\latest.json" `
    -InstallerFileName $installerFile `
    -Version $version

Write-Host "Installer created in: $(Join-Path $projectRoot 'dist\installer')"
