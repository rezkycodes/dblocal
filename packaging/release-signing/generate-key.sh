#!/usr/bin/env bash
# Generate an Ed25519 release-signing key pair for DBLocal, save a
# passphrase-encrypted PKCS#8 (AES-256) offline backup, and upload the
# private key to GitHub Actions secret RELEASE_SIGNING_KEY over stdin.
#
# Usage:
#   bash packaging/release-signing/generate-key.sh --backup /path/outside/repo/dblocal-signing.enc.pem [--repo owner/name]
#   bash packaging/release-signing/generate-key.sh --no-backup [--repo owner/name]
#
# Safety properties:
# - Never prints the private key to stdout or stderr.
# - Stores temporary key material in a 0700 temp directory with 0600 files.
# - Removes the temporary directory on exit, error, or signal (EXIT/INT/TERM/HUP).
# - Encrypted backup uses `openssl pkcs8 -topk8 -v2 aes-256-cbc -iter 600000`
#   (a high PBKDF2 iteration count, so a stolen file resists offline guessing)
#   with an interactive passphrase prompt (never an argument or environment
#   variable), mode 0600, refusing to overwrite existing files and refusing
#   paths inside any git work tree.
# - Prints ONLY the public key (64-char hex) and the single file to paste it in.
set -euo pipefail

REPO="rezkycodes/dblocal"
BACKUP_PATH=""
NO_BACKUP=0

while [ "$#" -gt 0 ]; do
  case "$1" in
    --repo)
      [ "$#" -ge 2 ] || { echo "error: --repo requires owner/name" >&2; exit 2; }
      REPO="$2"
      shift 2
      ;;
    --repo=*)
      REPO="${1#--repo=}"
      shift
      ;;
    --backup)
      [ "$#" -ge 2 ] || { echo "error: --backup requires a destination path outside the git repo" >&2; exit 2; }
      BACKUP_PATH="$2"
      shift 2
      ;;
    --backup=*)
      BACKUP_PATH="${1#--backup=}"
      shift
      ;;
    --no-backup)
      NO_BACKUP=1
      shift
      ;;
    -h|--help)
      sed -n '2,18p' "$0"
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

if [ -n "$BACKUP_PATH" ] && [ "$NO_BACKUP" -eq 1 ]; then
  echo "error: --backup and --no-backup are mutually exclusive" >&2
  exit 2
fi

if [ -z "$BACKUP_PATH" ] && [ "$NO_BACKUP" -eq 0 ]; then
  cat >&2 <<'EOF'
error: an encrypted offline backup is required by default because GitHub
Actions secrets cannot be read back (if lost, installed apps cannot verify
a key rotation release).

Pass an encrypted backup path outside any git repository:
  bash packaging/release-signing/generate-key.sh --backup /path/outside/repo/dblocal-signing.enc.pem

Or explicitly opt out (not recommended for production):
  bash packaging/release-signing/generate-key.sh --no-backup
EOF
  exit 2
fi

command -v openssl >/dev/null 2>&1 || { echo "error: openssl is required" >&2; exit 1; }
command -v gh >/dev/null 2>&1 || { echo "error: GitHub CLI (gh) is required" >&2; exit 1; }

if [ -n "$BACKUP_PATH" ]; then
  if [ -e "$BACKUP_PATH" ] || [ -L "$BACKUP_PATH" ]; then
    echo "error: backup path already exists (refusing to overwrite): $BACKUP_PATH" >&2
    exit 1
  fi
  backup_parent="$(dirname "$BACKUP_PATH")"
  if [ ! -d "$backup_parent" ]; then
    echo "error: backup parent directory does not exist: $backup_parent" >&2
    exit 1
  fi
  abs_parent="$(cd "$backup_parent" && pwd -P)"
  if command -v git >/dev/null 2>&1; then
    if git -C "$abs_parent" rev-parse --is-inside-work-tree >/dev/null 2>&1 \
      || git -C "$abs_parent" rev-parse --git-dir >/dev/null 2>&1; then
      echo "error: refusing to write private key backup inside a git repository: $BACKUP_PATH" >&2
      exit 1
    fi
  fi
  BACKUP_PATH="${abs_parent}/$(basename "$BACKUP_PATH")"
fi

