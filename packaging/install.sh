#!/bin/sh
# DBLocal installer (F-PKG-03).
# Usage:
#   curl -fsSL https://raw.githubusercontent.com/rezkycodes/dblocal/main/packaging/install.sh | sh
#   sh install.sh [--version <tag>] [--dry-run] [--prefix <dir>] [--sudo]
#
# POSIX sh. Idempotent (re-run to upgrade in place). Never calls sudo unless
# --sudo is passed. On unsupported OS/arch it prints a clear message and exits
# without installing anything.
#
# Environment overrides (also used by offline tests):
#   DBLOCAL_REPO                 owner/name for GitHub Releases (default: rezkycodes/dblocal)
#   DBLOCAL_RELEASE_BASE_URL     base URL that contains release assets and checksums.txt
#                                (skips GitHub API resolution; for tests/local mirrors)
#   DBLOCAL_TEST_UNAME_S         override `uname -s` result
#   DBLOCAL_TEST_UNAME_M         override `uname -m` result
#   DBLOCAL_TEST_SHA256_CMD      force checksum tool: "sha256sum" or "shasum"

set -eu

# --- helpers -----------------------------------------------------------------

log() { printf '%s\n' "$*" >&2; }
info() { log "dblocal: $*"; }
warn() { log "dblocal: warning: $*"; }
err()  { log "dblocal: error: $*"; }
die()  { err "$*"; exit 1; }

# Temporary directory for downloads/extraction; cleaned up by the EXIT trap.
tmpdir=""

cleanup() {
  if [ -n "$tmpdir" ] && [ -d "$tmpdir" ]; then
    rm -rf "$tmpdir"
  fi
}

# In POSIX sh a handler for INT/TERM that merely returns lets the script
# continue. Exit explicitly with the conventional signal codes instead.
on_int()  { cleanup; trap - EXIT; exit 130; }
on_term() { cleanup; trap - EXIT; exit 143; }
on_hup()  { cleanup; trap - EXIT; exit 129; }

usage() {
  cat >&2 <<'USAGE'
Usage: install.sh [options]

Options:
  --version <tag>   Install a specific release tag (e.g. v0.1.0). Default: latest.
  --dry-run         Resolve what would be installed and where, but download and
                    install nothing.
  --prefix <dir>    Install the `dblocal` binary into <dir> instead of the
                    default location.
  --sudo            Allow running privileged install steps via sudo when
                    installing into a directory you cannot write to. Never used
                    unless this flag is passed.
  -h, --help        Show this help.

Environment:
  DBLOCAL_REPO               GitHub repo (default: rezkycodes/dblocal)
  DBLOCAL_RELEASE_BASE_URL   Override the release asset base URL (testing)
USAGE
}

have_cmd() { command -v "$1" >/dev/null 2>&1; }

uname_s() {
  if [ -n "${DBLOCAL_TEST_UNAME_S:-}" ]; then printf '%s\n' "$DBLOCAL_TEST_UNAME_S"; else uname -s; fi
}
uname_m() {
  if [ -n "${DBLOCAL_TEST_UNAME_M:-}" ]; then printf '%s\n' "$DBLOCAL_TEST_UNAME_M"; else uname -m; fi
}

# --- argument parsing ---------------------------------------------------------

VERSION=""
DRY_RUN=0
PREFIX=""
USE_SUDO=0

validate_tag() {
  # Accept semver-like release tags: v1.2.3, 1.2.3, and prerelease/build forms
  # such as v1.2.3-rc.1 or v1.2.3+meta.1. Reject anything else (including
  # shell metacharacters and regex-surprising inputs) because the tag flows
  # into URLs and into filesystem lookups.
  tag="$1"
  case "$tag" in
    ''|*[!0-9A-Za-z.+-]*) return 1 ;;
  esac
  printf '%s\n' "$tag" | grep -Eq '^v?[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.-]+)?$'
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    --version)
      [ "$#" -ge 2 ] || die "--version requires a tag argument"
      VERSION="$2"; shift 2 ;;
    --version=*)
      VERSION="${1#--version=}"; shift ;;
    --dry-run)
      DRY_RUN=1; shift ;;
    --prefix)
      [ "$#" -ge 2 ] || die "--prefix requires a directory argument"
      PREFIX="$2"; shift 2 ;;
    --prefix=*)
      PREFIX="${1#--prefix=}"; shift ;;
    --sudo)
      USE_SUDO=1; shift ;;
    -h|--help)
      usage; exit 0 ;;
    --)
      shift; break ;;
    -*)
      die "unknown option: $1 (try --help)" ;;
    *)
      die "unexpected argument: $1 (try --help)" ;;
  esac
