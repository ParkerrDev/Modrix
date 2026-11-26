// SPDX-License-Identifier: GPL-2.0-only
//! Turns the builder's declarative description into resolved metadata rows.
//!
//! This is where handles become 1-based row numbers and strings become heap
//! offsets. It runs to completion *before* any width is computed, which is the
//! whole point: an ECMA-335 column is 2 or 4 bytes depending on final row
//! counts, so nothing may be serialised until every row exists.

use crate::builder::AssemblyBuilder;
use crate::error::{Error, Result};
use crate::handles::{
    CustomAttributeType, FieldId, HasCustomAttribute, MemberRefParent, MethodId, PropertyId,
    ResolutionScope,
};
use crate::heaps::{BlobHeap, GuidHeap, StringHeap};
use crate::model::{Constant, field_attributes};
use crate::rows::{
    AssemblyRefRow, AssemblyRow, ConstantRow, CustomAttributeRow, FieldRow, GenericParamRow,
    InterfaceImplRow, MemberRefRow, MethodDefRow, MethodSemanticsRow, ModuleRow, NestedClassRow,
    ParamRow, PropertyMapRow, PropertyRow, TableSet, TypeDefRow, TypeRefRow, TypeSpecRow,
};
use crate::signature::{element_type, field_signature, property_signature};
use crate::tables::{
    CUSTOM_ATTRIBUTE_TYPE, HAS_CONSTANT, HAS_CUSTOM_ATTRIBUTE, HAS_SEMANTICS, MEMBER_REF_PARENT,
    RESOLUTION_SCOPE, RowCounts, TYPE_OR_METHOD_DEF, table,
};

/// Ceiling on rows in any one table. Far above anything a real interop
/// assembly needs, and far below where 32-bit row arithmetic gets risky.
const MAX_ROWS: usize = 0x00FF_FFFF;

/// `MethodSemantics.Semantics` value for a property setter.
const SEMANTICS_SETTER: u16 = 0x0001;
/// `MethodSemantics.Semantics` value for a property getter.
const SEMANTICS_GETTER: u16 = 0x0002;

/// A fully resolved assembly: rows, heaps and final row counts.
pub(crate) struct Image {
    pub(crate) tables: TableSet,
    pub(crate) strings: StringHeap,
    pub(crate) blobs: BlobHeap,
    pub(crate) guids: GuidHeap,
    pub(crate) counts: RowCounts,
}

/// Where each type's member runs start, so a `(type, slot)` handle can be
/// turned into an absolute row number.
struct MemberLayout {
    fields: Vec<u32>,
    methods: Vec<u32>,
    properties: Vec<u32>,
}

/// Resolves the builder's description into [`Image`].
///
/// # Errors
///
/// Propagates heap and table overflow, dangling handles, literal fields with
/// no constant, and signature encoding failures.
pub(crate) fn materialize(builder: &AssemblyBuilder) -> Result<Image> {
    let mut state = Materializer::new(builder)?;
    state.emit_module()?;
    state.emit_type_refs()?;
    state.emit_types()?;
    state.emit_type_specs()?;
    state.emit_member_refs()?;
    state.emit_custom_attributes()?;
    state.emit_assembly()?;
    state.emit_assembly_refs()?;
    state.sort_tables();
    state.finish()
}

struct Materializer<'a> {
    builder: &'a AssemblyBuilder,
    tables: TableSet,
    strings: StringHeap,
    blobs: BlobHeap,
    guids: GuidHeap,
    layout: MemberLayout,
}

impl<'a> Materializer<'a> {
    fn new(builder: &'a AssemblyBuilder) -> Result<Self> {
        Ok(Self {
            layout: plan_members(builder)?,
            builder,
            tables: TableSet::default(),
            strings: StringHeap::new(),
            blobs: BlobHeap::new(),
            guids: GuidHeap::default(),
        })
    }

