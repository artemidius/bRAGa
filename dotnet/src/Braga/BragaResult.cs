// SPDX-License-Identifier: Apache-2.0

namespace BragaNet;

/// <summary>The result code returned by every fallible <c>braga_*</c> entry point.</summary>
/// <remarks>
/// The discriminants are the frozen ABI values (<c>braga.h</c> 0..9); a wrapper maps them directly
/// and never renumbers them. <see cref="BragaResultExtensions.FromCode"/> fails closed to
/// <see cref="Panic"/> for a code this version does not know.
/// </remarks>
public enum BragaResult
{
    /// <summary>The call succeeded.</summary>
    Ok = 0,

    /// <summary>Null pointer, bad UTF-8, bad length, or null out-parameter.</summary>
    InvalidArg = 1,

    /// <summary>Configuration resolution or validation failed.</summary>
    Config = 2,

    /// <summary>Key file load failed.</summary>
    Key = 3,

    /// <summary>Storage error: wrong key, I/O, or a numeric SQLite code.</summary>
    Store = 4,

    /// <summary>Embedder build or provider call failed.</summary>
    Embed = 5,

    /// <summary>Nothing embedded for the configured model.</summary>
    NotFound = 6,

    /// <summary>I/O error outside the store.</summary>
    Io = 7,

    /// <summary>A Rust panic was caught inside the library.</summary>
    Panic = 8,

    /// <summary>Allocation or JSON serialization failed.</summary>
    Oom = 9,
}

/// <summary>Helpers for <see cref="BragaResult"/>.</summary>
public static class BragaResultExtensions
{
    /// <summary>
    /// Decodes a raw ABI result code, failing closed to <see cref="BragaResult.Panic"/> for anything
    /// outside the header's 0..9 range.
    /// </summary>
    public static BragaResult FromCode(int code) =>
        code is >= (int)BragaResult.Ok and <= (int)BragaResult.Oom
            ? (BragaResult)code
            : BragaResult.Panic;

    /// <summary>Renders the header's constant name, for diagnostics.</summary>
    public static string Name(this BragaResult result) => result switch
    {
        BragaResult.Ok => "OK",
        BragaResult.InvalidArg => "INVALID_ARG",
        BragaResult.Config => "CONFIG",
        BragaResult.Key => "KEY",
        BragaResult.Store => "STORE",
        BragaResult.Embed => "EMBED",
        BragaResult.NotFound => "NOT_FOUND",
        BragaResult.Io => "IO",
        BragaResult.Panic => "PANIC",
        BragaResult.Oom => "OOM",
        _ => "UNKNOWN",
    };
}
