# Release Flow

## Install layout

The installer deploys the app in this shape:

```text
Zapret Hub/
  Zapret Hub.exe
  bundle/
    ...
```

The application first looks for `bundle/` next to the installed executable.

## Build the installer

Run:

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\build-installer.ps1 -BundleTag 1.10.0 -TelegramProxyTag v1.9.1
```

Optional custom bundle source:

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\build-installer.ps1 -BundlePath "D:\some\bundle"
```

Results:

- installer: `dist\installer\zapret-hub-setup-<version>.exe`
- update manifest: `dist\installer\latest.json`

## Update model

Current update support is installer-based:

- keep the same `AppId` in Inno Setup
- increase `version` in `Cargo.toml`
- rebuild the installer
- distribute the new installer

Installing a newer version upgrades the existing install in place.

Before replacing `bundle/`, the installer backs up and then restores user-owned lists, `ACTIVE_DISCORD_UDP.bin`, `ACTIVE_GAME_UDP.bin`, Game Filter state, and diagnostic logs. A failed restore leaves its timestamped backup next to the installation for recovery.

`latest.json` is generated so a future app-side update checker can compare versions against a hosted manifest.

## Telegram note

Starting with `Telegram Desktop 6.7.2`, the desktop client may no longer require the bundled Telegram WS proxy for the main MTProxy-related workaround path.

Because of that, the app should treat Telegram WS proxy as an optional compatibility tool:

- profile launch must work without forcing `telegram_proxy.cmd`
- manual Telegram WS proxy start should stay available
- UI copy should explain that the proxy is mainly for older Telegram Desktop builds or explicit troubleshooting
- `CF media` must require the user to provide their own Cloudflare domain; do not ship a personal/shared default domain in public releases
- setup instructions for `CF media` live in `docs/TELEGRAM_CF_MEDIA.md`

## Important packaging rule

Do not ship only the Rust executable.

The end-user package must include:

- `Zapret Hub.exe`
- the full `bundle/` directory

The Inno Setup installer already packages both.
