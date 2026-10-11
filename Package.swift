// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Lavdeck",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "LavdeckKit", targets: ["LavdeckKit"]),
        .executable(name: "lavdeck", targets: ["lavdeck"]),
    ],
    targets: [
        .target(name: "LavdeckKit", linkerSettings: [
            .linkedFramework("IOKit"), .linkedFramework("IOUSBHost"), .linkedFramework("AVFoundation"),
        ]),
        .executableTarget(name: "lavdeck", dependencies: ["LavdeckKit"]),
        .executableTarget(name: "LavdeckApp", dependencies: ["LavdeckKit"], linkerSettings: [
            .linkedFramework("Carbon"), .linkedFramework("ServiceManagement"),
        ]),
        .testTarget(name: "LavdeckKitTests", dependencies: ["LavdeckKit"]),
    ]
)
