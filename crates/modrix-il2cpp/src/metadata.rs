// SPDX-License-Identifier: GPL-2.0-only
//! Reading `global-metadata.dat`: the section table and the record decoders.

use std::path::Path;

use crate::defs::{
    self as size, Assembly, FieldDef, GenericContainer, MethodDef, ParamDef, PropertyDef, Run,
    TypeDef,
};
use crate::error::{Error, Result};
use crate::reader::Reader;

/// IL2CPP's magic number at the head of every metadata file.
pub const SANITY: u32 = 0xFAB1_1BAF;

/// Metadata versions this build understands.
pub const SUPPORTED_VERSIONS: &[i32] = &[29];

/// Ceiling on records in any one section, so a corrupt size cannot make us
/// try to allocate an enormous vector.
const MAX_RECORDS: usize = 4_000_000;

/// Positional indices of the sections we read, within the header's flat array
/// of `(offset, size)` pairs. The order is fixed by the metadata version.
mod section {
    /// The `#Strings`-equivalent blob of NUL-terminated names.
    pub const STRINGS: usize = 2;
    /// `Il2CppGenericParameter[]`.
    pub const GENERIC_PARAMS: usize = 12;
    /// `Il2CppGenericContainer[]`.
    pub const GENERIC_CONTAINERS: usize = 14;
    /// `Il2CppPropertyDefinition[]`.
    pub const PROPERTIES: usize = 4;
    /// `Il2CppMethodDefinition[]`.
    pub const METHODS: usize = 5;
    /// `Il2CppParameterDefinition[]`.
    pub const PARAMS: usize = 10;
    /// `Il2CppFieldDefaultValue[]` - field index, type index, blob offset.
    pub const FIELD_DEFAULTS: usize = 7;
    /// The shared blob holding field and parameter constant values.
    pub const DEFAULT_DATA: usize = 8;
    /// `Il2CppFieldDefinition[]`.
    pub const FIELDS: usize = 11;
    /// `int32[]` - each entry a binary type-table index for an implemented
    /// interface. Types slice it via their `interfaces` run.
    pub const INTERFACES: usize = 16;
    /// `Il2CppTypeDefinition[]`.
    pub const TYPE_DEFS: usize = 19;
    /// `Il2CppImageDefinition[]`.
    pub const IMAGES: usize = 20;
    /// `Il2CppAssemblyDefinition[]`.
    pub const ASSEMBLIES: usize = 21;
}

/// A (offset, byte-size) pair naming one section of the metadata file.
#[derive(Debug, Clone, Copy, Default)]
struct MetaSection {
    offset: usize,
    size: usize,
}

impl MetaSection {
    /// How many fixed-size records this section holds.
    fn records(self, name: &'static str, record: usize) -> Result<usize> {
        if record == 0 || !self.size.is_multiple_of(record) {
            return Err(Error::RaggedSection {
                section: name,
                size: self.size,
                record,
            });
        }
        let count = self.size.checked_div(record).unwrap_or(0);
        if count > MAX_RECORDS {
            return Err(Error::BoundExceeded {
                what: name,
                limit: MAX_RECORDS,
            });
        }
        Ok(count)
    }

    /// Byte offset of record `index`.
    fn record_at(self, index: usize, record: usize) -> usize {
        self.offset.saturating_add(index.saturating_mul(record))
    }
}

/// A parsed `global-metadata.dat`.
#[derive(Debug)]
pub struct Metadata {
    raw: Vec<u8>,
    version: i32,
    strings: MetaSection,
    methods: MetaSection,
    properties: MetaSection,
    generic_params: MetaSection,
    generic_containers: MetaSection,
    params: MetaSection,
    fields: MetaSection,
    field_defaults: MetaSection,
    default_data: MetaSection,
    interfaces: MetaSection,
    type_defs: MetaSection,
    images: MetaSection,
    assemblies: MetaSection,
}

