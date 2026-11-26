// SPDX-License-Identifier: GPL-2.0-only
//
// Modrix.Sons.Sdk - the API Sons of the Forest mods are written against.
//
// The namespaces and type names here deliberately mirror RedLoader's
// (`RedLoader.RLog`, `SonsSdk.SonsMod`, `SonsSdk.SdkEvents`) rather than using
// Modrix-branded ones. That is the whole point: every Sons of the Forest mod
// ever written targets those names, and a source-compatible SDK means an
// existing mod builds against Modrix by changing nothing at all. Inventing
// `ModrixMod`/`ModrixLog` would have forced every mod author to fork their
// code for no benefit to anyone.
//
// Only the surface real mods actually use is implemented. It is deliberately
// small, and every member that the loader has not yet wired up says so at
// runtime instead of silently doing nothing - a mod author debugging a hook
// that never fires deserves a log line, not silence.

using System;
using System.Collections.Generic;

namespace RedLoader
{
    /// <summary>
    /// A parameterless callback. Named for RedLoader/MelonLoader source
    /// compatibility; it is an ordinary delegate.
    /// </summary>
    public delegate void LemonAction();

    /// <summary>
    /// A subscribable event. Mods reach this through
    /// <see cref="SonsSdk.SdkEvents"/> and typically cast to it explicitly,
    /// which is why it is public rather than an implementation detail.
    /// </summary>
    /// <typeparam name="T">The delegate type subscribers supply.</typeparam>
    public class MelonEventBase<T> where T : Delegate
    {
        private readonly List<Subscription> _subscribers = new();

        private readonly struct Subscription
        {
            public Subscription(T action, int priority, bool once)
            {
                Action = action;
                Priority = priority;
                Once = once;
            }

            public T Action { get; }
            public int Priority { get; }
            public bool Once { get; }
        }

        /// <summary>Registers <paramref name="action"/> for this event.</summary>
        /// <param name="action">The callback to invoke.</param>
        /// <param name="priority">Lower runs earlier; ties keep insertion order.</param>
        /// <param name="unsubscribeOnFirstInvocation">Fire once, then drop it.</param>
        public void Subscribe(T action, int priority = 0, bool unsubscribeOnFirstInvocation = false)
        {
            if (action is null)
            {
                return;
            }

            _subscribers.Add(new Subscription(action, priority, unsubscribeOnFirstInvocation));
            // Stable sort by priority: two mods registering at the same
            // priority must keep the order they registered in, or load order
            // stops being reproducible.
            _subscribers.Sort(static (a, b) => a.Priority.CompareTo(b.Priority));
        }

        /// <summary>Removes a previously registered callback.</summary>
        /// <param name="action">The callback to remove.</param>
        public void Unsubscribe(T action)
        {
            _subscribers.RemoveAll(s => Equals(s.Action, action));
        }

        /// <summary>
        /// Invokes every subscriber. One subscriber throwing must not stop the
        /// rest: a single bad mod should not silently disable every other
        /// mod's per-frame work.
        /// </summary>
        public void Invoke()
        {
            if (_subscribers.Count == 0)
            {
                return;
            }

            var snapshot = _subscribers.ToArray();
            foreach (var subscription in snapshot)
            {
                try
                {
                    if (subscription.Action is Action plain)
                    {
                        plain();
                    }
                    else
                    {
                        subscription.Action.DynamicInvoke();
                    }
                }
                catch (Exception error)
                {
                    RLog.Error($"event subscriber threw: {error}");
                }

                if (subscription.Once)
                {
                    Unsubscribe(subscription.Action);
                }
            }
        }
    }

    /// <summary>
    /// Mod logging. Output goes to the loader's log file and, when one is
    /// attached, the console.
    /// </summary>
    public static class RLog
    {
        /// <summary>Where each line is written. Set by the loader at startup.</summary>
        public static Action<string> Sink { get; set; }

        /// <summary>Writes an informational line.</summary>
        /// <param name="message">The message.</param>
        public static void Msg(object message) => Write("INFO", message);

        /// <summary>Writes a warning.</summary>
        /// <param name="message">The message.</param>
        public static void Warning(object message) => Write("WARN", message);

        /// <summary>Writes an error.</summary>
        /// <param name="message">The message.</param>
        public static void Error(object message) => Write("ERROR", message);

        private static void Write(string level, object message)
        {
            var line = $"[{level}] {message}";
            var sink = Sink;
            if (sink is not null)
            {
                try
                {
                    sink(line);
                    return;
                }
                catch
                {
                    // A broken sink must never take a mod down with it; fall
                    // through to the console below.
                }
            }

            Console.WriteLine(line);
        }
    }
}

namespace SonsSdk
{
    using RedLoader;

    /// <summary>
    /// Base class for every Sons of the Forest mod.
    /// </summary>
    /// <remarks>
    /// The loader constructs one instance per discovered mod assembly, then
    /// drives the lifecycle in this order:
    /// <list type="number">
    ///   <item><description><see cref="OnInitializeMod"/> - before the game's
    ///     systems exist. Configuration only; touching game state here will
    ///     throw.</description></item>
    ///   <item><description><see cref="OnSdkInitialized"/> - once the game and
    ///     SDK are up. This is where game state becomes safe to touch.</description></item>
    ///   <item><description><see cref="OnGameStart"/> - a world has loaded.</description></item>
    ///   <item><description><see cref="OnApplicationQuit"/> then
    ///     <see cref="OnDeinitializeMod"/> - on shutdown.</description></item>
    /// </list>
    /// </remarks>
    public abstract class SonsMod
    {
        /// <summary>
        /// Apply every <c>[HarmonyPatch]</c> class in this assembly during
        /// initialization. Set it in the constructor.
        /// </summary>
        protected bool HarmonyPatchAll { get; set; }

