// SPDX-License-Identifier: GPL-2.0-only

using System;
using System.Collections.Generic;
using System.Diagnostics;
using System.IO;
using System.IO.Compression;
using System.Linq;
using System.Net.Http;
using System.Reflection;
using System.Security.Cryptography;
using System.Text;
using System.Threading.Tasks;

using Cpp2IL.Core;
using Cpp2IL.Core.Api;
using Cpp2IL.Core.OutputFormats;
using Cpp2IL.Core.ProcessingLayers;
using Cpp2IL.Core.InstructionSets;
using Il2CppInterop.Generator;
using Il2CppInterop.Generator.Runners;
using Il2CppInterop.HarmonySupport;
using Il2CppInterop.Runtime.Startup;
using LibCpp2IL;

namespace Modrix.Loader;

/// <summary>
/// Builds and starts the IL2CPP interop layer.
/// </summary>
/// <remarks>
/// <para>
/// An IL2CPP game ships no managed assemblies, so a mod has nothing to call.
/// Bridging that needs two distinct artifacts, and conflating them is the
/// mistake that costs days:
/// </para>
/// <list type="bullet">
///   <item><description><b>Dummy assemblies</b> - types, fields and method
///     signatures with no bodies, reconstructed from `global-metadata.dat` by
///     Cpp2IL. These are enough to <em>compile</em> against and nothing
///     more: the CLR refuses to load a type whose method has no
///     RVA.</description></item>
///   <item><description><b>Interop assemblies</b> - the same shape, but with
///     real bodies that marshal through `il2cpp_runtime_invoke`. Il2CppInterop
///     generates these from the dummies, and they are what actually
///     <em>runs</em>.</description></item>
/// </list>
/// <para>
/// Generation happens once per game build and is cached against a hash of the
/// inputs, because it takes minutes. It runs at first launch rather than at
/// install time for one reason: the Unity version and the metadata are read out
/// of the live process, so the output is guaranteed to match the game actually
/// running rather than whatever was installed when the user last pressed a
/// button.
/// </para>
/// </remarks>
internal static class InteropManager
{
    /// <summary>Where Unity publishes the stripped base libraries per version.</summary>
    private const string UnityBaseLibrariesSource =
        "https://unity.bepinex.dev/libraries/{VERSION}.zip";

    /// <summary>Interop assemblies, relative to the loader directory.</summary>
    private const string InteropDirName = "interop";

    /// <summary>Unity base libraries, relative to the loader directory.</summary>
    private const string UnityLibsDirName = "unity-libs";

    /// <summary>Stamp recording the inputs the current output was built from.</summary>
    private const string HashFileName = "interop.hash";

    /// <summary>Resolved paths and the detected Unity version.</summary>
    internal sealed class Context
    {
        internal Context(string gameRoot, string loaderDir, Version unityVersion)
        {
            GameRoot = gameRoot;
            LoaderDir = loaderDir;
            UnityVersion = unityVersion;
        }

        internal string GameRoot { get; }
        internal string LoaderDir { get; }
        internal Version UnityVersion { get; }

        internal string GameAssemblyPath => Path.Combine(GameRoot, "GameAssembly.dll");
        internal string InteropDir => Path.Combine(LoaderDir, InteropDirName);
        internal string UnityLibsDir => Path.Combine(LoaderDir, UnityLibsDirName);
        internal string HashPath => Path.Combine(LoaderDir, HashFileName);

        /// <summary>`&lt;Game&gt;_Data/il2cpp_data/Metadata/global-metadata.dat`.</summary>
        internal string MetadataPath
        {
            get
            {
                foreach (var dir in Directory.EnumerateDirectories(GameRoot, "*_Data"))
                {
                    var candidate = Path.Combine(
                        dir, "il2cpp_data", "Metadata", "global-metadata.dat");
                    if (File.Exists(candidate))
                    {
                        return candidate;
                    }
                }
                throw new FileNotFoundException(
                    $"no il2cpp global-metadata.dat under any *_Data directory in {GameRoot}");
            }
        }
    }

    /// <summary>
    /// Generate the interop assemblies if needed, then start the runtime.
    /// </summary>
    /// <param name="gameRoot">The game's install directory.</param>
    /// <param name="loaderDir">`&lt;game&gt;/_Modrix`.</param>
    /// <param name="log">Progress sink.</param>
    /// <returns>The context, so callers can report paths.</returns>
    internal static Context Initialize(string gameRoot, string loaderDir, Action<string> log)
    {
        var unityVersion = DetectUnityVersion(gameRoot);
        var context = new Context(gameRoot, loaderDir, unityVersion);
        log($"unity version = {unityVersion}");

        // Before anything patches: Harmony's code generation must not fall back
        // to MonoMod's Cecil backend, which cannot resolve corlib from here.
        DetourSetup.Configure(log);

        EnsureInteropAssemblies(context, log);
        RegisterAssemblyResolver(context, log);
        StartRuntime(context, log);
        return context;
    }

