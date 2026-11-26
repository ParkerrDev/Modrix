// SPDX-License-Identifier: GPL-2.0-only
//! The community mod store a game can declare (`[store]` in its
//! definition). The catalog is fetched on the async runtime; an install
//! resolves the mod and its dependencies into hand-off jobs and queues them
//! on the service, so a store install is the same download, stage and
//! route path a browser click takes.

use std::collections::HashSet;

use modrix_core::GameId;
use modrix_service::store::{SOTF_MODS_KIND, StoreClient, StoreItem, installed_keys};

use crate::bus::Event;
use crate::state::{App, Tone};

/// What the Store screen shows.
#[derive(Debug, Default)]
pub struct StoreView {
    /// The game the catalog belongs to.
    pub game: Option<GameId>,
    /// A fetch is running.
    pub loading: bool,
    /// Installable items, most downloaded first.
    pub items: Vec<StoreItem>,
    /// The last fetch's error.
    pub error: Option<String>,
    /// Mod ids whose install is being resolved.
    pub installing: HashSet<String>,
}

/// The installs a store resolve queued.
#[derive(Debug)]
pub struct Queued {
    /// The requested mod's name.
    pub name: String,
    /// Downloads queued, dependencies included.
    pub jobs: usize,
    /// Dependencies the catalog lacks.
    pub missing: Vec<String>,
}

impl App {
    /// The selected game's store origin, when its client exists here.
    fn store_origin(&self) -> Option<String> {
        self.lib
            .store
            .as_ref()
            .filter(|d| d.kind == SOTF_MODS_KIND)
            .map(|d| d.base_url.clone())
    }

    /// Fetch the catalog unless the selected game's is loaded or loading.
    pub fn open_store(&mut self) {
        let loaded = self.store.game == self.lib.game && !self.store.items.is_empty();
        if !self.store.loading && !loaded {
            self.fetch_store();
        }
    }

    /// Fetch the selected game's catalog in the background.
    pub fn fetch_store(&mut self) {
        let (Some(rt), Some(base), Some(game)) = (&self.rt, self.store_origin(), self.lib.game)
        else {
            return;
        };
        self.store = StoreView {
            game: Some(game),
            loading: true,
            ..StoreView::default()
        };
        let bus = self.bus.clone();
        rt.spawn(async move {
            let result = match StoreClient::new(&base) {
                Ok(client) => client.catalog().await.map(|c| c.items),
                Err(error) => Err(error),
            };
            bus.send(Event::StoreCatalog(
                game,
                result.map_err(|e| format!("{e:#}")),
            ));
        });
    }

    /// Fold a fetched catalog in, if it is still the selected game's.
    pub fn on_store_catalog(&mut self, game: GameId, result: Result<Vec<StoreItem>, String>) {
        if self.store.game != Some(game) {
            return;
        }
        self.store.loading = false;
        match result {
            Ok(items) => self.store.items = items,
            Err(error) => self.store.error = Some(error),
        }
    }

    /// Install `mod_id` and what it needs through the download queue.
    pub fn store_install(&mut self, mod_id: String) {
        let domain = self.game().and_then(|g| g.nexus_domain.clone());
        let (Some(rt), Some(base)) = (self.rt.clone(), self.store_origin()) else {
            return;
        };
        if !self.store.installing.insert(mod_id.clone()) {
            return;
        }
        let installed = installed_keys(self.lib.mods.iter().map(|m| m.name.as_str()));
        let service = self.service.clone();
        let bus = self.bus.clone();
        rt.spawn(async move {
            let result = resolve_and_queue(&base, &mod_id, &installed, domain, service).await;
            bus.send(Event::StoreQueued(mod_id, result));
        });
    }

    /// Report a finished store resolve.
    pub fn on_store_queued(&mut self, mod_id: &str, result: Result<Queued, String>) {
        self.store.installing.remove(mod_id);
        match result {
            Ok(q) if q.missing.is_empty() => self.note(
                Tone::Ok,
                format!("Queued {} ({} downloads)", q.name, q.jobs),
            ),
            Ok(q) => self.note(
                Tone::Info,
                format!(
                    "Queued {}, but the store lacks {}: it may not load",
                    q.name,
                    q.missing.join(", ")
                ),
            ),
            Err(error) => self.note(Tone::Error, error),
        }
    }
}

async fn resolve_and_queue(
    base: &str,
    mod_id: &str,
    installed: &HashSet<String>,
    domain: Option<String>,
    service: Option<modrix_service::Service>,
) -> Result<Queued, String> {
    let service = service.ok_or("the service is not running")?;
    let client = StoreClient::new(base).map_err(|e| format!("{e:#}"))?;
    let (jobs, plan) = client
        .install_jobs(mod_id, installed, domain.as_deref())
        .await
        .map_err(|e| format!("{e:#}"))?;
    let name = plan
        .items
        .last()
        .map_or_else(|| mod_id.to_owned(), |i| i.name.clone());
    let count = jobs.len();
    for job in jobs {
        service.enqueue_job(job).map_err(|e| format!("{e:#}"))?;
    }
    Ok(Queued {
        name,
        jobs: count,
        missing: plan.missing,
    })
}
