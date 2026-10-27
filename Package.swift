// swift-tools-version: 6.0
// Lavdeck's plugin SDK for Swift. The app itself is Rust (see Cargo.toml); this package is for
// plugins written in Swift: the plugin protocol and transcript types, and a reader for the
// take library.
import PackageDescription

let package = Package(
    name: "Lavdeck",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "LavdeckPluginKit", targets: ["LavdeckPluginKit"]),
        .library(name: "LavdeckKit", targets: ["LavdeckKit"]),
    ],
    targets: [
        .target(name: "LavdeckPluginKit", path: "sdk/swift/Sources/LavdeckPluginKit"),
        .target(name: "LavdeckKit", dependencies: ["LavdeckPluginKit"], path: "sdk/swift/Sources/LavdeckKit",
                linkerSettings: [.linkedFramework("AVFoundation")]),
    ]
)
