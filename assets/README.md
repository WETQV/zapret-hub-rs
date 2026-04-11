# Assets

## App icon

Place the Windows application icon here:

- `assets/icons/app.ico`

Recommended icon contents:

- `16x16`
- `24x24`
- `32x32`
- `48x48`
- `64x64`
- `128x128`
- `256x256`

Notes:

- final build format should be a single multi-size `.ico`
- keep a source file separately, for example `assets/icons/app-source.svg` or `assets/icons/app-source.png`
- Windows Explorer and the taskbar will pick different embedded sizes from the `.ico` file
- the build stays valid even if `app.ico` is missing, but the executable will then use the default icon
