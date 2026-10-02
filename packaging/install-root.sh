#!/usr/bin/env bash
# Root-only part of the install (no build). Usage: pkexec install-root.sh <user> <project-dir>
# Access model: per-device, per-session udev uaccess ACLs. Nobody is added to the
# 'input' group (that would let every program the user runs read every keyboard).
set -euo pipefail
U="$1"; HERE="$2"
[[ $EUID -eq 0 ]] || { echo "must run as root" >&2; exit 1; }
id "$U" >/dev/null
install -Dm755 "$HERE/target/release/inputforge" /usr/local/bin/inputforge
# Rules from older versions (group-based uinput, over-broad hidraw).
rm -f /etc/udev/rules.d/99-inputforge.rules /etc/udev/rules.d/70-inputforge-lighting.rules
install -Dm644 "$HERE/packaging/70-inputforge.rules" /etc/udev/rules.d/70-inputforge.rules
echo uinput > /etc/modules-load.d/inputforge.conf
modprobe uinput || true
udevadm control --reload-rules
udevadm trigger --subsystem-match=misc --subsystem-match=hidraw --subsystem-match=input --action=change
udevadm settle --timeout=10 || true
install -Dm644 "$HERE/packaging/inputforge.desktop" /usr/share/applications/inputforge.desktop
install -Dm644 "$HERE/assets/inputforge.svg" /usr/share/icons/hicolor/scalable/apps/inputforge.svg
for s in 16 22 24 32 48 64 128 256 512; do
  [[ -f "$HERE/target/icons/inputforge-$s.png" ]] && install -Dm644 "$HERE/target/icons/inputforge-$s.png" "/usr/share/icons/hicolor/${s}x${s}/apps/inputforge.png"
done
gtk-update-icon-cache -qtf /usr/share/icons/hicolor 2>/dev/null || true
update-desktop-database -q /usr/share/applications 2>/dev/null || true
if id -nG "$U" | tr ' ' '\n' | grep -qx input; then
  echo "NOTE: $U is still in the 'input' group from an older InputForge install."
  echo "      InputForge no longer needs it. Remove it with: sudo gpasswd -d $U input"
fi
echo "OK"
