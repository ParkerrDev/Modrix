<!-- SPDX-License-Identifier: GPL-2.0-only -->
# ModManager

A fast, native, cross-platform (Linux, Windows, macOS) mod manager in Rust - an
open-source [Vortex](https://www.nexusmods.com/about/vortex/) replacement with a
**GUI, a TUI, and a CLI** over one engine. It handles `nxm://` "Download with
Manager" links automatically, supports any game via community plugins, and treats
Steam + Proton as first-class.

- **License:** GPL-2.0-only. Every dependency is GPLv2-compatible, enforced by
  `cargo-deny` in CI.
- **Reliability:** the codebase follows the Power of Ten for Rust (forbid
  `unsafe`, panic-free, bounded, lint-enforced) because the deploy engine touches
  users' game files. See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) §9.3.
- **No telemetry, ever.**

## Documentation

- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) - decisions, crate design, algorithms.
- [`docs/ROADMAP.md`](docs/ROADMAP.md) - phased build plan.
- [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) - the build brief.

## Workspace

| Crate | Role |
|---|---|
| `modman-core` | Engine: domain, deployment, manifest, storage. No UI deps. |
| `modman-plugin` | Lua (mlua) plugin host + `game.toml` loader + FOMOD. |
| `modman-nexus` | Nexus Mods `ModSource`: API client + `nxm://` resolver. |
| `modman-ipc` | Single-instance guard + loopback listener. |
| `modman-protocol` | Tiny `nxm://` OS handler that forwards to the running instance. |
| `modman-cli` | `clap` frontend (binary: `modman`). |
| `modman-tui` | `ratatui` frontend. |
| `modman-gui` | `iced` frontend. |

## Build

```sh
cargo build
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
cargo deny check
```

Requires the pinned toolchain in `rust-toolchain.toml` (installed automatically by
`rustup`).
