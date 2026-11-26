// SPDX-License-Identifier: GPL-2.0-only
//! Starting the embedded service, game discovery, the community registry and
//! the updater. Everything slow runs on a worker thread or the async runtime.

use std::path::Path;
use std::sync::Arc;

use modrix_core::{Engine, GameDef, Paths};
use modrix_service::{Binding, Service};

use crate::bus::{Bus, Event};
use crate::state::{
    App, DefChoice, Detected, InstalledPlugin, Link, RegistryChoice, Tone, UpdateState,
};

/// Simultaneously active downloads (matches `modrix serve`).
const MAX_CONCURRENT: u8 = 4;

/// Everything boot hands back.
pub struct Booted {
    /// The running service.
    pub service: Service,
    /// Pairing, when this process is primary.
    pub link: Option<Link>,
    /// Selectable definitions.
    pub defs: Vec<DefChoice>,
    /// Supported games found installed.
    pub detected: Vec<Detected>,
    /// Resolved locations.
    pub paths: Paths,
}

/// Boot on a worker thread inside the runtime (the listener and stager
/// spawn onto it), posting [`Event::Booted`] when done.
pub fn start(rt: tokio::runtime::Handle, progress: Arc<modrix_core::Progress>, bus: Bus) {
    let spawned = std::thread::Builder::new()
        .name("modrix-boot".to_owned())
        .spawn(move || {
            let _guard = rt.enter();
            let result = start_service(&progress).map(Box::new);
            bus.send(Event::Booted(result));
        });
    if let Err(error) = spawned {
        tracing::error!(%error, "could not start the boot thread");
    }
}

fn start_service(progress: &Arc<modrix_core::Progress>) -> Result<Booted, String> {
    let paths = Paths::resolve().map_err(|e| e.to_string())?;
    let mut engine =
        Engine::open_with_progress(&paths, Arc::clone(progress)).map_err(|e| e.to_string())?;
    modrix_plugin::register_lua_logic(&mut engine);
    let defs = discover_defs(&paths);
    let detected = detect_installs(&defs);
    let service = Service::new(engine, MAX_CONCURRENT).map_err(|e| format!("{e:#}"))?;
    let link = match service
        .bind(&paths.instance_lock(), modrix_ipc::DEFAULT_PORT)
        .map_err(|e| format!("{e:#}"))?
    {
        Binding::Primary { port, token, .. } => Some(Link { port, token }),
        Binding::AlreadyRunning => None,
    };
    Ok(Booted {
        service,
        link,
        defs,
        detected,
        paths,
    })
}

/// Every definition this installation can register a game from.
#[must_use]
pub fn discover_defs(paths: &Paths) -> Vec<DefChoice> {
    let mut defs: Vec<DefChoice> = modrix_core::defcat::discover_defs(paths)
        .into_iter()
        .map(|entry| DefChoice {
            id: entry.def.id,
            name: entry.def.name,
            toml: entry.toml,
        })
        .collect();
    defs.sort_by_key(|d| d.name.to_lowercase());
    defs
}

/// Probe each definition's install detection (best-effort Steam metadata).
#[must_use]
pub fn detect_installs(defs: &[DefChoice]) -> Vec<Detected> {
    defs.iter()
        .filter_map(|choice| {
            let def = GameDef::from_toml_str(&choice.toml, Path::new("<definition>")).ok()?;
            let install = modrix_core::detect::detect_install(&def)?;
            Some(Detected {
                def: choice.clone(),
                install,
            })
        })
        .collect()
}

impl App {
    /// Rescan definitions, detection and installed plugins after a change.
    pub fn rediscover(&mut self) {
        let Some(paths) = self.paths.clone() else {
            return;
        };
        self.defs = discover_defs(&paths);
        self.detected = detect_installs(&self.defs);
        self.refresh_installed_plugins();
    }

    /// Re-read the plugins installed from the registry.
    pub fn refresh_installed_plugins(&mut self) {
        let Some(paths) = &self.paths else {
            return;
        };
        self.installed_plugins = modrix_registry::installed_at(paths)
            .into_iter()
            .map(|m| InstalledPlugin {
                id: m.id,
                name: m.name,
                version: m.version,
            })
            .collect();
    }

    /// Load the registry index in the background (offline leaves it empty).
    pub fn fetch_registry(&mut self) {
        let (Some(rt), Some(paths)) = (&self.rt, self.paths.clone()) else {
            return;
        };
        self.registry_loading = true;
        let bus = self.bus.clone();
        rt.spawn(async move {
            bus.send(Event::Registry(registry_index(paths).await));
        });
    }

