<!-- SPDX-License-Identifier: GPL-2.0-only -->
# Modrix brand assets

Everything in `dist/` is generated. Edit the spec in `modrix_brand.py`, never the
output.

```sh
./build.sh          # SVG + HTML + PNG + ICO into dist/
python3 modrix_brand.py --verify   # SVG + HTML only, and check the geometry
```

`build.sh` needs `python3`, `rsvg-convert` (librsvg), and `magick` (ImageMagick,
only for the `.ico`). The generator itself is Python standard library only: no
font has to be installed, and none is needed to render the results either.

## The marks

| File | Use |
| --- | --- |
| `modrix-wordmark.svg` | MODRIX on one line, black background. Headers, GUI sidebar. |
| `modrix-wordmark-transparent.svg` | Same, no background. Dark surfaces only, RIX is white. |
| `modrix-icon.svg` | MOD over RIX, square, black background. App icon, favicon. |
| `modrix-icon-transparent.svg` | Same, no background. |
| `modrix-icon-rounded.svg` | Same with rounded corners, for platforms that expect them. |
| `modrix.ico` | Windows icon, 16 through 256. Ready to wire into the NSIS installer. |
| `png/` | Icons at 16 to 1024, wordmarks at 33 to 264 tall. |
| `preview.html` | Self-contained page showing every lockup, size, and colour. |

## Spec

Two-tone: `MOD` in the accent blue, `RIX` in white, on black.

| Colour | Hex | Role |
| --- | --- | --- |
| blue | `#3B82F6` | `MOD` |
| white | `#FFFFFF` | `RIX` |
| black | `#000000` | background |

The blue is the same value as `theme::AURORA.accent` in
`crates/modrix-gui/src/theme.rs`. If one changes, change the other: the logo and
the running app are meant to share one accent.

Typeface is **Noto Sans Bold**. Its outlines are baked into `glyphs.json` as SVG
path data, so the marks render identically everywhere with no font dependency and
no `@font-face`.

Geometry is expressed in cap-height units, so each lockup is defined by a single
number (its cap height) and scales exactly:

| Measure | Value |
| --- | --- |
| wordmark side padding | 0.35 cap |
| wordmark vertical padding | 0.22 cap |
| icon canvas | 4.0 cap, square |
| icon baseline to baseline | 1.63 cap |
| rounded corner radius | 0.18 of the canvas |
| `RIX` tracking | computed so its ink width equals `MOD`'s |

That last rule is what makes the icon's two lines flush: `RIX` is letter-spaced
out until it measures exactly as wide as `MOD`, rather than being tracked by a
hand-picked amount. Change the cap height and the two lines stay flush.

## How the spec was derived

The generator reproduces reference artwork that was supplied as two PNGs. Rather
than eyeball it, the palette and geometry were measured out of those pixels:

- Colours came straight from a pixel histogram: `#000000`, `#3B82F6`, `#FFFFFF`.
- The typeface was identified by rendering every installed bold sans at a matched
  cap height and scoring per-glyph width ratios against the reference. Noto Sans
  Bold won by a wide margin, and was the only candidate whose capital `I` is
  serifed, which the reference clearly is (28px wide over a 14px stem at cap 62).
- Padding, cap height, baselines, and tracking were read off ink bounding boxes
  and column runs, then divided by cap height to get the ratios above.

`modrix_brand.py --verify` re-checks the generated geometry against those measured
reference values and fails if a change drifts out of tolerance. Rendered at the
reference's own 239px, the icon lands within one pixel on every glyph edge, and
ink coverage matches to 0.2 percent.

## Regenerating the outlines

`glyphs.json` only needs rebuilding if the wordmark text or typeface changes. It
holds the six outlines, their advances, and their bounds, in 1000-unit em space:

```sh
uv run --with fonttools python - <<'PY'
from fontTools.ttLib import TTFont
from fontTools.pens.svgPathPen import SVGPathPen
from fontTools.pens.boundsPen import BoundsPen
import json
font = TTFont("/usr/share/fonts/noto/NotoSans-Bold.ttf")
gs, cmap = font.getGlyphSet(), font.getBestCmap()
data = {"unitsPerEm": font["head"].unitsPerEm,
        "capHeight": font["OS/2"].sCapHeight, "glyphs": {}}
for ch in "MODRIX":
    name = cmap[ord(ch)]
    pen, bp = SVGPathPen(gs), BoundsPen(gs)
    gs[name].draw(pen); gs[name].draw(bp)
    x0, y0, x1, y1 = bp.bounds
    data["glyphs"][ch] = {"d": pen.getCommands(), "advance": font["hmtx"][name][0],
                          "xMin": x0, "xMax": x1, "yMin": y0, "yMax": y1}
json.dump(data, open("glyphs.json", "w"), indent=1)
PY
```

## Licensing

The marks are part of Modrix and are GPL-2.0-only like the rest of the tree.

Noto Sans is licensed under the SIL Open Font License 1.1 by the Noto Project
Authors. `glyphs.json` contains outlines for six characters converted to path
data, used as artwork rather than redistributed as font software, which is the
ordinary way a typeface is used to set a logo. The OFL is not in Modrix's
dependency graph and is not checked by `cargo deny`, which only sees Rust crates.
