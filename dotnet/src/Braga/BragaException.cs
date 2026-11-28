// SPDX-License-Identifier: Apache-2.0

namespace BragaNet;

/// <summary>A failing <c>braga_*</c> call.</summary>
/// <remarks>
/// <see cref="Code"/> is the ABI result discriminant. The exception message is the result code's
/// header name followed by the sanitized text from <c>braga_last_error</c>; it never contains key
/// material, file content, or query text.
/// </remarks>
public sealed class BragaException : Exception
{
    /// <summary>Creates an exception for a bare result code.</summary>
    public BragaException(BragaResult code)
        : this(code, string.Empty)
    {
    }

    /// <summary>Creates an exception for a result code and its sanitized message.</summary>
    public BragaException(BragaResult code, string message)
        : base(Render(code, message))
    {
        Code = code;
    }

    /// <summary>The ABI result discriminant of the failing call.</summary>
    public BragaResult Code { get; }

    private static string Render(BragaResult code, string message) =>
        string.IsNullOrEmpty(message) ? code.Name() : $"{code.Name()}: {message}";
}
