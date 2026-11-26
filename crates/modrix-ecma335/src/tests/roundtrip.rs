// SPDX-License-Identifier: GPL-2.0-only
//! L0: emit, re-parse, and prove every row and reference survived.

use super::fixtures::{add_corlib, box_of, emit_and_verify, full_surface, full_surface_marked};
use crate::builder::AssemblyBuilder;
use crate::handles::{HasCustomAttribute, TypeDefOrRef};
use crate::model::{
    CORE_LIBRARY_NAME, CORE_LIBRARY_PUBLIC_KEY_TOKEN, Constant, Field, Method, TypeDefinition,
    Version, field_attributes, method_attributes, type_attributes,
};
use crate::signature::{MethodSignature, Type};
use crate::tables::table;

#[test]
fn the_full_surface_assembly_round_trips() {
    let (bytes, parsed) = emit_and_verify(&full_surface());
    assert_eq!(bytes.get(..2), Some(b"MZ".as_slice()));
    // Seven types: <Module>, IThing, Box`1, Widget, Inner, Point, Color.
    assert_eq!(parsed.counts.get(table::TYPE_DEF), 7);
    assert_eq!(
        parsed.type_def_name(1).unwrap(),
        (String::new(), "<Module>".to_owned())
    );
    assert_eq!(
        parsed.type_def_name(2).unwrap(),
        ("MyLib".to_owned(), "IThing".to_owned())
    );
    assert_eq!(
        parsed.type_def_name(7).unwrap(),
        ("MyLib".to_owned(), "Color".to_owned())
    );
}

#[test]
fn all_five_streams_are_present_in_roslyns_order() {
    let (_, parsed) = emit_and_verify(&full_surface());
    assert_eq!(
        parsed.stream_names,
        vec!["#~", "#Strings", "#US", "#GUID", "#Blob"]
    );
}

/// The metadata root's `Length` field is read as `16 + Length` to find the
/// stream headers, so an unpadded 11 makes Roslyn abort with CS0009.
#[test]
fn the_metadata_version_length_is_a_multiple_of_four() {
    let (_, parsed) = emit_and_verify(&full_surface());
    assert_eq!(parsed.version_length % 4, 0);
    assert_eq!(parsed.version_length, 12);
}

#[test]
fn the_assembly_row_and_corlib_reference_are_exactly_right() {
    let (_, parsed) = emit_and_verify(&full_surface());
    assert_eq!(parsed.counts.get(table::ASSEMBLY), 1);
    let assembly = parsed.tables.assembly.first().copied().unwrap();
    assert_eq!(parsed.string(assembly.name).unwrap(), "MyLib");
    assert_eq!(assembly.version, [1, 0, 0, 0]);

    assert_eq!(
        parsed.assembly_ref_names().unwrap(),
        vec![CORE_LIBRARY_NAME]
    );
    let reference = parsed.tables.assembly_refs.first().copied().unwrap();
    assert_eq!(reference.version, [9, 0, 0, 0]);
    assert_eq!(
        parsed.blob(reference.public_key_or_token).unwrap(),
        CORE_LIBRARY_PUBLIC_KEY_TOKEN.to_vec()
    );
}

/// Enum literals without a `Constant` row make the *consumer* fail with a bare
/// CS7038, so the writer refuses to emit one.
#[test]
fn enum_literals_get_constant_rows() {
    let (_, parsed) = emit_and_verify(&full_surface());
    assert_eq!(parsed.counts.get(table::CONSTANT), 2);
    let values: Vec<Vec<u8>> = parsed
        .tables
        .constants
        .iter()
        .map(|row| parsed.blob(row.value).unwrap())
        .collect();
    assert!(values.contains(&vec![0, 0, 0, 0]));
    assert!(values.contains(&vec![1, 0, 0, 0]));
    assert!(
        parsed.tables.constants.windows(2).all(|pair| {
            pair.first().map(|row| row.parent) <= pair.get(1).map(|row| row.parent)
        })
    );
}

