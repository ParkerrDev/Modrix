// SPDX-License-Identifier: GPL-2.0-only
//! Metadata table numbers, coded-index families and column widths.
//!
//! Column widths are the highest-risk part of an ECMA-335 writer because a
//! wrong width does not fail - it shifts every subsequent byte of the table
//! stream, producing a file that parses into nonsense. Two rules make it work:
//!
//! 1. Widths are derived **only** from final row counts and final heap sizes,
//!    never from a running total.
//! 2. A coded index widens at `1 << (16 - tag_bits)`, **not** at `1 << 16`.
//!    `TypeDefOrRef` spends 2 bits on its tag, so it widens at 16 384 rows -
//!    a threshold a plausible assembly actually crosses.

/// ECMA-335 Partition II table numbers.
pub(crate) mod table {
    /// `Module` (II.22.30).
    pub(crate) const MODULE: u8 = 0x00;
    /// `TypeRef` (II.22.38).
    pub(crate) const TYPE_REF: u8 = 0x01;
    /// `TypeDef` (II.22.37).
    pub(crate) const TYPE_DEF: u8 = 0x02;
    /// `Field` (II.22.15).
    pub(crate) const FIELD: u8 = 0x04;
    /// `MethodDef` (II.22.26).
    pub(crate) const METHOD_DEF: u8 = 0x06;
    /// `Param` (II.22.33).
    pub(crate) const PARAM: u8 = 0x08;
    /// `InterfaceImpl` (II.22.23).
    pub(crate) const INTERFACE_IMPL: u8 = 0x09;
    /// `MemberRef` (II.22.25).
    pub(crate) const MEMBER_REF: u8 = 0x0A;
    /// `Constant` (II.22.9).
    pub(crate) const CONSTANT: u8 = 0x0B;
    /// `CustomAttribute` (II.22.10).
    pub(crate) const CUSTOM_ATTRIBUTE: u8 = 0x0C;
    /// `DeclSecurity` (II.22.11); never emitted, needed for coded-index tags.
    pub(crate) const DECL_SECURITY: u8 = 0x0E;
    /// `StandAloneSig` (II.22.36); never emitted, needed for tags.
    pub(crate) const STAND_ALONE_SIG: u8 = 0x11;
    /// `Event` (II.22.13); never emitted, needed for tags.
    pub(crate) const EVENT: u8 = 0x14;
    /// `PropertyMap` (II.22.35).
    pub(crate) const PROPERTY_MAP: u8 = 0x15;
    /// `Property` (II.22.34).
    pub(crate) const PROPERTY: u8 = 0x17;
    /// `MethodSemantics` (II.22.28).
    pub(crate) const METHOD_SEMANTICS: u8 = 0x18;
    /// `ModuleRef` (II.22.31); never emitted, needed for tags.
    pub(crate) const MODULE_REF: u8 = 0x1A;
    /// `TypeSpec` (II.22.39).
    pub(crate) const TYPE_SPEC: u8 = 0x1B;
    /// `Assembly` (II.22.2).
    pub(crate) const ASSEMBLY: u8 = 0x20;
    /// `AssemblyRef` (II.22.5).
    pub(crate) const ASSEMBLY_REF: u8 = 0x23;
    /// `File` (II.22.19); never emitted, needed for tags.
    pub(crate) const FILE: u8 = 0x26;
    /// `ExportedType` (II.22.14); never emitted, needed for tags.
    pub(crate) const EXPORTED_TYPE: u8 = 0x27;
    /// `ManifestResource` (II.22.24); never emitted, needed for tags.
    pub(crate) const MANIFEST_RESOURCE: u8 = 0x28;
    /// `NestedClass` (II.22.32).
    pub(crate) const NESTED_CLASS: u8 = 0x29;
    /// `GenericParam` (II.22.20).
    pub(crate) const GENERIC_PARAM: u8 = 0x2A;
    /// `MethodSpec` (II.22.29); never emitted, needed for tags.
    pub(crate) const METHOD_SPEC: u8 = 0x2B;
    /// `GenericParamConstraint` (II.22.21); never emitted, needed for tags.
    pub(crate) const GENERIC_PARAM_CONSTRAINT: u8 = 0x2C;
}

