#!/usr/bin/env bash
# install-linux-deps.sh — install the build dependencies GPUI needs on Ubuntu.
#
# Source of truth: Zed's script/linux at commit
#   a19e19440d4102668830a9da579c3ee71d2fbf1a
#   https://github.com/zed-industries/zed/blob/a19e19440d4102668830a9da579c3ee71d2fbf1a/script/linux
# This script reproduces the Ubuntu/Debian `apt` branch of that file for Ubuntu
# 24.04 (base dependency array plus libstdc++-14-dev, which Zed's script adds
# when PRETTY_NAME matches `.+24\.(04|10)`).
#
# Scope: CI and developer machines that already have passwordless sudo, as
# GitHub-hosted runners do. It does not implement Fedora/openSUSE/Arch/... — copy
# the relevant branch from Zed's script/linux if you need those.
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
  gettext-base
  elfutils
  libsqlite3-dev
  musl-tools
  musl-dev
  build-essential
  pipewire
  xdg-desktop-portal
)

# Zed adds a libstdc++-N-dev depending on the distro. This helper targets the
# Ubuntu 24.04 runner used by CI.
deps+=( libstdc++-14-dev )

sudo apt-get update
sudo apt-get install -y --no-install-recommends "${deps[@]}"

printf 'install-linux-deps: done\n' >&2
