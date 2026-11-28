// SPDX-License-Identifier: Apache-2.0

/// The result code returned by every fallible `braga_*` entry point.
///
/// The discriminants are the frozen ABI values (`braga.h` 0..9); a wrapper maps them directly and
/// never renumbers them. `init(code:)` fails closed to `.panic` for a code this version does not
/// know.
public enum BragaResult: Int32, CaseIterable, Equatable, Sendable {
    /// The call succeeded.
    case ok = 0
    /// Null pointer, bad UTF-8, bad length, or null out-parameter.
    case invalidArg = 1
    /// Configuration resolution or validation failed.
    case config = 2
    /// Key file load failed.
    case key = 3
    /// Storage error: wrong key, I/O, or a numeric SQLite code.
    case store = 4
    /// Embedder build or provider call failed.
    case embed = 5
    /// Nothing embedded for the configured model.
    case notFound = 6
    /// I/O error outside the store.
    case io = 7
    /// A Rust panic was caught inside the library.
    case panic = 8
    /// Allocation or JSON serialization failed.
    case oom = 9

    /// Decodes a raw ABI result code, failing closed to `.panic` outside the header's 0..9 range.
    public init(code: Int32) {
        self = BragaResult(rawValue: code) ?? .panic
    }

    /// Renders the header's constant name, for diagnostics.
    public var name: String {
        switch self {
        case .ok: "OK"
        case .invalidArg: "INVALID_ARG"
        case .config: "CONFIG"
        case .key: "KEY"
        case .store: "STORE"
        case .embed: "EMBED"
        case .notFound: "NOT_FOUND"
        case .io: "IO"
        case .panic: "PANIC"
        case .oom: "OOM"
        }
    }
}

/// The reconciler state reported by `braga_status`.
///
/// The discriminants are the frozen ABI values (`braga.h` 0..3). `init(code:)` fails closed to
/// `.error`, matching the library's own mapping of out-of-range atomic values.
public enum BragaStatus: Int32, CaseIterable, Equatable, Sendable {
    /// The index matches the scan directory; no update is in flight.
    case upToDate = 0
    /// An update is in flight and nothing has been written yet.
    case checking = 1
    /// An update is in flight and writing or embedding.
    case updating = 2
    /// The last open or update failed.
    case error = 3

    /// Decodes a raw status, failing closed to `.error` outside the header's 0..3 range.
    public init(code: Int32) {
        self = BragaStatus(rawValue: code) ?? .error
    }

    /// Renders the header's constant name, for diagnostics.
    public var name: String {
        switch self {
        case .upToDate: "UP_TO_DATE"
        case .checking: "CHECKING"
        case .updating: "UPDATING"
        case .error: "ERROR"
        }
    }
}
