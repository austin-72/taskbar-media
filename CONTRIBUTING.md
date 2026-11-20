# Contributing

Bug reports and pull requests are welcome.

## Reporting a bug

Use the bug report template. The version, your Windows version, the last lines of
`crash.log` (Settings, More, "Open the log folder") and a picture (Settings,
More, "Save pictures of the widget") answer most first questions.

## Building

You need Windows 10 or 11 with:

- [Rust](https://rustup.rs) 1.85 or later (the `x86_64-pc-windows-msvc`
  toolchain, which is the default on Windows)
- Visual Studio Build Tools with the "Desktop development with C++" workload,
  which brings the MSVC linker and the Windows SDK (for `rc.exe`)

Then:

    cargo build --release
    cargo test

The exe is `target\release\TaskbarMedia.exe`. `.cargo/config.toml` links the
C runtime statically, so the exe needs nothing installed beside it.

## Layout of the code

| File | What is in it |
| --- | --- |
| `src/main.rs` | Windows, timers, mouse and keyboard handling, popups, the supervisor |
| `src/render.rs` | Layout and hit-testing (pure, unit tested) and all drawing (GDI+) |
| `src/settings.rs` | The settings window |
| `src/config.rs` | Settings: defaults, storage in the registry, import and export |
| `src/media.rs` | The worker thread that talks to Windows' media sessions |
| `src/volume.rs`, `src/devices.rs` | Volume mixer and output devices (Core Audio) |
| `src/probe.rs`, `src/reserve.rs` | Keeping clear of app buttons on Windows 11 and 10 |
| `src/launch.rs` | Finding and opening the player's window |
| `src/tray.rs`, `src/update.rs`, `src/crash.rs` | Tray icon, update check, crash log |

## Before sending a pull request

- `cargo test` passes and `cargo clippy` is no noisier than before. The
  tests cover the logic that does not need a taskbar: layout and hit-testing,
  settings parsing and presets, which player is followed, text placement,
  colors, version comparison.
- Anything a user can see is described in `CHANGELOG.md` under the next
  version, and in `README.md` if it is a feature.
- Go through the relevant part of [docs/TESTING.md](docs/TESTING.md). Much of
  this app is about how it sits on a real taskbar, which no automated test
  covers.

## Releasing

1. Set `version` in `Cargo.toml` (and run a build so `Cargo.lock` follows).
2. Add the version to `CHANGELOG.md`.
3. Push, then publish a GitHub release tagged `v` plus that version.

The workflow refuses a tag that does not match `Cargo.toml`, runs the tests,
and attaches `TaskbarMedia.exe` and `TaskbarMedia.exe.sha256` to the release.
Wait for a green run on `main` before publishing, so that a failing test
cannot leave a release without its exe. The debug symbols
(`TaskbarMedia.pdb`) are kept with each workflow run; with them, an
`exe+0x...` address from a user's `crash.log` can be turned into a source
line.

## Testing crash recovery

Set the environment variable `TASKBARMEDIA_TEST` before starting the exe to
make the widget fail on purpose about five seconds after it starts:
`crash` makes it crash, `hang` makes it stop responding. See
[docs/TESTING.md](docs/TESTING.md).
