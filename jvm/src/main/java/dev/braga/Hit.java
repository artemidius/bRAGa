// SPDX-License-Identifier: Apache-2.0

package dev.braga;

import com.fasterxml.jackson.annotation.JsonProperty;

/**
 * One retrieval hit, mirroring an object of the JSON array {@code braga_query} emits.
 *
 * @param chunkId lowercase hex of the chunk's 32-byte SHA-256 digest
 * @param path path of the file the chunk belongs to
 * @param lineStart 1-based, inclusive first line of the chunk
 * @param lineEnd 1-based, inclusive last line of the chunk
 * @param content the exact chunk text
 * @param score fused reciprocal-rank-fusion score
 */
public record Hit(
        @JsonProperty("chunk_id") String chunkId,
        @JsonProperty("path") String path,
        @JsonProperty("line_start") int lineStart,
        @JsonProperty("line_end") int lineEnd,
        @JsonProperty("content") String content,
        @JsonProperty("score") double score) {}
