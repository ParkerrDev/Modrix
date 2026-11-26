// SPDX-License-Identifier: GPL-2.0-only
//! A test-only reader that parses this crate's own output back into rows.
//!
//! This exists because ECMA-335 metadata has essentially no redundancy: a
//! wrong column width or a dangling index produces a file that is *structurally
//! plausible* and silently wrong. Re-parsing the bytes with an independent
//! walker and then proving that every coded index lands on a present, in-range
//! row is the cheapest way to catch that class of bug without a .NET toolchain
//! in the loop.
//!
//! It deliberately shares only the row *structs* with the writer, never the
//! code that fills them in.

use crate::rows::{
    AssemblyRefRow, AssemblyRow, ConstantRow, CustomAttributeRow, FieldRow, GenericParamRow,
    InterfaceImplRow, MemberRefRow, MethodDefRow, MethodSemanticsRow, ModuleRow, NestedClassRow,
    ParamRow, PropertyMapRow, PropertyRow, TableSet, TypeDefRow, TypeRefRow, TypeSpecRow,
};
use crate::tables::{
    CUSTOM_ATTRIBUTE_TYPE, CodedIndex, EMITTED_TABLES, HAS_CONSTANT, HAS_CUSTOM_ATTRIBUTE,
    HAS_SEMANTICS, MEMBER_REF_PARENT, RESOLUTION_SCOPE, RowCounts, TYPE_DEF_OR_REF,
    TYPE_OR_METHOD_DEF, Widths, table,
};

/// Reader result; the message is a human-readable failure for a test to print.
pub(crate) type VResult<T> = core::result::Result<T, String>;

/// A byte cursor that never indexes and never panics.
struct Cursor<'a> {
    data: &'a [u8],
    position: usize,
}

impl<'a> Cursor<'a> {
    fn new(data: &'a [u8]) -> Self {
        Self { data, position: 0 }
    }

    fn seek(&mut self, position: usize) -> VResult<()> {
        if position > self.data.len() {
            return Err(format!(
                "seek to {position} past end of {} bytes",
                self.data.len()
            ));
        }
        self.position = position;
        Ok(())
    }

    fn take(&mut self, count: usize) -> VResult<&'a [u8]> {
        let end = self
            .position
            .checked_add(count)
            .ok_or_else(|| "cursor overflow".to_owned())?;
        let slice = self
            .data
            .get(self.position..end)
            .ok_or_else(|| format!("read of {count} bytes at {} ran past end", self.position))?;
        self.position = end;
        Ok(slice)
    }

    fn u8(&mut self) -> VResult<u8> {
        Ok(u8::from_le_bytes(read_array::<1>(self.take(1)?)?))
    }

    fn u16(&mut self) -> VResult<u16> {
        Ok(u16::from_le_bytes(read_array::<2>(self.take(2)?)?))
    }

    fn u32(&mut self) -> VResult<u32> {
        Ok(u32::from_le_bytes(read_array::<4>(self.take(4)?)?))
    }

    fn u64(&mut self) -> VResult<u64> {
        Ok(u64::from_le_bytes(read_array::<8>(self.take(8)?)?))
    }

    fn index(&mut self, width: u8) -> VResult<u32> {
        if width == 2 {
            Ok(u32::from(self.u16()?))
        } else {
            self.u32()
        }
    }

    fn skip(&mut self, count: usize) -> VResult<()> {
        self.take(count).map(|_| ())
    }
}

fn read_array<const N: usize>(slice: &[u8]) -> VResult<[u8; N]> {
    <[u8; N]>::try_from(slice).map_err(|_| format!("expected {N} bytes, found {}", slice.len()))
}

/// Everything the reader recovered from an emitted assembly.
pub(crate) struct Parsed {
    pub(crate) heap_sizes: u8,
    pub(crate) counts: RowCounts,
    pub(crate) tables: TableSet,
    pub(crate) strings: Vec<u8>,
    pub(crate) blobs: Vec<u8>,
    pub(crate) guids: Vec<u8>,
    pub(crate) version_length: u32,
    pub(crate) stream_names: Vec<String>,
}

