// SPDX-License-Identifier: GPL-2.0-only

using System;
using System.IO;
using System.Reflection;
using System.Runtime.InteropServices;
using System.Threading;

namespace Modrix.Loader;

/// <summary>
/// The single managed function the native proxy calls.
/// </summary>
/// <remarks>
/// <para>
/// The shape of <see cref="Start"/> is dictated by the host, not chosen:
/// <c>coreclr_create_delegate</c> builds a reverse-P/Invoke stub from the
/// method's own signature, so it must take zero parameters and return void.
/// Adding a parameter would not fail to compile - it would corrupt the stack at
/// call time, inside the game.
/// </para>
/// <para>
/// The type is <c>internal</c> on purpose, mirroring RedLoader's
/// <c>Doorstop.Entrypoint</c>: CoreCLR resolves the entry point reflectively and
/// does not require it to be public, and nothing else should ever call this.
/// </para>
/// <para>
/// For milestone M0 the body does exactly one thing - prove that managed code
/// ran inside the IL2CPP process - and then returns. No mod discovery, no
/// interop assemblies, no Harmony.
/// </para>
/// </remarks>
internal static class Entrypoint
{
    /// <summary>Prefix shared with the native side so the log reads as one stream.</summary>
    private const string Tag = "[modrix]";

    /// <summary>Log file name inside the loader directory.</summary>
    private const string LogFileName = "Latest.log";

    /// <summary>Attempts to make when the log file is briefly locked.</summary>
    private const int WriteAttempts = 5;

    /// <summary>Milliseconds between write attempts.</summary>
    private const int WriteBackoffMs = 20;

    /// <summary>
    /// Native entry point. Zero parameters, void return - non-negotiable.
    /// </summary>
    /// <remarks>
    /// Nothing may escape this method. An exception crossing back through the
    /// reverse-P/Invoke stub tears down the process, and the user sees the game
    /// close with no window, no dialog and no log - which is precisely the
    /// failure mode a loader must never have. Everything is therefore caught and
    /// written to a crash file.
    /// </remarks>
    public static void Start()
    {
        string loaderDirectory = ResolveLoaderDirectory();
        try
        {
            Report(loaderDirectory);
        }
        catch (Exception error)
        {
            WriteCrashFile(loaderDirectory, error);
        }
    }

    /// <summary>Writes the milestone line and the diagnostics behind it.</summary>
    private static void Report(string loaderDirectory)
    {
        string logPath = Path.Combine(loaderDirectory, LogFileName);

        // The acceptance line. Everything else in this method is context for
        // when it does NOT appear.
        Append(logPath, $"managed entrypoint reached: Modrix.Loader {Version()}");

        Append(logPath, $"managed runtime = {RuntimeInformation.FrameworkDescription}");
        Append(logPath, $"managed base directory = {AppContext.BaseDirectory}");

        // Echoed back so a mismatch between what the proxy published and what
        // the runtime sees - a real risk under Wine - is visible in one place.
        foreach (string name in new[]
                 {
                     "MODRIX_INITIALIZED",
                     "MODRIX_PROCESS_PATH",
                     "MODRIX_GAME_ROOT",
                     "MODRIX_LOADER_DIR",
                     "MODRIX_GAME_ASSEMBLY",
                 })
        {
            string value = Environment.GetEnvironmentVariable(name) ?? "<unset>";
            Append(logPath, $"env {name} = {value}");
        }

        LoadMods(logPath);
    }