#[test]
fn a_literal_field_without_a_constant_is_refused() {
    let mut builder = AssemblyBuilder::new("Bad", Version::new(1, 0, 0, 0));
    let corlib = add_corlib(&mut builder);
    let color = builder
        .add_type_def(TypeDefinition::new(
            "Bad",
            "Color",
            type_attributes::PUBLIC_VALUE_TYPE,
            Some(corlib.enumeration.into()),
        ))
        .unwrap();
    builder
        .add_field(
            color,
            Field::new("value__", field_attributes::ENUM_BACKING, Type::I4),
        )
        .unwrap();
    builder
        .add_field(
            color,
            Field::new(
                "Red",
                field_attributes::ENUM_MEMBER,
                Type::ValueType(TypeDefOrRef::Definition(color)),
            ),
        )
        .unwrap();
    assert!(matches!(
        builder.build(),
        Err(crate::Error::LiteralWithoutConstant { .. })
    ));
}

/// Member runs are the invariant that makes `(type, slot)` handles safe: each
/// type's start must be where the previous type's run ended.
#[test]
fn member_runs_are_contiguous_and_in_type_order() {
    let (_, parsed) = emit_and_verify(&full_surface());
    let starts: Vec<u32> = parsed
        .tables
        .type_defs
        .iter()
        .map(|row| row.field_list)
        .collect();
    // <Module> owns nothing, IThing owns nothing, Box owns one field.
    assert_eq!(starts.first().copied(), Some(1));
    assert_eq!(starts.get(1).copied(), Some(1));
    assert_eq!(starts.get(2).copied(), Some(1));
    assert_eq!(starts.get(3).copied(), Some(2));
    let last = starts.last().copied().unwrap();
    assert!(last <= parsed.counts.get(table::FIELD).saturating_add(1));
}

#[test]
fn nested_types_carry_no_namespace_and_get_a_nested_class_row() {
    let (_, parsed) = emit_and_verify(&full_surface());
    assert_eq!(parsed.counts.get(table::NESTED_CLASS), 1);
    let nested = parsed.tables.nested_classes.first().copied().unwrap();
    let (namespace, name) = parsed.type_def_name(nested.nested).unwrap();
    assert_eq!(name, "Inner");
    assert!(namespace.is_empty(), "expected empty, got {namespace:?}");
    let (_, enclosing) = parsed.type_def_name(nested.enclosing).unwrap();
    assert_eq!(enclosing, "Widget");
}

#[test]
fn a_nested_type_with_a_namespace_is_refused() {
    let mut builder = AssemblyBuilder::new("Bad", Version::new(1, 0, 0, 0));
    let corlib = add_corlib(&mut builder);
    let outer = builder
        .add_type_def(TypeDefinition::new(
            "Bad",
            "Outer",
            type_attributes::PUBLIC_CLASS,
            Some(corlib.object.into()),
        ))
        .unwrap();
    let mut inner = TypeDefinition::new(
        "Bad",
        "Inner",
        type_attributes::NESTED_PUBLIC,
        Some(corlib.object.into()),
    );
    inner.enclosing = Some(outer);
    assert!(matches!(
        builder.add_type_def(inner),
        Err(crate::Error::NestedTypeHasNamespace { .. })
    ));
}

#[test]
fn generic_parameters_land_on_both_types_and_methods() {
    let (_, parsed) = emit_and_verify(&full_surface());
    // One on Box`1, one on Widget.Swap<T>.
    assert_eq!(parsed.counts.get(table::GENERIC_PARAM), 2);
    for row in &parsed.tables.generic_params {
        assert_eq!(parsed.string(row.name).unwrap(), "T");
        assert_eq!(row.number, 0);
    }
}

#[test]
fn properties_produce_a_map_and_two_semantics_rows() {
    let (_, parsed) = emit_and_verify(&full_surface());
    assert_eq!(parsed.counts.get(table::PROPERTY_MAP), 1);
    assert_eq!(parsed.counts.get(table::PROPERTY), 1);
    assert_eq!(parsed.counts.get(table::METHOD_SEMANTICS), 2);
    let property = parsed.tables.properties.first().copied().unwrap();
    assert_eq!(parsed.string(property.name).unwrap(), "Count");
    // PROPERTY | HASTHIS, zero index parameters, ELEMENT_TYPE_I4.
    assert_eq!(
        parsed.blob(property.signature).unwrap(),
        vec![0x28, 0x00, 0x08]
    );
}

