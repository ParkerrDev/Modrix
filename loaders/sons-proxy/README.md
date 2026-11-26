<!-- SPDX-License-Identifier: GPL-2.0-only -->

# `modrix-sons-proxy`: the Sons of the Forest injection proxy

A Windows `version.dll` that loads before Sons of the Forest starts, forwards
every real `version.dll` export so the game is unaffected, waits for the IL2CPP
runtime, hosts CoreCLR, and calls one managed method.

**This is milestone M0 and only M0.** Its entire purpose is to answer one
question with evidence: *can managed .NET code run inside this IL2CPP game,
under Proton, without breaking it?* There is no mod loading here. See
[`../../docs/SONS-LOADER.md`](../../docs/SONS-LOADER.md) for where it goes next.

---

## The unsafe policy

The main Modrix workspace declares `unsafe_code = "forbid"` because the deploy
engine rewrites users' game directories, and a memory-safety bug there costs
someone their saves. An in-process injector cannot exist without `unsafe`.

**This crate is therefore not in that workspace.** It is a separate Cargo
workspace under `loaders/`, which the root manifest's `members = ["crates/*"]`
never sees. `cargo build`, `cargo clippy` and `cargo test` at the repository root
are completely unaffected: that is verified, not assumed.

In exchange for being allowed `unsafe`, the rules here are *stricter* than the
main workspace in the ways that matter:

| Rule | Where |
| --- | --- |
| `clippy::undocumented_unsafe_blocks` = **deny** | every `unsafe` block carries a `// SAFETY:` comment justifying it |
| `clippy::multiple_unsafe_ops_per_block` = **deny** | one unsafe operation per block, so no comment covers two things |
| All FFI confined to `src/win.rs` | one file to audit for pointer mistakes |
| `panic = "abort"` in every profile | unwinding across the FFI boundary into Unity's stack is UB; making it impossible is cheaper than being careful |
| `unwrap_used`, `panic`, `todo`, `unimplemented`, `indexing_slicing` = **deny** | identical to the main workspace |
| `missing_docs` = **deny**, `# Errors` on every fallible fn | identical to the main workspace |
| Functions ≤ 60 lines, ≤ 5 arguments | identical to the main workspace (`clippy.toml`) |

Everything that *can* be safe Rust is safe Rust. `layout`, `log`, `error` and the
export name table have no FFI at all, compile on Linux, and carry the unit tests,
which is why `cargo test` is meaningful on a machine that cannot run the
artefact.

### Where the `unsafe` actually is

1. **`src/exports.rs`**: seventeen `#[unsafe(naked)]` forwarders.
2. **`src/win.rs`**: `LoadLibraryW`, `GetProcAddress`, `GetModuleHandleW`,
   `GetModuleFileNameW`, `GetSystemDirectoryW`, `SetEnvironmentVariableW`,
   `GetLastError`, and the `ntdll!wine_get_version` probe.
3. **`src/coreclr.rs`**: two hosting calls and the managed delegate call.
4. **`src/il2cpp.rs`**: one call to `il2cpp_domain_get`.
5. **`src/lib.rs`**: `DllMain` itself.

That is the whole list.

---

## How the export forwarding works, and why not the other two ways

Each of the seventeen exports is a naked function that tail-jumps through a
pointer table filled in at `DLL_PROCESS_ATTACH`:

```asm
mov  rax, qword ptr [rip + SLOTS + N*8]
test rax, rax
je   .null            ; real DLL lacked the export: return 0 (= failure)
jmp  rax              ; tail jump - arguments and return value untouched
```

`RAX` is scratch and is not an argument register in the Microsoft x64 calling
convention, and `RSP` is never touched, so register *and* stack arguments arrive
intact and the real function returns straight to the game.

- **Doorstop** emits `void *exp_X(void) { return __X__(); }` and depends on the
  compiler tail-calling at `-Os`. That is correct by luck; `VerInstallFileW`
  takes seven arguments and would be corrupted by any prologue.
- **MelonLoader** rewrites its own stubs at runtime with
  `VirtualProtect(PAGE_EXECUTE_READWRITE)` + `mov r11, addr; jmp r11`. Correct,
  but it leaves our `.text` writable-and-executable for the life of the process.

`injection.md` names the naked `jmp` thunk "the only strictly correct
implementation". It also needs no RWX memory, so it is what this crate does. This
is a deliberate, documented deviation from decision **D6** in `PLAN.md`, which
chose MelonLoader's stub patching; both are unconditionally correct, and this one
is smaller and does not make our own code writable.

