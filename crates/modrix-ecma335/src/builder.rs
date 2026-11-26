// SPDX-License-Identifier: GPL-2.0-only
//! The public builder: collect a declarative description, then emit bytes.
//!
//! Nothing is encoded while the description is being collected. Members are
//! kept attached to the type that owns them and are only flattened into
//! `Field` / `MethodDef` rows by [`AssemblyBuilder::build`], because ECMA-335
//! stores a type's member run as a start index and takes the end from the
//! *next* type's start. Grouping members by owner is therefore not a style
//! choice - interleaving them silently reassigns members to the wrong types -
//! so the API makes it impossible to get wrong.

use std::collections::HashMap;

use crate::encode::compress_unsigned;
use crate::error::{Error, Result};
use crate::handles::{
    AssemblyRefId, CustomAttributeType, FieldId, HasCustomAttribute, MemberRefId, MemberRefParent,
    MethodId, PropertyId, ResolutionScope, TypeDefId, TypeRefId, TypeSpecId,
};
use crate::model::{
    AssemblyReference, Field, Method, Property, TypeDefinition, Version, method_attributes,
};
use crate::signature::{MethodSignature, Type};

/// SHA-1, the only hash algorithm identifier in practical use.
pub const HASH_ALGORITHM_SHA1: u32 = 0x0000_8004;

/// Namespace of `ReferenceAssemblyAttribute`.
const COMPILER_SERVICES: &str = "System.Runtime.CompilerServices";
/// Namespace of `TargetFrameworkAttribute`.
const VERSIONING: &str = "System.Runtime.Versioning";

/// A `TypeRef` the caller asked for.
#[derive(Clone, Debug)]
pub(crate) struct TypeRefEntry {
    pub(crate) scope: ResolutionScope,
    pub(crate) namespace: String,
    pub(crate) name: String,
}

/// A `MemberRef` the caller asked for.
#[derive(Clone, Debug)]
pub(crate) struct MemberRefEntry {
    pub(crate) parent: MemberRefParent,
    pub(crate) name: String,
    pub(crate) signature: Vec<u8>,
}

/// A `CustomAttribute` the caller asked for.
#[derive(Clone, Debug)]
pub(crate) struct CustomAttributeEntry {
    pub(crate) parent: HasCustomAttribute,
    pub(crate) constructor: CustomAttributeType,
    pub(crate) value: Vec<u8>,
}

/// A type plus the members that belong to it.
#[derive(Clone, Debug)]
pub(crate) struct TypeEntry {
    pub(crate) definition: TypeDefinition,
    pub(crate) fields: Vec<Field>,
    pub(crate) methods: Vec<Method>,
    pub(crate) properties: Vec<Property>,
}

/// Builds one .NET reference assembly.
///
/// # Example
///
/// ```
/// use modrix_ecma335::{AssemblyBuilder, ResolutionScope, Version};
/// use modrix_ecma335::model::{AssemblyReference, TypeDefinition, type_attributes};
///
/// let mut builder = AssemblyBuilder::new("MyLib", Version::new(1, 0, 0, 0));
/// let corlib = builder.add_assembly_ref(AssemblyReference::core_library(9));
/// let object = builder.add_type_ref(ResolutionScope::AssemblyRef(corlib), "System", "Object");
/// builder.add_type_def(TypeDefinition::new(
///     "MyLib",
///     "Widget",
///     type_attributes::PUBLIC_CLASS,
///     Some(object.into()),
/// ))?;
/// let bytes = builder.build()?;
/// assert_eq!(&bytes[..2], b"MZ");
/// # Ok::<(), modrix_ecma335::Error>(())
/// ```
#[derive(Clone, Debug)]
pub struct AssemblyBuilder {
    pub(crate) name: String,
    pub(crate) version: Version,
    pub(crate) module_name: String,
    pub(crate) mvid: [u8; 16],
    pub(crate) hash_algorithm: u32,
    pub(crate) flags: u32,
    pub(crate) public_key: Vec<u8>,
    pub(crate) culture: String,
    pub(crate) assembly_refs: Vec<AssemblyReference>,
    pub(crate) type_refs: Vec<TypeRefEntry>,
    pub(crate) type_specs: Vec<Vec<u8>>,
    type_spec_index: HashMap<Vec<u8>, u32>,
    pub(crate) types: Vec<TypeEntry>,
    pub(crate) member_refs: Vec<MemberRefEntry>,
    pub(crate) custom_attributes: Vec<CustomAttributeEntry>,
}

