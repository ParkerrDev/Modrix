// SPDX-License-Identifier: GPL-2.0-only
//! The pure deployment planner.
//!
//! Given the enabled mods (already resolved to their file lists) in load order
//! and the current manifest, this computes a [`DeployPlan`] - the exact set of
//! files to add, replace, keep, and remove - with **no I/O whatsoever**. That
//! makes it exhaustively unit-testable and is what lets us pin invariant I5:
//! the conflict winner is a pure function of load order, and every shadowed mod
//! is surfaced (never silently dropped).

use std::collections::BTreeMap;
use std::path::PathBuf;

use crate::deploy::manifest::DeployedRow;
use crate::id::{GameId, ModId};

/// One file a mod contributes: where it should land (relative to the deploy
/// root, `/`-separated) and the absolute source in the staging store.
#[derive(Debug, Clone)]
pub(crate) struct ResolvedFile {
    /// Target path relative to the game's deploy root.
    pub target_rel: String,
    /// Absolute source path in the staging store.
    pub source: PathBuf,
}

/// A file to place (a new target, or one whose winning source changed).
#[derive(Debug, Clone)]
pub(crate) struct PlannedAdd {
    /// The mod that wins this target.
    pub mod_id: ModId,
    /// Target path relative to the deploy root.
    pub target_rel: String,
    /// Absolute source in the staging store.
    pub source: PathBuf,
    /// The manifest row previously at this target, if we already owned it (used
    /// to reconstruct pre-state on rollback and to carry a displaced-original
    /// backup across a replace).
    pub prior: Option<DeployedRow>,
}

/// A previously deployed file that is no longer wanted and must be removed.
#[derive(Debug, Clone)]
pub(crate) struct PlannedRemove {
    /// The manifest row describing the file to remove.
    pub row: DeployedRow,
}

/// A target claimed by more than one enabled mod.
///
/// The engine never hides conflicts: the last mod in load order wins, and every
/// mod it shadowed is listed so a frontend can show the user what was overridden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Conflict {
    /// The contested target path (relative to the deploy root).
    pub target: String,
    /// The mod that won (last in load order).
    pub winner: ModId,
    /// The mods it shadowed, in load order.
    pub shadowed: Vec<ModId>,
}

/// A computed deployment: the transformation from the current on-disk state to
/// the desired one, plus the conflicts observed while resolving it.
#[derive(Debug)]
pub struct DeployPlan {
    pub(crate) game: GameId,
    pub(crate) target_root: PathBuf,
    pub(crate) backup_root: PathBuf,
    pub(crate) adds: Vec<PlannedAdd>,
    pub(crate) removes: Vec<PlannedRemove>,
    /// Rows that are already correct on disk and carry into the new manifest.
    pub(crate) keep: Vec<DeployedRow>,
    conflicts: Vec<Conflict>,
}

impl DeployPlan {
    /// Number of files this plan will place (new or replaced).
    #[must_use]
    pub fn to_add(&self) -> usize {
        self.adds.len()
    }

    /// Number of files this plan will remove.
    #[must_use]
    pub fn to_remove(&self) -> usize {
        self.removes.len()
    }

    /// Number of already-correct files left untouched.
    #[must_use]
    pub fn unchanged(&self) -> usize {
        self.keep.len()
    }

    /// The conflicts observed while resolving the file tree.
    #[must_use]
    pub fn conflicts(&self) -> &[Conflict] {
        &self.conflicts
    }

    /// Whether applying this plan would change nothing on disk.
    #[must_use]
    pub fn is_noop(&self) -> bool {
        self.adds.is_empty() && self.removes.is_empty()
    }
}

/// Resolve the desired file tree from `ordered_mods` (in load order) and diff it
/// against `current` to produce a plan. Pure: no filesystem access.
pub(crate) fn plan(
    game: GameId,
    target_root: PathBuf,
    backup_root: PathBuf,
    ordered_mods: &[(ModId, Vec<ResolvedFile>)],
    current: &[DeployedRow],
) -> DeployPlan {
    let (desired, conflicts) = resolve_tree(ordered_mods);

    let current_by_target: BTreeMap<&str, &DeployedRow> = current
        .iter()
        .map(|row| (row.target_rel.as_str(), row))
        .collect();

    let mut adds = Vec::new();
    let mut keep = Vec::new();
    // BTreeMap iteration is sorted by target → deterministic add ordering.
    for (target, win) in &desired {
        match current_by_target.get(target.as_str()) {
            Some(row) if row.source == win.source && row.mod_id == win.mod_id => {
                keep.push((*row).clone());
            }
            Some(row) => adds.push(PlannedAdd {
                mod_id: win.mod_id,
                target_rel: target.clone(),
                source: win.source.clone(),
                prior: Some((*row).clone()),
            }),
            None => adds.push(PlannedAdd {
                mod_id: win.mod_id,
                target_rel: target.clone(),
                source: win.source.clone(),
                prior: None,
            }),
        }
    }

    let removes = current
        .iter()
        .filter(|row| !desired.contains_key(&row.target_rel))
        .map(|row| PlannedRemove { row: row.clone() })
        .collect();

    DeployPlan {
        game,
        target_root,
        backup_root,
        adds,
        removes,
        keep,
        conflicts,
    }
}

