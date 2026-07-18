# CLAUDE.md

This file provides guidance to Claude Code (claude.ai/code) when working with code in this repository.

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
cargo deny check                                         # license + advisory gate
cargo run -p modrix-gui                                  # run the GUI
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

Toolchain is pinned in `rust-toolchain.toml` (rustup installs it automatically). CI (`.github/workflows/ci.yml`) runs fmt + clippy `-D warnings` + tests on all three OSes, plus `cargo deny check licenses advisories bans sources`.

## Hard rules

**Power of Ten for Rust** - enforced by workspace lints in `Cargo.toml` + `clippy.toml`, not optional. The deploy engine touches users' game files, so:
- `unsafe` is forbidden workspace-wide; `.unwrap()`, `panic!`, `todo!`, `unimplemented!`, and `v[i]` indexing are **denied** in production code (use `?`, `.get(i)`); `.expect("why it can't fail")` only for real invariants.
- Functions ≤60 lines (clippy `too-many-lines`). Bounded loops; no recursion over untrusted input (archives, FOMOD XML) - use explicit worklists with depth/count caps.
- Arithmetic on file sizes/counts uses `checked_*`/`saturating_*`; `overflow-checks = true` even in release.
- Tests are exempt from unwrap/panic/indexing (see `clippy.toml`), but not from arithmetic discipline.
- The `power-of-ten-rust` skill covers this discipline in depth.

**GPL-2.0-only licensing** - mechanically gated by `deny.toml`. Apache-2.0-only dependencies are rejected (incompatible with GPLv2); dual `MIT OR Apache-2.0` crates pass via the MIT arm. This is why the HTTP stack is hyper + rustls + `rustls-rustcrypto` + `rustls-native-certs` and **never reqwest** (pulls `ring`/`sync_wrapper`). Every source file starts with `// SPDX-License-Identifier: GPL-2.0-only` (or the comment style of its format).

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
- **`modrix-mcp`**: MCP server (hand-rolled stdio JSON-RPC, serde_json only) exposing the full engine surface as tools + installed skill files as resources; run via `modrix mcp`.
- **`modrix-update`**: in-app updater over GitHub Releases, reusing `modrix-download`'s hyper+rustls client (never reqwest/ring). `check` compares the latest release tag (semver) to `CARGO_PKG_VERSION`; on Windows `download`+`apply` fetch and silently run the NSIS installer, then the GUI quits so it can self-replace. Best-effort: offline / private-repo / up-to-date all resolve to "no update". Non-Windows platforms get a manual-download link.
- **`modrix-cli`** (`modrix`), **`modrix-tui`** (ratatui), **`modrix-gui`** (Iced 0.13; screens in `src/view/*.rs`) - thin frontends. `modrix-gui` is the only crate allowed to link a GUI toolkit.

Game definitions are two-tier: declarative `games/<id>/game.toml` (parsed in core; `api_version = 2` carries capabilities - load-order strategy, content dirs, base files, external scans, health checks, store-detection ids/`registry_keys`, `required_files`, `mod_base` (deploy into the install dir or the user profile) - so core has NO game-specific logic) covers most games; `game.lua` only when logic is required. Frontends gate features on `Engine::capabilities(game)`. **86 games, ported from Vortex, ship built-in** - embedded from `games/<id>/` by `crates/modrix-core/build.rs` (drop in a `game.toml` and it ships, no code change), spanning Bethesda/Gamebryo (Data/ + Plugins.txt + script-extender health), Unity (BepInEx/UMM), and many drop-into-a-folder titles; Skyrim SE and Subnautica are the reference defs. Install detection is Steam (cross-platform, VDF parsing) plus GOG/Epic/Xbox/Origin/Uplay and generic vendor `registry_keys` (Windows-only, via the `winreg` crate - target-gated since `unsafe` is forbidden). Reaching Vortex's 250+ is via the community registry as more defs are authored; the other Vortex games' logic lives on Nexus servers, not in-repo.

## Conventions

- Errors: `thiserror` typed errors in library crates (`error.rs`), `anyhow` only at binary edges.
- Core never prints; frontends own their output sinks (clippy denies `print_stdout`/`print_stderr`).
- Schema changes are new numbered files in `crates/modrix-core/migrations/` - never edit an existing migration.
- No decorative Unicode glyphs in UI text.
