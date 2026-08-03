# Zapret Hub 0.1.5

`Zapret Hub` remains a Windows GUI and installer wrapper around external bypass tools. This release does not claim authorship of `zapret`, `zapret-discord-youtube`, `tg-ws-proxy`, WinDivert, Telegram Desktop, or their bypass logic.

## What changed

- Release packaging is pinned to `zapret-discord-youtube 1.10.0` and `tg-ws-proxy v1.9.1`.
- Runtime shutdown now sends stop requests together and uses one eight-second deadline instead of serial waits.
- Profile selection is a native Rust worker: no visible PowerShell window, progress and cancellation are shown in the GUI, and results are saved in `utils\test results` with `ANALYTICS` and `Best strategy` sections.
- The expanded search supports a standard HTTP/TLS/ping run, a DPI 16–20 KB run, and selecting individual profiles.
- Profiles now include upstream `general (EXP).bat` automatically.
- The Profiles tab can independently select UDP fake files for Discord Voice and GameFilter. Files are matched by SHA-256 and swapped through a verified temporary copy.
- Both installer and in-app bundle updates preserve user lists, active UDP fake files, Game Filter state, and diagnostic logs.

## Included upstream versions

- `zapret-discord-youtube`: `1.10.0`
- `tg-ws-proxy`: `v1.9.1`

## Artifacts

- Installer: `zapret-hub-setup-0.1.5.exe`
- Update manifest: `latest.json`
