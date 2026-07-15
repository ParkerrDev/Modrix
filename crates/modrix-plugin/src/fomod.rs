// SPDX-License-Identifier: GPL-2.0-only
//! The FOMOD installer engine.
//!
//! Parses `fomod/ModuleConfig.xml` (the `ModConfig` 5.0 schema used across
//! Nexus), computes default selections, and materializes a chosen option set
//! into the staged tree. The original archive layout is parked under
//! `.fomod-src/` (hidden, never deployed), and selections are materialized
//! from it by hardlink - so re-configuring is cheap and lossless.
//!
//! Deliberate simplifications, matching what a mod manager can actually know:
//! `gameDependency`/`fomodDependency`/`fileDependency` conditions are treated
//! as satisfied; `flagDependency` conditions are evaluated exactly.

use std::collections::{BTreeSet, HashMap};
use std::fs;
use std::path::{Path, PathBuf};

use crate::{Error, Result};

/// Upper bound on file operations one installer may request.
const MAX_OPS: usize = 100_000;
/// Upper bound on the ModuleConfig.xml size we will parse.
const MAX_CONFIG_BYTES: u64 = 16 * 1024 * 1024;
/// Where the original archive layout is parked inside the staged tree.
const SRC_DIR: &str = ".fomod-src";

// --- model -------------------------------------------------------------------

/// A parsed FOMOD installer.
#[derive(Debug, Clone)]
pub struct Installer {
    /// `<moduleName>`.
    pub module_name: String,
    /// `fomod/info.xml` `<Name>`, when present (usually the cleaner name).
    pub info_name: Option<String>,
    /// `fomod/info.xml` `<Version>`, when present.
    pub info_version: Option<String>,
    /// Files always installed.
    pub required: Vec<FileOp>,
    /// The wizard pages.
    pub steps: Vec<Step>,
    /// Flag-conditional file sets applied after the wizard.
    pub conditional: Vec<Pattern>,
    /// `<moduleImage>` path, when present.
    pub module_image: Option<String>,
}

/// One wizard page.
#[derive(Debug, Clone)]
pub struct Step {
    /// Page title.
    pub name: String,
    /// Visibility condition on flags set by earlier pages.
    pub visible: Option<Dep>,
    /// Option groups on this page.
    pub groups: Vec<Group>,
}

/// A group of selectable plugins.
#[derive(Debug, Clone)]
pub struct Group {
    /// Group title.
    pub name: String,
    /// Selection rule.
    pub kind: GroupKind,
    /// The options.
    pub plugins: Vec<Plugin>,
}

/// How many plugins of a group may/must be selected.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GroupKind {
    /// Exactly one (radio buttons).
    ExactlyOne,
    /// Zero or one.
    AtMostOne,
    /// One or more.
    AtLeastOne,
    /// Zero or more (checkboxes).
    Any,
    /// All, locked.
    All,
}

/// One selectable option.
#[derive(Debug, Clone)]
pub struct Plugin {
    /// Option label.
    pub name: String,
    /// Long description.
    pub description: String,
    /// Preview image path inside the archive, when the installer ships one.
    pub image: Option<String>,
    /// Files installed when selected.
    pub files: Vec<FileOp>,
    /// Flags set when selected.
    pub flags: Vec<(String, String)>,
    /// Selection type.
    pub kind: TypeDescriptor,
}

/// A plugin's selection type, possibly flag-dependent.
#[derive(Debug, Clone)]
pub enum TypeDescriptor {
    /// A fixed type.
    Simple(PluginKind),
    /// `defaultType` + condition patterns evaluated against current flags.
    Dependent {
        /// Fallback type.
        default: PluginKind,
        /// `(condition, type)` pairs; first match wins.
        patterns: Vec<(Dep, PluginKind)>,
    },
}

/// The FOMOD plugin types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PluginKind {
    /// Freely selectable.
    Optional,
    /// Must be selected.
    Required,
    /// Preselected.
    Recommended,
    /// Cannot be selected.
    NotUsable,
    /// Selectable with a warning.
    CouldBeUsable,
}

/// A copy instruction from the archive layout into the mod tree.
#[derive(Debug, Clone)]
pub struct FileOp {
    /// Path inside the archive (backslashes normalized).
    pub source: String,
    /// Path inside the mod tree ("" = tree root).
    pub destination: String,
    /// Whether `source` names a folder.
    pub is_folder: bool,
    /// Higher priority overwrites lower.
    pub priority: i64,
}

