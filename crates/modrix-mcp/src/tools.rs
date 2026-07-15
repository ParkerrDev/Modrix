// SPDX-License-Identifier: GPL-2.0-only
//! The MCP tool surface: one tool per engine capability. Tool results carry
//! the same JSON payloads as the CLI's `--json` mode, as text content.
//!
//! Game selection: every game-scoped tool takes an optional `game` argument
//! (plugin id or numeric id); omitted, the active game is used, then the
//! sole registered game.

use serde_json::{Value, json};

use modrix_core::{Engine, Game, Mod};

/// Everything a tool handler can reach.
pub struct Ctx {
    engine: Engine,
    runtime: tokio::runtime::Runtime,
}

impl Ctx {
    /// Wrap an engine (opened and plugin-registered by the caller).
    ///
    /// # Errors
    ///
    /// Returns an I/O error if the async runtime cannot start.
    pub fn new(engine: Engine) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self { engine, runtime })
    }

    pub(crate) fn engine(&self) -> &Engine {
        &self.engine
    }
}

type ToolResult = Result<Value, String>;
type Handler = fn(&Ctx, &Value) -> ToolResult;

/// One MCP tool.
struct Tool {
    name: &'static str,
    description: &'static str,
    schema: Value,
    read_only: bool,
    run: Handler,
}

/// `tools/list` result.
pub(crate) fn list() -> Value {
    let tools: Vec<Value> = all()
        .iter()
        .map(|t| {
            json!({
                "name": t.name,
                "description": t.description,
                "inputSchema": t.schema,
                "annotations": { "readOnlyHint": t.read_only },
            })
        })
        .collect();
    json!({ "tools": tools })
}

/// `tools/call`: run the named tool; failures become `isError` content, so
/// the agent sees *what* failed instead of a dead connection.
pub(crate) fn call(ctx: &Ctx, params: &Value) -> Value {
    let name = params.get("name").and_then(Value::as_str).unwrap_or("");
    let args = params
        .get("arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let outcome = match all().iter().find(|t| t.name == name) {
        Some(tool) => (tool.run)(ctx, &args),
        None => Err(format!("unknown tool `{name}`")),
    };
    match outcome {
        Ok(data) => json!({
            "content": [{ "type": "text", "text": data.to_string() }],
            "isError": false,
        }),
        Err(message) => json!({
            "content": [{ "type": "text", "text": message }],
            "isError": true,
        }),
    }
}

/// A JSON-Schema object with `properties` and `required`.
fn schema(required: &[&str], properties: &Value) -> Value {
    json!({ "type": "object", "properties": properties, "required": required })
}

/// The optional `game` property fragment.
fn game_prop() -> Value {
    json!({ "game": { "type": "string", "description":
        "Game plugin id or numeric id; omit for the active game" } })
}

fn all() -> Vec<Tool> {
    let mut tools = game_tools();
    tools.extend(game_tools_b());
    tools.extend(profile_tools());
    tools.extend(mod_tools());
    tools.extend(toggle_tools());
    tools.extend(mod_tools_b());
    tools.extend(order_tools());
    tools.extend(rule_tools());
    tools.extend(override_tools());
    tools.extend(esp_tools());
    tools.extend(deploy_tools());
    tools.extend(deploy_tools_b());
    tools.extend(misc_tools());
    tools.extend(download_tools());
    tools
}

// --- shared resolution ------------------------------------------------------

fn err<T: std::fmt::Display>(e: T) -> String {
    e.to_string()
}

fn str_arg<'a>(args: &'a Value, key: &str) -> Result<&'a str, String> {
    args.get(key)
        .and_then(Value::as_str)
        .ok_or_else(|| format!("missing string argument `{key}`"))
}