    fn emit_module(&mut self) -> Result<()> {
        let name = self.strings.intern(&self.builder.module_name)?;
        let mvid = self.guids.add(self.builder.mvid)?;
        self.tables.module.push(ModuleRow {
            generation: 0,
            name,
            mvid,
            enc_id: 0,
            enc_base_id: 0,
        });
        Ok(())
    }

    fn emit_type_refs(&mut self) -> Result<()> {
        for entry in &self.builder.type_refs {
            let scope = match entry.scope {
                ResolutionScope::Module => RESOLUTION_SCOPE.encode(table::MODULE, 1)?,
                ResolutionScope::AssemblyRef(id) => {
                    RESOLUTION_SCOPE.encode(table::ASSEMBLY_REF, row(id.0)?)?
                }
                ResolutionScope::TypeRef(id) => {
                    RESOLUTION_SCOPE.encode(table::TYPE_REF, row(id.0)?)?
                }
            };
            let row = TypeRefRow {
                resolution_scope: scope,
                name: self.strings.intern(&entry.name)?,
                namespace: self.strings.intern(&entry.namespace)?,
            };
            self.tables.type_refs.push(row);
        }
        Ok(())
    }

    fn emit_types(&mut self) -> Result<()> {
        for (index, entry) in self.builder.types.iter().enumerate() {
            let type_index = u32::try_from(index).unwrap_or(u32::MAX);
            let definition = &entry.definition;
            let extends = match definition.extends {
                Some(token) => token.coded_value()?,
                None => 0,
            };
            let row = TypeDefRow {
                flags: definition.flags,
                name: self.strings.intern(&definition.name)?,
                namespace: self.strings.intern(&definition.namespace)?,
                extends,
                field_list: start_of(&self.layout.fields, type_index)?,
                method_list: start_of(&self.layout.methods, type_index)?,
            };
            self.tables.type_defs.push(row);
            self.emit_fields(type_index)?;
            self.emit_methods(type_index)?;
            self.emit_properties(type_index)?;
            self.emit_type_relations(type_index)?;
        }
        Ok(())
    }

    fn emit_fields(&mut self, type_index: u32) -> Result<()> {
        let Some(entry) = self.builder.types.get(as_index(type_index)) else {
            return Ok(());
        };
        for (slot, field) in entry.fields.iter().enumerate() {
            let literal = field.flags & field_attributes::LITERAL != 0;
            if literal && field.constant.is_none() {
                return Err(Error::LiteralWithoutConstant {
                    name: field.name.clone(),
                });
            }
            let signature = field_signature(&field.field_type)?;
            let mut flags = field.flags;
            if field.constant.is_some() {
                flags |= field_attributes::HAS_DEFAULT;
            }
            let row = FieldRow {
                flags,
                name: self.strings.intern(&field.name)?,
                signature: self.blobs.intern(&signature)?,
            };
            self.tables.fields.push(row);
            if let Some(constant) = field.constant.as_ref() {
                let field_row = self.field_row(FieldId {
                    owner: type_index,
                    slot: u32::try_from(slot).unwrap_or(u32::MAX),
                })?;
                self.emit_constant(HAS_CONSTANT.encode(table::FIELD, field_row)?, constant)?;
            }
        }
        Ok(())
    }

    fn emit_constant(&mut self, parent: u32, constant: &Constant) -> Result<()> {
        let (kind, bytes) = encode_constant(constant);
        let value = self.blobs.intern(&bytes)?;
        self.tables.constants.push(ConstantRow {
            kind,
            parent,
            value,
        });
        Ok(())
    }

