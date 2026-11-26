// SPDX-License-Identifier: GPL-2.0-only
//! The interop model: what to emit, before deciding how to encode it.
//!
//! Reading IL2CPP gives one flat world - 24,056 types and 182,725 methods with
//! no assembly boundaries in sight. A mod, though, compiles against
//! *assemblies*: `Assembly-CSharp.dll`, `Sons.dll`, `UnityEngine.CoreModule.dll`.
//! This module performs that partition and decides what belongs in the output.
//!
//! Two decisions shape everything downstream:
//!
//! **Everything is emitted, and everything is made public.** This is the
//! "publicized assembly" convention every Unity modding toolchain uses -
//! `RedLoader` ships `UnityEngine.CoreModule-nstrip.dll` for exactly this
//! reason. A normal reference assembly exposes only the public API, but a mod
//! is not a normal consumer: Harmony patches private methods by name, and mods
//! routinely read private fields. Emitting only the public surface makes the
//! reference assembly technically correct and practically useless - the three
//! mods in the reference repo fail on `DebugConsole.CheckDebugInputs`,
//! `ItemInstance._itemID` and `WorldSimActor._state`, all non-public.
//!
//! Widening visibility is safe here because these assemblies are *reference
//! only*: they are never loaded at runtime, so nothing they claim can violate
//! the real type's encapsulation. The runtime binding is against the game's
//! actual IL2CPP types via the loader.
//!
//! **A type belongs to exactly one assembly.** Each image declares a
//! contiguous run of type definitions, so ownership is a range lookup. Types
//! referenced from another assembly become `TypeRef` rows pointing at an
//! `AssemblyRef` - which is why the owner map has to be built before any
//! signature can be encoded.

use std::collections::HashMap;

use modrix_il2cpp::{GameBinary, Metadata, ResolvedType};

use crate::error::{Error, Result};

/// A type that will be emitted, with its members already resolved.
#[derive(Debug, Clone)]
pub struct TypeModel {
    /// Index into the metadata's type-definition table.
    pub type_def: usize,
    /// The type's own name, IL2CPP-style (``List`1``).
    pub name: String,
    /// Its namespace, empty for the global namespace.
    pub namespace: String,
    /// `TypeAttributes`.
    pub flags: u32,
    /// The base type, absent for interfaces and `System.Object`.
    pub base: Option<ResolvedType>,
    /// Enclosing type, when nested.
    pub declaring_type: Option<usize>,
    /// Public fields, in declaration order.
    pub fields: Vec<FieldModel>,
    /// Public methods, in declaration order.
    pub methods: Vec<MethodModel>,
    /// Public properties, in declaration order.
    pub properties: Vec<PropertyModel>,
    /// Declared generic parameter names, from the type's generic container.
    pub generic_params: Vec<String>,
}

/// A field to emit.
#[derive(Debug, Clone)]
pub struct FieldModel {
    /// The field name.
    pub name: String,
    /// Its type.
    pub ty: ResolvedType,
    /// `FieldAttributes`.
    pub flags: u32,
    /// The compile-time constant, for a `const` field or enum member. ECMA-335
    /// *requires* one on every literal field; omitting it makes the consumer
    /// fail with an opaque `CS7038` rather than a useful diagnostic.
    pub constant: Option<i64>,
}

/// A method to emit. Bodies are native, so only the signature survives - which
/// is all a reference assembly needs.
#[derive(Debug, Clone)]
pub struct MethodModel {
    /// The method name (`.ctor`, `get_X`, …).
    pub name: String,
    /// The return type.
    pub returns: ResolvedType,
    /// Parameters, in order.
    pub parameters: Vec<ParamModel>,
    /// `MethodAttributes`.
    pub flags: u16,
    /// Whether the method is static (affects the signature's calling
    /// convention byte, not just the attributes).
    pub is_static: bool,
    /// Declared generic parameter names, from the method's generic container.
    /// Empty for a non-generic method.
    pub generic_params: Vec<String>,
}

/// A property to emit.
///
/// C# binds `x.Foo` through a `Property` row plus `MethodSemantics` links to
/// its accessors - the `get_Foo` method alone is not enough. Omitting these
/// makes every property access fail with `CS0117`, which is the single most
/// visible gap in a naive reference assembly.
#[derive(Debug, Clone)]
pub struct PropertyModel {
    /// The property name.
    pub name: String,
    /// Its type, taken from the getter's return or the setter's value.
    pub ty: ResolvedType,
    /// Index into this type's `methods` of the getter, if any.
    pub getter: Option<usize>,
    /// Index into this type's `methods` of the setter, if any.
    pub setter: Option<usize>,
}

