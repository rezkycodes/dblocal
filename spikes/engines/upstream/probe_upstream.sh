#!/usr/bin/env bash
# Upstream source table probe: HEAD-only metadata for PostgreSQL, MySQL 8.4 LTS,
# MariaDB LTS and Valkey across os x arch. Downloads NOTHING except small JSON/API
# responses and checksum texts.
set -euo pipefail
SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
. "$SCRIPT_DIR/../lib/common.sh"

OUT="${OUT:-/tmp/upstream-probe.txt}"
: > "$OUT"

emit() { printf '%s\n' "$*" | tee -a "$OUT"; }

probe() {
  local label="$1" url="$2"
  local out code clen
  out="$(curl -sSL -m 25 -o /dev/null -I -w '%{http_code}|%{url_effective}' "$url" 2>/dev/null || echo "ERR||")"
  code="${out%%|*}"
  url_eff="${out#*|}"
  clen="$(curl -sSL -m 25 -I "$url" 2>/dev/null | tr -d '\r' | awk 'tolower($1)=="content-length:"{v=$2} END{print v}')"
  printf '%-58s %-5s %-12s %s\n' "$label" "$code" "${clen:-?}" "$url_eff" | tee -a "$OUT"
}

emit "### PostgreSQL (theseus-rs/postgresql-binaries) — GitHub Releases API"
python3 - <<'PY' 2>&1 | tee -a "$OUT"
import json, urllib.request
d = json.load(urllib.request.urlopen(
    "https://api.github.com/repos/theseus-rs/postgresql-binaries/releases?per_page=100",
    timeout=60))
want = {"18.7.0", "17.12.0", "16.16.0"}
for r in d:
    if r["tag_name"] not in want:
        continue
    print(f"\n-- tag {r['tag_name']} ({r['published_at']})")
    for a in sorted(r["assets"], key=lambda x: x["name"]):
        n = a["name"]
        if n.endswith((".sha256", ".manifest.json", ".provenance.json",
                       ".sbom-attestation.json", ".spdx.json")):
            continue
        print(f"  {n:62s} {a['size']:>11d}  {a['browser_download_url']}")
    for a in sorted(r["assets"], key=lambda x: x["name"]):
        if a["name"].endswith(".sha256") and "x86_64-unknown-linux-gnu" in a["name"]:
            print(f"  checksum-file: {a['browser_download_url']}")
PY

emit ""
emit "### MySQL 8.4 LTS (dev.mysql.com -> cdn.mysql.com)"
for u in \
  "https://dev.mysql.com/get/Downloads/MySQL-8.4/mysql-8.4.10-linux-glibc2.28-x86_64.tar.xz" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-linux-glibc2.28-x86_64.tar.xz" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-linux-glibc2.28-aarch64.tar.xz" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-macos15-x86_64.tar.gz" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-macos15-arm64.tar.gz" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-macos14-x86_64.tar.gz" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-macos14-arm64.tar.gz" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-winx64.zip" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-winx64.msi" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-winx64-arm64.zip" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-winx64-arm64.msi" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-linux-glibc2.28-x86_64.tar.xz.sha256" \
  "https://cdn.mysql.com/Downloads/MySQL-8.4/mysql-8.4.10-linux-glibc2.28-x86_64.tar.xz.asc" \
  ; do
  probe "$(basename "$u")" "$u"
done

emit ""
emit "### MariaDB REST API — current LTS lines"
python3 - <<'PY' 2>&1 | tee -a "$OUT"
import json, urllib.request
idx = json.load(urllib.request.urlopen(
    "https://downloads.mariadb.org/rest-api/mariadb/", timeout=60))
print("major_releases:", [r["release_id"] for r in idx["major_releases"]])
for rel in ("11.8", "11.4", "13.2"):
    try:
        d = json.load(urllib.request.urlopen(
            f"https://downloads.mariadb.org/rest-api/mariadb/{rel}/", timeout=60))
        print(f"\n-- {rel}: releases {list(d['releases'].keys())[:5]}")
    except Exception as e:
        print(f"\n-- {rel}: ERROR {e}")
    except SystemExit:
        pass
PY

emit ""
emit "### MariaDB 11.8.10 artifact probe (HEAD only, linux x86_64/aarch64, macOS, Windows)"
for u in \
  "https://downloads.mariadb.org/rest-api/mariadb/11.8.10/mariadb-11.8.10-linux-systemd-x86_64.tar.gz" \
  "https://downloads.mariadb.org/rest-api/mariadb/11.8.10/mariadb-11.8.10-linux-systemd-aarch64.tar.gz" \
  "https://downloads.mariadb.org/rest-api/mariadb/11.8.10/mariadb-11.8.10-osx-aarch64.tar.gz" \
  "https://downloads.mariadb.org/rest-api/mariadb/11.8.10/mariadb-11.8.10-osx-x86_64.tar.gz" \
  "https://downloads.mariadb.org/rest-api/mariadb/11.8.10/mariadb-11.8.10-winx64.zip" \
  "https://downloads.mariadb.org/rest-api/mariadb/11.8.10/mariadb-11.8.10-winx64.msi" \
  ; do
  probe "$(basename "$u")" "$u"
done

emit ""
emit "### Valkey (download.valkey.io) — Linux only, per-platform variants"
for v in 9.2.0 9.1.4; do
  for variant in noble jammy; do
    for arch in x86_64 aarch64; do
      u="https://download.valkey.io/releases/valkey-${v}-${variant}-${arch}.tar.gz"
      probe "valkey-${v}-${variant}-${arch}.tar.gz" "$u"
      probe "valkey-${v}-${variant}-${arch}.tar.gz.sha256" "$u.sha256"
    done
  done
done
for guess in macos-x86_64 macos-aarch64 darwin-x86_64 darwin-aarch64 windows-x86_64 windows-aarch64; do
  probe "valkey-9.2.0-${guess}.tar.gz (guess)" \
    "https://download.valkey.io/releases/valkey-9.2.0-${guess}.tar.gz"
done

emit ""
emit "### Valkey GitHub releases (assets present?)"
python3 - <<'PY' 2>&1 | tee -a "$OUT"
import json, urllib.request
d = json.load(urllib.request.urlopen(
    "https://api.github.com/repos/valkey-io/valkey/releases?per_page=10", timeout=60))
for r in d:
    print(f"{r['tag_name']:12s} assets={len(r['assets'])} tarball={r['tarball_url']}")
PY

emit ""
emit "done -> $OUT"