impl Metadata {
    /// Read and validate a metadata file from disk.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if unreadable, [`Error::BadSanity`] if it is not IL2CPP
    /// metadata, [`Error::UnsupportedVersion`] for an unknown layout, or
    /// [`Error::RaggedSection`] if a section's size contradicts that layout.
    pub fn open(path: &Path) -> Result<Self> {
        let raw = std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(raw)
    }

    /// Validate and index an in-memory metadata blob.
    ///
    /// # Errors
    ///
    /// As [`Metadata::open`], minus the I/O case.
    pub fn parse(raw: Vec<u8>) -> Result<Self> {
        let reader = Reader::new(&raw);
        let found = reader.u32_at(0)?;
        if found != SANITY {
            return Err(Error::BadSanity {
                expected: SANITY,
                found,
            });
        }
        let version = reader.i32_at(4)?;
        if !SUPPORTED_VERSIONS.contains(&version) {
            return Err(Error::UnsupportedVersion {
                found: version,
                supported: SUPPORTED_VERSIONS,
            });
        }
        let parsed = Self {
            version,
            strings: read_section(&reader, section::STRINGS)?,
            methods: read_section(&reader, section::METHODS)?,
            properties: read_section(&reader, section::PROPERTIES)?,
            generic_params: read_section(&reader, section::GENERIC_PARAMS)?,
            generic_containers: read_section(&reader, section::GENERIC_CONTAINERS)?,
            params: read_section(&reader, section::PARAMS)?,
            fields: read_section(&reader, section::FIELDS)?,
            field_defaults: read_section(&reader, section::FIELD_DEFAULTS)?,
            default_data: read_section(&reader, section::DEFAULT_DATA)?,
            interfaces: read_section(&reader, section::INTERFACES)?,
            type_defs: read_section(&reader, section::TYPE_DEFS)?,
            images: read_section(&reader, section::IMAGES)?,
            assemblies: read_section(&reader, section::ASSEMBLIES)?,
            raw,
        };
        parsed.validate_member_runs()?;
        Ok(parsed)
    }

    /// The metadata schema version.
    #[must_use]
    pub const fn version(&self) -> i32 {
        self.version
    }