impl AssemblyBuilder {
    /// Starts an assembly named `name` at `version`.
    ///
    /// `<Module>` is pre-seeded as `TypeDef` row 1, as ECMA-335 II.22.37
    /// requires, and the module name defaults to `<name>.dll`. The MVID is
    /// derived deterministically from the name and version so that two runs
    /// over the same input produce byte-identical files; call
    /// [`AssemblyBuilder::set_mvid`] to override it.
    #[must_use]
    pub fn new(name: &str, version: Version) -> Self {
        let module = TypeEntry {
            definition: TypeDefinition::new(String::new(), "<Module>", 0, None),
            fields: Vec::new(),
            methods: Vec::new(),
            properties: Vec::new(),
        };
        Self {
            module_name: format!("{name}.dll"),
            mvid: derive_mvid(name, version),
            name: name.to_owned(),
            version,
            hash_algorithm: HASH_ALGORITHM_SHA1,
            flags: 0,
            public_key: Vec::new(),
            culture: String::new(),
            assembly_refs: Vec::new(),
            type_refs: Vec::new(),
            type_specs: Vec::new(),
            type_spec_index: HashMap::new(),
            types: vec![module],
            member_refs: Vec::new(),
            custom_attributes: Vec::new(),
        }
    }

    /// Overrides the `Module.Name` string, which defaults to `<name>.dll`.
    pub fn set_module_name(&mut self, name: &str) {
        name.clone_into(&mut self.module_name);
    }

    /// Overrides the module version identifier.
    pub fn set_mvid(&mut self, mvid: [u8; 16]) {
        self.mvid = mvid;
    }

    /// Sets `Assembly.PublicKey` and, implicitly, the identity the consumer
    /// will match on. Leave empty for an unsigned reference assembly.
    pub fn set_public_key(&mut self, key: Vec<u8>) {
        self.public_key = key;
    }

    /// Sets `Assembly.Culture`; empty means culture-neutral.
    pub fn set_culture(&mut self, culture: &str) {
        culture.clone_into(&mut self.culture);
    }

    /// Sets `Assembly.Flags`.
    pub fn set_assembly_flags(&mut self, flags: u32) {
        self.flags = flags;
    }

    /// Adds an `AssemblyRef` row.
    ///
    /// For net6.0 and later, exactly one of these must be
    /// [`AssemblyReference::core_library`]: Roslyn will not accept
    /// `mscorlib`, `netstandard` or `System.Private.CoreLib` as a substitute
    /// and reports CS0012 for every imported type.
    pub fn add_assembly_ref(&mut self, reference: AssemblyReference) -> AssemblyRefId {
        let index = u32::try_from(self.assembly_refs.len()).unwrap_or(u32::MAX);
        self.assembly_refs.push(reference);
        AssemblyRefId(index)
    }

    /// Adds a `TypeRef` row naming an imported type.
    pub fn add_type_ref(
        &mut self,
        scope: ResolutionScope,
        namespace: &str,
        name: &str,
    ) -> TypeRefId {
        let index = u32::try_from(self.type_refs.len()).unwrap_or(u32::MAX);
        self.type_refs.push(TypeRefEntry {
            scope,
            namespace: namespace.to_owned(),
            name: name.to_owned(),
        });
        TypeRefId(index)
    }

    /// Interns a `TypeSpec` row for a constructed type.
    ///
    /// Only needed where a constructed type must be named by a *token*: a base
    /// class, an interface, a `MemberRef` parent, or a custom-attribute type.
    /// A `GENERICINST` that appears inside a field or method signature blob
    /// needs no `TypeSpec` row at all.
    ///
    /// # Errors
    ///
    /// Propagates any [`Type::encode`] failure.
    pub fn add_type_spec(&mut self, constructed: &Type) -> Result<TypeSpecId> {
        let mut signature = Vec::new();
        constructed.encode(&mut signature)?;
        if let Some(existing) = self.type_spec_index.get(&signature) {
            return Ok(TypeSpecId(*existing));
        }
        let index = u32::try_from(self.type_specs.len()).unwrap_or(u32::MAX);
        self.type_spec_index.insert(signature.clone(), index);
        self.type_specs.push(signature);
        Ok(TypeSpecId(index))
    }

