param(
    [string]$BundlePath = "C:\Users\mejik\Downloads\zapret-discord-youtube-1.9.8c",
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
$githubHostlistEntries = @(
    "github.com",
    "www.github.com",
    "gist.github.com",
    "api.github.com",
    "github.githubassets.com",
    "githubassets.com",
    "githubusercontent.com",
    "raw.githubusercontent.com",
    "objects.githubusercontent.com",
    "avatars.githubusercontent.com",
    "camo.githubusercontent.com",
    "user-images.githubusercontent.com"
)
$utf8NoBom = New-Object System.Text.UTF8Encoding($false)
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

function Write-Utf8Script {
    param(
        [string]$Path,
        [string]$Content
    )

    [System.IO.File]::WriteAllText($Path, $Content, [System.Text.Encoding]::UTF8)
}

function Ensure-HubScripts {
    param([string]$BundleRoot)

    $hubDir = Join-Path $BundleRoot "hub"
    New-Item -ItemType Directory -Force -Path $hubDir | Out-Null

    $profileScripts = @{
        "run_full_simple_fake_alt2.cmd" = "general (SIMPLE FAKE ALT2).bat"
        "run_full_alt11.cmd" = "general (ALT11).bat"
        "run_full_fake_tls_auto_alt3.cmd" = "general (FAKE TLS AUTO ALT3).bat"
        "run_full_alt7.cmd" = "general (ALT7).bat"
    }

    foreach ($entry in $profileScripts.GetEnumerator()) {
        $scriptContent = @"
@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."

start "" "%ROOT%\$($entry.Value)"
"@
        Write-Utf8Script -Path (Join-Path $hubDir $entry.Key) -Content $scriptContent
    }

    Write-Utf8Script -Path (Join-Path $hubDir "hub.cmd") -Content @"
@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."

:menu
cls
echo.
echo   ZAPRET HUB
echo   ----------
echo.
echo   1. Start main profile (SIMPLE FAKE ALT2)
echo   2. Start ALT11
echo   3. Start FAKE TLS AUTO ALT3
echo   4. Start ALT7
echo   5. Start Telegram Desktop proxy
echo   6. Stop all
echo   7. Install service / open service manager
echo   8. Remove service
echo   9. Configure for friends
echo   10. Open upstream service manager
echo.
echo   0. Exit
echo.
set "choice="
set /p "choice=Select option: "

if "%choice%"=="1" start "" "%~dp0run_full_simple_fake_alt2.cmd"
if "%choice%"=="2" start "" "%~dp0run_full_alt11.cmd"
if "%choice%"=="3" start "" "%~dp0run_full_fake_tls_auto_alt3.cmd"
if "%choice%"=="4" start "" "%~dp0run_full_alt7.cmd"
if "%choice%"=="5" start "" "%~dp0start_telegram_proxy_silent.cmd"
if "%choice%"=="6" call "%~dp0stop_all.cmd"
if "%choice%"=="7" start "" "%ROOT%\service.bat"
if "%choice%"=="8" call "%~dp0remove_service.cmd"
if "%choice%"=="9" call "%~dp0configure_for_friends.cmd"
if "%choice%"=="10" start "" "%ROOT%\service.bat"
if "%choice%"=="0" exit /b

goto menu
"@

    Write-Utf8Script -Path (Join-Path $hubDir "configure_for_friends.cmd") -Content @"
@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."

echo all>"%ROOT%\utils\game_filter.enabled"
break>"%ROOT%\lists\ipset-all.txt"

echo Preconfigured:
echo   Game Filter = enabled (TCP and UDP)
echo   IPSet Filter = any
echo.
echo Open the service manager and install the SIMPLE FAKE ALT2 profile if needed.
pause
"@

    Write-Utf8Script -Path (Join-Path $hubDir "install_service_simple_fake_alt2.cmd") -Content @"
@echo off
chcp 65001 > nul
cd /d "%~dp0"
set "ROOT=%~dp0.."

start "" "%ROOT%\service.bat"
"@

    Write-Utf8Script -Path (Join-Path $hubDir "remove_service.cmd") -Content @"
@echo off
chcp 65001 > nul
cd /d "%~dp0"

sc query zapret > nul 2>&1
if not errorlevel 1 (
    net stop zapret > nul 2>&1
    sc delete zapret > nul 2>&1
)

tasklist /FI "IMAGENAME eq winws.exe" | find /I "winws.exe" > nul
if not errorlevel 1 (
    taskkill /IM winws.exe /F > nul 2>&1
)

sc stop WinDivert > nul 2>&1
sc stop WinDivert14 > nul 2>&1

echo Service removal command completed.
"@

    Write-Utf8Script -Path (Join-Path $hubDir "stop_all.cmd") -Content @"
@echo off
chcp 65001 > nul
cd /d "%~dp0"

sc query zapret > nul 2>&1
if not errorlevel 1 (
    net stop zapret > nul 2>&1
)

tasklist /FI "IMAGENAME eq winws.exe" | find /I "winws.exe" > nul
if not errorlevel 1 (
    taskkill /IM winws.exe /F > nul 2>&1
)

tasklist /FI "IMAGENAME eq TgWsProxy_windows.exe" | find /I "TgWsProxy_windows.exe" > nul
if not errorlevel 1 (
    taskkill /IM TgWsProxy_windows.exe /F > nul 2>&1
)

sc stop WinDivert > nul 2>&1
sc stop WinDivert14 > nul 2>&1

echo Bypass processes were stopped.
"@
}

function Add-StagedHostlistEntries {
    param(
        [string]$BundleRoot,
        [string[]]$Entries
    )

    $listPath = Join-Path $BundleRoot "lists\list-general.txt"
    if (-not (Test-Path $listPath)) {
        throw "Hostlist not found: $listPath"
    }

    $lines = [System.Collections.Generic.List[string]]::new()
    $lines.AddRange([System.IO.File]::ReadAllLines($listPath, [System.Text.Encoding]::UTF8))

    $existing = [System.Collections.Generic.HashSet[string]]::new([System.StringComparer]::OrdinalIgnoreCase)
    foreach ($line in $lines) {
        $trimmed = $line.Trim()
        if ($trimmed -and -not $trimmed.StartsWith("#")) {
            [void]$existing.Add($trimmed)
        }
    }

    $added = 0
    foreach ($entry in $Entries) {
        if (-not $existing.Contains($entry)) {
            if ($added -eq 0) {
                $lines.Add("")
            }
            $lines.Add($entry)
            [void]$existing.Add($entry)
            $added++
        }
    }

    [System.IO.File]::WriteAllLines($listPath, $lines, $utf8NoBom)
}

function Patch-StagedBundle {
    param([string]$BundleRoot)

    Ensure-HubScripts -BundleRoot $BundleRoot
    Add-StagedHostlistEntries -BundleRoot $BundleRoot -Entries $githubHostlistEntries

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
        $utf8NoBom
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
        $utf8NoBom
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
