// SPDX-License-Identifier: GPL-2.0-only

using System;
using System.Collections.Generic;
using System.Reflection;
using System.Threading;

using HarmonyLib;

namespace Modrix.Loader;

/// <summary>
/// Distributes the loader's per-frame tick to mods and to queued work.
/// </summary>
/// <remarks>
/// <para>
/// This type does not produce ticks, it only fans them out. They come from
/// <see cref="PumpBehaviour"/> - an injected <c>MonoBehaviour</c> whose
/// <c>Update</c> Unity calls like any other, which is the only per-frame callback
/// on this game that is both reachable and a safe point. That file records what
/// else was tried.
/// </para>
/// <para>
/// The tick body is held to one rule: <b>it must never throw into Unity.</b> It
/// runs over a hundred times a second on the main thread, so an escaping
/// exception is a dead game rather than a logged error. Every callback is
/// individually guarded, and one that throws repeatedly is dropped rather than
/// left to fail forever.
/// </para>
/// </remarks>
internal static class UnityPump
{
    /// <summary>
    /// How many times one callback may throw before it is dropped.
    /// </summary>
    /// <remarks>
    /// A callback failing every frame produces thousands of identical log lines a
    /// minute and buries whatever else went wrong. Three strikes is enough to
    /// capture the stack trace and establish that it is not transient.
    /// </remarks>
    private const int FaultsBeforeDrop = 3;

    /// <summary>Harmony identity, also what appears in Harmony's own logs.</summary>
    private const string HarmonyId = "dev.modrix.loader.pump";

    /// <summary>
    /// Static, native-invoked, per-frame methods patched to obtain the tick.
    /// </summary>
    /// <remarks>
    /// <para>
    /// Every constraint in this list was established by killing the game:
    /// </para>
    /// <list type="bullet">
    ///   <item><description><b>Must be static.</b> Patching the instance method
    ///     <c>EventSystem.Update</c> attaches cleanly and then kills the game on
    ///     every launch; patching statics - as
    ///     <see cref="MainThreadGate"/> does - is reliable.</description></item>
    ///   <item><description><b>Must not be inlined or stripped.</b>
    ///     <c>Time.get_deltaTime</c> is inlined into its callers and
    ///     <c>Application.InvokeOnBeforeRender</c> has no call site left, so both
    ///     patch successfully and never fire.</description></item>
    ///   <item><description><b>Must not be the runtime's dispatcher.</b> Detouring
    ///     <c>il2cpp_runtime_invoke</c> gives a tick and costs half of all launches
    ///     - see <see cref="MainThreadGate"/>.</description></item>
    ///   <item><description><b>Cannot be an injected component.</b>
    ///     <c>ClassInjector.RegisterTypeInIl2Cpp</c> hooks runtime internals
    ///     through the same managed detour path and is fatal here, even from a real
    ///     Unity callback.</description></item>
    /// </list>
    /// <para>
    /// What satisfies all of it is the render loop. This game runs HDRP, so
    /// Unity's native renderer calls
    /// <c>RenderPipelineManager.DoRenderLoop_Internal</c> once per frame, and the
    /// pipeline calls <c>BeginContextRendering</c> alongside it. Both are static
    /// and both have real call sites.
    /// </para>
    /// </remarks>
    private static readonly (string Type, string Method)[] Candidates =
    {
        ("UnityEngine.Rendering.RenderPipelineManager", "DoRenderLoop_Internal"),
        ("UnityEngine.Rendering.RenderPipelineManager", "BeginContextRendering"),
    };

    /// <summary>Interop assembly holding the pump targets.</summary>
    private const string CoreModuleAssembly = "UnityEngine.CoreModule";

    private static readonly object Gate = new();

    /// <summary>Frame callbacks, and the fault count for each.</summary>
    private static readonly List<Subscriber> Subscribers = new();

    /// <summary>Work queued from other threads, to run on the next frame.</summary>
    private static readonly Queue<Action> Pending = new();

    /// <summary>Ticks are skipped until this time. See <c>SonsSdk.LoaderControl</c>.</summary>
    private static readonly System.Diagnostics.Stopwatch SuspendClock = new();

    private static long _suspendedUntilMs;

    private static Action<string>? _log;
    private static long _ticks;
    private static int _mainThreadId;
    private static bool _started;