    /// Adds a `TypeDef` row.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DanglingHandle`] if the enclosing type handle does not
    /// name a type of this assembly, or [`Error::NestedTypeHasNamespace`] if a
    /// nested type carries a namespace (ECMA-335 requires it to be empty; the
    /// namespace comes from the enclosing type).
    pub fn add_type_def(&mut self, definition: TypeDefinition) -> Result<TypeDefId> {
        if let Some(enclosing) = definition.enclosing {
            self.check_type(enclosing)?;
            if !definition.namespace.is_empty() {
                return Err(Error::NestedTypeHasNamespace {
                    name: definition.name.clone(),
                });
            }
        }
        let index = u32::try_from(self.types.len()).unwrap_or(u32::MAX);
        self.types.push(TypeEntry {
            definition,
            fields: Vec::new(),
            methods: Vec::new(),
            properties: Vec::new(),
        });
        Ok(TypeDefId(index))
    }

    /// Adds a field to `owner`.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DanglingHandle`] if `owner` is not a type of this
    /// assembly.
    pub fn add_field(&mut self, owner: TypeDefId, field: Field) -> Result<FieldId> {
        let entry = self.type_mut(owner)?;
        let slot = u32::try_from(entry.fields.len()).unwrap_or(u32::MAX);
        entry.fields.push(field);
        Ok(FieldId {
            owner: owner.0,
            slot,
        })
    }

    /// Adds a method to `owner`. `MethodDef.RVA` is always emitted as 0.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DanglingHandle`] if `owner` is not a type of this
    /// assembly.
    pub fn add_method(&mut self, owner: TypeDefId, method: Method) -> Result<MethodId> {
        let entry = self.type_mut(owner)?;
        let slot = u32::try_from(entry.methods.len()).unwrap_or(u32::MAX);
        entry.methods.push(method);
        Ok(MethodId {
            owner: owner.0,
            slot,
        })
    }

    /// Adds a property to `owner`, together with its accessor links.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DanglingHandle`] if `owner` or either accessor handle
    /// does not name a member of this assembly.
    pub fn add_property(&mut self, owner: TypeDefId, property: Property) -> Result<PropertyId> {
        for accessor in [property.getter, property.setter].into_iter().flatten() {
            self.check_method(accessor)?;
        }
        let entry = self.type_mut(owner)?;
        let slot = u32::try_from(entry.properties.len()).unwrap_or(u32::MAX);
        entry.properties.push(property);
        Ok(PropertyId {
            owner: owner.0,
            slot,
        })
    }

    /// Adds a `MemberRef` row, typically an imported attribute constructor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DanglingHandle`] if `parent` does not resolve, or
    /// propagates a signature encoding failure.
    pub fn add_member_ref(
        &mut self,
        parent: MemberRefParent,
        name: &str,
        signature: &MethodSignature,
    ) -> Result<MemberRefId> {
        self.check_member_ref_parent(parent)?;
        let index = u32::try_from(self.member_refs.len()).unwrap_or(u32::MAX);
        self.member_refs.push(MemberRefEntry {
            parent,
            name: name.to_owned(),
            signature: signature.to_blob()?,
        });
        Ok(MemberRefId(index))
    }

    /// Adds a `CustomAttribute` row. `value` is a raw attribute blob: the
    /// `0x0001` prolog, the fixed constructor arguments, then a named-argument
    /// count.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DanglingHandle`] if the parent or the constructor does
    /// not resolve.
    pub fn add_custom_attribute(
        &mut self,
        parent: HasCustomAttribute,
        constructor: CustomAttributeType,
        value: Vec<u8>,
    ) -> Result<()> {
        self.check_custom_attribute_parent(parent)?;
        match constructor {
            CustomAttributeType::MethodDef(id) => self.check_method(id)?,
            CustomAttributeType::MemberRef(id) => self.check_member_ref(id)?,
        }
        self.custom_attributes.push(CustomAttributeEntry {
            parent,
            constructor,
            value,
        });
        Ok(())
    }

