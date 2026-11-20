# Linux build dependencies

DBLocal's GUI uses `gpui-pre` (wgpu/Vulkan) and `gpui-component`. On Linux you need X11/Wayland client libraries and a working Vulkan ICD at runtime. This host is Fedora 44 (Wayland, GNOME 50.6); package names differ by distro.

## Fedora

```sh
sudo dnf install \
  wayland-devel libxkbcommon-devel libxkbcommon-x11-devel \
  libxcb-devel libX11-devel libXcursor-devel libXrandr-devel libXi-devel \
  mesa-libGL-devel mesa-vulkan-drivers vulkan-loader vulkan-tools \
  fontconfig-devel freetype-devel
```

Notes:
- `libxkbcommon-x11-devel` provides the unversioned `libxkbcommon-x11.so` symlink. The runtime `libxkbcommon-x11.so.0` is not enough to link the X11 backend (`cargo build -p dblocal-gui --features x11` fails with `cannot find -lxkbcommon-x11` without the `-devel` package). W0-C observed this on this host.
- `alsa-lib-devel` is *not* required by DBLocal's current dependency set (no audio crate). Do not add it unless a future dependency needs it.
- `libsecret-devel` is intentionally not required: the current keyring backend must build without libsecret-1 (see AGENTS.md environment notes).

## Debian / Ubuntu

```sh
sudo apt install \
  libwayland-dev libxkbcommon-dev libxkbcommon-x11-dev \
  libxcb1-dev libx11-dev libxcursor-dev libxrandr-dev libxi-dev \
  libgl1-mesa-dev mesa-vulkan-drivers libvulkan1 vulkan-tools \
  libfontconfig1-dev libfreetype-dev
```

## Runtime: Vulkan

The GUI refuses to start without a usable Vulkan GPU driver/ICD. On a misconfigured system it exits with code `78` and prints install/verification instructions (it never panics and never shows a black window).

Verify:
```sh
vulkaninfo --summary
ls /usr/share/vulkan/icd.d/
```

If `VK_ICD_FILENAMES` or `VK_DRIVER_FILES` is set to an invalid path, the pre-flight treats that as a missing/broken Vulkan stack.