    /// <summary>A frame callback plus how many times it has faulted.</summary>
    private sealed class Subscriber
    {
        internal Subscriber(string name, Action action)
        {
            Name = name;
            Action = action;
        }

        internal string Name { get; }
        internal Action Action { get; }
        internal int Faults { get; set; }
        internal bool Dropped { get; set; }
    }

    /// <summary>Frames observed since the pump started.</summary>
    internal static long Ticks => Interlocked.Read(ref _ticks);

    /// <summary>The Unity main thread's managed id, or 0 before the first tick.</summary>
    internal static int MainThreadId => Volatile.Read(ref _mainThreadId);

    /// <summary>True when something has been queued for the main thread.</summary>
    /// <remarks>
    /// Read from the dispatcher hook on every managed call once the loader is up,
    /// so it is a plain count check under the existing lock rather than anything
    /// cleverer.
    /// </remarks>
    internal static bool HasPendingWork
    {
        get
        {
            lock (Gate)
            {
                return Pending.Count > 0;
            }
        }
    }

    /// <summary>True when called from the Unity main thread.</summary>
    internal static bool OnMainThread =>
        Environment.CurrentManagedThreadId == MainThreadId && MainThreadId != 0;

    /// <summary>Make the pump ready to receive ticks.</summary>
    /// <param name="log">Where to report faults.</param>
    internal static void Start(Action<string> log)
    {
        lock (Gate)
        {
            if (_started)
            {
                return;
            }
            _started = true;
            _log = log;
        }
        // A mod that starts a scene load tells us, because we cannot safely
        // detect one from the dispatcher. See SonsSdk.LoaderControl.
        SuspendClock.Start();
        SonsSdk.LoaderControl.SuspendHook = seconds => Suspend(seconds, log);
        // One queued no-op is enough: the next dispatch sees pending work, ticks
        // once, and that tick runs every per-frame subscriber - including the
        // mod that asked.
        SonsSdk.LoaderControl.RequestTickHook = () => Post(() => { });

        var postfix = typeof(UnityPump).GetMethod(
            nameof(Tick),
            BindingFlags.Static | BindingFlags.Public)
            ?? throw new InvalidOperationException($"{nameof(Tick)} is missing");

        var harmony = new Harmony(HarmonyId);
        var unity = Assembly.Load(CoreModuleAssembly);
        var attached = 0;
        foreach (var (typeName, methodName) in Candidates)
        {
            try
            {
                var type = unity.GetType(typeName)
                    ?? throw new InvalidOperationException(
                        $"{typeName} not found in {CoreModuleAssembly}");
                foreach (var candidate in type.GetMethods(
                    BindingFlags.Static | BindingFlags.Public | BindingFlags.NonPublic))
                {
                    if (!string.Equals(candidate.Name, methodName, StringComparison.Ordinal))
                    {
                        continue;
                    }
                    harmony.Patch(candidate, postfix: new HarmonyMethod(postfix));
                    log($"per-frame pump attached to {typeName}.{methodName}");
                    attached++;
                    break;
                }
            }
            catch (Exception error)
            {
                // More than one candidate exists precisely so that one being
                // absent, stripped or unpatchable is survivable.
                log($"per-frame pump could not attach to {typeName}.{methodName}: "
                    + $"{ModHost.Describe(error)}");
            }
        }

        if (attached == 0)
        {
            throw new InvalidOperationException(
                "no per-frame hook attached; mods would load but never update");
        }
    }

    /// <summary>Stop ticking for a while, at a mod's request.</summary>
    /// <remarks>
    /// Extends an existing suspension rather than replacing it, so two mods each
    /// asking for quiet cannot shorten the other's window.
    /// </remarks>
    private static void Suspend(float seconds, Action<string> log)
    {
        var until = SuspendClock.ElapsedMilliseconds + (long)(seconds * 1000);
        lock (Gate)
        {
            if (until > _suspendedUntilMs)
            {
                _suspendedUntilMs = until;
            }
        }
        log($"per-frame pump suspended for {seconds:F0}s at a mod's request");
    }

    /// <summary>True while a mod has asked for the pump to stay quiet.</summary>
    private static bool Suspended
    {
        get
        {
            lock (Gate)
            {
                return SuspendClock.ElapsedMilliseconds < _suspendedUntilMs;
            }
        }
    }

