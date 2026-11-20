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

http_get() {
  # http_get <url> <output-file>
  url="$1"; out="$2"
  case "$DOWNLOADER" in
    curl) curl -fsSL --proto '=https' --tlsv1.2 -o "$out" "$url" 2>/dev/null || curl -fsSL -o "$out" "$url" ;;
    wget) wget -qO "$out" "$url" ;;
    *) die "internal: no downloader selected" ;;
  esac
}

http_get_stdout() {
  url="$1"
  case "$DOWNLOADER" in
    curl) curl -fsSL --proto '=https' --tlsv1.2 "$url" 2>/dev/null || curl -fsSL "$url" ;;
    wget) wget -qO - "$url" ;;
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
  if [ -w "$(dirname "$dest")" ] || [ -w "$dest" ] 2>/dev/null; then
    # Overwrite in place (idempotent upgrade). Use cp+chmod for portability.
    cp "$src" "$dest"
    chmod 0755 "$dest"
    return
  fi
  if [ "$USE_SUDO" -eq 1 ] && have_cmd sudo; then
    info "installing to $dest with sudo (--sudo requested)"
    sudo cp "$src" "$dest"
    sudo chmod 0755 "$dest"
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

# --- release resolution -------------------------------------------------------

resolve_release_base() {
  if [ -n "${DBLOCAL_RELEASE_BASE_URL:-}" ]; then
    # Trim trailing slash for consistent joining.
    printf '%s\n' "${DBLOCAL_RELEASE_BASE_URL%/}"
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
  fi

  printf 'https://github.com/%s/releases/download/%s\n' "$REPO" "$tag"
}

expected_checksum() {
  sums_file="$1"; asset_name="$2"
  # checksums.txt format: "<sha256>  <filename>" (sha256sum/shasum output).
  line="$(grep -E "[[:space:]]${asset_name}$|[[:space:]]\*${asset_name}$" "$sums_file" | head -n 1 || true)"
  [ -n "$line" ] || die "asset '$asset_name' not found in checksums.txt"
  printf '%s\n' "$line" | awk '{print $1}'
}

# --- main ---------------------------------------------------------------------

main() {
  detect_target
  pick_downloader
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
  # shellcheck disable=SC2064
  trap "rm -rf '$tmpdir'" EXIT INT TERM

  sums="$tmpdir/checksums.txt"
  archive="$tmpdir/$asset"

  info "downloading checksums: $base/checksums.txt"
  http_get "$base/checksums.txt" "$sums" || die "failed to download checksums.txt from $base"

  expected="$(expected_checksum "$sums" "$asset")"
  [ -n "$expected" ] || die "could not read expected checksum for $asset"

  info "downloading $asset"
  http_get "$base/$asset" "$archive" || die "failed to download $asset from $base"

  info "verifying SHA-256"
  verify_sha256 "$archive" "$expected"

  info "extracting"
  mkdir -p "$tmpdir/extract"
  tar -xzf "$archive" -C "$tmpdir/extract"

  bin_src="$tmpdir/extract/dblocal"
  [ -f "$bin_src" ] || die "archive did not contain expected 'dblocal' binary"

  info "installing to $dest_bin"
  install_file "$bin_src" "$dest_bin"

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
