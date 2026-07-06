<!-- SPDX-License-Identifier: GPL-2.0-only -->
# ModManager - Architecture

A multi-platform, open-source, Vortex-style mod manager written in Rust, with a
GUI, a TUI, and a CLI over one engine. Automated `nxm://` download→install,
any-game via plugins, Steam/Proton first-class.

Ethos: suckless-in-spirit - minimal, composable, hackable, no telemetry, small
dependency surface, single static-ish binaries, everything the GUI does the CLI
can do - plus safety-critical discipline (**Power of Ten for Rust**, §9.3) on the
engine that touches users' files.

---

## 1. Goals & non-goals

**Goals**
- Linux, Windows, macOS. Consistent look and behavior across all three.
- One engine, three faces: GUI + TUI + CLI.
- Seamless `nxm://` handling: click "Download with Manager" (or our extension's
  button) → resolved, downloaded, installed to the right game, automatically.
- Any game via community plugins; Steam + Proton as first-class citizens.
- Fast, snappy, low-bloat. Not a webview, not Electron, not TypeScript.

**Non-goals for v1 (designed-for, not built yet)**
- VFS / overlay deployment (OverlayFS/FUSE/USVFS). Link/copy first.
- Sites other than Nexus Mods (the client is abstracted, but only Nexus ships).
- Cloud sync, mobile, telemetry.

---

## 2. Locked decisions

| Area | Choice | Rationale |
|---|---|---|
| Language | **Rust** | Safety where it matters most: the file-deployment engine touches users' game installs. Cargo + the crate ecosystem make 3 frontends cheap. |
| GUI | **Iced** (MIT, pure-Rust) | GPLv2-compatible; one renderer → identical on all 3 OSes; tiny self-contained binary; themed toward a clean macOS-like look. |
| GUI - ruled out | Slint | Was the first pick, but its free build is **GPLv3-only**, incompatible with our GPLv2 goal (see §11). Only `modman-gui` links a toolkit, so this stayed a cheap swap. |
| TUI | **ratatui** | Mature, the standard. |
| CLI | **clap** (derive) | Standard, scriptable. |
| Plugins | **Two-tier: `game.toml` + `game.lua` (`mlua`, vendored Lua 5.4)** | Most games are just data; Lua only when logic is needed. |
| Deployment | **Link → symlink → copy, transactional manifest** | Cross-platform, reversible, ships sooner. VFS deferred. |
| Async/net | **tokio + reqwest** (rustls) | Concurrent, resumable downloads; Nexus API. |
| Storage | **SQLite via `rusqlite`** | mods × profiles × files × conflicts is relational + transactional. |
| Reliability | **Power of Ten for Rust** | forbid `unsafe`, panic-free, bounded loops, lint-enforced in CI - the deploy engine is safety-critical (§9.3). |
| Steam | **`steamlocate`** | Parses `libraryfolders.vdf` / `appmanifest_*.acf`. |
| Paths | **`directories`** | XDG / Known Folders / Application Support. |
| Errors | **`thiserror`** (libs) / **`anyhow`** (binary edges) | Typed errors in the engine; ergonomic context at the top. |
| License | **GPL-2.0-only** | Free/open-source; preference of v2 over v3. It's what drives the GUI choice (see §11). |

---

## 3. Workspace layout

A single Cargo workspace. **All logic in `modman-core`; frontends are thin.**
Dependency direction is a hard rule: **`modman-core` depends on no UI, no
site-specific, and no protocol crate; frontends depend on core; no cycles.**

```
modman/  (cargo workspace)
├── modman-core        # engine: games, profiles, mod store, deploy, conflicts,
│                      #   manifest, transactions. ZERO UI/network-UI deps.
│                      #   Owns the declarative `game.toml` loader (pure data, no
│                      #   code exec) so the engine can add games without linking
│                      #   the Lua host.
├── modman-plugin      # mlua host + the sandboxed `modman` API given to Lua.
│                      #   Also: FOMOD installer. (The `game.toml` loader lives in
│                      #   core; this crate adds the Lua tier on top of it.)
├── modman-nexus       # Nexus API client + nxm:// link resolver (behind a
│                      #   `ModSource` trait, so other sites slot in later).
├── modman-ipc         # single-instance guard + loopback listener. The one
│                      #   ingress for both the OS protocol handler and the
│                      #   browser extension.
├── modman-protocol    # tiny binary the OS launches for nxm://; forwards the
│                      #   URL to the running instance (or starts a headless one).
├── modman-cli         # thin: clap over core. The scriptable surface.
├── modman-tui         # thin: ratatui over core.
├── modman-gui         # thin: Iced over core. The only crate that links a GUI toolkit.
└── extension/         # userscript (v1) + WebExtension (later). JS - unavoidable
                       #   in a browser.
```