/// The tables this writer emits, in the ascending order the `#~` stream
/// requires. Row data follows the same order, contiguous and unpadded.
pub(crate) const EMITTED_TABLES: [u8; 18] = [
    table::MODULE,
    table::TYPE_REF,
    table::TYPE_DEF,
    table::FIELD,
    table::METHOD_DEF,
    table::PARAM,
    table::INTERFACE_IMPL,
    table::MEMBER_REF,
    table::CONSTANT,
    table::CUSTOM_ATTRIBUTE,
    table::PROPERTY_MAP,
    table::PROPERTY,
    table::METHOD_SEMANTICS,
    table::TYPE_SPEC,
    table::ASSEMBLY,
    table::ASSEMBLY_REF,
    table::NESTED_CLASS,
    table::GENERIC_PARAM,
];

/// The `Sorted` bitmask Roslyn writes: exactly the tables ECMA-335 requires to
/// be sorted by their key column.
pub(crate) const SORTED_MASK: u64 = 0x0000_1600_3301_FA00;

/// One coded-index family: a tag width plus the table each tag value selects.
///
/// The slot position *is* the tag value, so unused tags are `None`
/// (`CustomAttributeType` leaves 0, 1 and 4 unused).
#[derive(Clone, Copy, Debug)]
pub(crate) struct CodedIndex {
    /// Family name, used in diagnostics.
    pub(crate) name: &'static str,
    /// How many low bits hold the tag.
    pub(crate) tag_bits: u8,
    /// Tag value -> table number.
    pub(crate) tags: &'static [Option<u8>],
}

impl CodedIndex {
    /// Row count at which this family switches from 2-byte to 4-byte columns.
    ///
    /// This is `1 << (16 - tag_bits)`. Using `1 << 16` here instead is the
    /// classic silent-corruption bug: `TypeDefOrRef` would stay two bytes past
    /// 16 383 rows and every row after it would be misparsed.
    pub(crate) const fn threshold(self) -> u32 {
        1_u32 << (16_u8.saturating_sub(self.tag_bits))
    }

    /// Packs a 1-based `row` of `table` into this family's encoding.
    ///
    /// # Errors
    ///
    /// Returns [`crate::Error::CodedIndexOverflow`] if `table` is not part of
    /// this family or the shifted row would not fit in 32 bits.
    pub(crate) fn encode(self, table: u8, row: u32) -> crate::Result<u32> {
        let tag = self
            .tags
            .iter()
            .position(|slot| *slot == Some(table))
            .and_then(|tag| u32::try_from(tag).ok())
            .ok_or(crate::Error::CodedIndexOverflow {
                family: self.name,
                table,
                row,
            })?;
        row.checked_shl(u32::from(self.tag_bits))
            .filter(|shifted| (shifted >> self.tag_bits) == row)
            .and_then(|shifted| shifted.checked_add(tag))
            .ok_or(crate::Error::CodedIndexOverflow {
                family: self.name,
                table,
                row,
            })
    }

    /// Splits a coded index back into `(table, row)`; `row` 0 means nil.
    #[cfg(test)]
    pub(crate) fn decode(self, value: u32) -> Option<(u8, u32)> {
        let tag_mask = (1_u32 << self.tag_bits).saturating_sub(1);
        let tag = usize::try_from(value & tag_mask).ok()?;
        let table = (*self.tags.get(tag)?)?;
        Some((table, value >> self.tag_bits))
    }
}