/// A method parameter.
#[derive(Debug, Clone)]
pub struct ParamModel {
    /// The parameter name.
    pub name: String,
    /// Its type.
    pub ty: ResolvedType,
}

/// One assembly's worth of types.
#[derive(Debug, Clone)]
pub struct AssemblyModel {
    /// Assembly name, without extension.
    pub name: String,
    /// Version, as `major.minor.build.revision`.
    pub version: String,
    /// The public types this assembly owns.
    pub types: Vec<TypeModel>,
}

/// The whole game, partitioned into assemblies.
#[derive(Debug)]
pub struct GameModel {
    /// Every assembly, in metadata order.
    pub assemblies: Vec<AssemblyModel>,
    /// Which assembly owns each type definition, by index. Signature encoding
    /// needs this to decide `TypeDef` versus `TypeRef`.
    pub owner_of: HashMap<usize, usize>,
}

/// How many types may be walked before we call the input malformed.
const MAX_TYPES: usize = 1_000_000;

impl GameModel {
    /// Build the model from a game's metadata and binary.
    ///
    /// # Errors
    ///
    /// Propagates any read failure from the metadata or binary, or
    /// [`Error::TooLarge`] if the game declares implausibly many types.
    pub fn build(metadata: &Metadata, binary: &GameBinary) -> Result<Self> {
        let type_count = metadata.type_count()?;
        if type_count > MAX_TYPES {
            return Err(Error::TooLarge {
                what: "type definitions",
                limit: MAX_TYPES,
            });
        }
        let assemblies = metadata.assemblies()?;
        let mut owner_of = HashMap::with_capacity(type_count);
        for (index, assembly) in assemblies.iter().enumerate() {
            for type_index in assembly.types.indices() {
                owner_of.insert(type_index, index);
            }
        }

        // Read the constant table once; per-field lookup would rescan 26k rows.
        let constants = metadata.default_values()?;

        let mut built = Vec::with_capacity(assemblies.len());
        for assembly in &assemblies {
            let mut types = Vec::new();
            for type_index in assembly.types.indices() {
                types.push(build_type(metadata, binary, type_index, &constants)?);
            }
            built.push(AssemblyModel {
                name: assembly.name.clone(),
                version: assembly.version.clone(),
                types,
            });
        }
        Ok(Self {
            assemblies: built,
            owner_of,
        })
    }

    /// Total number of types across every assembly.
    #[must_use]
    pub fn type_count(&self) -> usize {
        self.assemblies
            .iter()
            .map(|a| a.types.len())
            .fold(0_usize, usize::saturating_add)
    }
}

/// Resolve a *binary type-table* index to the type definition it names.
///
/// IL2CPP stores `declaringTypeIndex` and `parentIndex` as indices into the
/// type table that lives in `GameAssembly.dll`, not into the metadata's own
/// type-definition table. Treating one as a definition index silently points at
/// an unrelated type: nested types then fail to find their enclosing type and
/// get emitted at top level, where every compiler-generated `<>c` collides and
/// the runtime rejects the assembly outright.
fn resolve_type_def(binary: &GameBinary, type_index: i32) -> Result<Option<usize>> {
    let Ok(slot) = usize::try_from(type_index) else {
        return Ok(None);
    };
    Ok(match binary.resolve(slot)? {
        ResolvedType::Named { type_def, .. } | ResolvedType::Generic { type_def, .. } => {
            Some(type_def)
        }
        _ => None,
    })
}

