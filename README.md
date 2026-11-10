# Taskbar Media

A now-playing widget that sits on the Windows taskbar, immediately left of the
notification area (tray icons and clock).

![Taskbar Media on a dark and a light taskbar](docs/preview.png)

From left to right:

- A small settings button
- Album art; click it to bring up the app that is playing
- Previous, play/pause, next
- Title and artist over a seek bar with elapsed / total time (click or drag)

It uses Windows' own media session API, so it works with anything that shows
up in the Windows media flyout: Spotify (desktop and Store versions), browsers,
Media Player, foobar2000, and so on. The widget hides itself when nothing is
playing and when a full-screen app is in front. Titles that are too long to
fit scroll. The widget keeps clear of app icons: on Windows 10 the taskbar's
app area is narrowed to make room for it, and on Windows 11 the widget shrinks.

## Use

Run `TaskbarMedia.exe`. There is no installer and no other files.

- Click the gear, right-click the widget, or use the tray icon for settings
- Mouse wheel over the widget changes the volume
- Shift + mouse wheel over a widget nudges it left or right

## Settings

<img src="docs/settings.png" alt="The settings window" width="436">

- **Start with Windows**
- **Hide while paused**
- **Tray icon** and **wheel volume** on or off
- **Show media from**: any app, or Spotify only
- **Show on**: the main taskbar, all taskbars, or one chosen display
- **Text size**, **width**, **text color**
- **Rendering**: smooth (drawn at 2-3x and scaled down) or sharp (pixel-snapped)
- **Keep the widget clear of app icons**
- **Show elapsed and total time**
- **Scroll long titles**

Requires Windows 10 (1809 or later) or Windows 11, 64-bit, with the taskbar at
the bottom or top of the screen. Taskbars on other displays exist only when
"Show my taskbar on all displays" is on in Windows taskbar settings.

## Build

    cargo build --release

Building on Windows needs the MSVC toolchain and Windows SDK (for `rc.exe`).
Each push is also built by GitHub Actions, and publishing a release on GitHub
attaches a freshly built `TaskbarMedia.exe` to it.

Settings live in `HKCU\Software\TaskbarMedia`; "Start with Windows" adds a
value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

## License

[MIT](LICENSE)