/// `TypeDefOrRef`: the family used by `Extends`, `InterfaceImpl.Interface` and
/// every `CLASS`/`VALUETYPE` token inside a signature blob.
pub(crate) const TYPE_DEF_OR_REF: CodedIndex = CodedIndex {
    name: "TypeDefOrRef",
    tag_bits: 2,
    tags: &[
        Some(table::TYPE_DEF),
        Some(table::TYPE_REF),
        Some(table::TYPE_SPEC),
    ],
};

/// `HasConstant`: the parent of a `Constant` row.
pub(crate) const HAS_CONSTANT: CodedIndex = CodedIndex {
    name: "HasConstant",
    tag_bits: 2,
    tags: &[
        Some(table::FIELD),
        Some(table::PARAM),
        Some(table::PROPERTY),
    ],
};

/// `HasCustomAttribute`: the widest family at 5 tag bits, so it widens at only
/// 2 048 rows.
pub(crate) const HAS_CUSTOM_ATTRIBUTE: CodedIndex = CodedIndex {
    name: "HasCustomAttribute",
    tag_bits: 5,
    tags: &[
        Some(table::METHOD_DEF),
        Some(table::FIELD),
        Some(table::TYPE_REF),
        Some(table::TYPE_DEF),
        Some(table::PARAM),
        Some(table::INTERFACE_IMPL),
        Some(table::MEMBER_REF),
        Some(table::MODULE),
        Some(table::DECL_SECURITY),
        Some(table::PROPERTY),
        Some(table::EVENT),
        Some(table::STAND_ALONE_SIG),
        Some(table::MODULE_REF),
        Some(table::TYPE_SPEC),
        Some(table::ASSEMBLY),
        Some(table::ASSEMBLY_REF),
        Some(table::FILE),
        Some(table::EXPORTED_TYPE),
        Some(table::MANIFEST_RESOURCE),
        Some(table::GENERIC_PARAM),
        Some(table::METHOD_SPEC),
        Some(table::GENERIC_PARAM_CONSTRAINT),
    ],
};

/// `CustomAttributeType`: only tags 2 and 3 are defined.
pub(crate) const CUSTOM_ATTRIBUTE_TYPE: CodedIndex = CodedIndex {
    name: "CustomAttributeType",
    tag_bits: 3,
    tags: &[
        None,
        None,
        Some(table::METHOD_DEF),
        Some(table::MEMBER_REF),
        None,
    ],
};

/// `MemberRefParent`: the owner of a `MemberRef` row.
pub(crate) const MEMBER_REF_PARENT: CodedIndex = CodedIndex {
    name: "MemberRefParent",
    tag_bits: 3,
    tags: &[
        Some(table::TYPE_DEF),
        Some(table::TYPE_REF),
        Some(table::MODULE_REF),
        Some(table::METHOD_DEF),
        Some(table::TYPE_SPEC),
    ],
};

/// `HasSemantics`: what a `MethodSemantics` row attaches to.
pub(crate) const HAS_SEMANTICS: CodedIndex = CodedIndex {
    name: "HasSemantics",
    tag_bits: 1,
    tags: &[Some(table::EVENT), Some(table::PROPERTY)],
};

/// `TypeOrMethodDef`: what owns a `GenericParam` row.
pub(crate) const TYPE_OR_METHOD_DEF: CodedIndex = CodedIndex {
    name: "TypeOrMethodDef",
    tag_bits: 1,
    tags: &[Some(table::TYPE_DEF), Some(table::METHOD_DEF)],
};

/// `ResolutionScope`: where a `TypeRef` is looked up.
pub(crate) const RESOLUTION_SCOPE: CodedIndex = CodedIndex {
    name: "ResolutionScope",
    tag_bits: 2,
    tags: &[
        Some(table::MODULE),
        Some(table::MODULE_REF),
        Some(table::ASSEMBLY_REF),
        Some(table::TYPE_REF),
    ],
};

