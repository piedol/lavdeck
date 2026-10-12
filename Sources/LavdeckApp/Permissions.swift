import AppKit
import AVFoundation
import CoreGraphics
import Foundation
import Observation

/// System permissions plugins can need. Lavdeck asks for each one itself, once, however many plugins
/// declare it; plugin processes run under Lavdeck's grants and never prompt on their own.
@MainActor
@Observable
final class PermissionCenter {
    enum State { case granted, notAsked, denied }

    /// Bumped after a request so views re-read the states.
    private(set) var revision = 0
    /// macOS applies a new screen recording grant only after Lavdeck relaunches, so ask at most once per
    /// launch; after that, say what to do instead of prompting again.
    private(set) var askedForScreenThisLaunch = false

    func state(_ permission: String) -> State {
        _ = revision
        switch permission {
        case "camera": return Self.avState(.video)
        case "microphone": return Self.avState(.audio)
        case "screen-recording": return CGPreflightScreenCaptureAccess() ? .granted : .denied
        default: return .granted
        }
    }

    func granted(_ permissions: [String]) -> [String] { permissions.filter { state($0) == .granted } }

    /// Asks for any of `permissions` not yet granted. Camera and microphone show the system prompt;
    /// screen recording shows macOS's prompt (or points to System Settings) and takes effect after
    /// Lavdeck is reopened.
    func request(_ permissions: [String]) async {
        for permission in Set(permissions) where state(permission) != .granted {
            switch permission {
            case "camera": _ = await AVCaptureDevice.requestAccess(for: .video)
            case "microphone": _ = await AVCaptureDevice.requestAccess(for: .audio)
            case "screen-recording":
                guard !askedForScreenThisLaunch else { continue }
                askedForScreenThisLaunch = true
                _ = CGRequestScreenCaptureAccess()
            default: break
            }
        }
        revision += 1
    }

    func openSettings(for permission: String) {
        let anchor = switch permission {
        case "camera": "Privacy_Camera"
        case "microphone": "Privacy_Microphone"
        default: "Privacy_ScreenCapture"
        }
        if let url = URL(string: "x-apple.systempreferences:com.apple.preference.security?\(anchor)") {
            NSWorkspace.shared.open(url)
        }
        revision += 1
    }

    /// The permission a capture source needs, e.g. `screen` → `screen-recording`.
    static func permission(forSource source: String) -> String? {
        switch source {
        case "screen": "screen-recording"
        case "camera": "camera"
        default: nil
        }
    }

    static func name(_ permission: String) -> String {
        switch permission {
        case "screen-recording": "Screen recording"
        case "camera": "Camera"
        case "microphone": "Microphone"
        case "network": "Network"
        default: permission
        }
    }

    private static func avState(_ type: AVMediaType) -> State {
        switch AVCaptureDevice.authorizationStatus(for: type) {
        case .authorized: .granted
        case .notDetermined: .notAsked
        default: .denied
        }
    }
}