    fn reader(&self) -> Reader<'_> {
        Reader::new(&self.raw)
    }

    /// Read a name from the shared string blob by byte offset.
    fn string(&self, index: i32) -> Result<String> {
        let offset = self
            .strings
            .offset
            .saturating_add(usize::try_from(index).unwrap_or(0));
        self.reader().cstr_at(offset)
    }

    /// Number of type definitions.
    ///
    /// # Errors
    ///
    /// [`Error::RaggedSection`] if the section size contradicts the layout.
    pub fn type_count(&self) -> Result<usize> {
        self.type_defs.records("typeDefinitions", size::TYPE_DEF)
    }

    /// Number of field definitions.
    ///
    /// # Errors
    ///
    /// As [`Metadata::type_count`].
    pub fn field_count(&self) -> Result<usize> {
        self.fields.records("fields", size::FIELD)
    }

    /// Number of method definitions.
    ///
    /// # Errors
    ///
    /// As [`Metadata::type_count`].
    pub fn method_count(&self) -> Result<usize> {
        self.methods.records("methods", size::METHOD)
    }

    /// Number of parameter definitions.
    ///
    /// # Errors
    ///
    /// As [`Metadata::type_count`].
    pub fn param_count(&self) -> Result<usize> {
        self.params.records("parameters", size::PARAM)
    }

    /// Number of assemblies. Every assembly has exactly one image, so this
    /// equals the image count; a mismatch means the layout is wrong.
    ///
    /// # Errors
    ///
    /// As [`Metadata::type_count`].
    pub fn assembly_count(&self) -> Result<usize> {
        self.assemblies.records("assemblies", size::ASSEMBLY)
    }

    /// Number of images.
    ///
    /// # Errors
    ///
    /// As [`Metadata::type_count`].
    pub fn image_count(&self) -> Result<usize> {
        self.images.records("images", size::IMAGE)
    }

    /// Cross-check the record layouts against invariants that cannot hold by
    /// coincidence: every type's member counts must sum to exactly the shared
    /// tables' row counts, and every run must stay in bounds.
    ///
    /// This is what turns "these offsets looked right on one game" into a
    /// checked assumption. A build whose layout differs fails here rather than
    /// silently producing garbage signatures.
    ///
    /// # Errors
    ///
    /// [`Error::LayoutMismatch`] if a sum or a bound does not hold.
    pub fn validate_member_runs(&self) -> Result<()> {
        let (types, fields, methods) = (
            self.type_count()?,
            self.field_count()?,
            self.method_count()?,
        );
        let (mut field_sum, mut method_sum) = (0_usize, 0_usize);
        for index in 0..types {
            let def = self.type_def(index)?;
            field_sum = field_sum.saturating_add(def.fields.count);
            method_sum = method_sum.saturating_add(def.methods.count);
            check_bound("field", def.fields, fields)?;
            check_bound("method", def.methods, methods)?;
        }
        expect_sum("fields", field_sum, fields)?;
        expect_sum("methods", method_sum, methods)?;

        let mut param_sum = 0_usize;
        for index in 0..methods {
            param_sum = param_sum.saturating_add(self.method_def(index)?.parameters.count);
        }
        expect_sum("parameters", param_sum, self.param_count()?)
    }

    /// The binary type-table index of interface entry `index`.
    ///
    /// Types reach these through their `interfaces` run; the table is a flat
    /// array of 4-byte indices shared by every type.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the entry lies outside the file.
    pub fn interface_type(&self, index: usize) -> Result<i32> {
        self.reader().i32_at(self.interfaces.record_at(index, 4))
    }

    /// Number of entries in the shared interface table.
    ///
    /// # Errors
    ///
    /// As [`Metadata::type_count`].
    pub fn interface_count(&self) -> Result<usize> {
        self.interfaces.records("interfaces", 4)
    }

    /// Read the type definition at `index`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the record or one of its names lies outside the
    /// file.
    pub fn type_def(&self, index: usize) -> Result<TypeDef> {
        let base = self.type_defs.record_at(index, size::TYPE_DEF);
        let r = self.reader();
        let at = |offset: usize| base.saturating_add(offset);
        Ok(TypeDef {
            name: self.string(r.i32_at(base)?)?,
            namespace: self.string(r.i32_at(at(4))?)?,
            byval_type: r.i32_at(at(8))?,
            declaring_type: r.i32_at(at(12))?,
            parent_type: r.i32_at(at(16))?,
            generic_container: r.i32_at(at(24))?,
            flags: r.u32_at(at(28))?,
            fields: Run::new(r.i32_at(at(32))?, r.u16_at(at(68))?),
            methods: Run::new(r.i32_at(at(36))?, r.u16_at(at(64))?),
            properties: Run::new(r.i32_at(at(44))?, r.u16_at(at(66))?),
            nested_types: Run::new(r.i32_at(at(48))?, r.u16_at(at(72))?),
            interfaces: Run::new(r.i32_at(at(52))?, r.u16_at(at(76))?),
            token: r.u32_at(at(84))?,
        })
    }

    /// Read the field definition at `index`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the record lies outside the file.
    pub fn field_def(&self, index: usize) -> Result<FieldDef> {
        let base = self.fields.record_at(index, size::FIELD);
        let r = self.reader();
        Ok(FieldDef {
            name: self.string(r.i32_at(base)?)?,
            type_index: r.i32_at(base.saturating_add(4))?,
            token: r.u32_at(base.saturating_add(8))?,
        })
    }

    /// Read the method definition at `index`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the record lies outside the file.
    pub fn method_def(&self, index: usize) -> Result<MethodDef> {
        let base = self.methods.record_at(index, size::METHOD);
        let r = self.reader();
        let at = |offset: usize| base.saturating_add(offset);
        Ok(MethodDef {
            name: self.string(r.i32_at(base)?)?,
            declaring_type: r.i32_at(at(4))?,
            return_type: r.i32_at(at(8))?,
            parameters: Run::new(r.i32_at(at(12))?, r.u16_at(at(30))?),
            generic_container: r.i32_at(at(16))?,
            token: r.u32_at(at(20))?,
            flags: r.u16_at(at(24))?,
            impl_flags: r.u16_at(at(26))?,
            slot: r.u16_at(at(28))?,
        })
    }

    /// Read the generic container at `index`, when the index is valid.
    ///
    /// A negative index means "not generic", which every caller has to handle,
    /// so it is folded into `None` here rather than pushed onto them.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the record lies outside the file.
    pub fn generic_container(&self, index: i32) -> Result<Option<GenericContainer>> {
        let Ok(slot) = usize::try_from(index) else {
            return Ok(None);
        };
        if slot
            >= self
                .generic_containers
                .records("genericContainers", size::GENERIC)?
        {
            return Ok(None);
        }
        let base = self.generic_containers.record_at(slot, size::GENERIC);
        let r = self.reader();
        Ok(Some(GenericContainer {
            owner: r.i32_at(base)?,
            arg_count: r.i32_at(base.saturating_add(4))?,
            is_method: r.i32_at(base.saturating_add(8))? != 0,
            param_start: r.i32_at(base.saturating_add(12))?,
        }))
    }

    /// The declared names of a container's generic parameters (`T`, `TKey`).
    ///
    /// Using the real names rather than synthesised ones matters: they appear
    /// in compiler diagnostics and in IDE tooltips a mod author reads.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if a record lies outside the file.
    pub fn generic_param_names(&self, container: &GenericContainer) -> Result<Vec<String>> {
        let total = self
            .generic_params
            .records("genericParameters", size::GENERIC)?;
        let start = usize::try_from(container.param_start).unwrap_or(0);
        let count = usize::try_from(container.arg_count).unwrap_or(0).min(64);
        let mut names = Vec::with_capacity(count);
        for offset in 0..count {
            let slot = start.saturating_add(offset);
            if slot >= total {
                break;
            }
            let base = self.generic_params.record_at(slot, size::GENERIC);
            // Il2CppGenericParameter: ownerIndex, nameIndex, then constraints.
            names.push(self.string(self.reader().i32_at(base.saturating_add(4))?)?);
        }
        Ok(names)
    }

    /// Read the property definition at `index`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the record lies outside the file.
    pub fn property_def(&self, index: usize) -> Result<PropertyDef> {
        let base = self.properties.record_at(index, size::PROPERTY);
        let r = self.reader();
        let at = |offset: usize| base.saturating_add(offset);
        Ok(PropertyDef {
            name: self.string(r.i32_at(base)?)?,
            get: r.i32_at(at(4))?,
            set: r.i32_at(at(8))?,
            attrs: r.u32_at(at(12))?,
            token: r.u32_at(at(16))?,
        })
    }

    /// Number of property definitions.
    ///
    /// # Errors
    ///
    /// As [`Metadata::type_count`].
    pub fn property_count(&self) -> Result<usize> {
        self.properties.records("properties", size::PROPERTY)
    }

    /// Read the parameter definition at `index`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the record lies outside the file.
    pub fn param_def(&self, index: usize) -> Result<ParamDef> {
        let base = self.params.record_at(index, size::PARAM);
        let r = self.reader();
        Ok(ParamDef {
            name: self.string(r.i32_at(base)?)?,
            token: r.u32_at(base.saturating_add(4))?,
            type_index: r.i32_at(base.saturating_add(8))?,
        })
    }

    /// Read the assembly at `index`, including the type run its image owns.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the record or its image lies outside the file.
    pub fn assembly(&self, index: usize) -> Result<Assembly> {
        let base = self.assemblies.record_at(index, size::ASSEMBLY);
        let r = self.reader();
        let image_index = r.i32_at(base)?;
        // Il2CppAssemblyDefinition: imageIndex, token, referencedAssemblyStart,
        // referencedAssemblyCount, then an inline Il2CppAssemblyNameDefinition
        // whose name comes first and whose version quad sits at +24.
        let name_base = base.saturating_add(16);
        let part = |slot: usize| -> Result<u16> {
            r.u16_at(
                name_base
                    .saturating_add(24)
                    .saturating_add(slot.saturating_mul(4)),
            )
        };
        Ok(Assembly {
            name: self.string(r.i32_at(name_base)?)?,
            version: format!("{}.{}.{}.{}", part(0)?, part(1)?, part(2)?, part(3)?),
            types: self.image_types(image_index)?,
        })
    }

    /// The run of type definitions an image owns.
    fn image_types(&self, image_index: i32) -> Result<Run> {
        let index = usize::try_from(image_index).unwrap_or(0);
        let base = self.images.record_at(index, size::IMAGE);
        let r = self.reader();
        // Il2CppImageDefinition: nameIndex, assemblyIndex, typeStart, typeCount.
        let start = r.i32_at(base.saturating_add(8))?;
        let count = r.u32_at(base.saturating_add(12))?;
        Ok(Run {
            start: usize::try_from(start).ok().filter(|_| count > 0),
            count: usize::try_from(count).unwrap_or(0),
        })
    }

    /// Every assembly, in metadata order.
    ///
    /// # Errors
    ///
    /// As [`Metadata::assembly`].
    pub fn assemblies(&self) -> Result<Vec<Assembly>> {
        let count = self.assembly_count()?;
        (0..count).map(|index| self.assembly(index)).collect()
    }
}

