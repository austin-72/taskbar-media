# Taskbar Media

A now-playing widget that sits on the Windows taskbar, immediately left of the
notification area (tray icons and clock).

From left to right:

- A small settings button
- Album art; click it to bring up the app that is playing
- Previous, play/pause, next
- Title and artist over a seek bar with elapsed / total time (click or drag)

It uses Windows' own media session API, so it works with anything that shows
up in the Windows media flyout: Spotify (desktop and Store versions), browsers,
Media Player, foobar2000, and so on. The widget hides itself when nothing is
playing and when a full-screen app is in front.

## Use

Run `TaskbarMedia.exe`. There is no installer and no other files.

- Click the gear (or right-click the widget) for settings
- Shift + mouse wheel over a widget nudges it left or right

## Settings

- **Start with Windows**
- **Hide while paused**
- **Show media from**: any app, or Spotify only
- **Show on**: the main taskbar, all taskbars, or one chosen display
- **Text size**, **width**, **text color**
- **Rendering**: smooth (drawn at 2-3x and scaled down) or sharp (pixel-snapped)
- **Show elapsed and total time**

Requires Windows 10 (1809 or later) or Windows 11, 64-bit, with the taskbar at
the bottom or top of the screen. Taskbars on other displays exist only when
"Show my taskbar on all displays" is on in Windows taskbar settings.

## Build

    cargo build --release

Settings live in `HKCU\Software\TaskbarMedia`; "Start with Windows" adds a
value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.

## Publish

`publish-to-github.bat` commits everything in this folder with the message in
`commit-message.txt` and pushes it to GitHub.
