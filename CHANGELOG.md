# Changelog

## 1.1.0

### Added
- Shuffle and repeat buttons, shown for players that support them.
- Player menu (right-click) to choose which app the widget follows when
  several are playing.
- The mouse wheel now changes the player's own volume, with a brief level
  read-out on the widget. System volume is still available in settings.
- The seek bar takes its color from the album art.
- Click the title to copy "Artist - Title".
- Update check: a dot on the gear and a menu entry when a newer release is
  on GitHub. Can be turned off.
- Version shown in the settings window.

### Changed
- The settings window is laid out in two columns.
- The exe's version number now comes from Cargo.toml alone.
- MSVC builds include the C runtime, so the exe has no VCRUNTIME140.dll
  dependency.

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
