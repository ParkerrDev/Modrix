<!-- SPDX-License-Identifier: GPL-2.0-only -->
<h1 align="center">
  <img alt="Modrix logo" src="branding/dist/modrix-wordmark-transparent.svg" />
   <br>
  MOD MANAGER
</h1>

<img width="2502" height="1481" alt="image" src="https://github.com/user-attachments/assets/1b86a463-84be-45f3-966d-677f7f5fc6ea" />

Modrix is a fast, native mod manager for Linux, Windows, and macOS - an open-source replacement for [Vortex](https://www.nexusmods.com/about/vortex/) with a GUI, a TUI, and a CLI over one shared engine. Downloads come from your own browser through a zero-config extension, so there is no site API and no API key. Any game is supported through community plugins, and Steam plus Proton are first-class. The deploy engine never edits your game folder in place: it hardlinks mod files in, backs up anything it overwrites, journals every step for crash recovery, and reverses cleanly on undeploy. It will not silently clobber a file you changed by hand.

```bash
# Windows: download and run the installer (per-user, no admin, auto-updating)
https://github.com/ParkerrDev/Modrix/releases/latest

# Any platform: build and install from source
cargo install --git https://github.com/ParkerrDev/Modrix --locked modrix-gui modrix-cli
```

> Cross-platform (Linux, Windows, macOS). GPL-2.0-only, zero telemetry, no Nexus API key.
> 86 games ship built-in (ported from Vortex) - Bethesda/Gamebryo, Unity (BepInEx/UMM), and many more. Further games install from the community registry.
> Windows builds update themselves through GitHub Releases.

---

<details>
<summary><b>How it works</b></summary>

<br>

Modrix is one engine (`modrix-core`) with three thin frontends (GUI, TUI, CLI) over it. Every frontend embeds the same hand-off service, so a browser download works whichever one you have open. The parts that matter:

### Browser hand-off, not a site API

Modrix never talks to the Nexus API and never asks for a key. Instead, a small browser extension intercepts the download you click and hands the URL plus your session cookies to the local engine over a loopback HTTP listener. The engine downloads the file itself, segmented and resumable, and stages it into your library.

```
Click "Download" on nexusmods.com
        |
        v
  browser extension  ---- POST /download ---->  modrix-ipc (127.0.0.1:<port>)
   (URL + cookies + UA)                                |
                                                       v
                                              modrix-download
                                       segmented, resumable, checksum-verified
                                                       |
                                                       v
                                              modrix-core store
                                         archive staged, normalized, ready
```

The loopback port is also the single-instance guard: binding it is what makes one Modrix the primary. The extension needs no token (browser-extension origins are trusted automatically); a rotating token is only a fallback for other clients.

### The deploy engine

Deployment is split in two so the risky half is a pure function and the destructive half is transactional.

- **Planner (`deploy/plan.rs`)** does no I/O. It resolves the virtual file tree from your enabled mods in load order (later mod wins), and surfaces every conflict.
- **Applier (`deploy/apply.rs`)** carries the plan out. It places each file by hardlink, falling back to symlink, then copy. It backs up any pre-existing game file before overwriting it, writes a journal before touching anything so a crash is recoverable, and commits the manifest with a temp-file-plus-atomic-rename. Undeploy is the reverse walk, and removals are hash-checked so a file you edited after deploy is never deleted out from under you.

Five invariants are enforced by tests: reversibility, idempotence, no-silent-clobber, crash-safety, and determinism.

### Games are plugins

Modrix has no game-specific logic baked into the engine. A game is a declarative `game.toml` (`api_version = 2`) that describes its capabilities: the load-order strategy, content directories, base files, external scans, and health checks. Frontends light up features based on `Engine::capabilities(game)`, so Subnautica (which has no plugin load order) simply hides the Load Order tab. Games that need real logic can add a sandboxed `game.lua`; most do not.

86 games ported from Vortex ship built-in - embedded at build time from `games/<id>/game.toml` (plus a `game.lua` for the few that need logic), so dropping in a new definition ships it with no code change. Further games install from the community plugin registry, verified by sha256.

</details>

---

<details>
<summary><b>Installation</b></summary>

<br>

### Windows installer (recommended on Windows)

Download `Modrix-Setup-<version>.exe` from the [latest release](https://github.com/ParkerrDev/Modrix/releases/latest) and run it. The installer is per-user: it installs to `%LOCALAPPDATA%\Programs\Modrix`, needs no administrator rights, adds a Start Menu shortcut and an uninstaller, and lets Modrix update itself in place from then on.

### Linux

Download `Modrix-linux-x86_64.tar.gz` from the [latest release](https://github.com/ParkerrDev/Modrix/releases/latest) and extract the `modrix-gui`, `modrix`, and `modrix-tui` binaries onto your `PATH`, or build from source (below). Linux and macOS do not auto-update in-app; pull a newer release or rebuild.

### Build and install from source (any platform)

Builds and installs both the GUI (`modrix-gui`) and the CLI/service (`modrix`). This is the only path on macOS, which has no prebuilt binary.

```bash
cargo install --git https://github.com/ParkerrDev/Modrix --locked modrix-gui modrix-cli
```

### Build from a clone

```bash
# Prerequisites: the toolchain pinned in rust-toolchain.toml (rustup installs it)
git clone https://github.com/ParkerrDev/Modrix
cd Modrix
cargo build --release
cargo install --path crates/modrix-gui --locked    # modrix-gui
cargo install --path crates/modrix-cli --locked    # modrix
```

### Browser extension

The extension makes "Download" on Nexus hand off to Modrix. It needs no token or pairing while Modrix is running.

1. Open your browser's extensions page in developer mode.
2. Load the `extension/` folder unpacked.
3. That is all. Click Download on nexusmods.com with Modrix (GUI, TUI, or `modrix serve`) open.

### Uninstall

```bash
# Windows: use the entry in Add/Remove Programs, or the Start Menu uninstaller.

# Source install:
cargo uninstall modrix-gui modrix-cli
rm -rf ~/.local/share/modrix        # library + database (your staged mods)
rm -rf ~/.config/modrix             # config
rm -rf ~/.cache/modrix              # artwork + download staging
```

### Requirements

| Requirement | Notes |
|---|---|
| Linux, Windows, or macOS | Steam plus Proton is first-class on Linux |
| Rust (pinned toolchain) | Build-time only; `rustup` installs it from `rust-toolchain.toml` |
| A GPU or software renderer | The GUI uses `wgpu` (falls back to `tiny-skia`) |

</details>

---

<details>
<summary><b>Quick start</b></summary>

<br>

1. Launch `modrix-gui`. On **Games**, register your game. If it is installed through Steam, Modrix usually detects it and offers a one-click "Add and switch"; otherwise point it at the install directory.
2. Load the `extension/` folder unpacked in your browser (see Installation). No token needed.
3. Click **Download** on nexusmods.com. The file downloads segmented, stages itself into the library, and appears under **Mods**.
4. Enable the mod, resolve any conflicts under **Conflicts**, then hit **Deploy**. Use **Verify** to confirm the game folder matches the manifest, and **Undeploy** to reverse it.

The GUI embeds the same service as `modrix serve`, so hand-offs work whichever is running. Only one instance holds the loopback port at a time.

</details>

---

<details>
<summary><b>CLI reference</b></summary>

<br>

The binary is `modrix`. Everything the GUI can do is scriptable, and `--json` wraps any command in a stable machine envelope for agents and scripts.

### Global flags

| Flag | Description |
|---|---|
| `--json` | Emit a machine-readable envelope: `{"ok":true,"data":...}` on success, `{"ok":false,"error":...}` on stderr with a non-zero exit |
| `--help` / `--version` | Standard clap help and version |

### Games and profiles

| Command | Description |
|---|---|
| `modrix game list` | List registered games |
| `modrix game detect` | Probe Steam (and, on Windows, GOG/Epic/Xbox/Origin/Uplay) for installed, supported games |
| `modrix game add <def> <path>` | Register a game from a `game.toml` (or catalog id) at an install path |
| `modrix game active` / `set-active` | Show or change the active game |
| `modrix game capabilities` | Show what the active game's plugin enables |
| `modrix profile list` / `create <name>` / `switch <name>` | Manage mod profiles |

### Mods and deployment

| Command | Description |
|---|---|
| `modrix mod add <archive>` | Stage a mod archive into the library |
| `modrix mod list` | List installed mods in load order |
| `modrix mod enable <id>` / `disable <id>` | Toggle a mod |
| `modrix mod remove <id>` / `reinstall <id>` | Remove or reinstall a mod |
| `modrix mod conflicts` | Show file conflicts between enabled mods |
| `modrix mod rule ...` / `override ...` | Resolve conflicts by rule or per-file override |
| `modrix mod hash <archive>` | Print an archive's sha256 (duplicate-install guard) |
| `modrix deploy` / `undeploy` / `verify` | Apply, reverse, or check the deployment |
| `modrix loadorder` | Show or edit the mod load order |

### Plugins (`.esp` load order), health, external

| Command | Description |
|---|---|
| `modrix plugins list` / `order` / `auto-sort` / `sync` | Bethesda plugin (`.esp`) load order |
| `modrix health` | Report issues and deploy blockers |
| `modrix external` | List unmanaged content already in the game folder |

### Downloads, FOMOD, registry, service

| Command | Description |
|---|---|
| `modrix downloads list` / `status` / `cancel` / `add <url>` | Drive the live download queue |
| `modrix fomod show <mod>` / `apply <mod> --choices <json>` | Inspect or script a FOMOD installer |
| `modrix registry search` / `info` / `install` / `uninstall` / `update` / `list` | The community game-support registry |
| `modrix plugin validate <dir>` | Validate a plugin (used by registry CI) |
| `modrix serve` | Run the hand-off service headless |
| `modrix mcp` | Run the MCP server over stdio (agent integration) |
| `modrix paths` | Print the data, config, and cache directories |

</details>

---

<details>
<summary><b>Games and plugins</b></summary>

<br>

### Built-in

**86 games**, ported from Vortex and embedded at build time (`games/<id>/game.toml`, plus a `game.lua` for the few that need dynamic logic). By family:

| Family | Examples | Handling |
|---|---|---|
| Bethesda / Gamebryo | Skyrim SE, Fallout 4, Oblivion, Morrowind, Starfield | `Data/` deploy, Plugins.txt load order, script-extender (SKSE/F4SE/…) health, `.esp/.esm/.esl` sorting |
| Unity (BepInEx / UMM) | Subnautica, Bloodstained, Oxygen Not Included | mods deploy into the loader's plugin root; no load-order file |
| Everything else | Grim Dawn, Cyberpunk 2077, The Witcher 3, Stardew Valley | drop-into-a-folder mod roots, per-game content and external-scan rules |

Install detection covers Steam (cross-platform) plus GOG, Epic, Xbox, Origin and Uplay on Windows (registry / manifest reads, no store APIs). Reaching Vortex's full 250+ catalog is the community registry's job as more definitions are authored.

### GameDef v2 capabilities

A `games/<id>/game.toml` with `api_version = 2` declares everything the engine needs, so core carries no per-game code:

| Capability | What it controls |
|---|---|
| `load_order` | Strategy (e.g. `plugins_txt`), the appdata directory, plugin extensions, vanilla plugins |
| `content_dirs` | Which top-level directories in an archive are real mod content |
| `base_files` | Vanilla files the game ships, so they are never flagged as leftovers |
| `external_scan` | Ecosystems (SKSE, BepInEx) to scan for unmanaged content |
| `health` | Loader and recommended-mod checks that only run when declared |
| store ids + `registry_keys` | Detect installs from Steam / GOG / Epic / Xbox / Origin / Uplay and vendor registry keys |
| `required_files` | Files that must exist for a detected or entered directory to be accepted as this game |
| `mod_base` | Whether `mod_root` is anchored to the install dir (default) or the user profile - Documents / AppData, inside the Proton prefix on Linux (The Sims, Baldur's Gate 3, Factorio, …) |

Add your own definitions at `<config>/games/<id>/game.toml`. When logic is required (rare), a sandboxed `game.lua` beside it returns a stage plan that core validates and applies. Plugins never write files directly.

### The community registry

`modrix registry search` and the GUI's **Games** screen list game-support plugins from the curated [`ParkerrDev/modrix-plugins`](https://github.com/ParkerrDev/modrix-plugins) repository. Installing one fetches the plugin, verifies each file by sha256, and unpacks it atomically into `<data>/plugins/<id>`. `MODRIX_REGISTRY=<dir-or-url>` overrides the source (a local clone, for example).

</details>

---

<details>
<summary><b>Configuration and data locations</b></summary>

<br>

Modrix follows the platform's standard directories (via the `directories` crate). Run `modrix paths` to print the exact locations.

| Directory | Linux default | Holds |
|---|---|---|
| Data | `~/.local/share/modrix` | SQLite database, staged mod library, installed plugins |
| Config | `~/.config/modrix` | GUI theme (`gui.toml`), extra game definitions |
| Cache | `~/.cache/modrix` | Steam artwork bakes, download staging, update downloads |

On Windows these map to `%LOCALAPPDATA%` and `%APPDATA%`; on macOS to `~/Library`.

- **Extra games:** drop a `game.toml` at `<config>/games/<id>/game.toml`.
- **GUI theme:** the theme picker in **Settings** persists to `<config>/gui.toml` (Aurora glass by default, Gold as an alternative).
- **Registry source:** `MODRIX_REGISTRY` overrides where game-support plugins are fetched from.

</details>

---

<details>
<summary><b>Architecture</b></summary>

<br>

Modrix is a Cargo workspace. `modrix-core` depends on no UI, site-specific, or protocol crate; the frontends are thin and only present core's action layer.

### Workspace

| Crate | Role |
|---|---|
| `modrix-core` | The engine: domain model, transactional deploy, SQLite storage, `game.toml` loader, health, conflict rules. No UI deps. |
| `modrix-download` | Segmented, resumable, checksum-verified download engine, fed by the extension hand-off. |
| `modrix-ipc` | Loopback listener plus single-instance guard; the port binding is the guard. |
| `modrix-service` | The embedded hand-off service every frontend hosts (engine plus downloads plus listener). |
| `modrix-protocol` | Dormant `nxm://` OS handler; identity extraction only. |
| `modrix-plugin` | Sandboxed Lua (mlua) plugin host plus the FOMOD installer. |
| `modrix-registry` | Community plugin registry client: index fetch, sha256-verified install, gc. |
| `modrix-mcp` | MCP server exposing the engine surface as tools plus skill files as resources. |
| `modrix-update` | In-app updater over GitHub Releases (reuses the hyper plus rustls client). |
| `modrix-cli` | The `clap` frontend (binary: `modrix`). |
| `modrix-tui` | The `ratatui` frontend. |
| `modrix-gui` | The `iced` frontend (the only crate allowed to link a GUI toolkit). |

### Deploy data flow

```
enabled mods (load order)
        |
        v
  deploy/plan.rs         pure, no I/O
        |  resolve virtual file tree, later mod wins, surface conflicts
        v
  DeployPlan
        |
        v
  deploy/apply.rs        transactional
        |  write journal BEFORE any change
        |  place each file: hardlink -> symlink -> copy
        |  back up pre-existing game files
        |  commit manifest via temp-file + atomic rename
        v
  game folder            manifest recorded; undeploy is the reverse walk
```

### Safety invariants (enforced by tests)

1. **Reversibility.** Deploy then undeploy restores the pristine game tree.
2. **Idempotence.** A redeploy with no changes is a no-op.
3. **No silent clobber.** A file you edited after deploy is backed up, never overwritten or deleted blindly.
4. **Crash-safety.** The journal is written before any file moves, so an interrupted deploy is recovered on next launch.
5. **Determinism.** The planner is a pure function of the enabled set and load order.

### Licensing gate

The whole tree is GPL-2.0-only, enforced mechanically by `cargo-deny` in CI. Apache-2.0-only dependencies are rejected as GPLv2-incompatible, which is why the HTTP stack is `hyper` plus `rustls` plus `rustls-rustcrypto` plus `rustls-native-certs` and never `reqwest` (which pulls `ring`). The sole carve-out is a documented [GUI linking exception](docs/LICENSE-EXCEPTIONS.md) for the windowing and text crates the Iced GUI needs; the engine links zero Apache-2.0 code.

### Power of Ten for Rust

Because the deploy engine touches users' game files, the codebase follows a Rust adaptation of the Power of Ten, enforced by workspace lints (not convention):

- `unsafe` is forbidden workspace-wide; `.unwrap()`, `panic!`, `todo!`, and `v[i]` indexing are denied in production code.
- Functions stay under 60 lines; loops are bounded; no recursion over untrusted input (archives, FOMOD XML).
- Arithmetic on file sizes and counts uses `checked_*` / `saturating_*`, with overflow checks on even in release.

See [`docs/ARCHITECTURE.md`](docs/ARCHITECTURE.md) for the decisions, algorithms, and the full invariant list.

</details>

---

<details>
<summary><b>Auto-updates (Windows)</b></summary>

<br>

Windows builds keep themselves current through GitHub Releases, with no telemetry and no background service.

- On launch, Modrix asks GitHub for the latest release and compares its version (semver) to the running build. This is best-effort: offline, rate-limited, or up-to-date all resolve quietly to "no update" and never delay startup.
- When a newer release exists, a notification appears and **Settings** grows an **Updates** card with the version, its notes, and a one-click **Download and install**.
- Installing downloads the signed installer, launches it silently, and quits Modrix so the installer can replace the running executable and relaunch it. Because the install is per-user, no administrator prompt is involved.

The updater reuses the same GPLv2-clean `hyper` plus `rustls` HTTP client as the rest of Modrix (never `reqwest`). Pre-releases (betas) are skipped, so a beta never auto-updates a stable install. On Linux and macOS the same check links you to the release page for a manual download.

</details>

---

<details>
<summary><b>Building and contributing</b></summary>

<br>

The hard requirement, enforced in CI on Linux, Windows, and macOS: the workspace must build clean, tests must pass, and `clippy -D warnings` plus `cargo-deny` must be green.

### Dev setup

```bash
git clone https://github.com/ParkerrDev/Modrix
cd Modrix
cargo build
cargo test --all --all-features                            # what CI runs
cargo clippy --all-targets --all-features -- -D warnings   # zero warnings required
cargo fmt --all -- --check
cargo deny check                                           # license + advisory gate
```

Run the GUI with `cargo run -p modrix-gui`, the CLI with `cargo run -p modrix-cli -- <args>`.

### Adding a game

Most games need only a declarative definition. Drop a `game.toml` at `<config>/games/<id>/game.toml` with `api_version = 2` and the capability blocks described above, then register it (`modrix game add`). To share it, open a PR to [`ParkerrDev/modrix-plugins`](https://github.com/ParkerrDev/modrix-plugins); its CI validates the plugin and regenerates the index.

### Conventions

- **Core stays UI-free.** All business logic lives in `modrix-core`; a frontend only presents the action layer. `modrix-core` depends on no UI or site-specific crate.
- **Power of Ten.** No `unsafe`, no `.unwrap()` / `panic!` / indexing in production code, functions under 60 lines, bounded loops, checked arithmetic. The `clippy -D warnings` gate enforces this.
- **Typed errors.** `thiserror` in library crates, `anyhow` only at binary edges. Core never prints; frontends own their output.
- **Migrations are append-only.** Schema changes are new numbered files in `crates/modrix-core/migrations/`; never edit an existing migration.
- **No decorative Unicode glyphs in UI text.**

### Making a pull request

1. Fork the repository and branch: `feat/short-description` or `fix/short-description`.
2. Keep commits focused, one logical change each.
3. Run the full gate before pushing: `cargo fmt --all -- --check`, `cargo clippy --all-targets --all-features -- -D warnings`, `cargo test --all --all-features`, `cargo deny check`.
4. Open a PR against `main` describing what it changes, how you tested it, and any edge cases considered. For deploy-engine changes, note which invariant your test protects.

</details>

---

<details>
<summary><b>License</b></summary>

<br>

Modrix is released under the **GNU General Public License v2.0 only** (`GPL-2.0-only`).

You are free to use, modify, and distribute it under the terms of the GPLv2. Every dependency is GPLv2-compatible, verified by `cargo-deny` in CI. The one documented carve-out is a [GUI linking exception](docs/LICENSE-EXCEPTIONS.md) for the Apache-2.0 windowing and text crates the Iced GUI links; the engine itself links zero Apache-2.0 code.

See [`LICENSE`](LICENSE) for the full text.

</details>
