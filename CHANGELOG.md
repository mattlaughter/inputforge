# Changelog

## Unreleased

- Wheel tilt (Razer Naga and anything else that sends `REL_HWHEEL` instead of a button) shows up in Assignments as Tilt left / Tilt right, and can be remapped. A held tilt repeats about every 20ms; one gesture is one press, not a burst of clicks, and it no longer also scrolls sideways.
- Hardware DPI for the Razer Naga V2 HyperSpeed on its Sensitivity page (100–30000). Sent as a HID feature report, as OpenRazer does, and the page shows the DPI the mouse reports back. It reads the current DPI when the page opens, writes once when you let go of the slider, and has a refresh button for after you use the mouse's own DPI buttons. `inputforge --naga-dpi [N]` reads or sets it from a terminal.

## 0.3.1 — 2026-10-02

- G915 TKL over LIGHTSPEED: the keyboard reports itself as `046d:408e` (its receiver as `c545` or
  `c547`), which 0.3.0 didn't recognise, so there was no Lighting page. Both are recognised now, and
  behind a receiver InputForge finds the keyboard's paired slot itself instead of assuming slot 1.
- `--lighting-info` now probes the keyboard (read-only) and prints which HID++ features it
  answers for, per receiver slot.
- Replies are matched to the slot they were sent to, so another HID++ program (Solaar) talking to
  the same receiver can't be mistaken for our answers.
- Known limitation, documented in the README: Per-key, Solid and effect modes take lighting
  control from the keyboard's onboard profile, so its own idle-dim/sleep timers stop applying
  until onboard mode is restored (e.g. by switching profile on the keyboard).

## 0.3.0 — 2026-10-02

- **New: Logitech G915 TKL lighting** (wired `046d:c343`, or through its LIGHTSPEED receiver
  `046d:c545`): static, per-key, breathing, cycle and off, with a TKL-shaped key painter (no numpad,
  no G-keys). It uses the G815's per-key protocol; because the TKL numbers its HID++ features
  differently, InputForge asks the keyboard where they are instead of assuming. **Not yet tested
  on real hardware**; please report results.
- The udev rules cover the TKL's lighting interface (interface 2) only, never its typing interface.

## 0.2.1 — 2026-10-02

- Fix: in 0.2.0 the lighting rule never matched (udev needs every `ATTRS{}` key on the same
  parent, but vendor id and interface number are on different ones), so lighting failed with
  "No permission to change lighting (/dev/hidraw5)" once the `input` group was removed. The rule
  now matches on `ENV{ID_VENDOR_ID/ID_MODEL_ID/ID_USB_INTERFACE_NUM}`.

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