done
[ "$#" -eq 0 ] || die "unexpected argument(s): $* (try --help)"

if [ -n "$VERSION" ] && ! validate_tag "$VERSION"; then
  die "invalid --version tag '$VERSION' (expected e.g. v1.2.3 or v1.2.3-rc.1)"
fi

REPO="${DBLOCAL_REPO:-rezkycodes/dblocal}"

# --- platform detection -------------------------------------------------------

detect_target() {
  os_raw="$(uname_s)"
  arch_raw="$(uname_m)"

  case "$os_raw" in
    Linux)  os="linux" ;;
    Darwin) os="macos" ;;
    *)
      die "unsupported operating system '$os_raw'. DBLocal install.sh supports Linux and macOS only."
      ;;
  esac

  case "$arch_raw" in
    x86_64|amd64)   arch="x86_64" ;;
    aarch64|arm64)  arch="aarch64" ;;
    *)
      die "unsupported architecture '$arch_raw' on $os_raw. Supported: x86_64, aarch64/arm64."
      ;;
  esac

  # Release artifact naming (F-PKG-01/02): linux per-arch, macOS universal.
  if [ "$os" = "macos" ]; then
    TARGET="macos-universal"
  else
    TARGET="linux-$arch"
  fi
}

# --- download / checksum tooling ----------------------------------------------

pick_downloader() {
  if have_cmd curl; then
    DOWNLOADER="curl"
  elif have_cmd wget; then
    DOWNLOADER="wget"
  else
    die "neither 'curl' nor 'wget' is available; cannot download releases"
  fi
}

require_https_url() {
  # require_https_url <url> <what>
  # https is always fine. Plain http is tolerated ONLY for a loopback host, which
  # is what the offline tests and a local mirror use. Everything else is refused.
  url="$1"; what="$2"
  case "$url" in
    https://*)
      return 0
      ;;
    http://*)
      rest="${url#http://}"
      authority="${rest%%/*}"
      case "$authority" in
        127.0.0.1|127.0.0.1:*) return 0 ;;
        localhost|localhost:*) return 0 ;;
        \[::1\]|\[::1\]:*)     return 0 ;;
      esac
      die "$what uses plain http for a non-local host ($url); refusing. TLS is required — use an https URL."
      ;;
    *)
      die "$what must be an absolute https URL (got: $url)"
      ;;
  esac
}

# Strict TLS flags. The https-only pair is used for every real download; plain
# http is permitted only for a loopback host (offline tests / a local mirror),
# and even then only http/https are allowed as protocols. If the installed curl
# cannot understand the flags, we fail loudly; we never retry a weaker request.
CURL_TLS_FLAGS_HTTPS="--proto =https --proto-redir =https --tlsv1.2"
CURL_TLS_FLAGS_LOOPBACK="--proto =http,https --proto-redir =http,https --tlsv1.2"

is_loopback_url() {
  case "$1" in
    http://*)
      rest="${1#http://}"
      case "${rest%%/*}" in
        127.0.0.1|127.0.0.1:*|localhost|localhost:*|\[::1\]|\[::1\]:*) return 0 ;;
      esac
      ;;
  esac
  return 1
}

check_curl_supports_tls_flags() {
  [ "$DOWNLOADER" = "curl" ] || return 0
  # Test-only override: the offline suite injects a stub curl whose --help does
  # not advertise real flags. Never set this in normal use.
  if [ "${DBLOCAL_TEST_SKIP_CURL_CAPS:-0}" = "1" ]; then
    return 0
  fi
  # Verify the flags exist without performing a request. `curl --help all` lists
  # long options; older curl lacks --proto-redir. If we cannot confirm support we
  # stop — a possibly downgraded download is worse than no install.
  help_out="$(curl --help all 2>/dev/null || curl --help 2>/dev/null || true)"
  case "$help_out" in
    *--proto-redir*) return 0 ;;
  esac
  die "installed curl does not advertise '--proto-redir' (too old?). Refusing to download over a connection that could be downgraded. Upgrade curl, or use wget."
}

