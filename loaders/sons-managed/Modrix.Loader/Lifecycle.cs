// SPDX-License-Identifier: GPL-2.0-only

using System;
using System.Collections.Generic;

using SonsSdk;

namespace Modrix.Loader;

/// <summary>
/// Drives the mod lifecycle across the two threads it actually spans.
/// </summary>
/// <remarks>
/// <para>
/// The split matters and is not arbitrary:
/// </para>
/// <list type="bullet">
///   <item><description><b>Bootstrap thread</b> - discovery, assembly loading,
///     construction, <c>OnInitializeMod</c>. None of this touches Unity, and
///     doing it here keeps the game's own startup unblocked.</description></item>
///   <item><description><b>Unity main thread</b> - <c>OnSdkInitialized</c>,
///     <c>OnGameStart</c>, every per-frame update, and shutdown. Anything that
///     can reach the scene graph belongs here, and running it anywhere else is
///     the single most common way to corrupt an IL2CPP
///     process.</description></item>
/// </list>
/// <para>
/// A mod that throws is reported and kept. Its remaining hooks still run,
/// because a mod that fails one hook is far more useful than a mod that silently
/// disappears - and the per-frame path has its own fault budget in
/// <see cref="UnityPump"/> so a mod cannot fail 60 times a second forever.
/// </para>
/// </remarks>
internal static class Lifecycle
{
    /// <summary>Attribution for pump faults raised by mod update callbacks.</summary>
    private const string PumpName = "mods";

    /// <summary>
    /// Hand the loaded mods over to the Unity main thread and start ticking them.
    /// </summary>
    /// <remarks>
    /// Called from the bootstrap thread. The main-thread work is queued rather
    /// than run, because at this point the first Unity frame has not happened
    /// yet - <see cref="UnityPump.Post"/> runs it at the start of the next one.
    /// </remarks>
    /// <param name="mods">The mods that loaded and constructed.</param>
    /// <param name="log">Progress and fault sink.</param>
    internal static void Attach(List<ModHost.LoadedMod> mods, Action<string> log)
    {
        UnityPump.Post(() => SdkInitialized(mods, log));
        UnityPump.OnFrame(PumpName, () => Tick(mods));
        log($"lifecycle attached: {mods.Count} mods will tick each frame");
    }

    /// <summary>
    /// Fire <c>OnSdkInitialized</c>, on the main thread, exactly once.
    /// </summary>
    /// <remarks>
    /// This is the hook a mod uses for anything touching game state, so it is
    /// deliberately the first thing that runs on the main thread - before any
    /// update tick, so a mod's update can assume its own setup already ran.
    /// </remarks>
    private static void SdkInitialized(List<ModHost.LoadedMod> mods, Action<string> log)
    {
        foreach (var mod in mods)
        {
            try
            {
                ModHost.Invoke(mod.Instance, "InvokeSdkInitialized");
            }
            catch (Exception error)
            {
                log($"  {mod.Id}: OnSdkInitialized threw - {ModHost.Describe(error)}");
            }
        }

        // The SDK's own event, for code that subscribes rather than subclasses.
        try
        {
            SdkEvents.OnSdkInitialized.Invoke();
        }
        catch (Exception error)
        {
            log($"  SdkEvents.OnSdkInitialized threw - {ModHost.Describe(error)}");
        }
        log($"OnSdkInitialized dispatched to {mods.Count} mods on the main thread");
    }

    /// <summary>
    /// One frame: every mod's update callback, then the SDK's update event.
    /// </summary>
    /// <remarks>
    /// Exceptions are NOT caught per mod here. <see cref="UnityPump"/> wraps this
    /// whole callback with a fault budget, and catching individually would mean a
    /// mod throwing every frame is logged every frame - the exact log flood the
    /// budget exists to prevent. The cost is that one throwing mod skips the rest
    /// of the frame's mods; the budget drops it after three, which resolves that.
    /// </remarks>
    private static void Tick(List<ModHost.LoadedMod> mods)
    {
        foreach (var mod in mods)
        {
            ModHost.Invoke(mod.Instance, "InvokeUpdate");
        }
        SdkEvents.OnInWorldUpdate.Invoke();
    }
}
