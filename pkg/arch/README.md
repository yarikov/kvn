# Arch Linux Build

Local PKGBUILD for building `kvn` from source.

## Build & Install

```bash
cd pkg/arch
makepkg -si
```

## Dependencies

Build: `rust`, `cargo`.

Runtime:

- `sing-box` 1.14 or newer
- `dbus` (used by zbus)
- `gcc-libs`
- `libcap`

Optional:

- `wl-clipboard` — clipboard integration on Wayland
- `xclip` — clipboard integration on X11 (preferred)
- `xsel` — clipboard integration on X11 (alternative)
- `xdg-utils` — open the project support page from the TUI

## Clean

```bash
cd pkg/arch
makepkg -C
```
