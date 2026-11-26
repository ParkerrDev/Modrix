// SPDX-License-Identifier: GPL-2.0-only
//! Encoding one assembly's model as a .NET reference assembly.
//!
//! Two ordering constraints drive the shape of this module, and both come from
//! ECMA-335 rather than from taste:
//!
//! **A base type must already exist when its subclass is declared.** The
//! builder takes `extends` at construction, so types cannot be added in
//! metadata order - a class whose base sits later in the same assembly would
//! have nothing to point at. Inheritance cannot cycle, so a dependency-ordered
//! pass always terminates; [`Layout::order`] does that, and also respects the
//! stricter rule that a nested type's enclosing type must come first.
//!
//! **Signatures may reference anything.** They are encoded in a second pass,
//! once every local type has an id, so a field can mention a type declared
//! later in the file.
//!
//! Generic arity is taken from the *name*. IL2CPP keeps the CLI convention of
//! encoding it as a `` `N `` suffix (``List`1``), and Roslyn cross-checks the
//! suffix against the declared `GenericParam` rows - so the suffix is the
//! authoritative source and the rows are generated to match it. Method arity
//! has no such marker, so it is inferred from the highest method-parameter
//! reference the signature actually makes.

use std::collections::HashMap;

use modrix_ecma335::model::{field_attributes, method_attributes, method_impl};
use modrix_ecma335::{
    AssemblyBuilder, AssemblyReference, Field, GenericParameter, Method, MethodSignature,
    ResolutionScope, Type, TypeDefId, TypeDefOrRef, TypeDefinition, TypeRefId, Version,
};
use modrix_il2cpp::{Metadata, ResolvedType, TypeKind};

use crate::error::{Error, Result};
use crate::model::{AssemblyModel, GameModel, TypeModel};

/// The framework major version the emitted references target. Mods build as
/// `net6.0`, which pins the `System.Runtime` reference to 6.0.0.0.
const FRAMEWORK_MAJOR: u16 = 6;

/// Guard on the dependency-ordering loop.
const MAX_ORDER_ROUNDS: usize = 64;

/// Per-assembly emission state.
struct Emitter<'a> {
    builder: AssemblyBuilder,
    model: &'a GameModel,
    metadata: &'a Metadata,
    /// Which assembly is being emitted, so foreign types can be told apart.
    own: usize,
    corlib: modrix_ecma335::AssemblyRefId,
    /// Assembly index -> its `AssemblyRef` row.
    refs: HashMap<usize, modrix_ecma335::AssemblyRefId>,
    /// Metadata type index -> local `TypeDef` row.
    local: HashMap<usize, TypeDefId>,
    /// Metadata type index -> interned `TypeRef` row.
    foreign: HashMap<usize, TypeRefId>,
}

/// Emit every assembly of a game as a reference assembly.
///
/// Returns `(assembly name, bytes)` pairs. An assembly with no public types is
/// skipped: an empty reference is useless and only slows a build down.
///
/// # Errors
///
/// Propagates read failures and any encoding error from `modrix-ecma335`.
pub fn emit_all(model: &GameModel, metadata: &Metadata) -> Result<Vec<(String, Vec<u8>)>> {
    let mut out = Vec::with_capacity(model.assemblies.len());
    for (index, assembly) in model.assemblies.iter().enumerate() {
        // The game's embedded corlib is never emitted; its types redirect onto
        // the real System.Runtime instead. See the `corlib` module.
        if assembly.types.is_empty() || crate::corlib::is_corlib_assembly(&assembly.name) {
            continue;
        }
        out.push((
            assembly.name.clone(),
            emit_assembly(model, metadata, index)?,
        ));
    }
    Ok(out)
}

/// Emit a single assembly by index.
///
/// # Errors
///
/// As [`emit_all`], plus [`Error::NoSuchAssembly`] for an out-of-range index.
pub fn emit_assembly(model: &GameModel, metadata: &Metadata, index: usize) -> Result<Vec<u8>> {
    let assembly = model
        .assemblies
        .get(index)
        .ok_or(Error::NoSuchAssembly { index })?;
    let mut emitter = Emitter::new(model, metadata, index, assembly);
    emitter.declare_types(assembly)?;
    emitter.encode_members(assembly)?;
    Ok(emitter.builder.build()?)
}