    /// Registry entries whose support is not installed yet.
    #[must_use]
    pub fn available_support(&self) -> Vec<(usize, &RegistryChoice)> {
        self.registry
            .iter()
            .enumerate()
            .filter(|(_, r)| !self.defs.iter().any(|d| d.id == r.id))
            .collect()
    }

    /// Install game support from the registry.
    pub fn install_support(&mut self, index: usize) {
        let (Some(rt), Some(paths)) = (&self.rt, self.paths.clone()) else {
            return;
        };
        let Some(choice) = self.registry.get(index).cloned() else {
            return;
        };
        self.busy = self.busy.saturating_add(1);
        let bus = self.bus.clone();
        rt.spawn(async move {
            let result = async {
                let client = registry_client(&paths)?;
                client
                    .install(&choice.entry)
                    .await
                    .map_err(|e| e.to_string())?;
                Ok(choice.id)
            }
            .await;
            bus.send(Event::SupportInstalled(result));
        });
    }

    /// Remove an installed registry plugin.
    pub fn uninstall_plugin(&mut self, id: &str) {
        let Some(paths) = self.paths.clone() else {
            return;
        };
        let referenced: Vec<String> = self.lib.games.iter().map(|g| g.plugin_id.clone()).collect();
        match registry_client(&paths)
            .and_then(|c| c.uninstall(id, &referenced).map_err(|e| e.to_string()))
        {
            Ok(()) => self.note(Tone::Ok, format!("{id} removed")),
            Err(error) => self.note(Tone::Error, error),
        }
        self.rediscover();
    }

    /// Remove every plugin no registered game uses.
    pub fn gc_plugins(&mut self) {
        let Some(paths) = self.paths.clone() else {
            return;
        };
        let referenced: Vec<String> = self.lib.games.iter().map(|g| g.plugin_id.clone()).collect();
        match registry_client(&paths).and_then(|c| c.gc(&referenced).map_err(|e| e.to_string())) {
            Ok(removed) if removed.is_empty() => self.note(Tone::Info, "No unused plugins"),
            Ok(removed) => self.note(Tone::Ok, format!("Removed unused: {}", removed.join(", "))),
            Err(error) => self.note(Tone::Error, error),
        }
        self.rediscover();
    }

    /// Check GitHub Releases for a newer build.
    pub fn check_updates(&mut self) {
        let Some(rt) = &self.rt else {
            return;
        };
        self.update = UpdateState::Checking;
        let bus = self.bus.clone();
        rt.spawn(async move {
            let result = modrix_update::check(env!("CARGO_PKG_VERSION"))
                .await
                .map_err(|e| e.to_string());
            bus.send(Event::UpdateChecked(result));
        });
    }

    /// Download and launch the pending update's installer.
    pub fn start_update(&mut self) {
        let UpdateState::Available(info) = &self.update else {
            return;
        };
        let info = info.clone();
        if !info.can_self_install() {
            self.note(
                Tone::Info,
                "Updates install manually on this platform: see the release page.",
            );
            return;
        }
        let (Some(rt), Some(paths)) = (&self.rt, &self.paths) else {
            return;
        };
        let dir = paths.cache_dir().join("updates");
        self.update = UpdateState::Installing(info.clone());
        let bus = self.bus.clone();
        rt.spawn(async move {
            let result = async {
                let installer = modrix_update::download(&info, &dir)
                    .await
                    .map_err(|e| e.to_string())?;
                modrix_update::apply(&installer).map_err(|e| e.to_string())
            }
            .await;
            bus.send(Event::UpdateApplied(result));
        });
        self.note(Tone::Info, "Downloading the update");
    }
}

fn registry_client(paths: &Paths) -> Result<modrix_registry::RegistryClient, String> {
    modrix_registry::RegistryClient::new(modrix_registry::RegistrySource::resolve(), paths)
        .map_err(|e| e.to_string())
}

async fn registry_index(paths: Paths) -> Result<Vec<RegistryChoice>, String> {
    let client = registry_client(&paths)?;
    let index = client.index(false).await.map_err(|e| e.to_string())?;
    Ok(index
        .plugins
        .into_iter()
        .map(|entry| RegistryChoice {
            id: entry.id.clone(),
            name: entry.name.clone(),
            version: entry.version.clone(),
            entry,
        })
        .collect())
}
