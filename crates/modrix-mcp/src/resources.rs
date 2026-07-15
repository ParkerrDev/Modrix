// SPDX-License-Identifier: GPL-2.0-only
//! MCP resources: skill files. Two guides ship in the binary (using Modrix,
//! authoring plugins); each installed game-support plugin contributes its
//! own game-specific skills from `<data>/plugins/<id>/skills/*.md` - so an
//! agent learns how to mod a game the moment support for it is installed.

use serde_json::{Value, json};

use crate::tools::Ctx;

/// Most skill files listed per plugin (bounded scan).
const MAX_SKILLS: usize = 16;

/// Built-in guides compiled into the binary.
const BUILTIN: [(&str, &str, &str); 2] = [
    (
        "modrix://skill/usage",
        "Managing mods with Modrix",
        include_str!("../../../skills/modrix-usage/SKILL.md"),
    ),
    (
        "modrix://skill/plugin-authoring",
        "Authoring Modrix game-support plugins",
        include_str!("../../../skills/modrix-plugin-authoring/SKILL.md"),
    ),
];

/// `resources/list`.
pub(crate) fn list(ctx: &Ctx) -> Value {
    let mut resources: Vec<Value> = BUILTIN
        .iter()
        .map(|(uri, name, _)| resource(uri, name))
        .collect();
    for (uri, name, _) in installed_skills(ctx) {
        resources.push(resource(&uri, &name));
    }
    json!({ "resources": resources })
}

/// `resources/read`.
pub(crate) fn read(ctx: &Ctx, params: &Value) -> Result<Value, String> {
    let uri = params
        .get("uri")
        .and_then(Value::as_str)
        .ok_or("missing `uri`")?;
    let text = BUILTIN
        .iter()
        .find(|(u, _, _)| *u == uri)
        .map(|(_, _, text)| (*text).to_owned())
        .or_else(|| {
            installed_skills(ctx)
                .into_iter()
                .find(|(u, _, _)| u == uri)
                .map(|(_, _, text)| text)
        })
        .ok_or_else(|| format!("unknown resource `{uri}`"))?;
    Ok(json!({
        "contents": [{ "uri": uri, "mimeType": "text/markdown", "text": text }],
    }))
}

fn resource(uri: &str, name: &str) -> Value {
    json!({ "uri": uri, "name": name, "mimeType": "text/markdown" })
}

/// `(uri, name, contents)` of every installed plugin's skill files.
fn installed_skills(ctx: &Ctx) -> Vec<(String, String, String)> {
    let mut out = Vec::new();
    for plugin in modrix_registry::installed_at(ctx.engine().paths()) {
        let dir = ctx
            .engine()
            .paths()
            .data_dir()
            .join("plugins")
            .join(&plugin.id)
            .join("skills");
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten().take(MAX_SKILLS) {
            let path = entry.path();
            let file = entry.file_name().to_string_lossy().into_owned();
            let is_markdown = std::path::Path::new(&file)
                .extension()
                .is_some_and(|e| e.eq_ignore_ascii_case("md"));
            if !is_markdown {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            out.push((
                format!("modrix://skill/{}/{file}", plugin.id),
                format!("{} - game skill", plugin.name),
                text,
            ));
        }
    }
    out
}