/// Resolve the game: explicit `game` arg → active game → sole game.
fn game_of(ctx: &Ctx, args: &Value) -> Result<Game, String> {
    let games = ctx.engine().games().map_err(err)?;
    if let Some(sel) = args.get("game").and_then(Value::as_str) {
        return games
            .into_iter()
            .find(|g| g.plugin_id == sel || g.id.to_string() == sel)
            .ok_or_else(|| format!("no game matches `{sel}`"));
    }
    if let Ok(Some(active)) = ctx.engine().active_game() {
        return Ok(active);
    }
    match games.as_slice() {
        [] => Err("no games registered - add_game first".to_owned()),
        [_only] => games
            .into_iter()
            .next()
            .ok_or_else(|| "unreachable".to_owned()),
        _ => Err("several games registered - pass `game`".to_owned()),
    }
}

fn mod_of(ctx: &Ctx, game: &Game, selector: &str) -> Result<Mod, String> {
    ctx.engine()
        .mods(game.id)
        .map_err(err)?
        .into_iter()
        .find(|m| m.name == selector || m.id.to_string() == selector)
        .ok_or_else(|| format!("no mod matches `{selector}`"))
}

fn active_profile(ctx: &Ctx, game: &Game) -> Result<modrix_core::Profile, String> {
    ctx.engine().active_profile(game.id).map_err(err)
}

fn to_value<T: serde::Serialize>(value: &T) -> ToolResult {
    serde_json::to_value(value).map_err(err)
}

// --- games -------------------------------------------------------------------

fn game_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_games",
            description: "Registered games with their install paths.",
            schema: schema(&[], &json!({})),
            read_only: true,
            run: |ctx, _| to_value(&ctx.engine().games().map_err(err)?),
        },
        Tool {
            name: "get_active_game",
            description: "The game currently being worked on (null if none).",
            schema: schema(&[], &json!({})),
            read_only: true,
            run: |ctx, _| to_value(&ctx.engine().active_game().map_err(err)?),
        },
        Tool {
            name: "set_active_game",
            description: "Switch which game is being managed.",
            schema: schema(&["game"], &game_prop()),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                ctx.engine().set_active_game(game.id).map_err(err)?;
                to_value(&game)
            },
        },
    ]
}

fn game_tools_b() -> Vec<Tool> {
    vec![
        Tool {
            name: "detect_games",
            description: "Probe every known definition for installs on disk.",
            schema: schema(&[], &json!({})),
            read_only: true,
            run: |ctx, _| {
                let found: Vec<Value> = modrix_core::defcat::discover_defs(ctx.engine().paths())
                    .iter()
                    .filter_map(|entry| {
                        modrix_core::detect::detect_install(&entry.def).map(|path| {
                            json!({ "id": entry.def.id, "name": entry.def.name,
                                    "install": path })
                        })
                    })
                    .collect();
                Ok(Value::Array(found))
            },
        },
        Tool {
            name: "game_capabilities",
            description: "What the game supports: load order, external scan, health checks.",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                to_value(&ctx.engine().capabilities(game.id).map_err(err)?)
            },
        },
        Tool {
            name: "add_game",
            description: "Register a game: catalog definition by `game_id`, or an explicit \
                          `def_path` to a game.toml, plus its install directory.",
            schema: schema(
                &["install_path"],
                &json!({
                    "game_id": { "type": "string" },
                    "def_path": { "type": "string" },
                    "install_path": { "type": "string" },
                    "store": { "type": "string" },
                }),
            ),
            read_only: false,
            run: add_game,
        },
    ]
}

