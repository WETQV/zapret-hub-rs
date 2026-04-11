# Zapret Hub 0.1.2

`Zapret Hub` is a Windows GUI and installer wrapper around external bypass tools. We do not claim authorship of `zapret`, `zapret-discord-youtube`, `tg-ws-proxy`, `WinDivert`, Telegram Desktop, or their bypass logic. This release only packages, launches, configures, and documents those tools for a simpler Windows workflow.

## What's changed

- Added `CF media` support for Telegram media loading cases where messages work but photos, videos, reactions, or some files do not.
- Removed the shared default Cloudflare domain from public builds. Users must provide their own Cloudflare domain for `CF media`.
- Added a setup guide for personal Cloudflare domains: [`docs/TELEGRAM_CF_MEDIA.md`](TELEGRAM_CF_MEDIA.md).
- `CF media` now syncs the local `TG WS Proxy` config before launch, so stale tray settings do not silently override the app.
- `CF media` uses `DC4 only`, matching the upstream recommendation for the media-loading scenario.
- Zapret Hub automatically adds the user's `CF media` domain and `kws1/kws2/kws3/kws4/kws5/kws203` hosts to the user hostlist.
- Added launch diagnostics for Telegram proxy startup.
- Bundled `TG WS Proxy` was staged from the current upstream latest release, `v1.6.0`.

## Included upstream versions

- `zapret-discord-youtube`: `1.9.7b`
- `tg-ws-proxy`: `v1.6.0`

## Credits and upstream projects

- `zapret` by bol-van: https://github.com/bol-van/zapret
- Windows bundle lineage used by this app: https://github.com/Flowseal/zapret-discord-youtube
- Telegram WS proxy: https://github.com/Flowseal/tg-ws-proxy
- `WinDivert`: https://reqrypt.org/windivert.html
- Telegram Desktop: https://github.com/telegramdesktop/tdesktop
- `egui` / `eframe`: https://github.com/emilk/egui
- Inno Setup: https://jrsoftware.org/isinfo.php

## Notes

- `CF media` requires a user-controlled domain connected to Cloudflare in `Full setup`.
- Cloudflare `SSL/TLS` mode must be `Flexible`.
- Cloudflare DNS records must be proxied through the orange cloud.
- Telegram media and calls are not guaranteed for every chat or channel; this is an upstream limitation of the current proxy approach.

## Artifacts

- Installer: `zapret-hub-setup-0.1.2.exe`
- Update manifest: `latest.json`
