#!/usr/bin/env bash
# SPDX-License-Identifier: GPL-2.0-only
#
# Build every Modrix brand asset: SVG and HTML from the generator, then PNG and
# ICO by rasterising those SVGs. Rerunnable and deterministic; dist/ is rebuilt
# from scratch each time.
#
# Needs: python3, rsvg-convert (librsvg), magick (ImageMagick, for the .ico).

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST="$HERE/dist"
PNG="$DIST/png"

need() {
    command -v "$1" >/dev/null 2>&1 || { echo "missing required tool: $1" >&2; exit 1; }
}
need python3
need rsvg-convert

ICON_SIZES=(16 24 32 48 64 128 256 512 1024)
WORDMARK_HEIGHTS=(33 66 132 264)
ICO_SIZES=(16 24 32 48 64 128 256)

rm -rf "$DIST"
python3 "$HERE/modrix_brand.py" --verify
mkdir -p "$PNG"

echo
echo "rasterising PNGs"
for variant in modrix-icon modrix-icon-transparent modrix-icon-rounded; do
    for size in "${ICON_SIZES[@]}"; do
        rsvg-convert -w "$size" -h "$size" \
            "$DIST/$variant.svg" -o "$PNG/$variant-$size.png"
    done
done
for variant in modrix-wordmark modrix-wordmark-transparent; do
    for height in "${WORDMARK_HEIGHTS[@]}"; do
        rsvg-convert -h "$height" "$DIST/$variant.svg" -o "$PNG/$variant-${height}h.png"
    done
done
echo "  $(find "$PNG" -name '*.png' | wc -l) files in dist/png"

if command -v magick >/dev/null 2>&1; then
    ico_inputs=()
    for size in "${ICO_SIZES[@]}"; do
        ico_inputs+=("$PNG/modrix-icon-$size.png")
    done
    magick "${ico_inputs[@]}" "$DIST/modrix.ico"
    echo "  dist/modrix.ico (${ICO_SIZES[*]})"
else
    echo "  skipping modrix.ico: magick not installed" >&2
fi

echo
echo "done. open dist/preview.html to review."
