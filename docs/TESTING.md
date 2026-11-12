# Test checklist

A pass through everything, roughly riskiest first. Tick what works; anything
that does not, note what you saw. If the widget disappears at any point,
check Settings, More, "Open the log folder" for `crash.log`.

## Before you start

- [ ] Exit any older version, then start the new exe.
- [ ] Task Manager shows two TaskbarMedia processes (one supervises the other).
- [ ] Play something in Spotify.

## Crash recovery

- [ ] End the *larger* TaskbarMedia process in Task Manager: nothing comes
      back (ending it by hand counts as deliberate).
- [ ] Right-click, Exit: both processes go away.

## Right-click menu

- [ ] Right-click the widget: the styled menu opens above it.
- [ ] Right-click the tray icon: the same menu opens near the mouse.
- [ ] Click elsewhere, and press Esc: it closes both ways.
- [ ] Open (player), Copy track name, Settings and Exit each do their job.
- [ ] Volume mixer and Output device open those popups in its place.
- [ ] With two players running, Player lists both; picking one switches.

## Volume mixer

- [ ] Click the speaker button: the mixer opens above it.
- [ ] System slider changes the system volume.
- [ ] Each app's slider changes only that app (check in the Windows mixer).
- [ ] Clicking a row's speaker icon mutes and un-mutes it.
- [ ] Mouse wheel over a row adjusts it.
- [ ] Start a second app playing: it appears within a second.
- [ ] Click elsewhere: the mixer closes.

## Output device

- [ ] Add "Output device" in Settings, Layout. Click it: the list opens.
- [ ] Pick another device: sound moves, the tick follows, the icon changes.
- [ ] If it says "Could not switch output", or the widget restarts, note it.

## Settings window

- [ ] The live preview at the top matches the real widget.
- [ ] It updates when you change something on Layout or Appearance.
- [ ] Drag a row in the layout list: it moves, and the widget follows.
- [ ] Drag a row near the bottom of the list (after scrolling): still works.
- [ ] Each preset under "Start from" applies and the window refreshes.
- [ ] Export, then Reset everything, then Import: your settings come back.
- [ ] Custom text and highlight colors apply.

## Layout options

- [ ] Hide and show each item in turn; nothing overlaps or gets cut off.
- [ ] Stack shuffle above repeat: both are legible and clickable.
- [ ] Each Button spacing and Button size setting looks right.
- [ ] "Title on its own" and "Seek bar on its own" side by side: seek works.
- [ ] Circle and square album art.
- [ ] Each Text option on Appearance.

## Everyday behaviour

- [ ] Player arrows switch between two players; greyed with one.
- [ ] Shuffle and repeat are greyed out for a browser tab.
- [ ] Hover card appears after resting the mouse, and goes when it leaves.
- [ ] Mouse wheel over the widget changes the player's volume.
- [ ] Full-screen video (YouTube, x.com, F11): the widget hides, and comes
      back afterwards without a click.
- [ ] Restart Explorer (Task Manager): the widget and tray icon return.
- [ ] Sign out and back in with "Start with Windows" on: it starts, and
      `crash.log` has no new lines from signing out.

## Windows 11 only

- [ ] Fill the taskbar with apps: the widget shrinks rather than covering them.
- [ ] On a second display's taskbar, the widget sits just left of the clock.
