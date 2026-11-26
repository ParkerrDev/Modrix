// SPDX-License-Identifier: GPL-2.0-only
//! The type model and the signature-blob encoder (ECMA-335 II.23.2).
//!
//! A signature blob is a self-contained byte string: it never uses metadata
//! table indices, only *compressed* `TypeDefOrRef` coded indices. That is why
//! signatures can be encoded before column widths are known, and why a
//! `GENERICINST` inside a field or method signature needs no `TypeSpec` row -
//! a `TypeSpec` is only required where a generic instantiation must be named
//! by a metadata *token* (a base class, an interface, a `MemberRef` parent).

use crate::encode::{compress_signed, compress_unsigned};
use crate::error::{Error, Result};
use crate::handles::TypeDefOrRef;

/// ECMA-335 `ELEMENT_TYPE_*` constants (II.23.1.16).
pub mod element_type {
    /// Terminator.
    pub const END: u8 = 0x00;
    /// `void`.
    pub const VOID: u8 = 0x01;
    /// `System.Boolean`.
    pub const BOOLEAN: u8 = 0x02;
    /// `System.Char`.
    pub const CHAR: u8 = 0x03;
    /// `System.SByte`.
    pub const I1: u8 = 0x04;
    /// `System.Byte`.
    pub const U1: u8 = 0x05;
    /// `System.Int16`.
    pub const I2: u8 = 0x06;
    /// `System.UInt16`.
    pub const U2: u8 = 0x07;
    /// `System.Int32`.
    pub const I4: u8 = 0x08;
    /// `System.UInt32`.
    pub const U4: u8 = 0x09;
    /// `System.Int64`.
    pub const I8: u8 = 0x0A;
    /// `System.UInt64`.
    pub const U8: u8 = 0x0B;
    /// `System.Single`.
    pub const R4: u8 = 0x0C;
    /// `System.Double`.
    pub const R8: u8 = 0x0D;
    /// `System.String`.
    pub const STRING: u8 = 0x0E;
    /// Unmanaged pointer.
    pub const PTR: u8 = 0x0F;
    /// Managed reference (`ref`/`out`).
    pub const BYREF: u8 = 0x10;
    /// Value type named by a `TypeDefOrRef`.
    pub const VALUETYPE: u8 = 0x11;
    /// Reference type named by a `TypeDefOrRef`.
    pub const CLASS: u8 = 0x12;
    /// Generic parameter of the enclosing *type*.
    pub const VAR: u8 = 0x13;
    /// Multi-dimensional array.
    pub const ARRAY: u8 = 0x14;
    /// Generic instantiation.
    pub const GENERICINST: u8 = 0x15;
    /// `System.TypedReference`.
    pub const TYPEDBYREF: u8 = 0x16;
    /// `System.IntPtr`.
    pub const I: u8 = 0x18;
    /// `System.UIntPtr`.
    pub const U: u8 = 0x19;
    /// Function pointer.
    pub const FNPTR: u8 = 0x1B;
    /// `System.Object`.
    pub const OBJECT: u8 = 0x1C;
    /// Vector (single-dimension, zero-based) array.
    pub const SZARRAY: u8 = 0x1D;
    /// Generic parameter of the enclosing *method*.
    pub const MVAR: u8 = 0x1E;
    /// Required custom modifier.
    pub const CMOD_REQD: u8 = 0x1F;
    /// Optional custom modifier.
    pub const CMOD_OPT: u8 = 0x20;
}

/// Leading byte of a `Field` signature (II.23.2.4).
pub const FIELD_SIGNATURE: u8 = 0x06;
/// Leading byte of a `Property` signature (II.23.2.5), before `HASTHIS`.
pub const PROPERTY_SIGNATURE: u8 = 0x08;
/// `HASTHIS` bit of a calling convention byte.
pub const CALLCONV_HASTHIS: u8 = 0x20;
/// `EXPLICITTHIS` bit of a calling convention byte.
pub const CALLCONV_EXPLICITTHIS: u8 = 0x40;
/// `GENERIC` bit of a calling convention byte.
pub const CALLCONV_GENERIC: u8 = 0x10;
/// `VARARG` calling convention.
pub const CALLCONV_VARARG: u8 = 0x05;

