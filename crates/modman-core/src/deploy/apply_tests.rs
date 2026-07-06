// SPDX-License-Identifier: GPL-2.0-only
//! The deploy-engine invariant tests (I1-I4).
//!
//! Everything runs in a temp directory with a fake game and fake mods. Because
//! the temp dir and the staging store share a filesystem, `place` always
//! hardlinks - the same path production takes on a single-volume install. Trees
//! are compared by relative-path → content, which is the practical meaning of
//! "byte-identical game directory."

use std::collections::BTreeMap;
use std::fs;
use std::path::{Path, PathBuf};

use rusqlite::Connection;

use super::{Faults, apply};
use crate::deploy::plan::{ResolvedFile, plan};
use crate::deploy::{journal, manifest};
use crate::id::{GameId, ModId, ProfileId};
use crate::paths::Paths;

/// A fake game install + staging store + database, all under one temp dir.
struct Harness {
    _tmp: tempfile::TempDir,
    paths: Paths,
    conn: Connection,
    game: GameId,
    profile: ProfileId,
    target_root: PathBuf,
    store: PathBuf,
    next_mod: i64,
}

impl Harness {
    fn new() -> Self {
        let tmp = tempfile::tempdir().unwrap();
        let paths = Paths::rooted_at(tmp.path());
        paths.ensure_dirs().unwrap();
        let conn = crate::db::open(&paths.database_file()).unwrap();
        let target_root = tmp.path().join("game");
        let store = tmp.path().join("store");
        fs::create_dir_all(&target_root).unwrap();
        fs::create_dir_all(&store).unwrap();

        conn.execute(
            "INSERT INTO games (id, plugin_id, name, install_path, store, staging_root) \
             VALUES (1, 'test', 'Test Game', ?1, 'manual', ?2)",
            rusqlite::params![target_root.to_string_lossy(), store.to_string_lossy()],
        )
        .unwrap();
        conn.execute(
            "INSERT INTO profiles (id, game_id, name, is_active) VALUES (1, 1, 'default', 1)",
            [],
        )
        .unwrap();

        Self {
            _tmp: tmp,
            paths,
            conn,
            game: GameId::from_raw(1),
            profile: ProfileId::from_raw(1),
            target_root,
            store,
            next_mod: 1,
        }
    }

    /// Write a pristine foreign file directly into the game directory.
    fn write_foreign(&self, rel: &str, content: &[u8]) {
        let path = rel_join(&self.target_root, rel);
        fs::create_dir_all(path.parent().unwrap()).unwrap();
        fs::write(path, content).unwrap();
    }

    /// Stage a mod: write its files into the store, insert its row, and return
    /// the `(ModId, files)` pair the planner consumes.
    fn add_mod(&mut self, files: &[(&str, &[u8])]) -> (ModId, Vec<ResolvedFile>) {
        let id = self.next_mod;
        self.next_mod = self.next_mod.checked_add(1).unwrap();
        let staged = self.store.join(format!("mod{id}"));
        let resolved = files
            .iter()
            .map(|(rel, content)| {
                let source = rel_join(&staged, rel);
                fs::create_dir_all(source.parent().unwrap()).unwrap();
                fs::write(&source, content).unwrap();
                ResolvedFile {
                    target_rel: (*rel).to_owned(),
                    source,
                }
            })
            .collect();
        self.conn
            .execute(
                "INSERT INTO mods (id, game_id, name, staged_path) VALUES (?1, 1, ?2, ?3)",
                rusqlite::params![id, format!("mod{id}"), staged.to_string_lossy()],
            )
            .unwrap();
        (ModId::from_raw(id), resolved)
    }

    fn current_rows(&self) -> Vec<manifest::DeployedRow> {
        manifest::current_deployment(&self.conn, self.game).unwrap()
    }

    fn deploy(
        &self,
        ordered: &[(ModId, Vec<ResolvedFile>)],
        faults: &Faults,
    ) -> crate::Result<crate::DeployReport> {
        let current = self.current_rows();
        let p = plan(
            self.game,
            self.target_root.clone(),
            self.paths.backup_root(),
            ordered,
            &current,
        );
        apply(&self.conn, &self.paths, &p, self.profile, faults)
    }

    fn undeploy(&self, faults: &Faults) -> crate::Result<crate::DeployReport> {
        self.deploy(&[], faults)
    }

    fn tree(&self) -> BTreeMap<String, Vec<u8>> {
        read_tree(&self.target_root)
    }

    fn recover(&self) -> journal::Recovered {
        journal::recover(&self.conn, &self.paths).unwrap()
    }

    fn journal_files_gone(&self) -> bool {
        !self.paths.journal_file().exists() && !self.paths.commit_file().exists()
    }
}