/// A flag-conditional file set.
#[derive(Debug, Clone)]
pub struct Pattern {
    /// The condition.
    pub dep: Dep,
    /// Files applied when it holds.
    pub files: Vec<FileOp>,
}

/// A dependency condition.
#[derive(Debug, Clone)]
pub enum Dep {
    /// `<flagDependency flag value>`.
    Flag(String, String),
    /// `<fileDependency file state>`: whether a file is present in the
    /// install. `wants_missing` inverts (state="Missing").
    File {
        /// Lowercased filename.
        file: String,
        /// True when the condition requires the file to be absent.
        wants_missing: bool,
    },
    /// Conditions we cannot evaluate (game version, fomod version) -
    /// treated satisfied.
    Always,
    /// All children hold.
    And(Vec<Dep>),
    /// Any child holds.
    Or(Vec<Dep>),
}

/// The set of lowercased filenames present in the install (game Data dir +
/// files provided by enabled mods). Drives `fileDependency` conditions -
/// exactly how Vortex decides which patches to recommend.
pub type Present = std::collections::HashSet<String>;

impl Dep {
    /// Evaluate against the current flags and installed files.
    #[must_use]
    pub fn eval<S: std::hash::BuildHasher>(
        &self,
        flags: &HashMap<String, String, S>,
        present: &Present,
    ) -> bool {
        match self {
            Self::Flag(name, value) => flags.get(name).map_or("", String::as_str) == value,
            Self::File {
                file,
                wants_missing,
            } => present.contains(file) != *wants_missing,
            Self::Always => true,
            Self::And(children) => children.iter().all(|d| d.eval(flags, present)),
            Self::Or(children) => {
                children.is_empty() || children.iter().any(|d| d.eval(flags, present))
            }
        }
    }
}

/// Chosen plugin indices: `selected[step][group]` = set of plugin indices.
pub type Selections = Vec<Vec<BTreeSet<usize>>>;

// --- detection & parsing -------------------------------------------------------

/// Find the `fomod/` directory (any case) holding a `ModuleConfig.xml` -
/// at the tree root for a fresh stage, or inside `.fomod-src/` once an
/// install pass has parked the original layout.
#[must_use]
pub fn fomod_dir(staged_root: &Path) -> Option<PathBuf> {
    find_fomod_in(staged_root).or_else(|| find_fomod_in(&staged_root.join(SRC_DIR)))
}

fn find_fomod_in(dir: &Path) -> Option<PathBuf> {
    let entries = fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir()
            && entry
                .file_name()
                .to_string_lossy()
                .eq_ignore_ascii_case("fomod")
            && find_ci(&path, "ModuleConfig.xml").is_some()
        {
            return Some(path);
        }
    }
    None
}

/// Parse the staged tree's FOMOD installer, if it has one.
///
/// # Errors
///
/// Returns [`Error::Fomod`] if a `ModuleConfig.xml` exists but cannot be
/// parsed.
pub fn parse(staged_root: &Path) -> Result<Option<Installer>> {
    let Some(dir) = fomod_dir(staged_root) else {
        return Ok(None);
    };
    let Some(config) = find_ci(&dir, "ModuleConfig.xml") else {
        return Ok(None);
    };
    let text = read_bounded(&config)?;
    let doc = roxmltree::Document::parse(text.trim_start_matches('\u{feff}'))
        .map_err(|e| fomod_err(&config, &e.to_string()))?;
    let root = doc.root_element();
    let mut installer = Installer {
        module_name: child_text(root, "moduleName").unwrap_or_default(),
        info_name: None,
        info_version: None,
        required: child(root, "requiredInstallFiles")
            .map(|n| parse_files(n))
            .unwrap_or_default(),
        steps: parse_steps(root),
        conditional: parse_conditional(root),
        module_image: child(root, "moduleImage")
            .map(|n| attr(n, "path").replace('\\', "/"))
            .filter(|p| !p.is_empty()),
    };
    if let Some(info) = find_ci(&dir, "info.xml").and_then(|p| read_bounded(&p).ok()) {
        parse_info(&info, &mut installer);
    }
    Ok(Some(installer))
}

fn parse_info(text: &str, installer: &mut Installer) {
    if let Ok(doc) = roxmltree::Document::parse(text.trim_start_matches('\u{feff}')) {
        let root = doc.root_element();
        installer.info_name = descendant_text(root, "Name");
        installer.info_version = descendant_text(root, "Version");
    }
}