    /// Stamps the assembly with `ReferenceAssemblyAttribute` and
    /// `TargetFrameworkAttribute`, the two markers a real reference assembly
    /// carries. `framework` is a moniker such as `.NETCoreApp,Version=v9.0`.
    ///
    /// # Errors
    ///
    /// Propagates the underlying `TypeRef` / `MemberRef` failures.
    pub fn mark_reference_assembly(
        &mut self,
        corlib: AssemblyRefId,
        framework: &str,
    ) -> Result<()> {
        let scope = ResolutionScope::AssemblyRef(corlib);
        let marker = self.add_type_ref(scope, COMPILER_SERVICES, "ReferenceAssemblyAttribute");
        let target = self.add_type_ref(scope, VERSIONING, "TargetFrameworkAttribute");
        let no_args = MethodSignature::instance_method(Type::Void, Vec::new());
        let one_string = MethodSignature::instance_method(Type::Void, vec![Type::String]);
        let marker_ctor =
            self.add_member_ref(MemberRefParent::TypeRef(marker), ".ctor", &no_args)?;
        let target_ctor =
            self.add_member_ref(MemberRefParent::TypeRef(target), ".ctor", &one_string)?;
        self.add_custom_attribute(
            HasCustomAttribute::Assembly,
            CustomAttributeType::MemberRef(marker_ctor),
            attribute_blob_empty(),
        )?;
        self.add_custom_attribute(
            HasCustomAttribute::Assembly,
            CustomAttributeType::MemberRef(target_ctor),
            attribute_blob_string(framework)?,
        )
    }

    /// Convenience: adds a public parameterless instance constructor.
    ///
    /// # Errors
    ///
    /// Returns [`Error::DanglingHandle`] if `owner` does not resolve.
    pub fn add_default_constructor(&mut self, owner: TypeDefId) -> Result<MethodId> {
        self.add_method(
            owner,
            Method::new(
                ".ctor",
                method_attributes::PUBLIC_CONSTRUCTOR,
                MethodSignature::instance_method(Type::Void, Vec::new()),
            ),
        )
    }

    /// Serialises the whole description into a PE32+ reference assembly.
    ///
    /// # Errors
    ///
    /// Returns an [`Error`] if any handle dangles, any literal field lacks its
    /// `Constant` row, a heap or table exceeds its ceiling, or the image
    /// layout overflows.
    pub fn build(&self) -> Result<Vec<u8>> {
        let image = crate::materialize::materialize(self)?;
        crate::pe::write_image(&image)
    }

    fn type_mut(&mut self, id: TypeDefId) -> Result<&mut TypeEntry> {
        let rows = u32::try_from(self.types.len()).unwrap_or(u32::MAX);
        usize::try_from(id.0)
            .ok()
            .and_then(|index| self.types.get_mut(index))
            .ok_or(Error::DanglingHandle {
                kind: "TypeDef",
                index: id.0,
                rows,
            })
    }

    fn check_type(&self, id: TypeDefId) -> Result<()> {
        check(self.types.len(), id.0, "TypeDef")
    }

    fn check_member_ref(&self, id: MemberRefId) -> Result<()> {
        check(self.member_refs.len(), id.0, "MemberRef")
    }

    fn check_method(&self, id: MethodId) -> Result<()> {
        let methods = self
            .types
            .get(usize::try_from(id.owner).unwrap_or(usize::MAX))
            .map_or(0, |entry| entry.methods.len());
        check(self.types.len(), id.owner, "TypeDef")?;
        check(methods, id.slot, "MethodDef")
    }

    fn check_field(&self, id: FieldId) -> Result<()> {
        let fields = self
            .types
            .get(usize::try_from(id.owner).unwrap_or(usize::MAX))
            .map_or(0, |entry| entry.fields.len());
        check(self.types.len(), id.owner, "TypeDef")?;
        check(fields, id.slot, "Field")
    }

    fn check_property(&self, id: PropertyId) -> Result<()> {
        let properties = self
            .types
            .get(usize::try_from(id.owner).unwrap_or(usize::MAX))
            .map_or(0, |entry| entry.properties.len());
        check(self.types.len(), id.owner, "TypeDef")?;
        check(properties, id.slot, "Property")
    }