impl<'a> Emitter<'a> {
    fn new(
        model: &'a GameModel,
        metadata: &'a Metadata,
        own: usize,
        assembly: &AssemblyModel,
    ) -> Self {
        let version = parse_version(&assembly.version);
        let mut builder = AssemblyBuilder::new(&assembly.name, version);
        let corlib = builder.add_assembly_ref(AssemblyReference::core_library(FRAMEWORK_MAJOR));
        Self {
            builder,
            model,
            metadata,
            own,
            corlib,
            refs: HashMap::new(),
            local: HashMap::new(),
            foreign: HashMap::new(),
        }
    }

    /// Declare every type, in an order that satisfies base and nesting first.
    fn declare_types(&mut self, assembly: &AssemblyModel) -> Result<()> {
        for position in Layout::order(assembly, self.own, self.model) {
            let Some(model) = assembly.types.get(position) else {
                continue;
            };
            if let Some(id) = self.declare_one(model)? {
                self.local.insert(model.type_def, id);
            }
        }
        Ok(())
    }

    /// Declare one type, resolving its base, interfaces and generic arity.
    ///
    /// Returns `None` when the type cannot be declared correctly and must be
    /// dropped instead - see the nesting check below.
    fn declare_one(&mut self, model: &TypeModel) -> Result<Option<TypeDefId>> {
        let extends = match &model.base {
            Some(base) => self.token_of(base)?,
            None => None,
        };
        let mut definition = TypeDefinition::new(
            model.namespace.clone(),
            model.name.clone(),
            crate::model::publicize_type(model.flags),
            extends,
        );
        if let Some(outer) = model
            .declaring_type
            .and_then(|d| self.local.get(&d).copied())
        {
            definition = definition.nested_in(outer);
        }
        for interface in self.interfaces_of(model)? {
            definition = definition.with_interface(interface);
        }
        // Arity comes from the metadata's generic container, which carries the
        // real declared names. Roslyn cross-checks the count against the name's
        // `N suffix, so a mismatch here is a hard error - but the container and
        // the suffix always agree, both being written by the same compiler.
        for name in &model.generic_params {
            definition = definition.with_generic_parameter(GenericParameter::new(name.clone()));
        }
        Ok(Some(self.builder.add_type_def(definition)?))
    }

    /// The interface tokens a type implements.
    fn interfaces_of(&mut self, model: &TypeModel) -> Result<Vec<TypeDefOrRef>> {
        let def = self.metadata.type_def(model.type_def)?;
        let total = self.metadata.interface_count()?;
        let mut out = Vec::with_capacity(def.interfaces.count);
        for slot in def.interfaces.indices() {
            if slot >= total {
                continue;
            }
            let raw = self.metadata.interface_type(slot)?;
            let Ok(type_slot) = usize::try_from(raw) else {
                continue;
            };
            // An interface reached through a constructed generic type would
            // need a TypeSpec token; skip those rather than emit a malformed
            // InterfaceImpl row. Mods bind against methods, not the
            // interface relationship, so this is a tolerable v1 gap.
            if let Some(token) = self.token_of_index(type_slot)? {
                out.push(token);
            }
        }
        Ok(out)
    }

    /// Encode every type's fields and methods.
    fn encode_members(&mut self, assembly: &AssemblyModel) -> Result<()> {
        for model in &assembly.types {
            let Some(owner) = self.local.get(&model.type_def).copied() else {
                continue;
            };
            self.encode_fields(owner, model)?;
            let method_ids = self.encode_methods(owner, model)?;
            self.encode_properties(owner, model, &method_ids)?;
        }
        Ok(())
    }

    fn encode_fields(&mut self, owner: TypeDefId, model: &TypeModel) -> Result<()> {
        for field in &model.fields {
            let Some(ty) = self.convert(&field.ty)? else {
                continue;
            };
            let mut flags = u16::try_from(field.flags).unwrap_or(field_attributes::PUBLIC);
            let mut row = Field::new(field.name.clone(), flags, ty.clone());
            match field.constant {
                Some(value) => row = row.with_constant(constant_for(&ty, value)),
                // A literal with no recoverable constant cannot be emitted:
                // ECMA-335 requires the pair. Demote it to a plain static
                // field so the name still exists rather than dropping it.
                None if flags & field_attributes::LITERAL != 0 => {
                    flags &= !field_attributes::LITERAL;
                    flags |= field_attributes::STATIC;
                    row = Field::new(field.name.clone(), flags, ty);
                }
                None => {}
            }
            self.builder.add_field(owner, row)?;
        }
        Ok(())
    }