fn parse_steps(root: roxmltree::Node<'_, '_>) -> Vec<Step> {
    let Some(steps) = child(root, "installSteps") else {
        return Vec::new();
    };
    let mut out: Vec<Step> = elements(steps, "installStep")
        .map(|step| Step {
            name: attr(step, "name"),
            visible: child(step, "visible").map(parse_dep_group),
            groups: child(step, "optionalFileGroups")
                .map(parse_groups)
                .unwrap_or_default(),
        })
        .collect();
    sort_by_order(&mut out, order_of(steps), |s| &s.name);
    out
}

/// The `order` attribute of a list element; the schema default is Ascending.
fn order_of(node: roxmltree::Node<'_, '_>) -> Order {
    match node.attribute("order").unwrap_or("Ascending") {
        "Explicit" => Order::Explicit,
        "Descending" => Order::Descending,
        _ => Order::Ascending,
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Order {
    Explicit,
    Ascending,
    Descending,
}

/// Sort a parsed list by display name per its `order` attribute (Vortex
/// parity: document order only when Explicit).
fn sort_by_order<T>(items: &mut [T], order: Order, name: impl Fn(&T) -> &str) {
    match order {
        Order::Explicit => {}
        Order::Ascending => items.sort_by(|a, b| name(a).cmp(name(b))),
        Order::Descending => items.sort_by(|a, b| name(b).cmp(name(a))),
    }
}

fn parse_groups(groups: roxmltree::Node<'_, '_>) -> Vec<Group> {
    let mut out: Vec<Group> = elements(groups, "group")
        .map(|g| Group {
            name: attr(g, "name"),
            kind: match attr(g, "type").as_str() {
                "SelectExactlyOne" => GroupKind::ExactlyOne,
                "SelectAtMostOne" => GroupKind::AtMostOne,
                "SelectAtLeastOne" => GroupKind::AtLeastOne,
                "SelectAll" => GroupKind::All,
                _ => GroupKind::Any,
            },
            plugins: child(g, "plugins")
                .map(|p| {
                    let mut plugins: Vec<Plugin> =
                        elements(p, "plugin").map(parse_plugin).collect();
                    sort_by_order(&mut plugins, order_of(p), |pl| &pl.name);
                    plugins
                })
                .unwrap_or_default(),
        })
        .collect();
    sort_by_order(&mut out, order_of(groups), |g| &g.name);
    out
}

fn parse_plugin(node: roxmltree::Node<'_, '_>) -> Plugin {
    Plugin {
        name: attr(node, "name"),
        description: child_text(node, "description").unwrap_or_default(),
        image: child(node, "image")
            .map(|n| attr(n, "path").replace('\\', "/"))
            .filter(|p| !p.is_empty()),
        files: child(node, "files")
            .map(|n| parse_files(n))
            .unwrap_or_default(),
        flags: child(node, "conditionFlags")
            .map(|n| {
                elements(n, "flag")
                    .map(|f| (attr(f, "name"), f.text().unwrap_or("").trim().to_owned()))
                    .collect()
            })
            .unwrap_or_default(),
        kind: parse_type(node),
    }
}

fn parse_type(plugin: roxmltree::Node<'_, '_>) -> TypeDescriptor {
    let Some(descriptor) = child(plugin, "typeDescriptor") else {
        return TypeDescriptor::Simple(PluginKind::Optional);
    };
    if let Some(simple) = child(descriptor, "type") {
        return TypeDescriptor::Simple(kind_of(&attr(simple, "name")));
    }
    let Some(dependent) = child(descriptor, "dependencyType") else {
        return TypeDescriptor::Simple(PluginKind::Optional);
    };
    let default =
        child(dependent, "defaultType").map_or(PluginKind::Optional, |n| kind_of(&attr(n, "name")));
    let patterns = child(dependent, "patterns")
        .map(|ps| {
            elements(ps, "pattern")
                .filter_map(|p| {
                    let dep = child(p, "dependencies").map(parse_dep_group)?;
                    let kind = child(p, "type").map(|t| kind_of(&attr(t, "name")))?;
                    Some((dep, kind))
                })
                .collect()
        })
        .unwrap_or_default();
    TypeDescriptor::Dependent { default, patterns }
}

fn kind_of(name: &str) -> PluginKind {
    match name {
        "Required" => PluginKind::Required,
        "Recommended" => PluginKind::Recommended,
        "NotUsable" => PluginKind::NotUsable,
        "CouldBeUsable" => PluginKind::CouldBeUsable,
        _ => PluginKind::Optional,
    }
}

fn parse_conditional(root: roxmltree::Node<'_, '_>) -> Vec<Pattern> {
    let Some(cond) = child(root, "conditionalFileInstalls") else {
        return Vec::new();
    };
    let Some(patterns) = child(cond, "patterns") else {
        return Vec::new();
    };
    elements(patterns, "pattern")
        .filter_map(|p| {
            Some(Pattern {
                dep: child(p, "dependencies").map(parse_dep_group)?,
                files: child(p, "files").map(|n| parse_files(n))?,
            })
        })
        .collect()
}

/// Parse a `<dependencies>`-style node (also used for `<visible>`).
fn parse_dep_group(node: roxmltree::Node<'_, '_>) -> Dep {
    let operator = node.attribute("operator").unwrap_or("And");
    let children: Vec<Dep> = node
        .children()
        .filter(roxmltree::Node::is_element)
        .map(|c| match c.tag_name().name() {
            "flagDependency" => Dep::Flag(attr(c, "flag"), attr(c, "value")),
            "fileDependency" => Dep::File {
                file: attr(c, "file").to_ascii_lowercase(),
                wants_missing: attr(c, "state").eq_ignore_ascii_case("missing"),
            },
            "dependencies" => parse_dep_group(c),
            // game / fomod version dependencies: not decidable here.
            _ => Dep::Always,
        })
        .collect();
    if operator.eq_ignore_ascii_case("or") {
        Dep::Or(children)
    } else {
        Dep::And(children)
    }
}

fn parse_files(node: roxmltree::Node<'_, '_>) -> Vec<FileOp> {
    node.children()
        .filter(roxmltree::Node::is_element)
        .filter_map(|c| {
            let is_folder = match c.tag_name().name() {
                "folder" => true,
                "file" => false,
                _ => return None,
            };
            Some(FileOp {
                source: attr(c, "source").replace('\\', "/"),
                destination: attr(c, "destination").replace('\\', "/"),
                is_folder,
                priority: c
                    .attribute("priority")
                    .and_then(|p| p.trim().parse().ok())
                    .unwrap_or(0),
            })
        })
        .collect()
}

// --- roxmltree helpers ---------------------------------------------------------

fn child<'a, 'input>(
    node: roxmltree::Node<'a, 'input>,
    name: &str,
) -> Option<roxmltree::Node<'a, 'input>> {
    node.children()
        .find(|c| c.is_element() && c.tag_name().name() == name)
}

fn elements<'a, 'input: 'a>(
    node: roxmltree::Node<'a, 'input>,
    name: &'a str,
) -> impl Iterator<Item = roxmltree::Node<'a, 'input>> + 'a {
    node.children()
        .filter(move |c| c.is_element() && c.tag_name().name() == name)
}

