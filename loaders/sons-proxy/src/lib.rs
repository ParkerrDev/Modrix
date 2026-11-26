// SPDX-License-Identifier: GPL-2.0-only
//! `version.dll` proxy that boots a .NET 6 runtime inside Sons of the Forest.
//!
//! # What this is
//!
//! Sons of the Forest is an IL2CPP Unity build: its C# was compiled ahead of
//! time to native code, so there is no `Assembly-CSharp.dll` to patch and
//! nothing for a file-copying mod manager to do. Making it moddable at all
//! requires getting code into the running process. `UnityPlayer.dll` imports
//! `VERSION.dll`, so a DLL of that name next to the game executable is loaded by
//! Windows before the game's first frame. This crate is that DLL.
//!
//! # Milestone M0 - and only M0
//!
//! This crate proves one claim: **managed .NET code can run inside the game, and
//! the game still plays**. The chain is
//!
//! 1. `DLL_PROCESS_ATTACH` - forward our seventeen exports to the real
//!    `version.dll` so the game is unaffected, and fix the working directory,
//!    which Steam does not set to the game folder.
//! 2. A spawned thread waits for `GameAssembly.dll` to export a non-null
//!    `il2cpp_domain_get()`. No hooking - see [`il2cpp`].
//! 3. `coreclr_initialize` + `coreclr_create_delegate` against the vendored
//!    runtime in `_Modrix/dotnet` - see [`coreclr`].
//! 4. `Modrix.Loader.Entrypoint.Start()` writes the last line of
//!    `_Modrix/Latest.log`.
//!
//! Deliberately **not** here: `Il2CppInterop`, Dobby or any detour,
//! `il2cpp_runtime_invoke` hooking, the `Internal_ActiveSceneChanged` trigger,
//! mod discovery, `manifest.json`, the SDK, and installation. Those are M1-M3.
//!
//! # The unsafe policy
//!
//! The main Modrix workspace sets `unsafe_code = "forbid"`, and this crate is
//! outside it precisely so that stays true. In exchange, the rules here are
//! stricter than "unsafe is allowed":
//!
//! - `undocumented_unsafe_blocks` and `multiple_unsafe_ops_per_block` are
//!   **deny**, so every `unsafe` block does exactly one thing and says why it is
//!   sound.
//! - All FFI is confined to [`win`]; the rest of the crate is ordinary Rust.
//! - `panic = "abort"` in every profile, because unwinding into Unity's stack is
//!   undefined behaviour. Fallible paths return `Result` and log.
//! - `unwrap_used`, `panic`, `indexing_slicing` are denied exactly as in the
//!   main workspace.
//!
//! # Nothing here fails loudly on its own
//!
//! A DLL that fails inside a game process has no console, no stderr and no
//! dialog. Every step therefore reports into `_Modrix/Latest.log` and every
//! failure is swallowed after being logged: a broken loader must leave a
//! playable game behind, not a crash.

pub mod error;
pub mod exports;
pub mod layout;
pub mod log;

#[cfg(windows)]
pub(crate) mod coreclr;
#[cfg(windows)]
pub(crate) mod il2cpp;
#[cfg(windows)]
pub(crate) mod proxy;
#[cfg(windows)]
pub(crate) mod win;

#[cfg(windows)]
pub use attach::DllMain;

#[cfg(windows)]
mod attach {
    use crate::error::Error;
    use crate::layout::Layout;
    use core::ffi::c_void;
    use core::time::Duration;
    use windows_sys::Win32::Foundation::{BOOL, HMODULE, TRUE};

    /// `fdwReason` value Windows passes when the DLL is first mapped.
    const DLL_PROCESS_ATTACH: u32 = 1;

