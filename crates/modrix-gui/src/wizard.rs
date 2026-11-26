// SPDX-License-Identifier: GPL-2.0-only
//! The FOMOD options wizard: opening it (parsed on the engine thread),
//! picking options with each group's rule, paging, applying and dismissing.

use modrix_core::{Engine, ModId};
use modrix_plugin::fomod;

use crate::bus::Reply;
use crate::state::{App, Tone, Wizard};

impl App {
    /// Pop the next queued mod into the wizard, if none is open.
    pub fn open_next_wizard(&mut self) {
        if self.wizard.is_some() || self.wizard_queue.is_empty() {
            return;
        }
        let id = self.wizard_queue.remove(0);
        self.configure(id);
    }

    /// Open the wizard for a mod (parsing runs on the engine thread).
    pub fn configure(&mut self, id: ModId) {
        let Some(m) = self.lib.mods.iter().find(|m| m.id == id).cloned() else {
            return;
        };
        self.run(
            false,
            Box::new(move |e| {
                let present = modrix_service::present_files(e, m.game_id);
                match fomod::parse(&m.staged_path) {
                    Ok(Some(installer)) => {
                        let selections = fomod::defaults(&installer, &present);
                        Ok(Reply::Wizard(Box::new(Wizard {
                            mod_id: m.id,
                            mod_name: m.name,
                            staged_path: m.staged_path,
                            installer,
                            selections,
                            step: 0,
                            focus: None,
                            present,
                        })))
                    }
                    Ok(None) => Ok(Reply::Note(
                        Tone::Info,
                        "No installer options in this mod".to_owned(),
                    )),
                    Err(error) => Err(error.to_string()),
                }
            }),
        );
    }

    /// Check every usable option in a group, or clear all but Required.
    pub fn wizard_group_set(&mut self, step: usize, group: usize, all: bool) {
        let Some(wizard) = &mut self.wizard else {
            return;
        };
        let flags = fomod::flags_of(&wizard.installer, &wizard.selections, &wizard.present);
        let Some(g) = wizard
            .installer
            .steps
            .get(step)
            .and_then(|s| s.groups.get(group))
        else {
            return;
        };
        if g.kind == fomod::GroupKind::All {
            return;
        }
        let picked: std::collections::BTreeSet<usize> = g
            .plugins
            .iter()
            .enumerate()
            .filter(|(_, p)| {
                let kind = fomod::plugin_kind(&p.kind, &flags, &wizard.present);
                if all {
                    kind != fomod::PluginKind::NotUsable
                } else {
                    kind == fomod::PluginKind::Required
                }
            })
            .map(|(i, _)| i)
            .collect();
        if let Some(sel) = wizard
            .selections
            .get_mut(step)
            .and_then(|s| s.get_mut(group))
        {
            *sel = picked;
        }
    }

    /// Toggle an option by its group's rule.
    pub fn wizard_pick(&mut self, step: usize, group: usize, plugin: usize) {
        let Some(wizard) = &mut self.wizard else {
            return;
        };
        wizard.focus = Some((step, group, plugin));
        let Some(kind) = wizard
            .installer
            .steps
            .get(step)
            .and_then(|s| s.groups.get(group))
            .map(|g| g.kind)
        else {
            return;
        };
        if let Some(sel) = wizard
            .selections
            .get_mut(step)
            .and_then(|s| s.get_mut(group))
        {
            pick(kind, sel, plugin);
        }
    }

    /// Next (+1) or previous (-1) page.
    pub fn wizard_step(&mut self, delta: i8) {
        let Some(wizard) = &mut self.wizard else {
            return;
        };
        let pages = wizard.visible_steps().len().max(1);
        wizard.step = crate::actions::step(wizard.step, delta, pages);
    }

    /// Apply the chosen options.
    pub fn wizard_finish(&mut self) {
        let Some(wizard) = self.wizard.take() else {
            return;
        };
        self.run(true, Box::new(move |e| apply(e, &wizard)));
        self.open_next_wizard();
    }

    /// Close without applying: the defaults stand and the mod counts as
    /// reviewed, so it is not offered again.
    pub fn wizard_cancel(&mut self) {
        if let Some(wizard) = self.wizard.take() {
            let id = wizard.mod_id;
            self.run(
                false,
                Box::new(move |e| {
                    e.set_install_state(id, "fomod")
                        .map(|()| Reply::Quiet)
                        .map_err(|e| e.to_string())
                }),
            );
        }
        self.open_next_wizard();
    }
}

/// One option press under a group rule.
pub fn pick(kind: fomod::GroupKind, sel: &mut std::collections::BTreeSet<usize>, plugin: usize) {
    match kind {
        fomod::GroupKind::All => {}
        fomod::GroupKind::ExactlyOne => {
            sel.clear();
            sel.insert(plugin);
        }
        fomod::GroupKind::AtMostOne => {
            let had = sel.remove(&plugin);
            sel.clear();
            if !had {
                sel.insert(plugin);
            }
        }
        fomod::GroupKind::Any => {
            if !sel.remove(&plugin) {
                sel.insert(plugin);
            }
        }
        fomod::GroupKind::AtLeastOne => {
            if !sel.contains(&plugin) {
                sel.insert(plugin);
            } else if sel.len() > 1 {
                sel.remove(&plugin);
            }
        }
    }
}

fn apply(e: &Engine, wizard: &Wizard) -> Result<Reply, String> {
    let ops = fomod::resolve(&wizard.installer, &wizard.selections, &wizard.present);
    let placed = fomod::apply(&wizard.staged_path, &ops).map_err(|e| e.to_string())?;
    e.set_install_state(wizard.mod_id, "fomod")
        .map_err(|e| e.to_string())?;
    Ok(Reply::Note(
        Tone::Ok,
        format!(
            "Options applied to {}: {placed} files, deploy to update",
            wizard.mod_name
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    #[test]
    fn group_rules() {
        let mut sel = BTreeSet::from([0]);
        pick(fomod::GroupKind::ExactlyOne, &mut sel, 2);
        assert_eq!(sel, BTreeSet::from([2]));
        pick(fomod::GroupKind::AtMostOne, &mut sel, 2);
        assert!(sel.is_empty());
        pick(fomod::GroupKind::AtLeastOne, &mut sel, 1);
        pick(fomod::GroupKind::AtLeastOne, &mut sel, 1);
        assert_eq!(sel, BTreeSet::from([1]));
        pick(fomod::GroupKind::Any, &mut sel, 3);
        assert_eq!(sel, BTreeSet::from([1, 3]));
    }
}