/// Build one type's model, resolving every member signature.
fn build_type(
    metadata: &Metadata,
    binary: &GameBinary,
    index: usize,
    constants: &HashMap<usize, modrix_il2cpp::DefaultValue>,
) -> Result<TypeModel> {
    let def = metadata.type_def(index)?;
    let base = usize::try_from(def.parent_type)
        .ok()
        .map(|slot| binary.resolve(slot))
        .transpose()?;

    let mut fields = Vec::with_capacity(def.fields.count);
    for slot in def.fields.indices() {
        let field = metadata.field_def(slot)?;
        let type_slot = usize::try_from(field.type_index).unwrap_or(0);
        // IL2CPP's field record holds only name/type/token - the field's
        // FieldAttributes ride on the `attrs` word of its TYPE entry. Verified
        // against the reference game: LocalPlayer._transform reads Public(6)
        // and _inventory reads Private(1), matching the real declarations.
        let attrs = u32::from(binary.type_at(type_slot)?.attrs);
        fields.push(FieldModel {
            name: field.name,
            ty: binary.resolve(type_slot)?,
            flags: publicize_field(attrs),
            constant: constants.get(&slot).map(|d| d.value),
        });
    }

    let methods = build_methods(metadata, binary, &def)?;
    let properties = build_properties(metadata, &def, &methods)?;
    let generic_params = match metadata.generic_container(def.generic_container)? {
        Some(container) => metadata.generic_param_names(&container)?,
        None => Vec::new(),
    };

    Ok(TypeModel {
        type_def: index,
        name: def.name,
        namespace: def.namespace,
        flags: def.flags,
        base,
        declaring_type: resolve_type_def(binary, def.declaring_type)?,
        fields,
        methods,
        properties,
        generic_params,
    })
}

/// Resolve a type's properties, mapping IL2CPP's accessor indices - which are
/// relative to the type's method run - onto positions in the emitted method
/// list. Only properties whose accessors survived the public filter are kept.
fn build_properties(
    metadata: &Metadata,
    def: &modrix_il2cpp::TypeDef,
    methods: &[MethodModel],
) -> Result<Vec<PropertyModel>> {
    // The public filter drops methods, so emitted positions no longer match
    // metadata order; index by name to bridge the two.
    let position: HashMap<&str, usize> = methods
        .iter()
        .enumerate()
        .map(|(index, method)| (method.name.as_str(), index))
        .collect();
    let start = def.methods.start.unwrap_or(0);
    let mut out = Vec::with_capacity(def.properties.count);
    for slot in def.properties.indices() {
        let property = metadata.property_def(slot)?;
        let accessor = |relative: i32| -> Result<Option<usize>> {
            let Ok(offset) = usize::try_from(relative) else {
                return Ok(None);
            };
            let absolute = start.saturating_add(offset);
            let name = metadata.method_def(absolute)?.name;
            Ok(position.get(name.as_str()).copied())
        };
        let getter = accessor(property.get)?;
        let setter = accessor(property.set)?;
        // A property with neither accessor public is unbindable; skip it.
        let Some(ty) = property_type(methods, getter, setter) else {
            continue;
        };
        out.push(PropertyModel {
            name: property.name,
            ty,
            getter,
            setter,
        });
    }
    Ok(out)
}

/// A property's type: the getter's return, else the setter's last parameter.
fn property_type(
    methods: &[MethodModel],
    getter: Option<usize>,
    setter: Option<usize>,
) -> Option<ResolvedType> {
    if let Some(method) = getter.and_then(|index| methods.get(index)) {
        return Some(method.returns.clone());
    }
    setter
        .and_then(|index| methods.get(index))
        .and_then(|method| method.parameters.last())
        .map(|param| param.ty.clone())
}

