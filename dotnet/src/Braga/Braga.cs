// SPDX-License-Identifier: Apache-2.0

using System.Text;
using System.Text.Json;

namespace BragaNet;

/// <summary>An open bRAGa index handle and its typed query surface.</summary>
/// <remarks>
/// The handle owns one <c>braga_handle *</c> and frees it exactly once in <see cref="Dispose"/>;
/// disposing twice is a no-op and every method reports <see cref="BragaResult.InvalidArg"/> after
/// that (except <see cref="Status"/>, which fails closed to <see cref="BragaStatus.Error"/>). A
/// handle is <b>not</b> thread-safe: use it from one thread at a time, as <c>braga.h</c> documents.
/// The reserved key slots of <c>braga_update</c>/<c>braga_query</c> stay <c>NULL/0</c> in base mode.
/// </remarks>
public sealed class Braga : IDisposable
{
    private static readonly UTF8Encoding StrictUtf8 = new(
        encoderShouldEmitUTF8Identifier: false,
        throwOnInvalidBytes: true);

    private readonly BragaSafeHandle _handle;
    private bool _disposed;

    private Braga(BragaSafeHandle handle)
    {
        _handle = handle;
    }

    /// <summary>The ABI version the loaded library implements.</summary>
    /// <exception cref="DllNotFoundException">The cdylib could not be found.</exception>
    /// <exception cref="InvalidOperationException">The library's ABI version mismatched.</exception>
    public static uint AbiVersion() => NativeLib.AbiVersion;

    /// <summary>
    /// Opens the configured index and key.
    /// </summary>
    /// <param name="configPath">
    /// The TOML path; <see langword="null"/> or empty defers to <c>BRAGA_TOML</c>, <c>BRAGA_CONFIG</c>,
    /// the <c>BRAGA_*__*</c> environment variables, and the built-in defaults.
    /// </param>
    /// <param name="keyPath">
    /// The key path; <see langword="null"/> or empty defers to <c>BRAGA_KEY_FILE</c> and then
    /// <c>~/.braga/key</c>.
    /// </param>
    /// <returns>A live handle that must be disposed.</returns>
    /// <exception cref="BragaException">The library reported a non-OK result.</exception>
    public static Braga Open(string? configPath = null, string? keyPath = null)
    {
        NativeLib.EnsureLoaded();
        byte[]? config = EncodePath(configPath);
        byte[]? key = EncodePath(keyPath);
        nint opened = 0;
        BragaResult result;
        unsafe
        {
            fixed (byte* configPtr = config)
            fixed (byte* keyPtr = key)
            {
                result = BragaResultExtensions.FromCode(
                    NativeMethods.braga_open(
                        configPtr,
                        (nuint)(config?.Length ?? 0),
                        keyPtr,
                        (nuint)(key?.Length ?? 0),
                        &opened));
            }
        }

        if (result != BragaResult.Ok)
        {
            throw Failure(result);
        }

        if (opened == 0)
        {
            throw new BragaException(BragaResult.Panic, "braga_open reported OK without a handle");
        }

        return new Braga(new BragaSafeHandle(opened));
    }

    /// <summary>
    /// Writes the default <c>braga.toml</c> and a fresh key file, refusing to overwrite either
    /// existing target.
    /// </summary>
    /// <param name="configPath">The TOML path; <see langword="null"/> or empty writes <c>braga.toml</c> in the current directory.</param>
    /// <param name="keyPath">The key path; <see langword="null"/> or empty follows <see cref="Open"/>'s key resolution.</param>
    /// <exception cref="BragaException">The library reported a non-OK result.</exception>
    public static void Init(string? configPath = null, string? keyPath = null)
    {
        NativeLib.EnsureLoaded();
        byte[]? config = EncodePath(configPath);
        byte[]? key = EncodePath(keyPath);
        BragaResult result;
        unsafe
        {
            fixed (byte* configPtr = config)
            fixed (byte* keyPtr = key)
            {
                result = BragaResultExtensions.FromCode(
                    NativeMethods.braga_init(
                        configPtr,
                        (nuint)(config?.Length ?? 0),
                        keyPtr,
                        (nuint)(key?.Length ?? 0)));
            }
        }

        if (result != BragaResult.Ok)
        {
            throw Failure(result);
        }
    }

    /// <summary>The raw ABI handle, exposed to the conformance tests only.</summary>
    internal nint NativeHandle => _handle.DangerousGetHandle();

