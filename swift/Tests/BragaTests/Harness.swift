// SPDX-License-Identifier: Apache-2.0

import Foundation
import Network

/// A hermetic fixture: a temporary tree with a sample document, a 32-byte key file, and a loopback
/// OpenAI-compatible embeddings stub, mirroring `go/harness_test.go` and the .NET harness.
final class Harness {
    /// The sample document the semantic round-trip indexes.
    static let sample = "# Notes\n\nThe quick brown fox jumps over the lazy dog.\n"

    /// The query text that must find `sample`.
    static let query = "quick brown fox"

    /// The fixture root, which also holds the index database.
    let root: String
    /// The stub-backed configuration file.
    let config: String
    /// The 32-byte key file.
    let key: String

    private let stub: EmbeddingStub

    init() throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("braga-swift-\(UUID().uuidString)", isDirectory: true)
            .path
        let docs = root + "/docs"
        try FileManager.default.createDirectory(atPath: docs, withIntermediateDirectories: true)
        try Data(Harness.sample.utf8).write(to: URL(fileURLWithPath: docs + "/notes.md"))

        let key = root + "/key"
        try Data(repeating: 0x5A, count: 32).write(to: URL(fileURLWithPath: key))

        let stub = try EmbeddingStub()
        let config = root + "/braga.toml"
        let body = """
            [scan]
            dir = \(toml(docs))

            [embedding]
            provider = "openai-compatible"
            url = \(toml("http://127.0.0.1:\(stub.port)/v1/embeddings"))
            model = "stub-embed"

            [storage]
            path = \(toml(root + "/index.db"))
            """
        try Data(body.utf8).write(to: URL(fileURLWithPath: config))

        self.root = root
        self.config = config
        self.key = key
        self.stub = stub
    }

    deinit {
        stub.stop()
        try? FileManager.default.removeItem(atPath: root)
    }

    /// Writes another file in the fixture root and returns its path.
    func write(_ name: String, _ contents: Data) throws -> String {
        let path = root + "/" + name
        try contents.write(to: URL(fileURLWithPath: path))
        return path
    }
}

/// Renders a TOML basic string for a path or URL.
private func toml(_ value: String) -> String {
    "\"" + value
        .replacingOccurrences(of: "\\", with: "\\\\")
        .replacingOccurrences(of: "\"", with: "\\\"") + "\""
}

/// A loopback HTTP server answering every OpenAI-compatible embeddings request with one
/// `[1,0,0,0]` vector per input, the Rust golden vector.
private final class EmbeddingStub: @unchecked Sendable {
    /// The ephemeral port the listener bound.
    private(set) var port: UInt16 = 0

    private let listener: NWListener
    private let queue = DispatchQueue(label: "org.braga.swift.embedding-stub")

    init() throws {
        let parameters = NWParameters.tcp
        parameters.requiredInterfaceType = .loopback
        let listener = try NWListener(using: parameters, on: .any)
        self.listener = listener

        listener.newConnectionHandler = { [weak self] connection in
            self?.accept(connection)
        }

        let ready = DispatchSemaphore(value: 0)
        listener.stateUpdateHandler = { state in
            if case .ready = state { ready.signal() }
        }
        listener.start(queue: queue)
        guard ready.wait(timeout: .now() + 5) == .success,
              let bound = listener.port?.rawValue, bound > 0
        else {
            listener.cancel()
            throw StubError("the embeddings stub did not become ready")
        }
        port = bound
    }

    func stop() {
        listener.cancel()
    }

    private func accept(_ connection: NWConnection) {
        connection.start(queue: queue)
        receive(connection, buffer: Data())
    }

    private func receive(_ connection: NWConnection, buffer: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 64 * 1024) {
            [weak self] data, _, isComplete, error in
            guard let self, error == nil else {
                connection.cancel()
                return
            }
            var accumulated = buffer
            if let data { accumulated.append(data) }
            if let request = HTTPRequest(accumulated) {
                self.respond(connection, inputs: request.inputCount)
            } else if isComplete {
                connection.cancel()
            } else {
                self.receive(connection, buffer: accumulated)
            }
        }
    }

    private func respond(_ connection: NWConnection, inputs: Int) {
        var body = "{\"data\":["
        for index in 0..<inputs {
            if index > 0 { body += "," }
            body += "{\"index\":\(index),\"embedding\":[1,0,0,0]}"
        }
        body += "]}"

        let payload = Data(body.utf8)
        var response = Data(
            "HTTP/1.1 200 OK\r\n"
                .appending("Content-Type: application/json\r\n")
                .appending("Content-Length: \(payload.count)\r\n")
                .appending("Connection: close\r\n\r\n")
                .utf8
        )
        response.append(payload)
        connection.send(content: response, completion: .contentProcessed { _ in
            connection.cancel()
        })
    }
}

/// A complete HTTP/1.1 request, as far as the stub needs to understand it.
private struct HTTPRequest {
    let inputCount: Int

    init?(_ buffer: Data) {
        guard let headerEnd = buffer.range(of: Data("\r\n\r\n".utf8)) else { return nil }
        let header = String(decoding: buffer[..<headerEnd.lowerBound], as: UTF8.self)
        var contentLength = 0
        for line in header.components(separatedBy: "\r\n") {
            guard let colon = line.firstIndex(of: ":") else { continue }
            let name = line[..<colon].trimmingCharacters(in: .whitespaces)
            guard name.caseInsensitiveCompare("Content-Length") == .orderedSame else { continue }
            let value = line[line.index(after: colon)...].trimmingCharacters(in: .whitespaces)
            contentLength = Int(value) ?? 0
        }

        let start = headerEnd.upperBound
        guard buffer.count - start >= contentLength else { return nil }
        let body = Data(buffer[start..<(start + contentLength)])
        inputCount = HTTPRequest.countInputs(body)
    }

    private static func countInputs(_ body: Data) -> Int {
        guard let object = try? JSONSerialization.jsonObject(with: body),
              let root = object as? [String: Any],
              let input = root["input"] as? [Any]
        else {
            return 0
        }
        return input.count
    }
}

/// A fixture failure, reported through `Harness.init()` throwing.
private struct StubError: Error, CustomStringConvertible {
    let description: String

    init(_ description: String) {
        self.description = description
    }
}