**Licensing:** the project ships **GPL-2.0-only** (free/open-source; preference of
v2 over v3). Every dependency is MIT/BSD/LGPL-2.1 or dual-licensed with an MIT arm,
all GPLv2-compatible, enforced by `cargo-deny` in CI. See §11 for why the GUI is
Iced and not Slint.

---

## 4. Core engine (`modman-core`)

The correctness-critical heart. Everything else is presentation or I/O. Built to
the Power of Ten reliability standard (§9.3): panic-free, `unsafe`-free, bounded.

### 4.1 Domain model
- **Game**: a resolved install: plugin id, name, install path, store (Steam/…),
  Steam AppID, mod-staging root, deploy target(s).
- **Profile**: a named, switchable set of enabled mods + load order for a game.
- **Mod**: a staged archive's extracted contents in the central store, plus
  provenance (Nexus mod/file id, version, source).
- **Deployment manifest**: the exact record of every file we placed into the
  game, how (hardlink/symlink/copy), and what original (if any) we displaced.

### 4.2 Deployment algorithm (link/copy, transactional)
Split into a **pure planner** (no I/O, trivially testable) and a **transactional
applier**.
1. **Resolve** the virtual file tree from enabled mods in load order
   (`target_path → (mod, source_path)`; later mod wins conflicts). Conflicts are
   surfaced, not silently resolved.
2. **Diff** the resolved tree against the current manifest → adds / removes / no-op.
3. **Back up** any pre-existing, non-ours game file we're about to overwrite into
   a store, recorded in the manifest.
4. **Apply adds** with fallback: **hardlink** (same filesystem) → **symlink** →
   **copy**. Record link type + source hash per file.
5. **Apply removes**: delete a deployed file only if it still matches the manifest
   (hash/link check - never clobber a file the user changed); restore any backup.
6. **Commit** the manifest transactionally (temp file + atomic rename) and keep a
   small **journal** so an interrupted deploy is recoverable on next launch.

**Undeploy / profile switch** is the reverse walk over the manifest. A `--dry-run`
and a verify pass are first-class. The five engine invariants (reversibility,
idempotence, no-silent-clobber, crash-safety, determinism) are specified and
tested in `crates/modrix-core/src/deploy/apply_tests.rs`.

### 4.3 Storage (SQLite sketch)
```sql
games(id, plugin_id, name, install_path, store, steam_appid, staging_root, ...)
profiles(id, game_id, name, is_active)
mods(id, game_id, name, version, source, nexus_mod_id, nexus_file_id,
     archive_path, staged_path, install_state)
profile_mods(profile_id, mod_id, enabled, load_order)      -- ordering lives here
deployed_files(id, profile_id, mod_id, target_path, source_path,
               link_type, source_hash, backup_path, deployed_at)  -- the manifest
downloads(id, source, url, nxm_uri, state, bytes_total, bytes_done, ...)
```
Game/plugin definitions and per-game config stay as plain files (see §5); SQLite
(WAL mode) holds the relational index and the manifest.

---

## 5. Plugin system (`modman-plugin`)

**Two tiers so nobody writes code they don't need to.**

### 5.1 Tier 1 - declarative `game.toml`
Covers the ~80% case. No code. Example shape:
```toml
api_version = 1
id          = "skyrimse"
name        = "Skyrim Special Edition"
steam_appid = 489830
install_probe = ["steam", "registry", "path-hint"]
mod_root      = "Data"                 # relative to install path
deploy        = "link"                 # link | copy
load_order    = "plugins_txt"          # named strategy provided by core
```

### 5.2 Tier 2 - `game.lua` (only when logic is required)
For custom installers, conditional deploy, special load-order formats, etc.
Loaded via **`mlua`** (vendored **Lua 5.4** - no system Lua dependency).

**Sandbox:** plugins get a curated `modman` table and **no raw `io`/`os`/`debug`/
`require`**. Every filesystem effect goes through the core's transactional layer
(a plugin returns an install/stage plan; it never writes files directly). Each
plugin call gets a step/time budget so a bad plugin can't hang or loop forever.
```lua
-- callbacks a plugin may implement:
function detect(ctx)      ... end   -- return install path or nil
function mod_root(ctx)    ... end   -- where files deploy, relative to install
function install(archive, ctx) ... end  -- drive a custom/FOMOD-like flow
function load_order(mods, ctx) ... end  -- return ordered list / write order file

-- API surface exposed to plugins (all mediated):
modman.game            -- paths, appid, store, profile
modman.fs.stage(src, dest)      -- register a file for deployment (goes to manifest)
modman.fs.exists / read_dir / read_text
modman.http.get(url)            -- rate-limited, through the client
modman.log.info / warn / error
modman.install.choose(step)     -- present a wizard step to the active frontend
```
**API versioning:** every plugin declares `api_version`; the host refuses or
shims mismatches. Plugins live in a discovered directory (user data dir +
bundled), one folder per game (`<id>/game.toml`, optional `<id>/game.lua`, assets).