http_get() {
  # http_get <url> <output-file>
  url="$1"; out="$2"
  require_https_url "$url" "download URL"
  case "$DOWNLOADER" in
    curl)
      if is_loopback_url "$url"; then flags="$CURL_TLS_FLAGS_LOOPBACK"; else flags="$CURL_TLS_FLAGS_HTTPS"; fi
      # shellcheck disable=SC2086 # word-splitting the flag list is intended
      curl -fsSL $flags -o "$out" "$url" || die "download failed (strict TLS): $url"
      ;;
    wget)
      # --https-only would reject the loopback http test/mirror URLs; require_https_url
      # has already confined plain http to loopback, so restrict protocols instead.
      if is_loopback_url "$url"; then
        wget -qO "$out" "$url" || die "download failed: $url"
      else
        wget --https-only -qO "$out" "$url" || die "download failed (strict TLS): $url"
      fi
      ;;
    *) die "internal: no downloader selected" ;;
  esac
}

http_get_stdout() {
  url="$1"
  require_https_url "$url" "download URL"
  case "$DOWNLOADER" in
    curl)
      if is_loopback_url "$url"; then flags="$CURL_TLS_FLAGS_LOOPBACK"; else flags="$CURL_TLS_FLAGS_HTTPS"; fi
      # shellcheck disable=SC2086 # word-splitting the flag list is intended
      curl -fsSL $flags "$url" || die "download failed (strict TLS): $url"
      ;;
    wget)
      if is_loopback_url "$url"; then
        wget -qO - "$url" || die "download failed: $url"
      else
        wget --https-only -qO - "$url" || die "download failed (strict TLS): $url"
      fi
      ;;
    *) die "internal: no downloader selected" ;;
  esac
}

pick_sha256() {
  forced="${DBLOCAL_TEST_SHA256_CMD:-}"
  if [ -n "$forced" ]; then
    case "$forced" in
      sha256sum|shasum) SHA256_TOOL="$forced" ;;
      *) die "DBLOCAL_TEST_SHA256_CMD must be 'sha256sum' or 'shasum'" ;;
    esac
    have_cmd "$SHA256_TOOL" || die "checksum tool '$SHA256_TOOL' not found"
    return
  fi
  if have_cmd sha256sum; then
    SHA256_TOOL="sha256sum"
  elif have_cmd shasum; then
    SHA256_TOOL="shasum"
  else
    die "no SHA-256 tool found (need 'sha256sum' or 'shasum -a 256')"
  fi
}

verify_sha256() {
  # verify_sha256 <file> <expected-hex>
  file="$1"; expected="$2"
  case "$SHA256_TOOL" in
    sha256sum) actual="$(sha256sum "$file" | awk '{print $1}')" ;;
    shasum)    actual="$(shasum -a 256 "$file" | awk '{print $1}')" ;;
    *) die "internal: no sha256 tool selected" ;;
  esac
  if [ "$actual" != "$expected" ]; then
    die "checksum mismatch for $(basename "$file"): expected $expected, got $actual"
  fi
}

# --- install destination ------------------------------------------------------

choose_prefix() {
  if [ -n "$PREFIX" ]; then
    printf '%s\n' "$PREFIX"
    return
  fi
  # Preference order (F-PKG-03 AC1): ~/.local/bin first, then /usr/local/bin
  # only when it is writable by this user. No privilege escalation here.
  if [ -n "${HOME:-}" ]; then
    home_bin="$HOME/.local/bin"
    if [ -d "$home_bin" ] || mkdir -p "$home_bin" 2>/dev/null; then
      printf '%s\n' "$home_bin"
      return
    fi
    warn "cannot create $home_bin; falling back"
  fi
  if [ -d "/usr/local/bin" ] && [ -w "/usr/local/bin" ]; then
    printf '%s\n' "/usr/local/bin"
    return
  fi
  die "no writable install directory found (set HOME, pass --prefix <dir>, or ensure /usr/local/bin is writable)"
}

