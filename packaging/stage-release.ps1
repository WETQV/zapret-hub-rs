param(
    [string]$BundlePath,
    [string]$StageRoot = "dist\stage",
    [string]$BundleTag = "1.10.0",
    [string]$TelegramProxyTag = "v1.9.1"
)

$ErrorActionPreference = "Stop"

$env:LANG = "en_US.UTF-8"
$env:LC_ALL = "en_US.UTF-8"
$env:PYTHONIOENCODING = "utf-8"

$projectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$stagePath = Join-Path $projectRoot $StageRoot
$bundleTarget = Join-Path $stagePath "bundle"
$exeSource = Join-Path $projectRoot "target\release\zapret-hub-rs.exe"
$exeTarget = Join-Path $stagePath "Zapret Hub.exe"
$whitelistSource = Join-Path $projectRoot "assets\builtin-whitelist.txt"
$whitelistTarget = Join-Path $stagePath "builtin-whitelist.txt"
$bundleReleaseApi = "https://api.github.com/repos/Flowseal/zapret-discord-youtube/releases/tags/$BundleTag"
$tgProxyReleaseApi = "https://api.github.com/repos/Flowseal/tg-ws-proxy/releases/tags/$TelegramProxyTag"
$tgProxyAssetName = "TgWsProxy_windows.exe"
$tgProxyVersionFileName = "TgWsProxy_windows.version.json"
$bundleVersionFileName = "ZapretBundle.version.json"
$upstreamUpdateCheckFlagRelativePath = "utils\check_updates.enabled"
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

