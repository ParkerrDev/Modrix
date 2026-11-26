// SPDX-License-Identifier: GPL-2.0-only

using System;
using System.Collections.Generic;
using System.IO;
using System.Reflection;
using System.Text;
using System.Text.Json;

namespace Modrix.Loader;

/// <summary>
/// Discovers mods in <c>&lt;game&gt;/Mods</c> and drives their lifecycle.
/// </summary>
/// <remarks>
/// <para>
/// The on-disk layout is split, and not obviously so: the payload assembly sits
/// <em>flat</em> at <c>Mods/&lt;Name&gt;.dll</c> while its metadata lives in a
/// folder beside it at <c>Mods/&lt;Name&gt;/manifest.json</c>. Enumeration is
/// non-recursive for exactly that reason - recursing would find the same mod
/// twice and load it twice.
/// </para>
/// <para>
/// Every rejection is logged with its reason. A mod that silently does not load
/// is the single most common complaint in this ecosystem, and almost all of it
/// traces to one of the four checks below rather than to anything mysterious.
/// </para>
/// </remarks>
internal static class ModHost
{
    /// <summary>Assemblies the loader supplies; a mod shipping its own copy is ignored.</summary>
    private static readonly string[] ReservedNames =
    {
        "Modrix.Loader",
        "Modrix.Sons.Sdk",
    };

    /// <summary>A discovered mod and the instance driving it.</summary>
    internal sealed class LoadedMod
    {
        internal LoadedMod(string id, string version, object instance, Type type)
        {
            Id = id;
            Version = version;
            Instance = instance;
            Type = type;
        }

        internal string Id { get; }
        internal string Version { get; }
        internal object Instance { get; }
        internal Type Type { get; }
    }

    /// <summary>
    /// Loads every mod under <paramref name="modsDirectory"/> and runs its
    /// initialization hook.
    /// </summary>
    /// <param name="modsDirectory">The game's <c>Mods</c> folder.</param>
    /// <param name="log">Where to report progress and rejections.</param>
    /// <returns>The mods that initialized successfully.</returns>
    internal static List<LoadedMod> LoadAll(string modsDirectory, Action<string> log)
    {
        var loaded = new List<LoadedMod>();
        if (!Directory.Exists(modsDirectory))
        {
            log($"no Mods directory at {modsDirectory}");
            return loaded;
        }

        // Non-recursive: the per-mod folder beside each DLL holds metadata, not
        // more assemblies.
        var candidates = Directory.GetFiles(modsDirectory, "*.dll", SearchOption.TopDirectoryOnly);
        Array.Sort(candidates, StringComparer.OrdinalIgnoreCase);
        log($"scanning {modsDirectory}: {candidates.Length} candidate assemblies");

        foreach (var path in candidates)
        {
            var name = Path.GetFileNameWithoutExtension(path);
            if (Array.IndexOf(ReservedNames, name) >= 0)
            {
                continue;
            }

            try
            {
                var mod = TryLoad(path, name, modsDirectory, log);
                if (mod is not null)
                {
                    loaded.Add(mod);
                }
            }
            catch (Exception error)
            {
                // One broken mod must never stop the others loading.
                log($"  {name}: FAILED - {Describe(error)}");
            }
        }

        log($"loaded {loaded.Count} of {candidates.Length} mods");
        return loaded;
    }