/// `MethodDefOrRef`: modelled for completeness; used by `MethodImpl` and
/// `MethodSpec`, neither of which a reference assembly needs.
#[cfg(test)]
pub(crate) const METHOD_DEF_OR_REF: CodedIndex = CodedIndex {
    name: "MethodDefOrRef",
    tag_bits: 1,
    tags: &[Some(table::METHOD_DEF), Some(table::MEMBER_REF)],
};

/// `HasFieldMarshall`: modelled for completeness.
#[cfg(test)]
pub(crate) const HAS_FIELD_MARSHALL: CodedIndex = CodedIndex {
    name: "HasFieldMarshall",
    tag_bits: 1,
    tags: &[Some(table::FIELD), Some(table::PARAM)],
};

/// `HasDeclSecurity`: modelled for completeness.
#[cfg(test)]
pub(crate) const HAS_DECL_SECURITY: CodedIndex = CodedIndex {
    name: "HasDeclSecurity",
    tag_bits: 2,
    tags: &[
        Some(table::TYPE_DEF),
        Some(table::METHOD_DEF),
        Some(table::ASSEMBLY),
    ],
};

/// `MemberForwarded`: modelled for completeness.
#[cfg(test)]
pub(crate) const MEMBER_FORWARDED: CodedIndex = CodedIndex {
    name: "MemberForwarded",
    tag_bits: 1,
    tags: &[Some(table::FIELD), Some(table::METHOD_DEF)],
};

/// `Implementation`: modelled for completeness.
#[cfg(test)]
pub(crate) const IMPLEMENTATION: CodedIndex = CodedIndex {
    name: "Implementation",
    tag_bits: 2,
    tags: &[
        Some(table::FILE),
        Some(table::ASSEMBLY_REF),
        Some(table::EXPORTED_TYPE),
    ],
};

/// Every family, for exhaustive width testing.
#[cfg(test)]
pub(crate) const ALL_CODED_INDICES: [CodedIndex; 13] = [
    TYPE_DEF_OR_REF,
    HAS_CONSTANT,
    HAS_CUSTOM_ATTRIBUTE,
    CUSTOM_ATTRIBUTE_TYPE,
    MEMBER_REF_PARENT,
    HAS_SEMANTICS,
    TYPE_OR_METHOD_DEF,
    RESOLUTION_SCOPE,
    METHOD_DEF_OR_REF,
    HAS_FIELD_MARSHALL,
    HAS_DECL_SECURITY,
    MEMBER_FORWARDED,
    IMPLEMENTATION,
];

/// Final row counts for all 64 tables, the sole input to every width decision.
#[derive(Clone, Copy, Debug)]
pub(crate) struct RowCounts {
    counts: [u32; 64],
}

impl Default for RowCounts {
    fn default() -> Self {
        Self { counts: [0; 64] }
    }
}

impl RowCounts {
    /// Records `rows` for `table`.
    pub(crate) fn set(&mut self, table: u8, rows: u32) {
        if let Some(slot) = self.counts.get_mut(usize::from(table)) {
            *slot = rows;
        }
    }

    /// Row count for `table`, or 0 when the table is absent.
    pub(crate) fn get(self, table: u8) -> u32 {
        self.counts.get(usize::from(table)).copied().unwrap_or(0)
    }

    /// The `Valid` bitmask: bit N set iff table N has at least one row.
    pub(crate) fn valid_mask(self) -> u64 {
        let mut mask = 0_u64;
        for (index, rows) in self.counts.iter().enumerate() {
            if *rows > 0 && index < 64 {
                mask |= 1_u64 << index;
            }
        }
        mask
    }

    /// Width of a *simple* index into `table`: 2 bytes below 65 536 rows.
    pub(crate) fn simple_width(self, table: u8) -> u8 {
        if self.get(table) >= (1 << 16) { 4 } else { 2 }
    }

    /// Width of a coded index: 4 bytes as soon as *any* table in the family
    /// reaches the family's threshold.
    pub(crate) fn coded_width(self, family: CodedIndex) -> u8 {
        let threshold = family.threshold();
        let wide = family
            .tags
            .iter()
            .flatten()
            .any(|table| self.get(*table) >= threshold);
        if wide { 4 } else { 2 }
    }
}

