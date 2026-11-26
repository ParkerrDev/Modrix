// SPDX-License-Identifier: GPL-2.0-only
//! Fully resolved metadata rows: every handle already turned into a row
//! number, every string and blob already turned into a heap index.
//!
//! These structs are the boundary between the two halves of the crate. The
//! writer materialises them and serialises them; the test-only reader parses
//! them back out of the finished bytes and re-checks every reference. Sharing
//! the struct - but not the code that produces it - is what makes the
//! round-trip a real check rather than a tautology.

/// `Module` (0x00).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ModuleRow {
    pub(crate) generation: u16,
    pub(crate) name: u32,
    pub(crate) mvid: u32,
    pub(crate) enc_id: u32,
    pub(crate) enc_base_id: u32,
}

/// `TypeRef` (0x01).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TypeRefRow {
    pub(crate) resolution_scope: u32,
    pub(crate) name: u32,
    pub(crate) namespace: u32,
}

/// `TypeDef` (0x02).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TypeDefRow {
    pub(crate) flags: u32,
    pub(crate) name: u32,
    pub(crate) namespace: u32,
    pub(crate) extends: u32,
    /// 1-based index of this type's first `Field` row. The run ends where the
    /// next type's start begins, so these must be non-decreasing.
    pub(crate) field_list: u32,
    /// 1-based index of this type's first `MethodDef` row.
    pub(crate) method_list: u32,
}

/// `Field` (0x04).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FieldRow {
    pub(crate) flags: u16,
    pub(crate) name: u32,
    pub(crate) signature: u32,
}

/// `MethodDef` (0x06). `rva` is always 0: a reference assembly has no bodies.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MethodDefRow {
    pub(crate) rva: u32,
    pub(crate) impl_flags: u16,
    pub(crate) flags: u16,
    pub(crate) name: u32,
    pub(crate) signature: u32,
    pub(crate) param_list: u32,
}

/// `Param` (0x08).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ParamRow {
    pub(crate) flags: u16,
    pub(crate) sequence: u16,
    pub(crate) name: u32,
}

/// `InterfaceImpl` (0x09), sorted by `class`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct InterfaceImplRow {
    pub(crate) class: u32,
    pub(crate) interface: u32,
}

/// `MemberRef` (0x0A).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MemberRefRow {
    pub(crate) class: u32,
    pub(crate) name: u32,
    pub(crate) signature: u32,
}

/// `Constant` (0x0B), sorted by `parent`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct ConstantRow {
    pub(crate) kind: u8,
    pub(crate) parent: u32,
    pub(crate) value: u32,
}

/// `CustomAttribute` (0x0C), sorted by `parent`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct CustomAttributeRow {
    pub(crate) parent: u32,
    pub(crate) constructor: u32,
    pub(crate) value: u32,
}

/// `PropertyMap` (0x15).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PropertyMapRow {
    pub(crate) parent: u32,
    pub(crate) property_list: u32,
}

/// `Property` (0x17).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct PropertyRow {
    pub(crate) flags: u16,
    pub(crate) name: u32,
    pub(crate) signature: u32,
}

/// `MethodSemantics` (0x18), sorted by `association`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct MethodSemanticsRow {
    pub(crate) semantics: u16,
    pub(crate) method: u32,
    pub(crate) association: u32,
}

/// `TypeSpec` (0x1B).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct TypeSpecRow {
    pub(crate) signature: u32,
}

/// `Assembly` (0x20). Exactly one row, and it is mandatory: without it Roslyn
/// treats the file as a netmodule and reports CS1509.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AssemblyRow {
    pub(crate) hash_algorithm: u32,
    pub(crate) version: [u16; 4],
    pub(crate) flags: u32,
    pub(crate) public_key: u32,
    pub(crate) name: u32,
    pub(crate) culture: u32,
}

/// `AssemblyRef` (0x23).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct AssemblyRefRow {
    pub(crate) version: [u16; 4],
    pub(crate) flags: u32,
    pub(crate) public_key_or_token: u32,
    pub(crate) name: u32,
    pub(crate) culture: u32,
    pub(crate) hash_value: u32,
}

/// `NestedClass` (0x29), sorted by `nested`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct NestedClassRow {
    pub(crate) nested: u32,
    pub(crate) enclosing: u32,
}

/// `GenericParam` (0x2A), sorted by `(owner, number)`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct GenericParamRow {
    pub(crate) number: u16,
    pub(crate) flags: u16,
    pub(crate) owner: u32,
    pub(crate) name: u32,
}

/// Every table of one assembly, with all indices resolved.
#[derive(Clone, Debug, Default)]
pub(crate) struct TableSet {
    pub(crate) module: Vec<ModuleRow>,
    pub(crate) type_refs: Vec<TypeRefRow>,
    pub(crate) type_defs: Vec<TypeDefRow>,
    pub(crate) fields: Vec<FieldRow>,
    pub(crate) methods: Vec<MethodDefRow>,
    pub(crate) params: Vec<ParamRow>,
    pub(crate) interface_impls: Vec<InterfaceImplRow>,
    pub(crate) member_refs: Vec<MemberRefRow>,
    pub(crate) constants: Vec<ConstantRow>,
    pub(crate) custom_attributes: Vec<CustomAttributeRow>,
    pub(crate) property_maps: Vec<PropertyMapRow>,
    pub(crate) properties: Vec<PropertyRow>,
    pub(crate) method_semantics: Vec<MethodSemanticsRow>,
    pub(crate) type_specs: Vec<TypeSpecRow>,
    pub(crate) assembly: Vec<AssemblyRow>,
    pub(crate) assembly_refs: Vec<AssemblyRefRow>,
    pub(crate) nested_classes: Vec<NestedClassRow>,
    pub(crate) generic_params: Vec<GenericParamRow>,
}