    /// <summary>Discovers and initializes the mods in the game's Mods folder.</summary>
    /// <remarks>
    /// Wrapped so that a failure here still leaves the milestone lines above in
    /// the log: knowing the runtime started but mod loading threw is far more
    /// actionable than an empty file.
    /// </remarks>
    private static void LoadMods(string logPath)
    {
        string? gameRoot = Environment.GetEnvironmentVariable("MODRIX_GAME_ROOT");
        if (string.IsNullOrWhiteSpace(gameRoot))
        {
            Append(logPath, "WARN MODRIX_GAME_ROOT unset - cannot locate Mods");
            return;
        }

        void Log(string message) => Append(logPath, message);

        // Point the SDK's logger at our file before any mod runs, so a mod's
        // own RLog.Msg output lands in the same stream as the loader's and the
        // ordering between them is real.
        RedLoader.RLog.Sink = Log;

        // IL2CPP interop must be up BEFORE any mod is constructed. A mod's type
        // references game types, so the CLR resolves them while loading the mod
        // assembly - if the interop assemblies are not generated and resolvable
        // by then, every mod fails with TypeLoadException on its first game
        // type. Generation is cached; only the first launch after a game update
        // pays the cost.
        string? loaderDir = Environment.GetEnvironmentVariable("MODRIX_LOADER_DIR");
        if (string.IsNullOrWhiteSpace(loaderDir))
        {
            Append(logPath, "WARN MODRIX_LOADER_DIR unset - cannot initialize interop");
            return;
        }
        try
        {
            var interop = InteropManager.Initialize(gameRoot!, loaderDir!, Log);
            InteropManager.PreloadInteropAssemblies(interop, Log);
        }
        catch (Exception error)
        {
            // Without interop, mods that touch game types cannot work - but the
            // loader itself is fine, and reporting precisely why is far more
            // useful than a silent absence of mods.
            Append(logPath, $"ERROR interop initialization failed: {ModHost.Describe(error)}");
            return;
        }

        // The gate is armed FIRST, before the pump. Both are Harmony patches made
        // from this thread, but attaching the pump costs over a second, and the
        // gate can only catch scene changes that happen after it is installed -
        // so arming it first buys that second back as earlier main-thread access.
        //
        // It does NOT buy the splash. Measured on this machine: the managed
        // entrypoint runs at 1.2s and the gate arms at 3.3s, while the game's
        // first scene is already active before either. There is no way to reach
        // the boot sequence from inside the process, and an attempt to trim its
        // dwell times from the loader was removed after measuring it save
        // nothing. The splash is not idle time anyway - vanilla reaches the title
        // at 12.1s and the loader at 12.3s, so those seconds are the game loading,
        // not waiting. What IS worth having is starting a save load the moment
        // the title exists (~7s) rather than when the menu finishes building
        // (~12s); SonsBridge's SONSBRIDGE_AUTOLOAD does that, and it is worth
        // about twenty seconds end to end.
        try
        {
            MainThreadGate.Open(() => StartOnMainThread(gameRoot!, Log), Log);
        }
        catch (Exception error)
        {
            Append(logPath, $"ERROR main-thread gate failed: {ModHost.Describe(error)}");
            return;
        }

        try
        {
            UnityPump.Start(Log);
        }
        catch (Exception error)
        {
            Append(logPath, $"ERROR per-frame pump failed: {ModHost.Describe(error)}");
            return;
        }

    }

    /// <summary>
    /// The rest of startup, running on the Unity main thread.
    /// </summary>
    /// <remarks>
    /// Ordering is deliberate. The pump goes in first, so a mod's own per-frame
    /// callback is already being driven by the time the mod exists. Mods load
    /// second. The gate is torn down from the first pump tick rather than here,
    /// because a detour cannot safely be undone from inside itself - and this
    /// method is running inside it.
    /// </remarks>
    private static void StartOnMainThread(string gameRoot, Action<string> log)
    {
        // The pump was armed before the gate; Start is idempotent, so this is
        // only a guard for the case where arming failed earlier.
        try
        {
            UnityPump.Start(log);
        }
        catch (Exception error)
        {
            log($"ERROR per-frame pump failed: {ModHost.Describe(error)}");
        }

        try
        {
            var mods = ModHost.LoadAll(Path.Combine(gameRoot, "Mods"), log);
            Lifecycle.Attach(mods, log);
        }
        catch (Exception error)
        {
            log($"ERROR mod loading failed: {ModHost.Describe(error)}");
        }
    }

