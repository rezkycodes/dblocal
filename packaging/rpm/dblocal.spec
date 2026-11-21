Name:           dblocal
Version:        %{_dblocal_version}
Release:        1%{?dist}
Summary:        Local database server manager

License:        GPL-3.0-only
URL:            https://github.com/rezkycodes/dblocal
Source0:        dblocal
Source1:        dblocal.desktop
Source2:        dblocal.svg
Source3:        COPYING
Source4:        LICENSE-RING-AWSLC-NOTES.md

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
# libvulkan is recommended (not required): GPUI needs a host Vulkan driver;
# missing Vulkan is reported with an actionable message (F-SPK-01 AC2).
Requires:       glibc
Requires:       libgcc
Requires:       libxcb
Requires:       libxkbcommon
Requires:       libxkbcommon-x11
Requires:       zlib
Recommends:     vulkan-loader

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
install -D -m 0644 %{SOURCE1} %{buildroot}%{_datadir}/applications/dblocal.desktop
install -D -m 0644 %{SOURCE2} %{buildroot}%{_datadir}/icons/hicolor/scalable/apps/dblocal.svg
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
dblocal: Berkas paket dihapus. Data pengguna di ~/.dblocal sengaja dipertahankan.
MSG
fi

%files
%license %{_defaultlicensedir}/%{name}/COPYING
%license %{_defaultlicensedir}/%{name}/LICENSE-RING-AWSLC-NOTES.md
%{_bindir}/dblocal
%{_datadir}/applications/dblocal.desktop
%{_datadir}/icons/hicolor/scalable/apps/dblocal.svg

%changelog
* %(date '+%a %b %d %Y') DBLocal Contributors <noreply@commandcode.ai> - %{_dblocal_version}-1
- Automated package build
