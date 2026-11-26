// SPDX-License-Identifier: GPL-2.0-only

using System;
using System.Collections.Generic;
using System.Reflection;
using System.Runtime.InteropServices;

using Il2CppInterop.Runtime.Injection;

using MonoMod.RuntimeDetour;

// Both libraries define an `IDetour`, and this file is the seam between them, so
// the Il2CppInterop one is aliased rather than imported plainly. Using the wrong
// one compiles in some positions and fails in others with a message about return
// types that does not mention the ambiguity at all.
using InteropDetour = Il2CppInterop.Runtime.Injection.IDetour;

namespace Modrix.Loader;

/// <summary>
/// Supplies Il2CppInterop with the native detours it cannot provide itself.
/// </summary>
/// <remarks>
/// <para>
/// Il2CppInterop deliberately does not ship a detour implementation. It needs
/// one - to hook IL2CPP internals when injecting a managed type, and to
/// redirect a native method when Harmony patches it - but which hooking library
/// to use is the host's decision, so it asks for an
/// <see cref="IDetourProvider"/> and calls into that.
/// </para>
/// <para>
/// Leaving it unset does not fail at startup. The runtime initializes happily,
/// interop assemblies load, and then the first Harmony patch of an IL2CPP method
/// throws <c>NullReferenceException</c> from inside
/// <c>Il2CppDetourMethodPatcher.DetourTo</c> - a stack that names neither this
/// interface nor the fact that it was never populated. That is why this exists as
/// its own file with this comment on it.
/// </para>
/// <para>
/// BepInEx and RedLoader each hand-wrote a native detour engine for this job.
/// This one delegates to MonoMod's, which is a maintained implementation of
/// exactly that, and needs only the small adaptation below.
/// </para>
/// </remarks>
internal sealed class Il2CppDetourProvider : IDetourProvider
{
    /// <summary>
    /// Keeps every hook delegate reachable for the life of the process.
    /// </summary>
    /// <remarks>
    /// <b>This list is load-bearing, not bookkeeping.</b>
    /// <see cref="Marshal.GetFunctionPointerForDelegate{TDelegate}"/> does not
    /// root its argument: the pointer stays valid only while the delegate is
    /// alive. Once collected, the native code the detour installed jumps into
    /// freed memory, and the game dies with an access violation somewhere
    /// unrelated, minutes later, with nothing tying it back to here. Detours are
    /// never uninstalled, so the list only grows to the number of patched
    /// methods and needs no eviction.
    /// </remarks>
    private static readonly List<Delegate> Roots = new();

    private static readonly object Gate = new();

    /// <summary>Create a detour from a native address to a managed delegate.</summary>
    /// <remarks>
    /// Not applied here. Il2CppInterop's contract is create-then-apply, so
    /// <c>ManualApply</c> is set and the caller decides when the redirect goes
    /// live.
    /// </remarks>
    /// <typeparam name="TDelegate">The hook's signature.</typeparam>
    /// <param name="original">Address of the function to redirect.</param>
    /// <param name="target">The managed replacement.</param>
    /// <returns>The unapplied detour.</returns>
    public InteropDetour Create<TDelegate>(nint original, TDelegate target)
        where TDelegate : Delegate
    {
        lock (Gate)
        {
            Roots.Add(target);
        }

        var replacement = Marshal.GetFunctionPointerForDelegate(target);
        var config = new NativeDetourConfig { ManualApply = true };
        // target.GetType(), not typeof(TDelegate). Il2CppInterop calls this with
        // the argument statically typed as `Delegate`, so TDelegate infers to
        // `System.Delegate` - which has no Invoke and therefore describes no
        // signature. The concrete delegate type is only available at runtime.
        return new Il2CppDetour(
            new NativeDetour(original, replacement, config),
            target.GetType());
    }

    /// <summary>Root a delegate for the process lifetime. See <see cref="Roots"/>.</summary>
    internal static void Root(Delegate value)
    {
        lock (Gate)
        {
            Roots.Add(value);
        }
    }
}

/// <summary>One native detour, adapted to the shape Il2CppInterop expects.</summary>
internal sealed class Il2CppDetour : InteropDetour
{
    private readonly NativeDetour _detour;

