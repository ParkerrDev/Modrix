// SPDX-License-Identifier: GPL-2.0-only
//! Serialises resolved rows into the `#~` table stream.
//!
//! Column widths arrive as a single [`Widths`] value computed from the final
//! row counts and heap sizes. Nothing in this module may consult a running
//! length: every width is already fixed before the first byte is written.

use crate::encode::{write_index, write_u16, write_u32, write_u64};
use crate::error::Result;
use crate::materialize::Image;
use crate::rows::TableSet;
use crate::tables::{EMITTED_TABLES, SORTED_MASK, Widths, table};

/// Builds the complete `#~` stream: header, row counts, then row data.
///
/// # Errors
///
/// Returns [`crate::Error::IndexTooWide`] if any column was handed a value
/// that does not fit the width computed for it.
pub(crate) fn table_stream(image: &Image) -> Result<Vec<u8>> {
    let widths = Widths::new(
        image.strings.bytes().len(),
        image.guids.count(),
        image.blobs.bytes().len(),
        image.counts,
    );
    let mut out = Vec::new();
    write_u32(&mut out, 0);
    out.push(2);
    out.push(0);
    out.push(widths.heap_sizes);
    // ECMA-335 II.24.2.6 shows this reserved byte as 1, not 0.
    out.push(1);
    let valid = image.counts.valid_mask();
    write_u64(&mut out, valid);
    write_u64(&mut out, SORTED_MASK);
    for table_number in 0..64_u8 {
        if valid & (1_u64 << table_number) != 0 {
            write_u32(&mut out, image.counts.get(table_number));
        }
    }
    write_rows(&mut out, &image.tables, widths)?;
    Ok(out)
}

/// Writes every present table's rows in ascending table-number order.
fn write_rows(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    for table_number in EMITTED_TABLES {
        write_table(out, tables, widths, table_number)?;
    }
    Ok(())
}

fn write_table(out: &mut Vec<u8>, tables: &TableSet, widths: Widths, number: u8) -> Result<()> {
    match number {
        table::MODULE => write_module(out, tables, widths),
        table::TYPE_REF => write_type_refs(out, tables, widths),
        table::TYPE_DEF => write_type_defs(out, tables, widths),
        table::FIELD => write_fields(out, tables, widths),
        table::METHOD_DEF => write_methods(out, tables, widths),
        table::PARAM => write_params(out, tables, widths),
        table::INTERFACE_IMPL => write_interface_impls(out, tables, widths),
        table::MEMBER_REF => write_member_refs(out, tables, widths),
        table::CONSTANT => write_constants(out, tables, widths),
        table::CUSTOM_ATTRIBUTE => write_custom_attributes(out, tables, widths),
        table::PROPERTY_MAP => write_property_maps(out, tables, widths),
        table::PROPERTY => write_properties(out, tables, widths),
        table::METHOD_SEMANTICS => write_method_semantics(out, tables, widths),
        table::TYPE_SPEC => write_type_specs(out, tables, widths),
        table::ASSEMBLY => write_assembly(out, tables, widths),
        table::ASSEMBLY_REF => write_assembly_refs(out, tables, widths),
        table::NESTED_CLASS => write_nested_classes(out, tables, widths),
        table::GENERIC_PARAM => write_generic_params(out, tables, widths),
        _ => Ok(()),
    }
}

fn write_module(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    for row in &tables.module {
        write_u16(out, row.generation);
        write_index(out, widths.string, row.name)?;
        write_index(out, widths.guid, row.mvid)?;
        write_index(out, widths.guid, row.enc_id)?;
        write_index(out, widths.guid, row.enc_base_id)?;
    }
    Ok(())
}

fn write_type_refs(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let scope = widths.resolution_scope;
    for row in &tables.type_refs {
        write_index(out, scope, row.resolution_scope)?;
        write_index(out, widths.string, row.name)?;
        write_index(out, widths.string, row.namespace)?;
    }
    Ok(())
}

fn write_type_defs(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let extends = widths.type_def_or_ref;
    let field = widths.field;
    let method = widths.method_def;
    for row in &tables.type_defs {
        write_u32(out, row.flags);
        write_index(out, widths.string, row.name)?;
        write_index(out, widths.string, row.namespace)?;
        write_index(out, extends, row.extends)?;
        write_index(out, field, row.field_list)?;
        write_index(out, method, row.method_list)?;
    }
    Ok(())
}