fn child_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    child(node, name)
        .and_then(|n| n.text())
        .map(|t| t.trim().to_owned())
}

fn descendant_text(node: roxmltree::Node<'_, '_>, name: &str) -> Option<String> {
    node.descendants()
        .find(|c| c.is_element() && c.tag_name().name() == name)
        .and_then(|n| n.text())
        .map(|t| t.trim().to_owned())
        .filter(|t| !t.is_empty())
}

fn attr(node: roxmltree::Node<'_, '_>, name: &str) -> String {
    node.attribute(name).unwrap_or("").trim().to_owned()
}

// --- selection logic -----------------------------------------------------------

/// A plugin's effective kind under the current flags.
#[must_use]
pub fn plugin_kind<S: std::hash::BuildHasher>(
    descriptor: &TypeDescriptor,
    flags: &HashMap<String, String, S>,
    present: &Present,
) -> PluginKind {
    match descriptor {
        TypeDescriptor::Simple(kind) => *kind,
        TypeDescriptor::Dependent { default, patterns } => patterns
            .iter()
            .find(|(dep, _)| dep.eval(flags, present))
            .map_or(*default, |(_, kind)| *kind),
    }
}

/// The default selections, exactly as the installer configured them
/// (Vortex parity): `Required` and `SelectAll` are locked in,
/// `Recommended` is preselected, and radio-style groups (`ExactlyOne`,
/// `AtLeastOne`) take their first usable option because they cannot be
/// empty. Nothing else is selected.
#[must_use]
pub fn defaults(installer: &Installer, present: &Present) -> Selections {
    select_defaults(installer, present)
}

