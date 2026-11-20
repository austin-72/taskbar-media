# Changelog

## 1.9.0 (2026-11-20)

A release of fixes and polish, from a review of every part of the app.

### Added
- Popups can be used from the keyboard: Up / Down, Home / End and Enter in
  the menu and device list; in the mixer, Left / Right change the volume and
  Space or M mutes.
- Starting the exe while it is already running opens the settings window.
- A widget that stops responding (not just one that crashes) is noticed
  within about a minute and restarted.
- The mixer and the device list say how many rows did not fit.
- Ctrl+Tab moves between the tabs of the settings window.
- Releases now come with a SHA-256 checksum file, and every build runs a
  set of automated tests first.

### Changed
- Pausing no longer makes the widget jump to something else that has long
  been making sound in the background; it moves on when another player
  starts (or had started in the ten seconds before the pause), or after a
  minute. Choosing "Automatic" takes effect at once.
- A layout preset is applied with an Apply button, after a confirmation,
  rather than the moment it is highlighted in the list.
- When nothing is playing, the app checks the taskbar a fifth as often.
- Menu and device rows act when the mouse button is released on them, like
  ordinary menus (slide off to cancel).
- A layout preset applied to "Other taskbars" only changes those taskbars.
- On a full Windows 11 taskbar the widget shrinks in more steps (narrower
  text, tighter and smaller buttons, then art and buttons only).
- Text stays more readable on a small taskbar.
- Enter no longer closes the settings window from a list or drop-down.
- "Start with Windows" repairs itself when the exe it pointed at has been
  moved or renamed, and Settings says when another copy is the one set to
  start.

### Fixed
- A player that stopped answering could freeze the widget for good; every
  request to a player now gives up after two seconds.
- Play / pause did nothing with players that offer "play" and "pause" but
  not the combined toggle.
- Album art could stay on the previous track's cover, or be drawn half
  loaded.
- The wheel over the widget did not change Firefox's own volume or that of
  installed web apps (it fell back to the system volume), and a touchpad
  scroll slammed the volume to 0 or 100.
- "Repeat one" was hard to tell from "repeat all": the "1" is now clear of
  the arrow, and sits beside the icon when shuffle and repeat are stacked.
- With the time labels turned off, a click on the left edge of the button
  after the seek bar jumped to the end of the track.
- "Seek bar on its own" showed the artist without the title, or nothing,
  for players that report no timeline.
- A switch of output device that only half worked is undone, instead of
  leaving Windows and the widget showing different devices.
- Wheel-up on a muted player now un-mutes it.
- Closing the mixer while dragging a slider could leave a ghost of it that
  would not go away.
- Alt+F4 with a popup open destroyed the popup window, so no popup opened
  again until restart; closing the app from outside (for example `taskkill`
  without `/f`) could leave an invisible process that blocked restarting.
- Popups could run off the screen with many apps, devices or players, and
  at 125% or 175% scaling the highlighted row drifted away from the mouse.
- Drawing is now aligned to whole pixels at every scaling (sharper album
  art and seek bar), and long titles scroll smoothly instead of in jerks.
- The highlight color is adjusted when it would be hard to see against the
  taskbar.
- Clicks at the right edge of a button next to the album art, settings
  button or player switcher went to the neighbour.
- The widget could become empty and unclickable (every item hidden, or only
  unavailable items shown); a layout always shows something now.
- With "Title on its own", the artist was not shown at all.
- After a crash or Task Manager kill on Windows 10, the taskbar's app area
  stayed narrowed.
- A full-screen browser video could be covered when the widget had been
  nudged to the right.
- The tray icon could be missing after signing in, and leaked an icon handle
  each time it was added.
- The settings window: it jumped back to the middle of the screen and to
  "Main taskbar" after choosing a preset; it was the wrong size on a display
  scaled differently from the main one; a stray gap in the text on the
  Behavior tab; a row could not be dragged past the visible part of the
  layout list; a chosen display or output device that was unplugged showed
  as "not set"; fonts leaked each time it opened.
