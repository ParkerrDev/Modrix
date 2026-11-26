// SPDX-License-Identifier: GPL-2.0-only
//! The declarative shapes callers hand to [`AssemblyBuilder`].
//!
//! [`AssemblyBuilder`]: crate::AssemblyBuilder

use crate::handles::{MethodId, TypeDefId, TypeDefOrRef};
use crate::signature::{MethodSignature, Type};

/// `TypeAttributes` bits (ECMA-335 II.23.1.15).
pub mod type_attributes {
    /// Visible outside the assembly.
    pub const PUBLIC: u32 = 0x0000_0001;
    /// A public type nested in another type.
    pub const NESTED_PUBLIC: u32 = 0x0000_0002;
    /// A private type nested in another type.
    pub const NESTED_PRIVATE: u32 = 0x0000_0003;
    /// Sequential field layout.
    pub const SEQUENTIAL_LAYOUT: u32 = 0x0000_0008;
    /// Explicit field layout.
    pub const EXPLICIT_LAYOUT: u32 = 0x0000_0010;
    /// The type is an interface.
    pub const INTERFACE: u32 = 0x0000_0020;
    /// The type cannot be instantiated directly.
    pub const ABSTRACT: u32 = 0x0000_0080;
    /// The type cannot be derived from.
    pub const SEALED: u32 = 0x0000_0100;
    /// The name carries meaning to tools.
    pub const SPECIAL_NAME: u32 = 0x0000_0400;
    /// The name carries meaning to the runtime.
    pub const RT_SPECIAL_NAME: u32 = 0x0000_0800;
    /// Static initialisers may run lazily.
    pub const BEFORE_FIELD_INIT: u32 = 0x0010_0000;

    /// A plain `public class`.
    pub const PUBLIC_CLASS: u32 = PUBLIC | BEFORE_FIELD_INIT;
    /// A `public interface`.
    pub const PUBLIC_INTERFACE: u32 = PUBLIC | INTERFACE | ABSTRACT;
    /// A `public struct` or `public enum`.
    pub const PUBLIC_VALUE_TYPE: u32 = PUBLIC | SEALED;
}

/// `FieldAttributes` bits (ECMA-335 II.23.1.5).
pub mod field_attributes {
    /// Private field.
    pub const PRIVATE: u16 = 0x0001;
    /// Assembly-visible field.
    pub const ASSEMBLY: u16 = 0x0003;
    /// Protected field.
    pub const FAMILY: u16 = 0x0004;
    /// Public field.
    pub const PUBLIC: u16 = 0x0006;
    /// `static`.
    pub const STATIC: u16 = 0x0010;
    /// `readonly`.
    pub const INIT_ONLY: u16 = 0x0020;
    /// `const`; requires a `Constant` row.
    pub const LITERAL: u16 = 0x0040;
    /// The name carries meaning to tools.
    pub const SPECIAL_NAME: u16 = 0x0200;
    /// The name carries meaning to the runtime.
    pub const RT_SPECIAL_NAME: u16 = 0x0400;
    /// A `Constant` row supplies the value.
    pub const HAS_DEFAULT: u16 = 0x8000;

    /// The `value__` field every enum must declare first.
    pub const ENUM_BACKING: u16 = PUBLIC | SPECIAL_NAME | RT_SPECIAL_NAME;
    /// An enum member.
    pub const ENUM_MEMBER: u16 = PUBLIC | STATIC | LITERAL;
}

/// `MethodImplAttributes` bits (II.23.1.11).
pub mod method_impl {
    /// The body is IL. The default.
    pub const IL: u16 = 0x0000;
    /// The body lives in the runtime rather than in this assembly.
    ///
    /// Legal with no RVA, which is what lets a body-less reference assembly
    /// still *load*: the CLR rejects an ordinary method with no RVA at type
    /// load time, taking the whole assembly with it.
    pub const INTERNAL_CALL: u16 = 0x1000;
}

