// SPDX-License-Identifier: GPL-2.0-only
//! Redirecting the game's base class library onto the real one.
//!
//! An IL2CPP game embeds its own corlib: `mscorlib`, `System`, `System.Core`
//! and friends appear in the metadata exactly like the game's own assemblies,
//! complete with `System.Object`, `System.String` and
//! `System.Collections.Generic.List<T>`.
//!
//! Emitting those verbatim breaks compilation in the worst way. A mod already
//! references the real .NET reference pack, so `System.Object` would exist
//! twice and every use becomes ambiguous - `CS0433`, on essentially every line.
//! The alternative that `Il2CppInterop` takes is to rename the whole tree to
//! `Il2CppSystem.*`, which avoids the clash but forces mod authors to write
//! `Il2CppSystem.Collections.Generic.List` instead of the type they mean.
//!
//! Modrix takes the third route: **redirect**. Game-corlib types are not
//! emitted at all; references to them are rewritten as `TypeRef` rows scoped to
//! the real `System.Runtime`, so `string` in a game signature binds to the
//! `string` the mod already has.
//!
//! That only works for types that genuinely exist in modern .NET with the same
//! name and arity, so redirection runs off an explicit allowlist. A type not on
//! it is **declined**, and the member using it is dropped. That direction is
//! deliberate: dropping a member costs a mod author one unavailable API, while
//! emitting a reference to a type the compiler cannot find fails the whole
//! build with an error that points nowhere useful.

/// Assemblies whose contents are the base class library rather than the game.
///
/// Matched case-insensitively against the metadata's assembly names. These are
/// never emitted; their types redirect.
const CORLIB_ASSEMBLIES: [&str; 10] = [
    "mscorlib",
    "System",
    "System.Core",
    "System.Runtime",
    "System.Xml",
    "System.Configuration",
    "netstandard",
    "System.Private.CoreLib",
    "System.Numerics",
    "System.Data",
];

/// Non-generic BCL types safe to redirect: present in .NET 6 under the same
/// namespace and name.
const REDIRECT_PLAIN: [&str; 46] = [
    "System.Object",
    "System.ValueType",
    "System.Enum",
    "System.Array",
    "System.Delegate",
    "System.MulticastDelegate",
    "System.Attribute",
    "System.Exception",
    "System.String",
    "System.Type",
    "System.Boolean",
    "System.Char",
    "System.SByte",
    "System.Byte",
    "System.Int16",
    "System.UInt16",
    "System.Int32",
    "System.UInt32",
    "System.Int64",
    "System.UInt64",
    "System.Single",
    "System.Double",
    "System.Decimal",
    "System.IntPtr",
    "System.UIntPtr",
    "System.Void",
    "System.DateTime",
    "System.TimeSpan",
    "System.Guid",
    "System.Uri",
    "System.Version",
    "System.Random",
    "System.EventArgs",
    "System.IDisposable",
    "System.IAsyncResult",
    "System.AsyncCallback",
    "System.IComparable",
    "System.IEquatable",
    "System.ICloneable",
    "System.IFormattable",
    "System.Collections.IEnumerable",
    "System.Collections.IEnumerator",
    "System.Collections.ICollection",
    "System.Collections.IList",
    "System.Collections.IDictionary",
    "System.Text.StringBuilder",
];

/// Generic BCL types safe to redirect, with their exact arity suffix.
const REDIRECT_GENERIC: [&str; 20] = [
    "System.Nullable`1",
    "System.Action`1",
    "System.Action`2",
    "System.Action`3",
    "System.Func`1",
    "System.Func`2",
    "System.Func`3",
    "System.Func`4",
    "System.IComparable`1",
    "System.IEquatable`1",
    "System.Collections.Generic.List`1",
    "System.Collections.Generic.Dictionary`2",
    "System.Collections.Generic.HashSet`1",
    "System.Collections.Generic.Queue`1",
    "System.Collections.Generic.Stack`1",
    "System.Collections.Generic.IEnumerable`1",
    "System.Collections.Generic.IEnumerator`1",
    "System.Collections.Generic.ICollection`1",
    "System.Collections.Generic.IList`1",
    "System.Collections.Generic.IDictionary`2",
];

/// Whether an assembly is part of the game's embedded base class library.
#[must_use]
pub fn is_corlib_assembly(name: &str) -> bool {
    CORLIB_ASSEMBLIES
        .iter()
        .any(|candidate| candidate.eq_ignore_ascii_case(name))
}

/// Whether a corlib type can be redirected onto the real `System.Runtime`.
///
/// `namespace` and `name` are the metadata's own strings, so `name` still
/// carries its `` `N `` arity suffix for generic types.
#[must_use]
pub fn is_redirectable(namespace: &str, name: &str) -> bool {
    // A nested type has no usable namespace of its own here, and its enclosing
    // type would need a separate scope; never redirect one.
    if name.contains('/') || namespace.is_empty() {
        return false;
    }
    let full = format!("{namespace}.{name}");
    if name.contains('`') {
        return REDIRECT_GENERIC.contains(&full.as_str());
    }
    REDIRECT_PLAIN.contains(&full.as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recognises_the_game_corlib_assemblies_case_insensitively() {
        assert!(is_corlib_assembly("mscorlib"));
        assert!(is_corlib_assembly("MSCORLIB"));
        assert!(is_corlib_assembly("System.Core"));
        assert!(!is_corlib_assembly("Assembly-CSharp"));
        assert!(!is_corlib_assembly("Sons"));
        assert!(
            !is_corlib_assembly("UnityEngine.CoreModule"),
            "Unity is the game's, not the BCL's"
        );
    }

    #[test]
    fn redirects_only_types_that_exist_in_modern_dotnet() {
        assert!(is_redirectable("System", "Object"));
        assert!(is_redirectable("System", "String"));
        assert!(is_redirectable("System.Collections.Generic", "List`1"));
        assert!(is_redirectable(
            "System.Collections.Generic",
            "Dictionary`2"
        ));

        // Arity must match exactly: List`2 is not List`1.
        assert!(!is_redirectable("System.Collections.Generic", "List`2"));
        // A type that may not exist, or may differ, is declined rather than
        // guessed - a dangling TypeRef fails the whole build.
        assert!(!is_redirectable("System", "__Canon"));
        assert!(!is_redirectable("System.Runtime.Remoting", "ObjRef"));
        // No namespace, or a nested name, is never redirectable.
        assert!(!is_redirectable("", "Object"));
        assert!(!is_redirectable("System", "Outer/Inner"));
    }
}
