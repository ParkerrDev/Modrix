// SPDX-License-Identifier: GPL-2.0-only
//! Detecting that the game's IL2CPP runtime exists - **without calling it**.
//!
//! # Why there is no `il2cpp_domain_get` probe here
//!
//! The obvious readiness check is to poll `il2cpp_domain_get` until it returns
//! non-null: it is an exported accessor, and reading a static that is null
//! before `il2cpp_init` looks harmless.
//!
//! It is not. Doing it kills the game, with a message box reading
//! **"Collecting from unknown thread"**. IL2CPP's garbage collector maintains a
//! registry of threads it is allowed to see, built as the runtime starts. A
//! call that reaches the runtime from a thread absent from that registry
//! aborts the process. Our bootstrap thread is necessarily absent: the GC was
//! initialised before the thread existed, and registering it means calling
//! `il2cpp_thread_attach`, which needs the very domain pointer we were trying
//! to fetch.
//!
//! This was isolated by bisection, not deduction. With the proxy stopping
//! after "GameAssembly.dll mapped" the game runs to 304 log lines and no
//! crash; letting it make the single accessor call drops that to 35 lines and
//! a crash. Waiting longer changes nothing - the thread is wrong, not early.
//!
//! So this module only ever asks the *Windows loader* a question, never the
//! runtime. `GetModuleHandleW` reports whether `GameAssembly.dll` is mapped,
//! which is all the proxy needs: it proves the game is far enough along to host
//! CoreCLR, and CoreCLR hosting is completely independent of IL2CPP anyway.
//!
//! Managed code that wants to touch IL2CPP must attach properly, from the
//! game's own main thread, which is the SDK's job and not the proxy's.

use core::time::Duration;
use std::time::Instant;

/// Module that carries the IL2CPP runtime in a Unity IL2CPP build. Its presence
/// is the signal; its exports are deliberately never called from here.
const GAME_ASSEMBLY: &str = "GameAssembly.dll";

/// Blocks until `GameAssembly.dll` is mapped, returning its base address.
///
/// Pure observation: this calls `GetModuleHandleW` and nothing else, so it
/// cannot perturb the runtime it is waiting for.
#[must_use]
pub fn wait_for_module(timeout: Duration, poll_interval: Duration) -> Option<usize> {
    let started = Instant::now();
    loop {
        if let Some(handle) = crate::win::module_handle(GAME_ASSEMBLY) {
            return Some(handle as usize);
        }
        if started.elapsed() >= timeout {
            return None;
        }
        std::thread::sleep(poll_interval);
    }
}
