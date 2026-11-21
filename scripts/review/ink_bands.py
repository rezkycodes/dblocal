#!/usr/bin/env python3
"""Prints the horizontal ink bands of a rectangle of a PNG, for reviewers and workers that cannot see images.

Usage: python3 -I scripts/review/ink_bands.py IMAGE.png X0 Y0 X1 Y1 [MIN_INK_PIXELS]

Coordinates are in IMAGE pixels (the committed renders are 2560x1640: logical 1280x820 at scale 2).
The background is the most common colour of the rectangle. A row has "ink" when at least
MIN_INK_PIXELS (default 3) sampled pixels differ from the background by more than 40 in some
channel. Consecutive ink rows form a band; bands closer than 6 rows are merged.
Prints one line per band: y0 y1 height, then the total. A rectangle with text lines inside it
prints one band per text line; an empty rectangle prints 0 bands.

Needs Pillow (`python3 -c "import PIL"`).
"""
import collections
import sys

from PIL import Image


def main():
    if len(sys.argv) < 6:
        print(__doc__)
        return 2
    path = sys.argv[1]
    x0, y0, x1, y1 = (int(v) for v in sys.argv[2:6])
    min_ink = int(sys.argv[6]) if len(sys.argv) > 6 else 3
    im = Image.open(path).convert("RGB")
    crop = im.crop((x0, y0, x1, y1))
    w, h = crop.size
    px = crop.load()
    bg = collections.Counter(px[x, y] for y in range(h) for x in range(0, w, 3)).most_common(1)[0][0]
    bands, cur = [], None
    for y in range(h):
        ink = sum(1 for x in range(0, w, 2) if max(abs(a - b) for a, b in zip(px[x, y], bg)) > 40)
        if ink >= min_ink:
            if cur is None:
                cur = [y, y]
            else:
                cur[1] = y
        elif cur is not None and y - cur[1] > 6:
            bands.append(tuple(cur))
            cur = None
    if cur is not None:
        bands.append(tuple(cur))
    print(f"image {im.size} rect {(x0, y0, x1, y1)} background {bg}")
    for a, b in bands:
        print(f"  band y {y0 + a}..{y0 + b}  height {b - a + 1}")
    print(f"{len(bands)} ink band(s)")
    return 0


if __name__ == "__main__":
    sys.exit(main())
