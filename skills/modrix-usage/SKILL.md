# Managing mods with Modrix (for AI agents)

Modrix is a cross-platform mod manager: one engine, three frontends (GUI,
CLI `modrix`, MCP server `modrix mcp`). Everything a user can do in the GUI,
you can do through the CLI (`--json` for machine output) or MCP tools - same
engine, same data.

## The mental model

- A **game** is a registered install (id like `skyrimse`). One game is
  *active* at a time; game-scoped commands default to it.
- **Mods** are staged archives in Modrix's own store - installing never
  touches the game. **Deploying** links the enabled mods into the game
  directory, transactionally: reversible (`undeploy`), verifiable
  (`verify`), crash-safe. The game directory is never the source of truth.
- **Profiles** are switchable enabled-sets + load orders per game.
- **Conflicts** (two mods shipping the same file) are resolved by *rules*
  ("winner overrides loser") or per-file pins. Deploy refuses to run over
  unresolved conflicts or missing dependencies - check `health` first.
- What a game supports is data-driven (`game capabilities`): only
  plugin-based games (Bethesda) have a plugin load order; BepInEx games
  have none. Read the game's `modrix://skill/...` resource for specifics.

## The correct workflow

1. `game active` / `game set-active` - know what you are managing.
2. Install: `mod add <archive>` (CLI) or `install_mod` (MCP). Check
   `mod hash <archive>` / `find_duplicate` first - same content twice is
   almost never wanted.
3. `health` - fix what it reports: missing masters block deploy; loader
   warnings mean a mod will silently do nothing.
4. `mod conflicts` → `mod rule set <loser> <winner>` until nothing is
   unresolved. Convention: patches beat the mods they patch.
5. Plugin games: `plugins auto-sort`, then review `plugins list`.
6. `deploy` → `verify`. Report the numbers.

## Launching and testing a change

Some games declare how to start themselves and which mod loader they need.
`game capabilities` reports `launch` and `mod_loader`; when either is false the
commands below do not apply to that game and will say so.

1. `loader status` / `loader_status` - **check this before anything else on a
   loader game.** It reports whether the loader is installed, whether interop
   assemblies exist, and whether a *rival* loader occupies the proxy slot. A
   game has exactly one such slot, so two loaders never coexist: installing over
   one silently breaks both. If `conflict` is set, stop and tell the user which
   loader is in the way rather than working around it.
2. `deploy` → `launch` (or `deploy_and_launch` / `modrix launch --deploy`) to
   push a change and start the game in one step.
3. `log --since <cursor>` / `read_game_log` to watch it boot. Pass the returned
   `next_cursor` back as `since` to follow along without re-reading. A
   `truncated: true` result means the game restarted and the log began again -
   reset your view, do not treat it as corruption.

**On Linux and macOS, read the launch warning.** Windows-only games run under
Proton/Wine, where a loader's proxy DLL is ignored unless the game's Steam
launch options carry the environment the definition asks for (typically
`WINEDLLOVERRIDES="version=n,b" %command%`). Modrix cannot inject it, because
Steam owns the game process. Without it the game starts perfectly and **no mods
load, with no error** - so if a user reports "my mods stopped working" on Linux,
check `launch`'s `readiness` before investigating anything else.

`launch` is fire-and-forget: it returns as soon as the game is handed off, not
when the game is ready. Poll the log if you need to know it actually started.

## Rules of engagement

- Never edit the game directory by hand; hand-placed files show up as
  EXTERNAL/foreign and Modrix will not manage them.
- Never delete files to "fix" a conflict - set a rule instead.
- `undeploy` before large experiments; it restores the game exactly.
- Downloads arrive via the browser extension into the running instance;
  observe them with `downloads list` / `list_downloads` - do not fetch mod
  files yourself.
- Game support comes from the plugin registry: `registry search`, `registry
  install <id>`. Installing support for a detected game auto-registers it in
  the GUI; from the CLI use `game add --game <id> --install <dir>`.