    /// <summary>Validate, load and construct a single mod.</summary>
    private static LoadedMod? TryLoad(
        string path,
        string name,
        string modsDirectory,
        Action<string> log)
    {
        var manifestPath = Path.Combine(modsDirectory, name, "manifest.json");
        if (!File.Exists(manifestPath))
        {
            log($"  {name}: skipped - no Mods/{name}/manifest.json");
            return null;
        }

        var manifest = ReadManifest(manifestPath, log);
        if (manifest is null)
        {
            return null;
        }
        if (!string.Equals(manifest.Value.Id, name, StringComparison.Ordinal))
        {
            log($"  {name}: skipped - manifest id '{manifest.Value.Id}' != assembly name");
            return null;
        }

        var assembly = Assembly.LoadFrom(path);
        var modType = FindModType(assembly);
        if (modType is null)
        {
            log($"  {name}: skipped - no SonsMod subclass");
            return null;
        }

        var instance = Activator.CreateInstance(modType);
        if (instance is null)
        {
            log($"  {name}: skipped - could not construct {modType.FullName}");
            return null;
        }

        // Patches go in BEFORE OnInitializeMod, so a mod's own initialization
        // already sees its patched world. This is also the only thing that makes
        // `HarmonyPatchAll = true` mean anything: the SDK exposes the flag but
        // nothing acted on it, so every [HarmonyPatch] class in every mod was
        // silently inert - the mod loaded, logged, and patched nothing.
        ApplyHarmonyPatches(assembly, name, instance, log);

        var mod = new LoadedMod(manifest.Value.Id, manifest.Value.Version, instance, modType);
        // A hook that throws does not un-load the mod. Its type is constructed
        // and its later hooks may still work, so it is reported and kept rather
        // than discarded - discarding it would also hide it from the mod list.
        try
        {
            Invoke(instance, "InvokeInitialize");
            log($"  {name} {manifest.Value.Version}: initialized ({modType.FullName})");
        }
        catch (Exception error)
        {
            log($"  {name} {manifest.Value.Version}: loaded, but OnInitializeMod threw - {Describe(error)}");
        }
        return mod;
    }

    /// <summary>
    /// Run <c>Harmony.PatchAll</c> for a mod that asked for it.
    /// </summary>
    /// <remarks>
    /// The flag lives behind an internal property on the SDK's base class, so it
    /// is read reflectively for the same reason the lifecycle shims are. A failing
    /// patch is reported and the mod is kept: a mod whose patches did not apply is
    /// still worth having loaded, and the alternative is a mod that vanishes with
    /// no explanation.
    /// </remarks>
    private static void ApplyHarmonyPatches(
        Assembly assembly,
        string name,
        object instance,
        Action<string> log)
    {
        var property = instance.GetType().GetProperty(
            "WantsHarmonyPatchAll",
            BindingFlags.Instance | BindingFlags.NonPublic | BindingFlags.Public
                | BindingFlags.FlattenHierarchy);
        if (property?.GetValue(instance) is not true)
        {
            return;
        }

        try
        {
            var harmony = new HarmonyLib.Harmony($"dev.modrix.mod.{name}");
            harmony.PatchAll(assembly);
            var patched = harmony.GetPatchedMethods();
            var count = 0;
            foreach (var _ in patched)
            {
                count++;
            }
            log($"  {name}: applied Harmony patches ({count} method(s))");
        }
        catch (Exception error)
        {
            log($"  {name}: Harmony PatchAll failed - {Describe(error)}");
        }
    }

    /// <summary>The mod's manifest fields the loader actually enforces.</summary>
    private readonly struct Manifest
    {
        internal Manifest(string id, string version)
        {
            Id = id;
            Version = version;
        }

        internal string Id { get; }
        internal string Version { get; }
    }

    /// <summary>
    /// Parse and validate <c>manifest.json</c>.
    /// </summary>
    /// <remarks>
    /// The version must parse as a .NET <see cref="Version"/>. A value like
    /// <c>1.0.0-beta.1</c> is valid against the published JSON schema but throws
    /// here, and a mod whose manifest throws is dropped with no other symptom -
    /// which is why this is checked explicitly and reported.
    /// </remarks>
    private static Manifest? ReadManifest(string path, Action<string> log)
    {
        using var stream = File.OpenRead(path);
        using var document = JsonDocument.Parse(stream);
        var root = document.RootElement;

        if (!root.TryGetProperty("id", out var idElement))
        {
            log($"  {Path.GetFileName(path)}: skipped - manifest has no 'id'");
            return null;
        }
        var id = idElement.GetString() ?? string.Empty;

        var version = root.TryGetProperty("version", out var versionElement)
            ? versionElement.GetString() ?? string.Empty
            : string.Empty;
        if (!Version.TryParse(version, out _))
        {
            log($"  {id}: skipped - version '{version}' is not a System.Version (use 1.0.0)");
            return null;
        }

        if (root.TryGetProperty("type", out var typeElement)
            && string.Equals(typeElement.GetString(), "Library", StringComparison.Ordinal))
        {
            log($"  {id}: skipped - type is Library, not Mod");
            return null;
        }

        return new Manifest(id, version);
    }