impl Parsed {
    /// Resolves a `#Strings` offset.
    pub(crate) fn string(&self, offset: u32) -> VResult<String> {
        let start = usize::try_from(offset).map_err(|_| "string offset overflow".to_owned())?;
        let tail = self
            .strings
            .get(start..)
            .ok_or_else(|| format!("#Strings offset {offset} is past the heap"))?;
        let end = tail
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| format!("#Strings entry at {offset} is not NUL-terminated"))?;
        let bytes = tail
            .get(..end)
            .ok_or_else(|| "string slice vanished".to_owned())?;
        String::from_utf8(bytes.to_vec()).map_err(|error| format!("#Strings not UTF-8: {error}"))
    }

    /// Resolves a `#Blob` offset, skipping its compressed length prefix.
    pub(crate) fn blob(&self, offset: u32) -> VResult<Vec<u8>> {
        let start = usize::try_from(offset).map_err(|_| "blob offset overflow".to_owned())?;
        let tail = self
            .blobs
            .get(start..)
            .ok_or_else(|| format!("#Blob offset {offset} is past the heap"))?;
        let (length, header) = decompress(tail)?;
        let from = header;
        let to = from
            .checked_add(usize::try_from(length).map_err(|_| "blob length overflow".to_owned())?)
            .ok_or_else(|| "blob length overflow".to_owned())?;
        Ok(tail
            .get(from..to)
            .ok_or_else(|| format!("#Blob entry at {offset} runs past the heap"))?
            .to_vec())
    }

    /// The namespace and name of a 1-based `TypeDef` row.
    pub(crate) fn type_def_name(&self, row: u32) -> VResult<(String, String)> {
        let index =
            usize::try_from(row.saturating_sub(1)).map_err(|_| "type row overflow".to_owned())?;
        let entry = self
            .tables
            .type_defs
            .get(index)
            .ok_or_else(|| format!("TypeDef row {row} does not exist"))?;
        Ok((self.string(entry.namespace)?, self.string(entry.name)?))
    }

    /// How many GUIDs the `#GUID` heap holds.
    pub(crate) fn guid_count(&self) -> u32 {
        u32::try_from(self.guids.len().checked_div(16).unwrap_or(0)).unwrap_or(u32::MAX)
    }
}

/// Reads a compressed unsigned integer, returning `(value, bytes consumed)`.
fn decompress(data: &[u8]) -> VResult<(u32, usize)> {
    let first = *data
        .first()
        .ok_or_else(|| "compressed integer is empty".to_owned())?;
    if first & 0x80 == 0 {
        return Ok((u32::from(first), 1));
    }
    if first & 0x40 == 0 {
        let second = *data
            .get(1)
            .ok_or_else(|| "truncated 2-byte compressed integer".to_owned())?;
        return Ok(((u32::from(first & 0x3F) << 8) | u32::from(second), 2));
    }
    let bytes = data
        .get(..4)
        .ok_or_else(|| "truncated 4-byte compressed integer".to_owned())?;
    let mut value = u32::from(first & 0x1F);
    for byte in bytes.iter().skip(1) {
        value = (value << 8) | u32::from(*byte);
    }
    Ok((value, 4))
}

/// Parses a finished reference assembly.
pub(crate) fn parse(image: &[u8]) -> VResult<Parsed> {
    let metadata = locate_metadata(image)?;
    parse_metadata(&metadata)
}

/// Walks the PE container to the metadata root and returns just that blob.
fn locate_metadata(image: &[u8]) -> VResult<Vec<u8>> {
    let mut cursor = Cursor::new(image);
    if cursor.take(2)? != b"MZ" {
        return Err("missing MZ signature".to_owned());
    }
    cursor.seek(0x3C)?;
    let pe_offset = usize::try_from(cursor.u32()?).map_err(|_| "e_lfanew overflow".to_owned())?;
    cursor.seek(pe_offset)?;
    if cursor.take(4)? != b"PE\0\0" {
        return Err("missing PE signature".to_owned());
    }
    cursor.skip(2)?;
    let section_count = cursor.u16()?;
    cursor.skip(12)?;
    let optional_size = usize::from(cursor.u16()?);
    cursor.skip(2)?;
    let optional_start = cursor.position;
    if cursor.u16()? != 0x020B {
        return Err("optional header is not PE32+".to_owned());
    }
    // Data directory 14 is the CLI header; it sits 112 bytes into the
    // optional header, after the 24 standard and 88 Windows-specific fields.
    cursor.seek(
        optional_start
            .checked_add(112)
            .and_then(|base| base.checked_add(14usize.checked_mul(8)?))
            .ok_or_else(|| "optional header overflow".to_owned())?,
    )?;
    let cli_rva = cursor.u32()?;
    let sections = read_sections(&mut cursor, optional_start, optional_size, section_count)?;
    read_metadata_blob(image, &sections, cli_rva)
}

