// SPDX-License-Identifier: GPL-2.0-only
//! The seventeen `version.dll` exports, forwarded to the real system DLL.
//!
//! # Why a naked `jmp` and not a C-style thunk
//!
//! Unity Doorstop emits `void *exp_X(void) { return __X__(); }` - a
//! zero-argument C function - and relies on the compiler tail-calling at `-Os`
//! so that RCX/RDX/R8/R9 and any stack arguments survive. That is correct only
//! by luck: at `-O0`, or with a stack-protector prologue, the thunk clobbers the
//! frame and any export taking more than four arguments (`VerInstallFileW` takes
//! seven) receives garbage.
//!
//! MelonLoader avoids the problem by rewriting its own export stubs at runtime
//! with `VirtualProtect(PAGE_EXECUTE_READWRITE)` + `mov r11, addr; jmp r11`.
//! That is unconditionally correct, but it makes our own `.text` writable and
//! executable for the life of the process, which is a needless liability inside
//! a game's address space.
//!
//! A `#[unsafe(naked)]` function that loads a pointer and tail-jumps is
//! unconditionally correct *and* needs no RWX memory - `injection.md` names it
//! "the only strictly correct implementation". Emitted code, verified with
//! `objdump` on the cross-compiled DLL:
//!
//! ```text
//! mov  rax, qword ptr [rip + SLOTS + N*8]
//! test rax, rax
//! je   .null
//! jmp  rax
//! .null:
//! xor  eax, eax
//! ret
//! ```
//!
//! RAX is scratch and is not an argument register in the Microsoft x64 calling
//! convention, so every argument - register and stack - reaches the real
//! function untouched, and the real function's `ret` returns straight to the
//! game. `rsp` is never adjusted, so stack arguments keep their offsets.
//!
//! The null branch exists only for the case where the real DLL loaded but did
//! not export a name; returning 0 reads as failure to every one of these APIs.
//! It cannot happen with a genuine `version.dll`, and the miss is logged.
//!
//! Rust exports `#[unsafe(no_mangle)] pub extern` functions from a `cdylib`
//! with `dllexport` automatically, so no `.def` file is involved and the export
//! names are undecorated. `build-windows.sh` asserts all seventeen are present
//! in the produced PE.

use core::sync::atomic::AtomicUsize;

/// Number of exports a `version.dll` proxy must provide.
///
/// Taken verbatim from Doorstop's `src/windows/proxy/proxylist.txt` and
/// confirmed against the released binary; guessing this set gets the game a
/// missing-import dialog on launch.
pub const SLOT_COUNT: usize = 17;

#[expect(
    clippy::declare_interior_mutable_const,
    reason = "the const is only a zero-initialiser for the static array; it is never read as a value"
)]
const EMPTY_SLOT: AtomicUsize = AtomicUsize::new(0);

/// Resolved addresses of the real exports, indexed the same as [`EXPORT_NAMES`].
///
/// Written once from `DLL_PROCESS_ATTACH` and read by the naked thunks. The
/// thunks read the slot with a plain `mov`, which is a benign race on x86-64:
/// the store is a single aligned 8-byte write under TSO, so a reader sees either
/// the old zero (and returns failure) or the final address, never a tear.
pub static SLOTS: [AtomicUsize; SLOT_COUNT] = [EMPTY_SLOT; SLOT_COUNT];