/// How deep a type tree this encoder will walk. Signatures are caller-built,
/// so the depth is bounded rather than trusted (Power of Ten rule 1).
pub const MAX_TYPE_DEPTH: u32 = 64;

/// A type as it appears inside a signature blob.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub enum Type {
    /// `void`; only valid as a return type.
    Void,
    /// `bool`.
    Boolean,
    /// `char`.
    Char,
    /// `sbyte`.
    I1,
    /// `byte`.
    U1,
    /// `short`.
    I2,
    /// `ushort`.
    U2,
    /// `int`.
    I4,
    /// `uint`.
    U4,
    /// `long`.
    I8,
    /// `ulong`.
    U8,
    /// `float`.
    R4,
    /// `double`.
    R8,
    /// `string`.
    String,
    /// `object`.
    Object,
    /// `nint` / `System.IntPtr`.
    IntPtr,
    /// `nuint` / `System.UIntPtr`.
    UIntPtr,
    /// `System.TypedReference`.
    TypedByRef,
    /// A reference type named by a token.
    Class(TypeDefOrRef),
    /// A value type or enum named by a token.
    ValueType(TypeDefOrRef),
    /// A vector array (`T[]`).
    SzArray(Box<Type>),
    /// A multi-dimensional array.
    Array {
        /// Element type.
        element: Box<Type>,
        /// Number of dimensions.
        rank: u32,
        /// Leading dimension sizes; may be shorter than `rank`.
        sizes: Vec<u32>,
        /// Leading dimension lower bounds; may be shorter than `rank`.
        lower_bounds: Vec<i32>,
    },
    /// A managed reference (`ref T` / `out T`).
    ByRef(Box<Type>),
    /// An unmanaged pointer (`T*`).
    Ptr(Box<Type>),
    /// A generic instantiation such as `Box<int>`.
    GenericInstance {
        /// `true` for a generic value type, `false` for a generic class.
        value_type: bool,
        /// The *open* generic type definition.
        definition: TypeDefOrRef,
        /// The type arguments.
        arguments: Vec<Type>,
    },
    /// The zero-based generic parameter of the enclosing type (`!0`).
    TypeParameter(u32),
    /// The zero-based generic parameter of the enclosing method (`!!0`).
    MethodParameter(u32),
    /// A required custom modifier applied to another type.
    RequiredModifier(TypeDefOrRef, Box<Type>),
    /// An optional custom modifier applied to another type.
    OptionalModifier(TypeDefOrRef, Box<Type>),
}

impl Type {
    /// Wraps `self` in `T[]`.
    #[must_use]
    pub fn array(self) -> Self {
        Self::SzArray(Box::new(self))
    }

    /// Wraps `self` in `ref T`.
    #[must_use]
    pub fn by_ref(self) -> Self {
        Self::ByRef(Box::new(self))
    }

    /// Wraps `self` in `T*`.
    #[must_use]
    pub fn pointer(self) -> Self {
        Self::Ptr(Box::new(self))
    }