    /// How long to wait for IL2CPP. Sons of the Forest reaches `il2cpp_init` in
    /// a few seconds; two minutes covers a cold Proton prefix on a slow disk
    /// without hanging a thread for the life of the process.
    const IL2CPP_TIMEOUT: Duration = Duration::from_mins(2);
    /// Poll spacing. `il2cpp_domain_get` is a static read, so this is cheap.
    const IL2CPP_POLL: Duration = Duration::from_millis(100);

    /// How far the proxy is allowed to get, for bisecting a startup crash.
    ///
    /// Injecting into a game means a fault shows up as "the game closed", with
    /// no stack and no message. Rather than guess which step is at fault, set
    /// `MODRIX_PROXY_STAGE` and let the log say how far it got:
    ///
    /// | value | stops after |
    /// |-------|-------------|
    /// | 0 | forwarding the exports (proves the thunks are safe) |
    /// | 1 | + fixing the working directory |
    /// | 2 | + spawning the bootstrap thread |
    /// | 3 | + waiting for `GameAssembly.dll` to be mapped |
    /// | 4 | + calling `il2cpp_domain_get` |
    /// | 5 | everything (the default) |
    fn stage() -> u8 {
        std::env::var("MODRIX_PROXY_STAGE")
            .ok()
            .and_then(|raw| raw.trim().parse::<u8>().ok())
            .unwrap_or(5)
    }

    /// DLL entry point.
    ///
    /// Reached through mingw's `DllMainCRTStartup`, which is the PE entry point
    /// of the produced binary - verified by disassembling the cross-compiled
    /// DLL, not assumed.
    ///
    /// Only export forwarding and the working-directory fix happen here. Loading
    /// CoreCLR under the loader lock would be asking for a deadlock, so the real
    /// work is moved to a spawned thread that also gives the game time to bring
    /// IL2CPP up.
    ///
    /// # Safety
    ///
    /// Called by the Windows loader with the module's own instance handle. Never
    /// call it from Rust.
    #[unsafe(no_mangle)]
    pub unsafe extern "system" fn DllMain(
        instance: HMODULE,
        reason: u32,
        _reserved: *mut c_void,
    ) -> BOOL {
        if reason == DLL_PROCESS_ATTACH {
            on_process_attach(instance);
        }
        // Returning FALSE would fail the load and take the game with it. Every
        // failure above has already been logged.
        TRUE
    }

    /// Everything that must happen before the game's first import call.
    fn on_process_attach(instance: HMODULE) {
        let Ok(process_path) = std::env::current_exe() else {
            return;
        };
        let Ok(layout) = Layout::from_process_path(process_path) else {
            return;
        };
        // Only the game's own process may bootstrap, but EVERY process that loads
        // this DLL must still get its exports forwarded - it may have loaded us
        // because it genuinely wanted `version.dll`. So the export forwarding
        // below is unconditional and only the loader work is gated.
        //
        // Without the gate, each of the several other PEs Proton starts from this
        // directory truncates `Latest.log` and then blocks 120 seconds waiting for
        // a `GameAssembly.dll` it will never map. See `Layout::owns_game`.
        let owns_game = layout.owns_game();
        let log_path = layout.log_path();
        if owns_game {
            crate::log::reset(&log_path);
        }

        let stage = stage();
        let log = |message: &str| {
            if owns_game {
                crate::log::line(&log_path, message);
            }
        };
        log(&format!("stage limit = {stage}"));

        // Doorstop's `fix_cwd`. Under Steam the working directory is Steam's,
        // not the game's, so every relative path the loader touches would
        // resolve somewhere else entirely. Only ours to change - moving another
        // process's working directory is not the proxy's business.
        if owns_game
            && stage >= 1
            && let Err(err) = std::env::set_current_dir(layout.game_root())
        {
            log(&format!("WARN cannot set working directory: {err}"));
        }

        match crate::proxy::install(instance, layout.game_root()) {
            Ok(attached) => {
                log(&crate::log::proxy_attached(&attached.slot, &attached.real));
                if !attached.missing.is_empty() {
                    let names = attached.missing.join(", ");
                    log(&format!("WARN unresolved exports: {names}"));
                }
            }
            // The game may still run - it only calls a couple of these APIs -
            // so this is reported, not fatal.
            Err(err) => log(&format!("ERROR proxy attach failed: {err}")),
        }

        if !owns_game {
            return;
        }

        log("stage 1 ok (cwd)");
        if stage < 2 {
            log("stopping: stage < 2");
            return;
        }
        spawn_bootstrap(layout);
    }

