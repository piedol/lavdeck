import Foundation

public enum MicError: Error, Equatable, CustomStringConvertible {
    case notConnected
    case notAllowed
    case timeout
    case rejected(code: UInt8)
    case notApplied
    case transport(String)

    public var description: String {
        switch self {
        case .notConnected: "Receiver is not connected"
        case .notAllowed: "Command is outside the verified allowlist"
        case .timeout: "Receiver did not answer"
        case .rejected(let code): String(format: "Receiver rejected the command (0x%02x)", code)
        case .notApplied: "Receiver accepted the command but its status did not change"
        case .transport(let message): "USB error: \(message)"
        }
    }
}

/// Talks to a DJI Mic Mobile RX over USB: keeps a live `MicStatus` from the receiver's pushes
/// and sends allowlisted commands. Reconnects automatically when the receiver is replugged.
public final class MicController: @unchecked Sendable {
    public private(set) var status: MicStatus {
        get { lock.withLock { _status } }
        set { lock.withLock { _status = newValue } }
    }
    public var isConnected: Bool { link.isOpen }

    /// Called on an arbitrary thread whenever a status push changes the model.
    public var onStatus: (@Sendable (MicStatus) -> Void)?
    /// Called on an arbitrary thread when the USB link opens (true) or closes (false).
    public var onConnectionChange: (@Sendable (Bool) -> Void)?

    private let lock = NSLock()
    private let link = USBLink()
    private var _status = MicStatus()
    private var parser = DUMLStreamParser()
    private var pending: [UInt16: CheckedContinuation<DUMLFrame, Error>] = [:]
    private var sequence = UInt16.random(in: 0x100...0x7fff)
    private var watchdog: Task<Void, Never>?

    public init() {
        link.onBytes = { [weak self] bytes in self?.handle(bytes) }
        link.onDisconnect = { [weak self] in self?.handleDisconnect() }
    }

    deinit { stop() }

    /// Opens the receiver if present and keeps retrying every second while it is absent.
    public func start() {
        guard watchdog == nil else { return }
        watchdog = Task.detached(priority: .utility) { [weak self] in
            while !Task.isCancelled {
                if let self, !self.link.isOpen, USBLink.isPresent() {
                    do {
                        try self.link.open()
                        self.onConnectionChange?(true)
                    } catch {}
                }
                try? await Task.sleep(for: .seconds(1))
            }
        }
    }

    public func stop() {
        watchdog?.cancel()
        watchdog = nil
        link.close()
        failAllPending(MicError.notConnected)
    }

    // MARK: Commands

    public func version() async throws -> String {
        let response = try await send(.getVersion)
        // Payload: result, then a NUL-padded hardware name, then firmware bytes.
        let name = response.payload.dropFirst(2).prefix { $0 != 0 }
        return MicStatusDecoder.ascii(name)
    }

    public func capabilities(id: UInt16 = 0) async throws -> MicCapabilities {
        let response = try await send(.getCapability(id: id))
        guard let caps = MicCapabilities(payload: response.payload) else { throw MicError.transport("bad capability payload") }
        return caps
    }

    /// Sets mono/stereo and waits until the receiver's status reports the new mode.
    public func setChannel(_ mode: ChannelMode) async throws {
        try await sendWrite(.setChannel(mode))
        try await waitForStatus { $0.receiver.channelMode == mode }
    }

    /// Sets receiver gain (dB) and waits until the receiver's status reports it.
    public func setReceiverGain(dB: Int) async throws {
        try await sendWrite(.setReceiverGain(dB: dB))
        try await waitForStatus { $0.receiver.gainDB == dB }
    }

    private func sendWrite(_ command: MicCommand) async throws {
        let response = try await send(command)
        guard let code = response.payload.first else { throw MicError.transport("empty ack") }
        guard code == 0 else { throw MicError.rejected(code: code) }
    }

    private func waitForStatus(timeout: Duration = .seconds(3), _ predicate: (MicStatus) -> Bool) async throws {
        let deadline = ContinuousClock.now + timeout
        while ContinuousClock.now < deadline {
            if predicate(status) { return }
            try await Task.sleep(for: .milliseconds(100))
        }
        throw MicError.notApplied
    }

    func send(_ command: MicCommand, timeout: Duration = .seconds(2)) async throws -> DUMLFrame {
        guard link.isOpen else { throw MicError.notConnected }
        let seq = lock.withLock { () -> UInt16 in
            sequence = sequence &+ 1
            if sequence < 0x100 { sequence = 0x100 }
            return sequence
        }
        guard let frame = command.frame(sequence: seq) else { throw MicError.notAllowed }
        return try await withCheckedThrowingContinuation { continuation in
            lock.withLock { pending[seq] = continuation }
            do {
                try link.write(frame.encoded())
            } catch {
                resume(seq, with: .failure(MicError.transport("\(error)")))
                return
            }
            Task.detached { [weak self] in
                try? await Task.sleep(for: timeout)
                self?.resume(seq, with: .failure(MicError.timeout))
            }
        }
    }

    // MARK: Incoming

    private func handle(_ bytes: [UInt8]) {
        let frames = lock.withLock { parser.append(bytes) }
        var changed = false
        for frame in frames {
            if frame.isResponse {
                resume(frame.sequence, with: .success(frame))
                continue
            }
            let updated: MicStatus? = lock.withLock {
                var copy = _status
                guard MicStatusDecoder.apply(frame, to: &copy) else { return nil }
                let differs = copy.receiver != _status.receiver || copy.transmitters != _status.transmitters
                _status = copy
                return differs ? copy : nil
            }
            if updated != nil { changed = true }
        }
        if changed { onStatus?(status) }
    }

    private func handleDisconnect() {
        lock.withLock {
            _status = MicStatus()
            parser = DUMLStreamParser()
        }
        failAllPending(MicError.notConnected)
        onConnectionChange?(false)
        onStatus?(status)
    }

    private func resume(_ seq: UInt16, with result: Result<DUMLFrame, Error>) {
        let continuation = lock.withLock { pending.removeValue(forKey: seq) }
        continuation?.resume(with: result)
    }

    private func failAllPending(_ error: Error) {
        let all = lock.withLock { () -> [CheckedContinuation<DUMLFrame, Error>] in
            defer { pending.removeAll() }
            return Array(pending.values)
        }
        all.forEach { $0.resume(throwing: error) }
    }
}
