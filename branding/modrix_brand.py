#!/usr/bin/env python3
# SPDX-License-Identifier: GPL-2.0-only
"""Generate the Modrix wordmark and icon as self-contained SVG, plus a preview page.

The brand is two-tone: `MOD` in the Aurora accent blue, `RIX` in white, on black.
Two lockups:

  wordmark  MODRIX set solid on one line, for headers and the GUI sidebar.
  icon      MOD over RIX, square, for app icons and favicons. RIX is tracked
            out until its ink width matches MOD's, so both lines are flush.

Glyph outlines are baked into `glyphs.json` as path data, so nothing here needs a
font installed at generation time and nothing needs one at render time either.
Every measurement below is expressed in cap-height units, so a lockup is defined
by one number (its cap height) and scales exactly to any size.

Usage:
    python3 modrix_brand.py            # write SVG + HTML into dist/
    python3 modrix_brand.py --verify   # also check output against the spec
"""

import json
import os
import sys

HERE = os.path.dirname(os.path.abspath(__file__))
DIST = os.path.join(HERE, "dist")

# --- palette ---------------------------------------------------------------
# BLUE is `theme::AURORA.accent` in crates/modrix-gui/src/theme.rs. If one
# moves, move the other: the logo and the running app share this color.
BLACK = "#000000"
BLUE = "#3B82F6"
WHITE = "#FFFFFF"

# --- geometry, in cap-height units -----------------------------------------
# Measured off the reference artwork, then rounded to the values that reproduce
# it to within a pixel. See README.md for the derivation.
PAD_X = 0.35          # wordmark side padding
PAD_Y = 0.22          # wordmark top and bottom padding
ICON_CANVAS = 4.0     # icon canvas is a square of 4 cap heights
ICON_BASELINE = 1.63  # icon baseline-to-baseline distance
ROUND_RADIUS = 0.18   # rounded-icon corner radius, as a fraction of the canvas

WORD_BLUE, WORD_WHITE = "MOD", "RIX"


def load_glyphs(path=os.path.join(HERE, "glyphs.json")):
    with open(path, encoding="utf-8") as fh:
        return json.load(fh)


class Font:
    """The six outlines we need, in font units, with y pointing up."""

    def __init__(self, data):
        self.upem = data["unitsPerEm"]
        self.cap = data["capHeight"]
        self.glyphs = data["glyphs"]

    def advance(self, ch):
        return self.glyphs[ch]["advance"]

    def ink_width(self, word, tracking=0.0):
        """Left edge of the first glyph to right edge of the last, in font units."""
        pen = 0.0
        for ch in word[:-1]:
            pen += self.advance(ch) + tracking
        return (pen + self.glyphs[word[-1]]["xMax"]) - self.glyphs[word[0]]["xMin"]

    def tracking_to_match(self, word, target_ink):
        """Letter-spacing that stretches `word` to exactly `target_ink` font units."""
        gaps = len(word) - 1
        if gaps <= 0:
            return 0.0
        return (target_ink - self.ink_width(word)) / gaps


def line_svg(font, word, color, tracking, x_ink_left, baseline, scale):
    """One line of text as an SVG group, positioned by its ink left edge."""
    # Font units have y up, SVG has y down, so the group flips y.
    origin = x_ink_left - font.glyphs[word[0]]["xMin"] * scale
    parts = [
        f'  <g fill="{color}" transform="translate({fmt(origin)} {fmt(baseline)}) '
        f'scale({fmt(scale)} {fmt(-scale)})">'
    ]
    pen = 0.0
    for ch in word:
        shift = f' transform="translate({fmt(pen)} 0)"' if pen else ""
        parts.append(f'    <path{shift} d="{font.glyphs[ch]["d"]}"/>')
        pen += font.advance(ch) + tracking
    parts.append("  </g>")
    return "\n".join(parts)


def fmt(value):
    """Trim float noise so the SVG stays readable and diffs stay small."""
    return f"{value:.3f}".rstrip("0").rstrip(".")


def wordmark(font, cap=23.0, background=True):
    """MODRIX on one line, set solid. Reference artwork is cap 23 at 147x33."""
    scale = cap / font.cap
    word = WORD_BLUE + WORD_WHITE
    ink = font.ink_width(word) * scale
    # Round the canvas to whole pixels, then centre the ink inside it, so the
    # exported PNGs land on integer sizes without a half-pixel drift.
    width = round(ink + 2 * PAD_X * cap)
    height = round(cap + 2 * PAD_Y * cap)
    baseline = (height - cap) / 2 + cap
    split = sum(font.advance(c) for c in WORD_BLUE) * scale

    body = []
    if background:
        body.append(f'  <rect width="{fmt(width)}" height="{fmt(height)}" fill="{BLACK}"/>')
    left = (width - ink) / 2
    body.append(line_svg(font, WORD_BLUE, BLUE, 0.0, left, baseline, scale))
    blue_ink_left = left - font.glyphs[WORD_BLUE[0]]["xMin"] * scale
    white_left = blue_ink_left + split + font.glyphs[WORD_WHITE[0]]["xMin"] * scale
    body.append(line_svg(font, WORD_WHITE, WHITE, 0.0, white_left, baseline, scale))
    return document(width, height, body, "Modrix wordmark")


