param(
    [string]$BundlePath = "C:\Users\mejik\Downloads\zapret-discord-youtube-1.9.7b",
    [string]$StageRoot = "dist\stage"
)

$ErrorActionPreference = "Stop"

$env:LANG = "en_US.UTF-8"
$env:LC_ALL = "en_US.UTF-8"
$env:PYTHONIOENCODING = "utf-8"

if (-not (Test-Path $BundlePath)) {
    throw "Bundle path not found: $BundlePath"
}

$projectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$stagePath = Join-Path $projectRoot $StageRoot
$bundleTarget = Join-Path $stagePath "bundle"
$exeSource = Join-Path $projectRoot "target\release\zapret-hub-rs.exe"
$exeTarget = Join-Path $stagePath "Zapret Hub.exe"
$whitelistSource = Join-Path $projectRoot "assets\builtin-whitelist.txt"
$whitelistTarget = Join-Path $stagePath "builtin-whitelist.txt"
$tgProxyReleaseApi = "https://api.github.com/repos/Flowseal/tg-ws-proxy/releases/latest"
$tgProxyAssetName = "TgWsProxy_windows.exe"
$tgProxyVersionFileName = "TgWsProxy_windows.version.json"
$bundleVersionFileName = "ZapretBundle.version.json"
$telegramProxyScript = @"
@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."
set "TG_PROXY_ARGS=--dc-ip 2:149.154.167.220 --dc-ip 4:149.154.167.220 --dc-ip 203:149.154.167.220"

if not exist "%ROOT%\TgWsProxy_windows.exe" (
    exit /b 1
)

tasklist /FI "IMAGENAME eq TgWsProxy_windows.exe" | find /I "TgWsProxy_windows.exe" > nul
if errorlevel 1 (
    start "" /B "%ROOT%\TgWsProxy_windows.exe" %TG_PROXY_ARGS%
)
"@
$telegramProxySilentScript = @"
@echo off
chcp 65001 > nul
cd /d "%~dp0"

start "" /min "%~dp0telegram_proxy.cmd"
"@

function Update-TextFile {
    param(
        [string]$Path,
        [hashtable]$Replacements
    )

    if (-not (Test-Path $Path)) {
        throw "File not found for patching: $Path"
    }

    $content = [System.IO.File]::ReadAllText($Path, [System.Text.Encoding]::UTF8)

    foreach ($entry in $Replacements.GetEnumerator()) {
        $content = $content.Replace($entry.Key, $entry.Value)
    }

    [System.IO.File]::WriteAllText($Path, $content, [System.Text.Encoding]::UTF8)
}

function Patch-StagedBundle {
    param([string]$BundleRoot)

    $profileScripts = @(
        "general (SIMPLE FAKE ALT2).bat",
        "general (ALT11).bat",
        "general (FAKE TLS AUTO ALT3).bat",
        "general (ALT7).bat"
    )

    foreach ($scriptName in $profileScripts) {
        $scriptPath = Join-Path $BundleRoot $scriptName
        Update-TextFile -Path $scriptPath -Replacements @{
            'start "zapret: %~n0" /min "%BIN%winws.exe"' = 'start "" /B "%BIN%winws.exe"'
        }
    }

    [System.IO.File]::WriteAllText(
        (Join-Path $BundleRoot "hub\telegram_proxy.cmd"),
        $telegramProxyScript,
        [System.Text.Encoding]::UTF8
    )
    [System.IO.File]::WriteAllText(
        (Join-Path $BundleRoot "hub\start_telegram_proxy_silent.cmd"),
        $telegramProxySilentScript,
        [System.Text.Encoding]::UTF8
    )
}

function Update-StagedTelegramProxy {
    param([string]$BundleRoot)

    $headers = @{
        "User-Agent" = "zapret-hub-rs-packaging"
        "Accept" = "application/vnd.github+json"
    }

    $release = Invoke-RestMethod -Headers $headers -Uri $tgProxyReleaseApi
    $asset = $release.assets | Where-Object { $_.name -eq $tgProxyAssetName } | Select-Object -First 1

    if (-not $asset) {
        throw "Latest tg-ws-proxy Windows asset not found in GitHub release metadata"
    }

    $targetPath = Join-Path $BundleRoot $tgProxyAssetName
    Invoke-WebRequest -Headers $headers -Uri $asset.browser_download_url -OutFile $targetPath

    $versionInfo = @{
        tag = $release.tag_name
        release_url = $release.html_url
        asset_url = $asset.browser_download_url
        digest = $asset.digest
    } | ConvertTo-Json -Depth 4

    [System.IO.File]::WriteAllText(
        (Join-Path $BundleRoot $tgProxyVersionFileName),
        $versionInfo,
        [System.Text.Encoding]::UTF8
    )
}

function Write-BundleVersionMetadata {
    param(
        [string]$BundleRoot,
        [string]$BundleSourcePath
    )

    $bundleFolderName = Split-Path $BundleSourcePath -Leaf
    $version = $bundleFolderName
    if ($version -like "zapret-discord-youtube-*") {
        $version = $version.Substring("zapret-discord-youtube-".Length)
    }

    $bundleInfo = @{
        version = $version
        source_folder = $bundleFolderName
    } | ConvertTo-Json -Depth 3

    [System.IO.File]::WriteAllText(
        (Join-Path $BundleRoot $bundleVersionFileName),
        $bundleInfo,
        [System.Text.Encoding]::UTF8
    )
}

if (Test-Path $stagePath) {
    Remove-Item -Recurse -Force $stagePath
}

New-Item -ItemType Directory -Force -Path $bundleTarget | Out-Null

Push-Location $projectRoot
try {
    cargo build --release
}
finally {
    Pop-Location
}

if (-not (Test-Path $exeSource)) {
    throw "Release executable not found: $exeSource"
}

Copy-Item $exeSource $exeTarget -Force
Copy-Item $whitelistSource $whitelistTarget -Force
Copy-Item (Join-Path $BundlePath "*") $bundleTarget -Recurse -Force
Write-BundleVersionMetadata -BundleRoot $bundleTarget -BundleSourcePath $BundlePath
Update-StagedTelegramProxy -BundleRoot $bundleTarget
Patch-StagedBundle -BundleRoot $bundleTarget

Write-Host "Staged application at: $stagePath"