    /// <summary>The first concrete <c>SonsMod</c> subclass in the assembly.</summary>
    private static Type? FindModType(Assembly assembly)
    {
        foreach (var type in assembly.GetTypes())
        {
            if (type.IsAbstract || !type.IsClass)
            {
                continue;
            }
            for (var baseType = type.BaseType; baseType is not null; baseType = baseType.BaseType)
            {
                if (string.Equals(baseType.FullName, "SonsSdk.SonsMod", StringComparison.Ordinal))
                {
                    return type;
                }
            }
        }
        return null;
    }

    /// <summary>
    /// Render an exception usefully.
    /// </summary>
    /// <remarks>
    /// Reflection wraps everything a mod throws in
    /// <see cref="System.Reflection.TargetInvocationException"/>, whose own
    /// message is the useless "Exception has been thrown by the target of an
    /// invocation." Unwrapping is the difference between a report that names
    /// the missing method and one that says nothing at all.
    /// </remarks>
    internal static string Describe(Exception error)
    {
        var actual = error;
        var hops = 0;
        while (actual is TargetInvocationException wrapper
               && wrapper.InnerException is not null
               && hops < 8)
        {
            actual = wrapper.InnerException;
            hops++;
        }
        var origin = actual.TargetSite is null
            ? string.Empty
            : $" in {actual.TargetSite.DeclaringType?.Name}.{actual.TargetSite.Name}";

        // Everything below the outermost frame. Harmony in particular reports
        // "IL Compile Error (unknown location)" and puts the actual cause -
        // a missing method, an unresolvable type - in the inner chain, so
        // stopping at the outer message describes the symptom and hides the bug.
        var chain = new StringBuilder();
        var inner = actual.InnerException;
        for (var depth = 0; inner is not null && depth < 8; depth++)
        {
            chain.Append($" <- {inner.GetType().Name}: {inner.Message}");
            inner = inner.InnerException;
        }
        return $"{actual.GetType().Name}{origin}: {actual.Message}{chain}";
    }

    /// <summary>
    /// Call one of the SDK's internal lifecycle shims.
    /// </summary>
    /// <remarks>
    /// The lifecycle methods are <c>protected</c> for source compatibility with
    /// RedLoader, so the host cannot call them directly; the SDK exposes
    /// <c>internal</c> <c>Invoke*</c> shims instead. Those are internal to the
    /// SDK assembly, not to this one, so reflection is the only route.
    /// </remarks>
    internal static void Invoke(object instance, string shim)
    {
        ShimFor(instance.GetType(), shim)?.Invoke(instance, null);
    }

    /// <summary>Cache of resolved lifecycle shims, keyed by type and shim name.</summary>
    /// <remarks>
    /// <c>InvokeUpdate</c> runs for every mod on every frame. Resolving it by
    /// reflection each time would put a metadata lookup per mod per frame on the
    /// main thread, for a result that cannot change once the type is loaded.
    /// </remarks>
    private static readonly Dictionary<(Type, string), MethodInfo?> Shims = new();

    private static readonly object ShimGate = new();

    /// <summary>Resolve a lifecycle shim once, then remember it - including its absence.</summary>
    private static MethodInfo? ShimFor(Type type, string shim)
    {
        lock (ShimGate)
        {
            if (Shims.TryGetValue((type, shim), out var cached))
            {
                return cached;
            }
            var method = type.GetMethod(
                shim,
                BindingFlags.Instance | BindingFlags.NonPublic
                    | BindingFlags.Public | BindingFlags.FlattenHierarchy);
            Shims[(type, shim)] = method;
            return method;
        }
    }
}
