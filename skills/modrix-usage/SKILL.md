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