    /// <summary>
    /// Read the Unity version out of the game's own data.
    /// </summary>
    /// <remarks>
    /// `globalgamemanagers` begins with a serialized-file header whose version
    /// string is plain ASCII, so this needs no Unity API and works before
    /// anything is initialized. The version selects which stripped base
    /// libraries to fetch, and a mismatch there produces interop assemblies
    /// that reference types the game does not have.
    /// </remarks>
    private static Version DetectUnityVersion(string gameRoot)
    {
        foreach (var dir in Directory.EnumerateDirectories(gameRoot, "*_Data"))
        {
            var path = Path.Combine(dir, "globalgamemanagers");
            if (!File.Exists(path))
            {
                continue;
            }
            var head = new byte[4096];
            using var stream = File.OpenRead(path);
            var read = stream.Read(head, 0, head.Length);
            var found = ScanVersion(head, read);
            if (found is not null)
            {
                return found;
            }
        }
        throw new InvalidOperationException(
            $"could not determine the Unity version from any *_Data/globalgamemanagers in {gameRoot}");
    }

    /// <summary>Find the first `N.N.NfN`-shaped ASCII run in a buffer.</summary>
    private static Version? ScanVersion(byte[] buffer, int length)
    {
        var text = Encoding.ASCII.GetString(buffer, 0, Math.Max(length, 0));
        var match = System.Text.RegularExpressions.Regex.Match(
            text, @"(\d+)\.(\d+)\.(\d+)[fpab]\d+");
        if (!match.Success)
        {
            return null;
        }
        return new Version(
            int.Parse(match.Groups[1].Value),
            int.Parse(match.Groups[2].Value),
            int.Parse(match.Groups[3].Value));
    }

    /// <summary>Regenerate the interop assemblies unless the cache is current.</summary>
    private static void EnsureInteropAssemblies(Context context, Action<string> log)
    {
        var expected = ComputeHash(context);
        if (File.Exists(context.HashPath)
            && File.ReadAllText(context.HashPath).Trim() == expected
            && Directory.Exists(context.InteropDir)
            && Directory.EnumerateFiles(context.InteropDir, "*.dll").Any())
        {
            log("interop assemblies are current");
            return;
        }

        log("generating interop assemblies (first run for this game build; takes a few minutes)");
        var stopwatch = Stopwatch.StartNew();

        Directory.CreateDirectory(context.InteropDir);
        foreach (var stale in Directory.EnumerateFiles(context.InteropDir, "*.dll"))
        {
            File.Delete(stale);
        }

        DownloadUnityLibs(context, log);
        var dummies = RunCpp2Il(context, log);
        RunGenerator(context, dummies, log);

        File.WriteAllText(context.HashPath, expected);
        log($"interop assemblies generated in {stopwatch.Elapsed}");
    }

    /// <summary>
    /// What the current output was built from.
    /// </summary>
    /// <remarks>
    /// Hashing the metadata and the binary means a game update invalidates the
    /// cache automatically. Without this, a patch silently leaves mods compiled
    /// and running against the previous build's type layout.
    /// </remarks>
    private static string ComputeHash(Context context)
    {
        using var sha = SHA256.Create();
        var parts = new List<string>
        {
            context.UnityVersion.ToString(),
            FileFingerprint(context.MetadataPath),
            FileFingerprint(context.GameAssemblyPath),
        };
        var joined = string.Join("|", parts);
        return Convert.ToHexString(sha.ComputeHash(Encoding.UTF8.GetBytes(joined)));
    }

    /// <summary>
    /// Size plus modification time, rather than a full content hash.
    /// </summary>
    /// <remarks>
    /// `GameAssembly.dll` is 100 MB; hashing it on every launch would add
    /// seconds to startup to detect something size+mtime already catches. A
    /// game update always changes both.
    /// </remarks>
    private static string FileFingerprint(string path)
    {
        var info = new FileInfo(path);
        return $"{info.Name}:{info.Length}:{info.LastWriteTimeUtc.Ticks}";
    }

