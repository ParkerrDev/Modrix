// SPDX-License-Identifier: GPL-2.0-only
//! Opaque handles to rows the builder has been asked to emit.
//!
//! Handles are zero-based positions in the builder's own vectors, never raw
//! metadata tokens. That matters for members: fields and methods are added
//! against their owning type and only get a table row number when [`build`]
//! flattens the types in order, because ECMA-335 requires each type's members
//! to be physically contiguous. A `FieldId` therefore names
//! `(type, position within that type)` and is resolved late.
//!
//! [`build`]: crate::AssemblyBuilder::build

use crate::error::Result;
use crate::tables::{TYPE_DEF_OR_REF, table};

/// Handle to an `AssemblyRef` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct AssemblyRefId(pub(crate) u32);

/// Handle to a `TypeRef` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeRefId(pub(crate) u32);

/// Handle to a `TypeSpec` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeSpecId(pub(crate) u32);

/// Handle to a `TypeDef` row. Handle 0 is always the `<Module>` pseudo-type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TypeDefId(pub(crate) u32);

/// Handle to a `MemberRef` row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MemberRefId(pub(crate) u32);

/// Handle to a `Field` row, as `(owning type, position within that type)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct FieldId {
    pub(crate) owner: u32,
    pub(crate) slot: u32,
}

/// Handle to a `MethodDef` row, as `(owning type, position within that type)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct MethodId {
    pub(crate) owner: u32,
    pub(crate) slot: u32,
}

/// Handle to a `Property` row, as `(owning type, position within that type)`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct PropertyId {
    pub(crate) owner: u32,
    pub(crate) slot: u32,
}

impl TypeDefId {
    /// The `<Module>` pseudo-type, which ECMA-335 requires as `TypeDef` row 1.
    pub const MODULE: Self = Self(0);
}

/// A token that can name a type: a definition, a reference, or a `TypeSpec`.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TypeDefOrRef {
    /// A type defined in this assembly.
    Definition(TypeDefId),
    /// A type imported from another assembly.
    Reference(TypeRefId),
    /// A constructed type held in the `TypeSpec` table.
    Specification(TypeSpecId),
}

impl TypeDefOrRef {
    /// The `(table, 1-based row)` this token names.
    pub(crate) fn target(self) -> Option<(u8, u32)> {
        let (table, index) = match self {
            Self::Definition(id) => (table::TYPE_DEF, id.0),
            Self::Reference(id) => (table::TYPE_REF, id.0),
            Self::Specification(id) => (table::TYPE_SPEC, id.0),
        };
        Some((table, index.checked_add(1)?))
    }

    /// The packed `TypeDefOrRef` coded index, as it appears both in table
    /// columns and (compressed) inside signature blobs.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::CodedIndexOverflow`] if the row number does not
    /// survive being shifted left by the two tag bits.
    pub fn coded_value(self) -> Result<u32> {
        let (table, row) = self.target().ok_or(crate::Error::CodedIndexOverflow {
            family: TYPE_DEF_OR_REF.name,
            table: table::TYPE_DEF,
            row: u32::MAX,
        })?;
        TYPE_DEF_OR_REF.encode(table, row)
    }
}

impl From<TypeDefId> for TypeDefOrRef {
    fn from(id: TypeDefId) -> Self {
        Self::Definition(id)
    }
}

impl From<TypeRefId> for TypeDefOrRef {
    fn from(id: TypeRefId) -> Self {
        Self::Reference(id)
    }
}

impl From<TypeSpecId> for TypeDefOrRef {
    fn from(id: TypeSpecId) -> Self {
        Self::Specification(id)
    }
}

/// Where a `TypeRef` is resolved.
///
/// For a net6.0-or-later reference assembly the corlib scope must be an
/// `AssemblyRef` naming `System.Runtime` with public key token
/// `b03f5f7f11d50a3a`; `mscorlib`, `netstandard` and `System.Private.CoreLib`
/// are all rejected by Roslyn with CS0012.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum ResolutionScope {
    /// This module (`Module` row 1).
    Module,
    /// Another assembly.
    AssemblyRef(AssemblyRefId),
    /// The enclosing type of a nested type reference.
    TypeRef(TypeRefId),
}

/// What a `MemberRef` row hangs off.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum MemberRefParent {
    /// A type defined here.
    TypeDef(TypeDefId),
    /// A type imported from elsewhere.
    TypeRef(TypeRefId),
    /// A constructed type.
    TypeSpec(TypeSpecId),
    /// A vararg call site on a local method.
    MethodDef(MethodId),
}

/// The constructor a `CustomAttribute` row invokes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CustomAttributeType {
    /// A constructor defined in this assembly.
    MethodDef(MethodId),
    /// A constructor imported from another assembly.
    MemberRef(MemberRefId),
}

/// What a `CustomAttribute` row decorates.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
#[non_exhaustive]
pub enum HasCustomAttribute {
    /// The assembly itself; this is where `ReferenceAssemblyAttribute` and
    /// `TargetFrameworkAttribute` go.
    Assembly,
    /// The module.
    Module,
    /// A type.
    TypeDef(TypeDefId),
    /// An imported type.
    TypeRef(TypeRefId),
    /// A field.
    Field(FieldId),
    /// A method.
    MethodDef(MethodId),
    /// A property.
    Property(PropertyId),
    /// A referenced assembly.
    AssemblyRef(AssemblyRefId),
}

#[cfg(test)]
mod tests {
    use super::{TypeDefId, TypeDefOrRef, TypeRefId, TypeSpecId};

    #[test]
    fn coded_values_match_the_hand_worked_examples() {
        // TypeDef row 7 -> (7 << 2) | 0 == 28; the enum field signature in the
        // reference emitter encodes exactly this.
        assert_eq!(
            TypeDefOrRef::Definition(TypeDefId(6))
                .coded_value()
                .unwrap(),
            28
        );
        // TypeRef row 1 -> (1 << 2) | 1 == 5.
        assert_eq!(
            TypeDefOrRef::Reference(TypeRefId(0)).coded_value().unwrap(),
            5
        );
        // TypeSpec row 1 -> (1 << 2) | 2 == 6.
        assert_eq!(
            TypeDefOrRef::Specification(TypeSpecId(0))
                .coded_value()
                .unwrap(),
            6
        );
    }
}
