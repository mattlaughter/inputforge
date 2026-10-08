<p align="center"><img src="assets/inputforge.svg" width="112" alt="InputForge icon"></p>

# InputForge

Keyboard and mouse control for Linux, in the style of Logitech G HUB, written in Rust. It works
at the kernel level (evdev + uinput), so remaps work the same on Wayland (KDE, Hyprland, GNOME),
X11, and in games.

![Home screen](docs/home.png)

## Screenshots

| | |
|---|---|
| ![Per-key keyboard lighting](docs/keyboard-lighting.png) | ![Mouse assignments](docs/mouse-assignments.png) |
| **Per-key lighting:** click or drag to paint keys on the G815 | **Assignments:** every button with its current function; click to change |
| ![Mouse lighting](docs/mouse-lighting.png) | ![Macro editor](docs/macros.png) |
| **Mouse lighting:** solid, breathing, rainbow, or match the keyboard | **Macros:** taps, held keys, typed text, delays and mouse moves |
| ![Settings](docs/settings.png) | |
| **Settings:** start at login in the tray, restore lighting, theme | |

## Features

- **Devices at a glance.** The home screen shows one card per keyboard and mouse. Click a card
  for its **Lighting**, **Assignments** and **Sensitivity** pages.
- **Assignments.** Every key or button the device has is listed with what it currently does.
  Click one to change it to another key, a shortcut (e.g. Ctrl+C), a macro, the autoclicker
  toggle, or nothing. Wheel tilt (a repeating horizontal scroll, not a button) is listed as
  Tilt left / Tilt right and can be remapped the same way. *Find by pressing* jumps to a key
  or tilt when you press it. Profiles let you keep several sets of assignments.
- **Built-in RGB lighting** for the Logitech G815/G813 and G915 TKL (wired or LIGHTSPEED receiver;
  the TKL is new and untested on hardware: per-key painting, solid, breathing,
  rainbow) and G600 (solid, breathing, rainbow). No OpenRGB or other tools are needed. The
  colours are restored at login.
- **Macros:** taps, key down/up, typed text (US QWERTY layout only), delays, mouse moves and scrolls. Each macro runs
  once per press, repeats while held, or loops until toggled off.
- **Autoclicker:** any button or key, 0.5–100 clicks/s, an optional click limit and a hotkey.
- **Sensitivity:** pointer and scroll speed per mouse, plus natural scrolling.
- **System tray.** Closing the window keeps everything running. Left-click the tray icon to
  open the window, middle-click to pause or resume, and use the menu to switch profiles or
  quit. You can also have it start at login.
- **Follows your desktop theme:** colours, light/dark mode, accent colour and fonts from KDE,
  GTK or the XDG portal.

**Emergency release:** press Left Ctrl + Right Ctrl + Esc to stop and let go of every device.

Settings → Advanced has the raw device list, a full rule editor, libratbag (DPI and polling for
other mice) and OpenRGB (other RGB devices).

## Install

### Arch / CachyOS / EndeavourOS

```bash
git clone https://github.com/mattlaughter/inputforge
cd inputforge/packaging/arch
makepkg -si
```

### Nobara / Fedora

```bash
sudo dnf install git rpm-build cargo rust gcc librsvg2-tools desktop-file-utils systemd-rpm-macros
git clone https://github.com/mattlaughter/inputforge
cd inputforge
packaging/rpm/build-rpm.sh
sudo dnf install ~/rpmbuild/RPMS/x86_64/inputforge-*.rpm
```

If the distro's Rust is older than 1.95, install rustup (`sudo dnf install rustup && rustup-init`)
and build with `packaging/rpm/build-rpm.sh --rustup`. The RPM build is fully offline because all
crates are vendored. `build-rpm.sh --srpm` makes a source RPM you can upload to Fedora COPR.

A ready-built Arch package is attached to each
[release](https://github.com/mattlaughter/inputforge/releases)
(`sudo pacman -U inputforge-*.pkg.tar.zst`).

### Any distro (from source)

Requires Rust 1.95 or newer.

```bash
cargo build --release
sudo ./install.sh     # binary to /usr/local/bin, udev rules, icons, menu entry, adds you to 'input'
```

## Logitech G915 TKL notes

Works wired or over the LIGHTSPEED receiver (the keyboard shows up as `046d:408e`). It is untested
by the author, so please report problems with the output of `inputforge --lighting-info`.

- Setting lighting from InputForge takes control from the keyboard's **onboard profile**. While
  it holds control, the keyboard's own idle dim and sleep timers (the ones Solaar or G HUB
  configure) don't run. Switching the onboard profile on the keyboard hands control back.
- InputForge doesn't read battery, switch hosts or change report rate. Keep Solaar for those; the
  two can run together as long as only one of them is changing lighting at a time. InputForge writes
  nothing to the keyboard until you change something on its Lighting page (or enable
  *Apply lighting at launch*).

## Permissions (no `input` group)

InputForge does **not** put you in the `input` group. That group lets every program you run read
every keyboard, which is keylogger-level access. Instead, access is granted per device and per
session through udev `uaccess` ACLs:

- **Virtual output** (`/dev/uinput`): granted to the user at the active local desktop, the same
  rule Steam uses for controller emulation.
- **Your keyboards and mice:** nothing by default. When you turn a device on in InputForge, it
  asks for your password once and adds a rule for **that device only** to
  `/etc/udev/rules.d/70-inputforge-devices.rules`. Turning it off doesn't remove the rule; use
  `pkexec inputforge --udev-revoke vvvv:pppp` (ids from `inputforge --list-devices`).
- **Lighting:** only the vendor-specific HID++/config interface of the supported Logitech devices
  is exposed over hidraw, never their typing interface.

What this does and doesn't protect: programs in other sessions, SSH logins and background
services get nothing. A program running in *your* desktop session can still read a device you
granted whenever InputForge isn't holding it, and while InputForge is active it holds the device
exclusively. The self-test reads InputForge's virtual devices, so it asks for your password.

Upgrading from 0.1 removes the old group-based rules. If 0.1 added you to `input`, remove yourself
with `sudo gpasswd -d $USER input`, log out and back in, then turn your devices on again.

## CLI

```bash
inputforge                  # open the window (or show the running one)
inputforge --background     # start in the tray only (used at login)
inputforge --lighting-info   # Logitech hidraw nodes and how they're classified (for bug reports)
inputforge --list-devices   # devices with vendor:product ids and access status
inputforge --apply-lighting # apply the saved lighting and exit
inputforge --selftest       # end-to-end engine test on a fake device (asks for password)
```

The config is saved automatically to `~/.config/inputforge/config.toml`.

## How it works

Each device you set up is opened with `EVIOCGRAB`, which gives InputForge exclusive access to
it. Its events go through the active profile's assignments and come back out through two
virtual devices, *InputForge Virtual Keyboard* and *InputForge Virtual Pointer*. The compositor
only sees those virtual devices, so a remap works in every application. Unplugged devices are
picked up again within a couple of seconds.

The lighting drivers talk to the devices' HID++ interfaces directly over `hidraw`. The
protocol details come from [OpenRGB](https://gitlab.com/CalcProgrammer1/OpenRGB)'s
documentation of those devices.

## License

[MIT](LICENSE) © 2026 Matt Laughter
