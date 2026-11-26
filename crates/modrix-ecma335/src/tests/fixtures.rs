// SPDX-License-Identifier: GPL-2.0-only
//! Shared fixtures: one assembly that exercises every feature the writer
//! supports, plus the L0 helper that every test runs on its output.

use crate::builder::AssemblyBuilder;
use crate::handles::{AssemblyRefId, ResolutionScope, TypeDefId, TypeDefOrRef, TypeRefId};
use crate::model::{
    AssemblyReference, Field, GenericParameter, Method, Parameter, Property, TypeDefinition,
    Version, field_attributes, method_attributes, type_attributes,
};
use crate::signature::{MethodSignature, Type};
use crate::verify::{Parsed, parse};

/// The core-library reference plus the base types every fixture imports.
pub(crate) struct Corlib {
    pub(crate) assembly: AssemblyRefId,
    pub(crate) object: TypeRefId,
    pub(crate) value_type: TypeRefId,
    pub(crate) enumeration: TypeRefId,
}

/// Adds the `System.Runtime` reference and the three base-type `TypeRef` rows,
/// targeting .NET 9.
pub(crate) fn add_corlib(builder: &mut AssemblyBuilder) -> Corlib {
    add_corlib_targeting(builder, 9)
}

/// The same, for a caller that must match an installed reference pack.
pub(crate) fn add_corlib_targeting(builder: &mut AssemblyBuilder, major: u16) -> Corlib {
    let corlib = builder.add_assembly_ref(AssemblyReference::core_library(major));
    let scope = ResolutionScope::AssemblyRef(corlib);
    Corlib {
        assembly: corlib,
        object: builder.add_type_ref(scope, "System", "Object"),
        value_type: builder.add_type_ref(scope, "System", "ValueType"),
        enumeration: builder.add_type_ref(scope, "System", "Enum"),
    }
}

/// The full-surface fixture: an interface, an open generic class, a class with
/// fields, a property, a virtual method and a generic method, a nested class, a
/// struct, and an enum with literal members.
pub(crate) fn full_surface() -> AssemblyBuilder {
    full_surface_targeting(9, false)
}

/// The same assembly, stamped with `ReferenceAssemblyAttribute` and
/// `TargetFrameworkAttribute` the way a real reference assembly is.
pub(crate) fn full_surface_marked() -> AssemblyBuilder {
    full_surface_targeting(9, true)
}

/// The fixture, bound to a specific .NET major version so it can be compiled
/// against that version's reference pack without a CS1701 identity warning.
pub(crate) fn full_surface_targeting(major: u16, mark: bool) -> AssemblyBuilder {
    let mut builder = AssemblyBuilder::new("MyLib", Version::new(1, 0, 0, 0));
    let corlib = add_corlib_targeting(&mut builder, major);
    let thing = add_interface(&mut builder);
    let boxed = add_box(&mut builder, corlib.object);
    let widget = add_widget(&mut builder, &corlib, thing, boxed);
    add_inner(&mut builder, corlib.object, widget);
    add_point(&mut builder, corlib.value_type);
    add_color(&mut builder, corlib.enumeration);
    if mark {
        builder
            .mark_reference_assembly(corlib.assembly, &format!(".NETCoreApp,Version=v{major}.0"))
            .unwrap();
    }
    builder
}

fn add_interface(builder: &mut AssemblyBuilder) -> TypeDefId {
    let thing = builder
        .add_type_def(TypeDefinition::new(
            "MyLib",
            "IThing",
            type_attributes::PUBLIC_INTERFACE,
            None,
        ))
        .unwrap();
    builder
        .add_method(
            thing,
            Method::new(
                "Go",
                method_attributes::INTERFACE_METHOD,
                MethodSignature::instance_method(Type::Void, Vec::new()),
            ),
        )
        .unwrap();
    thing
}

fn add_box(builder: &mut AssemblyBuilder, object: TypeRefId) -> TypeDefId {
    let boxed = builder
        .add_type_def(
            TypeDefinition::new(
                "MyLib",
                "Box`1",
                type_attributes::PUBLIC_CLASS,
                Some(object.into()),
            )
            .with_generic_parameter(GenericParameter::new("T")),
        )
        .unwrap();
    builder
        .add_field(
            boxed,
            Field::new("Item", field_attributes::PUBLIC, Type::TypeParameter(0)),
        )
        .unwrap();
    builder.add_default_constructor(boxed).unwrap();
    builder
        .add_method(
            boxed,
            Method::new(
                "Get",
                method_attributes::PUBLIC_INSTANCE,
                MethodSignature::instance_method(Type::TypeParameter(0), Vec::new()),
            ),
        )
        .unwrap();
    let open = box_of(boxed, Type::TypeParameter(0));
    builder
        .add_method(
            boxed,
            Method::new(
                "Make",
                method_attributes::PUBLIC_STATIC,
                MethodSignature::static_method(open, vec![Type::TypeParameter(0)]),
            )
            .with_parameter(Parameter::new("v")),
        )
        .unwrap();
    boxed
}

/// ``Box`1<argument>`` as it appears inside a signature blob - no `TypeSpec`
/// row is needed for a `GENERICINST` that is not referenced by a token.
pub(crate) fn box_of(definition: TypeDefId, argument: Type) -> Type {
    Type::GenericInstance {
        value_type: false,
        definition: TypeDefOrRef::Definition(definition),
        arguments: vec![argument],
    }
}

fn add_widget(
    builder: &mut AssemblyBuilder,
    corlib: &Corlib,
    thing: TypeDefId,
    boxed: TypeDefId,
) -> TypeDefId {
    let widget = builder
        .add_type_def(
            TypeDefinition::new(
                "MyLib",
                "Widget",
                type_attributes::PUBLIC_CLASS,
                Some(corlib.object.into()),
            )
            .with_interface(TypeDefOrRef::Definition(thing)),
        )
        .unwrap();
    add_widget_fields(builder, widget, boxed);
    add_widget_methods(builder, widget);
    widget
}

