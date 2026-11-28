// SPDX-License-Identifier: Apache-2.0

using System.Text.Json.Serialization;

namespace BragaNet;

/// <summary>One check in the doctor report, in the order the library ran it.</summary>
/// <param name="Name">The check's stable name.</param>
/// <param name="Passed">Whether the check passed.</param>
/// <param name="Detail">A sanitized explanation of the outcome.</param>
public sealed record DoctorCheck(
    [property: JsonPropertyName("name")] string Name,
    [property: JsonPropertyName("passed")] bool Passed,
    [property: JsonPropertyName("detail")] string Detail);
