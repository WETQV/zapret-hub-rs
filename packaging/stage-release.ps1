param(
    [string]$StageRoot = "dist\stage",
    # Empty = latest upstream release.
    [string]$BundleTag = ""
)

# Builds the release exe and stages it next to a freshly prepared bundle. The bundle is
# prepared by the app itself (`--stage-bundle`), so packaging and the in-app updater share
# one implementation (patched profiles, hub scripts, Telegram proxy, hostlists).

$ErrorActionPreference = "Stop"

$projectRoot = Resolve-Path (Join-Path $PSScriptRoot "..")
$stagePath = Join-Path $projectRoot $StageRoot
$bundleTarget = Join-Path $stagePath "bundle"

Push-Location $projectRoot
try {
    cargo build --release
    if ($LASTEXITCODE -ne 0) { throw "cargo build failed with exit code $LASTEXITCODE" }

    if (Test-Path -LiteralPath $stagePath) {
        Remove-Item -LiteralPath $stagePath -Recurse -Force
    }
    New-Item -ItemType Directory -Force -Path $stagePath | Out-Null

    # Debug build: console subsystem and an asInvoker manifest, so it runs without UAC.
    $stageArgs = @("run", "--quiet", "--", "--stage-bundle", $bundleTarget)
    if ($BundleTag) { $stageArgs += @("--bundle-tag", $BundleTag) }
    & cargo @stageArgs
    if ($LASTEXITCODE -ne 0) { throw "bundle staging failed with exit code $LASTEXITCODE" }

    Copy-Item -LiteralPath (Join-Path $projectRoot "target\release\zapret-hub-rs.exe") `
        -Destination (Join-Path $stagePath "Zapret Hub.exe") -Force
    Copy-Item -LiteralPath (Join-Path $projectRoot "assets\builtin-whitelist.txt") `
        -Destination (Join-Path $stagePath "builtin-whitelist.txt") -Force
}
finally {
    Pop-Location
}

Write-Host "Staged application at: $stagePath"
