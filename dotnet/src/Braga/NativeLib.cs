// SPDX-License-Identifier: Apache-2.0

using System.Runtime.InteropServices;

namespace BragaNet;

/// <summary>
/// Resolves, loads, and ABI-checks the <c>braga_ffi</c> cdylib, and copies its thread-local error
/// buffer into managed strings.
/// </summary>
/// <remarks>
/// The search order is <c>BRAGA_LIBRARY</c>, then <c>runtimes/&lt;rid&gt;/native/</c> next to the
/// application, then a walk up from the application directory to the repository root's
/// <c>target/release</c> (a development checkout). The resolved absolute path is loaded once and
/// registered as the assembly's DllImport resolver, so the generated imports never depend on the
/// dynamic linker's own search path.
/// </remarks>
internal static class NativeLib
{
    /// <summary>The <c>BRAGA_ABI_VERSION</c> this wrapper is written against.</summary>
    internal const uint ExpectedAbiVersion = 1;

    /// <summary>The RIDs the package declares; other runtimes fail fast.</summary>
    private static readonly string[] SupportedRids =
    [
        "win-x64",
        "osx-x64",
        "osx-arm64",
        "linux-x64",
        "linux-arm64",
    ];

    private static readonly string[] Profiles = ["release", "debug"];

    private static readonly Lazy<(nint Handle, uint AbiVersion)> Library =
        new(Load, LazyThreadSafetyMode.ExecutionAndPublication);

    /// <summary>The ABI version the loaded library implements.</summary>
    internal static uint AbiVersion => Library.Value.AbiVersion;

    /// <summary>Forces resolution and the ABI check before the first typed call.</summary>
    internal static void EnsureLoaded() => _ = Library.Value;

    /// <summary>
    /// Copies the most recent failure message. The buffer is thread-local, so this must run on the
    /// same thread, synchronously, immediately after the failing call. The handle is accepted for
    /// interface uniformity and never dereferenced.
    /// </summary>
    internal static unsafe string LastError(nint handle) =>
        Marshal.PtrToStringUTF8((nint)NativeMethods.braga_last_error(handle)) ?? string.Empty;

    private static (nint Handle, uint AbiVersion) Load()
    {
        string path = ResolvePath();
        nint handle = NativeLibrary.Load(path);
        NativeLibrary.SetDllImportResolver(
            typeof(NativeLib).Assembly,
            (name, _, _) => string.Equals(name, NativeMethods.Library, StringComparison.Ordinal)
                ? handle
                : 0);

        uint found = NativeMethods.braga_abi_version();
        if (found != ExpectedAbiVersion)
        {
            throw new InvalidOperationException(
                $"braga_ffi ABI version {found} does not match the expected {ExpectedAbiVersion}; "
                    + "the wrapper refuses to load a mismatched library");
        }

        return (handle, found);
    }

    private static string ResolvePath()
    {
        string rid = RuntimeInformation.RuntimeIdentifier;
        if (!SupportedRids.Contains(rid, StringComparer.Ordinal))
        {
            throw new PlatformNotSupportedException(
                $"braga supports {string.Join(", ", SupportedRids)}; this runtime is {rid}");
        }

        string? overridden = Environment.GetEnvironmentVariable("BRAGA_LIBRARY");
        if (!string.IsNullOrEmpty(overridden))
        {
            return Path.GetFullPath(overridden);
        }

        string name = LibraryFileName();
        string packaged = Path.Combine(AppContext.BaseDirectory, "runtimes", rid, "native", name);
        if (File.Exists(packaged))
        {
            return packaged;
        }

        for (DirectoryInfo? directory = new(AppContext.BaseDirectory);
             directory is not null;
             directory = directory.Parent)
        {
            foreach (string profile in Profiles)
            {
                string candidate = Path.Combine(directory.FullName, "target", profile, name);
                if (File.Exists(candidate))
                {
                    return candidate;
                }
            }
        }

        throw new DllNotFoundException(
            $"could not find {name}; build it with `cargo build --release -p braga-ffi` or set "
                + $"BRAGA_LIBRARY (looked under {packaged} and target/release above the application)");
    }

    private static string LibraryFileName()
    {
        if (OperatingSystem.IsMacOS())
        {
            return "libbraga_ffi.dylib";
        }

        if (OperatingSystem.IsWindows())
        {
            return "braga_ffi.dll";
        }

        if (OperatingSystem.IsLinux())
        {
            return "libbraga_ffi.so";
        }

        throw new PlatformNotSupportedException("braga supports macOS, Linux, and Windows");
    }
}
