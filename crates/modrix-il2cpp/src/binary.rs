// SPDX-License-Identifier: GPL-2.0-only
//! The IL2CPP type system, which lives in `GameAssembly.dll` rather than in
//! `global-metadata.dat`.
//!
//! This split is the single most awkward thing about reading IL2CPP. The
//! metadata file says "field 12 has type index 4007", but the *type table* it
//! indexes into is compiled into the native binary, because a type's runtime
//! representation depends on the target. So generating a managed signature for
//! anything at all requires both files.
//!
//! The table is reached through `Il2CppMetadataRegistration`, a struct of
//! count/pointer pairs that the binary does not export and does not describe.
//! We find it the way every IL2CPP tool does: by **anchoring on a value we
//! already know**. Two of its counts - `fieldOffsetsCount` and
//! `typeDefinitionsSizesCount` - are both exactly the number of type
//! definitions, which the metadata file told us, and they sit two 8-byte slots
//! apart. Scanning the data sections for that pair, then checking that every
//! pointer field lands inside the image, identifies the struct unambiguously
//! (verified: exactly one match in a 100 MB binary).

use crate::error::{Error, Result};
use crate::pe::PeImage;
use crate::reader::Reader;

/// Size of a pointer/count slot in the 64-bit registration struct.
const SLOT: usize = 8;
/// Slot index of `fieldOffsetsCount` within `Il2CppMetadataRegistration`.
const FIELD_OFFSETS_COUNT_SLOT: usize = 10;
/// Slot index of `typesCount`.
const TYPES_COUNT_SLOT: usize = 6;
/// Slot index of the `types` pointer.
const TYPES_PTR_SLOT: usize = 7;
/// Total slots we read from the struct.
const REGISTRATION_SLOTS: usize = 16;
/// Every pointer slot that must resolve inside the image for a candidate to be
/// believed. `metadataUsages` (slot 15) is legitimately null on v29, so it is
/// deliberately absent.
const POINTER_SLOTS: [usize; 7] = [1, 3, 5, 7, 9, 11, 13];
/// Upper bound on entries in the type table.
const MAX_TYPES: usize = 4_000_000;

/// `Il2CppTypeEnum` - the same discriminants as ECMA-335's `ELEMENT_TYPE_*`,
/// which is what makes translating a signature almost mechanical.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
#[non_exhaustive]
pub enum TypeKind {
    /// `void`
    Void = 0x01,
    /// `bool`
    Boolean = 0x02,
    /// `char`
    Char = 0x03,
    /// `sbyte`
    I1 = 0x04,
    /// `byte`
    U1 = 0x05,
    /// `short`
    I2 = 0x06,
    /// `ushort`
    U2 = 0x07,
    /// `int`
    I4 = 0x08,
    /// `uint`
    U4 = 0x09,
    /// `long`
    I8 = 0x0A,
    /// `ulong`
    U8 = 0x0B,
    /// `float`
    R4 = 0x0C,
    /// `double`
    R8 = 0x0D,
    /// `string`
    String = 0x0E,
    /// Unmanaged pointer.
    Ptr = 0x0F,
    /// Managed reference (`ref`/`out`).
    ByRef = 0x10,
    /// A struct.
    ValueType = 0x11,
    /// A reference type.
    Class = 0x12,
    /// A generic type parameter (`T` on a type).
    Var = 0x13,
    /// A multi-dimensional array.
    Array = 0x14,
    /// A constructed generic type.
    GenericInst = 0x15,
    /// `System.TypedReference`
    TypedByRef = 0x16,
    /// `nint`
    I = 0x18,
    /// `nuint`
    U = 0x19,
    /// A function pointer.
    FnPtr = 0x1B,
    /// `object`
    Object = 0x1C,
    /// A vector (single-dimension, zero-based array).
    SzArray = 0x1D,
    /// A generic method parameter (`T` on a method).
    MVar = 0x1E,
}