fn add_widget_fields(builder: &mut AssemblyBuilder, widget: TypeDefId, boxed: TypeDefId) {
    let public = field_attributes::PUBLIC;
    builder
        .add_field(widget, Field::new("Value", public, Type::I4))
        .unwrap();
    builder
        .add_field(widget, Field::new("Tags", public, Type::String.array()))
        .unwrap();
    builder
        .add_field(
            widget,
            Field::new("IntBox", public, box_of(boxed, Type::I4)),
        )
        .unwrap();
    builder
        .add_field(
            widget,
            Field::new("_count", field_attributes::PRIVATE, Type::I4),
        )
        .unwrap();
}

fn add_widget_methods(builder: &mut AssemblyBuilder, widget: TypeDefId) {
    builder.add_default_constructor(widget).unwrap();
    builder
        .add_method(
            widget,
            Method::new(
                "Go",
                method_attributes::PUBLIC_VIRTUAL | method_attributes::FINAL,
                MethodSignature::instance_method(Type::Void, Vec::new()),
            ),
        )
        .unwrap();
    let getter = builder
        .add_method(
            widget,
            Method::new(
                "get_Count",
                method_attributes::PROPERTY_ACCESSOR,
                MethodSignature::instance_method(Type::I4, Vec::new()),
            ),
        )
        .unwrap();
    let setter = builder
        .add_method(
            widget,
            Method::new(
                "set_Count",
                method_attributes::PROPERTY_ACCESSOR,
                MethodSignature::instance_method(Type::Void, vec![Type::I4]),
            )
            .with_parameter(Parameter::new("value")),
        )
        .unwrap();
    add_widget_statics(builder, widget);
    builder
        .add_property(
            widget,
            Property::new("Count", Type::I4)
                .with_getter(getter)
                .with_setter(setter),
        )
        .unwrap();
}

fn add_widget_statics(builder: &mut AssemblyBuilder, widget: TypeDefId) {
    builder
        .add_method(
            widget,
            Method::new(
                "DoThing",
                method_attributes::PUBLIC_INSTANCE,
                MethodSignature::instance_method(Type::Void, vec![Type::String]),
            )
            .with_parameter(Parameter::new("s")),
        )
        .unwrap();
    builder
        .add_method(
            widget,
            Method::new(
                "Add",
                method_attributes::PUBLIC_STATIC,
                MethodSignature::static_method(Type::I4, vec![Type::I4, Type::I4]),
            )
            .with_parameter(Parameter::new("a"))
            .with_parameter(Parameter::new("b")),
        )
        .unwrap();
    builder
        .add_method(
            widget,
            Method::new(
                "Swap",
                method_attributes::PUBLIC_STATIC,
                MethodSignature::static_method(
                    Type::Void,
                    vec![
                        Type::MethodParameter(0).by_ref(),
                        Type::MethodParameter(0).by_ref(),
                    ],
                ),
            )
            .with_generic_parameter(GenericParameter::new("T"))
            .with_parameter(Parameter::new("a"))
            .with_parameter(Parameter::new("b")),
        )
        .unwrap();
}

fn add_inner(builder: &mut AssemblyBuilder, object: TypeRefId, widget: TypeDefId) {
    let inner = builder
        .add_type_def(
            TypeDefinition::new(
                "",
                "Inner",
                type_attributes::NESTED_PUBLIC | type_attributes::BEFORE_FIELD_INIT,
                Some(object.into()),
            )
            .nested_in(widget),
        )
        .unwrap();
    builder
        .add_field(inner, Field::new("Z", field_attributes::PUBLIC, Type::I4))
        .unwrap();
    builder.add_default_constructor(inner).unwrap();
}

fn add_point(builder: &mut AssemblyBuilder, value_type: TypeRefId) {
    let point = builder
        .add_type_def(TypeDefinition::new(
            "MyLib",
            "Point",
            type_attributes::PUBLIC_VALUE_TYPE,
            Some(value_type.into()),
        ))
        .unwrap();
    for name in ["X", "Y"] {
        builder
            .add_field(point, Field::new(name, field_attributes::PUBLIC, Type::I4))
            .unwrap();
    }
}

fn add_color(builder: &mut AssemblyBuilder, enumeration: TypeRefId) {
    use crate::model::Constant;
    let color = builder
        .add_type_def(TypeDefinition::new(
            "MyLib",
            "Color",
            type_attributes::PUBLIC_VALUE_TYPE,
            Some(enumeration.into()),
        ))
        .unwrap();
    builder
        .add_field(
            color,
            Field::new("value__", field_attributes::ENUM_BACKING, Type::I4),
        )
        .unwrap();
    let literal = Type::ValueType(TypeDefOrRef::Definition(color));
    for (name, value) in [("Red", 0_i32), ("Blue", 1)] {
        builder
            .add_field(
                color,
                Field::new(name, field_attributes::ENUM_MEMBER, literal.clone())
                    .with_constant(Constant::I4(value)),
            )
            .unwrap();
    }
}

/// Emits, re-parses and fully validates an assembly. Every test funnels its
/// output through here, so L0 runs on all of them.
pub(crate) fn emit_and_verify(builder: &AssemblyBuilder) -> (Vec<u8>, Parsed) {
    let bytes = builder.build().expect("the fixture must build");
    let parsed = parse(&bytes).expect("emitted bytes must re-parse");
    parsed
        .check_references()
        .expect("every reference must resolve");
    (bytes, parsed)
}
