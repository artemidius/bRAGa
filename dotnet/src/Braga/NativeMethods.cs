// SPDX-License-Identifier: Apache-2.0

using System.Runtime.InteropServices;

namespace BragaNet;

/// <summary>
/// Source-generated P/Invoke declarations for the frozen C ABI in
/// <c>crates/braga-ffi/include/braga.h</c> (version 1).
/// </summary>
/// <remarks>
/// Every string crosses as a UTF-8 byte pointer plus an explicit length; lengths are authoritative.
/// Handles stay opaque <see cref="nint"/> values here and become a <see cref="BragaSafeHandle"/> only
/// at the typed layer. The library is resolved by name and pinned by <see cref="NativeLib"/>.
/// </remarks>
internal static unsafe partial class NativeMethods
{
    /// <summary>The native library name the DllImport resolver maps to the resolved cdylib.</summary>
    internal const string Library = "braga_ffi";

    [LibraryImport(Library)]
    internal static partial uint braga_abi_version();

    [LibraryImport(Library)]
    internal static partial int braga_open(
        byte* config_path,
        nuint config_path_len,
        byte* key_path,
        nuint key_path_len,
        nint* out_handle);

    [LibraryImport(Library)]
    internal static partial int braga_close(nint handle);

    [LibraryImport(Library)]
    internal static partial int braga_status(nint handle);

    [LibraryImport(Library)]
    internal static partial byte* braga_last_error(nint handle);

    [LibraryImport(Library)]
    internal static partial int braga_update(nint handle, byte* key, nuint key_len);

    [LibraryImport(Library)]
    internal static partial int braga_query(
        nint handle,
        byte* text,
        nuint text_len,
        uint top_k,
        byte* key,
        nuint key_len,
        byte** out_json,
        nuint* out_json_len);

    [LibraryImport(Library)]
    internal static partial int braga_doctor(nint handle, byte** out_json, nuint* out_json_len);

    [LibraryImport(Library)]
    internal static partial int braga_init(
        byte* config_path,
        nuint config_path_len,
        byte* key_path,
        nuint key_path_len);

    [LibraryImport(Library)]
    internal static partial void braga_string_free(byte* ptr);
}
