// SPDX-License-Identifier: GPL-2.0-only
//! Errors from reading IL2CPP metadata.

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, Error>;

/// Why a metadata file could not be read.
#[derive(Debug, thiserror::Error)]
#[non_exhaustive]
pub enum Error {
    /// The file does not begin with IL2CPP's magic number, so it is not
    /// `global-metadata.dat` at all (a common symptom of a game update
    /// leaving a partial download, or of pointing at the wrong file).
    #[error("not IL2CPP metadata: expected sanity 0x{expected:08X}, found 0x{found:08X}")]
    BadSanity {
        /// The magic number IL2CPP writes.
        expected: u32,
        /// What was actually there.
        found: u32,
    },

    /// The metadata version is one this build does not know how to read.
    /// Record layouts change between versions with no in-band description, so
    /// guessing would silently produce nonsense rather than fail.
    #[error("unsupported IL2CPP metadata version {found} (this build reads {supported:?})")]
    UnsupportedVersion {
        /// The version in the file.
        found: i32,
        /// The versions this build understands.
        supported: &'static [i32],
    },

    /// A read ran past the end of the file.
    #[error("metadata truncated: needed {need} bytes at offset {offset}, file is {have} bytes")]
    Truncated {
        /// Where the read started.
        offset: usize,
        /// How many bytes it wanted.
        need: usize,
        /// How large the file actually is.
        have: usize,
    },

    /// A string in the string table has no NUL terminator.
    #[error("metadata string at offset {offset} is not NUL-terminated")]
    Unterminated {
        /// Where the string started.
        offset: usize,
    },

    /// A section's byte size is not a whole number of its fixed-size records,
    /// which means the layout assumption is wrong for this file.
    #[error(
        "metadata section `{section}` is {size} bytes, not a multiple of its {record}-byte record"
    )]
    RaggedSection {
        /// The section name.
        section: &'static str,
        /// Its declared byte size.
        size: usize,
        /// The expected record size.
        record: usize,
    },

    /// A count exceeded this build's fixed ceiling.
    #[error("{what} exceeded its bound of {limit}")]
    BoundExceeded {
        /// What was being counted.
        what: &'static str,
        /// The ceiling.
        limit: usize,
    },

    /// The file is not a 64-bit PE image, so it cannot be `GameAssembly.dll`.
    #[error("not a 64-bit PE image: {detail}")]
    NotPe {
        /// What was wrong.
        detail: String,
    },

    /// `Il2CppMetadataRegistration` could not be located in the binary. Its
    /// layout is not described anywhere in the file, so it is found by
    /// anchoring on counts the metadata already told us; failing that search
    /// means the layout changed and guessing would produce silent nonsense.
    #[error("could not locate the IL2CPP type table: {detail}")]
    RegistrationNotFound {
        /// What the search saw.
        detail: String,
    },

    /// A type-table entry carries a discriminant this build does not model.
    #[error("unknown IL2CPP type kind 0x{raw:02X}")]
    UnknownTypeKind {
        /// The raw discriminant.
        raw: u8,
    },

    /// A cross-check on the record layout failed: per-type member counts did
    /// not sum to the shared table's row count, or a run reached past its
    /// table. The offsets this build assumes are wrong for this game.
    #[error("metadata layout mismatch: {what} expected {expected}, found {found}")]
    LayoutMismatch {
        /// Which table disagreed.
        what: &'static str,
        /// What the table's own row count says.
        expected: usize,
        /// What summing the per-type runs produced.
        found: usize,
    },

    /// The file could not be read from disk.
    #[error("could not read `{path}`: {source}")]
    Io {
        /// The path.
        path: std::path::PathBuf,
        /// The underlying error.
        source: std::io::Error,
    },
}
