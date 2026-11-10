# Changelog

## 1.0.0

### Added
- Tray icon with Settings, Open player and Exit, so the app can be reached
  when nothing is playing. Its tooltip shows the current track.
- Titles that are too long now scroll instead of being cut off.
- Scroll the mouse wheel over the widget to change the system volume.
- On Windows 11 the widget shrinks, down to album art and buttons only,
  rather than cover app icons on a crowded taskbar.
- On Windows 10 the taskbar's app area is narrowed to make room for the
  widget, so it no longer sits on top of app buttons.
- On Windows 11 the widget finds the real clock position on secondary
  taskbars instead of estimating it.
- The widget hides together with an auto-hiding taskbar.
- App icon and version information in the exe.
- Settings for the tray icon, wheel volume, title scrolling and shrinking.
- MIT license.
- GitHub Actions workflow that builds the exe and attaches it to releases.

### Changed
- The exe is now built as `TaskbarMedia.exe` directly.
- DPI awareness and themed controls are declared in the exe's manifest.

## 0.1.0

- First release: album art, title and artist, previous / play-pause / next,
  seek bar, click the art to open the player.
- Settings window, multi-display support, smooth rendering.
