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


GUIDE = "#3B82F6"


def annotated_icon(font, cap=180.0):
    """The icon with its dimension lines drawn, in cap units, from the same
    constants that place the glyphs. A spec sheet that cannot drift."""
    side = round(ICON_CANVAS * cap)
    gap = ICON_BASELINE * cap
    base_top = (side - (cap + gap)) / 2 + cap
    base_bot = base_top + gap
    cap_top = base_top - cap
    pad_l, pad_b = 108, 60

    def dim(x, y0, y1, label):
        """A vertical dimension line with end ticks and a right-aligned label."""
        return (
            f'<path d="M{x} {fmt(y0)}L{x} {fmt(y1)}M{x-5} {fmt(y0)}h10M{x-5} '
            f'{fmt(y1)}h10" stroke="{GUIDE}" stroke-width="1.5" fill="none"/>'
            f'<text x="{x-14}" y="{fmt((y0 + y1) / 2 + 4)}" text-anchor="end" '
            f'fill="{GUIDE}" font-size="13" font-family="ui-monospace,monospace">{label}</text>'
        )

    guides = "".join(
        f'<path d="M-96 {fmt(y)}H{side}" stroke="{GUIDE}" stroke-width="1" '
        f'stroke-dasharray="2 4" opacity=".45" fill="none"/>'
        for y in (cap_top, base_top, base_bot)
    )
    across = (
        f'<path d="M0 {side + 30}H{side}M0 {side + 25}v10M{side} {side + 25}v10" '
        f'stroke="{GUIDE}" stroke-width="1.5" fill="none"/>'
        f'<text x="{side / 2}" y="{side + 52}" text-anchor="middle" fill="{GUIDE}" '
        f'font-size="13" font-family="ui-monospace,monospace">'
        f"{ICON_CANVAS:.2f} cap</text>"
    )
    inner = icon(font, cap=cap, background="square")
    inner = inner[inner.index("<svg") :]
    inner = inner[inner.index(">") + 1 : inner.rindex("</svg>")]
    return (
        f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="{-pad_l} -20 '
        f'{side + pad_l + 20} {side + pad_b + 20}" role="img" '
        f'aria-label="Modrix icon with dimensions">{inner}{guides}'
        f'{dim(-34, cap_top, base_top, "1.00 cap")}'
        f'{dim(-34, base_top, base_bot, f"{ICON_BASELINE:.2f} cap")}'
        f"{across}</svg>"
    )


def swatch(name, value, note):
    return (
        f'<li class="sw"><span class="chip" style="background:{value}"></span>'
        f'<span class="sw-n">{name}</span><code>{value}</code>'
        f"<span class=\"sw-r\">{note}</span></li>"
    )


CSS = """
:root {
  --ground:#0A0C10; --surface:#111620; --line:#1E2530;
  --ink:#E7EBF3; --muted:#79839A; --accent:#3B82F6; --ok:#34D399;
  color-scheme: dark light;
}
@media (prefers-color-scheme: light) {
  :root { --ground:#F6F7FA; --surface:#FFFFFF; --line:#DFE4EC;
          --ink:#0A0C10; --muted:#5B6577; --accent:#2563EB; --ok:#059669; }
}
:root[data-theme="dark"] {
  --ground:#0A0C10; --surface:#111620; --line:#1E2530;
  --ink:#E7EBF3; --muted:#79839A; --accent:#3B82F6; --ok:#34D399;
}
:root[data-theme="light"] {
  --ground:#F6F7FA; --surface:#FFFFFF; --line:#DFE4EC;
  --ink:#0A0C10; --muted:#5B6577; --accent:#2563EB; --ok:#059669;
}
* { box-sizing: border-box; }
body {
  margin:0; background:var(--ground); color:var(--ink);
  font:15px/1.65 system-ui,-apple-system,"Segoe UI",Roboto,sans-serif;
  -webkit-font-smoothing: antialiased;
}
code, .mono, td.num { font-family: ui-monospace,SFMono-Regular,Menlo,Consolas,monospace; }
main { max-width: 940px; margin:0 auto; padding: 72px 28px 120px;
       display:flex; flex-direction:column; gap:64px; }
header { display:flex; flex-direction:column; gap:20px; }
.brandbar { background:#000; border:1px solid var(--line); border-radius:12px;
            padding:36px 40px; display:flex; }
.brandbar > div { width:260px; }
h1 { font-size:34px; line-height:1.1; margin:0; font-weight:800;
     letter-spacing:-.021em; text-wrap:balance; }
.lede { color:var(--muted); margin:0; max-width:62ch; }
section { display:flex; flex-direction:column; gap:20px; }
h2 { font-size:11px; text-transform:uppercase; letter-spacing:.15em;
     font-weight:700; color:var(--muted); margin:0;
     padding-bottom:12px; border-bottom:1px solid var(--line); }
.note { color:var(--muted); margin:0; font-size:14px; max-width:62ch; }
.row { display:flex; gap:20px; flex-wrap:wrap; align-items:flex-end; }
.panel { border:1px solid var(--line); border-radius:12px; padding:32px;
         display:flex; align-items:center; justify-content:center; }
.on-black { background:#000; } .on-white { background:#fff; }
.on-grey  { background:#6B7280; }
figure { margin:0; display:flex; flex-direction:column; gap:10px; align-items:center; }
figcaption { color:var(--muted); font-size:12px; text-align:center;
             font-family: ui-monospace,SFMono-Regular,Menlo,monospace; }
svg { display:block; width:100%; height:100%; }
figure .box { display:flex; align-items:center; }
.spec { background:var(--surface); border:1px solid var(--line);
        border-radius:12px; padding:28px 32px; }
.spec svg { max-width:520px; margin:0 auto; height:auto; }
ul.pal { list-style:none; margin:0; padding:0; display:flex;
         flex-direction:column; gap:2px; }
.sw { display:flex; align-items:center; gap:16px; padding:10px 0;
      border-top:1px solid var(--line); }
.sw:first-child { border-top:0; }
.chip { width:40px; height:40px; border-radius:8px; border:1px solid var(--line);
        flex:none; }
.sw-n { width:64px; font-weight:600; }
.sw code { width:96px; color:var(--accent); }
.sw-r { color:var(--muted); font-size:14px; }
.scroll { overflow-x:auto; }
table { border-collapse:collapse; width:100%; font-size:14px;
        font-variant-numeric: tabular-nums; }
th { text-align:left; font-size:11px; text-transform:uppercase;
     letter-spacing:.12em; color:var(--muted); font-weight:700;
     padding:0 16px 10px 0; white-space:nowrap; }
td { padding:10px 16px 10px 0; border-top:1px solid var(--line);
     vertical-align:top; }
td.num { text-align:right; white-space:nowrap; }
td.k { color:var(--muted); width:270px; }
.pass { color:var(--ok); font-weight:700; }
.files { columns:2; column-gap:40px; font-size:14px; }
.files p { margin:0 0 8px; break-inside:avoid; }
.files b { font-family: ui-monospace,SFMono-Regular,Menlo,monospace;
           font-weight:600; display:block; }
.files span { color:var(--muted); }
@media (max-width:640px) { .files { columns:1; } .sw-r { display:none; } }
"""


