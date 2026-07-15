<!-- SPDX-License-Identifier: GPL-2.0-only -->
# Modrix

A fast, native, cross-platform (Linux, Windows, macOS) mod manager in Rust - an
open-source [Vortex](https://www.nexusmods.com/about/vortex/) replacement with a
**GUI, a TUI, and a CLI** over one engine. It is a **download manager** with a
**browser extension** that hands your own browser downloads to the local engine
(no site API, no API key), supports any game via community plugins, and treats
Steam + Proton as first-class.

- **License:** GPL-2.0-only. Every dependency is GPLv2-compatible, enforced by
  `cargo-deny` in CI. Sole carve-out: a documented [GUI linking
  exception](docs/LICENSE-EXCEPTIONS.md) for the ten Apache-2.0 windowing/text
  crates the Iced GUI needs (`winit` et al.) - the engine links zero
  Apache-2.0 code.
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
| `modrix-core` | Engine: domain, deployment, manifest, storage. No UI deps. |
| `modrix-plugin` | Lua (mlua) plugin host + `game.toml` loader + FOMOD. |
| `modrix-download` | Segmented, resumable download engine (aria2/Motrix-style); fed by the browser extension's hand-off, not a site API. |
| `modrix-ipc` | Single-instance guard + loopback listener. |
| `modrix-protocol` | Tiny `nxm://` OS handler that forwards to the running instance. |
| `modrix-cli` | `clap` frontend (binary: `modrix`). |
| `modrix-service` | The embedded hand-off service every frontend hosts (engine + downloads + loopback listener). |
| `modrix-tui` | `ratatui` frontend. |
| `modrix-gui` | `iced` frontend. |

## Quick start (GUI)

```sh
cargo install --path crates/modrix-cli --locked    # `modrix`
cargo install --path crates/modrix-gui --locked    # `modrix-gui`
modrix-gui
```

1. **Games** → register your game (Skyrim SE ships built-in; point it at the
   install directory).
2. Load `extension/` unpacked in your browser, open its options, and paste the
   address + token from the GUI's **Settings** screen.
3. Click **Download** on nexusmods.com. The file downloads segmented, stages
   itself into the library, and appears under **Mods** - enable it and hit
   **Deploy**.

The GUI embeds the same `modrix-service` as `modrix serve`, so hand-offs work
whichever one is running (only one holds the port at a time).

## Build

```sh
cargo build
cargo test --all
cargo clippy --all-targets --all-features -- -D warnings
cargo deny check
```

Requires the pinned toolchain in `rust-toolchain.toml` (installed automatically by
`rustup`).