    /// Encode a type's methods, returning the emitted id for each model
    /// position so properties can link their accessors afterwards.
    fn encode_methods(
        &mut self,
        owner: TypeDefId,
        model: &TypeModel,
    ) -> Result<HashMap<usize, modrix_ecma335::MethodId>> {
        let mut ids = HashMap::new();
        // `InternalCall` is how a body-less method stays loadable, but the CLR
        // rejects it on a generic method or on any method of a generic class:
        // "Generic method or method in generic class is internal call, PInvoke,
        // or is defined in a COM Import class." Those would need a real IL body,
        // which a reference assembly does not carry, so they are omitted -
        // the type still loads, and only these members are absent.
        let owner_is_generic = !model.generic_params.is_empty();
        for (position, method) in model.methods.iter().enumerate() {
            if owner_is_generic || !method.generic_params.is_empty() {
                continue;
            }
            // A signature we cannot express is skipped whole: emitting a
            // method with a wrong signature is far worse than omitting it,
            // because the compiler would bind to it and then fail at a
            // confusing place.
            let Some(returns) = self.convert(&method.returns)? else {
                continue;
            };
            let mut parameters = Vec::with_capacity(method.parameters.len());
            let mut expressible = true;
            for param in &method.parameters {
                if let Some(ty) = self.convert(&param.ty)? {
                    parameters.push(ty);
                } else {
                    expressible = false;
                    break;
                }
            }
            if !expressible {
                continue;
            }
            let id = self.add_method(owner, method, returns, parameters)?;
            ids.insert(position, id);
        }
        Ok(ids)
    }

    /// Encode a type's properties, linking each to its emitted accessors.
    ///
    /// A property whose accessors were dropped (an inexpressible signature) is
    /// skipped: a `Property` row pointing at nothing is worse than no row.
    fn encode_properties(
        &mut self,
        owner: TypeDefId,
        model: &TypeModel,
        ids: &HashMap<usize, modrix_ecma335::MethodId>,
    ) -> Result<()> {
        for property in &model.properties {
            let Some(ty) = self.convert(&property.ty)? else {
                continue;
            };
            let getter = property.getter.and_then(|index| ids.get(&index).copied());
            let setter = property.setter.and_then(|index| ids.get(&index).copied());
            if getter.is_none() && setter.is_none() {
                continue;
            }
            let mut row = modrix_ecma335::Property::new(property.name.clone(), ty);
            if let Some(id) = getter {
                row = row.with_getter(id);
            }
            if let Some(id) = setter {
                row = row.with_setter(id);
            }
            self.builder.add_property(owner, row)?;
        }
        Ok(())
    }

    /// Add one method row, inferring its generic arity from the signature.
    fn add_method(
        &mut self,
        owner: TypeDefId,
        method: &crate::model::MethodModel,
        returns: Type,
        parameters: Vec<Type>,
    ) -> Result<modrix_ecma335::MethodId> {
        // The generic container is authoritative. Inferring arity from the
        // highest `!!N` a signature mentions silently gets `T AddComponent<T>()`
        // wrong, emitting it as non-generic - which is why every
        // `AddComponent<Foo>()` call in a real mod failed to bind.
        let arity = u32::try_from(method.generic_params.len()).unwrap_or(0);
        let mut signature = if method.is_static {
            MethodSignature::static_method(returns, parameters)
        } else {
            MethodSignature::instance_method(returns, parameters)
        };
        if arity > 0 {
            signature = signature.with_generic_parameters(arity);
        }
        let flags = method.flags | method_attributes::HIDE_BY_SIG;
        let mut row = Method::new(method.name.clone(), flags, signature);
        // Mark every method as an internal call.
        //
        // These assemblies carry no IL - a reference assembly's methods all
        // have RVA 0 - and the CLR refuses to *load* a type whose ordinary
        // method has no body: "Could not load type 'UnityEngine.Object'
        // because the method 'GetInstanceID' has no implementation (no RVA)".
        // That takes down the whole assembly, so a mod cannot even name a game
        // type at runtime.
        //
        // `InternalCall` declares "the body lives in the runtime, not here",
        // which is legal with no RVA. The type then loads, and only an actual
        // *call* fails - a far better failure mode, and the shape a real
        // interop layer needs anyway, since it binds these to IL2CPP thunks.
        row.impl_flags = method_impl::INTERNAL_CALL;
        for param in &method.parameters {
            row = row.with_parameter(modrix_ecma335::Parameter::new(param.name.clone()));
        }
        for name in &method.generic_params {
            row = row.with_generic_parameter(GenericParameter::new(name.clone()));
        }
        Ok(self.builder.add_method(owner, row)?)
    }