ensure_dir() {
  dir="$1"
  if [ -d "$dir" ]; then
    return
  fi
  if mkdir -p "$dir" 2>/dev/null; then
    return
  fi
  if [ "$USE_SUDO" -eq 1 ] && have_cmd sudo; then
    info "creating $dir with sudo (--sudo requested)"
    sudo mkdir -p "$dir"
    return
  fi
  die "cannot create install directory '$dir' (pass --prefix <dir> or re-run with --sudo)"
}

install_file() {
  src="$1"; dest="$2"
  dest_dir="$(dirname "$dest")"

  # Install atomically and safely when the destination is a running executable:
  # copying over an in-use ELF fails with ETXTBSY ("Text file busy") on Linux.
  # Instead, write to a temporary sibling in the destination directory, chmod it,
  # then rename(2) over the destination (atomic on the same filesystem; also
  # never leaves a half-written binary in place).
  if [ -w "$dest_dir" ] || [ -w "$dest" ] 2>/dev/null; then
    tmp_dest="${dest}.new.$$"
    if ! cp "$src" "$tmp_dest" 2>/dev/null; then
      rm -f "$tmp_dest" 2>/dev/null || true
      die "cannot write temporary install file '$tmp_dest'"
    fi
    chmod 0755 "$tmp_dest" || { rm -f "$tmp_dest"; die "cannot chmod '$tmp_dest'"; }
    if ! mv -f "$tmp_dest" "$dest" 2>/dev/null; then
      rm -f "$tmp_dest" 2>/dev/null || true
      die "cannot place binary at '$dest'"
    fi
    return
  fi

  if [ "$USE_SUDO" -eq 1 ] && have_cmd sudo; then
    info "installing to $dest with sudo (--sudo requested)"
    tmp_dest="${dest}.new.$$"
    sudo cp "$src" "$tmp_dest" || { sudo rm -f "$tmp_dest" 2>/dev/null || true; die "cannot write temporary install file '$tmp_dest' (sudo)"; }
    sudo chmod 0755 "$tmp_dest" || { sudo rm -f "$tmp_dest"; die "cannot chmod '$tmp_dest' (sudo)"; }
    sudo mv -f "$tmp_dest" "$dest" || { sudo rm -f "$tmp_dest" 2>/dev/null || true; die "cannot place binary at '$dest' (sudo)"; }
    return
  fi

  die "cannot write to '$dest' (pass --prefix <dir> or re-run with --sudo)"
}

path_warn() {
  bindir="$1"
  case ":${PATH:-}:" in
    *:"$bindir":*) return 0 ;;
  esac
  warn "install directory '$bindir' is not on PATH."
  warn "Add it for this shell, e.g.:"
  warn "  export PATH=\"$bindir:\$PATH\""
}

