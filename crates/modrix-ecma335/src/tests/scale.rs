// SPDX-License-Identifier: GPL-2.0-only
//! L1: synthetic assemblies sized to sit exactly on the width thresholds.
//!
//! 16 384 `TypeDef` rows is where `TypeDefOrRef` stops fitting in two bytes.
//! Below it nothing in this crate ever exercises a 4-byte coded index; above
//! it, every row of every table shifts. A writer with the `1 << 16` bug
//! produces a perfectly plausible file at 16 383 types and garbage at 16 384,
//! which is why the boundary is walked one row at a time.

use super::fixtures::{add_corlib_targeting, emit_and_verify};
use crate::builder::AssemblyBuilder;
use crate::handles::TypeRefId;
use crate::model::{
    Field, Method, TypeDefinition, Version, field_attributes, method_attributes, type_attributes,
};
use crate::signature::{MethodSignature, Type};
use crate::tables::{RowCounts, TYPE_DEF_OR_REF, table};

/// Builds an assembly with exactly `total` `TypeDef` rows, `<Module>`
/// included, each type carrying one field, a constructor and one method.
pub(crate) fn synthetic(total: u32, name_padding: usize) -> AssemblyBuilder {
    synthetic_targeting(total, name_padding, 9)
}

/// The same, bound to a specific .NET major version for the csc oracle.
pub(crate) fn synthetic_targeting(total: u32, name_padding: usize, major: u16) -> AssemblyBuilder {
    let mut builder = AssemblyBuilder::new("Scale", Version::new(1, 0, 0, 0));
    let corlib = add_corlib_targeting(&mut builder, major);
    let pad = "P".repeat(name_padding);
    for index in 0..total.saturating_sub(1) {
        add_synthetic_type(&mut builder, corlib.object, index, &pad);
    }
    builder
}

fn add_synthetic_type(builder: &mut AssemblyBuilder, object: TypeRefId, index: u32, pad: &str) {
    let type_id = builder
        .add_type_def(TypeDefinition::new(
            "Bulk",
            format!("T{index:06}{pad}"),
            type_attributes::PUBLIC_CLASS,
            Some(object.into()),
        ))
        .unwrap();
    builder
        .add_field(
            type_id,
            Field::new(
                format!("F{index:06}{pad}"),
                field_attributes::PUBLIC,
                Type::I4,
            ),
        )
        .unwrap();
    builder.add_default_constructor(type_id).unwrap();
    builder
        .add_method(
            type_id,
            Method::new(
                format!("M{index:06}{pad}"),
                method_attributes::PUBLIC_INSTANCE,
                MethodSignature::instance_method(Type::Void, Vec::new()),
            ),
        )
        .unwrap();
}

/// Asserts that the first and the last user type survived the round trip, and
/// returns the parsed image so callers can check widths.
fn check_scale(total: u32, name_padding: usize) -> crate::verify::Parsed {
    let (_, parsed) = emit_and_verify(&synthetic(total, name_padding));
    assert_eq!(parsed.counts.get(table::TYPE_DEF), total);
    let pad = "P".repeat(name_padding);
    let (namespace, first) = parsed.type_def_name(2).unwrap();
    assert_eq!(namespace, "Bulk");
    assert_eq!(first, format!("T{:06}{pad}", 0));
    let (_, last) = parsed.type_def_name(total).unwrap();
    assert_eq!(last, format!("T{:06}{pad}", total.saturating_sub(2)));
    parsed
}

/// What width the reader had to use for `TypeDefOrRef` in a parsed image.
fn type_def_or_ref_width(parsed: &crate::verify::Parsed) -> u8 {
    let mut counts = RowCounts::default();
    counts.set(table::TYPE_DEF, parsed.counts.get(table::TYPE_DEF));
    counts.set(table::TYPE_REF, parsed.counts.get(table::TYPE_REF));
    counts.set(table::TYPE_SPEC, parsed.counts.get(table::TYPE_SPEC));
    counts.coded_width(TYPE_DEF_OR_REF)
}

#[test]
fn a_single_type_assembly_round_trips() {
    let parsed = check_scale(2, 0);
    assert_eq!(parsed.counts.get(table::FIELD), 1);
    assert_eq!(parsed.counts.get(table::METHOD_DEF), 2);
}

#[test]
fn just_below_the_coded_index_threshold() {
    let parsed = check_scale(16_383, 0);
    assert_eq!(type_def_or_ref_width(&parsed), 2);
}

#[test]
fn exactly_at_the_coded_index_threshold() {
    let parsed = check_scale(16_384, 0);
    assert_eq!(type_def_or_ref_width(&parsed), 4);
}

#[test]
fn just_above_the_coded_index_threshold() {
    let parsed = check_scale(16_385, 0);
    assert_eq!(type_def_or_ref_width(&parsed), 4);
}

#[test]
fn well_past_the_coded_index_threshold() {
    let parsed = check_scale(20_007, 0);
    assert_eq!(type_def_or_ref_width(&parsed), 4);
    assert_eq!(parsed.counts.get(table::FIELD), 20_006);
    assert_eq!(parsed.counts.get(table::METHOD_DEF), 40_012);
}

/// A `#Strings` heap over 64 KB sets `HeapSizes` bit 0x01 and turns every
/// string column into a 4-byte index. The padding here forces that at a type
/// count far below the coded-index threshold, so the two effects are separable.
#[test]
fn a_large_string_heap_widens_string_indices() {
    let small = check_scale(64, 0);
    assert_eq!(small.heap_sizes & 0x01, 0);

    let large = check_scale(600, 40);
    assert_eq!(
        large.heap_sizes & 0x01,
        0x01,
        "a #Strings heap of {} bytes must set HeapSizes bit 0x01",
        large.strings.len()
    );
    assert!(large.strings.len() >= (1 << 16));
    // Only the string bit flips: the heap is large, the tables are not.
    assert_eq!(large.heap_sizes & 0x04, 0);
    assert_eq!(type_def_or_ref_width(&large), 2);
}