fn profile_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_profiles",
            description: "The game's profiles ('*' = active).",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                to_value(&ctx.engine().profiles(game.id).map_err(err)?)
            },
        },
        Tool {
            name: "create_profile",
            description: "Create a new profile for the game.",
            schema: schema(
                &["name"],
                &merged(game_prop(), &json!({ "name": {"type":"string"} })),
            ),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let name = str_arg(args, "name")?;
                to_value(&ctx.engine().create_profile(game.id, name).map_err(err)?)
            },
        },
        Tool {
            name: "switch_profile",
            description: "Make a profile active.",
            schema: schema(
                &["name"],
                &merged(game_prop(), &json!({ "name": {"type":"string"} })),
            ),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let name = str_arg(args, "name")?;
                let profile = ctx
                    .engine()
                    .profiles(game.id)
                    .map_err(err)?
                    .into_iter()
                    .find(|p| p.name == name)
                    .ok_or_else(|| format!("no profile `{name}`"))?;
                ctx.engine().set_active_profile(profile.id).map_err(err)?;
                to_value(&profile)
            },
        },
    ]
}

fn add_game(ctx: &Ctx, args: &Value) -> ToolResult {
    let install = std::path::PathBuf::from(str_arg(args, "install_path")?);
    let store = args
        .get("store")
        .and_then(Value::as_str)
        .unwrap_or("manual");
    let def = if let Some(path) = args.get("def_path").and_then(Value::as_str) {
        modrix_core::GameDef::from_file(std::path::Path::new(path)).map_err(err)?
    } else {
        let id = str_arg(args, "game_id")
            .map_err(|_| "pass `game_id` (catalog) or `def_path`".to_owned())?;
        modrix_core::defcat::find_def(ctx.engine().paths(), id)
            .ok_or_else(|| format!("no definition `{id}` in the catalog"))?
            .def
    };
    to_value(&ctx.engine().add_game(&def, &install, store).map_err(err)?)
}

fn merged(a: Value, b: &Value) -> Value {
    let mut out = a;
    if let (Some(out_map), Some(b_map)) = (out.as_object_mut(), b.as_object()) {
        for (k, v) in b_map {
            out_map.insert(k.clone(), v.clone());
        }
    }
    out
}

// --- mods ---------------------------------------------------------------------

fn mod_prop() -> Value {
    merged(
        game_prop(),
        &json!({ "mod": { "type": "string", "description": "Mod id or name" } }),
    )
}

fn mod_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_mods",
            description: "All staged mods of the game (with hashes and install times).",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                to_value(&ctx.engine().mods(game.id).map_err(err)?)
            },
        },
        Tool {
            name: "install_mod",
            description: "Install a mod archive (or directory); FOMOD options default. \
                          Check find_duplicate first to avoid double installs.",
            schema: schema(
                &["path"],
                &merged(game_prop(), &json!({ "path": {"type":"string"} })),
            ),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let path = std::path::PathBuf::from(str_arg(args, "path")?);
                let outcome =
                    modrix_service::install_file(ctx.engine(), game.id, &path).map_err(err)?;
                Ok(json!(format!("{outcome:?}")))
            },
        },
        Tool {
            name: "find_duplicate",
            description: "Hash an archive and report any already-installed mod with the \
                          same content.",
            schema: schema(
                &["path"],
                &merged(game_prop(), &json!({ "path": {"type":"string"} })),
            ),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let path = std::path::PathBuf::from(str_arg(args, "path")?);
                let hash = modrix_core::sha256_file(&path).map_err(err)?;
                let existing = ctx
                    .engine()
                    .find_by_archive_hash(game.id, &hash)
                    .map_err(err)?;
                Ok(json!({ "sha256": hash, "already_installed": existing }))
            },
        },
    ]
}

fn toggle_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "enable_mod",
            description: "Enable a mod in the active profile.",
            schema: schema(&["mod"], &mod_prop()),
            read_only: false,
            run: |ctx, args| set_mod_enabled(ctx, args, true),
        },
        Tool {
            name: "disable_mod",
            description: "Disable a mod in the active profile.",
            schema: schema(&["mod"], &mod_prop()),
            read_only: false,
            run: |ctx, args| set_mod_enabled(ctx, args, false),
        },
    ]
}

