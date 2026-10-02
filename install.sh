#!/usr/bin/env bash
# InputForge installer: builds the release binary and installs it with its udev
# rules (per-device uaccess, no 'input' group), icon and desktop entry. Run with: sudo ./install.sh
set -euo pipefail

if [[ $EUID -ne 0 ]]; then
  echo "Run as root: sudo ./install.sh" >&2
  exit 1
fi
TARGET_USER="${SUDO_USER:-}"
if [[ -z "$TARGET_USER" || "$TARGET_USER" == root ]]; then
  echo "Run via sudo from your normal user account." >&2
  exit 1
fi
HERE="$(cd "$(dirname "$0")" && pwd)"
USER_HOME="$(getent passwd "$TARGET_USER" | cut -d: -f6)"

echo "==> Building release binary as $TARGET_USER"
sudo -u "$TARGET_USER" bash -lc "cd '$HERE' && cargo build --release && packaging/render-icons.sh"

echo "==> Installing binary, udev rules, icons, desktop entry"
"$HERE/packaging/install-root.sh" "$TARGET_USER" "$HERE"

cat <<EOF

Done. Launch "InputForge" from your app menu, or run: inputforge
Turning a keyboard or mouse on asks for your password once, to grant access to that device.
Closing the window keeps it in the system tray; turn on Settings -> "Start at login".

Optional extras for the "DPI & RGB" tab:
  sudo pacman -S libratbag && sudo systemctl enable --now ratbagd   # mouse DPI / polling / LEDs
  sudo pacman -S openrgb  && openrgb --server                       # RGB lighting
EOF
