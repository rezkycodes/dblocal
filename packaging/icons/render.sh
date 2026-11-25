#!/usr/bin/env bash
# Regenerate DBLocal raster PNGs and Windows ICO from the SVG master.
#
# Usage:
#   packaging/icons/render.sh
#
# Requires: ImageMagick (`magick`).
set -euo pipefail

resolve_dir() {
  ( unset CDPATH; cd -- "$1" && pwd -P )
}

script_dir="$(resolve_dir "$(dirname -- "$0")")"
repo_root="$(resolve_dir "$script_dir/../..")"

svg="$repo_root/packaging/linux/dblocal.svg"
png_dir="$script_dir/png"
ico_out="$script_dir/dblocal.ico"

command -v magick >/dev/null 2>&1 || {
  echo "error: required tool not found: magick (ImageMagick)" >&2
  exit 1
}

[ -f "$svg" ] || {
  echo "error: SVG master not found: $svg" >&2
  exit 1
}

mkdir -p "$png_dir"

sizes=(16 24 32 48 64 128 256 512)
for size in "${sizes[@]}"; do
  out="$png_dir/dblocal-${size}.png"
  magick -background none -density 384 "$svg" \
    -resize "${size}x${size}!" \
    -colorspace sRGB \
    -define png:color-type=6 \
    "PNG32:${out}"
done

# Build Windows ICO with PNG-compressed entries (16, 24, 32, 48, 64, 128, 256).
python3 - "$png_dir" "$ico_out" <<'PY'
import struct, sys, os

png_dir, ico_out = sys.argv[1], sys.argv[2]
sizes = [16, 24, 32, 48, 64, 128, 256]
entries = []
for s in sizes:
    with open(os.path.join(png_dir, f"dblocal-{s}.png"), "rb") as f:
        entries.append((s, f.read()))

# ICONDIR: reserved(2)=0, type(2)=1 (ICO), count(2)
header = struct.pack("<HHH", 0, 1, len(entries))
offset = 6 + 16 * len(entries)
directory = bytearray()
payload = bytearray()
for s, data in entries:
    dim = 0 if s >= 256 else s
    # width, height, color_count, reserved, planes, bpp, size, offset
    directory += struct.pack("<BBBBHHII", dim, dim, 0, 0, 1, 32, len(data), offset)
    payload += data
    offset += len(data)

with open(ico_out, "wb") as f:
    f.write(header)
    f.write(directory)
    f.write(payload)
PY

echo "generated PNGs in $png_dir and ICO at $ico_out"
