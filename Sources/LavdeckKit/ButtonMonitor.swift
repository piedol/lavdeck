import Foundation
import IOKit.hid

/// Media/telephony button events the RX reports on its HID interface (report 0x06, consumer page).
/// Bit order follows the RX's report descriptor.
public enum MicButton: Int, CaseIterable, Sendable {
    case volumeUp = 0, volumeDown, mute, playPause, nextTrack, previousTrack, pause, stop
}

public struct MicButtonEvent: Sendable, Equatable {
    public var button: MicButton
    public var isDown: Bool
    public var timestamp: Date
}

/// Listens to the RX's HID consumer-control reports. With `seize = true` the device is opened
/// exclusively so macOS does not also act on the keys (e.g. change the system volume).
public final class ButtonMonitor: @unchecked Sendable {
    public var onEvent: (@Sendable (MicButtonEvent) -> Void)?
    /// Raw input reports (report ID, bytes) for diagnostics.
    public var onRawReport: (@Sendable (UInt8, [UInt8]) -> Void)?

    private var manager: IOHIDManager?
    private var buffer = UnsafeMutablePointer<UInt8>.allocate(capacity: 128)
    private var lastBits: UInt8 = 0
    private let seize: Bool

    public init(seize: Bool = false) { self.seize = seize }

    deinit {
        stop()
        buffer.deallocate()
    }

    /// Must be called on a thread with a running run loop (e.g. the main thread).
    @discardableResult
    public func start() -> IOReturn {
        let manager = IOHIDManagerCreate(kCFAllocatorDefault, IOOptionBits(kIOHIDOptionsTypeNone))
        IOHIDManagerSetDeviceMatching(manager, [
            kIOHIDVendorIDKey: USBLink.vendorID, kIOHIDProductIDKey: USBLink.productID,
        ] as CFDictionary)
        let context = Unmanaged.passUnretained(self).toOpaque()
        IOHIDManagerRegisterDeviceMatchingCallback(manager, { context, _, _, device in
            guard let context else { return }
            Unmanaged<ButtonMonitor>.fromOpaque(context).takeUnretainedValue().attach(device)
        }, context)
        IOHIDManagerScheduleWithRunLoop(manager, CFRunLoopGetCurrent(), CFRunLoopMode.defaultMode.rawValue)
        let options = seize ? IOOptionBits(kIOHIDOptionsTypeSeizeDevice) : IOOptionBits(kIOHIDOptionsTypeNone)
        let result = IOHIDManagerOpen(manager, options)
        self.manager = manager
        return result
    }

    public func stop() {
        guard let manager else { return }
        IOHIDManagerUnscheduleFromRunLoop(manager, CFRunLoopGetCurrent(), CFRunLoopMode.defaultMode.rawValue)
        IOHIDManagerClose(manager, IOOptionBits(kIOHIDOptionsTypeNone))
        self.manager = nil
    }

    private func attach(_ device: IOHIDDevice) {
        let context = Unmanaged.passUnretained(self).toOpaque()
        IOHIDDeviceRegisterInputReportCallback(device, buffer, 128, { context, _, _, _, reportID, report, length in
            guard let context else { return }
            let bytes = Array(UnsafeBufferPointer(start: report, count: length))
            Unmanaged<ButtonMonitor>.fromOpaque(context).takeUnretainedValue().handle(reportID: UInt8(reportID), bytes: bytes)
        }, context)
    }

    func handle(reportID: UInt8, bytes: [UInt8]) {
        onRawReport?(reportID, bytes)
        guard reportID == 0x06 else { return }
        // macOS may include the report ID as the first byte.
        let data = bytes.first == 0x06 && bytes.count >= 2 ? Array(bytes.dropFirst()) : bytes
        guard let bits = data.first else { return }
        let changed = bits ^ lastBits
        lastBits = bits
        for button in MicButton.allCases where changed >> button.rawValue & 1 == 1 {
            onEvent?(MicButtonEvent(button: button, isDown: bits >> button.rawValue & 1 == 1, timestamp: Date()))
        }
    }
}