    /// A token for a type used in a *token* position - a base class or an
    /// implemented interface - or `None` when it cannot be expressed.
    ///
    /// Primitives have to be handled here as well as named types. A base class
    /// is a token, not a signature, so `ELEMENT_TYPE_OBJECT` cannot be written
    /// inline: it must become a `TypeRef` to `System.Object`. IL2CPP records
    /// `UnityEngine.Object`'s base exactly that way, and returning `None` for it
    /// emitted a class with no base at all - which the runtime rejects with
    /// "Could not load type ... because the parent does not exist", taking the
    /// whole assembly with it.
    fn token_of(&mut self, ty: &ResolvedType) -> Result<Option<TypeDefOrRef>> {
        match ty {
            ResolvedType::Named { type_def, .. } | ResolvedType::Generic { type_def, .. } => {
                self.token_of_index(*type_def)
            }
            ResolvedType::Primitive(kind) => Ok(self.corlib_token(*kind)),
            _ => Ok(None),
        }
    }

    /// A `TypeRef` into the real corlib for a primitive named in a token
    /// position.
    fn corlib_token(&mut self, kind: TypeKind) -> Option<TypeDefOrRef> {
        let name = match kind {
            TypeKind::Object => "Object",
            TypeKind::String => "String",
            TypeKind::Void => "Void",
            TypeKind::Boolean => "Boolean",
            TypeKind::Char => "Char",
            TypeKind::I1 => "SByte",
            TypeKind::U1 => "Byte",
            TypeKind::I2 => "Int16",
            TypeKind::U2 => "UInt16",
            TypeKind::I4 => "Int32",
            TypeKind::U4 => "UInt32",
            TypeKind::I8 => "Int64",
            TypeKind::U8 => "UInt64",
            TypeKind::R4 => "Single",
            TypeKind::R8 => "Double",
            TypeKind::I => "IntPtr",
            TypeKind::U => "UIntPtr",
            TypeKind::TypedByRef => "TypedReference",
            _ => return None,
        };
        let scope = ResolutionScope::AssemblyRef(self.corlib);
        Some(TypeDefOrRef::Reference(
            self.builder.add_type_ref(scope, "System", name),
        ))
    }

    /// A token for a metadata type index, defining it locally or importing it.
    fn token_of_index(&mut self, type_def: usize) -> Result<Option<TypeDefOrRef>> {
        if let Some(id) = self.local.get(&type_def) {
            return Ok(Some(TypeDefOrRef::Definition(*id)));
        }
        if let Some(id) = self.foreign.get(&type_def) {
            return Ok(Some(TypeDefOrRef::Reference(*id)));
        }
        let Some(owner) = self.model.owner_of.get(&type_def).copied() else {
            return Ok(None);
        };
        let def = self.metadata.type_def(type_def)?;
        // A base-class-library type is rewritten onto the real corlib rather
        // than emitted, so `string` in a game signature binds to the `string`
        // the mod already references. Anything not on the allowlist is
        // declined; see the `corlib` module for why that direction is right.
        if self
            .model
            .assemblies
            .get(owner)
            .is_some_and(|a| crate::corlib::is_corlib_assembly(&a.name))
        {
            if !crate::corlib::is_redirectable(&def.namespace, &def.name) {
                return Ok(None);
            }
            let scope = ResolutionScope::AssemblyRef(self.corlib);
            let id = self.builder.add_type_ref(scope, &def.namespace, &def.name);
            self.foreign.insert(type_def, id);
            return Ok(Some(TypeDefOrRef::Reference(id)));
        }
        // A same-assembly type with no id yet is one the ordering pass has not
        // reached; that only happens for a cycle the metadata should not
        // contain, so decline rather than mis-scope it to an AssemblyRef.
        if owner == self.own {
            return Ok(None);
        }
        let scope = ResolutionScope::AssemblyRef(self.assembly_ref(owner)?);
        let id = self.builder.add_type_ref(scope, &def.namespace, &def.name);
        self.foreign.insert(type_def, id);
        Ok(Some(TypeDefOrRef::Reference(id)))
    }

