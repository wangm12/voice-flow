import Darwin
import Foundation

private enum ASRInputEvent: Sendable {
    case line(Data)
    case eof
    case lineTooLarge
    case readFailed
}

private final class ASRInputOverflow: @unchecked Sendable {
    private let lock = NSLock()
    private var didOverflow = false

    func mark() {
        lock.lock()
        didOverflow = true
        lock.unlock()
    }

    var occurred: Bool {
        lock.lock()
        defer { lock.unlock() }
        return didOverflow
    }
}

protocol ASRRuntimeHandling: Sendable {
    func releaseForShutdown() async
    func handleControl(_ request: SidecarRequest) async -> SidecarResponse
    func transcribe(_ request: SidecarRequest, cancellation: ASRRequestCancellation) async -> SidecarResponse
}

public final class SidecarServer: Sendable {
    private let runtime: ASRModelRuntime
    private let dispatcher: ASRRequestDispatcher

    public init(modelsRootPath: String, audioRootPath: String) {
        let runtime = ASRModelRuntime(modelsRootPath: modelsRootPath, audioRootPath: audioRootPath)
        self.runtime = runtime
        self.dispatcher = ASRRequestDispatcher(runtime: runtime)
    }

    public func run() async {
        let inputDescriptor = dup(STDIN_FILENO)
        guard inputDescriptor >= 0 else {
            SidecarIO.writeDiagnostic(code: "protocol_io_failed")
            await runtime.releaseForShutdown()
            return
        }
        let input = JSONLineReader(
            maximumLineBytes: ASRModelRuntime.maximumInputBytes,
            descriptor: inputDescriptor,
            ownsDescriptor: true
        )
        let (events, continuation) = AsyncStream<ASRInputEvent>.makeStream(
            bufferingPolicy: .bufferingOldest(4)
        )
        let overflow = ASRInputOverflow()
        let readerTask = Task.detached(priority: .utility) {
            while !Task.isCancelled {
                do {
                    guard let line = try input.nextLine() else {
                        if case .dropped = continuation.yield(.eof) { overflow.mark() }
                        break
                    }
                    if case .dropped = continuation.yield(.line(line)) {
                        overflow.mark()
                        break
                    }
                } catch JSONLineReaderError.lineTooLarge {
                    if case .dropped = continuation.yield(.lineTooLarge) { overflow.mark() }
                    break
                } catch {
                    if !Task.isCancelled, case .dropped = continuation.yield(.readFailed) {
                        overflow.mark()
                    }
                    break
                }
            }
            continuation.finish()
        }

        var shouldQuit = false
        for await event in events {
            switch event {
            case .line(let line):
                do {
                    let request = try SidecarProtocolCodec.decodeRequest(line)
                    shouldQuit = await dispatcher.route(request)
                } catch {
                    await dispatcher.writeMalformedRequest()
                }
            case .eof:
                break
            case .lineTooLarge:
                await dispatcher.writeMalformedRequest()
            case .readFailed:
                SidecarIO.writeDiagnostic(code: "protocol_io_failed")
            }
            if shouldQuit { break }
            if case .eof = event { break }
            if case .lineTooLarge = event { break }
            if case .readFailed = event { break }
        }

        input.cancel()
        readerTask.cancel()
        await readerTask.value
        input.closeOwnedDescriptor()
        if overflow.occurred { SidecarIO.writeDiagnostic(code: "protocol_input_overflow") }
        await dispatcher.finishInput()
    }
}