    /// <summary>Fetch the stripped Unity base libraries for this version.</summary>
    private static void DownloadUnityLibs(Context context, Action<string> log)
    {
        if (Directory.Exists(context.UnityLibsDir)
            && Directory.EnumerateFiles(context.UnityLibsDir, "*.dll").Any())
        {
            log("unity base libraries already present");
            return;
        }

        var version =
            $"{context.UnityVersion.Major}.{context.UnityVersion.Minor}.{context.UnityVersion.Build}";
        var source = UnityBaseLibrariesSource.Replace("{VERSION}", version);
        Directory.CreateDirectory(context.UnityLibsDir);

        log($"downloading unity base libraries {version}");
        using var client = new HttpClient();
        client.Timeout = TimeSpan.FromMinutes(5);

        // Buffered rather than streamed into ZipArchive: a zip's central
        // directory is at the END of the file, so reading one requires seeking,
        // and an HTTP response stream is forward-only. Handing it straight to
        // ZipArchive throws "The stream must support seeking". The archive is a
        // few tens of MB, so buffering it is the simple correct answer.
        var payload = client.GetByteArrayAsync(source).GetAwaiter().GetResult();
        using var buffer = new MemoryStream(payload, writable: false);
        using var archive = new ZipArchive(buffer, ZipArchiveMode.Read);
        archive.ExtractToDirectory(context.UnityLibsDir, overwriteFiles: true);
        log($"unity base libraries extracted ({payload.Length / (1024 * 1024)} MB)");
    }

    /// <summary>
    /// Populate Cpp2IL's plugin registries.
    /// </summary>
    /// <remarks>
    /// <para>
    /// Cpp2IL is built as a plugin host: its instruction-set decoders, binary
    /// format readers, processing layers and output formats all live behind
    /// registries that are empty until something fills them. Its own console
    /// frontend does that during startup - a library consumer gets nothing, and
    /// the first symptom is <c>InstructionSetHandlerNotRegisteredException:
    /// Instruction set handler not registered for instruction set x86_64</c>
    /// thrown from deep inside the application-context constructor.
    /// </para>
    /// <para>
    /// Every set is registered, not just x86-64. It costs nothing, and it is
    /// what makes this work unchanged on an ARM64 macOS build of the game rather
    /// than only on the x86-64 one in front of us.
    /// </para>
    /// <para>
    /// Registration is process-global and idempotent per registry, but calling
    /// it twice throws on a duplicate key, so it is guarded by a flag: interop
    /// generation can be re-entered when a mod is hot-reloaded.
    /// </para>
    /// </remarks>
    private static bool _handlersRegistered;

    private static void RegisterCpp2IlHandlers()
    {
        if (_handlersRegistered)
        {
            return;
        }
        _handlersRegistered = true;

        InstructionSetRegistry.RegisterInstructionSet<X86InstructionSet>(
            DefaultInstructionSets.X86_32);
        InstructionSetRegistry.RegisterInstructionSet<X86InstructionSet>(
            DefaultInstructionSets.X86_64);
        InstructionSetRegistry.RegisterInstructionSet<WasmInstructionSet>(
            DefaultInstructionSets.WASM);
        InstructionSetRegistry.RegisterInstructionSet<ArmV7InstructionSet>(
            DefaultInstructionSets.ARM_V7);
        InstructionSetRegistry.RegisterInstructionSet<NewArmV8InstructionSet>(
            DefaultInstructionSets.ARM_V8);

        // ELF/PE/Mach-O readers. Without these, loading the binary fails before
        // instruction decoding is ever reached.
        LibCpp2IlBinaryRegistry.RegisterBuiltInBinarySupport();
    }

    /// <summary>
    /// Reconstruct dummy assemblies from the game's metadata.
    /// </summary>
    /// <remarks>
    /// The `AttributeInjectorProcessingLayer` is not optional: it injects the
    /// attributes Il2CppInterop reads to correlate a managed member back to its
    /// native address. Without it the generator produces assemblies whose
    /// methods have nothing to call.
    /// </remarks>
    private static List<AsmResolver.DotNet.AssemblyDefinition> RunCpp2Il(
        Context context,
        Action<string> log)
    {
        Cpp2IL.Core.Logging.Logger.WarningLog += (message, source) =>
            log($"  cpp2il/{source}: {message.Trim()}");
        Cpp2IL.Core.Logging.Logger.ErrorLog += (message, source) =>
            log($"  cpp2il/{source} ERROR: {message.Trim()}");

        RegisterCpp2IlHandlers();

        log("running Cpp2IL over the game metadata");
        // Cpp2IL speaks AssetRipper's UnityVersion; Il2CppInterop speaks
        // System.Version. Convert at the boundary rather than letting either
        // type leak through this class's own API.
        var cpp2IlVersion = new AssetRipper.Primitives.UnityVersion(
            (ushort)context.UnityVersion.Major,
            (ushort)context.UnityVersion.Minor,
            (ushort)context.UnityVersion.Build);
        Cpp2IlApi.InitializeLibCpp2Il(
            context.GameAssemblyPath,
            context.MetadataPath,
            cpp2IlVersion,
            false);

        var appContext = Cpp2IlApi.CurrentAppContext
            ?? throw new InvalidOperationException(
                "Cpp2IL initialized without producing an application context");

        var layers = new List<Cpp2IlProcessingLayer> { new AttributeInjectorProcessingLayer() };
        foreach (var layer in layers)
        {
            layer.PreProcess(appContext, layers);
        }
        foreach (var layer in layers)
        {
            layer.Process(appContext);
        }

        var assemblies = new AsmResolverDllOutputFormatDefault().BuildAssemblies(appContext);

        // Release the 100 MB mapping before the generator starts allocating.
        LibCpp2IlMain.Reset();
        Cpp2IlApi.CurrentAppContext = null;

        log($"Cpp2IL produced {assemblies.Count} dummy assemblies");
        return assemblies;
    }