/// Read one `(offset, size)` pair from the header's flat array.
fn read_section(reader: &Reader<'_>, index: usize) -> Result<MetaSection> {
    // The array starts after the 8-byte sanity + version prologue.
    let base = 8_usize.saturating_add(index.saturating_mul(8));
    Ok(MetaSection {
        offset: usize::try_from(reader.i32_at(base)?).unwrap_or(0),
        size: usize::try_from(reader.i32_at(base.saturating_add(4))?).unwrap_or(0),
    })
}

/// Fail when a per-type run reaches past its shared table.
fn check_bound(what: &'static str, run: Run, rows: usize) -> Result<()> {
    if run.end() > rows {
        return Err(Error::LayoutMismatch {
            what,
            expected: rows,
            found: run.end(),
        });
    }
    Ok(())
}

/// Fail when the per-type counts do not sum to the shared table's row count.
fn expect_sum(what: &'static str, sum: usize, rows: usize) -> Result<()> {
    if sum != rows {
        return Err(Error::LayoutMismatch {
            what,
            expected: rows,
            found: sum,
        });
    }
    Ok(())
}

/// A field's compile-time constant, as stored in the metadata's shared
/// default-value blob.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DefaultValue {
    /// Binary type-table index describing the constant's type.
    pub type_index: i32,
    /// The decoded value. Every constant IL2CPP stores for an enum member or a
    /// `const` field is an integer, so one signed slot covers all of them.
    pub value: i64,
}