fn select_defaults(installer: &Installer, present: &Present) -> Selections {
    let mut flags = HashMap::new();
    let mut selections: Selections = Vec::new();
    for step in &installer.steps {
        let visible = step
            .visible
            .as_ref()
            .is_none_or(|d| d.eval(&flags, present));
        let mut step_sel = Vec::new();
        for group in &step.groups {
            let sel = if visible {
                default_group(group, &flags, present)
            } else {
                BTreeSet::new()
            };
            for i in &sel {
                if let Some(plugin) = group.plugins.get(*i) {
                    for (name, value) in &plugin.flags {
                        flags.insert(name.clone(), value.clone());
                    }
                }
            }
            step_sel.push(sel);
        }
        selections.push(step_sel);
    }
    selections
}

fn default_group(
    group: &Group,
    flags: &HashMap<String, String>,
    present: &Present,
) -> BTreeSet<usize> {
    let kind = |i: usize| {
        group.plugins.get(i).map_or(PluginKind::NotUsable, |p| {
            plugin_kind(&p.kind, flags, present)
        })
    };
    let mut sel: BTreeSet<usize> = (0..group.plugins.len())
        .filter(|i| match (group.kind == GroupKind::All, kind(*i)) {
            (true, k) => k != PluginKind::NotUsable,
            (false, PluginKind::Required | PluginKind::Recommended) => true,
            (false, _) => false,
        })
        .collect();
    // Radio-style groups keep a single choice; must-pick groups get one.
    if group.kind == GroupKind::ExactlyOne && sel.len() > 1 {
        let keep = sel.iter().copied().next();
        sel = keep.into_iter().collect();
    }
    if matches!(group.kind, GroupKind::ExactlyOne | GroupKind::AtLeastOne)
        && sel.is_empty()
        && let Some(first) = (0..group.plugins.len()).find(|i| kind(*i) != PluginKind::NotUsable)
    {
        sel.insert(first);
    }
    sel
}

/// The flags produced by a selection set (used for step visibility and
/// conditional installs).
#[must_use]
pub fn flags_of(
    installer: &Installer,
    selections: &Selections,
    present: &Present,
) -> HashMap<String, String> {
    let mut flags = HashMap::new();
    for (s, step) in installer.steps.iter().enumerate() {
        if !step
            .visible
            .as_ref()
            .is_none_or(|d| d.eval(&flags, present))
        {
            continue;
        }
        for (g, group) in step.groups.iter().enumerate() {
            let Some(sel) = selections.get(s).and_then(|ss| ss.get(g)) else {
                continue;
            };
            for i in sel {
                if let Some(plugin) = group.plugins.get(*i) {
                    for (name, value) in &plugin.flags {
                        flags.insert(name.clone(), value.clone());
                    }
                }
            }
        }
    }
    flags
}

/// All file operations a selection set installs, in apply order.
#[must_use]
pub fn resolve(installer: &Installer, selections: &Selections, present: &Present) -> Vec<FileOp> {
    let flags = flags_of(installer, selections, present);
    let mut ops = installer.required.clone();
    let mut visible_flags = HashMap::new();
    for (s, step) in installer.steps.iter().enumerate() {
        if !step
            .visible
            .as_ref()
            .is_none_or(|d| d.eval(&visible_flags, present))
        {
            continue;
        }
        for (g, group) in step.groups.iter().enumerate() {
            let Some(sel) = selections.get(s).and_then(|ss| ss.get(g)) else {
                continue;
            };
            for i in sel {
                if let Some(plugin) = group.plugins.get(*i) {
                    ops.extend(plugin.files.iter().cloned());
                    for (name, value) in &plugin.flags {
                        visible_flags.insert(name.clone(), value.clone());
                    }
                }
            }
        }
    }
    for pattern in &installer.conditional {
        if pattern.dep.eval(&flags, present) {
            ops.extend(pattern.files.iter().cloned());
        }
    }
    ops.sort_by_key(|op| op.priority);
    ops
}

// --- applying ------------------------------------------------------------------

