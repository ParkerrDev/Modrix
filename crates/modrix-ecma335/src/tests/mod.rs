// SPDX-License-Identifier: GPL-2.0-only
//! Validation, layered from cheapest to most authoritative.
//!
//! * [`widths`] pins the coded-index threshold arithmetic directly. It is the
//!   only check that cannot be fooled by the writer and the reader sharing a
//!   wrong assumption.
//! * [`roundtrip`] (L0) re-parses emitted bytes and proves every reference
//!   resolves.
//! * [`scale`] (L1) drives the row counts across the 16 384-row `TypeDefOrRef`
//!   boundary and past a 64 KB `#Strings` heap, where a wrong width stops
//!   being theoretical.
//! * [`csc`] (L3) is the real oracle: it compiles C# against the emitted DLL
//!   with the actual Roslyn compiler from the .NET SDK.

mod csc;
mod fixtures;
mod roundtrip;
mod scale;
mod widths;
