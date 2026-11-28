// SPDX-License-Identifier: Apache-2.0

namespace BragaNet;

/// <summary>The reconciler state reported by <c>braga_status</c>.</summary>
/// <remarks>
/// The discriminants are the frozen ABI values (<c>braga.h</c> 0..3).
/// <see cref="BragaStatusExtensions.FromCode"/> fails closed to <see cref="Error"/>, matching the
/// library's own mapping of out-of-range atomic values.
/// </remarks>
public enum BragaStatus
{
    /// <summary>The index matches the scan directory; no update is in flight.</summary>
    UpToDate = 0,

    /// <summary>An update is in flight and nothing has been written yet.</summary>
    Checking = 1,

    /// <summary>An update is in flight and writing/embedding.</summary>
    Updating = 2,

    /// <summary>The last open or update failed.</summary>
    Error = 3,
}

/// <summary>Helpers for <see cref="BragaStatus"/>.</summary>
public static class BragaStatusExtensions
{
    /// <summary>
    /// Decodes a raw status value, failing closed to <see cref="BragaStatus.Error"/> for anything
    /// outside the header's 0..3 range.
    /// </summary>
    public static BragaStatus FromCode(int code) =>
        code is >= (int)BragaStatus.UpToDate and <= (int)BragaStatus.Error
            ? (BragaStatus)code
            : BragaStatus.Error;

    /// <summary>Renders the header's constant name, for diagnostics.</summary>
    public static string Name(this BragaStatus status) => status switch
    {
        BragaStatus.UpToDate => "UP_TO_DATE",
        BragaStatus.Checking => "CHECKING",
        BragaStatus.Updating => "UPDATING",
        BragaStatus.Error => "ERROR",
        _ => "UNKNOWN",
    };
}
