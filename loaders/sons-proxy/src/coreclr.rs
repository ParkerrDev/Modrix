// SPDX-License-Identifier: GPL-2.0-only
//! Hosting CoreCLR with the two exports Doorstop uses, and nothing else.
//!
//! # Why not hostfxr
//!
//! MelonLoader goes through `nethost`/`hostfxr`, which resolves frameworks
//! properly and can use a system .NET - at the cost of needing `hostfxr.dll`, a
//! `*.runtimeconfig.json`, and a correct `DOTNET_ROOT` inside a Proton prefix.
//! Doorstop instead calls `coreclr_initialize` and `coreclr_create_delegate`
//! directly against a vendored runtime. That is two symbols, no config file and
//! no framework resolution, and it is the route already proven to work in this
//! exact game under this exact Proton prefix (PLAN.md D5). BepInEx's
//! `mini-coreclr` 6.0.7 `win-x64` ships `coreclr.dll` but deliberately no
//! `hostfxr.dll`, so this is also the only route that runtime supports.
//!
//! `coreclr_initialize` is given two properties. `APP_PATHS` - the corlib
//! directory, the managed host directory and the interop directory - is the
//! probing list, and is the one property Doorstop passes.
//! `TRUSTED_PLATFORM_ASSEMBLIES` is ours; see
//! `Layout::trusted_platform_assemblies` for why running without it breaks
//! Harmony in a way whose error message names none of this.

use crate::error::Error;
use crate::layout::{
    CORECLR_FILE_NAME, DOMAIN_FRIENDLY_NAME, ENTRY_METHOD, ENTRY_TYPE, Layout, MANAGED_ASSEMBLY,
};
use core::ffi::c_void;
use core::mem::MaybeUninit;
use std::ffi::CString;

/// `int coreclr_initialize(const char*, const char*, int, const char**, const char**, void**, unsigned int*)`.
type CoreclrInitialize = unsafe extern "system" fn(
    exe_path: *const u8,
    app_domain_friendly_name: *const u8,
    property_count: i32,
    property_keys: *const *const u8,
    property_values: *const *const u8,
    host_handle: *mut *mut c_void,
    domain_id: *mut u32,
) -> u32;

/// `int coreclr_create_delegate(void*, unsigned int, const char*, const char*, const char*, void**)`.
type CoreclrCreateDelegate = unsafe extern "system" fn(
    host_handle: *mut c_void,
    domain_id: u32,
    assembly_name: *const u8,
    type_name: *const u8,
    method_name: *const u8,
    delegate: *mut *mut c_void,
) -> u32;

/// The managed entrypoint: zero parameters, void return. Non-negotiable -
/// `coreclr_create_delegate` builds the reverse-P/Invoke stub from the method's
/// own signature, so any other shape corrupts the stack on call.
type ManagedStart = unsafe extern "system" fn();

/// A live CoreCLR host, plus the two hosting functions resolved from it.
pub struct Host {
    handle: *mut c_void,
    domain_id: u32,
    create_delegate: CoreclrCreateDelegate,
}

/// Loads the vendored `coreclr.dll` and initializes it.
///
/// # Errors
///
/// - [`Error::Missing`] when `_Modrix/dotnet/coreclr.dll` is absent.
/// - [`Error::Win32`] when it cannot be loaded.
/// - [`Error::MissingExport`] when either hosting symbol is absent, which means
///   the vendored runtime is not the one this code was written against.
/// - [`Error::InteriorNul`] when a path contains a NUL byte.
/// - [`Error::CoreClr`] with the HRESULT when initialization itself fails.
pub fn start(layout: &Layout) -> Result<Host, Error> {
    let coreclr_path = layout.coreclr_path();
    if !coreclr_path.is_file() {
        return Err(Error::Missing {
            what: "coreclr",
            path: coreclr_path,
        });
    }
    let module = crate::win::load_library(&coreclr_path)?;
    let initialize = resolve::<CoreclrInitialize>(module, "coreclr_initialize")?;
    let create_delegate = resolve::<CoreclrCreateDelegate>(module, "coreclr_create_delegate")?;

    let exe = cstring(
        &layout.process_path().to_string_lossy(),
        "MODRIX_PROCESS_PATH",
    )?;
    let domain_name = cstring(DOMAIN_FRIENDLY_NAME, "domain name")?;
    let app_paths_key = cstring("APP_PATHS", "APP_PATHS key")?;
    let app_paths_value = cstring(&layout.app_paths(), "APP_PATHS value")?;
    // See `Layout::trusted_platform_assemblies`. Without this second property,
    // managed code still runs, but every Harmony patch of an IL2CPP method fails
    // because Mono.Cecil cannot resolve `System.Private.CoreLib`.
    let trusted_key = cstring(
        "TRUSTED_PLATFORM_ASSEMBLIES",
        "TRUSTED_PLATFORM_ASSEMBLIES key",
    )?;
    let trusted_value = cstring(
        &layout.trusted_platform_assemblies(),
        "TRUSTED_PLATFORM_ASSEMBLIES value",
    )?;
    let keys = [
        app_paths_key.as_ptr().cast::<u8>(),
        trusted_key.as_ptr().cast::<u8>(),
    ];
    let values = [
        app_paths_value.as_ptr().cast::<u8>(),
        trusted_value.as_ptr().cast::<u8>(),
    ];

    let mut handle = MaybeUninit::<*mut c_void>::uninit();
    let mut domain_id = MaybeUninit::<u32>::uninit();
    // SAFETY: every pointer is a live NUL-terminated buffer owned by this frame,
    // the two arrays hold exactly `property_count` entries, and the two out
    // parameters are writable and correctly typed.
    let hr = unsafe {
        initialize(
            exe.as_ptr().cast::<u8>(),
            domain_name.as_ptr().cast::<u8>(),
            2,
            keys.as_ptr(),
            values.as_ptr(),
            handle.as_mut_ptr(),
            domain_id.as_mut_ptr(),
        )
    };
    if hr != 0 {
        return Err(Error::CoreClr {
            op: "coreclr_initialize",
            hr,
        });
    }
    // SAFETY: `coreclr_initialize` returning 0 means both out parameters were
    // written.
    let handle = unsafe { handle.assume_init() };
    // SAFETY: as above.
    let domain_id = unsafe { domain_id.assume_init() };
    Ok(Host {
        handle,
        domain_id,
        create_delegate,
    })
}