    fn emit_methods(&mut self, type_index: u32) -> Result<()> {
        let Some(entry) = self.builder.types.get(as_index(type_index)) else {
            return Ok(());
        };
        for (slot, method) in entry.methods.iter().enumerate() {
            let signature = method.signature.to_blob()?;
            let param_list = row(u32::try_from(self.tables.params.len()).unwrap_or(u32::MAX))?;
            let row_data = MethodDefRow {
                // A reference assembly carries no IL, so every body RVA is 0.
                rva: 0,
                impl_flags: method.impl_flags,
                flags: method.flags,
                name: self.strings.intern(&method.name)?,
                signature: self.blobs.intern(&signature)?,
                param_list,
            };
            self.tables.methods.push(row_data);
            for (position, parameter) in method.parameters.iter().enumerate() {
                let sequence = u16::try_from(position.saturating_add(1)).unwrap_or(u16::MAX);
                let name = self.strings.intern(&parameter.name)?;
                self.tables.params.push(ParamRow {
                    flags: parameter.flags,
                    sequence,
                    name,
                });
            }
            let method_row = self.method_row(MethodId {
                owner: type_index,
                slot: u32::try_from(slot).unwrap_or(u32::MAX),
            })?;
            let owner = TYPE_OR_METHOD_DEF.encode(table::METHOD_DEF, method_row)?;
            self.emit_generic_params(owner, &method.generic_parameters)?;
        }
        Ok(())
    }

    fn emit_properties(&mut self, type_index: u32) -> Result<()> {
        let Some(entry) = self.builder.types.get(as_index(type_index)) else {
            return Ok(());
        };
        if entry.properties.is_empty() {
            return Ok(());
        }
        self.tables.property_maps.push(PropertyMapRow {
            parent: row(type_index)?,
            property_list: start_of(&self.layout.properties, type_index)?,
        });
        for (slot, property) in entry.properties.iter().enumerate() {
            let signature = property_signature(
                property.has_this,
                &property.property_type,
                &property.index_parameters,
            )?;
            let row_data = PropertyRow {
                flags: property.flags,
                name: self.strings.intern(&property.name)?,
                signature: self.blobs.intern(&signature)?,
            };
            self.tables.properties.push(row_data);
            let property_row = self.property_row(PropertyId {
                owner: type_index,
                slot: u32::try_from(slot).unwrap_or(u32::MAX),
            })?;
            let association = HAS_SEMANTICS.encode(table::PROPERTY, property_row)?;
            self.emit_semantics(association, SEMANTICS_GETTER, property.getter)?;
            self.emit_semantics(association, SEMANTICS_SETTER, property.setter)?;
        }
        Ok(())
    }

    fn emit_semantics(
        &mut self,
        association: u32,
        semantics: u16,
        accessor: Option<MethodId>,
    ) -> Result<()> {
        let Some(accessor) = accessor else {
            return Ok(());
        };
        let method = self.method_row(accessor)?;
        self.tables.method_semantics.push(MethodSemanticsRow {
            semantics,
            method,
            association,
        });
        Ok(())
    }

    /// Interfaces, nesting and type-level generic parameters.
    fn emit_type_relations(&mut self, type_index: u32) -> Result<()> {
        let Some(entry) = self.builder.types.get(as_index(type_index)) else {
            return Ok(());
        };
        let class = row(type_index)?;
        for interface in &entry.definition.interfaces {
            self.tables.interface_impls.push(InterfaceImplRow {
                class,
                interface: interface.coded_value()?,
            });
        }
        if let Some(enclosing) = entry.definition.enclosing {
            self.tables.nested_classes.push(NestedClassRow {
                nested: class,
                enclosing: row(enclosing.0)?,
            });
        }
        let owner = TYPE_OR_METHOD_DEF.encode(table::TYPE_DEF, class)?;
        let parameters = entry.definition.generic_parameters.clone();
        self.emit_generic_params(owner, &parameters)
    }

    fn emit_generic_params(
        &mut self,
        owner: u32,
        parameters: &[crate::model::GenericParameter],
    ) -> Result<()> {
        for (position, parameter) in parameters.iter().enumerate() {
            let number = u16::try_from(position).unwrap_or(u16::MAX);
            let name = self.strings.intern(&parameter.name)?;
            self.tables.generic_params.push(GenericParamRow {
                number,
                flags: parameter.flags,
                owner,
                name,
            });
        }
        Ok(())
    }

    fn emit_type_specs(&mut self) -> Result<()> {
        for signature in &self.builder.type_specs {
            let blob = self.blobs.intern(signature)?;
            self.tables.type_specs.push(TypeSpecRow { signature: blob });
        }
        Ok(())
    }

