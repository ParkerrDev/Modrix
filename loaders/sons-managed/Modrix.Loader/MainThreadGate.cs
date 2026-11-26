// SPDX-License-Identifier: GPL-2.0-only

using System;
using System.Reflection;
using System.Threading;

using HarmonyLib;

namespace Modrix.Loader;

/// <summary>
/// Waits for the Unity main thread, then runs the rest of the loader on it.
/// </summary>
/// <remarks>
/// <para>
/// The loader is injected on a bootstrap thread so that minutes of interop
/// generation cannot stall the game's startup. That is the right trade, but it
/// leaves the loader on the wrong thread for everything that follows: mods call
/// into game types from <c>OnInitializeMod</c>, and an IL2CPP call from a thread
/// the runtime's collector never registered either aborts the process
/// ("Collecting from unknown thread") or simply never returns - with the mod's own
/// log line already written, so it looks like the mod worked and the loader
/// stopped.
/// </para>
/// <para>
/// <b>What this used to do, and why it does not any more.</b> The established
/// approach in this ecosystem - what BepInEx and RedLoader both do - is to detour
/// <c>il2cpp_runtime_invoke</c>, the runtime's universal managed-call dispatcher,
/// and wait for a known callback to arrive through it. That works, and it was
/// measurably unacceptable here: with the dispatcher detoured the game died on
/// roughly <b>half of all launches</b>, at the title screen, with no crash dump
/// and nothing in the log. With the detour removed and nothing else changed, four
/// launches out of four survived. The difference is exposure - the hook sits on
/// every managed call in the process, around 4,800 a second, each one a round trip
/// into CoreCLR.
/// </para>
/// <para>
/// The distinction from those loaders is that they hook the dispatcher from
/// <em>native</em> code with hand-written detour engines. Doing it from managed
/// code, through Harmony, puts a managed delegate in front of the runtime's own
/// dispatch path, and that is not a safe place for one.
/// </para>
/// <para>
/// So the gate now patches exactly one method:
/// <c>SceneManager.Internal_ActiveSceneChanged</c>. It is static, Unity calls it
/// from native code on the main thread when the active scene changes, and it runs
/// a handful of times per session rather than thousands of times a second. It is
/// also a real Unity safe point, which the middle of an arbitrary dispatch is not.
/// </para>
/// </remarks>
internal static class MainThreadGate
{
    /// <summary>Harmony identity, also what appears in Harmony's own logs.</summary>
    private const string HarmonyId = "dev.modrix.loader.gate";

    /// <summary>Interop assembly holding the hook target.</summary>
    private const string CoreModuleAssembly = "UnityEngine.CoreModule";

    /// <summary>Type declaring the hook target.</summary>
    private const string HookType = "UnityEngine.SceneManagement.SceneManager";

    /// <summary>
    /// The callback whose first invocation means "Unity is up, on the main thread".
    /// </summary>
    /// <remarks>
    /// Static and called directly by Unity's native scene manager, so IL2CPP
    /// cannot inline it away and a call site always exists. It is the same signal
    /// BepInEx and RedLoader wait for, reached by a far narrower route.
    /// </remarks>
    private const string HookMethod = "Internal_ActiveSceneChanged";

    private static Action<string>? _log;
    private static Action? _continuation;

    /// <summary>0 until the gate has opened; 1 afterwards.</summary>
    private static int _fired;

    /// <summary>Managed thread id of the Unity main thread, or 0 before opening.</summary>
    private static int _mainThreadId;

    /// <summary>True once the gate has handed control to the main thread.</summary>
    internal static bool Opened => Volatile.Read(ref _fired) != 0;

    /// <summary>The Unity main thread's managed id, or 0 before the gate opens.</summary>
    internal static int MainThreadId => Volatile.Read(ref _mainThreadId);

    /// <summary>
    /// Install the gate. <paramref name="continuation"/> runs once, on the Unity
    /// main thread, at the first active-scene change.
    /// </summary>
    /// <param name="continuation">The rest of the loader's startup.</param>
    /// <param name="log">Progress sink.</param>
    /// <exception cref="InvalidOperationException">
    /// The hook target is missing, which would mean the interop assemblies are not
    /// what this loader expects.
    /// </exception>
    internal static void Open(Action continuation, Action<string> log)
    {
        _continuation = continuation;
        _log = log;

        var target = Assembly.Load(CoreModuleAssembly).GetType(HookType)?.GetMethod(
            HookMethod,
            BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException(
                $"{HookType}.{HookMethod} not found in {CoreModuleAssembly}");

        var postfix = typeof(MainThreadGate).GetMethod(
            nameof(Opening),
            BindingFlags.Static | BindingFlags.NonPublic)
            ?? throw new InvalidOperationException($"{nameof(Opening)} is missing");

        new Harmony(HarmonyId).Patch(target, postfix: new HarmonyMethod(postfix));
        log($"main-thread gate armed on {HookType}.{HookMethod}");
    }

    /// <summary>
    /// Runs on the Unity main thread when the active scene changes.
    /// </summary>
    /// <remarks>
    /// Nothing here may throw: Unity called us from native code, and an exception
    /// crossing back terminates the process rather than being logged.
    /// </remarks>
    private static void Opening()
    {
        if (Interlocked.Exchange(ref _fired, 1) != 0)
        {
            return;
        }

        try
        {
            Volatile.Write(ref _mainThreadId, Environment.CurrentManagedThreadId);
            _log?.Invoke($"main-thread gate opened on {HookMethod} "
                + $"(thread {Environment.CurrentManagedThreadId})");
            _continuation?.Invoke();
        }
        catch (Exception error)
        {
            try
            {
                _log?.Invoke($"ERROR main-thread startup failed: {ModHost.Describe(error)}");
            }
            catch (Exception)
            {
                // Nowhere left to report to.
            }
        }
    }
}