impl Host {
    /// Builds a delegate to `Modrix.Loader.Entrypoint.Start` and calls it.
    ///
    /// The call is synchronous: managed code runs to completion on this thread
    /// before the function returns. For M0 that is a handful of file writes.
    ///
    /// # Errors
    ///
    /// [`Error::InteriorNul`] for a malformed name, or [`Error::CoreClr`] with
    /// the HRESULT when the assembly, type or method cannot be resolved -
    /// `0x80070002` here means `Modrix.Loader.dll` is not on `APP_PATHS`.
    pub fn run_managed_entrypoint(&self) -> Result<(), Error> {
        let assembly = cstring(MANAGED_ASSEMBLY, "managed assembly name")?;
        let type_name = cstring(ENTRY_TYPE, "managed type name")?;
        let method = cstring(ENTRY_METHOD, "managed method name")?;
        let mut delegate = MaybeUninit::<*mut c_void>::uninit();
        // SAFETY: `self.handle` and `self.domain_id` came from a successful
        // `coreclr_initialize`, the three names are live NUL-terminated buffers,
        // and `delegate` is a writable out parameter.
        let hr = unsafe {
            (self.create_delegate)(
                self.handle,
                self.domain_id,
                assembly.as_ptr().cast::<u8>(),
                type_name.as_ptr().cast::<u8>(),
                method.as_ptr().cast::<u8>(),
                delegate.as_mut_ptr(),
            )
        };
        if hr != 0 {
            return Err(Error::CoreClr {
                op: "coreclr_create_delegate",
                hr,
            });
        }
        // SAFETY: a 0 HRESULT means `delegate` was written.
        let raw = unsafe { delegate.assume_init() };
        if raw.is_null() {
            return Err(Error::CoreClr {
                op: "coreclr_create_delegate",
                hr: 0,
            });
        }
        // SAFETY: CoreCLR produced a reverse-P/Invoke stub for a `static void
        // Start()`, which is exactly `ManagedStart`.
        let start = unsafe { core::mem::transmute::<*mut c_void, ManagedStart>(raw) };
        // SAFETY: calling that stub. Managed exceptions are caught inside
        // `Start`, so nothing propagates back across this boundary.
        unsafe { start() };
        Ok(())
    }
}

/// Resolves a hosting symbol and transmutes it to its function type.
fn resolve<T: Copy>(
    module: windows_sys::Win32::Foundation::HMODULE,
    symbol: &'static str,
) -> Result<T, Error> {
    const {
        assert!(
            size_of::<T>() == size_of::<usize>(),
            "T must be a bare function pointer"
        );
    };
    let address = crate::win::proc_address(module, symbol).ok_or(Error::MissingExport {
        module: CORECLR_FILE_NAME,
        symbol,
    })?;
    // SAFETY: the address came from `GetProcAddress` on `coreclr.dll`, and the
    // const assertion above pins `T` to pointer size. The caller names `T` as
    // the documented signature of `symbol`.
    Ok(unsafe { core::mem::transmute_copy::<usize, T>(&address) })
}

/// UTF-8 `CString` for a C API, with the field named in the error.
fn cstring(text: &str, what: &'static str) -> Result<CString, Error> {
    CString::new(text).map_err(|_| Error::InteriorNul(what))
}
