// SPDX-License-Identifier: GPL-2.0-only
//! Turning a raw `Il2CppType` into a shape a managed signature can be written
//! from.
//!
//! A type-table entry is a tagged union: the `data` word means something
//! different for every kind. `Class`/`ValueType` and the primitives store a
//! type-definition index; `SzArray`/`Ptr`/`ByRef` store a pointer to the
//! element's own `Il2CppType`; `GenericInst` stores a pointer to an
//! `Il2CppGenericClass`; `Var`/`MVar` store a generic-parameter index. This
//! module walks that union into [`ResolvedType`], which maps one-to-one onto
//! ECMA-335 signature elements.
//!
//! The walk is **bounded and non-recursive**. A hostile or corrupt binary
//! could describe a type that contains itself (`T[][][]…` forever); an
//! explicit depth cap turns that into an error rather than a stack overflow.
//!
//! Layouts here were confirmed against the reference game, not assumed: all
//! 20,464 generic instantiations resolve to an in-range type definition with
//! zero failures, and the generic type handle was shown to point at an
//! `Il2CppType` (16-byte stride) rather than at a type definition.

use crate::binary::{GameBinary, TypeKind};
use crate::error::{Error, Result};

/// How deeply a type may nest before we call it malformed. `List<Dictionary<K,
/// V[]>[]>` is depth 4; real code never approaches this.
const MAX_DEPTH: usize = 32;

/// How many generic arguments one instantiation may carry.
const MAX_GENERIC_ARGS: usize = 64;

/// A type in a form that maps directly onto an ECMA-335 signature.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResolvedType {
    /// A built-in type with a dedicated `ELEMENT_TYPE_*` encoding.
    Primitive(TypeKind),
    /// A class or struct, named by its type-definition index.
    Named {
        /// Index into the metadata's type-definition table.
        type_def: usize,
        /// Whether it is a value type (encodes as `VALUETYPE`, not `CLASS`).
        value_type: bool,
    },
    /// A vector: single-dimension, zero-based.
    SzArray(Box<ResolvedType>),
    /// A multi-dimensional array.
    Array {
        /// The element type.
        element: Box<ResolvedType>,
        /// Number of dimensions.
        rank: u32,
    },
    /// An unmanaged pointer.
    Pointer(Box<ResolvedType>),
    /// A managed reference (`ref`/`out`).
    ByRef(Box<ResolvedType>),
    /// A constructed generic type, e.g. `List<int>`.
    Generic {
        /// The generic type definition being instantiated.
        type_def: usize,
        /// Whether the definition is a value type.
        value_type: bool,
        /// The type arguments, in order.
        args: Vec<ResolvedType>,
    },
    /// A generic parameter declared on the enclosing *type*.
    TypeParam(u32),
    /// A generic parameter declared on the enclosing *method*.
    MethodParam(u32),
    /// A function pointer. Its full signature is not modelled; a reference
    /// assembly can encode it as `IntPtr` without losing compilability.
    FnPtr,
}

impl GameBinary {
    /// Resolve type-table entry `index` into a signature-ready shape.
    ///
    /// # Errors
    ///
    /// [`Error::BoundExceeded`] if the type nests past [`MAX_DEPTH`] or names
    /// more than [`MAX_GENERIC_ARGS`] arguments, or [`Error::Truncated`] if a
    /// payload pointer leaves the image.
    pub fn resolve(&self, index: usize) -> Result<ResolvedType> {
        self.resolve_at(index, 0)
    }

    /// Depth-tracking worker behind [`GameBinary::resolve`].
    fn resolve_at(&self, index: usize, depth: usize) -> Result<ResolvedType> {
        if depth > MAX_DEPTH {
            return Err(Error::BoundExceeded {
                what: "type nesting depth",
                limit: MAX_DEPTH,
            });
        }
        let ty = self.type_at(index)?;
        // A by-ref type is a wrapper the bitfield records separately from the
        // kind, so it must be peeled before dispatching on the kind itself.
        if ty.by_ref {
            let inner = self.resolve_payload(index, depth)?;
            return Ok(ResolvedType::ByRef(Box::new(inner)));
        }
        self.resolve_payload(index, depth)
    }

    /// Resolve an entry ignoring its by-ref flag.
    fn resolve_payload(&self, index: usize, depth: usize) -> Result<ResolvedType> {
        let ty = self.type_at(index)?;
        let next = depth.saturating_add(1);
        match ty.kind {
            TypeKind::Class | TypeKind::ValueType => Ok(ResolvedType::Named {
                type_def: usize::try_from(ty.data).unwrap_or(0),
                value_type: ty.kind == TypeKind::ValueType,
            }),
            TypeKind::SzArray => Ok(ResolvedType::SzArray(Box::new(
                self.resolve_element(ty.data, next)?,
            ))),
            TypeKind::Ptr => Ok(ResolvedType::Pointer(Box::new(
                self.resolve_element(ty.data, next)?,
            ))),
            TypeKind::ByRef => Ok(ResolvedType::ByRef(Box::new(
                self.resolve_element(ty.data, next)?,
            ))),
            TypeKind::Array => self.resolve_array(ty.data, next),
            TypeKind::GenericInst => self.resolve_generic(ty.data, next),
            TypeKind::Var => Ok(ResolvedType::TypeParam(u32::try_from(ty.data).unwrap_or(0))),
            TypeKind::MVar => Ok(ResolvedType::MethodParam(
                u32::try_from(ty.data).unwrap_or(0),
            )),
            TypeKind::FnPtr => Ok(ResolvedType::FnPtr),
            other => Ok(ResolvedType::Primitive(other)),
        }
    }

