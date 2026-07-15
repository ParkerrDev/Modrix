# Authoring Modrix game-support plugins (for AI agents)

A plugin adds support for one game. It lives in the community registry
(github.com/ParkerrDev/modrix-plugins) as `plugins/<id>/` and installs into
`<data>/plugins/<id>/`. Most games need **only data** - write a `game.toml`
and you are done; write `game.lua` only when logic is unavoidable.

## plugin.toml (registry manifest)

```toml
id          = "mygame"     # = game.toml id; lowercase, stable forever
name        = "My Game"
version     = "1.0.0"      # bump on ANY change
api_version = 2            # = game.toml api_version
authors     = ["you"]
```

## game.toml (api_version 2) - the whole game model

```toml
api_version   = 2
id            = "mygame"
name          = "My Game"
steam_appid   = 123456            # enables Steam install detection + artwork
nexus_domain  = "mygame"          # routes nxm:// and browser downloads
mod_root      = "Mods"            # where mods deploy, relative to the install
                                   # ("" = install root; nested like
                                   # "BepInEx/plugins" is fine and preserves case)
deploy        = "link"            # or "copy" if the game breaks on links
install_probe = ["steam"]

content_dirs = ["textures", ...]  # archive-root dirs that ARE content,
                                   # never packaging wrappers to strip
base_files   = ["base.pak", "dlc*"]  # what the base game ships (lowercase,
                                      # trailing * = prefix) - never "foreign"

[load_order]                       # ONLY for games with an activation file
strategy    = "plugins_txt"
appdata_dir = "My Game"            # its folder under local app data

[[external_scan]]                  # how hand-installed mods look on disk
kind  = "folder"                   # "folder": each subdir = one mod
label = "plugin"                   # or "file" with exts = ["esp"] etc.
dir   = ""                         # relative to the deploy root

[health.loader]                    # optional: script-extender style check
plugins_dir = "SKSE/Plugins"
root_prefix = "skse"
message     = "…install the loader…"

[[health.recommended]]             # optional: known part-A-needs-part-B
if_file            = "path/in/mod.dll"
requires_root_file = "d3dx9_42.dll"
message            = "…"
```

Decide capabilities honestly: omit `[load_order]` for dependency-ordered
ecosystems (BepInEx/Unity) - frontends then hide load-order UI entirely.

## game.lua (Tier 2 - only when data cannot express it)

Callbacks, all optional: `detect()` → install path or nil;
`mod_root(install)` → string or nil; `install()` → stage via
`modrix.fs.stage(src, dest)` then `return true` (nil/false = use default
normalization); `load_order(plugins)` → reordered array or nil.

The sandbox is strict: only `table`/`string`/`math` stdlibs; no `io`, `os`,
`require`, `load`, `print`. Use `modrix.log.info/warn/debug`,
`modrix.fs.exists/read_dir/read_text` (jailed to the tree in question,
read-only, size-capped), and `modrix.game.{id,name,mod_root,steam_appid}`.
Budgets: ~5M instructions / 250ms / 64MiB per callback - precompute, don't
brute-force. You return *plans*; you can never write files.

## Skills

Ship `skills/<id>.skill.md`: what kind of game it is, the correct agent
workflow, and the pitfalls (see the skyrimse/subnautica plugins for the
shape). It is exposed to agents as an MCP resource after install.

## Submitting

1. `modrix plugin validate plugins/<id>` - must pass (CI runs consistency
   checks too).
2. `modrix plugin hash plugins/<id>` if you need the file hashes.
3. `python3 scripts/gen_index.py` in the registry repo, commit index.json.
4. One plugin per PR; GPL-2.0-only; ≤64 files, ≤1 MiB each, no binaries.