fn mod_tools_b() -> Vec<Tool> {
    vec![
        Tool {
            name: "remove_mod",
            description: "Delete a mod (withdraws its deployed files first).",
            schema: schema(&["mod"], &mod_prop()),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let m = mod_of(ctx, &game, str_arg(args, "mod")?)?;
                ctx.engine().delete_mod(m.id).map_err(err)?;
                Ok(json!({ "removed": m.name }))
            },
        },
        Tool {
            name: "reinstall_mod",
            description: "Re-stage a mod from its recorded archive.",
            schema: schema(&["mod"], &mod_prop()),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let m = mod_of(ctx, &game, str_arg(args, "mod")?)?;
                let fresh = ctx.engine().reinstall_mod(m.id).map_err(err)?;
                modrix_service::fomod_pass(ctx.engine(), &fresh).map_err(err)?;
                to_value(&fresh)
            },
        },
        Tool {
            name: "list_external_mods",
            description: "Mods in the game directory Modrix does NOT manage (read-only).",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                to_value(&ctx.engine().external_mods(game.id).map_err(err)?)
            },
        },
    ]
}

fn set_mod_enabled(ctx: &Ctx, args: &Value, on: bool) -> ToolResult {
    let game = game_of(ctx, args)?;
    let profile = active_profile(ctx, &game)?;
    let m = mod_of(ctx, &game, str_arg(args, "mod")?)?;
    ctx.engine()
        .set_enabled(profile.id, m.id, on)
        .map_err(err)?;
    Ok(json!({ "mod": m.name, "enabled": on }))
}

// --- ordering, conflicts, plugins ---------------------------------------------

fn order_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "get_load_order",
            description: "Enabled mods in deploy order (later wins file conflicts).",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                to_value(&ctx.engine().enabled_mods(profile.id).map_err(err)?)
            },
        },
        Tool {
            name: "set_load_order",
            description: "Set the mod load order: an array of mod ids/names, first to last.",
            schema: schema(
                &["mods"],
                &merged(
                    game_prop(),
                    &json!({ "mods": { "type": "array",
                    "items": {"type":"string"} } }),
                ),
            ),
            read_only: false,
            run: set_load_order,
        },
        Tool {
            name: "list_conflicts",
            description: "Pairwise mod conflicts and whether a rule resolves each.",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                to_value(&ctx.engine().mod_conflicts(profile.id).map_err(err)?)
            },
        },
    ]
}

fn rule_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "set_mod_rule",
            description: "Rule that `winner` overrides `loser` where their files collide.",
            schema: schema(
                &["loser", "winner"],
                &merged(
                    game_prop(),
                    &json!({ "loser": {"type":"string"},
                    "winner": {"type":"string"} }),
                ),
            ),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                let loser = mod_of(ctx, &game, str_arg(args, "loser")?)?;
                let winner = mod_of(ctx, &game, str_arg(args, "winner")?)?;
                ctx.engine()
                    .set_mod_rule(profile.id, loser.id, winner.id)
                    .map_err(err)?;
                Ok(json!({ "winner": winner.name, "loser": loser.name }))
            },
        },
        Tool {
            name: "clear_mod_rule",
            description: "Remove the rule between two mods.",
            schema: schema(
                &["a", "b"],
                &merged(
                    game_prop(),
                    &json!({ "a": {"type":"string"}, "b": {"type":"string"} }),
                ),
            ),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                let a = mod_of(ctx, &game, str_arg(args, "a")?)?;
                let b = mod_of(ctx, &game, str_arg(args, "b")?)?;
                ctx.engine()
                    .clear_mod_rule(profile.id, a.id, b.id)
                    .map_err(err)?;
                Ok(json!({ "cleared": true }))
            },
        },
    ]
}

