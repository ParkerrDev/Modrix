// SPDX-License-Identifier: GPL-2.0-only
//! Fixed-layout metadata records: type, field, method and parameter definitions.
//!
//! IL2CPP stores members in flat, globally-shared tables. A type does not own
//! its fields; it owns a *run* - a start index and a count into the one big
//! field table, with every type's run laid out contiguously. Reading a type's
//! members therefore means slicing the shared table, not walking a list.
//!
//! The layouts below are not described anywhere in the file. They were derived
//! by checking which record size divides each section's byte length exactly,
//! then confirmed against invariants that cannot hold by coincidence: summing
//! every type's `field_count` reproduces the field table's row count exactly
//! (124,512 on the reference game), and likewise for methods (182,725) and
//! parameters (193,667). [`crate::Metadata::validate_member_runs`] re-checks
//! those sums on every file it opens, so a build whose layout differs is
//! rejected instead of silently misread.

/// Byte size of `Il2CppTypeDefinition` in metadata v29.
pub(crate) const TYPE_DEF: usize = 88;
/// Byte size of `Il2CppFieldDefinition`.
pub(crate) const FIELD: usize = 12;
/// Byte size of `Il2CppMethodDefinition`.
pub(crate) const METHOD: usize = 32;
/// Byte size of `Il2CppParameterDefinition`.
pub(crate) const PARAM: usize = 12;
/// Byte size of `Il2CppGenericContainer` and `Il2CppGenericParameter` alike.
pub(crate) const GENERIC: usize = 16;
/// Byte size of `Il2CppPropertyDefinition`.
pub(crate) const PROPERTY: usize = 20;
/// Byte size of `Il2CppImageDefinition`.
pub(crate) const IMAGE: usize = 40;
/// Byte size of `Il2CppAssemblyDefinition`.
pub(crate) const ASSEMBLY: usize = 64;

/// A run of member records owned by one type: `[start, start + count)` in the
/// corresponding shared table. A negative start means the type owns none.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Run {
    /// First index in the shared table, or `None` when the run is empty.
    pub start: Option<usize>,
    /// How many records the run covers.
    pub count: usize,
}

impl Run {
    /// Build a run from the raw start/count pair, normalising IL2CPP's
    /// "negative means none" convention.
    pub(crate) fn new(start: i32, count: u16) -> Self {
        let count = usize::from(count);
        Self {
            start: usize::try_from(start).ok().filter(|_| count > 0),
            count,
        }
    }

    /// The indices this run covers.
    pub fn indices(self) -> impl Iterator<Item = usize> {
        let start = self.start.unwrap_or(0);
        let count = if self.start.is_some() { self.count } else { 0 };
        (0..count).filter_map(move |offset| start.checked_add(offset))
    }

    /// One past the last index, for bounds checking.
    pub(crate) fn end(self) -> usize {
        self.start
            .map_or(0, |start| start.saturating_add(self.count))
    }
}

/// A type definition: a class, struct, interface, enum or delegate.
#[derive(Debug, Clone)]
pub struct TypeDef {
    /// The type's own name, as IL2CPP records it (``List`1``).
    pub name: String,
    /// Its namespace, possibly empty.
    pub namespace: String,
    /// Index into the *binary's* type table for this type's own `Il2CppType`.
    pub byval_type: i32,
    /// Index into the binary's type table for the base type, or negative for
    /// `System.Object` and interfaces.
    pub parent_type: i32,
    /// Index of the enclosing type definition, or negative when top-level.
    pub declaring_type: i32,
    /// Index into the generic-container table, or negative when non-generic.
    pub generic_container: i32,
    /// `TypeAttributes` - visibility, `sealed`, `abstract`, `interface`.
    pub flags: u32,
    /// This type's fields, as a run into the shared field table.
    pub fields: Run,
    /// This type's methods, as a run into the shared method table.
    pub methods: Run,
    /// This type's properties, as a run into the shared property table.
    pub properties: Run,
    /// This type's nested types, as a run into the shared nested-type table.
    pub nested_types: Run,
    /// The interfaces it implements, as a run into the shared interface table.
    pub interfaces: Run,
    /// Metadata token.
    pub token: u32,
}

impl TypeDef {
    /// The namespace-qualified name, as a mod author would write it.
    #[must_use]
    pub fn full_name(&self) -> String {
        if self.namespace.is_empty() {
            self.name.clone()
        } else {
            format!("{}.{}", self.namespace, self.name)
        }
    }

    /// Whether this type is nested inside another.
    #[must_use]
    pub const fn is_nested(&self) -> bool {
        self.declaring_type >= 0
    }

    /// Whether the type is visible outside its own assembly. Only public types
    /// belong in a reference assembly a mod compiles against.
    #[must_use]
    pub const fn is_public(&self) -> bool {
        // TypeAttributes.VisibilityMask = 0x07; Public = 1, NestedPublic = 2.
        matches!(self.flags & 0x07, 1 | 2)
    }
}

/// A field definition.
#[derive(Debug, Clone)]
pub struct FieldDef {
    /// The field name.
    pub name: String,
    /// Index into the binary's type table for the field's type.
    pub type_index: i32,
    /// Metadata token.
    pub token: u32,
}