impl TypeKind {
    /// Map a raw discriminant, rejecting values this build does not model.
    #[must_use]
    pub const fn from_raw(raw: u8) -> Option<Self> {
        Some(match raw {
            0x01 => Self::Void,
            0x02 => Self::Boolean,
            0x03 => Self::Char,
            0x04 => Self::I1,
            0x05 => Self::U1,
            0x06 => Self::I2,
            0x07 => Self::U2,
            0x08 => Self::I4,
            0x09 => Self::U4,
            0x0A => Self::I8,
            0x0B => Self::U8,
            0x0C => Self::R4,
            0x0D => Self::R8,
            0x0E => Self::String,
            0x0F => Self::Ptr,
            0x10 => Self::ByRef,
            0x11 => Self::ValueType,
            0x12 => Self::Class,
            0x13 => Self::Var,
            0x14 => Self::Array,
            0x15 => Self::GenericInst,
            0x16 => Self::TypedByRef,
            0x18 => Self::I,
            0x19 => Self::U,
            0x1B => Self::FnPtr,
            0x1C => Self::Object,
            0x1D => Self::SzArray,
            0x1E => Self::MVar,
            _ => return None,
        })
    }

    /// Whether this kind's `data` field carries a type-definition index rather
    /// than a pointer to another structure.
    #[must_use]
    pub const fn data_is_type_index(self) -> bool {
        matches!(
            self,
            Self::Class
                | Self::ValueType
                | Self::Void
                | Self::Boolean
                | Self::Char
                | Self::I1
                | Self::U1
                | Self::I2
                | Self::U2
                | Self::I4
                | Self::U4
                | Self::I8
                | Self::U8
                | Self::R4
                | Self::R8
                | Self::String
                | Self::I
                | Self::U
                | Self::Object
                | Self::TypedByRef
        )
    }
}

/// One entry of the IL2CPP type table.
#[derive(Debug, Clone, Copy)]
pub struct Il2CppType {
    /// What kind of type this is.
    pub kind: TypeKind,
    /// Kind-dependent payload: a type-definition index for `Class`/`ValueType`
    /// and the primitives, otherwise an image pointer to a further structure
    /// (a generic instantiation, an array shape, the element type).
    pub data: u64,
    /// Type attributes (visibility, `static`, and friends).
    pub attrs: u16,
    /// Whether this is a by-reference type.
    pub by_ref: bool,
}

/// The located `Il2CppMetadataRegistration`.
#[derive(Debug, Clone, Copy)]
pub struct Registration {
    /// File offset of the struct itself.
    pub offset: usize,
    /// Number of entries in the type table.
    pub types_count: usize,
    /// File offset of the type table (an array of pointers to `Il2CppType`).
    pub types_offset: usize,
}

/// The native half of an IL2CPP game: `GameAssembly.dll`.
#[derive(Debug)]
pub struct GameBinary {
    raw: Vec<u8>,
    image: PeImage,
    registration: Registration,
    /// Offset -> table index, built on first use by [`GameBinary::type_index_of`].
    reverse_types: std::sync::OnceLock<std::collections::HashMap<usize, usize>>,
}

