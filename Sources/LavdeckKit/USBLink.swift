import Foundation
import IOKit
import IOUSBHost

/// Bulk link to the RX's vendor interface 6 (IN 0x86 / OUT 0x06).
///
/// Opened with IOUSBHost **without** seizing it, so macOS's accessoryd keeps its own client on
/// the interface. Reads run on a dedicated thread; `onBytes` is called for every chunk.
final class USBLink: @unchecked Sendable {
    static let vendorID = 0x2ca3
    static let productID = 0x4011
    static let interfaceNumber = 6
    static let inEndpoint = 0x86
    static let outEndpoint = 0x06

    enum LinkError: Error { case notFound, open(Error), notOpen, write(Error) }

    private let lock = NSLock()
    private var interface: IOUSBHostInterface?
    private var inPipe: IOUSBHostPipe?
    private var outPipe: IOUSBHostPipe?
    private var reader: Thread?
    private var running = false

    var onBytes: (@Sendable ([UInt8]) -> Void)?
    var onDisconnect: (@Sendable () -> Void)?

    static func isPresent() -> Bool { findService() != 0 }

    private static func findService() -> io_service_t {
        let match = IOServiceMatching("IOUSBHostInterface") as NSMutableDictionary
        match["IOPropertyMatch"] = [
            "idVendor": vendorID, "idProduct": productID, "bInterfaceNumber": interfaceNumber,
        ]
        return IOServiceGetMatchingService(kIOMainPortDefault, match)
    }

    var isOpen: Bool { lock.withLock { interface != nil } }

    func open() throws {
        let service = Self.findService()
        guard service != 0 else { throw LinkError.notFound }
        do {
            let intf = try IOUSBHostInterface(__ioService: service, options: [], queue: nil, interestHandler: nil)
            let inPipe = try intf.copyPipe(withAddress: Self.inEndpoint)
            let outPipe = try intf.copyPipe(withAddress: Self.outEndpoint)
            lock.withLock {
                self.interface = intf
                self.inPipe = inPipe
                self.outPipe = outPipe
                self.running = true
            }
        } catch {
            IOObjectRelease(service)
            throw LinkError.open(error)
        }
        let thread = Thread { [weak self] in self?.readLoop() }
        thread.name = "LavdeckKit.USBLink.read"
        thread.qualityOfService = .userInitiated
        lock.withLock { reader = thread }
        thread.start()
    }

    func close() {
        let intf: IOUSBHostInterface? = lock.withLock {
            running = false
            defer { interface = nil; inPipe = nil; outPipe = nil }
            return interface
        }
        intf?.destroy()
    }

    func write(_ bytes: [UInt8]) throws {
        guard let pipe = lock.withLock({ outPipe }) else { throw LinkError.notOpen }
        let data = NSMutableData(bytes: bytes, length: bytes.count)
        var sent = 0
        do { try pipe.__sendIORequest(with: data, bytesTransferred: &sent, completionTimeout: 1.0) }
        catch { throw LinkError.write(error) }
    }

    private func readLoop() {
        let buffer = NSMutableData(length: 1024)!
        var consecutiveErrors = 0
        while true {
            guard let pipe = lock.withLock({ running ? inPipe : nil }) else { return }
            var count = 0
            do {
                try pipe.__sendIORequest(with: buffer, bytesTransferred: &count, completionTimeout: 0.5)
                consecutiveErrors = 0
                if count > 0 {
                    let bytes = Array(UnsafeBufferPointer(start: buffer.bytes.assumingMemoryBound(to: UInt8.self), count: count))
                    onBytes?(bytes)
                }
            } catch {
                let code = (error as NSError).code
                if code == Int(kIOReturnTimeout) { continue }
                consecutiveErrors += 1
                if consecutiveErrors >= 3 || !Self.isPresent() {
                    let wasRunning = lock.withLock { running }
                    close()
                    if wasRunning { onDisconnect?() }
                    return
                }
                Thread.sleep(forTimeInterval: 0.1)
            }
        }
    }
}