    /// <summary>Reports the reconciler state left by the last open or update.</summary>
    /// <returns><see cref="BragaStatus.Error"/> on a disposed handle, like a NULL handle in the ABI.</returns>
    public BragaStatus Status()
    {
        if (_disposed)
        {
            return BragaStatus.Error;
        }

        return BragaStatusExtensions.FromCode(NativeMethods.braga_status(NativeHandle));
    }

    /// <summary>
    /// Reconciles the scan directory and embeds every chunk missing a vector, blocking until the
    /// index is queryable.
    /// </summary>
    /// <exception cref="BragaException">The library reported a non-OK result.</exception>
    public void Update()
    {
        EnsureOpen();
        BragaResult result;
        unsafe
        {
            // The reserved key slot stays NULL in base mode; braga-extra makes it live without an ABI break.
            result = BragaResultExtensions.FromCode(NativeMethods.braga_update(NativeHandle, null, 0));
        }

        if (result != BragaResult.Ok)
        {
            throw Failure(result);
        }
    }

    /// <summary>Returns the top <paramref name="topK"/> hits for <paramref name="text"/>.</summary>
    /// <param name="text">Non-empty, valid UTF-8 query text.</param>
    /// <param name="topK">The number of hits to request; the value must fit in a uint32.</param>
    /// <returns>The hits; an empty index is <see cref="BragaResult.NotFound"/>, never an empty success.</returns>
    /// <exception cref="BragaException">The library reported a non-OK result.</exception>
    public IReadOnlyList<Hit> Query(string text, int topK = 10)
    {
        EnsureOpen();
        if (string.IsNullOrEmpty(text))
        {
            throw new BragaException(BragaResult.InvalidArg, "text must be non-empty valid UTF-8");
        }

        if (topK < 0)
        {
            throw new BragaException(BragaResult.InvalidArg, "topK must fit in a uint32");
        }

        byte[] payload = EncodeStrict(text, "text");
        nint json = 0;
        nuint length = 0;
        BragaResult result;
        unsafe
        {
            fixed (byte* textPtr = payload)
            {
                result = BragaResultExtensions.FromCode(
                    NativeMethods.braga_query(
                        NativeHandle,
                        textPtr,
                        (nuint)payload.Length,
                        (uint)topK,
                        null,
                        0,
                        (byte**)&json,
                        &length));
            }
        }

        if (result != BragaResult.Ok)
        {
            throw Failure(result);
        }

        return ParseJson<Hit[]>(json, length);
    }

    /// <summary>Runs the claims verifier and returns its typed report.</summary>
    /// <exception cref="BragaException">The library reported a non-OK result.</exception>
    public DoctorReport Doctor()
    {
        EnsureOpen();
        nint json = 0;
        nuint length = 0;
        BragaResult result;
        unsafe
        {
            result = BragaResultExtensions.FromCode(
                NativeMethods.braga_doctor(NativeHandle, (byte**)&json, &length));
        }

        if (result != BragaResult.Ok)
        {
            throw Failure(result);
        }

        return ParseJson<DoctorReport>(json, length);
    }

    /// <summary>Frees the native handle. Idempotent: a second call is a no-op.</summary>
    public void Dispose()
    {
        if (_disposed)
        {
            return;
        }

        _disposed = true;
        _handle.Dispose();
    }

    private static BragaException Failure(BragaResult code) => new(code, NativeLib.LastError(0));

    private void EnsureOpen()
    {
        if (_disposed)
        {
            throw new BragaException(BragaResult.InvalidArg, "handle is closed");
        }
    }

    private static byte[]? EncodePath(string? path) =>
        string.IsNullOrEmpty(path) ? null : EncodeStrict(path, "path");

    private static byte[] EncodeStrict(string value, string what)
    {
        try
        {
            return StrictUtf8.GetBytes(value);
        }
        catch (EncoderFallbackException)
        {
            throw new BragaException(BragaResult.InvalidArg, $"{what} must be valid UTF-8");
        }
    }

    /// <summary>
    /// Decodes the library's JSON output, reading exactly <paramref name="length"/> bytes so embedded
    /// NULs survive, and frees the buffer on every path.
    /// </summary>
    private static unsafe T ParseJson<T>(nint json, nuint length)
    {
        try
        {
            if (json == 0)
            {
                throw new BragaException(BragaResult.Panic, "the library returned no JSON");
            }

            ReadOnlySpan<byte> payload = new((void*)json, checked((int)length));
            return JsonSerializer.Deserialize<T>(payload)
                ?? throw new BragaException(BragaResult.Panic, "the library returned empty JSON");
        }
        finally
        {
            NativeMethods.braga_string_free((byte*)json);
        }
    }
}