/// `MethodAttributes` and `MethodImplAttributes` bits (II.23.1.10, II.23.1.11).
pub mod method_attributes {
    /// Private method.
    pub const PRIVATE: u16 = 0x0001;
    /// Protected method.
    pub const FAMILY: u16 = 0x0004;
    /// Public method.
    pub const PUBLIC: u16 = 0x0006;
    /// `static`.
    pub const STATIC: u16 = 0x0010;
    /// `sealed` override.
    pub const FINAL: u16 = 0x0020;
    /// `virtual`.
    pub const VIRTUAL: u16 = 0x0040;
    /// Hide by name *and* signature, which is what C# emits.
    pub const HIDE_BY_SIG: u16 = 0x0080;
    /// Occupies a new vtable slot.
    pub const NEW_SLOT: u16 = 0x0100;
    /// No body.
    pub const ABSTRACT: u16 = 0x0400;
    /// The name carries meaning to tools.
    pub const SPECIAL_NAME: u16 = 0x0800;
    /// The name carries meaning to the runtime.
    pub const RT_SPECIAL_NAME: u16 = 0x1000;

    /// A public instance method.
    pub const PUBLIC_INSTANCE: u16 = PUBLIC | HIDE_BY_SIG;
    /// A public static method.
    pub const PUBLIC_STATIC: u16 = PUBLIC | STATIC | HIDE_BY_SIG;
    /// A public constructor.
    pub const PUBLIC_CONSTRUCTOR: u16 = PUBLIC | HIDE_BY_SIG | SPECIAL_NAME | RT_SPECIAL_NAME;
    /// A new public virtual method.
    pub const PUBLIC_VIRTUAL: u16 = PUBLIC | HIDE_BY_SIG | NEW_SLOT | VIRTUAL;
    /// An interface method.
    pub const INTERFACE_METHOD: u16 = PUBLIC | HIDE_BY_SIG | NEW_SLOT | VIRTUAL | ABSTRACT;
    /// A property accessor.
    pub const PROPERTY_ACCESSOR: u16 = PUBLIC | HIDE_BY_SIG | SPECIAL_NAME;
}

/// A four-part assembly version.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Version {
    /// Major component.
    pub major: u16,
    /// Minor component.
    pub minor: u16,
    /// Build component.
    pub build: u16,
    /// Revision component.
    pub revision: u16,
}

impl Version {
    /// Builds a version from its four components.
    #[must_use]
    pub const fn new(major: u16, minor: u16, build: u16, revision: u16) -> Self {
        Self {
            major,
            minor,
            build,
            revision,
        }
    }
}

/// The name of the core library every net6.0-or-later reference must import.
pub const CORE_LIBRARY_NAME: &str = "System.Runtime";

/// The ECMA public key token `System.Runtime` is signed with. Roslyn matches
/// on name plus token; an empty or wrong token yields CS0012 for every type.
pub const CORE_LIBRARY_PUBLIC_KEY_TOKEN: [u8; 8] = [0xB0, 0x3F, 0x5F, 0x7F, 0x11, 0xD5, 0x0A, 0x3A];

/// An `AssemblyRef` row to emit.
#[derive(Clone, Debug)]
pub struct AssemblyReference {
    /// Simple name, without extension.
    pub name: String,
    /// Referenced version. A mismatch against the real assembly is only a
    /// CS1701 warning, so this is the soft part of the identity.
    pub version: Version,
    /// The 8-byte public key *token*, or the full key when
    /// `AssemblyFlags.PublicKey` is set.
    pub public_key_or_token: Vec<u8>,
    /// `AssemblyFlags`; 0 means the blob above holds a token.
    pub flags: u32,
    /// Culture name; empty means neutral.
    pub culture: String,
}