    fn emit_member_refs(&mut self) -> Result<()> {
        for entry in &self.builder.member_refs {
            let class = match entry.parent {
                MemberRefParent::TypeDef(id) => {
                    MEMBER_REF_PARENT.encode(table::TYPE_DEF, row(id.0)?)?
                }
                MemberRefParent::TypeRef(id) => {
                    MEMBER_REF_PARENT.encode(table::TYPE_REF, row(id.0)?)?
                }
                MemberRefParent::TypeSpec(id) => {
                    MEMBER_REF_PARENT.encode(table::TYPE_SPEC, row(id.0)?)?
                }
                MemberRefParent::MethodDef(id) => {
                    MEMBER_REF_PARENT.encode(table::METHOD_DEF, self.method_row(id)?)?
                }
            };
            let row_data = MemberRefRow {
                class,
                name: self.strings.intern(&entry.name)?,
                signature: self.blobs.intern(&entry.signature)?,
            };
            self.tables.member_refs.push(row_data);
        }
        Ok(())
    }

    fn emit_custom_attributes(&mut self) -> Result<()> {
        for entry in &self.builder.custom_attributes {
            let parent = self.custom_attribute_parent(entry.parent)?;
            let constructor = match entry.constructor {
                CustomAttributeType::MethodDef(id) => {
                    CUSTOM_ATTRIBUTE_TYPE.encode(table::METHOD_DEF, self.method_row(id)?)?
                }
                CustomAttributeType::MemberRef(id) => {
                    CUSTOM_ATTRIBUTE_TYPE.encode(table::MEMBER_REF, row(id.0)?)?
                }
            };
            let value = self.blobs.intern(&entry.value)?;
            self.tables.custom_attributes.push(CustomAttributeRow {
                parent,
                constructor,
                value,
            });
        }
        Ok(())
    }

    fn custom_attribute_parent(&self, parent: HasCustomAttribute) -> Result<u32> {
        let (target, row_number) = match parent {
            HasCustomAttribute::Assembly => (table::ASSEMBLY, 1),
            HasCustomAttribute::Module => (table::MODULE, 1),
            HasCustomAttribute::TypeDef(id) => (table::TYPE_DEF, row(id.0)?),
            HasCustomAttribute::TypeRef(id) => (table::TYPE_REF, row(id.0)?),
            HasCustomAttribute::AssemblyRef(id) => (table::ASSEMBLY_REF, row(id.0)?),
            HasCustomAttribute::Field(id) => (table::FIELD, self.field_row(id)?),
            HasCustomAttribute::MethodDef(id) => (table::METHOD_DEF, self.method_row(id)?),
            HasCustomAttribute::Property(id) => (table::PROPERTY, self.property_row(id)?),
        };
        HAS_CUSTOM_ATTRIBUTE.encode(target, row_number)
    }

    fn emit_assembly(&mut self) -> Result<()> {
        let version = self.builder.version;
        let row_data = AssemblyRow {
            hash_algorithm: self.builder.hash_algorithm,
            version: [
                version.major,
                version.minor,
                version.build,
                version.revision,
            ],
            flags: self.builder.flags,
            public_key: self.blobs.intern(&self.builder.public_key)?,
            name: self.strings.intern(&self.builder.name)?,
            culture: self.strings.intern(&self.builder.culture)?,
        };
        self.tables.assembly.push(row_data);
        Ok(())
    }

    fn emit_assembly_refs(&mut self) -> Result<()> {
        for entry in &self.builder.assembly_refs {
            let version = entry.version;
            let row_data = AssemblyRefRow {
                version: [
                    version.major,
                    version.minor,
                    version.build,
                    version.revision,
                ],
                flags: entry.flags,
                public_key_or_token: self.blobs.intern(&entry.public_key_or_token)?,
                name: self.strings.intern(&entry.name)?,
                culture: self.strings.intern(&entry.culture)?,
                hash_value: 0,
            };
            self.tables.assembly_refs.push(row_data);
        }
        Ok(())
    }

