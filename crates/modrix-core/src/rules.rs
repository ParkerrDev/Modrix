// SPDX-License-Identifier: GPL-2.0-only
//! Conflict-resolution rules (the Vortex model).
//!
//! Loose-file conflicts between mods are resolved deterministically:
//!
//! 1. A **file override** pins one target path to one providing mod.
//! 2. A **mod rule** ("winner loads after loser") makes the winner take every
//!    contested file against the loser.
//! 3. With neither, the **install order** decides (later mod wins).
//!
//! Rules form a directed graph over mods; a cycle makes the configuration
//! unsatisfiable, so cycles are detected and reported (and block deployment).

use std::collections::{BTreeMap, HashMap};

use crate::deploy::Conflict;
use crate::id::ModId;

/// Most rule edges walked while ordering (bounded loop).
const MAX_EDGES: usize = 100_000;

/// One rule: `winner` loads after (and therefore overrides) `loser`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
pub struct ModRule {
    /// The mod whose contested files are overridden.
    pub loser: ModId,
    /// The mod that provides every contested file of the pair.
    pub winner: ModId,
}

/// One contested file of a conflicting pair.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ConflictFile {
    /// The target path (relative to the deploy root).
    pub target: String,
    /// The mod currently winning this file.
    pub winner: ModId,
    /// Whether a per-file override pins this target.
    pub overridden: bool,
}

/// Two mods contesting one or more files, plus the rule state between them.
#[derive(Debug, Clone, serde::Serialize)]
pub struct ModConflict {
    /// One side of the pair (the smaller id - canonical order).
    pub first: ModId,
    /// The other side.
    pub second: ModId,
    /// The rule between the pair, if one is configured.
    pub rule: Option<ModRule>,
    /// Every contested file, with its current winner.
    pub files: Vec<ConflictFile>,
}

impl ModConflict {
    /// A conflict is resolved when a rule covers the pair or every contested
    /// file carries an explicit override.
    #[must_use]
    pub fn resolved(&self) -> bool {
        self.rule.is_some() || self.files.iter().all(|f| f.overridden)
    }
}

/// Reorder `mods` (given in install order) so every rule's winner comes after
/// its loser - a stable topological sort. Returns the new order plus the mods
/// caught in a rule cycle, if any (cycle members keep their current order, so
/// callers stay functional while surfacing the problem).
#[must_use]
pub fn effective_order(mods: &[ModId], rules: &[ModRule]) -> (Vec<ModId>, Vec<ModId>) {
    let index: HashMap<ModId, usize> = mods.iter().enumerate().map(|(i, m)| (*m, i)).collect();
    let mut indegree: Vec<usize> = vec![0; mods.len()];
    let mut edges: Vec<Vec<usize>> = vec![Vec::new(); mods.len()];
    for rule in rules.iter().take(MAX_EDGES) {
        // Rules about disabled/deleted mods are inert.
        let (Some(&from), Some(&to)) = (index.get(&rule.loser), index.get(&rule.winner)) else {
            continue;
        };
        if from == to {
            continue;
        }
        if let Some(out) = edges.get_mut(from) {
            out.push(to);
        }
        if let Some(d) = indegree.get_mut(to) {
            *d = d.saturating_add(1);
        }
    }
    // Kahn's algorithm, always emitting the lowest current-position ready
    // node - stable: unrelated mods keep their install order.
    let mut ready: Vec<usize> = indegree
        .iter()
        .enumerate()
        .filter(|(_, d)| **d == 0)
        .map(|(i, _)| i)
        .collect();
    let mut out = Vec::with_capacity(mods.len());
    let mut emitted = vec![false; mods.len()];
    while let Some(pos) = ready
        .iter()
        .enumerate()
        .min_by_key(|(_, i)| **i)
        .map(|(p, _)| p)
    {
        let node = ready.swap_remove(pos);
        if let Some(e) = emitted.get_mut(node) {
            *e = true;
        }
        if let Some(m) = mods.get(node) {
            out.push(*m);
        }
        for &next in edges.get(node).map_or(&[][..], Vec::as_slice) {
            if let Some(d) = indegree.get_mut(next) {
                *d = d.saturating_sub(1);
                if *d == 0 {
                    ready.push(next);
                }
            }
        }
    }
    // Whatever never became ready sits on a cycle.
    let mut cycle = Vec::new();
    for (i, m) in mods.iter().enumerate() {
        if !emitted.get(i).copied().unwrap_or(true) {
            out.push(*m);
            cycle.push(*m);
        }
    }
    (out, cycle)
}