### 5.3 FOMOD
`ModuleConfig.xml` / `info.xml` parsing (via `quick-xml`) lives in core so all
three frontends can drive the same install wizard; Lua plugins can override for
bespoke installers. This is table-stakes for compatibility with existing mods.
Bound recursion over the step/condition tree (§9.3).

---

## 6. Download pipeline (the "seamless" part)

Two ingress paths, unified at one loopback listener.

### 6.1 `nxm://` - the primary path (no extension needed)
We register as the OS handler for the `nxm` scheme:
- **Linux:** a `.desktop` with `MimeType=x-scheme-handler/nxm;` +
  `xdg-mime default modman.desktop x-scheme-handler/nxm`.
- **Windows:** `HKCU\Software\Classes\nxm` with `URL Protocol` + shell open command.
- **macOS:** `CFBundleURLTypes` in `Info.plist` (scheme `nxm`) /
  `LSSetDefaultHandlerForURLScheme`.

Nexus's "Download with Manager" fires:
```
nxm://<game_domain>/mods/<mod_id>/files/<file_id>?key=<k>&expires=<ts>&user_id=<uid>
```
`modman-protocol` forwards it to the running instance, which calls:
```
GET https://api.nexusmods.com/v1/games/{game}/mods/{mod}/files/{file}/download_link.json
    ?key={k}&expires={ts}          # key/expires required for free users; premium may omit
    header: apikey: <user_api_key>
```
→ resolve CDN URL → download (resumable, progress, checksum-verified) → hand to the
game plugin for install. Respect Nexus **rate limits** (`X-RL-*` headers); cache
metadata; back off on 429.

### 6.2 Extension / userscript - enhanced path
Adds our own button that `POST`s mod info to `http://127.0.0.1:<port>` (the
loopback listener). This is how we support **auto-handling** and **future non-Nexus
sites**. The auto-slow-download userscript composes here: for free users it can
auto-trigger the flow so the countdown isn't babysat. v1 ships a userscript (zero
install friction); a WebExtension can follow.

### 6.3 IPC seam (`modman-ipc`)
The loopback HTTP listener **is** the single-instance mechanism: whoever binds the
port is primary; a second launch (or `modman-protocol`) detects the bound port and
forwards its request instead of starting a duplicate. If nothing is running, a
headless engine starts to service the download, so browser clicks work even with
the GUI closed. Loopback-only + a per-session token; never bind non-localhost.

---

## 7. Frontends (thin, over core)

All three call the same **action layer** in `modman-core` (add mod, enable,
reorder, deploy, switch profile, resolve download, run installer). No business
logic in a frontend.

- **CLI (`clap`)**: the scriptable surface; built first because it proves the
  engine headless. Everything is reachable here.
- **TUI (`ratatui`)**: mod list, load-order reorder, conflict view, download queue.
- **GUI (`Iced`, MIT)**: the same, themed classy/macOS-like; the install wizard
  (FOMOD) renders here.

Because frontends are thin, the GUI toolkit is a localized, swappable choice - which
is exactly why the GPLv2 requirement (§11) only cost us a one-crate change.

---

## 8. Platform integration

- **Steam:** `steamlocate` finds Steam + parses `libraryfolders.vdf` and
  `appmanifest_<appid>.acf` for install dirs.
- **Proton (Linux):** games run under Proton have a prefix at
  `steamapps/compatdata/<appid>/pfx/`. Deploy must target the right paths and, for
  some games, into the prefix (e.g. `users/steamuser/Documents`). **This is where
  we can beat Vortex**, which barely handles Linux - Steam Deck is a headline
  use case. Isolate all Proton path-mapping in one module.
- **Config/data dirs:** `directories` (XDG on Linux, Known Folders on Windows,
  Application Support on macOS).
- **Archives:** `zip` + `sevenz-rust` (pure-Rust 7z) for the common cases. **RAR:**
  the `unrar` license has a field-of-use restriction that is **not GPL-compatible**
  (Debian classes it non-free), so we don't bundle it - RAR support comes via
  **libarchive** (BSD, GPL-compatible) or as an optional/external-tool feature.
  Most Nexus mods are zip/7z anyway.

---

