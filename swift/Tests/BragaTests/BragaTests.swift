// SPDX-License-Identifier: Apache-2.0

import Foundation
import XCTest

@testable import Braga

/// Conformance tests against the real `braga_ffi` cdylib, in parity with the other wrappers.
final class BragaTests: XCTestCase {
    func testAbiVersionIsOne() {
        XCTAssertEqual(Braga.abiVersion(), 1)
        XCTAssertEqual(FFI.abiVersion, 1)
    }

    func testEnumsMatchHeader() {
        let results: [(BragaResult, Int32)] = [
            (.ok, 0), (.invalidArg, 1), (.config, 2), (.key, 3), (.store, 4),
            (.embed, 5), (.notFound, 6), (.io, 7), (.panic, 8), (.oom, 9),
        ]
        for (result, code) in results {
            XCTAssertEqual(result.rawValue, code)
        }

        let statuses: [(BragaStatus, Int32)] = [
            (.upToDate, 0), (.checking, 1), (.updating, 2), (.error, 3),
        ]
        for (status, code) in statuses {
            XCTAssertEqual(status.rawValue, code)
        }

        XCTAssertEqual(BragaResult(code: 99), .panic)
        XCTAssertEqual(BragaResult(code: -1), .panic)
        XCTAssertEqual(BragaStatus(code: 99), .error)
        XCTAssertEqual(BragaStatus(code: -1), .error)
    }

    func testRoundTrip() throws {
        let harness = try Harness()
        let braga = try Braga.open(configPath: harness.config, keyPath: harness.key)

        XCTAssertEqual(braga.status(), .upToDate)
        requireError(code: .notFound) { _ = try braga.query(Harness.query, topK: 1) }

        try braga.update()
        XCTAssertEqual(braga.status(), .upToDate)

        let hits = try braga.query(Harness.query, topK: 1)
        XCTAssertEqual(hits.count, 1)
        let hit = try XCTUnwrap(hits.first)
        XCTAssertNotNil(
            hit.chunkId.range(of: "^[0-9a-f]{64}$", options: .regularExpression),
            "chunk_id = \(hit.chunkId)"
        )
        XCTAssertTrue(hit.path.hasSuffix("notes.md"), "path = \(hit.path)")
        XCTAssertTrue(hit.content.contains("quick brown fox"), "content = \(hit.content)")
        XCTAssertGreaterThanOrEqual(hit.lineStart, 1)
        XCTAssertGreaterThanOrEqual(hit.lineEnd, hit.lineStart)
        XCTAssertGreaterThan(hit.score, 0)

        let report = try braga.doctor()
        XCTAssertTrue(report.ok)
        XCTAssertEqual(report.checks.count, 6)
        for check in report.checks {
            XCTAssertTrue(check.passed, "\(check.name): \(check.detail)")
        }

        try braga.close()
        try braga.close()
        requireError(code: .invalidArg) { _ = try braga.query(Harness.query, topK: 1) }
        XCTAssertEqual(braga.status(), .error)
    }

    func testWrongKey() throws {
        let harness = try Harness()
        try Braga.open(configPath: harness.config, keyPath: harness.key).close()

        let wrong = try harness.write("wrong-key", Data(repeating: 0x11, count: 32))
        let error = requireError(code: .store) {
            _ = try Braga.open(configPath: harness.config, keyPath: wrong)
        }
        XCTAssertTrue(
            error.message.contains("wrong database key"),
            "message = \(error.message)"
        )
    }

    func testMalformedKey() throws {
        let harness = try Harness()
        let bad = try harness.write("bad-key", Data("not-a-key".utf8))

        let error = requireError(code: .key) {
            _ = try Braga.open(configPath: harness.config, keyPath: bad)
        }
        XCTAssertTrue(
            error.message.contains("expected exactly 32 bytes"),
            "message = \(error.message)"
        )
        XCTAssertFalse(error.message.contains("not-a-key"), "key material leaked")
    }

    func testMissingConfig() throws {
        let harness = try Harness()
        let error = requireError(code: .config) {
            _ = try Braga.open(configPath: harness.root + "/missing.toml", keyPath: harness.key)
        }
        XCTAssertTrue(error.message.contains("configuration"), "message = \(error.message)")
    }

    func testBrokenToml() throws {
        let harness = try Harness()
        let broken = try harness.write(
            "broken.toml",
            Data("[scan]\ndir = \"SUPER-SECRET\" nope\n".utf8)
        )

        let error = requireError(code: .config) {
            _ = try Braga.open(configPath: broken, keyPath: harness.key)
        }
        XCTAssertTrue(
            error.message.contains("invalid configuration"),
            "message = \(error.message)"
        )
        XCTAssertFalse(error.message.contains("SUPER-SECRET"), "file content leaked")
    }

    func testInitRefusesToClobber() throws {
        let harness = try Harness()
        let config = harness.root + "/init.toml"
        let key = harness.root + "/nested/init-key"

        try Braga.initialize(configPath: config, keyPath: key)
        XCTAssertTrue(FileManager.default.fileExists(atPath: config))
        let attributes = try FileManager.default.attributesOfItem(atPath: key)
        let size = try XCTUnwrap(attributes[.size] as? Int)
        XCTAssertEqual(size, 32)

        let error = requireError(code: .io) {
            try Braga.initialize(configPath: config, keyPath: key)
        }
        XCTAssertTrue(error.message.contains("already exists"), "message = \(error.message)")
    }

    func testNoCrash() throws {
        requireError(code: .invalidArg) { try FFI.close(nil) }
        XCTAssertEqual(FFI.status(nil), .error)
        requireError(code: .invalidArg) { try FFI.update(nil) }
        requireError(code: .invalidArg) { _ = try FFI.doctor(nil) }
        requireError(code: .invalidArg) {
            _ = try FFI.query(nil, text: Array(Harness.query.utf8), topK: 1)
        }

        let harness = try Harness()
        let braga = try Braga.open(configPath: harness.config, keyPath: harness.key)
        let handle = try XCTUnwrap(braga.nativeHandle)

        requireError(code: .invalidArg) { _ = try FFI.query(handle, text: [], topK: 1) }
        requireError(code: .invalidArg) {
            _ = try FFI.query(handle, text: [0xFF, 0xFE], topK: 1)
        }
        requireError(code: .invalidArg) { _ = try braga.query("") }
        requireError(code: .invalidArg) { _ = try braga.query(Harness.query, topK: -1) }
        requireError(code: .invalidArg) {
            _ = try braga.query(Harness.query, topK: Int(UInt32.max) + 1)
        }

        try braga.update()
        do {
            _ = try FFI.query(handle, text: Array(Harness.query.utf8), topK: UInt32.max)
        } catch let error as BragaError {
            XCTAssertNotEqual(error.code, .panic)
        }
    }

    @discardableResult
    private func requireError(
        code: BragaResult,
        file: StaticString = #filePath,
        line: UInt = #line,
        _ body: () throws -> Void
    ) -> BragaError {
        do {
            try body()
            XCTFail("expected a BragaError with code \(code.name)", file: file, line: line)
        } catch let error as BragaError {
            XCTAssertEqual(error.code, code, "unexpected error: \(error)", file: file, line: line)
            return error
        } catch {
            XCTFail("expected a BragaError, got \(error)", file: file, line: line)
        }
        return BragaError(code: .panic, message: "")
    }
}