/// Materialize `ops` into the staged tree. On first run the whole original
/// layout is parked under `.fomod-src/`; re-runs clear the visible tree and
/// re-materialize from the parked sources (hardlinks - cheap and lossless).
///
/// # Errors
///
/// Returns [`Error::Fomod`] on unresolvable sources, escape attempts, or I/O
/// failure.
pub fn apply(staged_root: &Path, ops: &[FileOp]) -> Result<usize> {
    if ops.len() > MAX_OPS {
        return Err(fomod_err(staged_root, "installer requests too many files"));
    }
    let src_root = staged_root.join(SRC_DIR);
    park_sources(staged_root, &src_root)?;
    let mut installed: usize = 0;
    for op in ops {
        let Some(source) = resolve_ci(&src_root, &op.source) else {
            // Configs routinely reference optional files that a given archive
            // simply does not ship; skip rather than fail the install.
            tracing::warn!(source = %op.source, "fomod source missing; skipped");
            continue;
        };
        let dest_rel = clean_destination(&op.destination, &op.source, op.is_folder)?;
        let dest = staged_root.join(&dest_rel);
        installed = installed.saturating_add(if source.is_dir() {
            copy_tree(&source, &dest)?
        } else {
            place_file(&source, &dest)?
        });
    }
    Ok(installed)
}

/// Move the original layout under `.fomod-src/` (first run), or clear the
/// materialized tree (re-run) so apply starts from a clean slate.
fn park_sources(staged_root: &Path, src_root: &Path) -> Result<()> {
    let first_run = !src_root.is_dir();
    if first_run {
        fs::create_dir_all(src_root).map_err(|e| io_err(src_root, &e))?;
    }
    let entries = fs::read_dir(staged_root).map_err(|e| io_err(staged_root, &e))?;
    for entry in entries.flatten() {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with('.') {
            continue;
        }
        let from = entry.path();
        if first_run {
            let to = src_root.join(&name);
            fs::rename(&from, &to).map_err(|e| io_err(&from, &e))?;
        } else if from.is_dir() {
            fs::remove_dir_all(&from).map_err(|e| io_err(&from, &e))?;
        } else {
            fs::remove_file(&from).map_err(|e| io_err(&from, &e))?;
        }
    }
    Ok(())
}

/// Normalize a destination: `/`-separators, no `..`, `Data/` prefix dropped
/// (destinations are Data-relative by convention). An empty file destination
/// keeps the source filename.
fn clean_destination(destination: &str, source: &str, is_folder: bool) -> Result<PathBuf> {
    let mut parts: Vec<&str> = destination
        .split('/')
        .filter(|p| !p.is_empty() && *p != ".")
        .collect();
    if parts.contains(&"..") {
        return Err(fomod_err(
            Path::new(destination),
            "destination escapes the mod",
        ));
    }
    if parts
        .first()
        .is_some_and(|p| p.eq_ignore_ascii_case("data"))
    {
        parts = parts.split_off(1);
    }
    let mut out: PathBuf = parts.iter().collect();
    if !is_folder && parts.is_empty() {
        let name = source.rsplit('/').next().unwrap_or(source);
        out = PathBuf::from(name);
    }
    Ok(out)
}

/// Resolve an archive-relative path (image, readme) against the staged tree:
/// the tree root for a fresh stage, or `.fomod-src/` once sources are parked.
/// Matching is case-insensitive, as installer configs routinely mis-case.
#[must_use]
pub fn source_path(staged_root: &Path, rel: &str) -> Option<PathBuf> {
    let rel = rel.replace('\\', "/");
    resolve_ci(staged_root, &rel).or_else(|| resolve_ci(&staged_root.join(SRC_DIR), &rel))
}

/// Resolve `rel` under `root`, matching each component case-insensitively.
fn resolve_ci(root: &Path, rel: &str) -> Option<PathBuf> {
    let mut current = root.to_path_buf();
    for part in rel.split('/').filter(|p| !p.is_empty() && *p != ".") {
        if part == ".." {
            return None;
        }
        current = find_ci(&current, part)?;
    }
    Some(current)
}

/// Find a directory entry by case-insensitive name.
fn find_ci(dir: &Path, name: &str) -> Option<PathBuf> {
    let exact = dir.join(name);
    if exact.exists() {
        return Some(exact);
    }
    fs::read_dir(dir)
        .ok()?
        .flatten()
        .find(|e| e.file_name().to_string_lossy().eq_ignore_ascii_case(name))
        .map(|e| e.path())
}

