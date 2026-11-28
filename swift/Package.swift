// swift-tools-version: 6.0
// SPDX-License-Identifier: Apache-2.0

import Foundation
import PackageDescription

// The package lives at <repo>/swift, and the C ABI cdylib at <repo>/target/release; deriving both
// from #filePath keeps the package relocatable, including on a CI runner.
let repoRoot = URL(fileURLWithPath: #filePath)
    .deletingLastPathComponent()
    .deletingLastPathComponent()
    .path
let release = repoRoot + "/target/release"

let package = Package(
    name: "Braga",
    platforms: [.macOS(.v13)],
    products: [
        .library(name: "Braga", targets: ["Braga"]),
    ],
    targets: [
        // A plain C target with a vendored header: SwiftPM generates the module map, and the linker
        // settings find the prebuilt cdylib without a pkg-config file or a systemLibrary target.
        .target(
            name: "CBraga",
            publicHeadersPath: "include",
            linkerSettings: [
                .linkedLibrary("braga_ffi"),
                .unsafeFlags([
                    "-L\(release)",
                    "-Xlinker", "-rpath",
                    "-Xlinker", release,
                ]),
            ]
        ),
        .target(name: "Braga", dependencies: ["CBraga"]),
        .testTarget(name: "BragaTests", dependencies: ["Braga"]),
    ]
)