/// Fold per-file plan conflicts into per-mod-pair conflicts annotated with the
/// configured rules and overrides. Pairs are canonical (`first < second`) and
/// sorted worst-first (unresolved, then by contested-file count).
#[must_use]
pub fn summarize<S: std::hash::BuildHasher>(
    conflicts: &[Conflict],
    rules: &[ModRule],
    overrides: &HashMap<String, ModId, S>,
) -> Vec<ModConflict> {
    let rule_of = |a: ModId, b: ModId| {
        rules
            .iter()
            .find(|r| (r.loser == a && r.winner == b) || (r.loser == b && r.winner == a))
            .copied()
    };
    let mut pairs: BTreeMap<(ModId, ModId), Vec<ConflictFile>> = BTreeMap::new();
    for conflict in conflicts {
        for loser in &conflict.shadowed {
            let (first, second) = if *loser < conflict.winner {
                (*loser, conflict.winner)
            } else {
                (conflict.winner, *loser)
            };
            pairs
                .entry((first, second))
                .or_default()
                .push(ConflictFile {
                    target: conflict.target.clone(),
                    winner: conflict.winner,
                    overridden: overrides.contains_key(&conflict.target.to_ascii_lowercase()),
                });
        }
    }
    let mut out: Vec<ModConflict> = pairs
        .into_iter()
        .map(|((first, second), files)| ModConflict {
            first,
            second,
            rule: rule_of(first, second),
            files,
        })
        .collect();
    out.sort_by_key(|c| (c.resolved(), std::cmp::Reverse(c.files.len())));
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;

    fn m(id: i64) -> ModId {
        ModId::from_raw(id)
    }

    #[test]
    fn no_rules_keeps_install_order() {
        let mods = vec![m(1), m(2), m(3)];
        let (order, cycle) = effective_order(&mods, &[]);
        assert_eq!(order, mods);
        assert!(cycle.is_empty());
    }

    #[test]
    fn a_rule_moves_the_winner_after_the_loser() {
        // Install order 1, 2 - but 1 must win against 2.
        let mods = vec![m(1), m(2)];
        let rules = vec![ModRule {
            loser: m(2),
            winner: m(1),
        }];
        let (order, cycle) = effective_order(&mods, &rules);
        assert_eq!(order, vec![m(2), m(1)]);
        assert!(cycle.is_empty());
    }

    #[test]
    fn unrelated_mods_stay_stable_around_a_rule() {
        let mods = vec![m(1), m(2), m(3), m(4)];
        let rules = vec![ModRule {
            loser: m(4),
            winner: m(2),
        }];
        let (order, _) = effective_order(&mods, &rules);
        assert_eq!(order, vec![m(1), m(3), m(4), m(2)]);
    }

    #[test]
    fn a_cycle_is_reported_and_order_preserved() {
        let mods = vec![m(1), m(2), m(3)];
        let rules = vec![
            ModRule {
                loser: m(1),
                winner: m(2),
            },
            ModRule {
                loser: m(2),
                winner: m(1),
            },
        ];
        let (order, cycle) = effective_order(&mods, &rules);
        assert_eq!(order.len(), 3);
        assert_eq!(cycle, vec![m(1), m(2)]);
        // The unrelated mod is unaffected.
        assert!(order.contains(&m(3)));
    }

    #[test]
    fn rules_about_absent_mods_are_inert() {
        let mods = vec![m(1), m(2)];
        let rules = vec![ModRule {
            loser: m(9),
            winner: m(1),
        }];
        let (order, cycle) = effective_order(&mods, &rules);
        assert_eq!(order, mods);
        assert!(cycle.is_empty());
    }

    #[test]
    fn summarize_groups_by_pair_and_flags_resolution() {
        let conflicts = vec![
            Conflict {
                target: "a.dds".to_owned(),
                winner: m(2),
                shadowed: vec![m(1)],
            },
            Conflict {
                target: "b.dds".to_owned(),
                winner: m(2),
                shadowed: vec![m(1)],
            },
            Conflict {
                target: "c.dds".to_owned(),
                winner: m(3),
                shadowed: vec![m(1)],
            },
        ];
        let rules = vec![ModRule {
            loser: m(1),
            winner: m(2),
        }];
        let overrides: HashMap<String, ModId> = HashMap::new();
        let pairs = summarize(&conflicts, &rules, &overrides);
        assert_eq!(pairs.len(), 2);
        // Unresolved pairs sort first.
        assert_eq!((pairs[0].first, pairs[0].second), (m(1), m(3)));
        assert!(!pairs[0].resolved());
        assert_eq!((pairs[1].first, pairs[1].second), (m(1), m(2)));
        assert!(pairs[1].resolved());
        assert_eq!(pairs[1].files.len(), 2);
    }

    #[test]
    fn full_overrides_resolve_a_pair_without_a_rule() {
        let conflicts = vec![Conflict {
            target: "Tex/a.dds".to_owned(),
            winner: m(2),
            shadowed: vec![m(1)],
        }];
        let overrides: HashMap<String, ModId> =
            [("tex/a.dds".to_owned(), m(1))].into_iter().collect();
        let pairs = summarize(&conflicts, &[], &overrides);
        assert!(pairs[0].resolved());
        assert!(pairs[0].files[0].overridden);
    }
}