/// The winning `(mod, source)` for a target.
struct Winner {
    mod_id: ModId,
    source: PathBuf,
}

/// Fold the ordered mods into `target → winner`, recording every shadowing.
/// Later mods win; the previously winning mod is pushed onto that target's
/// shadow list (invariant I5: deterministic, and nothing is hidden).
fn resolve_tree(
    ordered_mods: &[(ModId, Vec<ResolvedFile>)],
) -> (BTreeMap<String, Winner>, Vec<Conflict>) {
    let mut desired: BTreeMap<String, Winner> = BTreeMap::new();
    let mut shadowed: BTreeMap<String, Vec<ModId>> = BTreeMap::new();

    for (mod_id, files) in ordered_mods {
        for file in files {
            if let Some(prev) = desired.insert(
                file.target_rel.clone(),
                Winner {
                    mod_id: *mod_id,
                    source: file.source.clone(),
                },
            ) {
                // A different mod previously held this target → it is shadowed.
                if prev.mod_id != *mod_id {
                    shadowed
                        .entry(file.target_rel.clone())
                        .or_default()
                        .push(prev.mod_id);
                }
            }
        }
    }

    let conflicts = shadowed
        .into_iter()
        .filter_map(|(target, shadowed)| {
            desired.get(&target).map(|win| Conflict {
                target,
                winner: win.mod_id,
                shadowed,
            })
        })
        .collect();

    (desired, conflicts)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::LinkType;

    fn game() -> GameId {
        GameId::from_raw(1)
    }

    fn m(id: i64) -> ModId {
        ModId::from_raw(id)
    }

    fn file(target: &str, source: &str) -> ResolvedFile {
        ResolvedFile {
            target_rel: target.to_owned(),
            source: PathBuf::from(source),
        }
    }

    fn deployed(mod_id: i64, target: &str, source: &str) -> DeployedRow {
        DeployedRow {
            mod_id: m(mod_id),
            target_rel: target.to_owned(),
            source: PathBuf::from(source),
            link_type: LinkType::Hardlink,
            source_hash: "hash".to_owned(),
            backup_path: None,
        }
    }

    fn plan_of(ordered: &[(ModId, Vec<ResolvedFile>)], current: &[DeployedRow]) -> DeployPlan {
        plan(
            game(),
            PathBuf::from("/game"),
            PathBuf::from("/backups"),
            ordered,
            current,
        )
    }

    #[test]
    fn fresh_deploy_adds_every_file() {
        let mods = vec![(m(1), vec![file("a.esp", "/s/1/a"), file("b.esp", "/s/1/b")])];
        let p = plan_of(&mods, &[]);
        assert_eq!(p.to_add(), 2);
        assert_eq!(p.to_remove(), 0);
        assert_eq!(p.unchanged(), 0);
        assert!(p.conflicts().is_empty());
    }

    #[test]
    fn later_mod_wins_conflict_and_it_is_surfaced() {
        let mods = vec![
            (m(1), vec![file("shared.esp", "/s/1/shared")]),
            (m(2), vec![file("shared.esp", "/s/2/shared")]),
        ];
        let p = plan_of(&mods, &[]);
        assert_eq!(p.conflicts().len(), 1);
        let c = &p.conflicts()[0];
        assert_eq!(c.target, "shared.esp");
        assert_eq!(c.winner, m(2));
        assert_eq!(c.shadowed, vec![m(1)]);
        // The winning source is mod 2's file.
        let add = p
            .adds
            .iter()
            .find(|a| a.target_rel == "shared.esp")
            .unwrap();
        assert_eq!(add.source, PathBuf::from("/s/2/shared"));
    }

    #[test]
    fn conflict_winner_is_a_pure_function_of_load_order() {
        let ab = vec![
            (m(1), vec![file("x", "/s/1/x")]),
            (m(2), vec![file("x", "/s/2/x")]),
        ];
        let ba = vec![
            (m(2), vec![file("x", "/s/2/x")]),
            (m(1), vec![file("x", "/s/1/x")]),
        ];
        assert_eq!(plan_of(&ab, &[]).conflicts()[0].winner, m(2));
        assert_eq!(plan_of(&ba, &[]).conflicts()[0].winner, m(1));
    }

    #[test]
    fn unchanged_files_are_kept_not_re_added() {
        let mods = vec![(m(1), vec![file("a", "/s/1/a")])];
        let current = vec![deployed(1, "a", "/s/1/a")];
        let p = plan_of(&mods, &current);
        assert!(p.is_noop());
        assert_eq!(p.unchanged(), 1);
    }

    #[test]
    fn changed_source_becomes_a_replace_carrying_prior() {
        let mods = vec![(m(2), vec![file("a", "/s/2/a")])];
        let current = vec![deployed(1, "a", "/s/1/a")];
        let p = plan_of(&mods, &current);
        assert_eq!(p.to_add(), 1);
        assert_eq!(p.to_remove(), 0);
        assert!(p.adds[0].prior.is_some());
    }

    #[test]
    fn dropped_mod_files_are_removed() {
        let mods: Vec<(ModId, Vec<ResolvedFile>)> = vec![];
        let current = vec![deployed(1, "a", "/s/1/a"), deployed(1, "b", "/s/1/b")];
        let p = plan_of(&mods, &current);
        assert_eq!(p.to_remove(), 2);
        assert_eq!(p.to_add(), 0);
    }

    #[test]
    fn planning_is_deterministic() {
        let mods = vec![
            (m(1), vec![file("a", "/s/1/a"), file("z", "/s/1/z")]),
            (m(2), vec![file("a", "/s/2/a"), file("m", "/s/2/m")]),
        ];
        let first = plan_of(&mods, &[]);
        let second = plan_of(&mods, &[]);
        let targets = |p: &DeployPlan| {
            p.adds
                .iter()
                .map(|a| (a.target_rel.clone(), a.mod_id))
                .collect::<Vec<_>>()
        };
        assert_eq!(targets(&first), targets(&second));
        assert_eq!(first.conflicts(), second.conflicts());
    }

    // --- I5 as a property over random load orders ---------------------------
    use proptest::prelude::*;

    /// A pool of targets small enough that random mods collide often.
    const POOL: &[&str] = &["a.esp", "b.esp", "c.esp", "dir/d.esp", "dir/e.esp"];

    /// Build ordered mods from `per_mod[i]` = the target indices mod `i+1` owns.
    fn build(per_mod: &[Vec<usize>]) -> Vec<(ModId, Vec<ResolvedFile>)> {
        per_mod
            .iter()
            .enumerate()
            .map(|(i, targets)| {
                let mod_id = m(i64::try_from(i).unwrap_or(0).saturating_add(1));
                let files = targets
                    .iter()
                    .filter_map(|t| POOL.get(*t))
                    .map(|t| file(t, &format!("/s/{mod_id}/{t}")))
                    .collect();
                (mod_id, files)
            })
            .collect()
    }

    proptest! {
        #![proptest_config(ProptestConfig::with_cases(96))]

        /// I5: the plan is a pure function of its input, and every conflict's
        /// winner is the last mod in load order that provides that target.
        #[test]
        fn prop_deterministic_and_last_writer_wins(
            per_mod in prop::collection::vec(
                prop::collection::vec(0_usize..POOL.len(), 0..4),
                1..5,
            )
        ) {
            let ordered = build(&per_mod);
            let a = plan_of(&ordered, &[]);
            let b = plan_of(&ordered, &[]);

            let sig = |p: &DeployPlan| {
                let mut adds: Vec<_> =
                    p.adds.iter().map(|x| (x.target_rel.clone(), x.mod_id)).collect();
                adds.sort();
                adds
            };
            prop_assert_eq!(sig(&a), sig(&b));
            prop_assert_eq!(a.conflicts(), b.conflicts());

            for conflict in a.conflicts() {
                // The winner must be the last mod in order providing this target.
                let expected = ordered
                    .iter()
                    .rev()
                    .find(|(_, files)| files.iter().any(|f| f.target_rel == conflict.target))
                    .map(|(id, _)| *id);
                prop_assert_eq!(Some(conflict.winner), expected);
                prop_assert!(!conflict.shadowed.contains(&conflict.winner));
            }
        }
    }
}