/// A `GENERICINST` inside a field signature needs no `TypeSpec` row; the
/// fixture uses `Box<int>` as a field type and emits zero of them.
#[test]
fn generic_instances_in_signatures_need_no_type_spec() {
    let (_, parsed) = emit_and_verify(&full_surface());
    assert_eq!(parsed.counts.get(table::TYPE_SPEC), 0);
    let signatures: Vec<Vec<u8>> = parsed
        .tables
        .fields
        .iter()
        .map(|row| parsed.blob(row.signature).unwrap())
        .collect();
    // FIELD, GENERICINST, CLASS, TypeDefOrRef(Box`1 == row 3 -> 12), 1 arg, I4.
    assert!(signatures.contains(&vec![0x06, 0x15, 0x12, 0x0C, 0x01, 0x08]));
    // FIELD, SZARRAY, STRING.
    assert!(signatures.contains(&vec![0x06, 0x1D, 0x0E]));
}

#[test]
fn method_signatures_match_the_hand_worked_encodings() {
    let (_, parsed) = emit_and_verify(&full_surface());
    let blobs: Vec<Vec<u8>> = parsed
        .tables
        .methods
        .iter()
        .map(|row| parsed.blob(row.signature).unwrap())
        .collect();
    // instance void .ctor()
    assert!(blobs.contains(&vec![0x20, 0x00, 0x01]));
    // static int32 Add(int32, int32)
    assert!(blobs.contains(&vec![0x00, 0x02, 0x08, 0x08, 0x08]));
    // static void Swap<T>(!!0&, !!0&)
    assert!(blobs.contains(&vec![
        0x10, 0x01, 0x02, 0x01, 0x10, 0x1E, 0x00, 0x10, 0x1E, 0x00
    ]));
}

#[test]
fn the_reference_assembly_markers_round_trip() {
    let (_, parsed) = emit_and_verify(&full_surface_marked());
    assert_eq!(parsed.counts.get(table::CUSTOM_ATTRIBUTE), 2);
    assert_eq!(parsed.counts.get(table::MEMBER_REF), 2);
    let values: Vec<Vec<u8>> = parsed
        .tables
        .custom_attributes
        .iter()
        .map(|row| parsed.blob(row.value).unwrap())
        .collect();
    assert!(values.contains(&vec![0x01, 0x00, 0x00, 0x00]));
    assert!(values.iter().any(|value| value.len() > 4));
}

#[test]
fn output_is_byte_for_byte_reproducible() {
    let builder = full_surface();
    assert_eq!(builder.build().unwrap(), builder.build().unwrap());
    assert_eq!(full_surface().build().unwrap(), builder.build().unwrap());
}

#[test]
fn a_custom_attribute_on_a_member_resolves() {
    let mut builder = AssemblyBuilder::new("Attrs", Version::new(1, 0, 0, 0));
    let corlib = add_corlib(&mut builder);
    let holder = builder
        .add_type_def(TypeDefinition::new(
            "Attrs",
            "Holder",
            type_attributes::PUBLIC_CLASS,
            Some(corlib.object.into()),
        ))
        .unwrap();
    let ctor = builder.add_default_constructor(holder).unwrap();
    let field = builder
        .add_field(
            holder,
            Field::new("Value", field_attributes::PUBLIC, Type::I4),
        )
        .unwrap();
    builder
        .add_custom_attribute(
            HasCustomAttribute::Field(field),
            crate::handles::CustomAttributeType::MethodDef(ctor),
            crate::builder::attribute_blob_empty(),
        )
        .unwrap();
    let (_, parsed) = emit_and_verify(&builder);
    assert_eq!(parsed.counts.get(table::CUSTOM_ATTRIBUTE), 1);
}

/// Signature coverage for the element types the fixture does not reach.
#[test]
fn the_remaining_element_types_encode_and_survive() {
    let mut builder = AssemblyBuilder::new("Wide", Version::new(1, 0, 0, 0));
    let corlib = add_corlib(&mut builder);
    let holder = builder
        .add_type_def(
            TypeDefinition::new(
                "Wide",
                "Holder`1",
                type_attributes::PUBLIC_CLASS,
                Some(corlib.object.into()),
            )
            .with_generic_parameter(crate::model::GenericParameter::new("T")),
        )
        .unwrap();
    for (name, field_type) in exotic_field_types(holder) {
        builder
            .add_field(
                holder,
                Field::new(name, field_attributes::PUBLIC, field_type),
            )
            .unwrap();
    }
    builder
        .add_method(
            holder,
            Method::new(
                "Roundtrip",
                method_attributes::PUBLIC_STATIC,
                MethodSignature::static_method(Type::TypedByRef, vec![Type::Void.pointer()]),
            ),
        )
        .unwrap();
    let (_, parsed) = emit_and_verify(&builder);
    assert_eq!(parsed.counts.get(table::FIELD), 20);
}

