// SPDX-License-Identifier: GPL-2.0-only
//! A from-scratch ECMA-335 / PE32+ writer that emits .NET **reference
//! assemblies**: types, fields and method signatures, with no method bodies.
//!
//! ## Why this exists
//!
//! An IL2CPP game ships no managed assemblies, so there is nothing for a mod
//! author to compile against. Modrix reconstructs the missing surface from the
//! game's own metadata, and this crate is the last stage of that pipeline: it
//! turns a type model into a real DLL that `csc` accepts as a `-r:` reference.
//! Shelling out to the .NET SDK was rejected - it would make mod development
//! depend on a toolchain the user may not have - so the container, the
//! metadata tables and the signature blobs are all written here, in pure safe
//! Rust with no dependencies beyond `thiserror`.
//!
//! ## What "reference assembly" buys
//!
//! Every `MethodDef.RVA` is 0. That is what keeps the output at a couple of
//! kilobytes with a single section, no imports, no relocations and no entry
//! point - and it is legal, because a compiler only ever reads signatures.
//! The file is deliberately **not** loadable by the CLR; it is a compile-time
//! artefact.
//!
//! ## The rules that are not optional
//!
//! Each of these was established by ablation - removing it and watching what
//! broke - and each fails in a way that points nowhere near the cause:
//!
//! | Rule | Symptom when violated |
//! |------|-----------------------|
//! | An `Assembly` row must exist | `CS1509: ... is not an assembly` |
//! | The corlib reference must be `System.Runtime` with token `b03f5f7f11d50a3a` | `CS0012` on every imported type |
//! | Literal fields need a `Constant` row | the *consumer* fails with a bare `CS7038` |
//! | The metadata version length must be a multiple of 4 | `CS0009: Read out of bounds` |
//! | `#Blob` must be present | `CS0009: Read out of bounds` |
//! | Coded indices widen at `1 << (16 - tag_bits)` | silent corruption past 16 384 types |
//! | Member runs must be contiguous per owning type | members bind to the wrong type |
//! | Every coded index must resolve | Roslyn crashes with `ArgumentOutOfRangeException` |
//!
//! ## Example
//!
//! ```
//! use modrix_ecma335::{AssemblyBuilder, ResolutionScope, Version};
//! use modrix_ecma335::model::{
//!     AssemblyReference, Field, Method, TypeDefinition, field_attributes,
//!     method_attributes, type_attributes,
//! };
//! use modrix_ecma335::signature::{MethodSignature, Type};
//!
//! let mut builder = AssemblyBuilder::new("MyLib", Version::new(1, 0, 0, 0));
//! let corlib = builder.add_assembly_ref(AssemblyReference::core_library(9));
//! let scope = ResolutionScope::AssemblyRef(corlib);
//! let object = builder.add_type_ref(scope, "System", "Object");
//!
//! let widget = builder.add_type_def(TypeDefinition::new(
//!     "MyLib",
//!     "Widget",
//!     type_attributes::PUBLIC_CLASS,
//!     Some(object.into()),
//! ))?;
//! builder.add_field(widget, Field::new("Value", field_attributes::PUBLIC, Type::I4))?;
//! builder.add_default_constructor(widget)?;
//! builder.add_method(
//!     widget,
//!     Method::new(
//!         "Add",
//!         method_attributes::PUBLIC_STATIC,
//!         MethodSignature::static_method(Type::I4, vec![Type::I4, Type::I4]),
//!     ),
//! )?;
//!
//! let dll = builder.build()?;
//! assert_eq!(&dll[..2], b"MZ");
//! # Ok::<(), modrix_ecma335::Error>(())
//! ```

// Every public item is part of a byte-format contract, so an undocumented one
// is a gap in the format's only prose description.
#![deny(missing_docs)]

mod builder;
mod emit;
pub mod encode;
mod error;
mod handles;
mod heaps;
mod materialize;
mod metadata;
pub mod model;
mod pe;
mod rows;
pub mod signature;
mod tables;

#[cfg(test)]
mod verify;

#[cfg(test)]
mod tests;

pub use builder::{
    AssemblyBuilder, HASH_ALGORITHM_SHA1, attribute_blob_empty, attribute_blob_string,
};
pub use error::{Error, Result};
pub use handles::{
    AssemblyRefId, CustomAttributeType, FieldId, HasCustomAttribute, MemberRefId, MemberRefParent,
    MethodId, PropertyId, ResolutionScope, TypeDefId, TypeDefOrRef, TypeRefId, TypeSpecId,
};
pub use model::{
    AssemblyReference, CORE_LIBRARY_NAME, CORE_LIBRARY_PUBLIC_KEY_TOKEN, Constant, Field,
    GenericParameter, Method, Parameter, Property, TypeDefinition, Version,
};
pub use signature::{MethodSignature, Type};