    /// Applies the sort order ECMA-335 II.24.2.6 mandates for the tables named
    /// in the `Sorted` bitmask. Readers binary-search these.
    fn sort_tables(&mut self) {
        self.tables.constants.sort_by_key(|row| row.parent);
        self.tables.custom_attributes.sort_by_key(|row| row.parent);
        self.tables
            .interface_impls
            .sort_by_key(|row| (row.class, row.interface));
        self.tables
            .method_semantics
            .sort_by_key(|row| row.association);
        self.tables.nested_classes.sort_by_key(|row| row.nested);
        self.tables
            .generic_params
            .sort_by_key(|row| (row.owner, row.number));
    }

    fn field_row(&self, id: FieldId) -> Result<u32> {
        member_row(&self.layout.fields, id.owner, id.slot, "Field")
    }

    fn method_row(&self, id: MethodId) -> Result<u32> {
        member_row(&self.layout.methods, id.owner, id.slot, "MethodDef")
    }

    fn property_row(&self, id: PropertyId) -> Result<u32> {
        member_row(&self.layout.properties, id.owner, id.slot, "Property")
    }

    fn finish(self) -> Result<Image> {
        let counts = count_rows(&self.tables)?;
        Ok(Image {
            tables: self.tables,
            strings: self.strings,
            blobs: self.blobs,
            guids: self.guids,
            counts,
        })
    }
}

/// Walks the types once to fix where each type's `Field`, `MethodDef` and
/// `Property` runs begin.
///
/// ECMA-335 stores only the start of each run and infers the end from the next
/// row's start, so the runs must be contiguous and non-decreasing. Computing
/// them up front is what lets member handles stay `(type, slot)` pairs.
fn plan_members(builder: &AssemblyBuilder) -> Result<MemberLayout> {
    let mut layout = MemberLayout {
        fields: Vec::with_capacity(builder.types.len()),
        methods: Vec::with_capacity(builder.types.len()),
        properties: Vec::with_capacity(builder.types.len()),
    };
    let (mut fields, mut methods, mut properties) = (0_usize, 0_usize, 0_usize);
    for entry in &builder.types {
        layout.fields.push(row(count(fields, "Field")?)?);
        layout.methods.push(row(count(methods, "MethodDef")?)?);
        layout.properties.push(row(count(properties, "Property")?)?);
        fields = fields.saturating_add(entry.fields.len());
        methods = methods.saturating_add(entry.methods.len());
        properties = properties.saturating_add(entry.properties.len());
    }
    count(fields, "Field")?;
    count(methods, "MethodDef")?;
    count(properties, "Property")?;
    Ok(layout)
}

fn member_row(starts: &[u32], owner: u32, slot: u32, kind: &'static str) -> Result<u32> {
    let rows = u32::try_from(starts.len()).unwrap_or(u32::MAX);
    let start = starts
        .get(as_index(owner))
        .copied()
        .ok_or(Error::DanglingHandle {
            kind: "TypeDef",
            index: owner,
            rows,
        })?;
    start.checked_add(slot).ok_or(Error::DanglingHandle {
        kind,
        index: slot,
        rows,
    })
}

fn start_of(starts: &[u32], type_index: u32) -> Result<u32> {
    let rows = u32::try_from(starts.len()).unwrap_or(u32::MAX);
    starts
        .get(as_index(type_index))
        .copied()
        .ok_or(Error::DanglingHandle {
            kind: "TypeDef",
            index: type_index,
            rows,
        })
}

fn count(rows: usize, table: &'static str) -> Result<u32> {
    if rows > MAX_ROWS {
        return Err(Error::TableOverflow {
            table,
            rows,
            limit: MAX_ROWS,
        });
    }
    u32::try_from(rows).map_err(|_| Error::TableOverflow {
        table,
        rows,
        limit: MAX_ROWS,
    })
}

