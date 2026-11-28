// SPDX-License-Identifier: Apache-2.0

using System.Text.Json.Serialization;

namespace BragaNet;

/// <summary>The claims verifier's outcome: the ordered checks and their conjunction.</summary>
/// <param name="Ok">Whether every check passed.</param>
/// <param name="Checks">The checks, in the order the library ran them.</param>
public sealed record DoctorReport(
    [property: JsonPropertyName("ok")] bool Ok,
    [property: JsonPropertyName("checks")] IReadOnlyList<DoctorCheck> Checks);