def preview_html(font):
    """A self-contained specimen sheet: every lockup, size, colour, and the
    measurements that prove the marks match the reference artwork."""
    word = wordmark(font, background=False)
    boxes = "".join(
        f'<div class="panel {cls}"><div style="width:210px;height:47px">{word}</div></div>'
        for cls in ("on-black", "on-white", "on-grey")
    )
    scales = "".join(
        f'<figure><div class="box" style="height:{h}px">'
        f'<div style="height:{h}px;width:{round(h * 148 / 33)}px">{word}</div></div>'
        f"<figcaption>{h}px</figcaption></figure>"
        for h in WORDMARK_HEIGHTS[:3]
    )
    square, rounded = icon(font, background="square"), icon(font, background="rounded")
    mark = icon(font, background=None)
    lockups = (
        f'<figure><div style="width:220px;height:220px">{square}</div>'
        f"<figcaption>square</figcaption></figure>"
        f'<figure><div style="width:220px;height:220px">{rounded}</div>'
        f'<figcaption>rounded {ROUND_RADIUS}</figcaption></figure>'
        f'<figure><div class="panel on-grey" style="padding:0;border-radius:0">'
        f'<div style="width:220px;height:220px">{mark}</div></div>'
        f"<figcaption>transparent</figcaption></figure>"
    )
    ladder = "".join(
        f'<figure><div class="box" style="width:{s}px;height:{s}px">{square}</div>'
        f"<figcaption>{s}</figcaption></figure>"
        for s in ICON_SIZES if s <= 128
    )
    palette = (
        swatch("blue", BLUE, "MOD, and theme::AURORA.accent in the GUI")
        + swatch("white", WHITE, "RIX")
        + swatch("black", BLACK, "ground")
    )
    spec = "".join(
        f'<tr><td class="k">{k}</td><td>{v}</td></tr>' for k, v in (
            ("typeface", "Noto Sans Bold, outlines baked to paths"),
            ("wordmark side padding", f"{PAD_X} cap"),
            ("wordmark vertical padding", f"{PAD_Y} cap"),
            ("icon canvas", f"{ICON_CANVAS} cap, square"),
            ("icon baseline to baseline", f"{ICON_BASELINE} cap"),
            ("rounded corner radius", f"{ROUND_RADIUS} of the canvas"),
            ("RIX tracking", "computed so RIX ink width equals MOD ink width"),
        )
    )
    checks = "".join(
        f'<tr><td class="k">{label}</td><td class="num">{got:.2f}</td>'
        f'<td class="num">{want:.0f}</td><td class="num">{abs(got - want):.2f}</td>'
        f'<td class="num">{tol}</td>'
        f'<td class="num {"pass" if ok else ""}">{"pass" if ok else "FAIL"}</td></tr>'
        for label, got, want, tol, ok in verify_rows(font)
    )
    files = "".join(
        f"<p><b>{n}</b><span>{d}</span></p>" for n, d in (
            ("modrix-wordmark.svg", "one line, black ground"),
            ("modrix-wordmark-transparent.svg", "one line, no ground"),
            ("modrix-icon.svg", "square, black ground"),
            ("modrix-icon-transparent.svg", "square, no ground"),
            ("modrix-icon-rounded.svg", "square, rounded corners"),
            ("modrix.ico", "Windows, 16 to 256"),
            ("png/", "icons 16 to 1024, wordmarks 33 to 264 tall"),
        )
    )
    return f"""<!DOCTYPE html>
<!-- SPDX-License-Identifier: GPL-2.0-only -->
<!-- Generated by branding/modrix_brand.py. Do not edit by hand. -->
<html lang="en"><head><meta charset="utf-8">
<meta name="viewport" content="width=device-width,initial-scale=1">
<title>Modrix brand</title>
<style>{CSS}</style></head><body><main>

<header>
  <div class="brandbar"><div>{word}</div></div>
  <h1>Modrix brand</h1>
  <p class="lede">Two lockups, three colours, one number. Every mark below is
  drawn from a cap height and a handful of ratios, so the wordmark and the icon
  stay in proportion at any size. This page is written by
  <code>branding/modrix_brand.py</code> and shows the same SVGs it puts in
  <code>dist/</code>.</p>
</header>

<section>
  <h2>Wordmark</h2>
  <div class="row">{boxes}</div>
  <p class="note">RIX is white, so the transparent variant needs a dark ground.
  Anywhere else, use the variant that carries its own black.</p>
  <div class="row">{scales}</div>
</section>

<section>
  <h2>Icon</h2>
  <div class="row">{lockups}</div>
</section>

<section>
  <h2>Construction</h2>
  <div class="spec">{annotated_icon(font)}</div>
  <p class="note">The canvas is four cap heights square and the two baselines sit
  {ICON_BASELINE} cap apart. RIX is not tracked by eye: it is letter-spaced until
  its ink measures exactly as wide as MOD, which is what keeps both lines flush
  when the cap height changes.</p>
</section>

<section>
  <h2>Size ladder</h2>
  <div class="row">{ladder}</div>
  <p class="note">At 16px the two lines are three pixels tall each. Legibility
  gives out before the geometry does, so prefer the wordmark below 24px.</p>
</section>

<section>
  <h2>Palette</h2>
  <ul class="pal">{palette}</ul>
</section>

<section>
  <h2>Spec</h2>
  <div class="scroll"><table><tbody>{spec}</tbody></table></div>
</section>

<section>
  <h2>Verification</h2>
  <p class="note">The palette and geometry were measured out of the reference
  artwork rather than guessed. These checks re-run on every build and fail the
  pipeline if a change drifts out of tolerance.</p>
  <div class="scroll"><table>
    <thead><tr><th>measure</th><th>generated</th><th>reference</th>
    <th>delta</th><th>tolerance</th><th></th></tr></thead>
    <tbody>{checks}</tbody></table></div>
</section>

<section>
  <h2>Files</h2>
  <div class="files">{files}</div>
</section>

</main></body></html>
"""