impl GameBinary {
    /// Read `GameAssembly.dll` and locate its type table.
    ///
    /// `type_definitions` is the type-definition count from the metadata file;
    /// it is the anchor the search keys on, so it must come from the *same*
    /// game build.
    ///
    /// # Errors
    ///
    /// [`Error::Io`] if unreadable, [`Error::NotPe`] if it is not a 64-bit PE,
    /// or [`Error::RegistrationNotFound`] if the anchor does not appear exactly
    /// once - which means the layout changed and guessing would be unsafe.
    pub fn open(path: &std::path::Path, type_definitions: usize) -> Result<Self> {
        let raw = std::fs::read(path).map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })?;
        Self::parse(raw, type_definitions)
    }

    /// Parse an in-memory image. See [`GameBinary::open`].
    ///
    /// # Errors
    ///
    /// As [`GameBinary::open`], minus the I/O case.
    pub fn parse(raw: Vec<u8>, type_definitions: usize) -> Result<Self> {
        let image = PeImage::parse(&raw)?;
        let registration = find_registration(&raw, &image, type_definitions)?;
        Ok(Self {
            raw,
            image,
            registration,
            reverse_types: std::sync::OnceLock::new(),
        })
    }

    /// The located registration struct.
    #[must_use]
    pub const fn registration(&self) -> Registration {
        self.registration
    }

    /// The PE image, for callers that need to resolve their own pointers.
    #[must_use]
    pub const fn image(&self) -> &PeImage {
        &self.image
    }

    /// Number of entries in the type table.
    #[must_use]
    pub const fn type_count(&self) -> usize {
        self.registration.types_count
    }

    /// File offset of an image pointer.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the pointer does not land inside the image.
    pub(crate) fn offset_of(&self, pointer: u64) -> Result<usize> {
        self.image.va_to_offset(pointer).ok_or(Error::Truncated {
            offset: 0,
            need: SLOT,
            have: self.raw.len(),
        })
    }

    /// The type-table index of an `Il2CppType` reached by pointer.
    ///
    /// Payload pointers name a type structure directly rather than by index,
    /// but the rest of the reader works in indices, so the pointer is mapped
    /// back through the table. The table is small enough (tens of thousands)
    /// that a linear scan would be wasteful per lookup, so the reverse map is
    /// built once, lazily.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the pointer is not a table entry.
    pub(crate) fn type_index_of(&self, pointer: u64) -> Result<usize> {
        let offset = self.offset_of(pointer)?;
        let index = self
            .reverse_types
            .get_or_init(|| self.build_reverse_map())
            .get(&offset)
            .copied();
        index.ok_or(Error::Truncated {
            offset,
            need: SLOT,
            have: self.raw.len(),
        })
    }

    /// Map every type structure's file offset back to its table index.
    fn build_reverse_map(&self) -> std::collections::HashMap<usize, usize> {
        let reader = Reader::new(&self.raw);
        let mut map = std::collections::HashMap::with_capacity(self.registration.types_count);
        for index in 0..self.registration.types_count {
            let slot = self
                .registration
                .types_offset
                .saturating_add(index.saturating_mul(SLOT));
            let Ok(va) = reader.u64_at(slot) else {
                continue;
            };
            if let Some(offset) = self.image.va_to_offset(va) {
                map.entry(offset).or_insert(index);
            }
        }
        map
    }

    /// Read a `u64` at a file offset.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] past the end of the file.
    pub(crate) fn read_u64(&self, offset: usize) -> Result<u64> {
        Reader::new(&self.raw).u64_at(offset)
    }

    /// Read a `u32` at a file offset.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] past the end of the file.
    pub(crate) fn read_u32(&self, offset: usize) -> Result<u32> {
        Reader::new(&self.raw).u32_at(offset)
    }

    /// Read a `u8` at a file offset.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] past the end of the file.
    pub(crate) fn read_u8(&self, offset: usize) -> Result<u8> {
        Reader::new(&self.raw).u8_at(offset)
    }

    /// Read type-table entry `index`.
    ///
    /// # Errors
    ///
    /// [`Error::Truncated`] if the entry lies outside the file, or
    /// [`Error::UnknownTypeKind`] for a discriminant this build does not model.
    pub fn type_at(&self, index: usize) -> Result<Il2CppType> {
        if index >= self.registration.types_count {
            return Err(Error::BoundExceeded {
                what: "type index",
                limit: self.registration.types_count,
            });
        }
        let reader = Reader::new(&self.raw);
        let slot = self
            .registration
            .types_offset
            .saturating_add(index.saturating_mul(SLOT));
        let entry_va = reader.u64_at(slot)?;
        let entry = self.image.va_to_offset(entry_va).ok_or(Error::Truncated {
            offset: slot,
            need: SLOT,
            have: self.raw.len(),
        })?;
        // Il2CppType is { void* data; uint32 bitfield; } - 16 bytes with
        // padding. The bitfield packs attrs:16, type:8, then flag bits.
        let data = reader.u64_at(entry)?;
        let bits = reader.u32_at(entry.saturating_add(SLOT))?;
        let raw_kind = u8::try_from((bits >> 16) & 0xFF).unwrap_or(0);
        let kind = TypeKind::from_raw(raw_kind).ok_or(Error::UnknownTypeKind { raw: raw_kind })?;
        Ok(Il2CppType {
            kind,
            data,
            attrs: u16::try_from(bits & 0xFFFF).unwrap_or(0),
            by_ref: (bits >> 29) & 1 == 1,
        })
    }
}

/// Scan the image's data sections for `Il2CppMetadataRegistration`.
fn find_registration(raw: &[u8], image: &PeImage, type_definitions: usize) -> Result<Registration> {
    let anchor = (type_definitions as u64).to_le_bytes();
    let reader = Reader::new(raw);
    let mut found: Option<Registration> = None;

    for section in image.data_sections() {
        let start = usize::try_from(section.raw_offset).unwrap_or(0);
        let len = usize::try_from(section.raw_size).unwrap_or(0);
        let Ok(body) = reader.slice(start, len) else {
            continue;
        };
        for hit in anchor_pairs(body, &anchor) {
            let at = start.saturating_add(hit);
            let Some(candidate) = validate_candidate(&reader, image, at) else {
                continue;
            };
            // A second match means the anchor is not discriminating on this
            // build; refusing beats picking one at random.
            if found.is_some() {
                return Err(Error::RegistrationNotFound {
                    detail: "the anchor matched more than once - layout changed".to_owned(),
                });
            }
            found = Some(candidate);
        }
    }
    found.ok_or(Error::RegistrationNotFound {
        detail: format!(
            "no Il2CppMetadataRegistration whose field/type-definition counts equal \
             {type_definitions}"
        ),
    })
}