/// Declares one forwarder, and records its name and slot index so a
/// transcription mistake is a failing test rather than a corrupt jump.
macro_rules! version_exports {
    ($( ($index:expr, $name:ident) ),+ $(,)?) => {
        /// Export names in slot order.
        pub const EXPORT_NAMES: [&str; SLOT_COUNT] = [ $( stringify!($name) ),+ ];

        /// The slot index each entry of [`EXPORT_NAMES`] was declared with.
        ///
        /// Exists so a transcription mistake in the table above - a repeated or
        /// out-of-order index, which would silently make one export jump to
        /// another's target - is caught by a unit test instead of by a user.
        pub const DECLARED_SLOTS: [usize; SLOT_COUNT] = [ $( $index ),+ ];

        $(
            #[doc = concat!("Forwards `", stringify!($name), "` to the real `version.dll`.")]
            #[doc = ""]
            #[doc = "The declared signature is a placeholder: this is a naked tail-jump, so"]
            #[doc = "the real export's arguments and return value pass through unchanged."]
            #[doc = "It is never called from Rust."]
            #[doc = ""]
            #[doc = "# Safety"]
            #[doc = ""]
            #[doc = "Callable only by the Windows loader on behalf of a module that imported"]
            #[doc = "this name, with that export's real argument list."]
            #[cfg(all(windows, target_arch = "x86_64"))]
            #[unsafe(naked)]
            #[unsafe(no_mangle)]
            pub unsafe extern "system" fn $name() {
                core::arch::naked_asm!(
                    "mov rax, qword ptr [rip + {slots} + {offset}]",
                    "test rax, rax",
                    "je 2f",
                    "jmp rax",
                    "2:",
                    "xor eax, eax",
                    "ret",
                    slots = sym SLOTS,
                    offset = const ($index * 8usize),
                )
            }
        )+
    };
}

version_exports![
    (0, GetFileVersionInfoA),
    (1, GetFileVersionInfoByHandle),
    (2, GetFileVersionInfoExA),
    (3, GetFileVersionInfoExW),
    (4, GetFileVersionInfoSizeA),
    (5, GetFileVersionInfoSizeExA),
    (6, GetFileVersionInfoSizeExW),
    (7, GetFileVersionInfoSizeW),
    (8, GetFileVersionInfoW),
    (9, VerFindFileA),
    (10, VerFindFileW),
    (11, VerInstallFileA),
    (12, VerInstallFileW),
    (13, VerLanguageNameA),
    (14, VerLanguageNameW),
    (15, VerQueryValueA),
    (16, VerQueryValueW),
];

#[cfg(test)]
mod tests {
    use super::{DECLARED_SLOTS, EXPORT_NAMES, SLOT_COUNT, SLOTS};
    use core::sync::atomic::Ordering;
    use std::collections::BTreeSet;

    #[test]
    fn every_thunk_owns_exactly_its_table_index() {
        // A wrong index here would make one export jump to another export's
        // target - which "works" for the game right up until it does not.
        let expected: Vec<usize> = (0..SLOT_COUNT).collect();
        assert_eq!(DECLARED_SLOTS.to_vec(), expected);
    }

    #[test]
    fn the_table_is_the_doorstop_proxylist() {
        // Verbatim from Doorstop src/windows/proxy/proxylist.txt (version set).
        let expected = BTreeSet::from([
            "GetFileVersionInfoA",
            "GetFileVersionInfoByHandle",
            "GetFileVersionInfoExA",
            "GetFileVersionInfoExW",
            "GetFileVersionInfoSizeA",
            "GetFileVersionInfoSizeExA",
            "GetFileVersionInfoSizeExW",
            "GetFileVersionInfoSizeW",
            "GetFileVersionInfoW",
            "VerFindFileA",
            "VerFindFileW",
            "VerInstallFileA",
            "VerInstallFileW",
            "VerLanguageNameA",
            "VerLanguageNameW",
            "VerQueryValueA",
            "VerQueryValueW",
        ]);
        assert_eq!(
            EXPORT_NAMES.iter().copied().collect::<BTreeSet<_>>(),
            expected
        );
        assert_eq!(
            EXPORT_NAMES.len(),
            SLOT_COUNT,
            "duplicate name in the table"
        );
    }

    #[test]
    fn slots_start_null_so_an_unresolved_export_fails_closed() {
        assert_eq!(SLOTS.len(), EXPORT_NAMES.len());
        assert!(SLOTS.iter().all(|slot| slot.load(Ordering::SeqCst) == 0));
    }
}