/// Converts a 0-based handle into a 1-based metadata row number.
fn row(index: u32) -> Result<u32> {
    index.checked_add(1).ok_or(Error::LayoutOverflow {
        what: "a metadata row number",
    })
}

fn as_index(value: u32) -> usize {
    usize::try_from(value).unwrap_or(usize::MAX)
}

fn count_rows(tables: &TableSet) -> Result<RowCounts> {
    let mut counts = RowCounts::default();
    counts.set(table::MODULE, count(tables.module.len(), "Module")?);
    counts.set(table::TYPE_REF, count(tables.type_refs.len(), "TypeRef")?);
    counts.set(table::TYPE_DEF, count(tables.type_defs.len(), "TypeDef")?);
    counts.set(table::FIELD, count(tables.fields.len(), "Field")?);
    counts.set(table::METHOD_DEF, count(tables.methods.len(), "MethodDef")?);
    counts.set(table::PARAM, count(tables.params.len(), "Param")?);
    let interfaces = count(tables.interface_impls.len(), "InterfaceImpl")?;
    counts.set(table::INTERFACE_IMPL, interfaces);
    counts.set(
        table::MEMBER_REF,
        count(tables.member_refs.len(), "MemberRef")?,
    );
    counts.set(table::CONSTANT, count(tables.constants.len(), "Constant")?);
    let attributes = count(tables.custom_attributes.len(), "CustomAttribute")?;
    counts.set(table::CUSTOM_ATTRIBUTE, attributes);
    counts.set(
        table::PROPERTY_MAP,
        count(tables.property_maps.len(), "PropertyMap")?,
    );
    counts.set(table::PROPERTY, count(tables.properties.len(), "Property")?);
    let semantics = count(tables.method_semantics.len(), "MethodSemantics")?;
    counts.set(table::METHOD_SEMANTICS, semantics);
    counts.set(
        table::TYPE_SPEC,
        count(tables.type_specs.len(), "TypeSpec")?,
    );
    counts.set(table::ASSEMBLY, count(tables.assembly.len(), "Assembly")?);
    let refs = count(tables.assembly_refs.len(), "AssemblyRef")?;
    counts.set(table::ASSEMBLY_REF, refs);
    let nested = count(tables.nested_classes.len(), "NestedClass")?;
    counts.set(table::NESTED_CLASS, nested);
    let generics = count(tables.generic_params.len(), "GenericParam")?;
    counts.set(table::GENERIC_PARAM, generics);
    Ok(counts)
}

/// The `(Constant.Type, Constant.Value)` pair for a compile-time value.
fn encode_constant(constant: &Constant) -> (u8, Vec<u8>) {
    match constant {
        Constant::Boolean(value) => (element_type::BOOLEAN, vec![u8::from(*value)]),
        Constant::Char(value) => (element_type::CHAR, value.to_le_bytes().to_vec()),
        Constant::I1(value) => (element_type::I1, value.to_le_bytes().to_vec()),
        Constant::U1(value) => (element_type::U1, value.to_le_bytes().to_vec()),
        Constant::I2(value) => (element_type::I2, value.to_le_bytes().to_vec()),
        Constant::U2(value) => (element_type::U2, value.to_le_bytes().to_vec()),
        Constant::I4(value) => (element_type::I4, value.to_le_bytes().to_vec()),
        Constant::U4(value) => (element_type::U4, value.to_le_bytes().to_vec()),
        Constant::I8(value) => (element_type::I8, value.to_le_bytes().to_vec()),
        Constant::U8(value) => (element_type::U8, value.to_le_bytes().to_vec()),
        Constant::R4(value) => (element_type::R4, value.to_le_bytes().to_vec()),
        Constant::R8(value) => (element_type::R8, value.to_le_bytes().to_vec()),
        Constant::String(value) => (
            element_type::STRING,
            value.encode_utf16().flat_map(u16::to_le_bytes).collect(),
        ),
        // A null literal is encoded as ELEMENT_TYPE_CLASS with a 4-byte zero.
        Constant::NullReference => (element_type::CLASS, vec![0, 0, 0, 0]),
    }
}
