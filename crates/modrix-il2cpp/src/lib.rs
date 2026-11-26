// SPDX-License-Identifier: GPL-2.0-only
//! Reader for Unity's IL2CPP `global-metadata.dat`.
//!
//! ## Why this exists
//!
//! An IL2CPP game has no managed assemblies. Unity compiles C# to C++ ahead of
//! time, so the shipped game is `GameAssembly.dll` (native code) plus
//! `global-metadata.dat` (every type, method, field and string name, stripped
//! of bodies). There is no `Assembly-CSharp.dll` for a mod to reference.
//!
//! To make such a game moddable at all, a loader must read that metadata and
//! **generate managed proxy assemblies** - one .NET DLL per original assembly,
//! with the same namespaces, types and method signatures, whose bodies
//! marshal into the native code. Those generated DLLs are what a mod author
//! compiles against and what Harmony patches at runtime.
//!
//! This crate is the first half of that pipeline: it turns the binary metadata
//! into a structured model. It is deliberately **pure, safe Rust with no
//! `unsafe` and no FFI** - it only ever reads a file - which is why it can
//! live inside the main Modrix workspace under the same Power of Ten lints as
//! the deploy engine.
//!
//! ## Layout discovery
//!
//! Record sizes are not described in the file; they are implied by the version.
//! Rather than trusting a transcribed C header, the sizes here were derived
//! from a real game's metadata by checking which candidate size divides each
//! section's byte length exactly, and cross-checked against invariants that
//! must hold (every assembly has exactly one image, so those two counts match).
//! [`Metadata::parse`] re-checks divisibility on every file it reads, so a
//! version whose layout differs is rejected loudly rather than misread.

mod binary;
mod defs;
mod error;
mod metadata;
mod pe;
mod reader;
mod resolve;

