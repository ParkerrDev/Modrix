// SPDX-License-Identifier: GPL-2.0-only

using System;
using System.Collections.Generic;
using System.IO;
using System.Runtime.CompilerServices;

using MonoMod.Utils;

namespace Modrix.Loader;

/// <summary>
/// Configures MonoMod's dynamic-method backend before Harmony is used.
/// </summary>
/// <remarks>
/// <para>
/// Harmony does not rewrite a method in place. It generates a replacement and
/// points the original at it, and the generation goes through MonoMod's
/// <c>DynamicMethodDefinition</c>, which has two backends:
/// </para>
/// <list type="bullet">
///   <item><description><b>DynamicMethod</b> - emits IL straight into the
///     running runtime via <c>System.Reflection.Emit</c>. Needs nothing from
///     disk.</description></item>
///   <item><description><b>Cecil</b> - builds a whole in-memory assembly with
///     Mono.Cecil and loads it. To do that, Cecil must <em>resolve</em> every
///     assembly the new one references, starting with
///     <c>System.Private.CoreLib</c>.</description></item>
/// </list>
/// <para>
/// The Cecil backend cannot work here. Cecil's default resolver searches the
/// process directory and the GAC; our runtime lives in <c>_Modrix/dotnet</c>,
/// which is on CoreCLR's <c>APP_PATHS</c> but is not a place Cecil looks. The
/// symptom is a <c>HarmonyException: IL Compile Error (unknown location)</c>
/// whose real cause, four frames down the inner chain, is
/// <c>AssemblyResolutionException: Failed to resolve assembly
/// 'System.Private.CoreLib'</c>. Nothing about that message points at the
/// backend choice, which is why this is worth a file of its own.
/// </para>
/// <para>
/// So the backend is pinned to <c>DynamicMethod</c> - but only after checking
/// that dynamic IL is actually available, because forcing a backend that cannot
/// run would trade a decipherable error for an opaque one.
/// </para>
/// </remarks>
internal static class DetourSetup
{
    /// <summary>
    /// MonoMod's environment variable selecting the DMD backend.
    /// </summary>
    /// <remarks>
    /// Set through the environment rather than through MonoMod's own
    /// <c>Switches</c> API, because that type exists only in MonoMod 25 and up
    /// while the pinned version here is 22.x. The variable is read by both.
    /// </remarks>
    private const string DmdTypeVariable = "MONOMOD_DMDTYPE";

    /// <summary>The backend that needs no assembly resolution.</summary>
    private const string DynamicMethodBackend = "DynamicMethod";

    /// <summary>
    /// Pin the DMD backend, reporting what was decided and why.
    /// </summary>
    /// <remarks>
    /// Must run before the first Harmony patch. It is called from interop
    /// startup rather than from <see cref="UnityPump"/>, because mods patch too
    /// and the first patch may well be a mod's, not ours.
    /// </remarks>
    /// <param name="log">Where to report the decision.</param>
    internal static void Configure(Action<string> log)
    {
        var dynamicCode = RuntimeFeature.IsDynamicCodeSupported;
        var dynamicIl = DynamicMethodDefinition.IsDynamicILAvailable;
        log($"monomod: dynamic code = {dynamicCode}, dynamic IL = {dynamicIl}");

        if (!dynamicIl)
        {
            // Cecil is the only remaining backend, and it will fail to resolve
            // corlib. Say so now, with the fix, instead of letting the first
            // patch produce "IL Compile Error (unknown location)".
            log("monomod: WARN dynamic IL unavailable - Harmony patches will use "
                + "the Cecil backend and are expected to fail to resolve "
                + "System.Private.CoreLib");
            return;
        }

        Environment.SetEnvironmentVariable(DmdTypeVariable, DynamicMethodBackend);
        log($"monomod: DMD backend pinned to {DynamicMethodBackend}");

        EnsureTrustedPlatformAssemblies(log);
    }

    /// <summary>The AppContext key Cecil reads to find the platform assemblies.</summary>
    private const string TrustedAssembliesKey = "TRUSTED_PLATFORM_ASSEMBLIES";

    /// <summary>
    /// Guarantee <c>TRUSTED_PLATFORM_ASSEMBLIES</c> has a value.
    /// </summary>
    /// <remarks>
    /// <para>
    /// Harmony's <c>DelegateTypeFactory</c> builds the delegate type for an
    /// IL2CPP detour with Cecil, unconditionally - the DMD backend choice above
    /// does not affect it. Writing that type means writing an
    /// <c>UnmanagedFunctionPointer</c> attribute whose argument is a
    /// <c>CallingConvention</c> enum, and Cecil must resolve the enum to learn
    /// its underlying type. It looks for platform assemblies in exactly one
    /// place: <c>AppContext.GetData("TRUSTED_PLATFORM_ASSEMBLIES")</c>.
    /// </para>
    /// <para>
    /// A normal .NET application gets that from the host. We are not a normal
    /// application - the proxy calls <c>coreclr_initialize</c> directly, and it
    /// is the proxy's job to pass it. It does. This is the belt to that
    /// suspenders: the value is checked, and reconstructed here if absent, so
    /// that an older proxy paired with a newer managed loader degrades into a
    /// log line rather than into "IL Compile Error (unknown location)" on every
    /// Harmony patch in the process.
    /// </para>
    /// </remarks>
    private static void EnsureTrustedPlatformAssemblies(Action<string> log)
    {
        var existing = AppContext.GetData(TrustedAssembliesKey) as string;
        if (!string.IsNullOrEmpty(existing))
        {
            var count = existing!.Split(new[] { ';', Path.PathSeparator }).Length;
            log($"monomod: {TrustedAssembliesKey} supplied by host ({count} assemblies)");
            return;
        }

        // Reconstruct from where the runtime and this assembly actually live,
        // rather than from the environment block, so it is right even if the
        // proxy published nothing at all.
        var runtimeDir = Path.GetDirectoryName(typeof(object).Assembly.Location);
        var hostDir = AppContext.BaseDirectory;
        var loaderDir = Path.GetDirectoryName(typeof(DetourSetup).Assembly.Location);

        var assemblies = new List<string>();
        foreach (var dir in new[] { runtimeDir, hostDir, loaderDir })
        {
            if (string.IsNullOrWhiteSpace(dir) || !Directory.Exists(dir))
            {
                continue;
            }
            foreach (var dll in Directory.GetFiles(dir!, "*.dll"))
            {
                if (!assemblies.Contains(dll))
                {
                    assemblies.Add(dll);
                }
            }
        }

        if (assemblies.Count == 0)
        {
            log($"monomod: WARN {TrustedAssembliesKey} is unset and could not be "
                + "reconstructed - Harmony patches of IL2CPP methods will fail");
            return;
        }

        // AppDomain.SetData forwards to the AppContext data store that
        // AppContext.GetData - and therefore Cecil - reads. AppContext.SetData
        // itself is not on net6.0's reference assembly.
        AppDomain.CurrentDomain.SetData(TrustedAssembliesKey, string.Join(';', assemblies));
        log($"monomod: {TrustedAssembliesKey} reconstructed ({assemblies.Count} assemblies)");
    }
}
