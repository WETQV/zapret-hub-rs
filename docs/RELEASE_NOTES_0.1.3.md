# Zapret Hub 0.1.3

`Zapret Hub` is a Windows GUI and installer wrapper around external bypass tools. We do not claim authorship of `zapret`, `zapret-discord-youtube`, `tg-ws-proxy`, `WinDivert`, Telegram Desktop, or their bypass logic. This release updates the packaged upstream tools and keeps the existing installer-based delivery model.

## What's changed

- Updated the packaged Windows bundle source to `zapret-discord-youtube 1.9.8c`.
- Bundled `TG WS Proxy` is staged from the current upstream latest release, `v1.7.0`.
- Updated release defaults and the legacy fallback bundle path to the `1.9.8c` bundle folder.
- Added common GitHub web, API, asset, and raw-content domains to the packaged general hostlist.
- Reworked the app into focused tabs and added in-app bundle update checks for future upstream releases.
- Added the full upstream profile list, selectable persisted main profiles, and an in-app ipset refresh action for the current bundle.
- Added startup-only soft notifications for app, Zapret bundle, and Tg proxy updates, with per-release dismissal and a global settings toggle.
- Added project links for Zapret Hub, upstream zapret, zapret-discord-youtube, and tg-ws-proxy in Settings.

## Included upstream versions

- `zapret-discord-youtube`: `1.9.8c`
- `tg-ws-proxy`: `v1.7.0`
- `zapret` by bol-van: `v72.12`

## Credits and upstream projects

- `zapret` by bol-van: https://github.com/bol-van/zapret
- Windows bundle lineage used by this app: https://github.com/Flowseal/zapret-discord-youtube
- Telegram WS proxy: https://github.com/Flowseal/tg-ws-proxy
- `WinDivert`: https://reqrypt.org/windivert.html
- Telegram Desktop: https://github.com/telegramdesktop/tdesktop
- `egui` / `eframe`: https://github.com/emilk/egui
- Inno Setup: https://jrsoftware.org/isinfo.php

## Notes

- `CF media` still requires a user-controlled Cloudflare domain in `Full setup`.
- Telegram media and calls are not guaranteed for every chat or channel; this remains an upstream limitation of the current proxy approach.
- AndroidHub is not part of this Windows installer release.

## Artifacts

- Installer: `zapret-hub-setup-0.1.3.exe`
- Update manifest: `latest.json`
