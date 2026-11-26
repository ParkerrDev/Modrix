// SPDX-License-Identifier: GPL-2.0-only
//! Errors raised while building a .NET reference assembly.

/// Result alias for this crate.
pub type Result<T> = core::result::Result<T, Error>;

/// Why an assembly could not be emitted.
///
/// Every variant is a *refusal to emit a corrupt file*. ECMA-335 metadata has
/// almost no redundancy, so a writer that silently truncated an index or wrote
/// a dangling row would produce a DLL that a compiler either mis-reads or
/// crashes on (a dangling `TypeRef.ResolutionScope` takes Roslyn down with an
/// `ArgumentOutOfRangeException`). Failing loudly here is the only way the
/// caller ever learns.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// A value exceeded the range of ECMA-335's compressed-integer encoding.
    #[error("value {value} exceeds the compressed-integer maximum of 0x1FFFFFFF")]
    CompressedRange {
        /// The value that could not be encoded.
        value: u32,
    },

    /// A signed value exceeded the compressed-signed-integer encoding range.
    #[error("value {value} is outside the compressed signed range -2^28..2^28-1")]
    CompressedSignedRange {
        /// The value that could not be encoded.
        value: i32,
    },

    /// A type signature nested deeper than this writer will encode. The
    /// encoder walks the type tree recursively, so an unbounded depth would be
    /// unbounded stack use on caller-supplied input.
    #[error("type signature nests deeper than the supported {limit} levels")]
    TypeTooDeep {
        /// The depth ceiling.
        limit: u32,
    },

    /// A metadata table grew past what this writer will emit. Row indices are
    /// 32-bit and heap offsets must stay addressable, so the ceiling is well
    /// below `u32::MAX`.
    #[error("the {table} table would hold {rows} rows, past the ceiling of {limit}")]
    TableOverflow {
        /// Human-readable table name.
        table: &'static str,
        /// The row count that was refused.
        rows: usize,
        /// The ceiling.
        limit: usize,
    },

    /// A metadata heap grew past what this writer will emit.
    #[error("the {heap} heap would grow to {size} bytes, past the ceiling of {limit}")]
    HeapOverflow {
        /// Heap name, for example `#Strings`.
        heap: &'static str,
        /// The size that was refused.
        size: usize,
        /// The ceiling.
        limit: usize,
    },

    /// A string destined for `#Strings` contained an interior NUL, which would
    /// silently truncate it because the heap is NUL-terminated.
    #[error("the name `{name}` contains a NUL byte and cannot go in #Strings")]
    NulInString {
        /// The offending string, with the NUL rendered as an escape.
        name: String,
    },

    /// A handle referred to a row that does not exist. This is the check that
    /// stops rule 8 (every coded index must resolve) from being violated: a
    /// handle minted by one builder and used in another would otherwise write
    /// a dangling index.
    #[error("{kind} handle #{index} does not name a row of this assembly ({rows} present)")]
    DanglingHandle {
        /// Which handle family, for example `TypeDef`.
        kind: &'static str,
        /// The zero-based handle value.
        index: u32,
        /// How many rows that table actually has.
        rows: u32,
    },

    /// A coded index could not be formed because the row number does not fit
    /// once the tag bits are shifted in.
    #[error("row {row} of table 0x{table:02X} does not fit the {family} coded index")]
    CodedIndexOverflow {
        /// The coded-index family name.
        family: &'static str,
        /// The target table number.
        table: u8,
        /// The 1-based row.
        row: u32,
    },

    /// A nested type carried a namespace. ECMA-335 requires a nested type's
    /// `Namespace` to be the empty string - the namespace comes from the
    /// enclosing type - and a non-empty one makes the type unresolvable under
    /// its apparent name.
    #[error("nested type `{name}` must have an empty namespace")]
    NestedTypeHasNamespace {
        /// The nested type's name.
        name: String,
    },

    /// A `literal` field had no compile-time value. The reference still loads
    /// and the member still resolves, so the failure surfaces only in the
    /// *consuming* compiler as a bare `CS7038` with no useful location.
    #[error("literal field `{name}` has no Constant row; consumers fail with CS7038")]
    LiteralWithoutConstant {
        /// The field's name.
        name: String,
    },

    /// A two-byte metadata column was handed a value larger than `u16::MAX`.
    /// The only way that happens is a column width computed before the row
    /// counts and heap sizes were final, so it is reported rather than
    /// truncated into a silently corrupt file.
    #[error("value {value} does not fit the 2-byte metadata column it was written to")]
    IndexTooWide {
        /// The value that did not fit.
        value: u32,
    },

    /// The image layout arithmetic overflowed, which means the requested
    /// assembly is far larger than a PE file can describe.
    #[error("image layout overflowed while computing {what}")]
    LayoutOverflow {
        /// Which quantity overflowed.
        what: &'static str,
    },
}
