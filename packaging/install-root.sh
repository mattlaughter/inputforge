#!/usr/bin/env bash
# Root-only part of the install (no build). Usage: pkexec install-root.sh <user> <project-dir>
set -euo pipefail
U="$1"; HERE="$2"
[[ $EUID -eq 0 ]] || { echo "must run as root" >&2; exit 1; }
id "$U" >/dev/null
install -Dm755 "$HERE/target/release/inputforge" /usr/local/bin/inputforge
install -Dm644 "$HERE/packaging/99-inputforge.rules" /etc/udev/rules.d/99-inputforge.rules
install -Dm644 "$HERE/packaging/70-inputforge-lighting.rules" /etc/udev/rules.d/70-inputforge-lighting.rules
echo uinput > /etc/modules-load.d/inputforge.conf
modprobe uinput || true
udevadm control --reload-rules
udevadm trigger --subsystem-match=input --subsystem-match=misc --subsystem-match=hidraw
usermod -aG input "$U"
install -Dm644 "$HERE/packaging/inputforge.desktop" /usr/share/applications/inputforge.desktop
install -Dm644 "$HERE/assets/inputforge.svg" /usr/share/icons/hicolor/scalable/apps/inputforge.svg
for s in 16 22 24 32 48 64 128 256 512; do
  [[ -f "$HERE/target/icons/inputforge-$s.png" ]] && install -Dm644 "$HERE/target/icons/inputforge-$s.png" "/usr/share/icons/hicolor/${s}x${s}/apps/inputforge.png"
done
gtk-update-icon-cache -qtf /usr/share/icons/hicolor 2>/dev/null || true
update-desktop-database -q /usr/share/applications 2>/dev/null || true
echo "OK"