impl AssemblyReference {
    /// The corlib reference a net6.0-or-later assembly must carry.
    ///
    /// `framework_major` is the .NET major version: 9 produces
    /// `System.Runtime, Version=9.0.0.0`.
    #[must_use]
    pub fn core_library(framework_major: u16) -> Self {
        Self {
            name: CORE_LIBRARY_NAME.to_owned(),
            version: Version::new(framework_major, 0, 0, 0),
            public_key_or_token: CORE_LIBRARY_PUBLIC_KEY_TOKEN.to_vec(),
            flags: 0,
            culture: String::new(),
        }
    }
}

/// A generic parameter on a type or a method.
#[derive(Clone, Debug)]
pub struct GenericParameter {
    /// Parameter name, for example `T`.
    pub name: String,
    /// `GenericParamAttributes`; 0 for an unconstrained parameter.
    pub flags: u16,
}

impl GenericParameter {
    /// An unconstrained parameter.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            flags: 0,
        }
    }
}

/// A compile-time constant, emitted as a `Constant` row.
///
/// Enum literal fields **must** have one. Without it the reference still loads
/// and the member still resolves, but the consuming compiler fails to emit
/// with a bare `CS7038`, which points nowhere.
#[derive(Clone, Debug, PartialEq)]
#[non_exhaustive]
pub enum Constant {
    /// `bool`.
    Boolean(bool),
    /// `char`.
    Char(u16),
    /// `sbyte`.
    I1(i8),
    /// `byte`.
    U1(u8),
    /// `short`.
    I2(i16),
    /// `ushort`.
    U2(u16),
    /// `int`.
    I4(i32),
    /// `uint`.
    U4(u32),
    /// `long`.
    I8(i64),
    /// `ulong`.
    U8(u64),
    /// `float`.
    R4(f32),
    /// `double`.
    R8(f64),
    /// A UTF-16 string literal.
    String(String),
    /// A null reference.
    NullReference,
}

/// A type to define.
#[derive(Clone, Debug)]
pub struct TypeDefinition {
    /// Namespace; **must** be empty for a nested type, whose namespace comes
    /// from its enclosing type instead.
    pub namespace: String,
    /// Type name; generic types use the arity suffix, for example ``Box`1``.
    pub name: String,
    /// `TypeAttributes`.
    pub flags: u32,
    /// Base type; `None` for interfaces and for `<Module>`.
    pub extends: Option<TypeDefOrRef>,
    /// Interfaces this type implements.
    pub interfaces: Vec<TypeDefOrRef>,
    /// Generic parameters, in declaration order.
    pub generic_parameters: Vec<GenericParameter>,
    /// Enclosing type for a nested type, which produces a `NestedClass` row.
    pub enclosing: Option<TypeDefId>,
}

impl TypeDefinition {
    /// A top-level type with no interfaces and no generic parameters.
    #[must_use]
    pub fn new(
        namespace: impl Into<String>,
        name: impl Into<String>,
        flags: u32,
        extends: Option<TypeDefOrRef>,
    ) -> Self {
        Self {
            namespace: namespace.into(),
            name: name.into(),
            flags,
            extends,
            interfaces: Vec::new(),
            generic_parameters: Vec::new(),
            enclosing: None,
        }
    }

    /// Nests this type inside `enclosing`.
    ///
    /// The namespace of a nested type must stay empty - it is inherited from
    /// the enclosing type - so this clears it.
    #[must_use]
    pub fn nested_in(mut self, enclosing: TypeDefId) -> Self {
        self.enclosing = Some(enclosing);
        self.namespace = String::new();
        self
    }

    /// Adds an implemented interface.
    #[must_use]
    pub fn with_interface(mut self, interface: TypeDefOrRef) -> Self {
        self.interfaces.push(interface);
        self
    }

    /// Adds a generic parameter.
    #[must_use]
    pub fn with_generic_parameter(mut self, parameter: GenericParameter) -> Self {
        self.generic_parameters.push(parameter);
        self
    }
}

/// A field to define.
#[derive(Clone, Debug)]
pub struct Field {
    /// Field name.
    pub name: String,
    /// `FieldAttributes`.
    pub flags: u16,
    /// Field type.
    pub field_type: Type,
    /// Compile-time value; required for `LITERAL` fields.
    pub constant: Option<Constant>,
}