/// A `.text`-style section, reduced to what RVA translation needs.
struct Section {
    virtual_address: u32,
    virtual_size: u32,
    raw_size: u32,
    raw_pointer: u32,
}

fn read_sections(
    cursor: &mut Cursor<'_>,
    optional_start: usize,
    optional_size: usize,
    count: u16,
) -> VResult<Vec<Section>> {
    cursor.seek(
        optional_start
            .checked_add(optional_size)
            .ok_or_else(|| "section table overflow".to_owned())?,
    )?;
    let mut sections = Vec::new();
    for _ in 0..count {
        cursor.skip(8)?;
        let virtual_size = cursor.u32()?;
        let virtual_address = cursor.u32()?;
        let raw_size = cursor.u32()?;
        let raw_pointer = cursor.u32()?;
        cursor.skip(16)?;
        sections.push(Section {
            virtual_address,
            virtual_size,
            raw_size,
            raw_pointer,
        });
    }
    Ok(sections)
}

fn rva_to_offset(sections: &[Section], rva: u32) -> VResult<usize> {
    for section in sections {
        let span = section.virtual_size.max(section.raw_size);
        let end = section.virtual_address.saturating_add(span);
        if rva >= section.virtual_address && rva < end {
            let delta = rva.saturating_sub(section.virtual_address);
            return usize::try_from(section.raw_pointer.saturating_add(delta))
                .map_err(|_| "file offset overflow".to_owned());
        }
    }
    Err(format!("RVA 0x{rva:08X} is in no section"))
}

fn read_metadata_blob(image: &[u8], sections: &[Section], cli_rva: u32) -> VResult<Vec<u8>> {
    let mut cursor = Cursor::new(image);
    cursor.seek(rva_to_offset(sections, cli_rva)?)?;
    if cursor.u32()? != 72 {
        return Err("CLI header size is not 72".to_owned());
    }
    cursor.skip(4)?;
    let metadata_rva = cursor.u32()?;
    let metadata_size =
        usize::try_from(cursor.u32()?).map_err(|_| "metadata size overflow".to_owned())?;
    cursor.seek(rva_to_offset(sections, metadata_rva)?)?;
    Ok(cursor.take(metadata_size)?.to_vec())
}

fn parse_metadata(metadata: &[u8]) -> VResult<Parsed> {
    let mut cursor = Cursor::new(metadata);
    if cursor.u32()? != crate::metadata::SIGNATURE {
        return Err("metadata root is not BSJB".to_owned());
    }
    cursor.skip(4)?;
    cursor.skip(4)?;
    let version_length = cursor.u32()?;
    if !version_length.is_multiple_of(4) {
        return Err(format!(
            "metadata version length {version_length} is not a multiple of 4"
        ));
    }
    cursor.skip(usize::try_from(version_length).map_err(|_| "version overflow".to_owned())?)?;
    cursor.skip(2)?;
    let stream_count = cursor.u16()?;
    let mut streams = Vec::new();
    for _ in 0..stream_count {
        streams.push(read_stream_header(&mut cursor)?);
    }
    let mut parsed = assemble(metadata, &streams)?;
    parsed.version_length = version_length;
    Ok(parsed)
}

/// A parsed stream header: name plus the bytes it covers.
struct StreamHeader {
    name: String,
    offset: usize,
    size: usize,
}

fn read_stream_header(cursor: &mut Cursor<'_>) -> VResult<StreamHeader> {
    let offset = usize::try_from(cursor.u32()?).map_err(|_| "stream offset overflow".to_owned())?;
    let size = usize::try_from(cursor.u32()?).map_err(|_| "stream size overflow".to_owned())?;
    let mut name = Vec::new();
    loop {
        let byte = cursor.u8()?;
        if byte == 0 {
            break;
        }
        name.push(byte);
    }
    while !cursor.position.is_multiple_of(4) {
        cursor.skip(1)?;
    }
    Ok(StreamHeader {
        name: String::from_utf8(name).map_err(|error| format!("stream name: {error}"))?,
        offset,
        size,
    })
}

fn slice_stream(metadata: &[u8], header: &StreamHeader) -> VResult<Vec<u8>> {
    let end = header
        .offset
        .checked_add(header.size)
        .ok_or_else(|| "stream extent overflow".to_owned())?;
    Ok(metadata
        .get(header.offset..end)
        .ok_or_else(|| format!("stream `{}` runs past the metadata root", header.name))?
        .to_vec())
}