actor ASRRequestDispatcher {
    private struct ActiveOperation {
        let id: UUID
        let request: SidecarRequest
        let cancellation: ASRRequestCancellation
        var task: Task<SidecarResponse, Never>?
    }

    private let runtime: any ASRRuntimeHandling
    private let responseWriter: @Sendable (SidecarResponse) -> Void
    private let beforeWorkerInstall: @Sendable () async -> Void
    private var activeOperation: ActiveOperation?
    private var activeWaiters: [CheckedContinuation<Void, Never>] = []
    private var controlRequestActive = false
    private var stopping = false

    init(
        runtime: any ASRRuntimeHandling,
        beforeWorkerInstall: @escaping @Sendable () async -> Void = {},
        responseWriter: @escaping @Sendable (SidecarResponse) -> Void = {
            SidecarIO.writeResponse($0, maximumBytes: ASRModelRuntime.maximumResponseBytes)
        }
    ) {
        self.runtime = runtime
        self.beforeWorkerInstall = beforeWorkerInstall
        self.responseWriter = responseWriter
    }

    func route(_ request: SidecarRequest) async -> Bool {
        guard !stopping else { return true }
        if request.op == "cancel" {
            guard let active = activeOperation,
                  active.cancellation.accept(request) else {
                write(invalidRequestResponse(request))
                return false
            }
            active.task?.cancel()
            return false
        }

        if request.op == "quit" && !validControlEnvelope(request) {
            write(invalidRequestResponse(request))
            return false
        }

        if let active = activeOperation {
            if request.op == "quit" {
                active.cancellation.cancelForShutdown()
                active.task?.cancel()
                await waitForActiveOperation()
                let response = await runtime.handleControl(request)
                write(response)
                stopping = true
                return true
            }
            write(invalidRequestResponse(request))
            return false
        }

        guard !controlRequestActive else {
            write(invalidRequestResponse(request))
            return false
        }

        if request.op == "transcribe" {
            let id = UUID()
            let cancellation = ASRRequestCancellation(original: request)
            activeOperation = ActiveOperation(
                id: id,
                request: request,
                cancellation: cancellation,
                task: nil
            )
            let runtime = self.runtime
            let task = Task.detached(priority: .userInitiated) {
                await withTaskCancellationHandler {
                    await runtime.transcribe(request, cancellation: cancellation)
                } onCancel: {
                    cancellation.cancelForShutdown()
                }
            }
            await beforeWorkerInstall()
            if activeOperation?.id == id {
                activeOperation?.task = task
                if cancellation.isCancellationRequested { task.cancel() }
            }
            Task {
                let response = await task.value
                self.finishOperation(id: id, response: response)
            }
            return false
        }

        controlRequestActive = true
        defer { controlRequestActive = false }
        let response = await runtime.handleControl(request)
        write(response)
        if request.op == "quit" {
            stopping = true
            return true
        }
        return false
    }

    func writeMalformedRequest() {
        let request = SidecarRequest(
            protocolVersion: sidecarProtocolVersion,
            requestId: "invalid",
            sessionId: nil,
            modelVersion: nil,
            op: "invalid"
        )
        write(invalidRequestResponse(request))
    }

    func finishInput() async {
        stopping = true
        activeOperation?.cancellation.cancelForShutdown()
        activeOperation?.task?.cancel()
        await waitForActiveOperation()
        await runtime.releaseForShutdown()
    }

    private func finishOperation(id: UUID, response: SidecarResponse) {
        guard let active = activeOperation, active.id == id else { return }
        let cancelRequest = active.cancellation.acceptedCancelRequest
        let terminal: SidecarResponse
        if active.cancellation.isCancellationRequested {
            terminal = SidecarResponse(
                request: active.request,
                status: "cancelled",
                platformSupported: response.platformSupported,
                loadedModelId: response.loadedModelId,
                loadedModelVersion: response.loadedModelVersion,
                loadedAttestation: response.loadedAttestation
            )
        } else {
            terminal = response
        }
        write(terminal)
        if let cancelRequest {
            write(SidecarResponse(
                request: cancelRequest,
                status: "cancelled",
                platformSupported: terminal.platformSupported,
                loadedModelId: terminal.loadedModelId,
                loadedModelVersion: terminal.loadedModelVersion,
                loadedAttestation: terminal.loadedAttestation
            ))
        }
        activeOperation = nil
        let waiters = activeWaiters
        activeWaiters.removeAll()
        for waiter in waiters { waiter.resume() }
    }

    private func waitForActiveOperation() async {
        guard activeOperation != nil else { return }
        await withCheckedContinuation { continuation in
            if activeOperation == nil { continuation.resume() }
            else { activeWaiters.append(continuation) }
        }
    }

    private func write(_ response: SidecarResponse) {
        responseWriter(response)
    }

    private func invalidRequestResponse(_ request: SidecarRequest) -> SidecarResponse {
        SidecarResponse(
            request: request,
            status: "error",
            platformSupported: ASRModelRuntime.platformSupported,
            error: SidecarFailure(code: "invalid_request", message: "The sidecar request is invalid.")
        )
    }

    private func validControlEnvelope(_ request: SidecarRequest) -> Bool {
        validSidecarIdentifier(request.requestId)
            && (request.sessionId.map(validSidecarIdentifier) ?? true)
            && request.protocolVersion == sidecarProtocolVersion
    }
}