/// Read an ECMA-style compressed unsigned integer, returning it and its width.
///
/// IL2CPP reuses the CLI blob compression: one byte under 0x80, two bytes under
/// 0xC0, four under 0xE0, and a 0xF0 escape carrying a raw 32-bit value.
fn read_compressed(reader: &Reader<'_>, offset: usize) -> Result<(u32, usize)> {
    let first = reader.u8_at(offset)?;
    let byte = |extra: usize| -> Result<u32> {
        Ok(u32::from(reader.u8_at(offset.saturating_add(extra))?))
    };
    if first & 0x80 == 0 {
        return Ok((u32::from(first), 1));
    }
    if first & 0xC0 == 0x80 {
        let high = u32::from(first & 0x3F) << 8;
        return Ok((high | byte(1)?, 2));
    }
    if first & 0xE0 == 0xC0 {
        let value = (u32::from(first & 0x1F) << 24) | (byte(1)? << 16) | (byte(2)? << 8) | byte(3)?;
        return Ok((value, 4));
    }
    if first == 0xF0 {
        let value = byte(1)? | (byte(2)? << 8) | (byte(3)? << 16) | (byte(4)? << 24);
        return Ok((value, 5));
    }
    // 0xFE / 0xFF are IL2CPP's sentinels for i32::MIN and "null"; neither can
    // appear as an enum member, so treat the byte as a literal small value.
    Ok((u32::from(first), 1))
}