/// Hardlink (or copy) one file, replacing any earlier lower-priority file.
fn place_file(source: &Path, dest: &Path) -> Result<usize> {
    if let Some(parent) = dest.parent() {
        fs::create_dir_all(parent).map_err(|e| io_err(parent, &e))?;
    }
    if dest.exists() {
        fs::remove_file(dest).map_err(|e| io_err(dest, &e))?;
    }
    if fs::hard_link(source, dest).is_err() {
        fs::copy(source, dest).map_err(|e| io_err(dest, &e))?;
    }
    Ok(1)
}

/// Recursively materialize a folder (bounded, symlink-free by construction -
/// the sources were validated at staging time).
fn copy_tree(source: &Path, dest: &Path) -> Result<usize> {
    let mut placed: usize = 0;
    let mut stack = vec![(source.to_path_buf(), dest.to_path_buf())];
    while let Some((from, to)) = stack.pop() {
        fs::create_dir_all(&to).map_err(|e| io_err(&to, &e))?;
        let entries = fs::read_dir(&from).map_err(|e| io_err(&from, &e))?;
        for entry in entries.flatten() {
            let src = entry.path();
            let target = to.join(entry.file_name());
            if src.is_dir() {
                stack.push((src, target));
            } else {
                placed = placed.saturating_add(place_file(&src, &target)?);
                if placed > MAX_OPS {
                    return Err(fomod_err(source, "installer places too many files"));
                }
            }
        }
    }
    Ok(placed)
}

/// Read a config file, decoding UTF-8, UTF-16 (either endianness, by BOM),
/// or Latin-1 - all of which appear in the wild.
fn read_bounded(path: &Path) -> Result<String> {
    let meta = fs::metadata(path).map_err(|e| io_err(path, &e))?;
    if meta.len() > MAX_CONFIG_BYTES {
        return Err(fomod_err(path, "config file too large"));
    }
    let bytes = fs::read(path).map_err(|e| io_err(path, &e))?;
    Ok(decode_text(&bytes))
}

fn decode_text(bytes: &[u8]) -> String {
    match (bytes.first(), bytes.get(1)) {
        (Some(0xFF), Some(0xFE)) => utf16(bytes.get(2..).unwrap_or_default(), u16::from_le_bytes),
        (Some(0xFE), Some(0xFF)) => utf16(bytes.get(2..).unwrap_or_default(), u16::from_be_bytes),
        _ => std::str::from_utf8(bytes).map_or_else(
            |_| bytes.iter().map(|b| char::from(*b)).collect(),
            str::to_owned,
        ),
    }
}

fn utf16(bytes: &[u8], from_bytes: fn([u8; 2]) -> u16) -> String {
    let units = bytes
        .chunks_exact(2)
        .filter_map(|pair| <[u8; 2]>::try_from(pair).ok())
        .map(from_bytes);
    char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{fffd}'))
        .collect()
}

fn fomod_err(path: &Path, message: &str) -> Error {
    Error::Fomod {
        path: path.to_path_buf(),
        message: message.to_owned(),
    }
}