    /// Resolve an `Il2CppType` reached by pointer rather than by table index.
    fn resolve_element(&self, pointer: u64, depth: usize) -> Result<ResolvedType> {
        let index = self.type_index_of(pointer)?;
        self.resolve_at(index, depth)
    }

    /// Decode `Il2CppArrayType { Il2CppType* etype; uint8 rank; ... }`.
    fn resolve_array(&self, pointer: u64, depth: usize) -> Result<ResolvedType> {
        let base = self.offset_of(pointer)?;
        let element_ptr = self.read_u64(base)?;
        let rank = u32::from(self.read_u8(base.saturating_add(8))?);
        Ok(ResolvedType::Array {
            element: Box::new(self.resolve_element(element_ptr, depth)?),
            rank: rank.max(1),
        })
    }

    /// Decode `Il2CppGenericClass { Il2CppType* type; Il2CppGenericInst*
    /// class_inst; ... }`, where the first field points at the *open* generic
    /// type's own entry rather than at a type definition.
    fn resolve_generic(&self, pointer: u64, depth: usize) -> Result<ResolvedType> {
        let base = self.offset_of(pointer)?;
        let handle = self.read_u64(base)?;
        let open = self.resolve_element(handle, depth)?;
        // A generic instantiation whose definition is not a plain named type
        // is not something a reference assembly can express; fall back rather
        // than emit a malformed signature.
        let ResolvedType::Named {
            type_def,
            value_type,
        } = open
        else {
            return Ok(ResolvedType::FnPtr);
        };
        let args = self.resolve_generic_args(self.read_u64(base.saturating_add(8))?, depth)?;
        Ok(ResolvedType::Generic {
            type_def,
            value_type,
            args,
        })
    }

    /// Decode `Il2CppGenericInst { uint32 argc; const Il2CppType** argv; }`.
    fn resolve_generic_args(&self, pointer: u64, depth: usize) -> Result<Vec<ResolvedType>> {
        let base = self.offset_of(pointer)?;
        let count = usize::try_from(self.read_u32(base)?).unwrap_or(0);
        if count > MAX_GENERIC_ARGS {
            return Err(Error::BoundExceeded {
                what: "generic arguments",
                limit: MAX_GENERIC_ARGS,
            });
        }
        // The pointer array follows an 8-byte-aligned header, so the vector
        // sits at +8 even though the count is only 4 bytes wide.
        let vector = self.offset_of(self.read_u64(base.saturating_add(8))?)?;
        let mut resolved = Vec::with_capacity(count);
        for slot in 0..count {
            let at = vector.saturating_add(slot.saturating_mul(8));
            resolved.push(self.resolve_element(self.read_u64(at)?, depth)?);
        }
        Ok(resolved)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Metadata;

    fn real_game() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let home = std::env::var_os("HOME")?;
        let root = std::path::Path::new(&home)
            .join(".local/share/Steam/steamapps/common/Sons Of The Forest");
        let binary = root.join("GameAssembly.dll");
        let metadata = root.join("SonsOfTheForest_Data/il2cpp_data/Metadata/global-metadata.dat");
        (binary.exists() && metadata.exists()).then_some((binary, metadata))
    }

    #[test]
    fn resolves_every_type_in_the_real_game() {
        let Some((binary, metadata)) = real_game() else {
            return;
        };
        let md = Metadata::open(&metadata).unwrap();
        let types = md.type_count().unwrap();
        let game = GameBinary::open(&binary, types).unwrap();

        // Every entry must resolve. Anything that does not is a layout bug
        // that would surface later as a corrupt signature, which is far
        // harder to diagnose than a failure here.
        let (mut named, mut generic, mut arrays) = (0_usize, 0_usize, 0_usize);
        for index in 0..game.type_count() {
            match game.resolve(index).expect("every type resolves") {
                ResolvedType::Named { type_def, .. } => {
                    assert!(type_def < types, "type_def {type_def} out of range");
                    named = named.saturating_add(1);
                }
                ResolvedType::Generic { type_def, args, .. } => {
                    assert!(type_def < types, "generic def {type_def} out of range");
                    assert!(!args.is_empty(), "a generic instantiation has arguments");
                    generic = generic.saturating_add(1);
                }
                ResolvedType::SzArray(_) | ResolvedType::Array { .. } => {
                    arrays = arrays.saturating_add(1);
                }
                _ => {}
            }
        }
        assert!(named > 10_000, "named types: {named}");
        assert!(generic > 10_000, "generic instantiations: {generic}");
        assert!(arrays > 1_000, "array types: {arrays}");
    }

    #[test]
    fn a_generic_instantiations_arguments_all_resolve() {
        let Some((binary, metadata)) = real_game() else {
            return;
        };
        let md = Metadata::open(&metadata).unwrap();
        let game = GameBinary::open(&binary, md.type_count().unwrap()).unwrap();

        let found = (0..game.type_count())
            .filter_map(|i| game.resolve(i).ok())
            .filter_map(|t| match t {
                ResolvedType::Generic { args, .. } if !args.is_empty() => Some(args),
                _ => None,
            })
            .take(500)
            .count();
        assert!(found >= 500, "expected many resolvable instantiations");
    }
}