        /// <summary>
        /// A per-frame callback. Assign a method in the constructor; the
        /// loader invokes it once per frame on the main thread.
        /// </summary>
        /// <remarks>
        /// This is the only per-frame hook, and it runs on Unity's main
        /// thread - work queued from any other thread must be marshalled
        /// through it.
        /// </remarks>
        protected Action OnUpdateCallback { get; set; }

        /// <summary>The mod's id, from its <c>manifest.json</c>.</summary>
        public string Id { get; internal set; } = string.Empty;

        /// <summary>The mod's version, from its <c>manifest.json</c>.</summary>
        public string Version { get; internal set; } = string.Empty;

        /// <summary>Called before the game's systems exist.</summary>
        protected virtual void OnInitializeMod() { }

        /// <summary>Called once the game and SDK are ready.</summary>
        protected virtual void OnSdkInitialized() { }

        /// <summary>Called when a world has finished loading.</summary>
        protected virtual void OnGameStart() { }

        /// <summary>Called after a scene is unloaded.</summary>
        /// <param name="buildIndex">The scene's build index.</param>
        /// <param name="sceneName">The scene's name.</param>
        public virtual void OnSceneWasUnloaded(int buildIndex, string sceneName) { }

        /// <summary>Called as the game shuts down.</summary>
        protected virtual void OnApplicationQuit() { }

        /// <summary>Called last, after <see cref="OnApplicationQuit"/>.</summary>
        protected virtual void OnDeinitializeMod() { }

        // --- loader entry points -------------------------------------------
        // The lifecycle methods above are `protected` for source compatibility
        // with RedLoader, so the host cannot call them directly. These
        // internal shims are how the loader drives a mod.

        internal void InvokeInitialize() => OnInitializeMod();

        internal void InvokeSdkInitialized() => OnSdkInitialized();

        internal void InvokeGameStart() => OnGameStart();

        internal void InvokeApplicationQuit() => OnApplicationQuit();

        internal void InvokeDeinitialize() => OnDeinitializeMod();

        internal void InvokeUpdate() => OnUpdateCallback?.Invoke();

        internal bool WantsHarmonyPatchAll => HarmonyPatchAll;
    }

    /// <summary>
    /// Game lifecycle events a mod can subscribe to.
    /// </summary>
    /// <summary>
    /// Lets a mod ask the loader to stop ticking across a risky transition.
    /// </summary>
    /// <remarks>
    /// <para>
    /// The loader's per-frame tick is driven from a detour on IL2CPP's dispatcher
    /// rather than from a Unity callback, because on this game nothing else
    /// works. That is fine in steady state and dangerous during a scene load: the
    /// tick lands in the middle of a native-to-managed transition, and running mod
    /// code there while the world is being torn down and rebuilt kills the process
    /// - reliably enough that loading a save with the pump running does not
    /// survive.
    /// </para>
    /// <para>
    /// So a mod that triggers a transition says so. This is deliberately explicit
    /// rather than automatic: the loader cannot detect a load starting without
    /// calling into the game from the very context that is unsafe.
    /// </para>
    /// <para>
    /// The hook is assigned by the loader at startup, exactly like
    /// <see cref="RedLoader.RLog.Sink"/>. If no loader is present the call is a
    /// no-op, so a mod can use it unconditionally.
    /// </para>
    /// </remarks>
    public static class LoaderControl
    {
        /// <summary>Assigned by the loader. Takes the number of seconds to pause.</summary>
        public static Action<float>? SuspendHook { get; set; }

        /// <summary>Assigned by the loader. Asks for one main-thread tick.</summary>
        public static Action? RequestTickHook { get; set; }

        /// <summary>
        /// Stop per-frame mod updates for a while.
        /// </summary>
        /// <param name="seconds">
        /// How long to stay quiet. Generous is correct - a load that finishes
        /// early costs nothing, one that overruns costs the process.
        /// </param>
        public static void SuspendUpdates(float seconds)
        {
            SuspendHook?.Invoke(seconds);
        }

        /// <summary>
        /// Ask the loader to run one main-thread tick soon.
        /// </summary>
        /// <remarks>
        /// <para>
        /// The loader does not tick continuously by default. Its tick comes from a
        /// detour on IL2CPP's dispatcher rather than from a Unity callback, and
        /// entering mod code from there is not safe at an arbitrary moment - often
        /// enough to kill the game that background ticking is not worth its cost.
        /// </para>
        /// <para>
        /// So a mod with work waiting says so, and pays the risk only when it has
        /// something to do. Calling this with nothing queued is harmless; calling
        /// it from any thread is fine.
        /// </para>
        /// </remarks>
        public static void RequestTick()
        {
            RequestTickHook?.Invoke();
        }
    }

    public static class SdkEvents
    {
        /// <summary>
        /// Fires every frame while a world is loaded - not at the menu. This
        /// is the hook for gameplay logic that needs the player to exist.
        /// </summary>
        public static readonly MelonEventBase<LemonAction> OnInWorldUpdate = new();

        /// <summary>Fires once when a world has finished loading.</summary>
        public static readonly MelonEventBase<LemonAction> OnGameStart = new();

        /// <summary>Fires once when the SDK is ready.</summary>
        public static readonly MelonEventBase<LemonAction> OnSdkInitialized = new();
    }
}