fn io_err(path: &Path, error: &std::io::Error) -> Error {
    fomod_err(path, &error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const CONFIG: &str = r#"<?xml version="1.0" encoding="utf-8"?>
<config>
  <moduleName>Sample</moduleName>
  <requiredInstallFiles>
    <folder source="Required" destination="" />
  </requiredInstallFiles>
  <installSteps order="Explicit">
    <installStep name="Main">
      <optionalFileGroups>
        <group name="Edition" type="SelectExactlyOne">
          <plugins order="Explicit">
            <plugin name="Full">
              <description>Everything.</description>
              <conditionFlags><flag name="full">On</flag></conditionFlags>
              <files><folder source="00 Core\Meshes" destination="meshes"/></files>
              <typeDescriptor><type name="Recommended"/></typeDescriptor>
            </plugin>
            <plugin name="Lite">
              <description>Less.</description>
              <files><folder source="10 Lite" destination=""/></files>
              <typeDescriptor><type name="Optional"/></typeDescriptor>
            </plugin>
          </plugins>
        </group>
        <group name="Extras" type="SelectAny">
          <plugins order="Explicit">
            <plugin name="Broken">
              <description/>
              <files><folder source="99 Broken" destination=""/></files>
              <typeDescriptor><type name="NotUsable"/></typeDescriptor>
            </plugin>
          </plugins>
        </group>
      </optionalFileGroups>
    </installStep>
  </installSteps>
  <conditionalFileInstalls>
    <patterns>
      <pattern>
        <dependencies operator="And"><flagDependency flag="full" value="On"/></dependencies>
        <files><file source="Extra\full.esp" destination=""/></files>
      </pattern>
    </patterns>
  </conditionalFileInstalls>
</config>"#;

    const INFO: &str = r"<fomod><Name>Sample Mod</Name><Version>1.2.3</Version></fomod>";

    fn write(path: &Path, bytes: &[u8]) {
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, bytes).unwrap();
    }

    fn build_tree(root: &Path) {
        write(&root.join("fomod/ModuleConfig.xml"), CONFIG.as_bytes());
        write(&root.join("fomod/info.xml"), INFO.as_bytes());
        write(&root.join("Required/base.esp"), b"base");
        write(&root.join("00 Core/Meshes/a.nif"), b"a");
        write(&root.join("10 Lite/meshes/a.nif"), b"lite");
        write(&root.join("Extra/full.esp"), b"extra");
    }

    #[test]
    fn parses_and_defaults_pick_recommended() {
        let tmp = tempfile::tempdir().unwrap();
        build_tree(tmp.path());
        let installer = parse(tmp.path()).unwrap().unwrap();
        assert_eq!(installer.module_name, "Sample");
        assert_eq!(installer.info_name.as_deref(), Some("Sample Mod"));
        assert_eq!(installer.info_version.as_deref(), Some("1.2.3"));
        let sel = defaults(&installer, &Present::new());
        assert_eq!(sel[0][0], BTreeSet::from([0])); // Recommended "Full"
        assert!(sel[0][1].is_empty()); // NotUsable never picked
    }

    #[test]
    fn resolve_includes_required_selected_and_conditional() {
        let tmp = tempfile::tempdir().unwrap();
        build_tree(tmp.path());
        let installer = parse(tmp.path()).unwrap().unwrap();
        let ops = resolve(
            &installer,
            &defaults(&installer, &Present::new()),
            &Present::new(),
        );
        let sources: Vec<_> = ops.iter().map(|o| o.source.as_str()).collect();
        assert!(sources.contains(&"Required"));
        assert!(sources.contains(&"00 Core/Meshes"));
        assert!(sources.contains(&"Extra/full.esp")); // flag "full" was set
        assert!(!sources.contains(&"10 Lite"));
    }

    #[test]
    fn apply_materializes_and_is_reconfigurable() {
        let tmp = tempfile::tempdir().unwrap();
        build_tree(tmp.path());
        let installer = parse(tmp.path()).unwrap().unwrap();
        let none = Present::new();
        let n = apply(
            tmp.path(),
            &resolve(&installer, &defaults(&installer, &none), &none),
        )
        .unwrap();
        assert_eq!(n, 3);
        assert!(tmp.path().join("base.esp").is_file());
        assert!(tmp.path().join("meshes/a.nif").is_file());
        assert!(tmp.path().join("full.esp").is_file());
        assert!(tmp.path().join(".fomod-src/10 Lite").is_dir());
        assert_eq!(fs::read(tmp.path().join("meshes/a.nif")).unwrap(), b"a");

        // Re-configure: pick "Lite" instead.
        let sel: Selections = vec![vec![BTreeSet::from([1]), BTreeSet::new()]];
        apply(tmp.path(), &resolve(&installer, &sel, &Present::new())).unwrap();
        assert_eq!(fs::read(tmp.path().join("meshes/a.nif")).unwrap(), b"lite");
        assert!(!tmp.path().join("full.esp").exists()); // flag no longer set
        assert!(tmp.path().join("base.esp").is_file()); // required stays

        // The installer must still be discoverable after apply parked the
        // sources - that is what powers re-opening the options wizard.
        let reparsed = parse(tmp.path()).unwrap();
        assert!(reparsed.is_some());
    }

    #[test]
    fn destination_data_prefix_is_dropped_and_escape_rejected() {
        assert_eq!(
            clean_destination("Data/meshes", "x", true).unwrap(),
            PathBuf::from("meshes")
        );
        assert_eq!(
            clean_destination("", "Extra/full.esp", false).unwrap(),
            PathBuf::from("full.esp")
        );
        assert!(clean_destination("../evil", "x", true).is_err());
    }
}