/// Join a `/`-separated relative path onto a base.
fn rel_join(base: &Path, rel: &str) -> PathBuf {
    let mut p = base.to_path_buf();
    for c in rel.split('/').filter(|c| !c.is_empty()) {
        p.push(c);
    }
    p
}

/// Read a directory tree into `relative-path → content`, following links so a
/// hardlink/symlink/copy of the same bytes compares equal. Bounded by an
/// explicit stack with a depth ceiling (no recursion over untrusted trees).
fn read_tree(root: &Path) -> BTreeMap<String, Vec<u8>> {
    let mut out = BTreeMap::new();
    let mut stack = vec![(root.to_path_buf(), 0_usize)];
    while let Some((dir, depth)) = stack.pop() {
        assert!(depth < 64, "tree deeper than the test bound");
        let Ok(entries) = fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push((path, depth.checked_add(1).unwrap()));
            } else {
                let rel = path
                    .strip_prefix(root)
                    .unwrap()
                    .to_string_lossy()
                    .replace('\\', "/");
                out.insert(rel, fs::read(&path).unwrap());
            }
        }
    }
    out
}

// ---------------------------------------------------------------------------
// I2 - idempotence: applying the same plan twice equals applying it once.
// ---------------------------------------------------------------------------
#[test]
fn i2_idempotent_redeploy_is_a_noop() {
    let mut h = Harness::new();
    let a = h.add_mod(&[("a.esp", b"aaa"), ("meshes/x.nif", b"xxx")]);
    let b = h.add_mod(&[("b.esp", b"bbb")]);
    let ordered = vec![a, b];

    let first = h.deploy(&ordered, &Faults::none()).unwrap();
    assert_eq!(first.added(), 3);
    let after_first = h.tree();

    let second = h.deploy(&ordered, &Faults::none()).unwrap();
    assert_eq!(second.added(), 0);
    assert_eq!(second.removed(), 0);
    assert_eq!(second.unchanged(), 3);
    assert_eq!(h.tree(), after_first);
}

// ---------------------------------------------------------------------------
// I1 - reversibility: deploy then undeploy ⇒ pristine game directory.
// ---------------------------------------------------------------------------
#[test]
fn i1_deploy_then_undeploy_restores_pristine_tree() {
    let mut h = Harness::new();
    // Pristine game files, one of which a mod will overwrite.
    h.write_foreign("Data/original.esp", b"ORIGINAL");
    h.write_foreign("Data/keep.txt", b"KEEP");
    let pristine = h.tree();

    let a = h.add_mod(&[("Data/original.esp", b"MODDED"), ("Data/new.esp", b"NEW")]);
    let b = h.add_mod(&[("Data/another.esp", b"ANOTHER")]);

    h.deploy(&[a, b], &Faults::none()).unwrap();
    // The overwrite really happened...
    assert_eq!(h.tree().get("Data/original.esp").unwrap(), b"MODDED");

    h.undeploy(&Faults::none()).unwrap();
    assert_eq!(
        h.tree(),
        pristine,
        "undeploy must restore the pristine tree"
    );
    assert!(
        h.current_rows().is_empty(),
        "manifest cleared after undeploy"
    );
}

// ---------------------------------------------------------------------------
// I3 - no silent clobber: originals are backed up; user edits are not destroyed.
// ---------------------------------------------------------------------------
#[test]
fn i3_foreign_original_is_backed_up_and_restorable() {
    let mut h = Harness::new();
    h.write_foreign("Data/x.esp", b"PRISTINE");
    let a = h.add_mod(&[("Data/x.esp", b"OVERRIDE")]);

    h.deploy(&[a], &Faults::none()).unwrap();
    assert_eq!(h.tree().get("Data/x.esp").unwrap(), b"OVERRIDE");
    // The pristine original survives in the content-addressed backup store.
    let backups = read_tree(&h.paths.backup_root());
    assert!(
        backups.values().any(|v| v == b"PRISTINE"),
        "displaced original must be backed up"
    );

    h.undeploy(&Faults::none()).unwrap();
    assert_eq!(h.tree().get("Data/x.esp").unwrap(), b"PRISTINE");
}

#[test]
fn i3_user_modified_deployed_file_is_not_clobbered() {
    let mut h = Harness::new();
    let a = h.add_mod(&[("Data/n.esp", b"MOD")]);
    h.deploy(&[a], &Faults::none()).unwrap();

    // The user edits our deployed file (breaking the hardlink with a fresh write).
    let target = rel_join(&h.target_root, "Data/n.esp");
    fs::remove_file(&target).unwrap();
    fs::write(&target, b"USER EDIT").unwrap();

    let report = h.undeploy(&Faults::none()).unwrap();
    assert_eq!(report.skipped_modified(), 1);
    assert_eq!(
        h.tree().get("Data/n.esp").unwrap(),
        b"USER EDIT",
        "a user-modified file must be left exactly as the user left it"
    );
}

