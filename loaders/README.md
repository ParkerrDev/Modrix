<!-- SPDX-License-Identifier: GPL-2.0-only -->

# `loaders/`: code that runs inside a game process

Everything under this directory executes **in another program's address space**.
That is a different risk class from the rest of Modrix, and it is kept out of the
main Cargo workspace for exactly that reason: the root manifest declares
`members = ["crates/*"]`, so `cargo build` / `cargo clippy` / `cargo test` at the
repository root never build anything here, and `unsafe_code = "forbid"` stays
true for all thirteen workspace crates.

| Directory | What it is | Language | Workspace |
| --- | --- | --- | --- |
| `sons-proxy/` | Sons of the Forest `version.dll` proxy + CoreCLR host | Rust `cdylib`, Windows x86-64 | its own |
| `sons-managed/Modrix.Loader/` | the managed entry point CoreCLR calls | C# `net6.0` | its own |

Read [`sons-proxy/README.md`](sons-proxy/README.md) first: it carries the unsafe
policy, the build commands, the install procedure and the M0 acceptance test.
The architecture and status live in
[`../docs/SONS-LOADER.md`](../docs/SONS-LOADER.md).

## The one-line summary

```sh
cd sons-proxy && ./build-windows.sh --managed --coreclr <mini-coreclr>/win-x64
```

produces `sons-proxy/dist/` (a `version.dll` plus a `_Modrix/` tree) and copies
it nowhere. Installing is manual and deliberate: a game has exactly one proxy
slot, and on most machines another loader already occupies it.