The seventeen names come verbatim from Doorstop's
`src/windows/proxy/proxylist.txt`; guessing the set gets a missing-import dialog
on launch instead of a game. `build-windows.sh` asserts all seventeen are in the
produced PE, and a unit test asserts each thunk owns its own table slot.

---

## Building

Requires the pinned toolchain (`rust-toolchain.toml` handles it) and a Windows
target. Cross-compiling from Linux needs `mingw-w64`:

```sh
rustup target add x86_64-pc-windows-gnu    # done automatically by rust-toolchain.toml
./build-windows.sh                         # -> dist/version.dll
./build-windows.sh --managed               # + dist/_Modrix/net6/Modrix.Loader.dll
./build-windows.sh --managed --coreclr <mini-coreclr>/win-x64   # + the runtime
```

`dist/` is a staging tree. **The script never writes outside this directory.**

To build by hand:

```sh
cargo build --release --target x86_64-pc-windows-gnu
cp target/x86_64-pc-windows-gnu/release/modrix_sons_proxy.dll version.dll
```

The file name is what matters: the Windows loader keys on it, not on the PE
export-directory name. Doorstop ships the identical binary as `winhttp.dll` and
RedLoader renames it to `version.dll`.

`x86_64-pc-windows-msvc` also works (`MODRIX_PROXY_TARGET=x86_64-pc-windows-msvc`)
if the MSVC toolchain is available; nothing in the source is GNU-specific.

### Checks

```sh
cargo fmt --all
cargo clippy --target x86_64-pc-windows-gnu --all-targets --all-features -- -D warnings
cargo clippy --all-targets --all-features -- -D warnings   # host, for the pure modules
cargo test                                                 # host
```

---

## The runtime this needs

BepInEx `mini-coreclr` 6.0.7, `win-x64`:

<https://github.com/BepInEx/dotnet-runtime/releases/download/6.0.7/mini-coreclr-Release.zip>

187 files including `coreclr.dll`, `clrjit.dll`, `System.Private.CoreLib.dll` and
a `.version` stamp reading `6.0.7`. It deliberately contains **no `hostfxr.dll`**,
which is exactly why this crate uses `coreclr_initialize` /
`coreclr_create_delegate` directly rather than `nethost`/`hostfxr`, and why no
`runtimeconfig.json` is involved. Verified: that `coreclr.dll` exports nine
symbols, among them both hosting entry points.

It is **not vendored into this repository** (67 MB staged). Download it, unzip
it, and point `--coreclr` at the `win-x64` folder.

---

## Installing: read this before touching a game directory

> **The proxy slot is single-occupancy.** A game has exactly one `version.dll`.
> MelonLoader and RedLoader both claim it; BepInEx 6 IL2CPP claims `winhttp.dll`.
> Whichever loader loses the race never runs, and the user gets a game with half
> its mods missing and no error anywhere. On the reference machine
> **MelonLoader currently owns `version.dll`** (10,810,880 bytes, PE export name
> `MelonLoader.Bootstrap.dll`). Move it aside; do not overwrite it.

Nothing in this repository installs itself. `Engine::install_loader` is
deliberately unimplemented: M0 is the payload that will make it worth writing.

```sh
SOTF="$HOME/.local/share/Steam/steamapps/common/Sons Of The Forest"

# 1. Park the incumbent loader. Reversible; keep the backup.
mv "$SOTF/version.dll" "$SOTF/version.melonloader.bak"

# 2. Install the M0 payload.
cp    dist/version.dll        "$SOTF/version.dll"
mkdir -p                      "$SOTF/_Modrix/net6" "$SOTF/_Modrix/dotnet"
cp    dist/_Modrix/net6/Modrix.Loader.dll "$SOTF/_Modrix/net6/"
cp -r dist/_Modrix/dotnet/.   "$SOTF/_Modrix/dotnet/"
```

Resulting layout:

```
Sons Of The Forest/
  SonsOfTheForest.exe
  version.dll                      <- this crate
  version.melonloader.bak          <- the incumbent, parked
  _Modrix/
    Latest.log                     <- created on first launch
    net6/Modrix.Loader.dll
    dotnet/coreclr.dll, .version, System.Private.CoreLib.dll, ... (187 files)
```

### Proton: the one setting without which nothing happens

