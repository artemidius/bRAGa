// SPDX-License-Identifier: Apache-2.0

import Foundation

/// An open bRAGa index handle and its typed query surface.
///
/// The handle owns one `braga_handle *` and frees it exactly once in `deinit`; `close()` is the
/// explicit, idempotent counterpart. Every method reports `.invalidArg` ("handle is closed") after
/// that, except `status()`, which fails closed to `.error`.
///
/// A handle is **not** thread-safe: use it from one thread at a time, as `braga.h` documents. No
/// `Sendable` conformance is declared, deliberately.
public final class Braga {
    private var handle: OpaquePointer?

    /// Opens the configured index and key.
    ///
    /// - Parameters:
    ///   - configPath: The TOML path; nil or empty defers to `BRAGA_TOML`, `BRAGA_CONFIG`, the
    ///     `BRAGA_*__*` environment variables, and the built-in defaults.
    ///   - keyPath: The key path; nil or empty defers to `BRAGA_KEY_FILE` and then `~/.braga/key`.
    /// - Throws: `BragaError` when the library reports a non-OK result.
    public init(configPath: String? = nil, keyPath: String? = nil) throws {
        handle = try FFI.open(configPath: configPath, keyPath: keyPath)
    }

    deinit {
        if let handle {
            try? FFI.close(handle)
        }
    }

    /// The ABI version the loaded library implements.
    public static func abiVersion() -> UInt32 {
        FFI.abiVersion
    }

    /// Opens the configured index and key.
    public static func open(configPath: String? = nil, keyPath: String? = nil) throws -> Braga {
        try Braga(configPath: configPath, keyPath: keyPath)
    }

    /// Writes the default `braga.toml` and a fresh key file, refusing to overwrite either existing
    /// target.
    public static func initialize(configPath: String? = nil, keyPath: String? = nil) throws {
        try FFI.initialize(configPath: configPath, keyPath: keyPath)
    }

    /// Whether the handle has been closed.
    public var isClosed: Bool { handle == nil }

    /// Reports the reconciler state left by the last open or update.
    public func status() -> BragaStatus {
        FFI.status(handle)
    }

    /// Reconciles the scan directory and embeds every chunk missing a vector, blocking until the
    /// index is queryable.
    public func update() throws {
        try FFI.update(try liveHandle())
    }

    /// Returns the top `topK` hits for `text`.
    ///
    /// An empty index is `.notFound`, never an empty success. Empty text and a `topK` that does
    /// not fit in a `uint32` are `.invalidArg`.
    public func query(_ text: String, topK: Int = 10) throws -> [Hit] {
        let handle = try liveHandle()
        guard !text.isEmpty else {
            throw BragaError(code: .invalidArg, message: "text must be non-empty valid UTF-8")
        }
        guard topK >= 0, topK <= Int(UInt32.max) else {
            throw BragaError(code: .invalidArg, message: "topK must fit in a uint32")
        }
        let json = try FFI.query(handle, text: Array(text.utf8), topK: UInt32(topK))
        return try JSONCoding.decode([Hit].self, from: json)
    }

    /// Runs the claims verifier and returns its typed report.
    public func doctor() throws -> DoctorReport {
        let json = try FFI.doctor(try liveHandle())
        return try JSONCoding.decode(DoctorReport.self, from: json)
    }

    /// Frees the handle. Idempotent: a second call is a no-op.
    public func close() throws {
        guard let handle else { return }
        self.handle = nil
        try FFI.close(handle)
    }

    /// The raw ABI handle, exposed to the conformance tests only.
    var nativeHandle: OpaquePointer? { handle }

    private func liveHandle() throws -> OpaquePointer {
        guard let handle else {
            throw BragaError(code: .invalidArg, message: "handle is closed")
        }
        return handle
    }
}