    /// <summary>Turn the dummies into runnable interop assemblies.</summary>
    private static void RunGenerator(
        Context context,
        List<AsmResolver.DotNet.AssemblyDefinition> dummies,
        Action<string> log)
    {
        log("generating interop assemblies");
        var options = new GeneratorOptions
        {
            GameAssemblyPath = context.GameAssemblyPath,
            Source = dummies,
            OutputDir = context.InteropDir,
            UnityBaseLibsDir = Directory.Exists(context.UnityLibsDir)
                ? context.UnityLibsDir
                : null,
        };

        // No .AddLogger: the generator's logging extension lives on a host type
        // whose shape differs across Il2CppInterop versions, and this call is
        // already bracketed by our own start/finish/duration lines. Failures
        // surface as an exception, which the caller reports.
        Il2CppInteropGenerator.Create(options)
            .AddInteropAssemblyGenerator()
            .Run();
    }

    /// <summary>
    /// Resolve interop assemblies by name.
    /// </summary>
    /// <remarks>
    /// A mod references `UnityEngine.CoreModule` and `Sons` by simple name.
    /// Those live in the interop directory, which is not an application probing
    /// path, so without this hook every mod fails with
    /// `FileNotFoundException: Could not load file or assembly 'Sons'` the
    /// instant it touches a game type.
    /// </remarks>
    private static void RegisterAssemblyResolver(Context context, Action<string> log)
    {
        AppDomain.CurrentDomain.AssemblyResolve += (_, args) =>
        {
            var name = new AssemblyName(args.Name).Name;
            if (string.IsNullOrEmpty(name))
            {
                return null;
            }
            var candidate = Path.Combine(context.InteropDir, name + ".dll");
            if (!File.Exists(candidate))
            {
                return null;
            }
            try
            {
                return Assembly.LoadFrom(candidate);
            }
            catch (Exception error)
            {
                log($"WARN could not load interop assembly {name}: {error.Message}");
                return null;
            }
        };
        log($"interop resolver registered for {context.InteropDir}");
    }

    /// <summary>
    /// Start Il2CppInterop.
    /// </summary>
    /// <remarks>
    /// `AddHarmonySupport` is what lets a mod's `[HarmonyPatch]` classes target
    /// IL2CPP methods; without it Harmony can only patch managed code, which in
    /// this process is just the loader itself.
    /// </remarks>
    private static void StartRuntime(Context context, Action<string> log)
    {
        log("starting Il2CppInterop runtime");
        // No DetourProvider is supplied. Il2CppInterop only needs one for
        // ClassInjector - registering a managed MonoBehaviour type with IL2CPP -
        // and nothing here does that: the bridge drives itself from the SDK's
        // per-frame callback rather than injecting a component. Calling game
        // methods and reading fields needs no detours at all. If class
        // injection is wanted later, this is where an IDetourProvider goes.
        Il2CppInteropRuntime.Create(new RuntimeConfiguration
        {
            UnityVersion = context.UnityVersion,

            // Required, despite being nullable. See Il2CppDetourProvider: with
            // this left null the runtime starts fine and the first Harmony patch
            // of an IL2CPP method throws NullReferenceException from inside
            // Il2CppInterop, with nothing in the stack naming this property.
            DetourProvider = new Il2CppDetourProvider(),
        })
        .AddHarmonySupport()
        .Start();
        log("Il2CppInterop runtime started");
    }

    /// <summary>Preload every interop assembly so first use is not a stall.</summary>
    internal static int PreloadInteropAssemblies(Context context, Action<string> log)
    {
        var loaded = 0;
        var files = Directory.EnumerateFiles(context.InteropDir, "*.dll").ToList();
        Parallel.ForEach(files, file =>
        {
            // netstandard facades collide with the host's own and are resolved
            // by the runtime, not by us.
            var name = Path.GetFileNameWithoutExtension(file);
            if (name.EndsWith("netstandard", StringComparison.OrdinalIgnoreCase))
            {
                return;
            }
            try
            {
                Assembly.LoadFrom(file);
                System.Threading.Interlocked.Increment(ref loaded);
            }
            catch
            {
                // A single unloadable interop assembly is not fatal; the ones a
                // mod actually needs will surface their own error on use.
            }
        });
        log($"preloaded {loaded} of {files.Count} interop assemblies");
        return loaded;
    }
}