fn override_tools() -> Vec<Tool> {
    vec![Tool {
        name: "set_file_override",
        description: "Pin one contested target path to a provider mod (null provider \
                          returns it to the rules).",
        schema: schema(
            &["target"],
            &merged(
                game_prop(),
                &json!({ "target": {"type":"string"},
                    "provider": {"type":"string"} }),
            ),
        ),
        read_only: false,
        run: |ctx, args| {
            let game = game_of(ctx, args)?;
            let profile = active_profile(ctx, &game)?;
            let target = str_arg(args, "target")?;
            let provider = match args.get("provider").and_then(Value::as_str) {
                Some(sel) => Some(mod_of(ctx, &game, sel)?.id),
                None => None,
            };
            ctx.engine()
                .set_file_override(profile.id, target, provider)
                .map_err(err)?;
            Ok(json!({ "target": target, "pinned": provider.is_some() }))
        },
    }]
}

fn esp_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_plugins",
            description: "The game's plugin (.esp/.esm/.esl) load order, with missing masters.",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                to_value(&ctx.engine().plugins(profile.id).map_err(err)?)
            },
        },
        Tool {
            name: "auto_sort_plugins",
            description: "LOOT-style sort: masters before dependents, master tier first.",
            schema: schema(&[], &game_prop()),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                to_value(&ctx.engine().auto_sort_plugins(profile.id).map_err(err)?)
            },
        },
        Tool {
            name: "sync_plugins_txt",
            description: "Rewrite the game's Plugins.txt from the current order.",
            schema: schema(&[], &game_prop()),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                let dir = ctx.engine().sync_plugins_txt(profile.id).map_err(err)?;
                Ok(json!({ "written_to": dir }))
            },
        },
    ]
}

fn set_load_order(ctx: &Ctx, args: &Value) -> ToolResult {
    let game = game_of(ctx, args)?;
    let profile = active_profile(ctx, &game)?;
    let list = args
        .get("mods")
        .and_then(Value::as_array)
        .ok_or("`mods` must be an array of ids/names")?;
    let mut ids = Vec::with_capacity(list.len());
    for entry in list {
        let sel = entry.as_str().ok_or("`mods` entries must be strings")?;
        ids.push(mod_of(ctx, &game, sel)?.id);
    }
    ctx.engine().set_load_order(profile.id, &ids).map_err(err)?;
    Ok(json!({ "ordered": ids.len() }))
}

// --- deploy + health ------------------------------------------------------------

fn deploy_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "check_health",
            description: "Setup problems, worst first (missing masters, loader checks, \
                          unresolved conflicts, foreign files).",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                to_value(&ctx.engine().health(profile.id).map_err(err)?)
            },
        },
        Tool {
            name: "deploy_blockers",
            description: "The health issues that will make deploy refuse to run.",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                to_value(&ctx.engine().deploy_blockers(profile.id).map_err(err)?)
            },
        },
        Tool {
            name: "plan_deploy",
            description: "Dry run: what deploy would add/remove, and file conflicts.",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                let plan = ctx.engine().plan(profile.id).map_err(err)?;
                Ok(
                    json!({ "to_add": plan.to_add(), "to_remove": plan.to_remove(),
                           "unchanged": plan.unchanged(),
                           "conflicts": plan.conflicts().len() }),
                )
            },
        },
    ]
}

fn deploy_tools_b() -> Vec<Tool> {
    vec![
        Tool {
            name: "deploy",
            description: "Make the game directory reflect the enabled mods (transactional, \
                          reversible). Refuses over blocking health issues.",
            schema: schema(&[], &game_prop()),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                let report = ctx.engine().deploy(profile.id).map_err(err)?;
                Ok(
                    json!({ "added": report.added(), "removed": report.removed(),
                           "unchanged": report.unchanged() }),
                )
            },
        },
        Tool {
            name: "undeploy",
            description: "Remove everything deployed, restoring displaced originals.",
            schema: schema(&[], &game_prop()),
            read_only: false,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                let report = ctx.engine().undeploy(profile.id).map_err(err)?;
                Ok(json!({ "removed": report.removed() }))
            },
        },
        Tool {
            name: "verify",
            description: "Check every deployed file against the manifest.",
            schema: schema(&[], &game_prop()),
            read_only: true,
            run: |ctx, args| {
                let game = game_of(ctx, args)?;
                let profile = active_profile(ctx, &game)?;
                let report = ctx.engine().verify(profile.id).map_err(err)?;
                Ok(
                    json!({ "clean": report.is_clean(), "checked": report.checked(),
                           "issues": report.issues() }),
                )
            },
        },
        Tool {
            name: "get_progress",
            description: "The engine's live progress for a long operation (null if idle).",
            schema: schema(&[], &json!({})),
            read_only: true,
            run: |ctx, _| to_value(&ctx.engine().progress().snapshot()),
        },
    ]
}