/// Resolve a type's public methods and their signatures.
///
/// Split out of [`build_type`] to keep both inside the workspace's 60-line
/// ceiling.
fn build_methods(
    metadata: &Metadata,
    binary: &GameBinary,
    def: &modrix_il2cpp::TypeDef,
) -> Result<Vec<MethodModel>> {
    let mut methods = Vec::with_capacity(def.methods.count);
    for slot in def.methods.indices() {
        let method = metadata.method_def(slot)?;
        let mut parameters = Vec::with_capacity(method.parameters.count);
        for param_slot in method.parameters.indices() {
            let param = metadata.param_def(param_slot)?;
            parameters.push(ParamModel {
                name: param.name,
                ty: binary.resolve(usize::try_from(param.type_index).unwrap_or(0))?,
            });
        }
        let generic_params = match metadata.generic_container(method.generic_container)? {
            Some(container) => metadata.generic_param_names(&container)?,
            None => Vec::new(),
        };
        methods.push(MethodModel {
            returns: binary.resolve(usize::try_from(method.return_type).unwrap_or(0))?,
            is_static: method.is_static(),
            generic_params,
            name: method.name,
            flags: publicize_method(method.flags),
            parameters,
        });
    }
    Ok(methods)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn real_game() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        let home = std::env::var_os("HOME")?;
        let root = std::path::Path::new(&home)
            .join(".local/share/Steam/steamapps/common/Sons Of The Forest");
        let binary = root.join("GameAssembly.dll");
        let metadata = root.join("SonsOfTheForest_Data/il2cpp_data/Metadata/global-metadata.dat");
        (binary.exists() && metadata.exists()).then_some((binary, metadata))
    }

    #[test]
    fn builds_a_model_of_the_real_game() {
        let Some((binary_path, metadata_path)) = real_game() else {
            return; // Game not installed.
        };
        let metadata = Metadata::open(&metadata_path).unwrap();
        let binary = GameBinary::open(&binary_path, metadata.type_count().unwrap()).unwrap();
        let model = GameModel::build(&metadata, &binary).unwrap();

        assert!(
            model.assemblies.len() > 100,
            "a Unity game ships many assemblies, got {}",
            model.assemblies.len()
        );
        // Every type must be owned by exactly one assembly, or a signature
        // referencing it could not decide between TypeDef and TypeRef.
        assert_eq!(model.owner_of.len(), metadata.type_count().unwrap());

        let named: Vec<&str> = model.assemblies.iter().map(|a| a.name.as_str()).collect();
        for expected in ["Assembly-CSharp", "Sons", "UnityEngine.CoreModule"] {
            assert!(named.contains(&expected), "missing assembly {expected}");
        }
        assert!(
            model.type_count() > 5_000,
            "expected thousands of public types, got {}",
            model.type_count()
        );
    }

    #[test]
    fn the_model_exposes_a_known_public_api() {
        let Some((binary_path, metadata_path)) = real_game() else {
            return;
        };
        let metadata = Metadata::open(&metadata_path).unwrap();
        let binary = GameBinary::open(&binary_path, metadata.type_count().unwrap()).unwrap();
        let model = GameModel::build(&metadata, &binary).unwrap();

        // LocalPlayer is what every Sons of the Forest mod reaches for, so it
        // is the canary: if it is missing or its members are wrong, mods will
        // not compile against the generated reference assemblies.
        let local_player = model
            .assemblies
            .iter()
            .flat_map(|a| &a.types)
            .find(|t| t.namespace == "TheForest.Utils" && t.name == "LocalPlayer")
            .expect("LocalPlayer is a public type");

        assert!(
            local_player
                .methods
                .iter()
                .any(|m| m.name == "get_IsInWorld"),
            "IsInWorld is public API used by mods"
        );
        assert!(
            local_player.fields.iter().any(|f| f.name == "_transform"),
            "_transform is a public field"
        );
        // Private members ARE exported, deliberately. These assemblies are
        // publicized: mods Harmony-patch private methods and read private
        // fields, so a reference assembly limited to the public API is
        // useless to them. `_inventory` is private in the game and must still
        // be here.
        assert!(
            local_player.fields.iter().any(|f| f.name == "_inventory"),
            "publicized output must expose private fields too"
        );
        assert!(
            local_player.fields.iter().all(|f| f.flags & 0x07 == 6),
            "every exported field must be widened to public"
        );
    }
}

/// Widen a `TypeAttributes` visibility nibble to public.
///
/// Nested types must use `NestedPublic` (2) rather than `Public` (1); ECMA-335
/// treats the two as different encodings and a nested type carrying `Public`
/// is malformed.
pub(crate) fn publicize_type(flags: u32) -> u32 {
    let visibility = flags & 0x07;
    let widened = if visibility >= 2 { 2 } else { 1 };
    (flags & !0x07) | widened
}

/// Widen a `FieldAttributes` access nibble to `Public` (6).
fn publicize_field(flags: u32) -> u32 {
    (flags & !0x07) | 6
}

/// Widen a `MethodAttributes` access nibble to `Public` (6).
fn publicize_method(flags: u16) -> u16 {
    (flags & !0x07) | 6
}

#[cfg(test)]
mod publicize_tests {
    use super::*;

    #[test]
    fn type_visibility_widens_without_disturbing_other_flags() {
        // NotPublic -> Public, keeping Sealed(0x100) and Abstract(0x80).
        assert_eq!(publicize_type(0x180), 0x181);
        // Already Public stays Public.
        assert_eq!(publicize_type(0x001), 0x001);
        // Every nested form collapses to NestedPublic, never plain Public -
        // a nested type carrying Public is malformed metadata.
        for nested in 2_u32..=7 {
            assert_eq!(publicize_type(nested) & 0x07, 2, "nested {nested}");
        }
    }

    #[test]
    fn member_access_widens_to_public() {
        // Private field with Static(0x10) keeps Static.
        assert_eq!(publicize_field(0x11), 0x16);
        // Private method with Static|Virtual keeps both.
        assert_eq!(publicize_method(0x0051), 0x0056);
    }
}
