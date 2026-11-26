// SPDX-License-Identifier: GPL-2.0-only
//! Generates managed reference assemblies from an IL2CPP game.
//!
//! This crate is the bridge between `modrix-il2cpp` (which reads a game's
//! `global-metadata.dat`) and `modrix-ecma335` (which writes .NET reference
//! assemblies). It owns the part in between: deciding which types belong to
//! which assembly and which of them are worth emitting at all.

mod corlib;
pub mod emit;
mod error;
pub mod model;

pub use emit::{emit_all, emit_assembly};
pub use error::{Error, Result};
pub use model::{AssemblyModel, FieldModel, GameModel, MethodModel, ParamModel, TypeModel};