def icon(font, cap=64.0, background="square"):
    """MOD over RIX in a square. Reference artwork is cap ~60 at 239x239."""
    scale = cap / font.cap
    side = round(ICON_CANVAS * cap)
    top_ink = font.ink_width(WORD_BLUE) * scale
    track = font.tracking_to_match(WORD_WHITE, font.ink_width(WORD_BLUE))
    gap = ICON_BASELINE * cap

    # Centre the block (cap top of line one to baseline of line two) vertically.
    block = cap + gap
    baseline_top = (side - block) / 2 + cap
    left = (side - top_ink) / 2

    body = []
    if background == "square":
        body.append(f'  <rect width="{fmt(side)}" height="{fmt(side)}" fill="{BLACK}"/>')
    elif background == "rounded":
        radius = ROUND_RADIUS * side
        body.append(
            f'  <rect width="{fmt(side)}" height="{fmt(side)}" '
            f'rx="{fmt(radius)}" ry="{fmt(radius)}" fill="{BLACK}"/>'
        )
    body.append(line_svg(font, WORD_BLUE, BLUE, 0.0, left, baseline_top, scale))
    # `track` is in font units because line_svg advances the pen in font units.
    body.append(line_svg(font, WORD_WHITE, WHITE, track, left, baseline_top + gap, scale))
    return document(side, side, body, "Modrix icon")


def document(width, height, body, title):
    inner = "\n".join(body)
    return (
        '<?xml version="1.0" encoding="UTF-8"?>\n'
        "<!-- SPDX-License-Identifier: GPL-2.0-only -->\n"
        "<!-- Generated by branding/modrix_brand.py. Do not edit by hand. -->\n"
        f'<svg xmlns="http://www.w3.org/2000/svg" width="{fmt(width)}" '
        f'height="{fmt(height)}" viewBox="0 0 {fmt(width)} {fmt(height)}" '
        'role="img" aria-label="Modrix">\n'
        f"  <title>{title}</title>\n"
        f"{inner}\n"
        "</svg>\n"
    )


VARIANTS = {
    "modrix-wordmark": lambda f: wordmark(f, background=True),
    "modrix-wordmark-transparent": lambda f: wordmark(f, background=False),
    "modrix-icon": lambda f: icon(f, background="square"),
    "modrix-icon-transparent": lambda f: icon(f, background=None),
    "modrix-icon-rounded": lambda f: icon(f, background="rounded"),
}


def write_svgs(font):
    os.makedirs(DIST, exist_ok=True)
    written = []
    for name, build in VARIANTS.items():
        path = os.path.join(DIST, name + ".svg")
        with open(path, "w", encoding="utf-8") as fh:
            fh.write(build(font))
        written.append(path)
    return written


ICON_SIZES = (16, 24, 32, 48, 64, 128, 256, 512, 1024)
WORDMARK_HEIGHTS = (33, 66, 132, 264)


def swatch(name, value, note):
    return (
        f'<div class="sw"><span class="chip" style="background:{value}"></span>'
        f"<b>{name}</b><code>{value}</code><span>{note}</span></div>"
    )