    /// Appends this type's signature encoding to `out`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::TypeTooDeep`] past [`MAX_TYPE_DEPTH`] nesting levels,
    /// [`Error::CompressedRange`] if a generic-parameter number or array shape
    /// is too large to compress, or [`Error::CodedIndexOverflow`] if a token
    /// cannot be packed.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        self.encode_at(out, 0)
    }

    fn encode_at(&self, out: &mut Vec<u8>, depth: u32) -> Result<()> {
        if depth >= MAX_TYPE_DEPTH {
            return Err(Error::TypeTooDeep {
                limit: MAX_TYPE_DEPTH,
            });
        }
        if let Some(primitive) = self.primitive_byte() {
            out.push(primitive);
            return Ok(());
        }
        let next = depth.saturating_add(1);
        match self {
            Self::Class(token) => Self::encode_token(out, element_type::CLASS, *token),
            Self::ValueType(token) => Self::encode_token(out, element_type::VALUETYPE, *token),
            Self::SzArray(element) => {
                Self::encode_wrapped(out, element_type::SZARRAY, element, next)
            }
            Self::ByRef(inner) => Self::encode_wrapped(out, element_type::BYREF, inner, next),
            Self::Ptr(inner) => Self::encode_wrapped(out, element_type::PTR, inner, next),
            Self::TypeParameter(index) => Self::encode_indexed(out, element_type::VAR, *index),
            Self::MethodParameter(index) => Self::encode_indexed(out, element_type::MVAR, *index),
            Self::RequiredModifier(token, inner) => {
                Self::encode_modifier(out, element_type::CMOD_REQD, *token, inner, next)
            }
            Self::OptionalModifier(token, inner) => {
                Self::encode_modifier(out, element_type::CMOD_OPT, *token, inner, next)
            }
            Self::GenericInstance {
                value_type,
                definition,
                arguments,
            } => Self::encode_generic(out, *value_type, *definition, arguments, next),
            Self::Array {
                element,
                rank,
                sizes,
                lower_bounds,
            } => {
                out.push(element_type::ARRAY);
                element.encode_at(out, next)?;
                encode_array_shape(out, *rank, sizes, lower_bounds)
            }
            _ => Ok(()),
        }
    }

    /// The single-byte encoding for the primitive cases, if this is one.
    fn primitive_byte(&self) -> Option<u8> {
        Some(match self {
            Self::Void => element_type::VOID,
            Self::Boolean => element_type::BOOLEAN,
            Self::Char => element_type::CHAR,
            Self::I1 => element_type::I1,
            Self::U1 => element_type::U1,
            Self::I2 => element_type::I2,
            Self::U2 => element_type::U2,
            Self::I4 => element_type::I4,
            Self::U4 => element_type::U4,
            Self::I8 => element_type::I8,
            Self::U8 => element_type::U8,
            Self::R4 => element_type::R4,
            Self::R8 => element_type::R8,
            Self::String => element_type::STRING,
            Self::Object => element_type::OBJECT,
            Self::IntPtr => element_type::I,
            Self::UIntPtr => element_type::U,
            Self::TypedByRef => element_type::TYPEDBYREF,
            _ => return None,
        })
    }

    fn encode_token(out: &mut Vec<u8>, tag: u8, token: TypeDefOrRef) -> Result<()> {
        out.push(tag);
        compress_unsigned(out, token.coded_value()?)
    }

    fn encode_wrapped(out: &mut Vec<u8>, tag: u8, inner: &Self, depth: u32) -> Result<()> {
        out.push(tag);
        inner.encode_at(out, depth)
    }

    fn encode_indexed(out: &mut Vec<u8>, tag: u8, index: u32) -> Result<()> {
        out.push(tag);
        compress_unsigned(out, index)
    }

    fn encode_modifier(
        out: &mut Vec<u8>,
        tag: u8,
        token: TypeDefOrRef,
        inner: &Self,
        depth: u32,
    ) -> Result<()> {
        out.push(tag);
        compress_unsigned(out, token.coded_value()?)?;
        inner.encode_at(out, depth)
    }

    fn encode_generic(
        out: &mut Vec<u8>,
        value_type: bool,
        definition: TypeDefOrRef,
        arguments: &[Self],
        depth: u32,
    ) -> Result<()> {
        out.push(element_type::GENERICINST);
        out.push(if value_type {
            element_type::VALUETYPE
        } else {
            element_type::CLASS
        });
        compress_unsigned(out, definition.coded_value()?)?;
        compress_unsigned(out, u32::try_from(arguments.len()).unwrap_or(u32::MAX))?;
        for argument in arguments {
            argument.encode_at(out, depth)?;
        }
        Ok(())
    }
}

/// Encodes an `ArrayShape` (II.23.2.13).
fn encode_array_shape(
    out: &mut Vec<u8>,
    rank: u32,
    sizes: &[u32],
    lower_bounds: &[i32],
) -> Result<()> {
    compress_unsigned(out, rank)?;
    compress_unsigned(out, u32::try_from(sizes.len()).unwrap_or(u32::MAX))?;
    for size in sizes {
        compress_unsigned(out, *size)?;
    }
    compress_unsigned(out, u32::try_from(lower_bounds.len()).unwrap_or(u32::MAX))?;
    for bound in lower_bounds {
        compress_signed(out, *bound)?;
    }
    Ok(())
}

