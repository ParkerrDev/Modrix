# AGENTS.md / CLAUDE.md

This file provides guidance to coding agents working in this repository - Claude Code
(claude.ai/code) and any tool that reads `AGENTS.md`.

`AGENTS.md` is a symlink to `CLAUDE.md`, so the two can never drift: they are one file
under two names. **Edit `CLAUDE.md`** - it is the real file, and some tools (including
Claude Code's own editor) refuse to write through a symlink. Do not replace the symlink
with a second copy.

## Project

Modrix: a GPL-2.0-only, cross-platform (Linux/Windows/macOS) mod manager in Rust - an open-source Vortex replacement with GUI + TUI + CLI over one engine. Downloads come from a browser-extension hand-off (`extension/`), **not** the Nexus API. Design docs: `docs/ARCHITECTURE.md` (decisions, algorithms), `docs/ROADMAP.md` (phases).

## Commands

```sh
cargo build
cargo test --all --all-features                          # what CI runs
cargo test -p modrix-core                                # one crate
cargo test -p modrix-core deploy::apply_tests            # one module's tests
cargo clippy --all-targets --all-features -- -D warnings # zero warnings required
cargo fmt --all -- --check
cargo deny check advisories bans sources                 # advisories gate (licenses are not checked)
cargo run -p modrix-gui                                  # run the GUI (Rune, native GPU UI)
cargo run -p modrix-gui -- --snap out.png --scene mods    # render one demo scene headless
cargo run -p modrix-cli -- <args>                        # binary is `modrix`
cargo run -p modrix-cli -- --json <args>                 # machine envelope for agents
cargo run -p modrix-cli -- mcp                           # MCP server over stdio
MODRIX_REGISTRY=<dir-or-url> …                           # override the plugin registry
makensis -DVERSION=<x.y.z> -DBINDIR=target/release \
  installer/windows/modrix.nsi                           # build the Windows installer
```

Releases are cut by pushing a `v*` tag (or running the **Release** workflow with a
version): `.github/workflows/release.yml` stamps the version, builds each OS, packages
the NSIS installer, and publishes a GitHub Release - which the in-app updater checks.

Toolchain is pinned in `rust-toolchain.toml` (rustup installs it automatically). CI (`.github/workflows/ci.yml`) runs fmt + clippy `-D warnings` + tests on all three OSes, plus `cargo deny check advisories bans sources`.

## Hard rules

**Power of Ten for Rust** - enforced by workspace lints in `Cargo.toml` + `clippy.toml`, not optional. The deploy engine touches users' game files, so:
- `unsafe` is forbidden workspace-wide; `.unwrap()`, `panic!`, `todo!`, `unimplemented!`, and `v[i]` indexing are **denied** in production code (use `?`, `.get(i)`); `.expect("why it can't fail")` only for real invariants.
- Functions ≤60 lines (clippy `too-many-lines`). Bounded loops; no recursion over untrusted input (archives, FOMOD XML) - use explicit worklists with depth/count caps.
- Arithmetic on file sizes/counts uses `checked_*`/`saturating_*`; `overflow-checks = true` even in release.
- Tests are exempt from unwrap/panic/indexing (see `clippy.toml`), but not from arithmetic discipline.
- The `power-of-ten-rust` skill covers this discipline in depth.

**Licensing** - the project is GPL-2.0-only and every source file starts with `// SPDX-License-Identifier: GPL-2.0-only` (or the comment style of its format). Dependency licenses are **not** gated: `deny.toml` checks advisories, bans and sources only. There is no rule against Apache-2.0 crates or against `reqwest`; the current hyper + rustls HTTP stack is kept because it works, not because anything forbids the alternative (see `docs/ARCHITECTURE.md §11`).

**Dependency direction:** `modrix-core` depends on no UI, site-specific, or protocol crate. Frontends are thin - all business logic lives in core; a frontend only presents core's action layer.

## Architecture

Cargo workspace, `crates/*`:

- **`modrix-core`**: the correctness-critical engine: domain model (`model.rs`), SQLite storage (`db.rs`, migrations in `migrations/*.sql`, rusqlite WAL), mod store (`store.rs`), engine facade (`engine.rs` - the action layer all frontends call), declarative `game.toml` loader (`gamedef.rs`), health checks (`health.rs`), conflict rules (`rules.rs`), progress reporting (`progress.rs`).
  - **`deploy/`** is the heart: a **pure planner** (`plan.rs`, no I/O - resolves the virtual file tree from enabled mods in load order, later mod wins, conflicts surfaced) and a **transactional applier** (`apply.rs` - hardlink → symlink → copy fallback, backups of pre-existing files, hash-checked removals that never clobber user-modified files, journal for crash recovery, manifest committed via temp-file + atomic rename). Undeploy is the reverse walk. Five tested invariants: reversibility, idempotence, no-silent-clobber, crash-safety, determinism (`apply_tests.rs`).
- **`modrix-download`**: segmented, resumable, checksum-verified download engine (clean-room aria2-style) fed by the browser extension; `.mmdl` control files, `.part` staging, FIFO queue, event stream.
- **`modrix-ipc`**: loopback HTTP listener + per-session token; binding the port **is** the single-instance guard. `POST /download` receives the extension's `HandoffJob` (URL + cookies + UA); never binds non-localhost.
- **`modrix-service`**: the embedded hand-off service (engine + downloads + IPC listener) that every frontend hosts, so browser clicks work whichever frontend runs.
- **`modrix-protocol`**: dormant `nxm://` OS handler; identity extraction only, not a download path.
- **`modrix-plugin`**: Lua (mlua, vendored 5.4) plugin host + FOMOD installer. Lua is sandboxed: no raw `io`/`os`/`require`; plugins return stage plans (validated in `core::logic`), never write files directly; per-call instruction/time/memory budgets.
- **`modrix-registry`**: community plugin registry client (curated `ParkerrDev/modrix-plugins` repo): index fetch, sha256-verified atomic install into `<data>/plugins/<id>` (where `core::defcat` looks), uninstall/gc. `MODRIX_REGISTRY` env overrides the source (local clone while the repo is private).
- **`modrix-il2cpp`**: reader for Unity's IL2CPP `global-metadata.dat` (types, methods, fields, assemblies). Pure safe Rust, no FFI - it only reads a file - which is why it sits in this workspace under the same lints as the engine. It is the first stage of generating managed interop assemblies so an IL2CPP game (Sons of the Forest) can be modded at all; see `docs/SONS-LOADER.md` for the full design and what remains.
- **`modrix-ecma335`**: a from-scratch ECMA-335 / PE32+ writer that emits .NET *reference* assemblies (types, fields, method signatures, no bodies). Pure safe Rust, no dependencies beyond `thiserror`. Validated by re-parsing its own output, by synthetic assemblies that straddle the coded-index width threshold, and by compiling real C# against it with the actual Roslyn `csc`.
- **`modrix-interop-gen`**: turns an IL2CPP game into compilable reference assemblies - partitions the flat type world into per-assembly models and encodes them via `modrix-ecma335`. This is what makes Sons of the Forest buildable with **no third-party loader**: `modrix interop generate` reads `GameAssembly.dll` + `global-metadata.dat` from the install and writes `_Modrix/interop/`. The game's embedded corlib is redirected onto the real `System.Runtime` rather than emitted, so `System.Object` does not exist twice.
- **`modrix-mcp`**: MCP server (hand-rolled stdio JSON-RPC, serde_json only) exposing the full engine surface as tools + installed skill files as resources; run via `modrix mcp`.
- **`modrix-update`**: in-app updater over GitHub Releases, reusing `modrix-download`'s hyper+rustls client. `check` compares the latest release tag (semver) to `CARGO_PKG_VERSION`; on Windows `download`+`apply` fetch and silently run the NSIS installer, then the GUI quits so it can self-replace. Best-effort: offline / private-repo / up-to-date all resolve to "no update". Non-Windows platforms get a manual-download link.
- **`modrix-cli`** (`modrix`), **`modrix-tui`** (ratatui), **`modrix-gui`** (Rune, a native GPU UI with no web view; screens in `src/ui/*.rs`, engine I/O on its own thread in `src/bus.rs`, demo scenes in `src/demo.rs` for `--snap out.png --scene <name>` renders) - thin frontends. `modrix-gui` is the only crate allowed to link a GUI toolkit.

  **GUI rules (Parker's, kept from the web build and binding on any toolkit):** the window is frameless with **no window controls at all** (Hyprland manages windows by keybind); the top bar and sidebar header are drag regions, nothing more. **No live blur on an always-visible layer**: no `backdrop_blur` on panels, sidebar or top bar - each one copies and blurs the whole 2560x1600 target per frame, which made the iGPU lag. Soft art is baked once on the CPU (`art.rs`), and the whole backdrop (wash, glow, hero, fade) is flattened into one image by `bake.rs`: every full-window layer costs the iGPU several ms a frame. No image or component is ever squished or letterboxed (cover-fit, anchored top), the sidebar cover always fills the full sidebar width, and images are never hidden during a resize. The layout is fluid: no fixed widths that squash when Hyprland tiles the window small. The MODRIX wordmark stays on one line; the notification bell has exactly one indicator (a count badge). All dropdowns and right-click menus are the app's own. The view only rebuilds when state it shows changed (`poll` returns `true` only then), so an idle window draws nothing.

Game definitions are two-tier: declarative `games/<id>/game.toml` (parsed in core; `api_version = 2` carries capabilities - load-order strategy, content dirs, base files, external scans, health checks, store-detection ids/`registry_keys`, `required_files`, `mod_base` (deploy into the install dir or the user profile) - so core has NO game-specific logic) covers most games; `game.lua` only when logic is required. Frontends gate features on `Engine::capabilities(game)`. `api_version = 3` adds three more, all `#[serde(default)]` so every existing definition still parses: `[launch]` (how to start the game, its log path, and the environment it needs - `env` is a `BTreeMap` so rendered launch options are byte-stable), `[mod_loader]` (an injected mod loader Modrix can detect, version and conflict-check; named `mod_loader` rather than `loader` because `loader` is already a nested key under `[health]` and the misplaced-key guard requires top-level names never be valid inside a table), and `park_root_binaries` (an explicit override for the "is a top-level DLL a loader or the mod payload?" heuristic - Sons of the Forest must set it `false` or every mod deploys to the install root and silently does nothing). **86 games, ported from Vortex, ship built-in** - embedded from `games/<id>/` by `crates/modrix-core/build.rs` (drop in a `game.toml` and it ships, no code change), spanning Bethesda/Gamebryo (Data/ + Plugins.txt + script-extender health), Unity (BepInEx/UMM), and many drop-into-a-folder titles; Skyrim SE and Subnautica are the reference defs. Install detection is Steam (cross-platform, VDF parsing) plus GOG/Epic/Xbox/Origin/Uplay and generic vendor `registry_keys` (Windows-only, via the `winreg` crate - target-gated since `unsafe` is forbidden). Reaching Vortex's 250+ is via the community registry as more defs are authored; the other Vortex games' logic lives on Nexus servers, not in-repo.

## Conventions

- Errors: `thiserror` typed errors in library crates (`error.rs`), `anyhow` only at binary edges.
- Core never prints; frontends own their output sinks (clippy denies `print_stdout`/`print_stderr`).
- Schema changes are new numbered files in `crates/modrix-core/migrations/` - never edit an existing migration.
- No decorative Unicode glyphs in UI text.