fn write_fields(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    for row in &tables.fields {
        write_u16(out, row.flags);
        write_index(out, widths.string, row.name)?;
        write_index(out, widths.blob, row.signature)?;
    }
    Ok(())
}

fn write_methods(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let param = widths.param;
    for row in &tables.methods {
        write_u32(out, row.rva);
        write_u16(out, row.impl_flags);
        write_u16(out, row.flags);
        write_index(out, widths.string, row.name)?;
        write_index(out, widths.blob, row.signature)?;
        write_index(out, param, row.param_list)?;
    }
    Ok(())
}

fn write_params(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    for row in &tables.params {
        write_u16(out, row.flags);
        write_u16(out, row.sequence);
        write_index(out, widths.string, row.name)?;
    }
    Ok(())
}

fn write_interface_impls(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let class = widths.type_def;
    let interface = widths.type_def_or_ref;
    for row in &tables.interface_impls {
        write_index(out, class, row.class)?;
        write_index(out, interface, row.interface)?;
    }
    Ok(())
}

fn write_member_refs(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let parent = widths.member_ref_parent;
    for row in &tables.member_refs {
        write_index(out, parent, row.class)?;
        write_index(out, widths.string, row.name)?;
        write_index(out, widths.blob, row.signature)?;
    }
    Ok(())
}

fn write_constants(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let parent = widths.has_constant;
    for row in &tables.constants {
        out.push(row.kind);
        out.push(0);
        write_index(out, parent, row.parent)?;
        write_index(out, widths.blob, row.value)?;
    }
    Ok(())
}

fn write_custom_attributes(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let parent = widths.has_custom_attribute;
    let constructor = widths.custom_attribute_type;
    for row in &tables.custom_attributes {
        write_index(out, parent, row.parent)?;
        write_index(out, constructor, row.constructor)?;
        write_index(out, widths.blob, row.value)?;
    }
    Ok(())
}

fn write_property_maps(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let parent = widths.type_def;
    let property = widths.property;
    for row in &tables.property_maps {
        write_index(out, parent, row.parent)?;
        write_index(out, property, row.property_list)?;
    }
    Ok(())
}

fn write_properties(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    for row in &tables.properties {
        write_u16(out, row.flags);
        write_index(out, widths.string, row.name)?;
        write_index(out, widths.blob, row.signature)?;
    }
    Ok(())
}

fn write_method_semantics(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let method = widths.method_def;
    let association = widths.has_semantics;
    for row in &tables.method_semantics {
        write_u16(out, row.semantics);
        write_index(out, method, row.method)?;
        write_index(out, association, row.association)?;
    }
    Ok(())
}

fn write_type_specs(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    for row in &tables.type_specs {
        write_index(out, widths.blob, row.signature)?;
    }
    Ok(())
}

fn write_assembly(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    for row in &tables.assembly {
        write_u32(out, row.hash_algorithm);
        for part in row.version {
            write_u16(out, part);
        }
        write_u32(out, row.flags);
        write_index(out, widths.blob, row.public_key)?;
        write_index(out, widths.string, row.name)?;
        write_index(out, widths.string, row.culture)?;
    }
    Ok(())
}

fn write_assembly_refs(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    for row in &tables.assembly_refs {
        for part in row.version {
            write_u16(out, part);
        }
        write_u32(out, row.flags);
        write_index(out, widths.blob, row.public_key_or_token)?;
        write_index(out, widths.string, row.name)?;
        write_index(out, widths.string, row.culture)?;
        write_index(out, widths.blob, row.hash_value)?;
    }
    Ok(())
}

fn write_nested_classes(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let type_def = widths.type_def;
    for row in &tables.nested_classes {
        write_index(out, type_def, row.nested)?;
        write_index(out, type_def, row.enclosing)?;
    }
    Ok(())
}

fn write_generic_params(out: &mut Vec<u8>, tables: &TableSet, widths: Widths) -> Result<()> {
    let owner = widths.type_or_method_def;
    for row in &tables.generic_params {
        write_u16(out, row.number);
        write_u16(out, row.flags);
        write_index(out, owner, row.owner)?;
        write_index(out, widths.string, row.name)?;
    }
    Ok(())
}
