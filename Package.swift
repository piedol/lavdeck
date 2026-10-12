// swift-tools-version: 6.0
import PackageDescription

let package = Package(
    name: "Lavdeck",
    platforms: [.macOS(.v15)],
    products: [
        .library(name: "LavdeckKit", targets: ["LavdeckKit"]),
        .library(name: "LavdeckPluginKit", targets: ["LavdeckPluginKit"]),
        .executable(name: "lavdeck", targets: ["lavdeck"]),
    ],
    targets: [
        .target(name: "LavdeckPluginKit"),
        .target(name: "LavdeckKit", dependencies: ["LavdeckPluginKit"], linkerSettings: [
            .linkedFramework("IOKit"), .linkedFramework("IOUSBHost"), .linkedFramework("AVFoundation"),
        ]),
        .executableTarget(name: "lavdeck", dependencies: ["LavdeckKit"]),
        // Screen and camera recording, the camera preview and each take's finished video. Swift 5 mode: it
        // wraps AVFoundation and ScreenCaptureKit delegate callbacks that predate strict concurrency.
        .target(name: "LavdeckCapture", dependencies: ["LavdeckKit", "LavdeckPluginKit"],
                swiftSettings: [.swiftLanguageMode(.v5)],
                linkerSettings: [
                    .linkedFramework("ScreenCaptureKit"), .linkedFramework("AVFoundation"),
                    .linkedFramework("CoreImage"), .linkedFramework("AppKit"),
                ]),
        .executableTarget(name: "LavdeckApp", dependencies: ["LavdeckKit", "LavdeckCapture"], linkerSettings: [
            .linkedFramework("Carbon"), .linkedFramework("ServiceManagement"),
        ]),
        .testTarget(name: "LavdeckKitTests", dependencies: ["LavdeckKit", "LavdeckPluginKit", "LavdeckCapture"]),
    ]
)
