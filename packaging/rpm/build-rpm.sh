#!/usr/bin/env bash
# Build InputForge RPMs. Run on Nobara/Fedora as your normal user (not root).
#
#   packaging/rpm/build-rpm.sh            # uses the distro's Rust
#   packaging/rpm/build-rpm.sh --rustup   # uses Rust from rustup (~/.cargo/bin)
#   packaging/rpm/build-rpm.sh --srpm     # only make the source RPM (for COPR)
#
# Output: ~/rpmbuild/RPMS/<arch>/inputforge-*.rpm and ~/rpmbuild/SRPMS/
set -euo pipefail
cd "$(dirname "$0")/../.."
HERE="$PWD"

RUSTUP=0; SRPM_ONLY=0
for a in "$@"; do
  case "$a" in
    --rustup) RUSTUP=1 ;;
    --srpm) SRPM_ONLY=1 ;;
    *) echo "unknown option: $a" >&2; exit 2 ;;
  esac
done

NAME=inputforge
VERSION=$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)
SPEC_VERSION=$(sed -n 's/^Version: *//p' packaging/rpm/inputforge.spec)
if [[ "$VERSION" != "$SPEC_VERSION" ]]; then
  echo "Cargo.toml version ($VERSION) != spec Version ($SPEC_VERSION); update the spec." >&2
  exit 1
fi

need() { command -v "$1" >/dev/null || { echo "missing '$1' — run: sudo dnf install $2" >&2; exit 1; }; }
need rpmbuild rpm-build
need cargo "cargo rust  (or install rustup and use --rustup)"
need rsvg-convert librsvg2-tools

# The crates need Rust 1.95+.
RUSTV=$(rustc --version | awk '{print $2}')
if [[ "$(printf '%s\n1.95.0\n' "$RUSTV" | sort -V | head -1)" != "1.95.0" ]]; then
  echo "Rust $RUSTV is too old (need 1.95+). Install rustup (sudo dnf install rustup && rustup-init)," >&2
  echo "then re-run with --rustup." >&2
  exit 1
fi

TOP="$HOME/rpmbuild"
mkdir -p "$TOP"/{SOURCES,SPECS,BUILD,RPMS,SRPMS}
WORK=$(mktemp -d)
trap 'rm -rf "$WORK"' EXIT

echo "==> Source tarball"
mkdir -p "$WORK/$NAME-$VERSION"
tar --exclude=./target --exclude=./vendor --exclude=./.git --exclude=./.cargo -cf - . \
  | tar -xf - -C "$WORK/$NAME-$VERSION"
tar -czf "$TOP/SOURCES/$NAME-$VERSION.tar.gz" -C "$WORK" "$NAME-$VERSION"

echo "==> Vendoring crates (offline build)"
cargo vendor --locked --quiet "$WORK/vendor" >/dev/null
tar -czf "$TOP/SOURCES/$NAME-$VERSION-vendor.tar.gz" -C "$WORK" vendor
cp packaging/rpm/inputforge.spec "$TOP/SPECS/"

if (( SRPM_ONLY )); then
  rpmbuild -bs "$TOP/SPECS/inputforge.spec"
  exit 0
fi

WITH=()
(( RUSTUP )) && WITH=(--with rustup)
echo "==> rpmbuild"
rpmbuild -ba "${WITH[@]}" "$TOP/SPECS/inputforge.spec"
echo
ls -1 "$TOP"/RPMS/*/"$NAME"-*.rpm "$TOP"/SRPMS/"$NAME"-*.src.rpm
echo
echo "Install with:  sudo dnf install $TOP/RPMS/$(uname -m)/$NAME-$VERSION-*.rpm"