// ---------------------------------------------------------------------------
// I4 - crash safety: a fault at *any* step recovers to fully-back or fully-fwd.
// ---------------------------------------------------------------------------
#[test]
fn i4_crash_at_every_step_recovers_cleanly() {
    // Reference pristine state and fully-deployed state (content maps are
    // independent of the per-harness temp paths).
    let pristine = {
        let mut h = Harness::new();
        setup_scenario(&mut h);
        h.tree()
    };
    let deployed = {
        let mut h = Harness::new();
        let ordered = setup_scenario(&mut h);
        h.deploy(&ordered, &Faults::none()).unwrap();
        h.tree()
    };
    assert_ne!(pristine, deployed, "scenario must actually change the tree");

    let mut covered_all = false;
    for step in 1..=256_usize {
        let mut h = Harness::new();
        let ordered = setup_scenario(&mut h);
        let faults = Faults::failing_at(step);
        let result = h.deploy(&ordered, &faults);

        if !faults.fired() && result.is_ok() {
            covered_all = true;
            break; // step exceeded the number of checkpoints: all crash points done
        }

        let recovered = h.recover();
        let tree = h.tree();
        assert!(
            h.journal_files_gone(),
            "recovery must leave no journal (step {step})"
        );
        // Every crash lands on exactly one of the two clean states.
        let rolled_back = tree == pristine && h.current_rows().is_empty();
        let rolled_forward = tree == deployed && !h.current_rows().is_empty();
        assert!(
            rolled_back || rolled_forward,
            "step {step}: tree is neither pristine nor fully deployed (recovered: {recovered:?})"
        );
    }
    assert!(
        covered_all,
        "the fault loop never exhausted all checkpoints"
    );
}

/// A scenario with a foreign original that gets overwritten, plus fresh files
/// and a second mod - enough moving parts to exercise backup, replace, and add.
fn setup_scenario(h: &mut Harness) -> Vec<(ModId, Vec<ResolvedFile>)> {
    h.write_foreign("Data/original.esp", b"ORIGINAL");
    h.write_foreign("Data/untouched.txt", b"UNTOUCHED");
    let a = h.add_mod(&[
        ("Data/original.esp", b"OVERRIDE"),
        ("Data/mesh/a.nif", b"AAA"),
    ]);
    let b = h.add_mod(&[("Data/b.esp", b"BBB")]);
    vec![a, b]
}

// ---------------------------------------------------------------------------
// I1 + I2 as properties over random foreign files and random overlapping mods.
// ---------------------------------------------------------------------------
use proptest::prelude::*;

/// Targets shared by foreign files and mods so overlaps (and thus backups,
/// replaces, and conflicts) happen often.
const POOL: &[&str] = &["a.esp", "b.esp", "c.esp", "sub/d.esp"];

fn content() -> impl Strategy<Value = Vec<u8>> {
    prop::collection::vec(any::<u8>(), 1..6)
}

/// A `(target index, content)` pair.
type Entry = (usize, Vec<u8>);
/// A generated scenario: pre-existing foreign files, then the mods to deploy.
type Scenario = (Vec<Entry>, Vec<Vec<Entry>>);

/// One `(target index, content)` pair.
fn entry() -> impl Strategy<Value = Entry> {
    (0..POOL.len(), content())
}

/// A random scenario: `(foreign files, mods)`, each item a `(target index,
/// content)` pair.
fn scenario() -> impl Strategy<Value = Scenario> {
    let foreign = prop::collection::vec(entry(), 0..4);
    let mods = prop::collection::vec(prop::collection::vec(entry(), 0..4), 1..4);
    (foreign, mods)
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// Whatever the mods and pre-existing files, a deploy is idempotent (I2) and
    /// an undeploy restores the exact pristine tree (I1).
    #[test]
    fn prop_deploy_is_idempotent_and_reversible((foreign, mods) in scenario()) {
        let mut h = Harness::new();
        for (idx, bytes) in &foreign {
            h.write_foreign(POOL[*idx], bytes);
        }
        let pristine = h.tree();

        let mut ordered = Vec::new();
        for files in &mods {
            let named: Vec<(&str, &[u8])> = files
                .iter()
                .map(|(idx, bytes)| (POOL[*idx], bytes.as_slice()))
                .collect();
            ordered.push(h.add_mod(&named));
        }

        h.deploy(&ordered, &Faults::none()).unwrap();
        let deployed = h.tree();

        // I2: applying the same plan again changes nothing.
        let again = h.deploy(&ordered, &Faults::none()).unwrap();
        prop_assert_eq!(again.added(), 0);
        prop_assert_eq!(again.removed(), 0);
        prop_assert_eq!(h.tree(), deployed);

        // I1: undeploy is an exact inverse.
        h.undeploy(&Faults::none()).unwrap();
        prop_assert_eq!(h.tree(), pristine);
        prop_assert!(h.current_rows().is_empty());
    }
}