tmpdir=""
cleanup() {
  if [ -n "$tmpdir" ] && [ -d "$tmpdir" ]; then
    rm -rf "$tmpdir"
  fi
}
trap cleanup EXIT INT TERM HUP

umask 077
tmpdir="$(mktemp -d)"
chmod 0700 "$tmpdir"

priv_pem="$tmpdir/release-signing.key.pem"
pub_pem="$tmpdir/release-signing.pub.pem"
pub_der="$tmpdir/release-signing.pub.der"

# Generate Ed25519 private key and derive public key.
openssl genpkey -algorithm ED25519 -out "$priv_pem" 2>/dev/null
chmod 0600 "$priv_pem"
openssl pkey -in "$priv_pem" -pubout -out "$pub_pem" 2>/dev/null
openssl pkey -in "$priv_pem" -pubout -outform DER -out "$pub_der" 2>/dev/null

# Extract the raw 32-byte Ed25519 public key (last 32 bytes of SubjectPublicKeyInfo DER).
pub_hex="$(tail -c 32 "$pub_der" | od -An -tx1 | tr -d ' \n')"
if [ "${#pub_hex}" -ne 64 ]; then
  echo "error: failed to extract 64-char hex Ed25519 public key" >&2
  exit 1
fi

# Sanity self-test with the exact OpenSSL command used by release.yml.
printf 'self-test\n' > "$tmpdir/test.txt"
openssl pkeyutl -sign -inkey "$priv_pem" -rawin -in "$tmpdir/test.txt" -out "$tmpdir/test.txt.sig"
openssl pkeyutl -verify -pubin -inkey "$pub_pem" -rawin -in "$tmpdir/test.txt" -sigfile "$tmpdir/test.txt.sig" >/dev/null

# Save passphrase-encrypted PKCS#8 offline backup before uploading or deleting temp key.
if [ -n "$BACKUP_PATH" ]; then
  echo "Creating passphrase-encrypted PKCS#8 (AES-256) backup at ${BACKUP_PATH}..."
  echo "OpenSSL will prompt you for an encryption passphrase."
  # PBKDF2 iterations: OpenSSL's default (2048) is weak against offline guessing
  # of a stolen backup, so raise it to the modern guidance of 600k.
  openssl pkcs8 -topk8 -v2 aes-256-cbc -iter 600000 -in "$priv_pem" -out "$BACKUP_PATH"
  chmod 0600 "$BACKUP_PATH"

  # Verify the encrypted backup decrypts to the exact same public key without
  # ever writing or printing the plaintext private key a second time.
  echo "Re-enter the passphrase once to verify the encrypted backup decrypts cleanly:"
  backup_pub_pem="$tmpdir/backup.pub.pem"
  if ! openssl pkey -in "$BACKUP_PATH" -pubout -out "$backup_pub_pem" \
    || ! cmp -s "$pub_pem" "$backup_pub_pem"; then
    rm -f "$BACKUP_PATH"
    echo "error: encrypted backup verification failed; removed incomplete backup" >&2
    exit 1
  fi
fi

echo "Uploading private key to GitHub Actions secret RELEASE_SIGNING_KEY on ${REPO}..."
gh secret set RELEASE_SIGNING_KEY --repo "$REPO" < "$priv_pem"

cat <<EOF

Done! RELEASE_SIGNING_KEY has been uploaded to ${REPO} without being printed.
The temporary plaintext private key in ${tmpdir} is being deleted now.
EOF

if [ -n "$BACKUP_PATH" ]; then
  cat <<EOF

Encrypted offline backup saved and verified (mode 0600):
  ${BACKUP_PATH}
Store this file in your password manager or offline vault (never commit it).

To restore RELEASE_SIGNING_KEY from the backup later without writing plaintext to disk:
  openssl pkey -in "${BACKUP_PATH}" | gh secret set RELEASE_SIGNING_KEY --repo "${REPO}"
EOF
fi

cat <<EOF

Public key (64-character lowercase hex, raw 32-byte Ed25519):
  ${pub_hex}

Next steps (commit ONLY the public key, never the private key):
  1. Add the hex line above to the single source of truth:
       packaging/release-signing/public-keys.txt
     (Both .github/workflows/release.yml and crates/core/src/update/verify.rs read it.)
  2. Commit and push to main before tagging a release.
EOF