/// Offsets where the anchor value appears twice, two slots apart - the
/// `fieldOffsetsCount` / `typeDefinitionsSizesCount` signature.
fn anchor_pairs<'a>(body: &'a [u8], anchor: &'a [u8; SLOT]) -> impl Iterator<Item = usize> + 'a {
    let gap = SLOT.saturating_mul(2);
    body.windows(SLOT)
        .enumerate()
        .filter(move |(index, window)| {
            if *window != anchor.as_slice() {
                return false;
            }
            let second = index.saturating_add(gap);
            body.get(second..second.saturating_add(SLOT)) == Some(anchor.as_slice())
        })
        .map(|(index, _)| index)
}

/// Check that a candidate hit really is the registration struct, by requiring
/// every pointer field to land inside the image.
fn validate_candidate(
    reader: &Reader<'_>,
    image: &PeImage,
    anchor_at: usize,
) -> Option<Registration> {
    let base = anchor_at.checked_sub(FIELD_OFFSETS_COUNT_SLOT.checked_mul(SLOT)?)?;
    let mut slots = [0_u64; REGISTRATION_SLOTS];
    for (index, slot) in slots.iter_mut().enumerate() {
        *slot = reader
            .u64_at(base.checked_add(index.checked_mul(SLOT)?)?)
            .ok()?;
    }
    if !POINTER_SLOTS.iter().all(|&i| {
        slots
            .get(i)
            .copied()
            .is_some_and(|p| image.is_image_pointer(p))
    }) {
        return None;
    }
    let types_count = usize::try_from(*slots.get(TYPES_COUNT_SLOT)?).ok()?;
    if types_count == 0 || types_count > MAX_TYPES {
        return None;
    }
    let types_offset = image.va_to_offset(*slots.get(TYPES_PTR_SLOT)?)?;
    Some(Registration {
        offset: base,
        types_count,
        types_offset,
    })
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
    fn type_kind_maps_known_discriminants_and_rejects_others() {
        assert_eq!(TypeKind::from_raw(0x08), Some(TypeKind::I4));
        assert_eq!(TypeKind::from_raw(0x1D), Some(TypeKind::SzArray));
        assert_eq!(TypeKind::from_raw(0x17), None, "0x17 is unassigned");
        assert_eq!(TypeKind::from_raw(0x00), None);
        assert!(TypeKind::Class.data_is_type_index());
        assert!(!TypeKind::GenericInst.data_is_type_index());
    }

    #[test]
    fn finds_the_registration_in_the_real_game_binary() {
        let Some((binary, metadata)) = real_game() else {
            return; // Game not installed.
        };
        let md = crate::Metadata::open(&metadata).expect("metadata parses");
        let type_definitions = md.type_count().unwrap();

        let game = GameBinary::open(&binary, type_definitions).expect("registration located");
        let reg = game.registration();
        assert!(
            reg.types_count > 10_000,
            "a Unity game has tens of thousands of types, got {}",
            reg.types_count
        );

        // Decode every type and confirm the table is coherent: no unknown
        // discriminants, and the class/valuetype payloads are in-range type
        // definition indices rather than garbage.
        let mut classes = 0_usize;
        for index in 0..reg.types_count {
            let ty = game.type_at(index).expect("every entry decodes");
            if matches!(ty.kind, TypeKind::Class | TypeKind::ValueType) {
                classes = classes.saturating_add(1);
                assert!(
                    usize::try_from(ty.data).unwrap_or(usize::MAX) < type_definitions,
                    "type {index} payload {} is not a valid type-definition index",
                    ty.data
                );
            }
        }
        assert!(
            classes > 1000,
            "expected many class/valuetype entries, got {classes}"
        );
    }

    #[test]
    fn refuses_a_binary_whose_anchor_does_not_appear() {
        let Some((binary, _)) = real_game() else {
            return;
        };
        let raw = std::fs::read(&binary).unwrap();
        // A count no build has: the search must fail rather than pick a
        // plausible-looking struct.
        assert!(matches!(
            GameBinary::parse(raw, 7),
            Err(Error::RegistrationNotFound { .. })
        ));
    }
}