    /// The `AssemblyRef` row for another game assembly, added on first use.
    fn assembly_ref(&mut self, owner: usize) -> Result<modrix_ecma335::AssemblyRefId> {
        if let Some(id) = self.refs.get(&owner) {
            return Ok(*id);
        }
        let assembly = self
            .model
            .assemblies
            .get(owner)
            .ok_or(Error::NoSuchAssembly { index: owner })?;
        let reference = AssemblyReference {
            name: assembly.name.clone(),
            version: parse_version(&assembly.version),
            culture: String::new(),
            public_key_or_token: Vec::new(),
            flags: 0,
        };
        let id = self.builder.add_assembly_ref(reference);
        self.refs.insert(owner, id);
        Ok(id)
    }

    /// Translate a resolved IL2CPP type into a signature type.
    ///
    /// `None` means "cannot be expressed in a reference assembly"; the caller
    /// drops the member rather than emit something wrong.
    fn convert(&mut self, ty: &ResolvedType) -> Result<Option<Type>> {
        Ok(match ty {
            ResolvedType::Primitive(kind) => primitive(*kind),
            ResolvedType::Named {
                type_def,
                value_type,
            } => self.named(*type_def, *value_type)?,
            ResolvedType::SzArray(inner) => {
                self.convert(inner)?.map(|t| Type::SzArray(Box::new(t)))
            }
            ResolvedType::Array { element, rank } => self.convert(element)?.map(|t| Type::Array {
                element: Box::new(t),
                rank: *rank,
                sizes: Vec::new(),
                lower_bounds: Vec::new(),
            }),
            ResolvedType::Pointer(inner) => self.convert(inner)?.map(|t| Type::Ptr(Box::new(t))),
            ResolvedType::ByRef(inner) => self.convert(inner)?.map(|t| Type::ByRef(Box::new(t))),
            ResolvedType::Generic {
                type_def,
                value_type,
                args,
            } => self.generic(*type_def, *value_type, args)?,
            ResolvedType::TypeParam(index) => Some(Type::TypeParameter(*index)),
            ResolvedType::MethodParam(index) => Some(Type::MethodParameter(*index)),
            // A function pointer's full signature is not modelled; IntPtr is
            // the right shape and keeps the member bindable.
            ResolvedType::FnPtr => Some(Type::IntPtr),
        })
    }

    fn named(&mut self, type_def: usize, value_type: bool) -> Result<Option<Type>> {
        Ok(self.token_of_index(type_def)?.map(|token| {
            if value_type {
                Type::ValueType(token)
            } else {
                Type::Class(token)
            }
        }))
    }

    fn generic(
        &mut self,
        type_def: usize,
        value_type: bool,
        args: &[ResolvedType],
    ) -> Result<Option<Type>> {
        let Some(definition) = self.token_of_index(type_def)? else {
            return Ok(None);
        };
        let mut arguments = Vec::with_capacity(args.len());
        for arg in args {
            let Some(converted) = self.convert(arg)? else {
                return Ok(None);
            };
            arguments.push(converted);
        }
        Ok(Some(Type::GenericInstance {
            value_type,
            definition,
            arguments,
        }))
    }
}

/// Dependency ordering for type declarations.
struct Layout;

impl Layout {
    /// Positions within `assembly.types`, ordered so that a type's base and
    /// enclosing type are always declared first.
    fn order(assembly: &AssemblyModel, own: usize, model: &GameModel) -> Vec<usize> {
        let mut pending: Vec<usize> = (0..assembly.types.len()).collect();
        let mut done: std::collections::HashSet<usize> = std::collections::HashSet::new();
        let mut out = Vec::with_capacity(pending.len());

        for _ in 0..MAX_ORDER_ROUNDS {
            if pending.is_empty() {
                break;
            }
            let mut progressed = false;
            let mut deferred = Vec::new();
            for position in pending {
                let Some(ty) = assembly.types.get(position) else {
                    continue;
                };
                if Self::ready(ty, own, model, &done) {
                    done.insert(ty.type_def);
                    out.push(position);
                    progressed = true;
                } else {
                    deferred.push(position);
                }
            }
            pending = deferred;
            if !progressed {
                // A cycle the metadata should not contain. Emit the rest in
                // metadata order: a base that never resolves just becomes
                // `None`, which is harmless. `declare_one` still refuses to
                // promote an unresolved nested type, so this cannot reintroduce
                // duplicate `<>c` names.
                out.extend(pending);
                break;
            }
        }
        out
    }

