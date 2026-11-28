// SPDX-License-Identifier: Apache-2.0

using System.Text.Json.Serialization;

namespace BragaNet;

/// <summary>One retrieval hit, mirroring the JSON object <c>braga_query</c> emits.</summary>
/// <param name="ChunkId">Lowercase hex of the chunk's 32-byte SHA-256 digest.</param>
/// <param name="Path">The chunk's source file path.</param>
/// <param name="LineStart">First line of the chunk, one-based.</param>
/// <param name="LineEnd">Last line of the chunk, inclusive.</param>
/// <param name="Content">The chunk's text; may contain embedded NUL bytes.</param>
/// <param name="Score">The similarity score, higher is closer.</param>
public sealed record Hit(
    [property: JsonPropertyName("chunk_id")] string ChunkId,
    [property: JsonPropertyName("path")] string Path,
    [property: JsonPropertyName("line_start")] int LineStart,
    [property: JsonPropertyName("line_end")] int LineEnd,
    [property: JsonPropertyName("content")] string Content,
    [property: JsonPropertyName("score")] double Score);