fn assemble(metadata: &[u8], streams: &[StreamHeader]) -> VResult<Parsed> {
    let named = |wanted: &str| -> VResult<Vec<u8>> {
        streams
            .iter()
            .find(|header| header.name == wanted)
            .ok_or_else(|| format!("stream `{wanted}` is missing"))
            .and_then(|header| slice_stream(metadata, header))
    };
    let tables_stream = named("#~")?;
    let strings = named("#Strings")?;
    let blobs = named("#Blob")?;
    let guids = named("#GUID")?;
    named("#US")?;
    let (heap_sizes, counts, tables) = parse_tables(&tables_stream)?;
    Ok(Parsed {
        heap_sizes,
        counts,
        tables,
        strings,
        blobs,
        guids,
        version_length: 0,
        stream_names: streams.iter().map(|header| header.name.clone()).collect(),
    })
}

fn parse_tables(stream: &[u8]) -> VResult<(u8, RowCounts, TableSet)> {
    let mut cursor = Cursor::new(stream);
    cursor.skip(4)?;
    let major = cursor.u8()?;
    let minor = cursor.u8()?;
    if (major, minor) != (2, 0) {
        return Err(format!("unexpected table schema version {major}.{minor}"));
    }
    let heap_sizes = cursor.u8()?;
    if cursor.u8()? != 1 {
        return Err("#~ reserved byte at offset 7 is not 1".to_owned());
    }
    let valid = cursor.u64()?;
    cursor.skip(8)?;
    let mut counts = RowCounts::default();
    for table_number in 0..64_u8 {
        if valid & (1_u64 << table_number) != 0 {
            counts.set(table_number, cursor.u32()?);
        }
    }
    for table_number in 0..64_u8 {
        if valid & (1_u64 << table_number) != 0 && !EMITTED_TABLES.contains(&table_number) {
            return Err(format!("unexpected table 0x{table_number:02X} present"));
        }
    }
    let widths = Widths::from_heap_sizes(heap_sizes, counts);
    let tables = read_all_tables(&mut cursor, counts, widths)?;
    Ok((heap_sizes, counts, tables))
}

fn rows_of(counts: RowCounts, table_number: u8) -> usize {
    usize::try_from(counts.get(table_number)).unwrap_or(0)
}

fn read_all_tables(
    cursor: &mut Cursor<'_>,
    counts: RowCounts,
    widths: Widths,
) -> VResult<TableSet> {
    let mut tables = TableSet::default();
    read_core_tables(cursor, counts, widths, &mut tables)?;
    read_member_tables(cursor, counts, widths, &mut tables)?;
    read_tail_tables(cursor, counts, widths, &mut tables)?;
    Ok(tables)
}

fn read_core_tables(
    cursor: &mut Cursor<'_>,
    counts: RowCounts,
    widths: Widths,
    tables: &mut TableSet,
) -> VResult<()> {
    for _ in 0..rows_of(counts, table::MODULE) {
        tables.module.push(ModuleRow {
            generation: cursor.u16()?,
            name: cursor.index(widths.string)?,
            mvid: cursor.index(widths.guid)?,
            enc_id: cursor.index(widths.guid)?,
            enc_base_id: cursor.index(widths.guid)?,
        });
    }
    let scope = widths.resolution_scope;
    for _ in 0..rows_of(counts, table::TYPE_REF) {
        tables.type_refs.push(TypeRefRow {
            resolution_scope: cursor.index(scope)?,
            name: cursor.index(widths.string)?,
            namespace: cursor.index(widths.string)?,
        });
    }
    let extends = widths.type_def_or_ref;
    let field = widths.field;
    let method = widths.method_def;
    for _ in 0..rows_of(counts, table::TYPE_DEF) {
        tables.type_defs.push(TypeDefRow {
            flags: cursor.u32()?,
            name: cursor.index(widths.string)?,
            namespace: cursor.index(widths.string)?,
            extends: cursor.index(extends)?,
            field_list: cursor.index(field)?,
            method_list: cursor.index(method)?,
        });
    }
    Ok(())
}

