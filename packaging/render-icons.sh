#!/usr/bin/env bash
# Render the app icon to PNG sizes for the hicolor theme (needs rsvg-convert).
set -euo pipefail
cd "$(dirname "$0")/.."
mkdir -p target/icons
for s in 16 22 24 32 48 64 128 256 512; do
  rsvg-convert -w "$s" -h "$s" assets/inputforge.svg -o "target/icons/inputforge-$s.png"
done