/// A method definition. Bodies live in native code, so only the signature is
/// recoverable - which is exactly what a reference assembly needs.
#[derive(Debug, Clone)]
pub struct MethodDef {
    /// The method name (`.ctor` for constructors, `get_X` for property getters).
    pub name: String,
    /// Index of the declaring type definition.
    pub declaring_type: i32,
    /// Index into the binary's type table for the return type.
    pub return_type: i32,
    /// This method's parameters, as a run into the shared parameter table.
    pub parameters: Run,
    /// Index into the generic-container table, or negative when non-generic.
    pub generic_container: i32,
    /// `MethodAttributes` - visibility, `static`, `virtual`, `abstract`.
    pub flags: u16,
    /// `MethodImplAttributes`.
    pub impl_flags: u16,
    /// vtable slot.
    pub slot: u16,
    /// Metadata token.
    pub token: u32,
}

impl MethodDef {
    /// Whether the method is callable from outside its assembly.
    #[must_use]
    pub const fn is_public(&self) -> bool {
        // MethodAttributes.MemberAccessMask = 0x07; Public = 6.
        self.flags & 0x07 == 6
    }

    /// Whether the method is static.
    #[must_use]
    pub const fn is_static(&self) -> bool {
        // MethodAttributes.Static = 0x0010.
        self.flags & 0x0010 != 0
    }
}

/// A property definition.
///
/// IL2CPP stores a property as a name plus the *relative* indices of its
/// accessors within the declaring type's method run - not absolute method
/// indices - so resolving one requires the owning type's `methods.start`.
#[derive(Debug, Clone)]
pub struct PropertyDef {
    /// The property name (`IsInWorld`).
    pub name: String,
    /// Getter index relative to the declaring type's method run, or negative.
    pub get: i32,
    /// Setter index relative to the declaring type's method run, or negative.
    pub set: i32,
    /// `PropertyAttributes`.
    pub attrs: u32,
    /// Metadata token.
    pub token: u32,
}

/// A type's or method's generic parameter list.
///
/// IL2CPP records arity here rather than in the name (for methods) or only in
/// the name (for types), so this is the authoritative source. Inferring a
/// method's arity from the highest `!!N` its signature mentions is wrong for
/// `T AddComponent<T>()`, whose arity is 1 but which may mention no parameter
/// at all in an erased signature.
#[derive(Debug, Clone)]
pub struct GenericContainer {
    /// The declaring type or method index.
    pub owner: i32,
    /// How many generic parameters.
    pub arg_count: i32,
    /// True when the owner is a method rather than a type.
    pub is_method: bool,
    /// First index into the shared generic-parameter table.
    pub param_start: i32,
}

/// A parameter definition.
#[derive(Debug, Clone)]
pub struct ParamDef {
    /// The parameter name.
    pub name: String,
    /// Index into the binary's type table for the parameter type.
    pub type_index: i32,
    /// Metadata token.
    pub token: u32,
}

/// One managed assembly the game was built from.
#[derive(Debug, Clone)]
pub struct Assembly {
    /// The assembly name, without extension (`Assembly-CSharp`, `Sons`).
    pub name: String,
    /// Version, as `major.minor.build.revision`.
    pub version: String,
    /// The type definitions this assembly owns, as a run.
    pub types: Run,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_run_with_a_negative_start_is_empty() {
        let run = Run::new(-1, 0);
        assert_eq!(run.start, None);
        assert_eq!(run.indices().count(), 0);
        assert_eq!(run.end(), 0);
    }

    #[test]
    fn a_zero_count_run_is_empty_even_with_a_valid_start() {
        // IL2CPP writes a plausible start with a zero count for a type that
        // declares no members; treating that as a one-element run would read a
        // neighbouring type's first member.
        let run = Run::new(500, 0);
        assert_eq!(run.indices().count(), 0);
    }

    #[test]
    fn a_run_covers_exactly_its_span() {
        let run = Run::new(10, 3);
        assert_eq!(run.indices().collect::<Vec<_>>(), vec![10, 11, 12]);
        assert_eq!(run.end(), 13);
    }

    #[test]
    fn visibility_masks_match_the_ecma_encoding() {
        let public = TypeDef {
            name: "X".to_owned(),
            namespace: String::new(),
            byval_type: 0,
            parent_type: -1,
            declaring_type: -1,
            generic_container: -1,
            flags: 1,
            fields: Run::new(-1, 0),
            methods: Run::new(-1, 0),
            properties: Run::new(-1, 0),
            nested_types: Run::new(-1, 0),
            interfaces: Run::new(-1, 0),
            token: 0,
        };
        assert!(public.is_public());
        assert!(!public.is_nested());

        let internal = TypeDef {
            flags: 0,
            ..public.clone()
        };
        assert!(!internal.is_public(), "NotPublic must not be exported");

        let nested_public = TypeDef {
            flags: 2,
            declaring_type: 4,
            ..public
        };
        assert!(nested_public.is_public());
        assert!(nested_public.is_nested());
    }
}