    /// <summary>
    /// Register a callback to run once per frame on the Unity main thread.
    /// </summary>
    /// <param name="name">Used to attribute faults in the log.</param>
    /// <param name="action">The callback. Must not block.</param>
    internal static void OnFrame(string name, Action action)
    {
        lock (Gate)
        {
            Subscribers.Add(new Subscriber(name, action));
        }
    }

    /// <summary>
    /// Queue work to run on the Unity main thread at the start of the next frame.
    /// </summary>
    /// <remarks>
    /// Fire-and-forget by design. Callers needing a result or a deadline build
    /// that on top - a bridge command, for instance, posts a closure that
    /// completes a <c>TaskCompletionSource</c> and waits on it with a timeout.
    /// Baking a timeout in here would put the waiting inside the frame loop.
    /// </remarks>
    /// <param name="work">The work to run.</param>
    internal static void Post(Action work)
    {
        lock (Gate)
        {
            Pending.Enqueue(work);
        }
    }

    /// <summary>
    /// One frame, on the Unity main thread.
    /// </summary>
    /// <remarks>
    /// Public because it is also the target of a runtime-emitted <c>Update</c>
    /// method (see <see cref="PumpBehaviour"/>), which lives in a dynamic assembly
    /// that cannot reach an internal member of this one. Nothing here may throw.
    /// </remarks>
    public static void Tick()
    {
        if (Volatile.Read(ref _mainThreadId) == 0)
        {
            Volatile.Write(ref _mainThreadId, Environment.CurrentManagedThreadId);
            Report($"first frame on main thread {Environment.CurrentManagedThreadId}");
        }
        Heartbeat(Interlocked.Increment(ref _ticks));

        // Queued work still runs while suspended - it is one-shot, explicitly
        // requested, and usually the very thing that asked for the quiet window.
        // Only the per-frame subscribers are held back.
        DrainPending();
        if (!Suspended)
        {
            RunSubscribers();
        }
    }

    /// <summary>Log that frames are arriving, at a decaying rate.</summary>
    /// <remarks>
    /// Two questions have to be answerable from the log alone: "did the pump ever
    /// tick?" and "is it still ticking?". The early marks answer the first without
    /// waiting; the periodic mark answers the second without turning the log into
    /// a frame counter.
    /// </remarks>
    private static void Heartbeat(long tick)
    {
        const long Periodic = 3600;
        if (tick == 1 || tick == 60 || tick == 600 || tick % Periodic == 0)
        {
            Report($"pump tick {tick}");
        }
    }

    /// <summary>Run everything queued from other threads.</summary>
    /// <remarks>
    /// The queue is drained into a batch under the lock and run outside it, so a
    /// queued action may itself call <see cref="Post"/> without deadlocking. Work
    /// queued during the drain waits for the next frame rather than extending
    /// this one.
    /// </remarks>
    private static void DrainPending()
    {
        var batch = new List<Action>();
        lock (Gate)
        {
            while (Pending.Count > 0)
            {
                batch.Add(Pending.Dequeue());
            }
        }

        foreach (var work in batch)
        {
            try
            {
                work();
            }
            catch (Exception error)
            {
                Report($"queued main-thread work threw: {ModHost.Describe(error)}");
            }
        }
    }

    /// <summary>Run each per-frame subscriber, guarding and dropping faulty ones.</summary>
    private static void RunSubscribers()
    {
        Subscriber[] snapshot;
        lock (Gate)
        {
            snapshot = Subscribers.ToArray();
        }

        foreach (var subscriber in snapshot)
        {
            if (subscriber.Dropped)
            {
                continue;
            }
            try
            {
                subscriber.Action();
            }
            catch (Exception error)
            {
                subscriber.Faults++;
                Report($"{subscriber.Name} frame callback threw "
                    + $"({subscriber.Faults}/{FaultsBeforeDrop}): {ModHost.Describe(error)}");
                if (subscriber.Faults >= FaultsBeforeDrop)
                {
                    subscriber.Dropped = true;
                    Report($"{subscriber.Name} frame callback dropped after "
                        + $"{FaultsBeforeDrop} faults");
                }
            }
        }
    }

    /// <summary>Log without ever throwing, because the caller is the frame loop.</summary>
    private static void Report(string message)
    {
        try
        {
            _log?.Invoke(message);
        }
        catch (Exception)
        {
            // A failing log sink must not take the game with it. There is nowhere
            // left to report this, which is precisely why it is empty.
        }
    }
}
