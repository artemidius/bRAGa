// SPDX-License-Identifier: Apache-2.0

import CBraga
import Foundation

/// The `BRAGA_ABI_VERSION` this wrapper is written against; `braga.h` freezes it at 1.
private let expectedABIVersion: UInt32 = 1

/// The thin marshalling layer over the frozen C ABI.
///
/// Every `import CBraga` lives here, so the typed facade and its tests never touch C. Strings
/// cross as UTF-8 bytes plus an explicit length (`NULL/0` for a nil or empty path, which defers to
/// the ABI's environment and defaults), JSON is read as exactly `out_json_len` bytes so embedded
/// NULs survive, and the thread-local error buffer is copied immediately after the call that set
/// it. The reserved `key`/`key_len` slots stay `NULL/0` in base mode.
enum FFI {
    /// Evaluated once: whether the loaded library implements the ABI this wrapper was built for.
    private static let abiCompatible: Bool = braga_abi_version() == expectedABIVersion

    /// The ABI version the loaded library implements.
    static var abiVersion: UInt32 {
        ensureCompatible()
        return braga_abi_version()
    }

    /// Refuses to proceed against a mismatched library, mirroring the other wrappers' fail-closed
    /// load check.
    static func ensureCompatible() {
        guard abiCompatible else {
            fatalError(
                "braga: library ABI version \(braga_abi_version()) does not match the wrapper's "
                    + "\(expectedABIVersion); refusing to load a mismatched library"
            )
        }
    }

    /// Resolves the configuration and key and returns a live handle.
    static func open(configPath: String?, keyPath: String?) throws -> OpaquePointer {
        ensureCompatible()
        return try withBytes(configPath) { config, configLength in
            try withBytes(keyPath) { key, keyLength in
                var handle: OpaquePointer?
                let code = decode(
                    braga_open(config, configLength, key, keyLength, &handle)
                )
                guard code == .ok, let handle else {
                    throw code == .ok
                        ? BragaError(
                            code: .panic,
                            message: "braga_open reported OK without a handle"
                        )
                        : failure(code)
                }
                return handle
            }
        }
    }

    /// Writes the default configuration and a fresh key file, refusing to clobber either.
    static func initialize(configPath: String?, keyPath: String?) throws {
        ensureCompatible()
        try withBytes(configPath) { config, configLength in
            try withBytes(keyPath) { key, keyLength in
                let code = decode(braga_init(config, configLength, key, keyLength))
                guard code == .ok else { throw failure(code) }
            }
        }
    }

    /// Frees the handle. The typed layer only calls this with a live handle.
    static func close(_ handle: OpaquePointer?) throws {
        ensureCompatible()
        let code = decode(braga_close(handle))
        guard code == .ok else { throw failure(code) }
    }

    /// Reports the reconciler state; a nil handle reports `ERROR`, as the ABI does.
    static func status(_ handle: OpaquePointer?) -> BragaStatus {
        ensureCompatible()
        return decodeStatus(braga_status(handle))
    }

    /// Reconciles the scan directory, blocking until the index is queryable.
    static func update(_ handle: OpaquePointer?) throws {
        ensureCompatible()
        // The reserved key slot stays NULL in base mode; braga-extra makes it live without an ABI break.
        let code = decode(braga_update(handle, nil, 0))
        guard code == .ok else { throw failure(code) }
    }

    /// Runs a query and returns its JSON bytes.
    static func query(_ handle: OpaquePointer?, text: [UInt8], topK: UInt32) throws -> Data {
        ensureCompatible()
        var out: UnsafeMutablePointer<CChar>?
        var outLength = 0
        let code: BragaResult
        if text.isEmpty {
            // An empty span crosses as NULL/0, which the ABI rejects with INVALID_ARG.
            code = decode(
                braga_query(handle, nil, 0, topK, nil, 0, &out, &outLength)
            )
        } else {
            let bytes = text
            code = bytes.withUnsafeBufferPointer { buffer in
                decode(
                    braga_query(
                        handle, buffer.baseAddress, buffer.count, topK, nil, 0, &out, &outLength
                    )
                )
            }
        }
        return try finish(code, out: out, length: outLength)
    }

    /// Runs the claims verifier and returns its JSON bytes.
    static func doctor(_ handle: OpaquePointer?) throws -> Data {
        ensureCompatible()
        var out: UnsafeMutablePointer<CChar>?
        var outLength = 0
        let code = decode(braga_doctor(handle, &out, &outLength))
        return try finish(code, out: out, length: outLength)
    }

    /// Decodes the C enum the Clang importer exposes as a raw-value struct.
    private static func decode(_ value: braga_result) -> BragaResult {
        BragaResult(code: Int32(truncatingIfNeeded: value.rawValue))
    }

    /// Decodes the C status enum the Clang importer exposes as a raw-value struct.
    private static func decodeStatus(_ value: braga_status_code) -> BragaStatus {
        BragaStatus(code: Int32(truncatingIfNeeded: value.rawValue))
    }

    /// Copies the library-owned JSON buffer on every path and then frees it.
    private static func finish(
        _ code: BragaResult,
        out: UnsafeMutablePointer<CChar>?,
        length: Int
    ) throws -> Data {
        defer { braga_string_free(out) }
        guard code == .ok else { throw failure(code) }
        guard let out else {
            throw BragaError(code: .panic, message: "the library returned no JSON")
        }
        return Data(bytes: out, count: length)
    }

    /// Encodes a path as UTF-8 bytes; nil or empty becomes `NULL/0` to defer to the ABI defaults.
    private static func withBytes<T>(
        _ value: String?,
        _ body: (UnsafePointer<UInt8>?, Int) throws -> T
    ) rethrows -> T {
        guard let value, !value.isEmpty else { return try body(nil, 0) }
        var bytes = Array(value.utf8)
        let length = bytes.count
        bytes.append(0)
        return try bytes.withUnsafeBufferPointer { buffer in
            try body(buffer.baseAddress, length)
        }
    }

    /// Copies `braga_last_error` immediately; the buffer is thread-local and must be read on the
    /// thread that produced it. It is library-owned and never freed.
    private static func failure(_ code: BragaResult) -> BragaError {
        let message = braga_last_error(nil).map { String(cString: $0) } ?? ""
        return BragaError(code: code, message: message)
    }
}