    /// Whether every same-assembly dependency of `ty` is already declared.
    fn ready(
        ty: &TypeModel,
        own: usize,
        model: &GameModel,
        done: &std::collections::HashSet<usize>,
    ) -> bool {
        let local_and_missing = |index: usize| {
            model.owner_of.get(&index).copied() == Some(own) && !done.contains(&index)
        };
        if let Some(outer) = ty.declaring_type
            && local_and_missing(outer)
        {
            return false;
        }
        if let Some(ResolvedType::Named { type_def, .. }) = &ty.base
            && local_and_missing(*type_def)
        {
            return false;
        }
        true
    }
}

/// Wrap a decoded integer constant in the `Constant` variant matching the
/// field's declared type.
///
/// IL2CPP stores every field constant as one compressed integer, so the width
/// and signedness come from the field type rather than from the blob. An enum
/// member's field type is the enum itself, which is a value type - its
/// underlying width is not recorded here, so `i32` is the safe default: it is
/// what C# uses unless an enum explicitly says otherwise.
fn constant_for(ty: &Type, value: i64) -> modrix_ecma335::Constant {
    use modrix_ecma335::Constant;
    match ty {
        Type::Boolean => Constant::Boolean(value != 0),
        Type::Char => Constant::Char(u16::try_from(value).unwrap_or(0)),
        Type::I1 => Constant::I1(i8::try_from(value).unwrap_or(0)),
        Type::U1 => Constant::U1(u8::try_from(value).unwrap_or(0)),
        Type::I2 => Constant::I2(i16::try_from(value).unwrap_or(0)),
        Type::U2 => Constant::U2(u16::try_from(value).unwrap_or(0)),
        Type::U4 => Constant::U4(u32::try_from(value).unwrap_or(0)),
        Type::I8 => Constant::I8(value),
        Type::U8 => Constant::U8(u64::try_from(value).unwrap_or(0)),
        _ => Constant::I4(i32::try_from(value).unwrap_or(0)),
    }
}

/// The `ELEMENT_TYPE_*` equivalent of an IL2CPP primitive kind.
fn primitive(kind: TypeKind) -> Option<Type> {
    Some(match kind {
        TypeKind::Void => Type::Void,
        TypeKind::Boolean => Type::Boolean,
        TypeKind::Char => Type::Char,
        TypeKind::I1 => Type::I1,
        TypeKind::U1 => Type::U1,
        TypeKind::I2 => Type::I2,
        TypeKind::U2 => Type::U2,
        TypeKind::I4 => Type::I4,
        TypeKind::U4 => Type::U4,
        TypeKind::I8 => Type::I8,
        TypeKind::U8 => Type::U8,
        TypeKind::R4 => Type::R4,
        TypeKind::R8 => Type::R8,
        TypeKind::String => Type::String,
        TypeKind::Object => Type::Object,
        TypeKind::I => Type::IntPtr,
        TypeKind::U => Type::UIntPtr,
        TypeKind::TypedByRef => Type::TypedByRef,
        _ => return None,
    })
}