// --- registry + downloads --------------------------------------------------------

fn misc_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "registry_search",
            description: "Search the community plugin registry (game support, skills).",
            schema: schema(&[], &json!({ "query": {"type":"string"} })),
            read_only: true,
            run: |ctx, args| {
                let query = args.get("query").and_then(Value::as_str).unwrap_or("");
                let client = registry_client(ctx)?;
                let index = ctx.runtime.block_on(client.index(false)).map_err(err)?;
                to_value(
                    &modrix_registry::RegistryClient::search(&index, query)
                        .into_iter()
                        .cloned()
                        .collect::<Vec<_>>(),
                )
            },
        },
        Tool {
            name: "registry_install",
            description: "Install game support from the registry (its skills land as \
                          modrix://skill resources).",
            schema: schema(&["id"], &json!({ "id": {"type":"string"} })),
            read_only: false,
            run: |ctx, args| {
                let id = str_arg(args, "id")?;
                let client = registry_client(ctx)?;
                let index = ctx.runtime.block_on(client.index(false)).map_err(err)?;
                let entry = index
                    .plugins
                    .iter()
                    .find(|p| p.id == id)
                    .ok_or_else(|| format!("plugin `{id}` not in the registry"))?;
                to_value(&ctx.runtime.block_on(client.install(entry)).map_err(err)?)
            },
        },
        Tool {
            name: "registry_list_installed",
            description: "Locally installed registry plugins.",
            schema: schema(&[], &json!({})),
            read_only: true,
            run: |ctx, _| to_value(&modrix_registry::installed_at(ctx.engine().paths())),
        },
    ]
}

fn download_tools() -> Vec<Tool> {
    vec![
        Tool {
            name: "list_downloads",
            description: "Downloads on the live Modrix instance (browser hand-offs included).",
            schema: schema(&[], &json!({})),
            read_only: true,
            run: |ctx, _| forward(ctx, "/downloads"),
        },
        Tool {
            name: "cancel_download",
            description: "Cancel a download on the live instance.",
            schema: schema(&["id"], &json!({ "id": {"type":"integer"} })),
            read_only: false,
            run: |ctx, args| {
                let id = args
                    .get("id")
                    .and_then(Value::as_u64)
                    .ok_or("`id` must be an integer")?;
                forward(ctx, &format!("/download/{id}/cancel"))
            },
        },
    ]
}

fn registry_client(ctx: &Ctx) -> Result<modrix_registry::RegistryClient, String> {
    modrix_registry::RegistryClient::new(
        modrix_registry::RegistrySource::resolve(),
        ctx.engine().paths(),
    )
    .map_err(err)
}

/// Forward a request to the running GUI/serve instance over loopback IPC.
fn forward(ctx: &Ctx, path: &str) -> ToolResult {
    let lockfile = ctx.engine().paths().instance_lock();
    let secondary = modrix_ipc::secondary_from_lock(&lockfile)
        .map_err(|_| "no running Modrix instance (open the GUI or `modrix serve`)".to_owned())?;
    let reply = ctx
        .runtime
        .block_on(secondary.send(path, ""))
        .map_err(err)?;
    if reply.status != 200 {
        return Err(format!("instance replied {}: {}", reply.status, reply.body));
    }
    serde_json::from_str(&reply.body).map_err(err)
}
