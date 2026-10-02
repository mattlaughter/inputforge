# Changelog

## 0.2.0 — 2026-10-01

**Permissions change: no more `input` group.** Membership in `input` gives every program the
user runs read access to every keyboard. Access is now per device and per session (udev
`uaccess` ACLs):

- `/dev/uinput` via `uaccess` (same as Steam's `60-steam-input.rules`).
- Keyboards/mice: none by default. Turning a device on in InputForge asks for the password once
  (`pkexec inputforge --udev-allow vvvv:pppp`) and adds a rule for that device only.
  `--udev-revoke` removes it.
- Lighting hidraw access is narrowed to the vendor HID++/config interface (interface 1) of the
  G815/G813/G600. 0.1 also exposed their typing/pointer interfaces over hidraw.
- Devices are listed from sysfs, so they appear in the app before access is granted.
- `--selftest` re-runs itself through pkexec, since it reads InputForge's own virtual devices.
- Installers and packages no longer add the user to `input`. Upgrades remove the 0.1 rules and
  print how to leave the group.

## 0.1.1 — 2026-10-01

- **Fix: cursor stutter.** The engine rescanned every input device every 2 s on the thread that
  forwards events, freezing the pointer for ~130 ms each time. It now only rescans when
  `/dev/input` changes.
- Mouse: KDE Plasma pointer settings (acceleration profile/speed, natural scroll, left-handed,
  scroll factor) of the grabbed mouse are copied to the virtual pointer, so System Settings
  keeps applying.
- Mouse: forward all 16 mouse buttons (0x110–0x11F); the G600's extra buttons were dropped.
- Return the window's memory to the OS when it is closed to the tray.

- Pin the Rust toolchain to 1.95 (`rust-toolchain.toml` + CI) so builds match the MSRV.
- Clamp pointer/scroll speed multipliers in the engine and on config load to the UI ranges
  (pointer 0.1–5.0, scroll 0.25–5.0), so hand-edited TOML cannot go wild.
- Split the device Sensitivity panel into `src/app/sensitivity.rs`.
- Fix: the emergency stop (Left Ctrl + Right Ctrl + Esc) now works when the keys arrive on
  different input nodes of the same keyboard. Covered by unit tests and `--selftest`.
- Autoclicker: engine rate clamp now matches the UI and docs (0.5–100 clicks/s).
- Add CI (cargo test + clippy).
- Docs: macro text is US QWERTY only; Rust 1.95+ required to build.

## 0.1.0

First release: keyboard/mouse remapping, macros, autoclicker, RGB lighting (G815/G813, G600),
system tray, Arch and RPM packaging.
