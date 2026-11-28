// SPDX-License-Identifier: Apache-2.0

using Microsoft.Win32.SafeHandles;

namespace BragaNet;

/// <summary>
/// Owns one <c>braga_handle *</c> and frees it with <c>braga_close</c> exactly once, whether through
/// <see cref="System.IDisposable.Dispose"/> or the finalizer.
/// </summary>
internal sealed class BragaSafeHandle : SafeHandleZeroOrMinusOneIsInvalid
{
    internal BragaSafeHandle(nint handle)
        : base(ownsHandle: true)
    {
        SetHandle(handle);
    }

    /// <summary>Releases the native handle; an already-invalid handle is a no-op.</summary>
    protected override bool ReleaseHandle()
    {
        if (IsInvalid)
        {
            return true;
        }

        return NativeMethods.braga_close(handle) == (int)BragaResult.Ok;
    }
}