impl Field {
    /// A field with no constant.
    #[must_use]
    pub fn new(name: impl Into<String>, flags: u16, field_type: Type) -> Self {
        Self {
            name: name.into(),
            flags,
            field_type,
            constant: None,
        }
    }

    /// Attaches a compile-time value and sets `HAS_DEFAULT`.
    #[must_use]
    pub fn with_constant(mut self, value: Constant) -> Self {
        self.constant = Some(value);
        self
    }
}

/// A named method parameter. Emitting these is optional - Roslyn synthesises
/// names when the `Param` table is absent - but without them callers lose
/// named arguments and `IntelliSense`.
#[derive(Clone, Debug)]
pub struct Parameter {
    /// Parameter name.
    pub name: String,
    /// `ParamAttributes`.
    pub flags: u16,
}

impl Parameter {
    /// A plain by-value parameter.
    #[must_use]
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            flags: 0,
        }
    }
}

/// A method to define. A reference assembly carries no bodies, so every
/// `MethodDef.RVA` is written as 0.
#[derive(Clone, Debug)]
pub struct Method {
    /// Method name; constructors are `.ctor` / `.cctor`.
    pub name: String,
    /// `MethodAttributes`.
    pub flags: u16,
    /// `MethodImplAttributes`; 0 means IL and managed.
    pub impl_flags: u16,
    /// The method's signature.
    pub signature: MethodSignature,
    /// Parameter names, in sequence order starting at 1.
    pub parameters: Vec<Parameter>,
    /// Generic parameters, in declaration order.
    pub generic_parameters: Vec<GenericParameter>,
}

impl Method {
    /// A method with no named parameters and no generic parameters.
    #[must_use]
    pub fn new(name: impl Into<String>, flags: u16, signature: MethodSignature) -> Self {
        Self {
            name: name.into(),
            flags,
            impl_flags: 0,
            signature,
            parameters: Vec::new(),
            generic_parameters: Vec::new(),
        }
    }

    /// Adds a named parameter; sequence numbers follow the insertion order.
    #[must_use]
    pub fn with_parameter(mut self, parameter: Parameter) -> Self {
        self.parameters.push(parameter);
        self
    }

    /// Adds a generic parameter and raises the signature's arity to match.
    #[must_use]
    pub fn with_generic_parameter(mut self, parameter: GenericParameter) -> Self {
        self.generic_parameters.push(parameter);
        self.signature.generic_parameter_count =
            u32::try_from(self.generic_parameters.len()).unwrap_or(u32::MAX);
        self
    }
}

/// A property to define, together with its accessors.
#[derive(Clone, Debug)]
pub struct Property {
    /// Property name.
    pub name: String,
    /// `PropertyAttributes`; 0 for an ordinary property.
    pub flags: u16,
    /// Property type.
    pub property_type: Type,
    /// `true` for an instance property.
    pub has_this: bool,
    /// Indexer parameter types.
    pub index_parameters: Vec<Type>,
    /// The `get` accessor, if any.
    pub getter: Option<MethodId>,
    /// The `set` accessor, if any.
    pub setter: Option<MethodId>,
}

impl Property {
    /// An instance property with no indexer parameters.
    #[must_use]
    pub fn new(name: impl Into<String>, property_type: Type) -> Self {
        Self {
            name: name.into(),
            flags: 0,
            property_type,
            has_this: true,
            index_parameters: Vec::new(),
            getter: None,
            setter: None,
        }
    }

    /// Attaches the `get` accessor.
    #[must_use]
    pub fn with_getter(mut self, getter: MethodId) -> Self {
        self.getter = Some(getter);
        self
    }

    /// Attaches the `set` accessor.
    #[must_use]
    pub fn with_setter(mut self, setter: MethodId) -> Self {
        self.setter = Some(setter);
        self
    }
}
