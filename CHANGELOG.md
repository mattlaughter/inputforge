# Changelog

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
