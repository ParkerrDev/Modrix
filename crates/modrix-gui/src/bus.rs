// SPDX-License-Identifier: GPL-2.0-only
//! Background work. One engine thread runs every engine call in FIFO order
//! (quick mutations, slow deploys, and the refresh queries alike), so the UI
//! thread never waits on the engine lock or the disk. Results come back as
//! [`Event`]s through a channel, and a waker asks Rune for a frame whose
//! `poll` drains them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};

use modrix_core::{Engine, GameId, ModId};

use crate::art::ArtData;
use crate::state::{DupPrompt, Library, RegistryChoice, Tone, Wizard};

/// What a finished engine job asks the UI to do.
#[derive(Debug)]
pub enum Reply {
    /// Nothing to report.
    Quiet,
    /// A notification.
    Note(Tone, String),
    /// An archive is already installed: ask before reinstalling.
    Duplicate(DupPrompt),
    /// Open the options wizard.
    Wizard(Box<Wizard>),
    /// A game was registered or picked: manage it.
    SelectGame(GameId, String),
}

/// An engine call run on the engine thread.
pub type Work = Box<dyn FnOnce(&Engine) -> Result<Reply, String> + Send>;

/// One unit of engine-thread work.
pub enum Job {
    /// Re-read engine state for `game` (None: the active game).
    Refresh {
        /// Game to read.
        game: Option<GameId>,
        /// Also run the expensive pass (plugins, health, conflicts, external).
        heavy: bool,
    },
    /// Run an engine call, then refresh.
    Run {
        /// Counts toward the busy indicator.
        slow: bool,
        /// The call.
        work: Work,
        /// Game to refresh afterwards.
        game: Option<GameId>,
    },
}

/// Events delivered to the UI thread.
pub enum Event {
    /// The service finished starting (or failed to).
    Booted(Result<Box<crate::boot::Booted>, String>),
    /// Fresh engine state, gathered after job `seq`.
    Snapshot {
        /// Last job sequence number processed before gathering.
        seq: u64,
        /// The data.
        lib: Box<Library>,
        /// Whether the heavy pass ran.
        heavy: bool,
    },
    /// A job finished.
    Done {
        /// Whether it was a slow job.
        slow: bool,
        /// Its reply.
        reply: Result<Reply, String>,
    },
    /// Staged plugin counts.
    PluginCounts(HashMap<ModId, usize>),
    /// Archives hashed: install or prompt.
    Hashed(Vec<(PathBuf, Option<String>)>),
    /// The registry index loaded.
    Registry(Result<Vec<RegistryChoice>, String>),
    /// A registry plugin install finished.
    SupportInstalled(Result<String, String>),
    /// The update check finished.
    UpdateChecked(Result<Option<modrix_update::UpdateInfo>, String>),
    /// The update installer launched (or failed to).
    UpdateApplied(Result<(), String>),
    /// A game's artwork resolved.
    Art(GameId, Box<ArtData>),
}

/// The sending half of the event channel plus the frame waker.
#[derive(Clone)]
pub struct Bus {
    tx: Sender<Event>,
    waker: Option<rune::Waker>,
}

impl Bus {
    /// A new channel; the receiver lives in the app state.
    #[must_use]
    pub fn channel() -> (Bus, Receiver<Event>) {
        let (tx, rx) = channel();
        (Bus { tx, waker: None }, rx)
    }

    /// Attach the waker Rune hands `setup`.
    pub fn set_waker(&mut self, waker: rune::Waker) {
        self.waker = Some(waker);
    }

    /// Post an event and wake the UI. A closed channel means the window is
    /// gone, so a failed send is simply dropped.
    pub fn send(&self, event: Event) {
        if self.tx.send(event).is_ok()
            && let Some(wake) = &self.waker
        {
            wake();
        }
    }
}

/// Start the engine thread. It runs until the job sender is dropped (the
/// window closed), which ends `recv` and the thread with it.
#[must_use]
pub fn spawn_engine_thread(engine: Arc<Mutex<Engine>>, bus: Bus) -> Option<Sender<(u64, Job)>> {
    let (tx, rx) = channel::<(u64, Job)>();
    let spawned = std::thread::Builder::new()
        .name("modrix-engine".to_owned())
        .spawn(move || {
            let mut worker = Worker {
                engine,
                rx,
                bus,
                last_seq: 0,
            };
            worker.serve();
        });
    match spawned {
        Ok(_) => Some(tx),
        Err(error) => {
            tracing::error!(%error, "could not start the engine thread");
            None
        }
    }
}

/// The engine thread's state.
struct Worker {
    engine: Arc<Mutex<Engine>>,
    rx: Receiver<(u64, Job)>,
    bus: Bus,
    last_seq: u64,
}

impl Worker {
    fn serve(&mut self) {
        // Not a bounded loop by design: it serves jobs for the window's
        // lifetime and ends when the UI drops the sender.
        while let Ok((seq, job)) = self.rx.recv() {
            self.last_seq = self.last_seq.max(seq);
            let (game, heavy) = match job {
                Job::Refresh { game, heavy } => (game, heavy),
                Job::Run { slow, work, game } => {
                    self.run(slow, work);
                    (game, true)
                }
            };
            // Coalesce: a burst of refresh requests needs one read.
            let (game, heavy) = self.coalesce(game, heavy);
            self.gather_and_send(game, heavy);
        }
    }

    /// Drain queued work, running jobs in order and merging refreshes.
    fn coalesce(&mut self, mut game: Option<GameId>, mut heavy: bool) -> (Option<GameId>, bool) {
        for _ in 0..256 {
            let Ok((seq, job)) = self.rx.try_recv() else {
                break;
            };
            self.last_seq = self.last_seq.max(seq);
            match job {
                Job::Refresh { game: g, heavy: h } => {
                    game = g.or(game);
                    heavy |= h;
                }
                Job::Run {
                    slow,
                    work,
                    game: g,
                } => {
                    self.run(slow, work);
                    game = g.or(game);
                    heavy = true;
                }
            }
        }
        (game, heavy)
    }

    fn run(&self, slow: bool, work: Work) {
        let reply = match self.engine.lock() {
            Ok(guard) => work(&guard),
            Err(_) => Err("engine lock poisoned".to_owned()),
        };
        self.bus.send(Event::Done { slow, reply });
    }

    fn gather_and_send(&self, game: Option<GameId>, heavy: bool) {
        let Ok(guard) = self.engine.lock() else {
            return;
        };
        let lib = crate::sync::gather(&guard, game, heavy);
        drop(guard);
        let counts_for: Vec<(ModId, PathBuf)> = if heavy {
            lib.mods
                .iter()
                .map(|m| (m.id, m.staged_path.clone()))
                .collect()
        } else {
            Vec::new()
        };
        self.bus.send(Event::Snapshot {
            seq: self.last_seq,
            lib: Box::new(lib),
            heavy,
        });
        if !counts_for.is_empty() {
            let counts = crate::sync::plugin_counts(&counts_for);
            self.bus.send(Event::PluginCounts(counts));
        }
    }
}
