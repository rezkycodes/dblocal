Name:           dblocal
Version:        %{_dblocal_version}
Release:        1%{?dist}
Summary:        Local database server manager

License:        GPL-3.0-only
URL:            https://github.com/rezkycodes/dblocal
Source0:        dblocal
Source1:        dev.dblocal.DBLocal.desktop
Source2:        dblocal.svg
Source3:        COPYING
Source4:        LICENSE-RING-AWSLC-NOTES.md
Source10:       dblocal-16.png
Source11:       dblocal-24.png
Source12:       dblocal-32.png
Source13:       dblocal-48.png
Source14:       dblocal-64.png
Source15:       dblocal-128.png
Source16:       dblocal-256.png
Source17:       dblocal-512.png

# Binary package built from a prebuilt DBLocal release binary (F-PKG-04).
# The build host does not compile DBLocal; CI provides the release binary.
%global debug_package %{nil}
%global __strip /bin/true
%global __os_install_post %{nil}

# Runtime dependencies derived from DT_NEEDED (readelf -d) of the release
# binary: libc.so.6, libgcc_s.so.1, libm.so.6, libxcb.so.1,
# libxkbcommon.so.0, libxkbcommon-x11.so.0, libz.so.1, ld-linux-x86-64.so.2.
# Fedora/RHEL package names:
#   libc.so.6, libm.so.6, ld-linux -> glibc
#   libgcc_s.so.1                  -> libgcc
#   libxcb.so.1                    -> libxcb
#   libxkbcommon.so.0              -> libxkbcommon
#   libxkbcommon-x11.so.0          -> libxkbcommon-x11
#   libz.so.1                      -> zlib-ng-compat or zlib (virtual provides)
# Optional runtime backends loaded by dlopen in the GPUI stack (F4):
# Vulkan, EGL, and Wayland client. Not hard Requires: GPUI can fall back to
# X11/xcb; missing optional backends should not block install on minimal/headless
# systems. libvulkan missing is reported with an actionable message (F-SPK-01 AC2).
Requires:       glibc
Requires:       libgcc
Requires:       libxcb
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       zlib
Recommends:     vulkan-loader
Recommends:     libwayland-client
Recommends:     mesa-libEGL

%description
DBLocal installs and runs local PostgreSQL, MySQL, MariaDB, and Valkey
servers side by side with no containers and no sudo. It keeps engine data
under ~/.dblocal and never exposes database ports beyond localhost.

%prep
# No source extraction: package is assembled from prebuilt inputs.

%build
# No build: binary is prebuilt.

%install
rm -rf %{buildroot}
install -D -m 0755 %{SOURCE0} %{buildroot}%{_bindir}/dblocal
install -D -m 0644 %{SOURCE1} %{buildroot}%{_datadir}/applications/dev.dblocal.DBLocal.desktop
install -D -m 0644 %{SOURCE2} %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/dblocal.svg
install -D -m 0644 %{SOURCE10} %{buildroot}%{_datadir}/icons/hicolor/16x16/apps/dblocal.png
install -D -m 0644 %{SOURCE11} %{buildroot}%{_datadir}/icons/hicolor/24x24/apps/dblocal.png
install -D -m 0644 %{SOURCE12} %{buildroot}%{_datadir}/icons/hicolor/32x32/apps/dblocal.png
install -D -m 0644 %{SOURCE13} %{buildroot}%{_datadir}/icons/hicolor/48x48/apps/dblocal.png
install -D -m 0644 %{SOURCE14} %{buildroot}%{_datadir}/icons/hicolor/64x64/apps/dblocal.png
install -D -m 0644 %{SOURCE15} %{buildroot}%{_datadir}/icons/hicolor/128x128/apps/dblocal.png
install -D -m 0644 %{SOURCE16} %{buildroot}%{_datadir}/icons/hicolor/256x256/apps/dblocal.png
install -D -m 0644 %{SOURCE17} %{buildroot}%{_datadir}/icons/hicolor/512x512/apps/dblocal.png
install -D -m 0644 %{SOURCE3} %{buildroot}%{_defaultlicensedir}/%{name}/COPYING
if [ -f %{SOURCE4} ]; then
  install -D -m 0644 %{SOURCE4} %{buildroot}%{_defaultlicensedir}/%{name}/LICENSE-RING-AWSLC-NOTES.md
fi

%post
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q %{_datadir}/applications || :
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f %{_datadir}/icons/hicolor || :
fi

%postun
if command -v update-desktop-database >/dev/null 2>&1; then
  update-desktop-database -q %{_datadir}/applications || :
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
  gtk-update-icon-cache -q -t -f %{_datadir}/icons/hicolor || :
fi
if [ "$1" -eq 0 ]; then
  cat >&2 <<'MSG'
dblocal: Package files removed. User data in ~/.dblocal was kept intentionally.
MSG
fi

%files
%license %{_defaultlicensedir}/%{name}/COPYING
%license %{_defaultlicensedir}/%{name}/LICENSE-RING-AWSLC-NOTES.md
%{_bindir}/dblocal
%{_datadir}/applications/dev.dblocal.DBLocal.desktop
%{_datadir}/icons/hicolor/scalable/apps/dblocal.svg
%{_datadir}/icons/hicolor/16x16/apps/dblocal.png
%{_datadir}/icons/hicolor/24x24/apps/dblocal.png
%{_datadir}/icons/hicolor/32x32/apps/dblocal.png
%{_datadir}/icons/hicolor/48x48/apps/dblocal.png
%{_datadir}/icons/hicolor/64x64/apps/dblocal.png
%{_datadir}/icons/hicolor/128x128/apps/dblocal.png
%{_datadir}/icons/hicolor/256x256/apps/dblocal.png
%{_datadir}/icons/hicolor/512x512/apps/dblocal.png

%changelog
* %(date '+%a %b %d %Y') DBLocal Contributors <noreply@commandcode.ai> - %{_dblocal_version}-1
- Automated package build
