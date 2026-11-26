// SPDX-License-Identifier: GPL-2.0-only

using System;
using System.Reflection;
using System.Reflection.Emit;

using Il2CppInterop.Runtime;
using Il2CppInterop.Runtime.Injection;

namespace Modrix.Loader;

/// <summary>
/// Replaces the bootstrap tick with a real Unity component.
/// </summary>
/// <remarks>
/// <para>
/// <see cref="MainThreadGate"/> can produce ticks, and does so to get the loader
/// started - but it is the wrong thing to keep. Its ticks come from the middle of
/// an arbitrary <c>il2cpp_runtime_invoke</c> dispatch, which is <b>not a safe
/// point</b>: the runtime is part-way through handing a native call to managed
/// code, and running mod code there intermittently corrupts whatever the game was
/// doing. The symptom is exactly as unhelpful as it sounds - the game dies
/// somewhere inside scene loading, on maybe one launch in three, with no crash
/// dump and nothing in the loader log.
/// </para>
/// <para>
/// Unity's own <c>Update</c> is a safe point by definition, so the durable pump
/// is an injected <c>MonoBehaviour</c> on a <c>DontDestroyOnLoad</c> object -
/// what every IL2CPP loader ends up with. Once it is running the gate's detour is
/// removed and the unsafe ticking stops entirely.
/// </para>
/// <para>
/// <b>Timing is the whole trick.</b> Injecting a class calls back into the
/// runtime, so doing it from the gate's own callback deadlocks: the dispatcher
/// frame we are inside still holds runtime locks. Installing from a later pump
/// tick, once that frame has unwound, avoids it - which is why this is deferred
/// rather than done during startup.
/// </para>
/// <para>
/// The type is emitted at runtime because deriving from <c>MonoBehaviour</c> at
/// compile time would mean referencing <c>UnityEngine.CoreModule</c>, and the
/// interop assemblies are generated on the user's machine from the user's copy of
/// the game. Emitting against whatever the generated assemblies actually contain
/// cannot drift out of sync, because it is read from them.
/// </para>
/// </remarks>
internal static class PumpBehaviour
{
    /// <summary>Interop assembly holding the Unity types used here.</summary>
    private const string CoreModule = "UnityEngine.CoreModule";

    /// <summary>Name of the emitted component.</summary>
    private const string BehaviourName = "ModrixPump";

    /// <summary>Name of the host object, as it appears in the hierarchy.</summary>
    private const string HostObjectName = "Modrix";

    /// <summary>Set once the component is confirmed to be driving ticks.</summary>
    private static bool _installed;

    /// <summary>True once Unity is calling the injected component's Update.</summary>
    internal static bool Live => _installed;

    /// <summary>
    /// Emit, inject and attach the pump component.
    /// </summary>
    /// <remarks>
    /// Must run on the Unity main thread, and must NOT run from inside the gate's
    /// dispatcher callback - see the class remarks.
    /// </remarks>
    /// <param name="log">Progress sink.</param>
    /// <exception cref="InvalidOperationException">
    /// A required Unity type or method is missing from the interop assemblies.
    /// </exception>
    internal static void Install(Action<string> log)
    {
        if (_installed)
        {
            return;
        }

        var unity = Assembly.Load(CoreModule);
        var behaviour = Emit(Resolve(unity, "UnityEngine.MonoBehaviour"));

        ClassInjector.RegisterTypeInIl2Cpp(behaviour);
        log($"injected {behaviour.Name} into il2cpp");

        Attach(unity, behaviour);
        _installed = true;
        log($"pump component attached to a persistent '{HostObjectName}' object");
    }

    /// <summary>Look up a Unity type, failing with the name if it is absent.</summary>
    private static Type Resolve(Assembly unity, string name)
    {
        return unity.GetType(name)
            ?? throw new InvalidOperationException($"{name} not found in {CoreModule}");
    }

    /// <summary>
    /// Define a <c>MonoBehaviour</c> subclass whose <c>Update</c> drives the pump.
    /// </summary>
    /// <remarks>
    /// Two details are mandatory rather than stylistic. The type must expose a
    /// constructor taking an <c>IntPtr</c> and pass it to the base constructor -
    /// that is how Il2CppInterop hands a managed wrapper its native object, and
    /// without it injection fails. And <c>Update</c> must be public and
    /// parameterless, because Unity finds it by name and signature.
    /// </remarks>
    private static Type Emit(Type monoBehaviour)
    {
        var assembly = AssemblyBuilder.DefineDynamicAssembly(
            new AssemblyName("Modrix.Loader.Dynamic"), AssemblyBuilderAccess.Run);
        var module = assembly.DefineDynamicModule("Modrix.Loader.Dynamic");
        var type = module.DefineType(
            BehaviourName,
            TypeAttributes.Public | TypeAttributes.Class,
            monoBehaviour);

        EmitPointerConstructor(type, monoBehaviour);
        EmitUpdate(type);

        return type.CreateType()
            ?? throw new InvalidOperationException($"could not create {BehaviourName}");
    }