    /// <summary>
    /// Concrete type of the hook delegate, which is also the signature of the
    /// function being detoured.
    /// </summary>
    /// <remarks>
    /// Carried explicitly because it cannot be recovered later: the interface
    /// erases it to <see cref="Delegate"/>, and a trampoline cannot be built
    /// without a signature.
    /// </remarks>
    private readonly Type _signature;

    private readonly object _gate = new();
    private IntPtr _trampoline;

    internal Il2CppDetour(NativeDetour detour, Type signature)
    {
        _detour = detour;
        _signature = signature;
    }

    /// <summary>The function being redirected.</summary>
    public IntPtr Target => _detour.Data.Method;

    /// <summary>Where it is redirected to.</summary>
    public IntPtr Detour => _detour.Data.Target;

    /// <summary>
    /// A callable native address for the original, pre-detour code.
    /// </summary>
    /// <remarks>
    /// <para>
    /// The tempting implementation - hand back MonoMod's internal
    /// <c>_BackupNative</c> - is wrong in a way that is very hard to diagnose.
    /// That buffer holds a *copy of the overwritten bytes* and nothing else: no
    /// jump back into the remainder of the original function. Executing it runs
    /// the first few instructions and then falls into whatever follows in memory,
    /// so the game dies on the first frame after the patch with an access
    /// violation and no managed stack. Il2CppInterop reads this property on every
    /// IL2CPP patch, so getting it wrong is not a corner case.
    /// </para>
    /// <para>
    /// Generated once and cached, and the delegate is rooted: the returned
    /// pointer is only valid while the delegate behind it is alive.
    /// </para>
    /// </remarks>
    public IntPtr OriginalTrampoline
    {
        get
        {
            lock (_gate)
            {
                if (_trampoline == IntPtr.Zero)
                {
                    var original = Trampoline(_signature);
                    Il2CppDetourProvider.Root(original);
                    _trampoline = Marshal.GetFunctionPointerForDelegate(original);
                }
                return _trampoline;
            }
        }
    }

    /// <summary>Make the redirect live.</summary>
    public void Apply()
    {
        _detour.Apply();
    }

    /// <summary>A callable delegate for the original, pre-detour code.</summary>
    /// <typeparam name="T">
    /// The requested delegate type. When this is <see cref="Delegate"/> itself -
    /// which is how Il2CppInterop calls it - the concrete signature captured at
    /// construction is used instead.
    /// </typeparam>
    /// <returns>A delegate invoking the original code.</returns>
    public T GenerateTrampoline<T>()
        where T : Delegate
    {
        var requested = typeof(T).GetMethod("Invoke") is null ? _signature : typeof(T);
        return (T)Trampoline(requested);
    }

    /// <summary>
    /// Build a trampoline delegate, supplying the signature explicitly.
    /// </summary>
    /// <remarks>
    /// <see cref="NativeDetour.GenerateTrampoline{T}"/> cannot be used. It
    /// forwards a null signature and expects the detour to carry the managed
    /// method it replaced - but a detour created from two raw addresses, which is
    /// all Il2CppInterop can give us for a native IL2CPP function, has no managed
    /// method. It throws <c>ArgumentNullException: A signature must be given if
    /// the NativeDetour doesn't hold a reference to a managed method</c>.
    /// </remarks>
    /// <param name="delegateType">Concrete delegate type describing the call.</param>
    /// <returns>A delegate invoking the original code.</returns>
    private Delegate Trampoline(Type delegateType)
    {
        // A delegate type's Invoke method IS the signature being asked for.
        var signature = delegateType.GetMethod("Invoke")
            ?? throw new InvalidOperationException(
                $"{delegateType.FullName} has no Invoke method");
        var generated = _detour.GenerateTrampoline(signature)
            ?? throw new InvalidOperationException(
                $"MonoMod produced no trampoline for {delegateType.FullName}");
        return ((MethodInfo)generated).CreateDelegate(delegateType);
    }

    /// <summary>Undo the redirect and release the detour.</summary>
    public void Dispose()
    {
        _detour.Dispose();
    }
}