/// Parse a `major.minor.build.revision` string, defaulting missing parts to 0.
fn parse_version(text: &str) -> Version {
    let mut parts = text.split('.').map(|p| p.parse::<u16>().unwrap_or(0));
    Version {
        major: parts.next().unwrap_or(0),
        minor: parts.next().unwrap_or(0),
        build: parts.next().unwrap_or(0),
        revision: parts.next().unwrap_or(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_a_four_part_version_and_tolerates_short_ones() {
        assert_eq!(
            parse_version("1.2.3.4"),
            Version {
                major: 1,
                minor: 2,
                build: 3,
                revision: 4
            }
        );
        assert_eq!(
            parse_version("2.0"),
            Version {
                major: 2,
                minor: 0,
                build: 0,
                revision: 0
            }
        );
    }

    #[test]
    fn every_il2cpp_primitive_maps_to_a_signature_type() {
        for kind in [
            TypeKind::Void,
            TypeKind::Boolean,
            TypeKind::Char,
            TypeKind::I1,
            TypeKind::U1,
            TypeKind::I2,
            TypeKind::U2,
            TypeKind::I4,
            TypeKind::U4,
            TypeKind::I8,
            TypeKind::U8,
            TypeKind::R4,
            TypeKind::R8,
            TypeKind::String,
            TypeKind::Object,
            TypeKind::I,
            TypeKind::U,
            TypeKind::TypedByRef,
        ] {
            assert!(primitive(kind).is_some(), "{kind:?} must map");
        }
        // Structural kinds are handled by `convert`, not by `primitive`.
        assert!(primitive(TypeKind::Class).is_none());
        assert!(primitive(TypeKind::GenericInst).is_none());
    }
}

#[cfg(test)]
mod real_game_tests {
    use super::*;
    use crate::model::GameModel;
    use modrix_il2cpp::GameBinary;

    fn real_game() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let home = std::env::var_os("HOME")?;
        let root = std::path::Path::new(&home)
            .join(".local/share/Steam/steamapps/common/Sons Of The Forest");
        let binary = root.join("GameAssembly.dll");
        let metadata = root.join("SonsOfTheForest_Data/il2cpp_data/Metadata/global-metadata.dat");
        (binary.exists() && metadata.exists()).then_some((binary, metadata))
    }

    /// No two types in one assembly may share a top-level (namespace, name).
    ///
    /// Compiler-generated closure classes are all named `<>c` and are unique
    /// only by nesting; two at top level make the runtime reject the whole
    /// assembly with `BadImageFormatException: Duplicate type with name`.
    #[test]
    fn no_assembly_declares_two_types_with_the_same_top_level_name() {
        let Some((binary_path, metadata_path)) = real_game() else {
            return;
        };
        let metadata = Metadata::open(&metadata_path).unwrap();
        let binary = GameBinary::open(&binary_path, metadata.type_count().unwrap()).unwrap();
        let model = GameModel::build(&metadata, &binary).unwrap();

        for (index, assembly) in model.assemblies.iter().enumerate() {
            if assembly.types.is_empty() || crate::corlib::is_corlib_assembly(&assembly.name) {
                continue;
            }
            // Reproduce exactly what the emitter declares, in the same order.
            let mut emitter = Emitter::new(&model, &metadata, index, assembly);
            emitter.declare_types(assembly).unwrap();

            let mut seen: std::collections::HashSet<(String, String)> =
                std::collections::HashSet::new();
            for position in Layout::order(assembly, index, &model) {
                let Some(ty) = assembly.types.get(position) else {
                    continue;
                };
                // Only top-level declarations share a namespace; a nested type
                // is scoped by its enclosing type.
                let declared_nested = ty
                    .declaring_type
                    .is_some_and(|d| emitter.local.contains_key(&d));
                if declared_nested || !emitter.local.contains_key(&ty.type_def) {
                    continue;
                }
                let key = (ty.namespace.clone(), ty.name.clone());
                assert!(
                    seen.insert(key.clone()),
                    "{} declares two top-level types named {}.{}",
                    assembly.name,
                    key.0,
                    key.1
                );
            }
        }
    }

    #[test]
    fn emits_reference_assemblies_for_the_real_game() {
        let Some((binary_path, metadata_path)) = real_game() else {
            return; // Game not installed.
        };
        let metadata = Metadata::open(&metadata_path).unwrap();
        let binary = GameBinary::open(&binary_path, metadata.type_count().unwrap()).unwrap();
        let model = GameModel::build(&metadata, &binary).unwrap();

        let emitted = emit_all(&model, &metadata).unwrap();
        assert!(
            emitted.len() > 50,
            "expected many assemblies, got {}",
            emitted.len()
        );
        // The corlib must NOT be emitted - its types redirect instead.
        assert!(
            !emitted.iter().any(|(name, _)| name == "mscorlib"),
            "the game corlib must be redirected, not emitted"
        );
        // The assemblies mods actually reference must be present and non-trivial.
        for wanted in ["Assembly-CSharp", "Sons", "UnityEngine.CoreModule"] {
            let Some((_, body)) = emitted.iter().find(|(name, _)| name == wanted) else {
                panic!("{wanted} was not emitted")
            };
            let bytes = body.len();
            assert!(bytes > 2048, "{wanted} is only {bytes} bytes");
            let dir = std::env::var("MODRIX_INTEROP_OUT").ok();
            if let Some(dir) = dir {
                let out = std::path::Path::new(&dir);
                std::fs::create_dir_all(out).unwrap();
                for (name, bytes) in &emitted {
                    std::fs::write(out.join(format!("{name}.dll")), bytes).unwrap();
                }
            }
        }
    }
}
