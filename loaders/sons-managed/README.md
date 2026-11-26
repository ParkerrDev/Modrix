<!-- SPDX-License-Identifier: GPL-2.0-only -->

# `sons-managed`: the managed half of the Sons of the Forest loader

One project today: `Modrix.Loader`, the assembly CoreCLR loads when the native
proxy hands off.

```sh
dotnet build -c Release Modrix.Loader/Modrix.Loader.csproj
# -> Modrix.Loader/bin/Release/net6.0/Modrix.Loader.dll
```

Deployed to `<game>/_Modrix/net6/Modrix.Loader.dll`, which is also the
`sentinel` that `games/sonsoftheforest/game.toml` uses to decide whether the
loader is installed.

## Three things that are not stylistic choices

**`net6.0`.** The proxy hosts BepInEx's `mini-coreclr` 6.0.7, and
`coreclr_create_delegate` loads this assembly into *that* runtime. A net8 or net9
assembly fails with a bare `TypeLoadException`, inside a game, under Proton.
`NETSDK1138` ("net6.0 is out of support") is expected and suppressed rather than
"fixed" by retargeting. Building it on a modern SDK downloads the .NET 6
targeting pack from NuGet on first build.

**`Modrix.Loader.Entrypoint.Start()`: zero parameters, void return.**
`coreclr_create_delegate` builds a reverse-P/Invoke stub from the method's own
signature. An extra parameter compiles fine and corrupts the stack at call time.
The type name and method name are hard-coded on the native side
(`src/layout.rs`); changing either here without changing both there produces
`coreclr_create_delegate returned 0x80131522` and nothing else.

**`Entrypoint` is `internal`.** CoreCLR resolves the entry point reflectively and
does not need it public: RedLoader's `Doorstop.Entrypoint` is `internal` for the
same reason. Nothing else should ever call it.

## Everything is caught

An exception escaping `Start` crosses back through the reverse-P/Invoke stub and
tears the process down: the user sees the game close with no window, no dialog
and no log. `Start` therefore catches everything and writes
`<game>/_Modrix/crash_<timestamp>.log`. A crash file with a stack trace is the
whole difference between a bug report and "it just closes".

## M0 scope

Read the `MODRIX_*` environment block the proxy published, append the milestone
line to `_Modrix/Latest.log`, echo the environment for diagnosis, return. No mod
discovery, no `manifest.json`, no Il2CppInterop, no SDK: those are M2 and M3.
