#!/usr/bin/env bash
# install-linux-deps.sh — install the build dependencies GPUI needs on Ubuntu.
#
# Source of truth: Zed's script/linux at commit
#   a19e19440d4102668830a9da579c3ee71d2fbf1a
#   https://github.com/zed-industries/zed/blob/a19e19440d4102668830a9da579c3ee71d2fbf1a/script/linux
# This script reproduces the Ubuntu/Debian `apt` branch of that file for Ubuntu
# 22.04 and 24.04 (base dependency array plus the matching libstdc++-N-dev).
#
# Scope: CI (including minimal Ubuntu containers running as root) and developer
# machines that already have passwordless sudo, as GitHub-hosted runners do. It
# does not implement Fedora/openSUSE/Arch/... — copy the relevant branch from
# Zed's script/linux if you need those.
#
# Note: DBLocal itself never asks for sudo at runtime; this is a *build* helper.
set -euo pipefail

if [ "$(uname -s)" != "Linux" ]; then
  printf 'install-linux-deps: not Linux, nothing to do\n' >&2
  exit 0
fi

if ! command -v apt-get >/dev/null 2>&1; then
  printf 'install-linux-deps: apt-get not found; this helper only covers Debian/Ubuntu.\n' >&2
  printf '  See Zed script/linux for other distributions.\n' >&2
  exit 1
fi

# Zed script/linux Ubuntu/Debian base dependency array.
deps=(
  gcc
  g++
  libasound2-dev
  libfontconfig-dev
  libgit2-dev
  libglib2.0-dev
  libssl-dev
  libva-dev
  libvulkan1
  libwayland-dev
  libx11-xcb-dev
  libxkbcommon-x11-dev
  libzstd-dev
  make
  cmake
  clang
  lld
  llvm
  jq
  git
  curl
  ca-certificates
  binutils
  file
  pkg-config
  gettext-base
  elfutils
  libsqlite3-dev
  musl-tools
  musl-dev
  build-essential
  pipewire
  xdg-desktop-portal
)

# Zed adds a libstdc++-N-dev depending on the distro. Match Ubuntu 22.04 vs 24.04+.
ubuntu_ver=""
if [ -r /etc/os-release ]; then
  # shellcheck disable=SC1091
  ubuntu_ver="$(. /etc/os-release && printf '%s' "${VERSION_ID:-}")"
fi
case "$ubuntu_ver" in
  22.*) deps+=( libstdc++-12-dev ) ;;
  *)    deps+=( libstdc++-14-dev ) ;;
esac

# `libvulkan1` (above) is only the loader: it enumerates ICD manifests and
# loads the driver behind them. Without a driver package there is no manifest
# in /usr/share/vulkan/icd.d and Vulkan reports zero adapters, so
# the GUI (`vulkan::preflight`) and every headless render test fail with
# "No usable headless GPU adapter found". GitHub's ubuntu-24.04 and
# ubuntu-24.04-arm images do not preinstall a Vulkan driver, so CI installs
# one explicitly. `mesa-vulkan-drivers` ships several ICDs; on a host with no
# GPU, mesa's lavapipe (`lvp_icd.*.json`, a CPU implementation) is the one that
# answers, which is exactly what the off-screen render tests need.
deps+=( mesa-vulkan-drivers )

apt_prefix=()
if [ "$(id -u)" -ne 0 ]; then
  apt_prefix=(sudo)
fi

export DEBIAN_FRONTEND=noninteractive
"${apt_prefix[@]}" apt-get update
"${apt_prefix[@]}" apt-get install -y --no-install-recommends "${deps[@]}"

printf 'install-linux-deps: done\n' >&2