    /// Moves the runtime work off the loader lock.
    ///
    /// The handle is dropped deliberately: this thread outlives the caller and
    /// nothing joins it.
    fn spawn_bootstrap(layout: Layout) {
        let log_path = layout.log_path();
        let spawned = std::thread::Builder::new()
            .name("modrix-bootstrap".to_owned())
            .spawn(move || run_bootstrap(&layout));
        if let Err(err) = spawned {
            crate::log::line(
                &log_path,
                &format!("ERROR cannot spawn bootstrap thread: {err}"),
            );
        }
    }

    /// Bootstrap thread body. Logs its own failure and returns; the game plays on.
    fn run_bootstrap(layout: &Layout) {
        let log_path = layout.log_path();
        if let Some(wine) = crate::win::wine_version() {
            crate::log::line(&log_path, &format!("host = wine {wine}"));
        }
        if let Err(err) = bootstrap(layout) {
            crate::log::line(&log_path, &format!("ERROR bootstrap failed: {err}"));
        }
    }

    /// Wait for IL2CPP, publish the environment, start CoreCLR, hand off.
    fn bootstrap(layout: &Layout) -> Result<(), Error> {
        let log_path = layout.log_path();
        let stage = stage();
        crate::log::line(&log_path, "stage 2 ok (bootstrap thread)");

        if stage < 3 {
            crate::log::line(&log_path, "stopping: stage < 3");
            return Ok(());
        }

        // Wait for the module, and then do NOT call into it.
        //
        // The obvious readiness probe - poll `il2cpp_domain_get` until it
        // returns non-null - kills the game. IL2CPP's garbage collector tracks
        // the threads it knows about, and any call that reaches the runtime
        // from a thread it never registered aborts the process with
        // "Collecting from unknown thread". Our bootstrap thread is by
        // definition unknown to it: the GC was initialised before we existed,
        // and attaching requires a domain we would have to call in to obtain.
        //
        // Waiting longer does not help - this is not a timing problem. The
        // thread is wrong, not early.
        //
        // Nothing here actually needs the domain. CoreCLR hosting is entirely
        // independent of IL2CPP, so the fix is simply to stop asking: wait for
        // the module to prove the game is up (a pure `GetModuleHandleW` loop
        // that touches nothing), then start the runtime. Managed code that
        // wants IL2CPP attaches properly, from the main thread, later.
        let game_assembly =
            crate::il2cpp::wait_for_module(IL2CPP_TIMEOUT, IL2CPP_POLL).ok_or(Error::Timeout {
                what: "GameAssembly.dll module",
                secs: IL2CPP_TIMEOUT.as_secs(),
            })?;
        crate::log::line(&log_path, &crate::log::game_assembly_mapped(game_assembly));

        if stage < 4 {
            crate::log::line(&log_path, "stopping: stage < 4");
            return Ok(());
        }

        // Published before the delegate call so the managed side never has to
        // re-derive a path, and never disagrees with the native side about one.
        for (name, value) in layout.env_pairs(game_assembly) {
            crate::win::set_env(name, &value)?;
        }

        let host = crate::coreclr::start(layout)?;
        let version = crate::layout::read_runtime_version(layout.dotnet_dir())
            .unwrap_or_else(|| "unknown".to_owned());
        crate::log::line(&log_path, &crate::log::coreclr_up(&version));

        host.run_managed_entrypoint()
    }
}