function Test-BundleRoot {
    param([string]$Path)

    return (Test-Path (Join-Path $Path "bin")) `
        -and (Test-Path (Join-Path $Path "lists")) `
        -and (Test-Path (Join-Path $Path "service.bat")) `
        -and (Test-Path (Join-Path $Path "general (SIMPLE FAKE ALT2).bat"))
}

function Assert-StagedBundle {
    param([string]$BundleRoot)

    foreach ($requiredPath in @("bin\winws.exe", "lists", "service.bat")) {
        if (-not (Test-Path -LiteralPath (Join-Path $BundleRoot $requiredPath))) {
            throw "Staged bundle is missing required path: $requiredPath"
        }
    }

    $profileScripts = @(Get-ChildItem -LiteralPath $BundleRoot -File -Filter "general*.bat")
    if ($profileScripts.Count -eq 0) {
        throw "Staged bundle does not contain general*.bat profiles"
    }

    foreach ($script in $profileScripts) {
        $content = [System.IO.File]::ReadAllText($script.FullName, [System.Text.Encoding]::UTF8)
        if ($content -notlike '*start "" /B "%BIN%winws.exe"*') {
            throw "Profile launcher was not patched for hidden winws start: $($script.Name)"
        }
    }
}

function Find-BundleRoot {
    param([string]$ExtractRoot)

    if (Test-BundleRoot -Path $ExtractRoot) {
        return $ExtractRoot
    }

    $child = Get-ChildItem -LiteralPath $ExtractRoot -Directory |
        Where-Object { Test-BundleRoot -Path $_.FullName } |
        Select-Object -First 1

    if (-not $child) {
        throw "Downloaded archive does not contain a valid zapret bundle"
    }

    return $child.FullName
}

function Resolve-BundleSource {
    param([string]$RequestedBundlePath)

    if (-not [string]::IsNullOrWhiteSpace($RequestedBundlePath)) {
        if (-not (Test-Path $RequestedBundlePath)) {
            throw "Bundle path not found: $RequestedBundlePath"
        }

        return @{
            Path = (Resolve-Path $RequestedBundlePath).Path
            CleanupRoot = $null
            Version = $null
            SourceFolder = $null
        }
    }

    $headers = @{
        "User-Agent" = "zapret-hub-rs-packaging"
        "Accept" = "application/vnd.github+json"
    }

    $release = Invoke-RestMethod -Headers $headers -Uri $bundleReleaseApi
    $assetName = "zapret-discord-youtube-$($release.tag_name).zip"
    $asset = $release.assets | Where-Object { $_.name -eq $assetName } | Select-Object -First 1
    if (-not $asset) {
        throw "Pinned bundle zip asset not found in GitHub release metadata: $assetName"
    }

    $downloadRoot = Join-Path ([System.IO.Path]::GetTempPath()) "zapret-hub-bundle-$([guid]::NewGuid())"
    $zipPath = Join-Path $downloadRoot $asset.name
    $extractRoot = Join-Path $downloadRoot "extract"
    New-Item -ItemType Directory -Force -Path $downloadRoot | Out-Null

    Invoke-WebRequest -Headers $headers -Uri $asset.browser_download_url -OutFile $zipPath
    Expand-Archive -LiteralPath $zipPath -DestinationPath $extractRoot -Force

    $bundleRoot = Find-BundleRoot -ExtractRoot $extractRoot

    return @{
        Path = $bundleRoot
        CleanupRoot = $downloadRoot
        Version = $release.tag_name
        SourceFolder = [System.IO.Path]::GetFileNameWithoutExtension($asset.name)
    }
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
setlocal EnableExtensions EnableDelayedExpansion
chcp 65001 > nul
cd /d "%~dp0"

set "FAILED="

call :stop_service zapret
call :kill_process winws.exe
call :kill_process TgWsProxy_windows.exe
call :stop_service WinDivert
call :stop_service WinDivert14
call :verify_service_stopped zapret
call :verify_process_stopped winws.exe
call :verify_process_stopped TgWsProxy_windows.exe
call :verify_service_stopped WinDivert
call :verify_service_stopped WinDivert14

if defined FAILED (
    echo Failed to stop: !FAILED!
    exit /b 1
)

sc delete zapret > nul 2>&1
sc delete WinDivert > nul 2>&1
sc delete WinDivert14 > nul 2>&1

echo Service removal command completed.
exit /b 0

:stop_service
sc query "%~1" > nul 2>&1
if errorlevel 1 exit /b 0
net stop "%~1" > nul 2>&1
for /l %%I in (1,1,10) do (
    sc query "%~1" 2>nul | findstr /I "RUNNING STOP_PENDING" > nul
    if errorlevel 1 exit /b 0
    timeout /t 1 /nobreak > nul
)
sc stop "%~1" > nul 2>&1
exit /b 0

:kill_process
for /l %%I in (1,1,3) do (
    tasklist /FI "IMAGENAME eq %~1" | find /I "%~1" > nul
    if errorlevel 1 exit /b 0
    taskkill /IM "%~1" /T /F > nul 2>&1
    timeout /t 1 /nobreak > nul
)
exit /b 0

:verify_service_stopped
sc query "%~1" > nul 2>&1
if errorlevel 1 exit /b 0
sc query "%~1" 2>nul | findstr /I "RUNNING STOP_PENDING" > nul
if not errorlevel 1 call :append_failure "service %~1"
exit /b 0

:verify_process_stopped
tasklist /FI "IMAGENAME eq %~1" | find /I "%~1" > nul
if not errorlevel 1 call :append_failure "process %~1"
exit /b 0

:append_failure
if defined FAILED (
    set "FAILED=!FAILED!; %~1"
) else (
    set "FAILED=%~1"
)
exit /b 0
"@

    Write-Utf8Script -Path (Join-Path $hubDir "stop_all.cmd") -Content @"
@echo off
setlocal EnableExtensions EnableDelayedExpansion
chcp 65001 > nul
cd /d "%~dp0"

set "FAILED="

call :stop_service zapret
call :kill_process winws.exe
call :kill_process TgWsProxy_windows.exe
call :stop_service WinDivert
call :stop_service WinDivert14
call :verify_service_stopped zapret
call :verify_process_stopped winws.exe
call :verify_process_stopped TgWsProxy_windows.exe
call :verify_service_stopped WinDivert
call :verify_service_stopped WinDivert14

if defined FAILED (
    echo Failed to stop: !FAILED!
    exit /b 1
)

echo Bypass processes were stopped.
exit /b 0

:stop_service
sc query "%~1" > nul 2>&1
if errorlevel 1 exit /b 0
net stop "%~1" > nul 2>&1
for /l %%I in (1,1,10) do (
    sc query "%~1" 2>nul | findstr /I "RUNNING STOP_PENDING" > nul
    if errorlevel 1 exit /b 0
    timeout /t 1 /nobreak > nul
)
sc stop "%~1" > nul 2>&1
exit /b 0

:kill_process
for /l %%I in (1,1,3) do (
    tasklist /FI "IMAGENAME eq %~1" | find /I "%~1" > nul
    if errorlevel 1 exit /b 0
    taskkill /IM "%~1" /T /F > nul 2>&1
    timeout /t 1 /nobreak > nul
)
exit /b 0

:verify_service_stopped
sc query "%~1" > nul 2>&1
if errorlevel 1 exit /b 0
sc query "%~1" 2>nul | findstr /I "RUNNING STOP_PENDING" > nul
if not errorlevel 1 call :append_failure "service %~1"
exit /b 0

:verify_process_stopped
tasklist /FI "IMAGENAME eq %~1" | find /I "%~1" > nul
if not errorlevel 1 call :append_failure "process %~1"
exit /b 0

:append_failure
if defined FAILED (
    set "FAILED=!FAILED!; %~1"
) else (
    set "FAILED=%~1"
)
exit /b 0
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

function Remove-Sha256SumEntry {
    param(
        [string]$BundleRoot,
        [string]$RelativePath
    )

    $checksumsPath = Join-Path $BundleRoot "SHA256SUMS.txt"
    if (-not (Test-Path -LiteralPath $checksumsPath)) {
        return
    }

    $normalizedRelativePath = $RelativePath.Replace("\", "/")
    $lines = [System.IO.File]::ReadAllLines($checksumsPath, [System.Text.Encoding]::UTF8)
    $filtered = New-Object System.Collections.Generic.List[string]
    $changed = $false

    foreach ($line in $lines) {
        if ([string]::IsNullOrWhiteSpace($line)) {
            $filtered.Add($line)
            continue
        }

        $parts = $line -split '\s+'
        $target = $parts[$parts.Length - 1].Replace("\", "/")
        if ($target.StartsWith("./")) {
            $target = $target.Substring(2)
        }

        if ($target -eq $normalizedRelativePath) {
            $changed = $true
            continue
        }

        $filtered.Add($line)
    }

    if ($changed) {
        [System.IO.File]::WriteAllLines($checksumsPath, $filtered, $utf8NoBom)
    }
}

function Disable-UpstreamUpdateChecks {
    param([string]$BundleRoot)

    $flagPath = Join-Path $BundleRoot $upstreamUpdateCheckFlagRelativePath
    if (Test-Path -LiteralPath $flagPath) {
        Remove-Item -LiteralPath $flagPath -Force
    }

    Remove-Sha256SumEntry -BundleRoot $BundleRoot -RelativePath $upstreamUpdateCheckFlagRelativePath
}

function Patch-StagedBundle {
    param([string]$BundleRoot)

    Ensure-HubScripts -BundleRoot $BundleRoot
    Add-StagedHostlistEntries -BundleRoot $BundleRoot -Entries $githubHostlistEntries
    Disable-UpstreamUpdateChecks -BundleRoot $BundleRoot

    $profileScripts = Get-ChildItem -LiteralPath $BundleRoot -File -Filter "general*.bat"
    foreach ($script in $profileScripts) {
        $scriptPath = $script.FullName
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
        throw "Pinned tg-ws-proxy Windows asset not found in GitHub release metadata"
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
        [string]$BundleSourcePath,
        [string]$Version,
        [string]$SourceFolder
    )

    if ([string]::IsNullOrWhiteSpace($SourceFolder)) {
        $SourceFolder = Split-Path $BundleSourcePath -Leaf
    }

    if ([string]::IsNullOrWhiteSpace($Version)) {
        $Version = $SourceFolder
        if ($Version -like "zapret-discord-youtube-*") {
            $Version = $Version.Substring("zapret-discord-youtube-".Length)
        }
    }

    $bundleInfo = @{
        version = $Version
        source_folder = $SourceFolder
    } | ConvertTo-Json -Depth 3

    [System.IO.File]::WriteAllText(
        (Join-Path $BundleRoot $bundleVersionFileName),
        $bundleInfo,
        $utf8NoBom
    )
}

$resolvedBundle = Resolve-BundleSource -RequestedBundlePath $BundlePath

try {
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
    Copy-Item (Join-Path $resolvedBundle.Path "*") $bundleTarget -Recurse -Force
    Write-BundleVersionMetadata `
        -BundleRoot $bundleTarget `
        -BundleSourcePath $resolvedBundle.Path `
        -Version $resolvedBundle.Version `
        -SourceFolder $resolvedBundle.SourceFolder
    Update-StagedTelegramProxy -BundleRoot $bundleTarget
    Patch-StagedBundle -BundleRoot $bundleTarget
    Assert-StagedBundle -BundleRoot $bundleTarget
}
finally {
    if ($resolvedBundle.CleanupRoot -and (Test-Path $resolvedBundle.CleanupRoot)) {
        Remove-Item -Recurse -Force $resolvedBundle.CleanupRoot
    }
}

Write-Host "Staged application at: $stagePath"
