# Zapret Hub 0.1.4

`Zapret Hub` is a Windows GUI and installer wrapper around external bypass tools. We do not claim authorship of `zapret`, `zapret-discord-youtube`, `tg-ws-proxy`, `WinDivert`, Telegram Desktop, or their bypass logic.

## What's changed

- Updated release metadata for `zapret-discord-youtube 1.9.9c`.
- Bundled `TG WS Proxy` is staged from the current upstream latest release, `v1.8.1`.
- Disabled the embedded `TG WS Proxy` update checker in the generated appdata config, so proxy startup does not open its own update/browser flow.
- Disabled the upstream Flowseal auto-update flag in packaged and in-app staged bundles, so profile startup does not open the upstream update/browser flow.
- Patched all discovered upstream profile launchers, including newly added profiles, instead of only the older fixed subset.
- Updated the installer build to prefer Inno Setup 7 and added a branded modern wizard background.
- Kept in-app bundle update handling staged: download first, apply only after runtime processes are stopped.

## Included upstream versions

- `zapret-discord-youtube`: `1.9.9c`
- `tg-ws-proxy`: `v1.8.1`

## Artifacts

- Installer: `zapret-hub-setup-0.1.4.exe`
- Update manifest: `latest.json`