install_desktop_and_icons() {
  # Install user-local desktop entry and hicolor icons on Linux when the
  # tarball ships a share/ tree. Never uses sudo; skips quietly for older
  # tarballs that contain only the binary.
  archive_path="$1"
  extract_dir="$2"
  installed_bin="$3"

  [ "$os" = "linux" ] || return 0
  if [ -n "${XDG_DATA_HOME:-}" ]; then
    data_home="$XDG_DATA_HOME"
  elif [ -n "${HOME:-}" ]; then
    data_home="$HOME/.local/share"
  else
    return 0
  fi

  desktop_member="share/applications/dev.dblocal.DBLocal.desktop"
  members="$(tar -tzf "$archive_path" 2>/dev/null || true)"
  if ! printf '%s\n' "$members" | grep -Fx "$desktop_member" >/dev/null 2>&1; then
    return 0
  fi

  # Extract only expected desktop and icon members; reject symlinks.
  tar -xzf "$archive_path" -C "$extract_dir" -- "$desktop_member" 2>/dev/null || return 0
  desktop_src="$extract_dir/$desktop_member"
  if [ -L "$desktop_src" ] || [ ! -f "$desktop_src" ]; then
    warn "skipping desktop entry (not a regular file)"
    return 0
  fi

  # Resolve absolute path of installed binary for Exec= / TryExec=.
  abs_dir="$(CDPATH='' cd -- "$(dirname "$installed_bin")" && pwd -P)"
  abs_bin="$abs_dir/$(basename "$installed_bin")"

  case "$abs_bin" in
    *[!A-Za-z0-9._/+@~-]*)
      warn "skipping desktop entry and icon install: install path '$abs_bin' contains characters outside [A-Za-z0-9._/+@~-]"
      return 0
      ;;
  esac

  apps_dir="$data_home/applications"
  mkdir -p "$apps_dir"
  desktop_dest="$apps_dir/dev.dblocal.DBLocal.desktop"
  desktop_tmp="${desktop_dest}.new.$$"
  DBLOCAL_EXEC_BIN="$abs_bin" awk '
    /^Exec=/    { print "Exec=" ENVIRON["DBLOCAL_EXEC_BIN"]; next }
    /^TryExec=/ { print "TryExec=" ENVIRON["DBLOCAL_EXEC_BIN"]; next }
    { print }
  ' "$desktop_src" > "$desktop_tmp"
  chmod 0644 "$desktop_tmp"
  mv -f "$desktop_tmp" "$desktop_dest"

  svg_member="share/icons/hicolor/scalable/apps/dblocal.svg"
  if printf '%s\n' "$members" | grep -Fx "$svg_member" >/dev/null 2>&1; then
    if tar -xzf "$archive_path" -C "$extract_dir" -- "$svg_member" 2>/dev/null; then
      svg_src="$extract_dir/$svg_member"
      if [ ! -L "$svg_src" ] && [ -f "$svg_src" ]; then
        svg_dir="$data_home/icons/hicolor/scalable/apps"
        mkdir -p "$svg_dir"
        cp "$svg_src" "$svg_dir/dblocal.svg"
        chmod 0644 "$svg_dir/dblocal.svg"
      fi
    fi
  fi

  for icon_size in 16 24 32 48 64 128 256 512; do
    png_member="share/icons/hicolor/${icon_size}x${icon_size}/apps/dblocal.png"
    if printf '%s\n' "$members" | grep -Fx "$png_member" >/dev/null 2>&1; then
      if tar -xzf "$archive_path" -C "$extract_dir" -- "$png_member" 2>/dev/null; then
        png_src="$extract_dir/$png_member"
        if [ ! -L "$png_src" ] && [ -f "$png_src" ]; then
          png_dir="$data_home/icons/hicolor/${icon_size}x${icon_size}/apps"
          mkdir -p "$png_dir"
          cp "$png_src" "$png_dir/dblocal.png"
          chmod 0644 "$png_dir/dblocal.png"
        fi
      fi
    fi
  done

  touch "$data_home/icons/hicolor" 2>/dev/null || true
  if [ -f "$data_home/icons/hicolor/icon-theme.cache" ] && have_cmd gtk-update-icon-cache; then
    gtk-update-icon-cache -q -t -f "$data_home/icons/hicolor" 2>/dev/null || true
  fi
  if have_cmd update-desktop-database; then
    update-desktop-database -q "$apps_dir" 2>/dev/null || true
  fi
  info "installed desktop entry and icons under $data_home"
}

# --- release resolution -------------------------------------------------------

resolve_release_base() {
  if [ -n "${DBLOCAL_RELEASE_BASE_URL:-}" ]; then
    # Trim trailing slash for consistent joining.
    base="${DBLOCAL_RELEASE_BASE_URL%/}"
    require_https_url "$base" "DBLOCAL_RELEASE_BASE_URL"
    printf '%s\n' "$base"
    return
  fi

  if [ -n "$VERSION" ]; then
    tag="$VERSION"
  else
    # Resolve latest tag via the GitHub Releases API. If the API is
    # unreachable, fail with a clear message (no silent downgrade).
    api="https://api.github.com/repos/${REPO}/releases/latest"
    info "resolving latest release from $api"
    json="$(http_get_stdout "$api")" || die "failed to query latest release from GitHub API"
    # Minimal JSON parsing without jq: grab the first tag_name value.
    tag="$(printf '%s' "$json" | tr -d '\n' | sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p')"
    [ -n "$tag" ] || die "could not determine latest release tag from GitHub API response"
    validate_tag "$tag" || die "latest release tag from API is not a valid version tag: '$tag'"
  fi

  printf 'https://github.com/%s/releases/download/%s\n' "$REPO" "$tag"
}

