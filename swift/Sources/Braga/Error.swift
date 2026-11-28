// SPDX-License-Identifier: Apache-2.0

/// A failing `braga_*` call.
///
/// `code` is the ABI result discriminant. `message` is the sanitized text from
/// `braga_last_error`: it never contains key material, file content, or query text.
public struct BragaError: Error, Equatable, CustomStringConvertible, Sendable {
    /// The ABI result discriminant of the failing call.
    public let code: BragaResult
    /// The sanitized message, empty when the library reported none.
    public let message: String

    /// Creates an error for a result code and its sanitized message.
    public init(code: BragaResult, message: String = "") {
        self.code = code
        self.message = message
    }

    /// Renders the code's header name followed by the sanitized message.
    public var description: String {
        message.isEmpty ? code.name : "\(code.name): \(message)"
    }
}