    /// <summary>Emit <c>public ModrixPump(IntPtr ptr) : base(ptr) { }</c>.</summary>
    private static void EmitPointerConstructor(TypeBuilder type, Type monoBehaviour)
    {
        var baseConstructor = monoBehaviour.GetConstructor(
            BindingFlags.Instance | BindingFlags.Public | BindingFlags.NonPublic,
            binder: null,
            types: new[] { typeof(IntPtr) },
            modifiers: null)
            ?? throw new InvalidOperationException(
                "MonoBehaviour has no IntPtr constructor - the interop assemblies "
                + "were not produced by Il2CppInterop");

        var constructor = type.DefineConstructor(
            MethodAttributes.Public | MethodAttributes.HideBySig
                | MethodAttributes.SpecialName | MethodAttributes.RTSpecialName,
            CallingConventions.Standard,
            new[] { typeof(IntPtr) });

        var il = constructor.GetILGenerator();
        il.Emit(OpCodes.Ldarg_0);
        il.Emit(OpCodes.Ldarg_1);
        il.Emit(OpCodes.Call, baseConstructor);
        il.Emit(OpCodes.Ret);
    }

    /// <summary>Emit <c>public void Update() =&gt; UnityPump.Tick();</c>.</summary>
    private static void EmitUpdate(TypeBuilder type)
    {
        var tick = typeof(UnityPump).GetMethod(
            nameof(UnityPump.Tick),
            BindingFlags.Static | BindingFlags.Public)
            ?? throw new InvalidOperationException($"{nameof(UnityPump.Tick)} is missing");

        var update = type.DefineMethod(
            "Update",
            MethodAttributes.Public | MethodAttributes.HideBySig,
            typeof(void),
            Type.EmptyTypes);

        var il = update.GetILGenerator();
        il.Emit(OpCodes.Call, tick);
        il.Emit(OpCodes.Ret);
    }

    /// <summary>
    /// Create a scene object that survives scene loads and attach the component.
    /// </summary>
    /// <remarks>
    /// <c>DontDestroyOnLoad</c> is the point of this method. Without it the pump
    /// dies the first time the player loads a save, and every mod silently stops
    /// updating at exactly the moment the game becomes interesting.
    /// </remarks>
    private static void Attach(Assembly unity, Type behaviour)
    {
        var gameObjectType = Resolve(unity, "UnityEngine.GameObject");
        var host = Activator.CreateInstance(gameObjectType, HostObjectName)
            ?? throw new InvalidOperationException("could not create the host GameObject");

        var dontDestroy = Resolve(unity, "UnityEngine.Object").GetMethod(
            "DontDestroyOnLoad",
            BindingFlags.Static | BindingFlags.Public)
            ?? throw new InvalidOperationException("Object.DontDestroyOnLoad not found");
        dontDestroy.Invoke(null, new[] { host });

        // AddComponent's il2cpp overload takes an Il2CppSystem.Type, which lives
        // in a generated assembly this project cannot reference. Il2CppType.From
        // returns exactly that type, so even calling it directly would not
        // compile - hence reflection, and `object` all the way to the invoke.
        var from = typeof(Il2CppType).GetMethod(
            "From",
            BindingFlags.Static | BindingFlags.Public,
            binder: null,
            types: new[] { typeof(Type) },
            modifiers: null)
            ?? throw new InvalidOperationException("Il2CppType.From(Type) not found");
        var componentType = from.Invoke(null, new object[] { behaviour })
            ?? throw new InvalidOperationException(
                $"il2cpp does not know {behaviour.Name} - injection did not take effect");

        var addComponent = gameObjectType.GetMethod(
            "AddComponent",
            BindingFlags.Instance | BindingFlags.Public,
            binder: null,
            types: new[] { componentType.GetType() },
            modifiers: null)
            ?? throw new InvalidOperationException(
                "GameObject.AddComponent(Il2CppSystem.Type) not found");
        addComponent.Invoke(host, new object[] { componentType });
    }
}