pub use binary::{GameBinary, Il2CppType, Registration, TypeKind};
pub use defs::{
    Assembly, FieldDef, GenericContainer, MethodDef, ParamDef, PropertyDef, Run, TypeDef,
};
pub use error::{Error, Result};
pub use metadata::{DefaultValue, Metadata, SANITY, SUPPORTED_VERSIONS};
pub use pe::{PeImage, Section as PeSection};
pub use resolve::ResolvedType;

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;

    /// The Sons of the Forest metadata on this machine, when it is installed.
    /// Tests that need a real game are skipped elsewhere rather than failing
    /// CI, which has no game directory.
    fn real_metadata() -> Option<std::path::PathBuf> {
        let home = std::env::var_os("HOME")?;
        let path = Path::new(&home).join(
            ".local/share/Steam/steamapps/common/Sons Of The Forest/\
             SonsOfTheForest_Data/il2cpp_data/Metadata/global-metadata.dat",
        );
        path.exists().then_some(path)
    }

    #[test]
    fn rejects_a_file_that_is_not_metadata() {
        let raw = b"not il2cpp metadata at all".to_vec();
        assert!(matches!(Metadata::parse(raw), Err(Error::BadSanity { .. })));
    }

    #[test]
    fn rejects_an_unknown_metadata_version() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&SANITY.to_le_bytes());
        raw.extend_from_slice(&99_i32.to_le_bytes());
        raw.resize(512, 0);
        assert!(matches!(
            Metadata::parse(raw),
            Err(Error::UnsupportedVersion { found: 99, .. })
        ));
    }

    #[test]
    fn rejects_a_truncated_header() {
        let mut raw = Vec::new();
        raw.extend_from_slice(&SANITY.to_le_bytes());
        raw.extend_from_slice(&29_i32.to_le_bytes());
        // Sanity and version are present but no section table follows.
        assert!(matches!(Metadata::parse(raw), Err(Error::Truncated { .. })));
    }

    #[test]
    fn reads_the_real_game_metadata() {
        let Some(path) = real_metadata() else {
            return; // Game not installed; covered by the synthetic tests.
        };
        let md = Metadata::open(&path).expect("real metadata parses");
        assert_eq!(md.version(), 29);

        // Every assembly has exactly one image. If the record sizes were
        // wrong these two would not agree, so this is the layout check.
        let assemblies = md.assembly_count().unwrap();
        assert_eq!(
            assemblies,
            md.image_count().unwrap(),
            "one image per assembly - a mismatch means the record layout is wrong"
        );
        assert!(assemblies > 100, "a Unity game ships many assemblies");
        assert!(md.type_count().unwrap() > 10_000);

        // The game's own code must be in there, by name.
        let names: Vec<String> = md
            .assemblies()
            .unwrap()
            .into_iter()
            .map(|a| a.name)
            .collect();
        for expected in ["Assembly-CSharp", "UnityEngine.CoreModule", "Sons"] {
            assert!(
                names.iter().any(|n| n == expected),
                "expected assembly `{expected}` among {} assemblies",
                names.len()
            );
        }
    }

    #[test]
    fn reads_real_type_names() {
        let Some(path) = real_metadata() else {
            return;
        };
        let md = Metadata::open(&path).unwrap();
        // Walk a bounded window and confirm we get plausible identifiers
        // rather than binary noise - the check that string indices resolve.
        let mut named = 0_usize;
        for index in 0..2000.min(md.type_count().unwrap()) {
            let def = md.type_def(index).unwrap();
            if !def.name.is_empty() && def.name.is_ascii() {
                named = named.saturating_add(1);
            }
        }
        assert!(
            named > 1900,
            "expected nearly every type to have a readable name, got {named}"
        );
    }

    #[test]
    fn decodes_a_real_type_with_its_members_and_signature_types() {
        let Some(path) = real_metadata() else {
            return; // Game not installed.
        };
        let md = Metadata::open(&path).unwrap();

        // LocalPlayer is the type every Sons of the Forest mod reaches for, so
        // it is the right end-to-end canary: if its members decode, the shared
        // field/method/parameter runs are being sliced correctly.
        let target = (0..md.type_count().unwrap())
            .map(|i| (i, md.type_def(i).unwrap()))
            .find(|(_, t)| t.namespace == "TheForest.Utils" && t.name == "LocalPlayer")
            .map(|(_, t)| t)
            .expect("TheForest.Utils.LocalPlayer exists in this game");

        assert!(target.is_public());
        assert!(!target.is_nested());
        assert!(target.fields.count > 50, "LocalPlayer has many fields");
        assert!(target.methods.count > 50, "and many methods");

        let field_names: Vec<String> = target
            .fields
            .indices()
            .map(|i| md.field_def(i).unwrap().name)
            .collect();
        assert!(
            field_names.iter().any(|n| n == "_transform"),
            "expected the _transform field among {}",
            field_names.len()
        );

        let methods: Vec<MethodDef> = target
            .methods
            .indices()
            .map(|i| md.method_def(i).unwrap())
            .collect();
        let getter = methods
            .iter()
            .find(|m| m.name == "get_IsInWorld")
            .expect("LocalPlayer exposes IsInWorld");
        assert!(getter.is_public());
        assert!(getter.is_static(), "IsInWorld is a static property");

        // Every method's declaring type must point back at some real type, and
        // every parameter must resolve - that is what the emitter relies on.
        for method in &methods {
            assert!(
                usize::try_from(method.declaring_type).unwrap_or(usize::MAX)
                    < md.type_count().unwrap()
            );
            for index in method.parameters.indices() {
                let param = md.param_def(index).unwrap();
                assert!(!param.name.is_empty(), "parameters are named");
            }
        }
    }

    #[test]
    fn every_signature_type_index_resolves_against_the_game_binary() {
        let Some(metadata) = real_metadata() else {
            return;
        };
        let binary = metadata
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|root| root.join("GameAssembly.dll"))
            .filter(|p| p.exists());
        let Some(binary) = binary else {
            return;
        };
        let md = Metadata::open(&metadata).unwrap();
        let types = md.type_count().unwrap();
        let game = GameBinary::open(&binary, types).unwrap();

        // The metadata says "this field has type index N"; N indexes a table
        // that lives in the BINARY. If these two files ever disagree, every
        // emitted signature would be silently wrong - so check the join.
        let type_table = game.type_count();
        let mut checked = 0_usize;
        for index in 0..md.field_count().unwrap().min(20_000) {
            let field = md.field_def(index).unwrap();
            let slot = usize::try_from(field.type_index).unwrap_or(usize::MAX);
            assert!(
                slot < type_table,
                "field {index} type index {slot} is outside the {type_table}-entry type table"
            );
            game.type_at(slot).expect("the referenced type decodes");
            checked = checked.saturating_add(1);
        }
        assert!(checked > 10_000, "checked {checked} field types");
    }
}