fn read_member_tables(
    cursor: &mut Cursor<'_>,
    counts: RowCounts,
    widths: Widths,
    tables: &mut TableSet,
) -> VResult<()> {
    for _ in 0..rows_of(counts, table::FIELD) {
        tables.fields.push(FieldRow {
            flags: cursor.u16()?,
            name: cursor.index(widths.string)?,
            signature: cursor.index(widths.blob)?,
        });
    }
    let param = widths.param;
    for _ in 0..rows_of(counts, table::METHOD_DEF) {
        tables.methods.push(MethodDefRow {
            rva: cursor.u32()?,
            impl_flags: cursor.u16()?,
            flags: cursor.u16()?,
            name: cursor.index(widths.string)?,
            signature: cursor.index(widths.blob)?,
            param_list: cursor.index(param)?,
        });
    }
    for _ in 0..rows_of(counts, table::PARAM) {
        tables.params.push(ParamRow {
            flags: cursor.u16()?,
            sequence: cursor.u16()?,
            name: cursor.index(widths.string)?,
        });
    }
    let class = widths.type_def;
    let interface = widths.type_def_or_ref;
    for _ in 0..rows_of(counts, table::INTERFACE_IMPL) {
        tables.interface_impls.push(InterfaceImplRow {
            class: cursor.index(class)?,
            interface: cursor.index(interface)?,
        });
    }
    let parent = widths.member_ref_parent;
    for _ in 0..rows_of(counts, table::MEMBER_REF) {
        tables.member_refs.push(MemberRefRow {
            class: cursor.index(parent)?,
            name: cursor.index(widths.string)?,
            signature: cursor.index(widths.blob)?,
        });
    }
    Ok(())
}

fn read_tail_tables(
    cursor: &mut Cursor<'_>,
    counts: RowCounts,
    widths: Widths,
    tables: &mut TableSet,
) -> VResult<()> {
    let constant_parent = widths.has_constant;
    for _ in 0..rows_of(counts, table::CONSTANT) {
        let kind = cursor.u8()?;
        cursor.skip(1)?;
        tables.constants.push(ConstantRow {
            kind,
            parent: cursor.index(constant_parent)?,
            value: cursor.index(widths.blob)?,
        });
    }
    let attribute_parent = widths.has_custom_attribute;
    let constructor = widths.custom_attribute_type;
    for _ in 0..rows_of(counts, table::CUSTOM_ATTRIBUTE) {
        tables.custom_attributes.push(CustomAttributeRow {
            parent: cursor.index(attribute_parent)?,
            constructor: cursor.index(constructor)?,
            value: cursor.index(widths.blob)?,
        });
    }
    read_property_tables(cursor, counts, widths, tables)?;
    for _ in 0..rows_of(counts, table::TYPE_SPEC) {
        tables.type_specs.push(TypeSpecRow {
            signature: cursor.index(widths.blob)?,
        });
    }
    read_identity_tables(cursor, counts, widths, tables)
}

fn read_property_tables(
    cursor: &mut Cursor<'_>,
    counts: RowCounts,
    widths: Widths,
    tables: &mut TableSet,
) -> VResult<()> {
    let type_def = widths.type_def;
    let property = widths.property;
    for _ in 0..rows_of(counts, table::PROPERTY_MAP) {
        tables.property_maps.push(PropertyMapRow {
            parent: cursor.index(type_def)?,
            property_list: cursor.index(property)?,
        });
    }
    for _ in 0..rows_of(counts, table::PROPERTY) {
        tables.properties.push(PropertyRow {
            flags: cursor.u16()?,
            name: cursor.index(widths.string)?,
            signature: cursor.index(widths.blob)?,
        });
    }
    let method = widths.method_def;
    let association = widths.has_semantics;
    for _ in 0..rows_of(counts, table::METHOD_SEMANTICS) {
        tables.method_semantics.push(MethodSemanticsRow {
            semantics: cursor.u16()?,
            method: cursor.index(method)?,
            association: cursor.index(association)?,
        });
    }
    Ok(())
}