expected_checksum() {
  sums_file="$1"; asset_name="$2"
  # checksums.txt format: "<sha256>  <filename>" (sha256sum/shasum output).
  # Match the asset name literally (never as a regex) to avoid '.' acting as a
  # wildcard or metacharacters in the name affecting the match.
  hash="$(awk -v n="$asset_name" '
    $2 == n || $2 == "*" n { print $1; found=1; exit }
    END { if (!found) exit 1 }
  ' "$sums_file")" || die "asset '$asset_name' not found in checksums.txt"
  [ -n "$hash" ] || die "could not read expected checksum for '$asset_name' from checksums.txt"
  printf '%s\n' "$hash"
}

# --- main ---------------------------------------------------------------------

main() {
  detect_target
  pick_downloader
  check_curl_supports_tls_flags
  pick_sha256

  base="$(resolve_release_base)"

  # If the version came from API resolution, infer the tag from the base URL.
  if [ -n "$VERSION" ]; then
    version_tag="$VERSION"
  else
    version_tag="${base##*/}"
  fi
  asset="dblocal-${version_tag}-${TARGET}.tar.gz"

  dest_prefix="$(choose_prefix)"
  dest_bin="$dest_prefix/dblocal"

  info "repository:   $REPO"
  info "platform:     $(uname_s)/$(uname_m) -> $TARGET"
  info "version:      $version_tag"
  info "asset:        $asset"
  info "install dir:  $dest_prefix"

  if [ "$DRY_RUN" -eq 1 ]; then
    info "dry-run: would download $base/$asset"
    info "dry-run: would verify SHA-256 against $base/checksums.txt"
    info "dry-run: would install to $dest_bin"
    info "dry-run: no files were downloaded or modified"
    return 0
  fi

  ensure_dir "$dest_prefix"

  tmpdir="$(mktemp -d 2>/dev/null || mktemp -d -t dblocal-install)"
  # Clean up temp files on exit, and exit cleanly on signals (INT->130, TERM->143).
  trap cleanup EXIT
  trap on_int  INT
  trap on_term TERM
  trap on_hup  HUP

  sums="$tmpdir/checksums.txt"
  archive="$tmpdir/$asset"

  info "downloading checksums: $base/checksums.txt"
  http_get "$base/checksums.txt" "$sums"

  expected="$(expected_checksum "$sums" "$asset")"

  info "downloading $asset"
  http_get "$base/$asset" "$archive"

  info "verifying SHA-256"
  verify_sha256 "$archive" "$expected"

  info "extracting"
  mkdir -p "$tmpdir/extract"
  # Extract only the expected member. Even though the checksum covers the
  # archive, extraction of an untrusted archive must be defensive against
  # unexpected members (traversal, extra files) and the binary itself must not
  # be a symlink.
  tar -xzf "$archive" -C "$tmpdir/extract" -- dblocal \
    || die "failed to extract 'dblocal' from archive (unexpected layout or unsafe archive)"

  bin_src="$tmpdir/extract/dblocal"
  if [ -L "$bin_src" ]; then
    die "extracted 'dblocal' is a symlink; refusing to install"
  fi
  [ -f "$bin_src" ] || die "archive did not contain expected 'dblocal' binary"

  info "installing to $dest_bin"
  install_file "$bin_src" "$dest_bin"
  install_desktop_and_icons "$archive" "$tmpdir/extract" "$dest_bin"

  info "installed dblocal ($version_tag) to $dest_bin"
  path_warn "$dest_prefix"

  log ""
  log "Next steps:"
  log "  1. Verify:   $dest_bin --help"
  log "  2. Run:      dblocal            (opens the GUI, once the app is released)"
  log "     or:       dblocal --help     (CLI; subcommands arrive in Phase 2)"
  log "  3. Docs:     https://github.com/${REPO}#readme"
  log ""
  log "DBLocal stores all data under ~/.dblocal and never requires sudo at runtime."
}

main "$@"