/// A `MethodDefSig` / `MethodRefSig` (II.23.2.1, II.23.2.2).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MethodSignature {
    /// Instance methods set `HASTHIS`.
    pub has_this: bool,
    /// Set `EXPLICITTHIS` when the `this` pointer is an explicit parameter.
    pub explicit_this: bool,
    /// `true` for the `VARARG` convention.
    pub vararg: bool,
    /// Generic arity; non-zero sets the `GENERIC` bit.
    pub generic_parameter_count: u32,
    /// Return type.
    pub return_type: Type,
    /// Parameter types, excluding `this` and the return type.
    pub parameters: Vec<Type>,
}

impl MethodSignature {
    /// A `static` method signature.
    #[must_use]
    pub fn static_method(return_type: Type, parameters: Vec<Type>) -> Self {
        Self {
            has_this: false,
            explicit_this: false,
            vararg: false,
            generic_parameter_count: 0,
            return_type,
            parameters,
        }
    }

    /// An instance method signature (`HASTHIS`).
    #[must_use]
    pub fn instance_method(return_type: Type, parameters: Vec<Type>) -> Self {
        Self {
            has_this: true,
            ..Self::static_method(return_type, parameters)
        }
    }

    /// Sets the generic arity, which turns on the `GENERIC` calling-convention
    /// bit and emits the arity ahead of the parameter count.
    #[must_use]
    pub fn with_generic_parameters(mut self, count: u32) -> Self {
        self.generic_parameter_count = count;
        self
    }

    /// The calling-convention byte this signature starts with.
    #[must_use]
    pub fn calling_convention(&self) -> u8 {
        let mut byte = if self.vararg { CALLCONV_VARARG } else { 0x00 };
        if self.generic_parameter_count > 0 {
            byte |= CALLCONV_GENERIC;
        }
        if self.has_this {
            byte |= CALLCONV_HASTHIS;
        }
        if self.explicit_this {
            byte |= CALLCONV_EXPLICITTHIS;
        }
        byte
    }

    /// Encodes the complete signature blob.
    ///
    /// # Errors
    ///
    /// Propagates any [`Type::encode`] failure.
    pub fn to_blob(&self) -> Result<Vec<u8>> {
        let mut out = vec![self.calling_convention()];
        if self.generic_parameter_count > 0 {
            compress_unsigned(&mut out, self.generic_parameter_count)?;
        }
        compress_unsigned(
            &mut out,
            u32::try_from(self.parameters.len()).unwrap_or(u32::MAX),
        )?;
        self.return_type.encode(&mut out)?;
        for parameter in &self.parameters {
            parameter.encode(&mut out)?;
        }
        Ok(out)
    }
}

/// Encodes a `FieldSig`: the `0x06` marker followed by the field's type.
///
/// # Errors
///
/// Propagates any [`Type::encode`] failure.
pub fn field_signature(field_type: &Type) -> Result<Vec<u8>> {
    let mut out = vec![FIELD_SIGNATURE];
    field_type.encode(&mut out)?;
    Ok(out)
}

/// Encodes a `PropertySig`.
///
/// # Errors
///
/// Propagates any [`Type::encode`] failure.
pub fn property_signature(
    has_this: bool,
    property_type: &Type,
    index_parameters: &[Type],
) -> Result<Vec<u8>> {
    let marker = if has_this {
        PROPERTY_SIGNATURE | CALLCONV_HASTHIS
    } else {
        PROPERTY_SIGNATURE
    };
    let mut out = vec![marker];
    compress_unsigned(
        &mut out,
        u32::try_from(index_parameters.len()).unwrap_or(u32::MAX),
    )?;
    property_type.encode(&mut out)?;
    for parameter in index_parameters {
        parameter.encode(&mut out)?;
    }
    Ok(out)
}