/// Every column width one assembly needs, resolved once from final row counts
/// and final heap sizes.
///
/// The widths are materialised into fields rather than recomputed per column
/// so that no serialisation step can accidentally consult a row count that has
/// moved on. It is also small enough to pass by value.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Widths {
    /// `#Strings` index width.
    pub(crate) string: u8,
    /// `#GUID` index width.
    pub(crate) guid: u8,
    /// `#Blob` index width.
    pub(crate) blob: u8,
    /// The `HeapSizes` byte the three heap widths encode to.
    pub(crate) heap_sizes: u8,
    /// Simple index into `Field`.
    pub(crate) field: u8,
    /// Simple index into `MethodDef`.
    pub(crate) method_def: u8,
    /// Simple index into `Param`.
    pub(crate) param: u8,
    /// Simple index into `TypeDef`.
    pub(crate) type_def: u8,
    /// Simple index into `Property`.
    pub(crate) property: u8,
    /// `TypeDefOrRef` coded index.
    pub(crate) type_def_or_ref: u8,
    /// `ResolutionScope` coded index.
    pub(crate) resolution_scope: u8,
    /// `HasConstant` coded index.
    pub(crate) has_constant: u8,
    /// `HasCustomAttribute` coded index.
    pub(crate) has_custom_attribute: u8,
    /// `CustomAttributeType` coded index.
    pub(crate) custom_attribute_type: u8,
    /// `MemberRefParent` coded index.
    pub(crate) member_ref_parent: u8,
    /// `HasSemantics` coded index.
    pub(crate) has_semantics: u8,
    /// `TypeOrMethodDef` coded index.
    pub(crate) type_or_method_def: u8,
}

impl Widths {
    /// Derives widths from final heap sizes and final row counts.
    pub(crate) fn new(strings: usize, guids: usize, blobs: usize, rows: RowCounts) -> Self {
        let mut heap_sizes = 0_u8;
        if strings >= (1 << 16) {
            heap_sizes |= 0x01;
        }
        if guids >= (1 << 16) {
            heap_sizes |= 0x02;
        }
        if blobs >= (1 << 16) {
            heap_sizes |= 0x04;
        }
        Self::from_heap_sizes(heap_sizes, rows)
    }

    /// Rebuilds widths from an already-encoded `HeapSizes` byte. The reader
    /// uses this so it derives widths exactly the way the writer did.
    pub(crate) fn from_heap_sizes(heap_sizes: u8, rows: RowCounts) -> Self {
        Self {
            string: if heap_sizes & 0x01 == 0 { 2 } else { 4 },
            guid: if heap_sizes & 0x02 == 0 { 2 } else { 4 },
            blob: if heap_sizes & 0x04 == 0 { 2 } else { 4 },
            heap_sizes,
            field: rows.simple_width(table::FIELD),
            method_def: rows.simple_width(table::METHOD_DEF),
            param: rows.simple_width(table::PARAM),
            type_def: rows.simple_width(table::TYPE_DEF),
            property: rows.simple_width(table::PROPERTY),
            type_def_or_ref: rows.coded_width(TYPE_DEF_OR_REF),
            resolution_scope: rows.coded_width(RESOLUTION_SCOPE),
            has_constant: rows.coded_width(HAS_CONSTANT),
            has_custom_attribute: rows.coded_width(HAS_CUSTOM_ATTRIBUTE),
            custom_attribute_type: rows.coded_width(CUSTOM_ATTRIBUTE_TYPE),
            member_ref_parent: rows.coded_width(MEMBER_REF_PARENT),
            has_semantics: rows.coded_width(HAS_SEMANTICS),
            type_or_method_def: rows.coded_width(TYPE_OR_METHOD_DEF),
        }
    }
}
