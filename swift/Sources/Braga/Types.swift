// SPDX-License-Identifier: Apache-2.0

import Foundation

/// One retrieval hit, mirroring the JSON object `braga_query` emits.
public struct Hit: Codable, Equatable, Sendable {
    /// The lowercase hex of the 32-byte SHA-256 chunk digest.
    public let chunkId: String
    /// The document path, as the scanner recorded it.
    public let path: String
    /// The first line of the chunk, 1-based.
    public let lineStart: Int
    /// The last line of the chunk, 1-based and never before `lineStart`.
    public let lineEnd: Int
    /// The chunk text.
    public let content: String
    /// The cosine similarity against the query embedding.
    public let score: Double
}

/// One check in the doctor report, in the order the library ran it.
public struct DoctorCheck: Codable, Equatable, Sendable {
    /// The check's stable name.
    public let name: String
    /// Whether the check passed.
    public let passed: Bool
    /// The sanitized detail line.
    public let detail: String
}

/// The claims verifier's outcome: the ordered checks and their conjunction.
public struct DoctorReport: Codable, Equatable, Sendable {
    /// Whether every check passed.
    public let ok: Bool
    /// The checks, in run order.
    public let checks: [DoctorCheck]
}

/// Decodes the ABI's snake_case JSON into the camelCase structs above.
enum JSONCoding {
    static let decoder: JSONDecoder = {
        let decoder = JSONDecoder()
        decoder.keyDecodingStrategy = .convertFromSnakeCase
        return decoder
    }()

    static func decode<T: Decodable>(_ type: T.Type, from data: Data) throws -> T {
        do {
            return try decoder.decode(type, from: data)
        } catch {
            throw BragaError(code: .panic, message: "could not decode the library's JSON")
        }
    }
}
