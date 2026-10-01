# RPM spec for Fedora-based distros (Nobara, Fedora).
# Build with packaging/rpm/build-rpm.sh, which creates the source and vendored-crate
# tarballs and runs rpmbuild. Builds fully offline from the vendored crates.

# Use `--with rustup` when the distro's Rust is older than the crates need (1.95):
# the build then uses whatever cargo is on PATH (e.g. ~/.cargo/bin from rustup).
%bcond_with rustup

Name:           inputforge
Version:        0.1.0
Release:        1%{?dist}
Summary:        Keyboard and mouse remapping, macros, autoclicker and RGB lighting

License:        MIT
URL:            https://github.com/mattlaughter/inputforge
Source0:        %{name}-%{version}.tar.gz
Source1:        %{name}-%{version}-vendor.tar.gz

ExclusiveArch:  x86_64 aarch64

%if %{without rustup}
BuildRequires:  cargo >= 1.95
BuildRequires:  rust >= 1.95
%endif
BuildRequires:  gcc
BuildRequires:  librsvg2-tools
BuildRequires:  desktop-file-utils
BuildRequires:  systemd-rpm-macros

# Loaded at runtime by winit/glutin (dlopen), so rpm can't detect them.
Requires:       libxkbcommon
Requires:       libwayland-client
Requires:       libwayland-egl
Requires:       mesa-libEGL
Requires:       libX11
Requires:       libXcursor
Requires:       libXi
Requires:       libxkbcommon-x11
# udevadm / modprobe in %%post
Requires(post): systemd-udev
Requires(post): kmod

%description
InputForge controls keyboards and mice at the kernel level (evdev + uinput), so it
works on Wayland, X11 and the console: key and button remapping, shortcuts,
macros, an autoclicker, pointer speed, and built-in RGB lighting for the Logitech
G815/G813 keyboards and G600 mouse. Runs in the system tray.

%prep
%autosetup -n %{name}-%{version}
tar -xzf %{SOURCE1}
mkdir -p .cargo
cat > .cargo/config.toml <<'EOF'
[source.crates-io]
replace-with = "vendored-sources"

[source.vendored-sources]
directory = "vendor"
EOF

%build
export CARGO_HOME="$PWD/.cargo-home"
cargo build --release --locked --offline
packaging/render-icons.sh

%install
install -Dm755 target/release/inputforge %{buildroot}%{_bindir}/inputforge
install -Dm644 packaging/99-inputforge.rules %{buildroot}%{_udevrulesdir}/99-inputforge.rules
install -Dm644 packaging/70-inputforge-lighting.rules %{buildroot}%{_udevrulesdir}/70-inputforge-lighting.rules
install -dm755 %{buildroot}%{_modulesloaddir}
echo uinput > %{buildroot}%{_modulesloaddir}/inputforge.conf
desktop-file-install --dir=%{buildroot}%{_datadir}/applications packaging/inputforge.desktop
install -Dm644 assets/inputforge.svg %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/inputforge.svg
for s in 16 22 24 32 48 64 128 256 512; do
  install -Dm644 target/icons/inputforge-$s.png \
    %{buildroot}%{_datadir}/icons/hicolor/${s}x${s}/apps/inputforge.png
done

%check
cargo test --release --locked --offline

%post
%udev_rules_update
modprobe uinput >/dev/null 2>&1 || :
udevadm trigger --subsystem-match=input --subsystem-match=misc --subsystem-match=hidraw >/dev/null 2>&1 || :
if [ $1 -eq 1 ]; then
  echo "InputForge: add yourself to the 'input' group, then log out and back in:"
  echo "    sudo usermod -aG input \$USER"
fi

%postun
%udev_rules_update

%files
%license LICENSE
%doc README.md
%{_bindir}/inputforge
%{_udevrulesdir}/99-inputforge.rules
%{_udevrulesdir}/70-inputforge-lighting.rules
%{_modulesloaddir}/inputforge.conf
%{_datadir}/applications/inputforge.desktop
%{_datadir}/icons/hicolor/*/apps/inputforge.*

%changelog
* Wed Sep 30 2026 oliverpissed - 0.1.0-1
- First RPM package