fn read_identity_tables(
    cursor: &mut Cursor<'_>,
    counts: RowCounts,
    widths: Widths,
    tables: &mut TableSet,
) -> VResult<()> {
    for _ in 0..rows_of(counts, table::ASSEMBLY) {
        let hash_algorithm = cursor.u32()?;
        let version = read_version(cursor)?;
        tables.assembly.push(AssemblyRow {
            hash_algorithm,
            version,
            flags: cursor.u32()?,
            public_key: cursor.index(widths.blob)?,
            name: cursor.index(widths.string)?,
            culture: cursor.index(widths.string)?,
        });
    }
    for _ in 0..rows_of(counts, table::ASSEMBLY_REF) {
        let version = read_version(cursor)?;
        tables.assembly_refs.push(AssemblyRefRow {
            version,
            flags: cursor.u32()?,
            public_key_or_token: cursor.index(widths.blob)?,
            name: cursor.index(widths.string)?,
            culture: cursor.index(widths.string)?,
            hash_value: cursor.index(widths.blob)?,
        });
    }
    let type_def = widths.type_def;
    for _ in 0..rows_of(counts, table::NESTED_CLASS) {
        tables.nested_classes.push(NestedClassRow {
            nested: cursor.index(type_def)?,
            enclosing: cursor.index(type_def)?,
        });
    }
    let owner = widths.type_or_method_def;
    for _ in 0..rows_of(counts, table::GENERIC_PARAM) {
        tables.generic_params.push(GenericParamRow {
            number: cursor.u16()?,
            flags: cursor.u16()?,
            owner: cursor.index(owner)?,
            name: cursor.index(widths.string)?,
        });
    }
    Ok(())
}

fn read_version(cursor: &mut Cursor<'_>) -> VResult<[u16; 4]> {
    Ok([cursor.u16()?, cursor.u16()?, cursor.u16()?, cursor.u16()?])
}

impl Parsed {
    /// Proves that every reference in every row lands somewhere real.
    ///
    /// This is the check that would have caught a coded index computed with a
    /// `1 << 16` threshold, a heap index written at the wrong width, or a
    /// `ResolutionScope` pointing at an `AssemblyRef` row that was never
    /// emitted - the last of which does not produce a compiler diagnostic, it
    /// produces a compiler crash.
    pub(crate) fn check_references(&self) -> VResult<()> {
        self.check_module()?;
        self.check_type_refs()?;
        self.check_type_defs()?;
        self.check_member_runs()?;
        self.check_members()?;
        self.check_relations()?;
        self.check_identity()?;
        Ok(())
    }

    fn coded(&self, family: CodedIndex, value: u32, what: &str) -> VResult<()> {
        if value == 0 {
            return Ok(());
        }
        let (target, row) = family
            .decode(value)
            .ok_or_else(|| format!("{what}: coded index {value} has an undefined tag"))?;
        let rows = self.counts.get(target);
        if row == 0 || row > rows {
            return Err(format!(
                "{what}: {} index names row {row} of table 0x{target:02X}, which has {rows} rows",
                family.name
            ));
        }
        Ok(())
    }

    fn simple(&self, target: u8, row: u32, what: &str) -> VResult<()> {
        let rows = self.counts.get(target);
        if row == 0 || row > rows {
            return Err(format!(
                "{what}: index names row {row} of table 0x{target:02X}, which has {rows} rows"
            ));
        }
        Ok(())
    }

    fn check_module(&self) -> VResult<()> {
        let row = self
            .tables
            .module
            .first()
            .ok_or_else(|| "the Module table is empty".to_owned())?;
        self.string(row.name)?;
        if row.mvid == 0 || row.mvid > self.guid_count() {
            return Err(format!("Module.Mvid ordinal {} is out of range", row.mvid));
        }
        Ok(())
    }

    fn check_type_refs(&self) -> VResult<()> {
        for (index, row) in self.tables.type_refs.iter().enumerate() {
            let what = format!("TypeRef row {}", index.saturating_add(1));
            self.coded(RESOLUTION_SCOPE, row.resolution_scope, &what)?;
            self.string(row.name)?;
            self.string(row.namespace)?;
        }
        Ok(())
    }

    fn check_type_defs(&self) -> VResult<()> {
        for (index, row) in self.tables.type_defs.iter().enumerate() {
            let what = format!("TypeDef row {}", index.saturating_add(1));
            self.coded(TYPE_DEF_OR_REF, row.extends, &what)?;
            self.string(row.name)?;
            self.string(row.namespace)?;
        }
        Ok(())
    }

