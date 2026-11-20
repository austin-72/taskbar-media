# Test checklist

A pass through everything, roughly riskiest first. Tick what works; anything
that does not, note what you saw. If the widget disappears at any point,
check Settings, More, "Open the log folder" for `crash.log`.

## Before you start

- [ ] Exit any older version, then start the new exe.
- [ ] Task Manager's Details tab shows two TaskbarMedia.exe processes (one
      supervises the other; the Processes tab may group them as one).
- [ ] Start the exe again while it is running: the settings window opens.
- [ ] Play something in Spotify.

## Crash recovery

- [ ] On the Details tab, end the TaskbarMedia.exe that uses more memory
      (the widget): nothing comes back (ending it by hand counts as
      deliberate), and on Windows 10 the app buttons get their space back.
- [ ] Start it again, then end the *smaller* one (the supervisor): the
      widget keeps running. Exit it from its menu.
- [ ] Right-click, Exit: both processes go away.
- [ ] `taskkill /im TaskbarMedia.exe` (without `/f`) in a command prompt:
      both processes end within a few seconds. (taskkill reports that one
      of the two could not be ended that way: that is the supervisor, which
      has no window; it ends by itself once the widget has.)
- [ ] A real crash: in a command prompt, `set TASKBARMEDIA_TEST=crash`, then
      start `TaskbarMedia.exe` from that prompt. The widget appears,
      vanishes after about five seconds and comes back, five times in all;
      then it stays away, and `crash.log` says why. On Windows 10 the app
      buttons have their full space back at the end.
- [ ] A hang: the same with `set TASKBARMEDIA_TEST=hang`. The widget freezes
      after about five seconds; within about a minute and a half it is
      ended and restarted, and `crash.log` says it stopped responding.
      (Close that command prompt afterwards, or exit from Task Manager.)
- [ ] With the widget running, end the supervisor (the smaller process),
      then start the exe again: the settings window opens.
- [ ] Alt+F4 while the right-click menu is open only closes the menu; the
      menu, mixer and device list still open afterwards.

## Right-click menu

- [ ] Right-click the widget: the styled menu opens above it.
- [ ] Right-click the tray icon: the same menu opens near the mouse.
- [ ] Click elsewhere, and press Esc: it closes both ways.
- [ ] Up / Down move the highlight and Enter chooses.
- [ ] Press on a row, slide off it and let go: nothing happens.
- [ ] Open (player), Copy track name, Settings and Exit each do their job.
- [ ] Volume mixer and Output device open those popups in its place.
- [ ] With two players running, Player lists both; picking one switches.

## Volume mixer

- [ ] Click the speaker button: the mixer opens above it.
- [ ] System slider changes the system volume.
- [ ] Each app's slider changes only that app (check in the Windows mixer).
- [ ] Clicking a row's speaker icon mutes and un-mutes it.
- [ ] Mouse wheel over a row adjusts it; on a touchpad, a slow two-finger
      scroll changes it gradually rather than jumping to 0 or 100.
- [ ] Up / Down choose a row, Left / Right change it, Space mutes.
- [ ] Press Esc while dragging a slider: the mixer closes and stays closed.
- [ ] With many apps making sound, the mixer stays on the screen and its
      last line says how many more apps there are.
- [ ] From the right-click menu, choose Volume mixer: the menu turns into
      the mixer without flashing, and Esc still closes it.
- [ ] Start a second app playing: it appears within a second.
- [ ] Click elsewhere: the mixer closes.

## Output device

- [ ] Add "Output device" in Settings, Layout. Click it: the list opens.
- [ ] Pick another device: sound moves, the tick follows, the icon changes.
- [ ] If it says "Could not switch output", or the widget restarts, note it.
- [ ] The volume button's level follows the new device straight away.

## Settings window

- [ ] The live preview at the top matches the real widget.
- [ ] It updates when you change something on Layout or Appearance.
- [ ] Drag a row in the layout list: it moves, and the widget follows.
- [ ] Drag a row near the bottom of the list (after scrolling): still works.
- [ ] Drag a row past the bottom edge of the list: the list scrolls and the
      row keeps moving down.
- [ ] Choose "Other taskbars" under Editing, then a preset: the window stays
      where it was, still editing the other taskbars, and the main
      taskbar's widget does not change.
- [ ] Hide every item one by one: the last one refuses, with a message.
- [ ] Ctrl+Tab moves through the tabs; Enter in a list does not close the
      window; Esc does.
- [ ] On a second display with different scaling, the window is the right
      size, and dragging it between displays resizes it on release.
- [ ] Choosing a preset under "Start from" changes nothing until Apply is
      pressed and confirmed; then it applies and the window refreshes.
- [ ] Hold a dragged row still just below the list: it keeps moving down.
- [ ] Alt+Tab away and back: the keyboard focus is where it was.
- [ ] Export, then Reset everything, then Import: your settings come back.
- [ ] Right after "Reset everything", Export and then Import that file: it
      is accepted.
- [ ] Import some unrelated text file: it is refused and nothing changes.
- [ ] "Save pictures of the widget" opens a folder with five PNG files.
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
- [ ] Mouse wheel over the widget changes the player's volume (also with
      Firefox as the player).
- [ ] Pause Spotify while a browser tab has long been making sound: the
      widget stays on Spotify. Start a video in the browser: it moves to
      the browser.
- [ ] Start a video, then pause Spotify a few seconds later: the widget
      moves to the browser at once.
- [ ] With a player picked by hand, choose Automatic: the widget goes
      straight to whatever is playing.
- [ ] Repeat one track: a "1" is clearly visible in or beside the repeat icon
      (also with shuffle and repeat stacked).
- [ ] With "Show accent color on Start and taskbar" on in Windows, the seek
      bar's played part is easy to tell from the taskbar.
- [ ] A long title scrolls smoothly.
- [ ] Light theme (Settings, Personalization, Colors): text and highlight
      are easy to read.
- [ ] "Use small taskbar buttons" (Windows 10): the title is still readable.
- [ ] Taskbar at the top of the screen: popups open below the widget.
- [ ] Auto-hide taskbar: the widget hides and returns with it.
- [ ] Full-screen video (YouTube, x.com, F11): the widget hides, and comes
      back afterwards without a click.
- [ ] Restart Explorer (Task Manager): the widget and tray icon return.
- [ ] Sign out and back in with "Start with Windows" on: it starts, the
      tray icon is there, and `crash.log` has no new lines from signing out.
- [ ] Move the exe to another folder and run it there: after signing out
      and in, that copy starts.
- [ ] Turn off the network, start the app, turn the network back on: within
      about five minutes of an available update, the dot appears.

## Windows 10 only

- [ ] Fill the taskbar with apps: the app buttons stop short of the widget.
- [ ] On a taskbar of a second display, the widget sits just left of the clock.

## Windows 11 only

- [ ] Fill the taskbar with apps: the widget shrinks rather than covering them.
- [ ] On a second display's taskbar, the widget sits just left of the clock.