    /// <summary>
    /// Where <c>Latest.log</c> lives.
    /// </summary>
    /// <remarks>
    /// <c>MODRIX_LOADER_DIR</c> is authoritative because the native side already
    /// derived it from the process path; re-deriving it here would create two
    /// sources of truth that can disagree. The fallbacks exist only so that a
    /// missing environment block produces a log rather than silence.
    /// </remarks>
    private static string ResolveLoaderDirectory()
    {
        string? configured = Environment.GetEnvironmentVariable("MODRIX_LOADER_DIR");
        if (!string.IsNullOrWhiteSpace(configured))
        {
            return configured!;
        }

        string? processPath = Environment.GetEnvironmentVariable("MODRIX_PROCESS_PATH");
        string? gameRoot = string.IsNullOrWhiteSpace(processPath)
            ? null
            : Path.GetDirectoryName(processPath);
        if (!string.IsNullOrWhiteSpace(gameRoot))
        {
            return Path.Combine(gameRoot!, "_Modrix");
        }

        // Last resort: this assembly lives in `<game>/_Modrix/net6`, so the
        // loader directory is its parent.
        string baseDirectory = AppContext.BaseDirectory;
        string? parent = Path.GetDirectoryName(baseDirectory.TrimEnd(
            Path.DirectorySeparatorChar,
            Path.AltDirectorySeparatorChar));
        return parent ?? baseDirectory;
    }

    /// <summary>Informational version, without any build metadata suffix.</summary>
    private static string Version()
    {
        Assembly self = typeof(Entrypoint).Assembly;
        string? informational = self
            .GetCustomAttribute<AssemblyInformationalVersionAttribute>()
            ?.InformationalVersion;
        if (string.IsNullOrWhiteSpace(informational))
        {
            return self.GetName().Version?.ToString() ?? "0.0.0";
        }

        int plus = informational!.IndexOf('+');
        return plus < 0 ? informational : informational.Substring(0, plus);
    }

    /// <summary>
    /// Appends one tagged line, retrying briefly if the file is momentarily
    /// locked. Best-effort: failing to log must not become the failure.
    /// </summary>
    private static void Append(string logPath, string message)
    {
        string line = $"{Tag} {message}{Environment.NewLine}";
        for (int attempt = 0; attempt < WriteAttempts; attempt++)
        {
            try
            {
                Directory.CreateDirectory(Path.GetDirectoryName(logPath) ?? ".");
                File.AppendAllText(logPath, line);
                return;
            }
            catch (IOException)
            {
                Thread.Sleep(WriteBackoffMs);
            }
            catch (UnauthorizedAccessException)
            {
                return;
            }
        }
    }

    /// <summary>
    /// Records a failure to its own timestamped file.
    /// </summary>
    /// <remarks>
    /// A separate file rather than a line in <c>Latest.log</c>: whatever broke
    /// may be the log itself, and a crash report that survives a truncated log
    /// is worth more than a tidy one that does not.
    /// </remarks>
    private static void WriteCrashFile(string loaderDirectory, Exception error)
    {
        try
        {
            Directory.CreateDirectory(loaderDirectory);
            string stamp = DateTime.Now.ToString("yyyyMMdd_HHmmss_fff");
            string path = Path.Combine(loaderDirectory, $"crash_{stamp}.log");
            File.WriteAllText(
                path,
                $"Modrix.Loader {Version()} failed in Entrypoint.Start{Environment.NewLine}"
                + $"{RuntimeInformation.FrameworkDescription}{Environment.NewLine}"
                + $"{Environment.NewLine}{error}{Environment.NewLine}");
        }
        catch (Exception)
        {
            // There is nowhere left to report to. Returning quietly keeps the
            // game running, which is the only remaining useful outcome.
        }
    }
}
