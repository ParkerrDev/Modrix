# The native Sons of the Forest loader

Status: **mods compile against the game with no third-party loader; the
injection chain is built but has never been run.** This document records the design, what is verified, and what remains, so
the next person picking it up does not have to re-derive any of it.

Milestone M0 (proxy → CoreCLR → managed entrypoint) exists in
[`loaders/`](../loaders/README.md) and builds. It has **never been executed**:
see [Milestone M0](#milestone-m0--proving-injection) for exactly what that means
and for the acceptance procedure someone with the game installed has to run.

## Why Modrix needs a loader at all

Every other game Modrix supports is modded by *placing files*. Sons of the
Forest is not. It is an **IL2CPP** build: Unity compiled its C# ahead of time to
native code, so the shipped game is `GameAssembly.dll` (100 MB of machine code)
plus `global-metadata.dat` (26 MB of names and layouts, no method bodies). There
is no `Assembly-CSharp.dll`. A mod cannot reference the game, and there is
nothing for Harmony to patch.

Making such a game moddable requires four things that a file-copying mod manager
does not have:

1. **Injection**: get our code running inside the game process.
2. **A managed runtime**: host .NET 6 in that process.
3. **Interop assemblies**: generate one managed proxy DLL per original
   assembly, so mods have something to compile against and patch.
4. **A mod host**: discover `Mods/*.dll`, resolve order, call lifecycle hooks.

Third-party loaders (RedLoader, MelonLoader, BepInEx 6 IL2CPP) each do all four.
Modrix doing it natively means users need one tool, not two, and Modrix can
manage the loader's lifecycle instead of treating it as an opaque prerequisite.

## Verified facts

Measured on a real install, not inferred:

| Fact | Value |
| --- | --- |
| Steam AppID | `1326470` |
| Unity | `2022.2.16f1` |
| Metadata version | **29** (`sanity = 0xFAB11BAF`) |
| `global-metadata.dat` | 26,505,936 bytes |
| Assemblies / images | 210 each (they must match: this is the layout check) |
| Type definitions | 24,056 |
| Method definitions | 182,725 |
| Field definitions | 124,512 |
| Proxy DLL slot | `version.dll` (shared with MelonLoader and RedLoader) |

Record sizes for v29, derived by finding which candidate size divides each
section's byte length exactly, then cross-checked against the
one-image-per-assembly invariant:

| Record | Bytes |
| --- | --- |
| `Il2CppImageDefinition` | 40 |
| `Il2CppAssemblyDefinition` | 64 |
| `Il2CppTypeDefinition` | 88 |
| `Il2CppMethodDefinition` | 32 |
| `Il2CppFieldDefinition` | 12 |

`crates/modrix-il2cpp` re-checks divisibility on every file it opens, so a game
update that changes the layout fails loudly instead of being misread.

## Architecture

The split is chosen so that **everything that can be safe Rust is safe Rust**,
and the unavoidable `unsafe` is quarantined in one crate outside the main
workspace. Modrix forbids `unsafe` workspace-wide because the deploy engine
touches users' game files; a runtime injector is a different risk class and does
not get to erode that guarantee.

```
                          OUT OF PROCESS  (safe Rust, main workspace)
  global-metadata.dat ──> modrix-il2cpp ──────> metadata model     [DONE]
                                   │
                                   v
                          modrix-interop-gen ──> managed .dll set  [DONE]
                          + modrix-ecma335       (197 ref assemblies)
  ────────────────────────────────-│────────────────────────────────────────
                                   v  IN PROCESS  (unsafe, separate workspace)
                          version.dll  (Rust cdylib proxy)          [M0 BUILT]
                                   │  forwards the 17 real version.dll exports,
                                   │  waits for il2cpp, then hosts CoreCLR via
                                   │  coreclr_initialize + coreclr_create_delegate
                                   v
                          Modrix.Loader.dll  (C#)                   [M0 BUILT]
                                   │  M0: proves managed code ran.
                                   │  M2: discovers Mods/*.dll, reads
                                   │  manifest.json, calls lifecycle hooks
                                   v
                          Modrix.Sons.Sdk.dll  (C#)                 [TODO]
                                      the API mods are written against
```

### Why CoreCLR directly and not hostfxr

Two routes exist and the reference loaders take one each. MelonLoader uses
`nethost` → `hostfxr_initialize_for_runtime_config` →
`hostfxr_get_runtime_delegate`, which resolves frameworks properly and can use a
system .NET, but needs `hostfxr.dll`, a `*.runtimeconfig.json`, and a correct
`DOTNET_ROOT` inside a Proton prefix. Unity Doorstop (and therefore RedLoader)
resolves exactly two symbols from a vendored `coreclr.dll` and calls them.

Modrix takes Doorstop's route. It is two symbols, no config file, no framework
resolution and no `DOTNET_ROOT`, and it is the route already proven to work in
this game under this Proton prefix. It is also the only route the runtime we ship
supports: BepInEx's `mini-coreclr` 6.0.7 `win-x64` contains `coreclr.dll`,
`clrjit.dll` and `System.Private.CoreLib.dll` but deliberately **no
`hostfxr.dll`**: verified against the real archive, whose `coreclr.dll` exports
nine symbols including both hosting entry points.

### Why the generator is Rust but the SDK is C#

The generator's *output* must be .NET, but the generator itself is just a binary
reader and an ECMA-335 writer: both plainly expressible in safe Rust, and doing
it in Rust means interop generation can run from the CLI with no .NET installed.
The mod host and SDK cannot be anything but C#, because mods are C#.

### Component status

| Component | Crate / project | Status |
| --- | --- | --- |
| Metadata reader | `crates/modrix-il2cpp` | **Done.** Parses the real 26 MB file; 10 tests, two against the live install. |
| Loader lifecycle | `crates/modrix-core/src/launch.rs` | **Done.** Status, conflict detection, launch, log tail. |
| Game definition | `games/sonsoftheforest/game.toml` | **Done.** `api_version = 3`. |
| Interop generator | `crates/modrix-interop-gen` + `crates/modrix-ecma335` | **Done.** Emits 197 reference assemblies (39 MB) from the reference game; a real mod project compiles against them. `modrix interop generate` drives it. |
| Proxy DLL | `loaders/sons-proxy` (separate workspace) | **M0 built.** Cross-compiles to a Windows PE with all 17 exports; 18 host-side tests. Never executed. |
| CoreCLR host | same crate, `src/coreclr.rs` | **M0 built.** Compiles; the hosting calls have never run. |
| Mod host + SDK | `loaders/sons-managed/Modrix.Loader` | **M0 built.** `dotnet build` against `net6.0` is green; entry point contract verified in the emitted metadata. Discovery/SDK not started. |

`install_loader` is deliberately **not implemented yet**: it would have nothing
to install. `loader_status` already reports the truth (not installed, interop
not generated), which is more useful than an action that half-works.


## Interop generation: what actually works today

`modrix interop generate --game sonsoftheforest` reads the two files already in
the game directory and writes 197 managed reference assemblies into
`_Modrix/interop/`. Nothing is downloaded, and no third-party loader has to be
installed. Verified end to end on the reference machine: a C# project using
`LocalPlayer`, `GameObject`, `Vector3`, `VailActor` and the `LocalPlayer.IsInWorld`
*property* compiles clean against the output.

Three decisions inside it are worth knowing before changing anything:

**Only the public surface is emitted.** A reference assembly exists so a
compiler can bind names, and private members are unbindable by definition.

**The game's own corlib is redirected, not emitted.** IL2CPP embeds `mscorlib`,
`System` and friends. Emitting them would make `System.Object` exist twice and
every use ambiguous (`CS0433`). Il2CppInterop solves this by renaming the tree to
`Il2CppSystem.*`; Modrix instead rewrites references to a curated allowlist of
BCL types onto the real `System.Runtime`, so `string` means `string`. A corlib
type not on the allowlist is declined and the member using it is dropped -
deliberately, because a dangling `TypeRef` fails the whole build with a
diagnostic that points nowhere, while a missing member costs one API.

**A member whose signature cannot be expressed is skipped, never approximated.**
Emitting a method with a subtly wrong signature is worse than omitting it: the
compiler binds to it and fails somewhere confusing.

### Known gaps in the generated output

- **Interfaces reached through a constructed generic type** are skipped; they
  would need a `TypeSpec` token.
- **Method generic arity is inferred** from the highest `!!N` the signature
  mentions, because IL2CPP records no arity marker for methods. A generic method
  that never references its own parameter is emitted as non-generic.
- **Enum literals with an unrecoverable constant** are demoted to plain static
  fields rather than dropped, so the name still resolves.

### Staleness

`_Modrix/interop/generated-from.txt` records the metadata version and type count.
A game update changes both, and the assemblies must be regenerated - otherwise
mods compile against yesterday's game and fail at runtime in ways that look like
mod bugs.

## Constraints that shape the design

**One proxy slot, and it is already taken.** A game has exactly one
`version.dll`. On the reference machine MelonLoader currently owns it. Two
loaders cannot coexist: whichever loads second never runs, and the user gets a
game with half its mods missing and no diagnostic. This is why
`[mod_loader] conflicts` exists and why `install_loader` must refuse rather than
guess. `modrix loader status` already reports this correctly.

**Proton makes the proxy inert by default.** Wine resolves `version.dll` to its
own builtin unless `WINEDLLOVERRIDES="version=n,b"` is set. Without it the game
starts perfectly and *no mods load*, with no error anywhere. Modrix surfaces this
as `LaunchReadiness.warning` rather than letting users discover it. Note Modrix
cannot inject the variable by spawning, because Steam owns the game process,
only the user's Steam launch options can carry it.

**Interop regenerates on every game update.** The metadata hash changes, so the
generated assemblies must be rebuilt, and obfuscated type names can shift,
breaking mods at compile time. Modrix should hash `GameAssembly.dll` and compare
against a stamp beside the interop output, so this is reported as "the game
updated, mods need rebuilding" rather than experienced as a mystery.

**Mods split across two paths.** The loader enumerates `Mods/*.dll`
non-recursively for payloads but reads `Mods/<id>/manifest.json` for metadata. A
mod legitimately owns both a bare top-level DLL and a folder. This is why
`park_root_binaries = false` is mandatory in the game definition: without it
Modrix's Bethesda-shaped heuristic parks every payload DLL to the install root,
deploy reports success, and nothing loads.

**`manifest.json` version must parse as `System.Version`.** `1.0.0-beta.1` is
schema-valid but throws at load, and the mod is silently dropped.

## Milestone M0: proving injection

M0 answers one question with evidence and nothing else: **can managed .NET code
run inside this IL2CPP game, under Proton, without breaking it?** Everything
downstream (interop assemblies, detours, mod discovery, the SDK) is worthless
if the answer is no, and expensive to debug if it is discovered later from inside
a Proton-hosted CoreCLR process.

### The chain

1. **`DLL_PROCESS_ATTACH`.** Derive our own file name, resolve the real DLL
   (`<slot>_original.dll` / `<slot>_alt.dll` in the game folder, else
   `%SystemDirectory%\<slot>.dll`), point all 17 export thunks at it, truncate
   `_Modrix/Latest.log`, and `SetCurrentDirectory` to the game folder (under
   Steam the working directory is Steam's, which is Doorstop's `fix_cwd`).
2. **A spawned thread** polls `GetModuleHandleW("GameAssembly.dll")` and then
   `il2cpp_domain_get()` until it returns non-null. Nothing is hooked.
3. **CoreCLR** is loaded from `_Modrix/dotnet/coreclr.dll` and initialized with a
   single `APP_PATHS` property = `_Modrix/dotnet;_Modrix/net6`.
4. **`Modrix.Loader.Entrypoint.Start()`** (zero parameters, void return) writes
   the final log line.

`coreclr.dll` is loaded on the spawned thread, never under the loader lock.

### The export forwarding, and the one documented deviation from PLAN.md

`PLAN.md` D6 chose MelonLoader's model: export the names, then overwrite our own
stubs at runtime with `VirtualProtect(PAGE_EXECUTE_READWRITE)` + `mov r11, addr;
jmp r11`. What shipped instead is a `#[unsafe(naked)]` function per export that
loads a pointer from a table and tail-jumps:

```asm
mov rax, qword ptr [rip + SLOTS + N*8]
test rax, rax
je .null
jmp rax
```

Both are unconditionally correct for exports whose arguments spill to the stack
(the reason D6 rejected Doorstop's zero-argument C thunk, which is correct only
because the compiler happens to tail-call at `-Os`). The naked thunk additionally
needs no RWX memory, and `injection.md` names it "the only strictly correct
implementation". Verified by disassembling the cross-compiled release DLL.

### Why the poll trigger and not the IAT hook

`PLAN.md` §3 lists the IAT hook on `UnityPlayer.dll`'s
`kernel32!GetProcAddress` (returning our own `il2cpp_init`) and then names the
poll as the *lower-risk path to implement first*, with the hook as M0b. That is
what happened. Whether IAT patching survives this Proton build is an open
question (`injection.md`, Unknowns), and M0's claim does not need it.

Polling proves the runtime is **up**. Interception would additionally prove we
are **ahead of it**, which is what M1's `il2cpp_runtime_invoke` detour will
require. The log line says which mechanism produced it, because the two prove
different things:

```
[modrix] il2cpp runtime ready (poll trigger), domain = 0x00007ff...
```

### Acceptance

Full procedure (install, launch options, the four required log lines, failure
triage, and how to revert) is in
[`loaders/sons-proxy/README.md`](../loaders/sons-proxy/README.md). In short: park
MelonLoader's `version.dll`, copy the staged tree in, set
`WINEDLLOVERRIDES="version=n,b" %command%`, reach the main menu, and read
`_Modrix/Latest.log`. **The game must still play**: a log with a broken game is
a failed M0.

### What is verified today, and what is not

Verified on a Linux host, without the game:

- The proxy cross-compiles to an x86-64 Windows PE exporting all 17
  `version.dll` names (checked with `pefile`), importing only system DLLs.
- The PE entry point `DllMainCRTStartup` calls our `DllMain` (checked with
  `objdump`).
- Each export thunk reads its own table slot at the right offset (checked with
  `objdump`; also a unit test on the declaration table).
- 18 unit tests over path derivation, the `MODRIX_*` environment block, log
  formatting and the `mini-coreclr` `.version` parse (CRLF, real bytes).
- `Modrix.Loader.dll` builds against `net6.0`, and its emitted metadata says
  `.NETCoreApp,Version=v6.0` with `Modrix.Loader.Entrypoint` non-public and
  `Start` public/static/void/zero-parameter.
- `mini-coreclr` 6.0.7 `win-x64` `coreclr.dll` exports both hosting symbols and
  ships no `hostfxr.dll`.

**Not verified: this has never run:**

- Whether Wine loads our `version.dll` at all with the override set.
- Whether `coreclr_initialize` succeeds under this Proton build. This is the
  single largest unknown in the whole design; the loader logs
  `host = wine <version>` immediately before trying, so a failure report carries
  the one fact needed to diagnose it.
- Whether `il2cpp_domain_get` returns non-null within the 120 s budget.
- Whether the game still plays with the proxy in place.
- Whether Rust's `windows-gnu` UCRT imports (`api-ms-win-crt-*`) resolve in the
  Proton prefix. Proton ships `ucrtbase.dll`, so they should.

None of these can be answered from Linux without the game running, and the
reference machine's `version.dll` belongs to MelonLoader, which was left
untouched.

## Suggested order of work

1. **`modrix-interop-gen`**: emit one managed assembly per IL2CPP assembly:
   types, fields, method signatures, no bodies. Even a signatures-only output
   unblocks *compiling* mods, which is the current hard blocker (`dotnet build`
   in SOTFMods fails with `CS0246` because no interop assemblies exist).
2. **Proxy + CoreCLR host**: *built (M0), not yet run.* The next action is not
   more code: it is running the acceptance test on the real install. Until that
   passes, everything after it is speculative.
3. **M0b: the IAT hook**: replace the poll with interception of `il2cpp_init`,
   which M1's detour work needs. Only worth attempting once M0 has passed, so a
   failure is attributable to the hook and not to the whole chain.
4. **Mod host + SDK**: discovery, manifest parsing, lifecycle hooks, `RLog`
   equivalent. Aim for source compatibility with `SonsSdk` where it is free, so
   existing mods port with an import change rather than a rewrite.
5. **`install_loader`**: now that M0 defines a payload: `version.dll`,
   `_Modrix/net6/Modrix.Loader.dll`, `_Modrix/dotnet/*`. It must still refuse
   when a rival loader owns the slot, and it must park rather than overwrite.

Step 1 is independently valuable and has no `unsafe` in it. Steps 2 to 5 are where
the real risk lives.

## The unsafe boundary

`loaders/sons-proxy` is a **separate Cargo workspace**, not a member of
`crates/*`. The root manifest's `members = ["crates/*"]` never sees it, so
`unsafe_code = "forbid"` remains true for all thirteen workspace crates and root
`cargo build` / `clippy` / `test` are unaffected: verified with
`cargo metadata`, which lists 15 packages and no `modrix-sons-proxy`.

In exchange, the loader workspace is *stricter* where it counts:
`undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block` are **deny**, so
every unsafe block does one thing and says why it is sound; all FFI is confined
to `src/win.rs`; `panic = "abort"` in every profile, because unwinding across the
FFI boundary into Unity's stack is undefined behaviour. The rest (`unwrap_used`,
`panic`, `indexing_slicing`, `missing_docs`, the 60-line function cap) matches
the main workspace exactly.

## Honest scope note

Steps 2 to 5 are a substantial native-runtime project, not an afternoon. The
metadata reader is deliberately the first piece because it is the hard
dependency for everything else, is verifiable today against a real game, and
carries none of the injection risk. Until step 1 lands, Sons of the Forest mods
in this repo cannot be compiled at all without a third-party loader installed,
which the shared props supports, and which `SotfLoader=redloader` selects.

M0 shifts the loader from "not started" to "written and compiling", which is a
smaller step than it looks: every genuinely uncertain thing about injection is
uncertain *at runtime*, on a Windows PE, inside a Proton prefix. The code being
correct on a Linux host is a precondition, not evidence. Treat M0 as unproven
until someone has read those four lines out of a real `_Modrix/Latest.log` and
then played the game.
