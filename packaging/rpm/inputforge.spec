# RPM spec for Fedora-based distros (Nobara, Fedora).
# Build with packaging/rpm/build-rpm.sh, which creates the source and vendored-crate
# tarballs and runs rpmbuild. Builds fully offline from the vendored crates.

# Use `--with rustup` when the distro's Rust is older than the crates need (1.95):
# the build then uses whatever cargo is on PATH (e.g. ~/.cargo/bin from rustup).
%bcond_with rustup

Name:           inputforge
Version:        0.3.1
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
%udev_rules_update
modprobe uinput >/dev/null 2>&1 || :
udevadm trigger --subsystem-match=misc --subsystem-match=hidraw --subsystem-match=input --action=change >/dev/null 2>&1 || :
if [ $1 -eq 1 ]; then
  echo "InputForge: turning a keyboard or mouse on asks for your password once to grant access"
  echo "to that device. No 'input' group membership is needed."
fi

%postun
%udev_rules_update

%files
%license LICENSE
%doc README.md
%{_bindir}/inputforge
%{_udevrulesdir}/70-inputforge.rules
%{_modulesloaddir}/inputforge.conf
%{_datadir}/applications/inputforge.desktop
%{_datadir}/icons/hicolor/*/apps/inputforge.*

%changelog
* Fri Oct 02 2026 Matt Laughter - 0.3.1-1
- G915 TKL: recognise 046d:408e / receiver c547; find the paired slot behind a receiver

* Fri Oct 02 2026 Matt Laughter - 0.3.0-1
- Add Logitech G915 TKL lighting (wired and LIGHTSPEED receiver)

* Fri Oct 02 2026 Matt Laughter - 0.3.0-1
- Add Logitech G915 TKL lighting (wired and LIGHTSPEED receiver)

* Fri Oct 02 2026 Matt Laughter - 0.2.1-1
- Fix lighting hidraw uaccess rule (ENV match; ATTRS keys must share one parent)

* Thu Oct 01 2026 Matt Laughter - 0.2.0-1
- Per-device uaccess udev rules instead of the input group

* Thu Oct 01 2026 Matt Laughter - 0.1.1-1
- Fix cursor stutter every 2 s; mirror KDE pointer settings; forward all mouse buttons

* Wed Sep 30 2026 Matt Laughter - 0.1.0-1
- First RPM package
