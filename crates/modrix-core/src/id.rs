// SPDX-License-Identifier: GPL-2.0-only
//! Strongly-typed row identifiers.
//!
//! Every table's primary key gets its own newtype over `i64` (SQLite's rowid)
//! so a `GameId` can never be passed where a `ModId` is expected. They are
//! `Copy` and cheap; conversion to/from the raw `i64` is explicit and
//! crate-internal at the database boundary.

/// Declare a transparent `i64` identifier newtype.
macro_rules! id_type {
    ($(#[$meta:meta])* $name:ident) => {
        $(#[$meta])*
        #[derive(
            Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash,
            serde::Serialize, serde::Deserialize,
        )]
        pub struct $name(i64);

        impl $name {
            /// The underlying database rowid.
            #[must_use]
            pub const fn get(self) -> i64 {
                self.0
            }

            /// Wrap a raw rowid returned by the database.
            pub(crate) const fn from_raw(raw: i64) -> Self {
                Self(raw)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                write!(f, "{}", self.0)
            }
        }
    };
}

id_type!(
    /// Identifies a row in `games`.
    GameId
);
id_type!(
    /// Identifies a row in `profiles`.
    ProfileId
);
id_type!(
    /// Identifies a row in `mods`.
    ModId
);
