import Darwin
import Foundation

/// A child process started with posix_spawn, with pipes on stdin, stdout and stderr. It runs under
/// Lavdeck's privacy permissions, so plugins share Lavdeck's grants instead of prompting on their own.
final class SpawnedProcess: @unchecked Sendable {
    let pid: pid_t
    let stdin: FileHandle
    let stdout: FileHandle
    let stderr: FileHandle
    private let exitSource: DispatchSourceProcess
    private let lock = NSLock()
    private var status: Int32?
    private var exitHandlers: [@Sendable (Int32) -> Void] = []

    enum SpawnError: Error, CustomStringConvertible {
        case failed(Int32)
        var description: String {
            switch self { case .failed(let code): String(cString: strerror(code)) }
        }
    }

    init(executable: URL, arguments: [String], environment: [String: String], directory: URL) throws {
        let input = Pipe(), output = Pipe(), errors = Pipe()
        var attributes: posix_spawnattr_t?
        posix_spawnattr_init(&attributes)
        defer { posix_spawnattr_destroy(&attributes) }
        // Only fds 0-2 reach the child.
        posix_spawnattr_setflags(&attributes, Int16(POSIX_SPAWN_CLOEXEC_DEFAULT))

        var actions: posix_spawn_file_actions_t?
        posix_spawn_file_actions_init(&actions)
        defer { posix_spawn_file_actions_destroy(&actions) }
        posix_spawn_file_actions_adddup2(&actions, input.fileHandleForReading.fileDescriptor, 0)
        posix_spawn_file_actions_adddup2(&actions, output.fileHandleForWriting.fileDescriptor, 1)
        posix_spawn_file_actions_adddup2(&actions, errors.fileHandleForWriting.fileDescriptor, 2)
        posix_spawn_file_actions_addchdir_np(&actions, directory.path)

        let argv = ([executable.path] + arguments).map { strdup($0) } + [nil]
        let envp = environment.map { strdup("\($0.key)=\($0.value)") } + [nil]
        defer { (argv + envp).forEach { free($0) } }

        var child: pid_t = 0
        let result = posix_spawn(&child, executable.path, &actions, &attributes, argv, envp)
        // The child has its own copies; close ours so EOF works both ways.
        try? input.fileHandleForReading.close()
        try? output.fileHandleForWriting.close()
        try? errors.fileHandleForWriting.close()
        guard result == 0 else { throw SpawnError.failed(result) }

        pid = child
        stdin = input.fileHandleForWriting
        stdout = output.fileHandleForReading
        stderr = errors.fileHandleForReading
        exitSource = DispatchSource.makeProcessSource(identifier: child, eventMask: .exit, queue: .global())
        exitSource.setEventHandler { [weak self] in self?.reap() }
        exitSource.resume()
        reap(blocking: false)  // in case it already exited before the source was armed
    }

    /// Exit status (or 128 + signal), once the process has exited.
    var terminationStatus: Int32? { lock.withLock { status } }

    /// Calls `handler` once with the exit status (immediately if it already exited).
    func onExit(_ handler: @escaping @Sendable (Int32) -> Void) {
        let done: Int32? = lock.withLock {
            if let status { return status }
            exitHandlers.append(handler)
            return nil
        }
        if let done { handler(done) }
    }

    func terminate() { kill(pid, SIGTERM) }

    private func reap(blocking: Bool = true) {
        var raw: Int32 = 0
        let r = waitpid(pid, &raw, blocking ? 0 : WNOHANG)
        guard r == pid else { return }
        let code: Int32 = (raw & 0x7f) == 0 ? (raw >> 8) & 0xff : 128 + (raw & 0x7f)
        let handlers: [@Sendable (Int32) -> Void] = lock.withLock {
            guard status == nil else { return [] }
            status = code
            defer { exitHandlers = [] }
            return exitHandlers
        }
        exitSource.cancel()
        handlers.forEach { $0(code) }
    }
}