fn exotic_field_types(holder: crate::handles::TypeDefId) -> Vec<(&'static str, Type)> {
    vec![
        ("Bool", Type::Boolean),
        ("Char", Type::Char),
        ("SByte", Type::I1),
        ("Byte", Type::U1),
        ("Short", Type::I2),
        ("UShort", Type::U2),
        ("Int", Type::I4),
        ("UInt", Type::U4),
        ("Long", Type::I8),
        ("ULong", Type::U8),
        ("Float", Type::R4),
        ("Double", Type::R8),
        ("Str", Type::String),
        ("Obj", Type::Object),
        ("NInt", Type::IntPtr),
        ("NUInt", Type::UIntPtr),
        ("Ptr", Type::I4.pointer()),
        ("Vector", Type::I4.array()),
        ("Var", Type::TypeParameter(0)),
        ("Nested", box_of(holder, Type::String).array()),
    ]
}

#[test]
fn multi_dimensional_arrays_encode_their_shape() {
    let mut blob = Vec::new();
    Type::Array {
        element: Box::new(Type::I4),
        rank: 2,
        sizes: vec![3, 4],
        lower_bounds: vec![-1, 0],
    }
    .encode(&mut blob)
    .unwrap();
    // ARRAY, I4, rank 2, 2 sizes {3,4}, 2 lower bounds {-1, 0}. The bounds are
    // compressed *signed*: -1 rotates its sign bit into bit 0, giving 0x7F.
    assert_eq!(
        blob,
        vec![0x14, 0x08, 0x02, 0x02, 0x03, 0x04, 0x02, 0x7F, 0x00]
    );
}

#[test]
fn custom_modifiers_wrap_the_type_they_modify() {
    let mut blob = Vec::new();
    Type::RequiredModifier(
        TypeDefOrRef::Definition(crate::handles::TypeDefId::MODULE),
        Box::new(Type::I4),
    )
    .encode(&mut blob)
    .unwrap();
    // CMOD_REQD, TypeDefOrRef(TypeDef row 1 -> 4), I4.
    assert_eq!(blob, vec![0x1F, 0x04, 0x08]);
}

#[test]
fn deeply_nested_types_are_rejected_rather_than_overflowing_the_stack() {
    let mut deep = Type::I4;
    for _ in 0..(crate::signature::MAX_TYPE_DEPTH.saturating_add(2)) {
        deep = deep.array();
    }
    let mut blob = Vec::new();
    assert!(matches!(
        deep.encode(&mut blob),
        Err(crate::Error::TypeTooDeep { .. })
    ));
}

#[test]
fn every_constant_kind_encodes() {
    let kinds = [
        Constant::Boolean(true),
        Constant::Char(0x0078),
        Constant::I1(-1),
        Constant::U1(2),
        Constant::I2(-3),
        Constant::U2(4),
        Constant::I4(-5),
        Constant::U4(6),
        Constant::I8(-7),
        Constant::U8(8),
        Constant::R4(1.5),
        Constant::R8(2.5),
        Constant::String("hi".to_owned()),
        Constant::NullReference,
    ];
    let mut builder = AssemblyBuilder::new("Consts", Version::new(1, 0, 0, 0));
    let corlib = add_corlib(&mut builder);
    let holder = builder
        .add_type_def(TypeDefinition::new(
            "Consts",
            "Holder",
            type_attributes::PUBLIC_CLASS,
            Some(corlib.object.into()),
        ))
        .unwrap();
    for (index, constant) in kinds.into_iter().enumerate() {
        builder
            .add_field(
                holder,
                Field::new(
                    format!("F{index}"),
                    field_attributes::PUBLIC | field_attributes::STATIC,
                    Type::I4,
                )
                .with_constant(constant),
            )
            .unwrap();
    }
    let (_, parsed) = emit_and_verify(&builder);
    assert_eq!(parsed.counts.get(table::CONSTANT), 14);
}