def preview_html(font):
    """A self-contained page for eyeballing every lockup at every size."""
    word = wordmark(font, background=False)
    mark = icon(font, background=None)
    square = icon(font, background="square")
    rounded = icon(font, background="rounded")
    ladder = "".join(
        f'<figure style="width:{s}px"><div style="width:{s}px;height:{s}px">{square}</div>'
        f"<figcaption>{s}</figcaption></figure>"
        for s in ICON_SIZES if s <= 256
    )
    scales = "".join(
        f'<figure><div style="height:{h}px">{word}</div>'
        f"<figcaption>{h}px tall</figcaption></figure>"
        for h in WORDMARK_HEIGHTS
    )
    palette = (
        swatch("blue", BLUE, "MOD, and theme::AURORA.accent in the GUI")
        + swatch("white", WHITE, "RIX")
        + swatch("black", BLACK, "background")
    )
    rows = "".join(
        f"<tr><td>{k}</td><td>{v}</td></tr>" for k, v in (
            ("typeface", "Noto Sans Bold, outlines baked to paths"),
            ("wordmark side padding", f"{PAD_X} cap"),
            ("wordmark vertical padding", f"{PAD_Y} cap"),
            ("icon canvas", f"{ICON_CANVAS} cap square"),
            ("icon baseline to baseline", f"{ICON_BASELINE} cap"),
            ("RIX tracking", "computed so RIX ink width equals MOD ink width"),
        )
    )
    return f"""<!DOCTYPE html>
<!-- SPDX-License-Identifier: GPL-2.0-only -->
<!-- Generated by branding/modrix_brand.py. Do not edit by hand. -->
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Modrix brand</title>
<style>
 :root {{ color-scheme: dark; }}
 body {{ margin:0; background:#0b0d10; color:#e6eaf2; font:15px/1.6 system-ui,sans-serif; }}
 main {{ max-width:1000px; margin:0 auto; padding:48px 24px 96px; }}
 h1 {{ font-size:28px; margin:0 0 4px; }}
 h2 {{ font-size:14px; text-transform:uppercase; letter-spacing:.1em;
       color:#8b93a3; margin:48px 0 16px; font-weight:600; }}
 p.lede {{ color:#8b93a3; margin:0 0 8px; }}
 .panel {{ background:#000; border:1px solid #1c2128; border-radius:10px; padding:32px; }}
 .panel.light {{ background:#fff; }}
 .panel.grey {{ background:#6b7280; }}
 .row {{ display:flex; gap:16px; flex-wrap:wrap; align-items:flex-end; }}
 figure {{ margin:0; }}
 figcaption {{ color:#8b93a3; font-size:12px; margin-top:8px; text-align:center; }}
 svg {{ display:block; width:100%; height:100%; }}
 figure > div {{ display:flex; align-items:center; }}
 .sw {{ display:flex; align-items:center; gap:12px; padding:8px 0; }}
 .chip {{ width:36px; height:36px; border-radius:6px; border:1px solid #2a3038; }}
 .sw b {{ width:60px; }} .sw code {{ width:90px; color:#93c5fd; }}
 .sw span:last-child {{ color:#8b93a3; font-size:13px; }}
 table {{ border-collapse:collapse; width:100%; font-size:14px; }}
 td {{ padding:8px 12px; border-top:1px solid #1c2128; }}
 td:first-child {{ color:#8b93a3; width:240px; }}
</style></head><body><main>
<h1>Modrix brand</h1>
<p class="lede">Generated by <code>branding/modrix_brand.py</code>. Every mark on this
page is the same SVG the pipeline writes to <code>dist/</code>.</p>

<h2>Wordmark</h2>
<div class="panel"><div style="height:44px;width:196px">{word}</div></div>
<div class="row" style="margin-top:16px">
  <div class="panel light" style="flex:1"><div style="height:44px;width:196px">{word}</div></div>
  <div class="panel grey" style="flex:1"><div style="height:44px;width:196px">{word}</div></div>
</div>
<p class="lede" style="margin-top:12px">On white and on mid grey the transparent
variant keeps RIX white, so it needs a dark surface. Use the black-background
variant anywhere else.</p>
<div class="row" style="margin-top:24px">{scales}</div>

<h2>Icon</h2>
<div class="row">
  <figure><div style="width:256px;height:256px">{square}</div>
    <figcaption>square</figcaption></figure>
  <figure><div style="width:256px;height:256px">{rounded}</div>
    <figcaption>rounded, {ROUND_RADIUS} radius</figcaption></figure>
  <figure><div class="panel" style="padding:0;background:#3b4252">
    <div style="width:256px;height:256px">{mark}</div></div>
    <figcaption>transparent</figcaption></figure>
</div>

<h2>Size ladder</h2>
<div class="row">{ladder}</div>

<h2>Palette</h2>
{palette}

<h2>Spec</h2>
<table>{rows}</table>
</main></body></html>
"""


def verify(font):
    """Check the generated geometry against the reference artwork it copies."""
    checks = []
    scale = 23.0 / font.cap
    ink = font.ink_width(WORD_BLUE + WORD_WHITE) * scale
    checks.append(("wordmark canvas width", ink + 2 * PAD_X * 23.0, 147.0, 1.5))
    checks.append(("wordmark canvas height", 23.0 + 2 * PAD_Y * 23.0, 33.0, 1.0))

    cap = 239.0 / ICON_CANVAS
    s = cap / font.cap
    top_ink = font.ink_width(WORD_BLUE) * s
    checks.append(("icon MOD ink width", top_ink, 196.0, 2.5))
    checks.append(("icon side padding", (239.0 - top_ink) / 2, 22.0, 1.5))
    checks.append(("icon baseline delta", ICON_BASELINE * cap, 97.0, 1.5))
    track = font.tracking_to_match(WORD_WHITE, font.ink_width(WORD_BLUE))
    checks.append(("icon RIX ink width", font.ink_width(WORD_WHITE, track) * s, 196.0, 2.5))

    ok = True
    for label, got, want, tol in checks:
        good = abs(got - want) <= tol
        ok = ok and good
        print(f"  [{'ok' if good else 'FAIL'}] {label:24s} {got:8.2f}  "
              f"reference {want:6.1f}  (tolerance {tol})")
    return ok


def main(argv):
    font = Font(load_glyphs())
    for path in write_svgs(font):
        print("wrote", os.path.relpath(path, HERE))
    page = os.path.join(DIST, "preview.html")
    with open(page, "w", encoding="utf-8") as fh:
        fh.write(preview_html(font))
    print("wrote", os.path.relpath(page, HERE))
    if "--verify" in argv:
        print("\nverifying against the reference artwork:")
        if not verify(font):
            return 1
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
