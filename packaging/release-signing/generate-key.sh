#!/usr/bin/env bash
# Generate an Ed25519 release-signing key pair for DBLocal and upload the
# private key to GitHub Actions secret RELEASE_SIGNING_KEY over stdin.
#
# Usage:
#   bash packaging/release-signing/generate-key.sh [--repo owner/name]
#
# Safety properties:
# - Never prints the private key to stdout or stderr.
# - Stores temporary key material in a 0700 temp directory with 0600 files.
# - Removes the temporary directory on exit, error, or signal (EXIT/INT/TERM/HUP).
# - Prints ONLY the public key (64-char hex + PEM) and where to commit it.
set -euo pipefail

REPO="rezkycodes/dblocal"
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
    -h|--help)
      sed -n '2,13p' "$0"
      exit 0
      ;;
    *)
      echo "error: unknown argument: $1" >&2
      exit 2
      ;;
  esac
done

command -v openssl >/dev/null 2>&1 || { echo "error: openssl is required" >&2; exit 1; }
command -v gh >/dev/null 2>&1 || { echo "error: GitHub CLI (gh) is required" >&2; exit 1; }

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

echo "Uploading private key to GitHub Actions secret RELEASE_SIGNING_KEY on ${REPO}..."
gh secret set RELEASE_SIGNING_KEY --repo "$REPO" < "$priv_pem"

cat <<EOF

Done! RELEASE_SIGNING_KEY has been uploaded to ${REPO} without being printed.

IMPORTANT — Offline Backup:
  Before closing your password manager / hardware backup workflow, consider
  keeping an encrypted offline backup of the signing key (or rotating to a new
  key pair if lost; TRUSTED_PUBLIC_KEYS supports multiple keys for rotation).
  The temporary private key file in ${tmpdir} is being deleted now.

Public key (64-character hex, raw 32-byte Ed25519):
  ${pub_hex}

Next steps (commit ONLY the public key, never the private key):
  1. Add the hex line above to:
       packaging/release-signing/public-keys.txt
  2. Add the same hex string to TRUSTED_PUBLIC_KEYS in:
       crates/core/src/update/verify.rs
  3. Commit and push to main before tagging a release.
EOF
