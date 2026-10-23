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

## Release with GitHub Actions (recommended)

1. Bump `version` in `Cargo.toml`, write `docs/RELEASE_NOTES_<version>.md`, merge to `main`.
2. Tag and push: `git tag v<version> && git push origin v<version>`.
3. The `Build release packages` workflow builds the installer and the portable zip on a
   Windows runner and attaches them (plus `latest.json`) to the release, creating it from the
   release notes if needed. It can also be started by hand from the Actions tab.

Manual build:

On Linux (cross-compiled, produces the staged app and a portable zip):

```bash
packaging/build-windows.sh                    # latest upstream bundle
BUNDLE_TAG=1.10.4 packaging/build-windows.sh  # pinned bundle
```

On Windows (adds the Inno Setup installer):

```powershell
powershell -ExecutionPolicy Bypass -File .\packaging\build-installer.ps1 -BundleTag 1.10.4
```

Both scripts prepare `bundle/` with `zapret-hub-rs --stage-bundle <dir> [--bundle-tag <tag>]`,
the same code path as the in-app updater, so the shipped bundle and an updated bundle are
always patched identically.

Results:

- staged app: `dist/stage/` (`Zapret Hub.exe`, `builtin-whitelist.txt`, `bundle/`)
- portable zip: `dist/portable/zapret-hub-<version>-portable.zip`
- installer: `dist\installer\zapret-hub-setup-<version>.exe` and `dist\installer\latest.json`

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