    /// The member-run invariant: starts must be non-decreasing and must stop
    /// at `rowcount + 1`, because a run ends where the next row's start begins.
    fn check_member_runs(&self) -> VResult<()> {
        let fields = self.counts.get(table::FIELD);
        let methods = self.counts.get(table::METHOD_DEF);
        check_run(
            self.tables.type_defs.iter().map(|row| row.field_list),
            fields,
            "TypeDef.FieldList",
        )?;
        check_run(
            self.tables.type_defs.iter().map(|row| row.method_list),
            methods,
            "TypeDef.MethodList",
        )?;
        check_run(
            self.tables.methods.iter().map(|row| row.param_list),
            self.counts.get(table::PARAM),
            "MethodDef.ParamList",
        )?;
        check_run(
            self.tables
                .property_maps
                .iter()
                .map(|row| row.property_list),
            self.counts.get(table::PROPERTY),
            "PropertyMap.PropertyList",
        )
    }

    fn check_members(&self) -> VResult<()> {
        for row in &self.tables.fields {
            self.string(row.name)?;
            self.blob(row.signature)?;
        }
        for row in &self.tables.methods {
            self.string(row.name)?;
            self.blob(row.signature)?;
            if row.rva != 0 {
                return Err("a reference assembly must have MethodDef.RVA == 0".to_owned());
            }
        }
        for row in &self.tables.params {
            self.string(row.name)?;
        }
        for row in &self.tables.properties {
            self.string(row.name)?;
            self.blob(row.signature)?;
        }
        for row in &self.tables.type_specs {
            self.blob(row.signature)?;
        }
        Ok(())
    }

    fn check_relations(&self) -> VResult<()> {
        for row in &self.tables.interface_impls {
            self.simple(table::TYPE_DEF, row.class, "InterfaceImpl.Class")?;
            self.coded(TYPE_DEF_OR_REF, row.interface, "InterfaceImpl.Interface")?;
        }
        for row in &self.tables.member_refs {
            self.coded(MEMBER_REF_PARENT, row.class, "MemberRef.Class")?;
            self.string(row.name)?;
            self.blob(row.signature)?;
        }
        for row in &self.tables.constants {
            self.coded(HAS_CONSTANT, row.parent, "Constant.Parent")?;
            self.blob(row.value)?;
        }
        for row in &self.tables.custom_attributes {
            self.coded(HAS_CUSTOM_ATTRIBUTE, row.parent, "CustomAttribute.Parent")?;
            self.coded(
                CUSTOM_ATTRIBUTE_TYPE,
                row.constructor,
                "CustomAttribute.Type",
            )?;
            self.blob(row.value)?;
        }
        for row in &self.tables.method_semantics {
            self.simple(table::METHOD_DEF, row.method, "MethodSemantics.Method")?;
            self.coded(
                HAS_SEMANTICS,
                row.association,
                "MethodSemantics.Association",
            )?;
        }
        for row in &self.tables.property_maps {
            self.simple(table::TYPE_DEF, row.parent, "PropertyMap.Parent")?;
        }
        Ok(())
    }

    fn check_identity(&self) -> VResult<()> {
        for row in &self.tables.assembly {
            self.string(row.name)?;
            self.string(row.culture)?;
            self.blob(row.public_key)?;
        }
        for row in &self.tables.assembly_refs {
            self.string(row.name)?;
            self.string(row.culture)?;
            self.blob(row.public_key_or_token)?;
            self.blob(row.hash_value)?;
        }
        for row in &self.tables.nested_classes {
            self.simple(table::TYPE_DEF, row.nested, "NestedClass.NestedClass")?;
            self.simple(table::TYPE_DEF, row.enclosing, "NestedClass.EnclosingClass")?;
        }
        for row in &self.tables.generic_params {
            self.coded(TYPE_OR_METHOD_DEF, row.owner, "GenericParam.Owner")?;
            self.string(row.name)?;
        }
        Ok(())
    }

    /// The identity of the core-library reference, if one is present.
    pub(crate) fn assembly_ref_names(&self) -> VResult<Vec<String>> {
        self.tables
            .assembly_refs
            .iter()
            .map(|row| self.string(row.name))
            .collect()
    }
}

fn check_run(starts: impl Iterator<Item = u32>, rows: u32, what: &'static str) -> VResult<()> {
    let limit = rows.saturating_add(1);
    let mut previous = 0_u32;
    for (index, start) in starts.enumerate() {
        if start < previous {
            return Err(format!(
                "{what}: row {} starts at {start}, before the previous run's {previous}",
                index.saturating_add(1)
            ));
        }
        if start == 0 || start > limit {
            return Err(format!(
                "{what}: row {} starts at {start}, outside 1..={limit}",
                index.saturating_add(1)
            ));
        }
        previous = start;
    }
    Ok(())
}