## 9. Security, safety & reliability

### 9.1 Security
- The loopback listener is **localhost-only + token-authed**; the browser can't
  reach a user's engine without the token.
- Lua plugins are **sandboxed** (no raw `io`/`os`/`debug`/`require`; fs/http
  mediated and budgeted).
- **No telemetry.**

### 9.2 File-operation safety
- File operations are **transactional, hash-checked, backed-up, and dry-runnable** -
  we never overwrite a user-modified file silently, and every deploy is reversible
  and crash-recoverable via the manifest + journal.

### 9.3 Reliability standard - Power of Ten for Rust
The whole codebase follows the **Power of Ten** discipline (safety-critical Rust,
adapted from NASA/JPL). This is an **architectural commitment**, not a style
preference, because the deploy engine manipulates users' game files - a bug there
loses saves and installs. It is mechanized so it holds from commit #1:

- **`#![forbid(unsafe_code)]` workspace-wide.** FFI (`mlua`, libarchive) stays
  behind vetted safe wrappers. No raw pointers, no memory-unsafety surface.
- **Panic-freedom in library code.** No `.unwrap()` / `panic!` / `todo!` / `v[i]`
  on fallible paths; propagate with `?`/`Result`. The only justified panic is
  `.expect("… why it cannot fail")` on a real invariant.
- **Bounded loops, no recursion over untrusted input** (mod trees, archives, FOMOD
  XML, load orders can be adversarial). Tree walks become explicit worklists with a
  depth/count cap; every loop has a visible ceiling.
- **Explicit arithmetic.** `overflow-checks = true` in release; `checked_*` /
  `saturating_*` on anything derived from file sizes or counts.
- **Validate at every trust boundary** (disk, network, env, plugin input) for
  semantics (ranges, invariants), not just shape.
- **Short functions (~60 lines), immutable by default** (minimize `let mut`).
- **Enforced mechanically:** pedantic clippy + cherry-picked `restriction` lints,
  `-D warnings`, `cargo-deny` (incl. the GPLv2 license gate), cross-OS CI. The exact
  `[workspace.lints]` / `clippy.toml` / `deny.toml` live at the
  repository root and are wired in Phase 0.

---

## 10. Key risks (ranked)

1. **Deployment engine correctness** - the make-or-break; ~60% of the real work.
   Rust + transactional manifest + the Power of Ten discipline + invariant tests +
   dry-run mitigate it.
2. **Nexus free-user flow / rate limits** - depends on the `nxm` key/expires
   handshake and the API budget; cache aggressively, degrade gracefully.
3. **FOMOD coverage** - many real mods need it; parser fidelity matters.
4. **Proton path mapping** - high-value, fiddly; isolate in a Steam/Proton module.

---

## 11. Licensing - why GPL-2.0, and why that picks the GUI

**GPL-2.0-only**, because you prefer v2 over v3 and want the manager free and open
source. Both are FSF/OSI free-software licenses; the difference is v3's added terms
(anti-tivoization, explicit patent grant, anti-DRM). Preferring v2 is a legitimate
choice - it's the Linux-kernel license.

**The one hard constraint:** GPLv2 and GPLv3 are *mutually incompatible* - you cannot
legally combine GPLv2-only code with GPLv3 code and redistribute it. That part is
genuinely unavoidable. It bit us only because **Slint's free build is GPLv3-only**, so
choosing Slint would force the whole app to v3. That was the *sole* thing pushing v3.

**The fix is just the GUI.** Slint → **Iced** (MIT). MIT is GPLv2-compatible, so the app
is GPL-2.0 cleanly. We give up Slint's free "Cupertino" theme and instead theme Iced
toward the macOS-like look - a little more styling, no blocker. Iced is also arguably
*more* suckless: pure Rust, one language, no DSL, no external framework.

**Rest of the tree is clean:** tokio, reqwest, rusqlite, ratatui, clap, mlua, serde,
directories, steamlocate, zip, sevenz-rust are all MIT or dual-licensed with an MIT
arm → GPLv2-compatible. Rule: take the MIT arm on dual-licensed crates, and avoid any
**Apache-2.0-only** dependency (Apache-2.0 is incompatible with GPLv2). `unrar` is
excluded for a related reason (§8). All of this is a mechanical `cargo-deny` gate.

**v2-only vs "v2 or later":** we ship **GPL-2.0-only**, which most faithfully honors
"v2 over v3." "GPL-2.0-or-later" would be the flexible alternative, but it lets v3 back
in as a possibility - so we don't use it.

**Escape hatch:** if you later decide Slint's free macOS look is worth more than v2, the
alternative is GPLv3 + Slint. It's a one-crate change.