    fn check_member_ref_parent(&self, parent: MemberRefParent) -> Result<()> {
        match parent {
            MemberRefParent::TypeDef(id) => self.check_type(id),
            MemberRefParent::TypeRef(id) => check(self.type_refs.len(), id.0, "TypeRef"),
            MemberRefParent::TypeSpec(id) => check(self.type_specs.len(), id.0, "TypeSpec"),
            MemberRefParent::MethodDef(id) => self.check_method(id),
        }
    }

    fn check_custom_attribute_parent(&self, parent: HasCustomAttribute) -> Result<()> {
        match parent {
            HasCustomAttribute::Assembly | HasCustomAttribute::Module => Ok(()),
            HasCustomAttribute::TypeDef(id) => self.check_type(id),
            HasCustomAttribute::TypeRef(id) => check(self.type_refs.len(), id.0, "TypeRef"),
            HasCustomAttribute::Field(id) => self.check_field(id),
            HasCustomAttribute::MethodDef(id) => self.check_method(id),
            HasCustomAttribute::Property(id) => self.check_property(id),
            HasCustomAttribute::AssemblyRef(id) => {
                check(self.assembly_refs.len(), id.0, "AssemblyRef")
            }
        }
    }
}

/// Rejects a handle that names no row, which is the guard against writing a
/// dangling coded index. A dangling `TypeRef.ResolutionScope` does not produce
/// a diagnostic in Roslyn - it takes the compiler down with an
/// `ArgumentOutOfRangeException`.
fn check(rows: usize, index: u32, kind: &'static str) -> Result<()> {
    let rows = u32::try_from(rows).unwrap_or(u32::MAX);
    if index < rows {
        Ok(())
    } else {
        Err(Error::DanglingHandle { kind, index, rows })
    }
}

/// The attribute blob for a constructor taking no arguments: prolog `0x0001`
/// then a zero named-argument count.
#[must_use]
pub fn attribute_blob_empty() -> Vec<u8> {
    vec![0x01, 0x00, 0x00, 0x00]
}

/// The attribute blob for a constructor taking one `string`.
///
/// # Errors
///
/// Returns [`Error::CompressedRange`] if the string is longer than a
/// compressed length prefix can describe.
pub fn attribute_blob_string(value: &str) -> Result<Vec<u8>> {
    let mut blob = vec![0x01, 0x00];
    let bytes = value.as_bytes();
    compress_unsigned(&mut blob, u32::try_from(bytes.len()).unwrap_or(u32::MAX))?;
    blob.extend_from_slice(bytes);
    blob.extend_from_slice(&[0x00, 0x00]);
    Ok(blob)
}

/// Derives a stable, non-zero MVID from the assembly identity.
///
/// A random GUID would make every build of the same input differ, which breaks
/// reproducible builds and content-addressed caching; the MVID only has to be
/// unique per distinct module, not unpredictable.
fn derive_mvid(name: &str, version: Version) -> [u8; 16] {
    let mut mvid = [0_u8; 16];
    let mut hash: u64 = 0xCBF2_9CE4_8422_2325;
    let mix = |byte: u8, hash: &mut u64| {
        *hash ^= u64::from(byte);
        *hash = hash.wrapping_mul(0x0000_0100_0000_01B3);
    };
    for byte in name.as_bytes() {
        mix(*byte, &mut hash);
    }
    for part in [
        version.major,
        version.minor,
        version.build,
        version.revision,
    ] {
        for byte in part.to_le_bytes() {
            mix(byte, &mut hash);
        }
    }
    let low = hash.to_le_bytes();
    let high = hash.rotate_left(17).to_le_bytes();
    if let Some(slot) = mvid.get_mut(0..8) {
        slot.copy_from_slice(&low);
    }
    if let Some(slot) = mvid.get_mut(8..16) {
        slot.copy_from_slice(&high);
    }
    // The Module row requires a non-zero MVID; FNV-1a of a non-empty identity
    // cannot be zero, but the fallback keeps that guarantee unconditional.
    if mvid.iter().all(|byte| *byte == 0) {
        mvid = [1_u8; 16];
    }
    mvid
}