def verify_rows(font):
    """Generated geometry against the reference artwork, as (label, got, want,
    tolerance, ok). The reference values are pixel measurements taken off the
    supplied artwork: the wordmark at 147x33 and the icon at 239x239."""
    scale = 23.0 / font.cap
    ink = font.ink_width(WORD_BLUE + WORD_WHITE) * scale
    cap = 239.0 / ICON_CANVAS
    s = cap / font.cap
    top_ink = font.ink_width(WORD_BLUE) * s
    track = font.tracking_to_match(WORD_WHITE, font.ink_width(WORD_BLUE))
    checks = [
        ("wordmark canvas width", ink + 2 * PAD_X * 23.0, 147.0, 1.5),
        ("wordmark canvas height", 23.0 + 2 * PAD_Y * 23.0, 33.0, 1.0),
        ("icon MOD ink width", top_ink, 196.0, 2.5),
        ("icon side padding", (239.0 - top_ink) / 2, 22.0, 1.5),
        ("icon baseline delta", ICON_BASELINE * cap, 97.0, 1.5),
        ("icon RIX ink width", font.ink_width(WORD_WHITE, track) * s, 196.0, 2.5),
    ]
    return [(lbl, got, want, tol, abs(got - want) <= tol)
            for lbl, got, want, tol in checks]


def verify(font):
    """Print the verification table. True when every check is inside tolerance."""
    rows = verify_rows(font)
    for label, got, want, tol, ok in rows:
        print(f"  [{'ok' if ok else 'FAIL'}] {label:24s} {got:8.2f}  "
              f"reference {want:6.1f}  (tolerance {tol})")
    return all(row[4] for row in rows)


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