- Exporting settings from a fresh install wrote a file that could not be
  imported, and importing an unrelated file could wipe the settings.
- The update check gave up for six hours when there was no network at
  sign-in, and could hang for ever on a stalled connection.
- The crash log is now written safely even when the crash is in memory
  allocation.

## 1.8.0 (2026-11-11)

### Added
- Live preview at the top of the settings window: the widget as it will
  look, updating as you change settings.
- Drag items in the layout list to reorder them.
- Layout presets: Default, Minimal, Compact, Everything and Classic.
- A restyled right-click menu matching the mixer and device list, with new
  entries for copying the track name and opening the volume mixer and
  output device list.
- The widget restarts itself if it ever crashes, and records what happened
  in a log file (Settings, More, "Open the log folder").
- "Save pictures of the widget" (Settings, More) writes PNG images of the
  widget and its popups, drawn by the app itself, for sharing or bug reports.

### Changed
- "Text width" and "Text" moved from the Layout tab to Appearance.

## 1.7.0 (2026-11-11)

### Added
- Button spacing setting: compact, normal, roomy, wide or extra wide.
- Option to stack shuffle above repeat in a single column.

### Changed
- Buttons that cannot be used right now are shown greyed out instead of
  disappearing: shuffle and repeat with players that lack them, and the
  player arrows when only one player is running. The widget no longer
  changes width as these come and go. The old behaviour is available as
  "Hide buttons that can't be used" in Settings, Layout.

## 1.6.0 (2026-11-10)

### Added
- Volume button on the widget. Clicking it opens a mixer above the taskbar
  with a slider and mute button for the system volume and for every app
  that is playing sound. Click elsewhere or press Esc to close it. The
  button's icon reflects the player's volume, or the system volume if you
  prefer (Settings, Behavior).

### Changed
- The output device button now opens a list styled like the mixer, with an
  icon for each device and a tick on the current one, in place of a plain
  Windows menu.
- The output device button's speaker icon is now a loudspeaker cabinet, so
  it is not confused with the volume icon.

## 1.5.0 (2026-11-10)

### Added
- Player switcher: up and down arrows just left of the album art that step
  through the apps currently playing. They only appear when more than one
  player is running, and the widget briefly names the player you switch to.

### Changed
- The settings button is now hidden by default to make room; settings are
  on the right-click menu and the tray icon. It can be shown again in
  Settings, Layout. Existing layouts are updated the same way once.
- With the settings button hidden, an available update is marked with a dot
  on the corner of the album art.

## 1.4.1 (2026-11-10)

### Fixed
- The widget no longer draws over full-screen video in a browser.
- The widget no longer stays hidden behind the taskbar after leaving
  full-screen.

## 1.4.0 (2026-11-10)

### Added
- "Title on its own" and "Seek bar on its own" layout items, so the title
  and seek bar can sit side by side or be used separately.
- A separate layout for taskbars on other displays.
- Custom text and highlight colors, chosen with the Windows color picker.
- Hover card: resting the mouse on the widget shows larger album art with
  the title, artist, album and player. Can be turned off.
- Export and import of all settings as a file, and "Reset everything".

### Changed
- The settings window has a fifth tab, More, holding the settings file
  buttons and the About section.
- README screenshots redrawn from the widget's actual layout code.

## 1.3.0 (2026-11-10)

### Added
- Output device button: an optional widget item that switches where sound
  plays. Click it for a list of playback devices, or set it to flip between
  two chosen devices. Its icon shows headphones or a speaker to match the
  current device. Turn it on in Settings, Layout.
- "Output device" submenu in the right-click and tray menus, available even
  with the button hidden.
- The widget briefly names the new device after a switch.

## 1.2.0 (2026-11-10)

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

## 1.1.0 (2026-11-10)

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

## 1.0.0 (2026-11-10)

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

## 0.1.0 (2026-11-09)

- First release: album art, title and artist, previous / play-pause / next,
  seek bar, click the art to open the player.
- Settings window, multi-display support, smooth rendering.
