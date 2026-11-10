# Changelog

## 1.3.0

### Added
- Output device button: an optional widget item that switches where sound
  plays. Click it for a list of playback devices, or set it to flip between
  two chosen devices. Its icon shows headphones or a speaker to match the
  current device. Turn it on in Settings, Layout.
- "Output device" submenu in the right-click and tray menus, available even
  with the button hidden.
- The widget briefly names the new device after a switch.

## 1.2.0

### Added
- Layout editor: choose which items show (settings button, album art,
  shuffle, previous, play/pause, next, repeat, title and seek bar) and put
  them in any order.
- Button size: small, medium or large.
- Text options: "Title · Artist", title only, or "Artist · Title" over the
  seek bar, or title over artist or album with no seek bar.
- Font choice.
- Highlight color: from the album art, the Windows accent color, or none.
- Album art shape: rounded, square or circle.
- Seek bar thickness.
- Optional subtle or solid background behind the widget.
- Click actions: choose what clicking the album art and the title does.

### Changed
- The settings window now has tabs: General, Layout, Appearance, Behavior.

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
