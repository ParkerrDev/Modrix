<!-- SPDX-License-Identifier: GPL-2.0-only -->
# ModManager - Build Roadmap

Phased so each stage produces something real and testable. The rules throughout:
**engine first, frontends thin** (the CLI exists before the TUI/GUI precisely to
prove the engine works headless), and **every crate is held to the Power of Ten
reliability standard** (see [`ARCHITECTURE.md §9.3`](ARCHITECTURE.md)), enforced in
CI from commit #1.

Legend: each phase lists its crates and a concrete **Done when** bar.

---

## Phase 0 - Foundation
Workspace + plumbing. No features yet.
- Cargo workspace, the crate skeletons from `ARCHITECTURE.md §3`.
- **Power of Ten enforcement from commit #1:** workspace `[lints]` (forbid
  `unsafe`, panic-free, indexing/arithmetic guards), `clippy.toml` (≤60-line
  functions), `deny.toml` (GPLv2 license gate + advisories). Full configs live
  at the repository root.
- `directories`-based config/data/cache paths; structured logging (`tracing`).
- SQLite schema + migrations (`rusqlite`); open/create the DB.
- GPL-2.0 `LICENSE` (full text) + SPDX headers on every source file; `README`.
- CI = fmt + clippy `-D warnings` + test + `cargo deny` on Linux/Windows/macOS.

**Done when:** CI green on all 3 OSes with **zero clippy warnings under
`-D warnings`**; `cargo deny` passes; the app creates its data dir and an empty DB.

---

## Phase 1 - Deployment engine + CLI  ← the core bet
The correctness-critical heart, exercised headless. Panic-free, `unsafe`-free,
bounded (Power of Ten).
- `modman-core`: domain model, mod store (stage an extracted folder), the pure
  planner + transactional applier (link→symlink→copy), manifest, backup/restore,
  journal, dry-run, verify.
- `modman-cli`: `game add`, `mod add <archive>`, `mod enable/disable`,
  `loadorder`, `deploy`, `undeploy`, `verify`, `profile` commands.
- One game as a **`game.toml`** (a simple Steam title) to prove the data-driven path.
- Heavy tests: the five engine invariants -
  reversibility (I1), idempotence (I2), no-silent-clobber (I3), crash-safety (I4,
  fault injection), determinism (I5) - via proptest + a temp-dir filesystem harness.

**Done when:** invariants I1-I5 pass on Linux/Windows/macOS, and from the CLI you can
stage mods, deploy via hardlinks, switch profiles, and fully roll back - verified
byte-identical.

---

## Phase 2 - Downloads: browser hand-off end-to-end
Make "click in browser → installed" real - **with no site API and no API key**
(corrected from the original API-based plan; see `ARCHITECTURE.md §6`).
- `modman-download`: a segmented, multi-connection, **resumable, checksum-verified**
  download engine (a clean-room aria2/Motrix-style engine, no aria2 code), on the
  GPLv2-clean hyper + rustls stack. FIFO queue + concurrency cap + progress events.
  Retains an `nxm://` identity parser (not a download mechanism).
- `modman-ipc`: loopback listener + single-instance guard + session token; a JSON
  `POST /download` hand-off endpoint (+ `GET /download/<id>`, `/downloads`).
- `modman-protocol`: the tiny OS-registered forwarder (dormant; identity-only).
- `extension/`: the **MV3 WebExtension** (Chrome + Firefox) that intercepts the
  browser's own download, captures URL + cookies + User-Agent + referrer, cancels
  it, and hands it to the loopback endpoint with the session token.
- `serve`: routes a hand-off to a game by domain and stages it on completion.

**Done when:** with the GUI closed, an extension hand-off downloads and installs to
the correct game automatically. (Live sites' download DOM and OS registration can't
be exercised in CI, so those are behind tests + a mock server and flagged for manual
verification; the headless pipeline is proven end-to-end against a mock.)

---

## Phase 3 - Plugins + installers
Open the gates for community compatibility.
- `modman-plugin`: `mlua` host, the sandboxed `modman` API, plugin discovery,
  `api_version` gating, per-call step/time budget.
- FOMOD engine in core (`ModuleConfig.xml`), driven from a frontend-agnostic
  wizard interface (CLI prompts for now); bounded condition tree.
- Port the Phase-1 game to a `game.lua` where it needs logic; document the plugin
  API and ship 2-3 example plugins.

**Done when:** a third party can add a new game with only a `game.toml`, or a
`game.lua` for a game needing a custom installer, without touching core.

---

## Phase 4 - TUI
- `modman-tui` (`ratatui`): mod list, load-order reorder, conflict view, download
  queue, profile switching, the FOMOD wizard.

**Done when:** the TUI can do everything the CLI can, interactively.

---

## Phase 5 - GUI
- `modman-gui` (**Iced**, MIT): the same feature set, themed classy/macOS-like;
  drag-to-reorder load order; the install wizard; settings.
- Optional system-tray / headless-background so browser clicks work while "closed."

**Done when:** a non-technical user can install a mod from the browser and manage
load order entirely in the GUI.

---

## Phase 6 - Ecosystem & polish
- WebExtension (Firefox/Chrome) to complement the userscript; groundwork for a
  second `ModSource` (non-Nexus site).
- Packaging: AppImage/Flatpak (Linux), MSI/portable (Windows), .app/dmg (macOS).
- Plugin repository / discovery.

---

## Phase 7 - Steam Deck / Proton hardening, then VFS
- Robust Proton prefix mapping (`compatdata/<appid>/pfx`), Deck ergonomics,
  launch-option integration.
- **Later:** VFS/overlay deployment (Linux OverlayFS/FUSE; USVFS-like on Windows)
  as an alternative to link/copy - isolated, no game-dir pollution.

---

## Critical path
`Phase 1 (deploy engine)` → `Phase 2 (nxm end-to-end)` are the two that prove the
whole thesis. If those land solidly, everything after is additive and low-risk.