Wine prefers its own builtin `version.dll` and the failure is **completely
silent**: the game launches perfectly and nothing injects. In Steam →
Sons of the Forest → Properties → Launch Options:

```
WINEDLLOVERRIDES="version=n,b" %command%
```

`n` = prefer the native (our) DLL, `b` = fall back to Wine's builtin.
`games/sonsoftheforest/game.toml` already declares this under `[launch.env]`, and
Modrix surfaces it as a launch-readiness warning, but Steam owns the game
process, so only the user's launch options can actually carry it.

---

## The M0 acceptance test

Single, binary, on the real install.

1. Install as above, with the launch option set.
2. Launch the game from Steam and reach the main menu.
3. `cat "$SOTF/_Modrix/Latest.log"`

**Pass** requires all four milestone lines, in this order:

```
[modrix] proxy attached, real version.dll = C:\windows\system32\version.dll
[modrix] il2cpp runtime ready (poll trigger), domain = 0x00007ff...   (non-zero)
[modrix] coreclr up, runtime = 6.0.7
[modrix] managed entrypoint reached: Modrix.Loader 0.1.0
```

…**and the game must still play normally.** Start a new game, move around, quit.
A loader that produces the log and a broken game is a failed M0.

Additional lines are expected and are not failures: `host = wine <version>`, the
`env MODRIX_* = …` echo from the managed side, `managed runtime = …`.

> **Note on line 2.** `PLAN.md` §3 writes this line as
> `il2cpp_init intercepted`. That wording belongs to **M0b**, which IAT-hooks
> `kernel32!GetProcAddress` in `UnityPlayer.dll` and returns its own
> `il2cpp_init`. M0 uses the lower-risk poll trigger `PLAN.md` itself prescribes
> ("*implemented first as the lower-risk path, with the IAT hook as M0b*"), so
> the line says what actually happened. Polling proves the runtime is **up**;
> interception would additionally prove we are **ahead of it**.

### Reading a failure

| Symptom | Meaning |
| --- | --- |
| `_Modrix/Latest.log` does not exist | The proxy never loaded. Nine times out of ten this is a missing `WINEDLLOVERRIDES`. |
| Only line 1 | IL2CPP never came up within 120 s, or `GameAssembly.dll` is not loaded under this name. |
| `ERROR bootstrap failed: coreclr not found at …` | The `dotnet/` copy is missing or incomplete. |
| `ERROR bootstrap failed: coreclr_initialize returned 0x…` | CoreCLR itself refused to start: the `host = wine …` line above it is the first thing to report. |
| `coreclr_create_delegate returned 0x80070002` | `Modrix.Loader.dll` is not on `APP_PATHS`; check `_Modrix/net6/`. |
| `_Modrix/crash_<timestamp>.log` exists | Managed code ran and threw. The file holds the full exception. |
| Game closes instantly with no log at all | Revert: restore `version.melonloader.bak` and report it. |

### Reverting

```sh
rm -f  "$SOTF/version.dll"
mv     "$SOTF/version.melonloader.bak" "$SOTF/version.dll"
rm -rf "$SOTF/_Modrix"
```

The game is untouched otherwise: nothing here writes to `SonsOfTheForest_Data`,
`GameAssembly.dll`, `boot.config` or any save.

---

## Known risks in M0 (none of them retired yet)

1. **Does CoreCLR initialize under this Proton build?** Unknown. RedLoader warns
   that Wine below 7.16 lacks `RtlRestoreContext` for 32-bit processes; this is
   64-bit, so the warning does not apply directly, but nothing has been measured.
   The `host = wine <version>` line exists to make the answer reportable.
2. **UCRT under Wine.** The produced DLL imports `api-ms-win-crt-*` (Rust's
   `windows-gnu` targets link the UCRT, not `msvcrt`). Proton ships
   `ucrtbase.dll`, so this should resolve, but if the proxy fails to load at all
   with no log, this is the second thing to check after `WINEDLLOVERRIDES`.
3. **`std::thread::spawn` from `DllMain`.** The spawn happens under the loader
   lock; the thread does not run until the lock is released, which is the
   documented, widely used pattern. Nothing heavier than `CreateThread` runs
   inside `DllMain`; CoreCLR is deliberately loaded on the spawned thread.
4. **Case sensitivity.** RedLoader's own config and shell script disagree
   (`Redloader.dll` vs `RedLoader.dll`) and Wine's case folding hides it. Every
   path here is exact-case and nothing depends on folding.
