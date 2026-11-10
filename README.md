# Taskbar Media

A now-playing widget that sits on the Windows taskbar, immediately left of the
notification area (tray icons and clock).

- Album art; click it to bring up the app that is playing
- Title and artist
- Seek bar with elapsed / total time (click or drag)
- Previous, play/pause, next

It uses Windows' own media session API, so it works with anything that shows
up in the Windows media flyout: Spotify (desktop and Store versions), browsers,
Media Player, foobar2000, and so on. The widget hides itself when nothing is
playing and when a full-screen app is in front.

## Use

Run `TaskbarMedia.exe`. There is no installer and no other files.

- Right-click the widget: Open player, Start with Windows, Reset position, Exit
- Shift + mouse wheel over the widget: nudge it left or right

Requires Windows 10 (1809 or later) or Windows 11, 64-bit, with the taskbar at
the bottom or top of the screen.

## Build

    cargo build --release

Settings live in `HKCU\Software\TaskbarMedia`; "Start with Windows" adds a
value under `HKCU\Software\Microsoft\Windows\CurrentVersion\Run`.