/// Zig-zag decode: the encoding maps 0, -1, 1, -2 onto 0, 1, 2, 3, so a small
/// negative value still compresses to a single byte.
fn zigzag(encoded: u32) -> i64 {
    let magnitude = i64::from(encoded >> 1);
    if encoded & 1 == 1 {
        magnitude.saturating_add(1).saturating_neg()
    } else {
        magnitude
    }
}

impl Metadata {
    /// The compile-time constant of field `field_index`, when it has one.
    ///
    /// Only `const` fields and enum members carry one; everything else returns
    /// `None`. The lookup is a linear scan of the default-value table, which is
    /// why [`Metadata::default_values`] exists for bulk use.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if a record or the blob lies outside the file.
    pub fn field_default_value(&self, field_index: usize) -> Result<Option<DefaultValue>> {
        Ok(self.default_values()?.remove(&field_index))
    }

    /// Every field constant, keyed by field index.
    ///
    /// Enum members are the reason this matters: ECMA-335 requires a `Constant`
    /// row for every literal field, and a reference assembly that omits one
    /// makes the *consumer* fail with an opaque `CS7038` rather than a useful
    /// diagnostic.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if a record or the blob lies outside the file.
    pub fn default_values(&self) -> Result<std::collections::HashMap<usize, DefaultValue>> {
        let count = self.field_defaults.records("fieldDefaultValues", 12)?;
        let reader = self.reader();
        let mut out = std::collections::HashMap::with_capacity(count);
        for index in 0..count {
            let base = self.field_defaults.record_at(index, 12);
            let field = reader.i32_at(base)?;
            let type_index = reader.i32_at(base.saturating_add(4))?;
            let data = reader.i32_at(base.saturating_add(8))?;
            let (Ok(field), Ok(data)) = (usize::try_from(field), usize::try_from(data)) else {
                continue;
            };
            let at = self.default_data.offset.saturating_add(data);
            let (encoded, _) = read_compressed(&reader, at)?;
            out.insert(
                field,
                DefaultValue {
                    type_index,
                    value: zigzag(encoded),
                },
            );
        }
        Ok(out)
    }
}

#[cfg(test)]
mod default_value_tests {
    use super::*;

    #[test]
    fn zigzag_round_trips_the_documented_mapping() {
        assert_eq!(zigzag(0), 0);
        assert_eq!(zigzag(1), -1);
        assert_eq!(zigzag(2), 1);
        assert_eq!(zigzag(3), -2);
        // RainType { None, Light, Medium, Heavy } stores 0, 2, 4, 6.
        assert_eq!(zigzag(4), 2);
        assert_eq!(zigzag(6), 3);
    }

    #[test]
    fn compressed_reader_handles_every_width() {
        // One byte under 0x80.
        let one = [0x06_u8];
        assert_eq!(read_compressed(&Reader::new(&one), 0).unwrap(), (6, 1));
        // Two bytes: 0x80 | high bits, then the low byte. 0x8200 -> 0x200 = 512.
        let two = [0x82_u8, 0x00];
        assert_eq!(read_compressed(&Reader::new(&two), 0).unwrap(), (0x200, 2));
        // Four bytes with the 0xC0 tag.
        let four = [0xC0_u8, 0x00, 0x10, 0x00];
        assert_eq!(
            read_compressed(&Reader::new(&four), 0).unwrap(),
            (0x1000, 4)
        );
        // The 0xF0 escape carries a little-endian u32.
        let esc = [0xF0_u8, 0x78, 0x56, 0x34, 0x12];
        assert_eq!(
            read_compressed(&Reader::new(&esc), 0).unwrap(),
            (0x1234_5678, 5)
        );
    }

    #[test]
    fn refuses_to_read_a_compressed_value_past_the_end() {
        let truncated = [0x82_u8];
        assert!(read_compressed(&Reader::new(&truncated), 0).is_err());
    }
}
