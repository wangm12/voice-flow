import Foundation
import XCTest

@testable import VoiceFlowMLXSidecarCore

final class ASRCancellationTests: XCTestCase {
  func testDispatcherJoinsSynchronizedWorkerBeforeOrderedPairAndAdmitsNPlusOneAfterAck() async {
    let gate = ASRDispatchWorkerGate()
    defer {
      gate.releaseWorker.signal()
      gate.releaseSynchronization.signal()
      gate.releaseTerminal.signal()
    }
    let recorder = ASRDispatchResponseRecorder()
    let runtime = ASRDispatchFixtureRuntime(gate: gate)
    let dispatcher = ASRRequestDispatcher(runtime: runtime, responseWriter: { response in
      recorder.append(response)
      if response.requestId == "transcribe-1" && response.status == "cancelled" {
        gate.terminalWritten.signal()
        gate.releaseTerminal.wait()
      }
    })
    let original = request(id: "transcribe-1", op: "transcribe")
    let cancel = request(id: "cancel-1", op: "cancel")
    let next = request(id: "transcribe-2", op: "transcribe")

    let originalShouldQuit = await dispatcher.route(original)
    XCTAssertFalse(originalShouldQuit)
    XCTAssertEqual(gate.workerStarted.wait(timeout: .now() + 2), .success)
    let cancelShouldQuit = await dispatcher.route(cancel)
    XCTAssertFalse(cancelShouldQuit)
    XCTAssertTrue(recorder.snapshot.isEmpty, "no terminal or acknowledgement may precede worker settlement")
    XCTAssertEqual(runtime.operationCount, 1, "the operation closure was entered, then its cancellation check rejected work")

    gate.releaseWorker.signal()
    XCTAssertEqual(gate.synchronizationEntered.wait(timeout: .now() + 2), .success)
    XCTAssertTrue(recorder.snapshot.isEmpty, "terminal must wait while the owning worker is still synchronizing")
    XCTAssertEqual(runtime.shutdownReleaseCount, 0, "shutdown release must wait for synchronization")
    gate.releaseSynchronization.signal()
    XCTAssertEqual(gate.workerSynchronized.wait(timeout: .now() + 2), .success)
    XCTAssertEqual(gate.terminalWritten.wait(timeout: .now() + 2), .success)
    XCTAssertEqual(recorder.snapshot.map(\.requestId), ["transcribe-1"])

    let nextRoute = Task { await dispatcher.route(next) }
    try? await Task.sleep(for: .milliseconds(40))
    XCTAssertEqual(recorder.snapshot.map(\.requestId), ["transcribe-1"], "N+1 must wait for the bound acknowledgement")
    gate.releaseTerminal.signal()
    let nextShouldQuit = await nextRoute.value
    XCTAssertFalse(nextShouldQuit)
    let reachedThreeResponses = await recorder.waitForCount(3)
    XCTAssertTrue(reachedThreeResponses, "N+1 did not finish after the cancellation pair")

    let responses = recorder.snapshot
    XCTAssertEqual(responses.map(\.requestId), ["transcribe-1", "cancel-1", "transcribe-2"])
    XCTAssertEqual(responses.map(\.status), ["cancelled", "cancelled", "completed"])
    XCTAssertEqual(runtime.transcriptionCount, 2)
    XCTAssertEqual(runtime.operationCount, 2, "the worker closure was entered for N and N+1")
    XCTAssertEqual(runtime.shutdownReleaseCount, 0, "the request owner remains live through N+1")
  }

  func testCancelBeforeWorkerHandleInstallSettlesWithoutStartingSyntheticOperation() async {
    let gate = ASRDispatchWorkerGate()
    defer {
      gate.releaseWorker.signal()
      gate.releaseInstallation.signal()
    }
    let recorder = ASRDispatchResponseRecorder()
    let runtime = ASRDispatchFixtureRuntime(gate: gate)
    let dispatcher = ASRRequestDispatcher(
      runtime: runtime,
      beforeWorkerInstall: {
        await withCheckedContinuation { continuation in
          DispatchQueue.global().async {
            gate.installationEntered.signal()
            gate.releaseInstallation.wait()
            continuation.resume()
          }
        }
      },
      responseWriter: recorder.append
    )
    let originalRequest = request(id: "transcribe-preinstall", op: "transcribe")
    let route = Task { await dispatcher.route(originalRequest) }
    XCTAssertEqual(gate.installationEntered.wait(timeout: .now() + 2), .success)
    XCTAssertEqual(gate.workerStarted.wait(timeout: .now() + 2), .success)
    let cancelShouldQuit = await dispatcher.route(cancel(requestId: "cancel-preinstall"))
    XCTAssertFalse(cancelShouldQuit)
    XCTAssertTrue(recorder.snapshot.isEmpty)

    gate.releaseInstallation.signal()
    let routeShouldQuit = await route.value
    XCTAssertFalse(routeShouldQuit)
    gate.releaseWorker.signal()
    let reachedTwoResponses = await recorder.waitForCount(2)
    XCTAssertTrue(reachedTwoResponses)
    XCTAssertEqual(gate.workerSynchronized.wait(timeout: .now() + 2), .success)
    XCTAssertEqual(runtime.operationCount, 1)
    XCTAssertEqual(recorder.snapshot.map(\.status), ["cancelled", "cancelled"])
  }

  func testCancellationBeforeWorkerEntryCheckSkipsTheSyntheticOperation() async {
    let gate = ASRDispatchWorkerGate()
    defer { gate.releaseWorkerEntry.signal() }
    let recorder = ASRDispatchResponseRecorder()
    let runtime = ASRDispatchFixtureRuntime(gate: gate, holdBeforeWorkerEntry: true)
    let dispatcher = ASRRequestDispatcher(runtime: runtime, responseWriter: recorder.append)
    let originalRequest = request(id: "transcribe-entry", op: "transcribe")
    let cancelRequest = cancel(requestId: "cancel-entry")
    let originalRoute = Task {
      await dispatcher.route(originalRequest)
    }
    XCTAssertEqual(gate.workerEntryHeld.wait(timeout: .now() + 2), .success)
    let cancelShouldQuit = await dispatcher.route(cancelRequest)
    XCTAssertFalse(cancelShouldQuit)
    XCTAssertTrue(runtime.cancellationRequested)
    XCTAssertTrue(recorder.snapshot.isEmpty)

    gate.releaseWorkerEntry.signal()
    let originalShouldQuit = await originalRoute.value
    XCTAssertFalse(originalShouldQuit)
    let reachedPair = await recorder.waitForCount(2)
    XCTAssertTrue(reachedPair)
    XCTAssertEqual(gate.workerSynchronized.wait(timeout: .now() + 2), .success)
    XCTAssertEqual(runtime.operationCount, 0, "pre-entry cancellation prevents worker closure invocation")
    XCTAssertEqual(recorder.snapshot.map(\.status), ["cancelled", "cancelled"])
  }

  func testCompletionWinsWhenCancelArrivesAfterTheOriginalTerminal() async {
    let gate = ASRDispatchWorkerGate()
    defer { gate.releaseWorker.signal() }
    let recorder = ASRDispatchResponseRecorder()
    let runtime = ASRDispatchFixtureRuntime(gate: gate)
    let dispatcher = ASRRequestDispatcher(runtime: runtime, responseWriter: recorder.append)
    let originalShouldQuit = await dispatcher.route(request(id: "transcribe-complete", op: "transcribe"))
    XCTAssertFalse(originalShouldQuit)
    XCTAssertEqual(gate.workerStarted.wait(timeout: .now() + 2), .success)
    gate.releaseWorker.signal()
    let reachedCompletedTerminal = await recorder.waitForCount(1)
    XCTAssertTrue(reachedCompletedTerminal)
    let lateCancelShouldQuit = await dispatcher.route(cancel(requestId: "cancel-after-complete"))
    XCTAssertFalse(lateCancelShouldQuit)
    let reachedLateCancelResponse = await recorder.waitForCount(2)
    XCTAssertTrue(reachedLateCancelResponse)
    XCTAssertEqual(recorder.snapshot.map(\.requestId), ["transcribe-complete", "cancel-after-complete"])
    XCTAssertEqual(recorder.snapshot.map(\.status), ["completed", "error"])
    XCTAssertFalse(runtime.cancellationRequested)
  }

  func testInvalidQuitDoesNotCancelActiveWorkerOrStopIdleDispatcher() async {
    let gate = ASRDispatchWorkerGate()
    defer { gate.releaseWorker.signal() }
    let recorder = ASRDispatchResponseRecorder()
    let runtime = ASRDispatchFixtureRuntime(gate: gate)
    let dispatcher = ASRRequestDispatcher(runtime: runtime, responseWriter: recorder.append)
    let originalRequest = request(id: "transcribe-valid", op: "transcribe")
    let originalRoute = Task {
      await dispatcher.route(originalRequest)
    }
    XCTAssertEqual(gate.workerStarted.wait(timeout: .now() + 2), .success)
    let badVersionShouldQuit = await dispatcher.route(quit(requestId: "bad-version", protocolVersion: 99))
    XCTAssertFalse(badVersionShouldQuit)
    let emptyIdShouldQuit = await dispatcher.route(quit(requestId: ""))
    XCTAssertFalse(emptyIdShouldQuit)
    XCTAssertFalse(runtime.cancellationRequested)
    XCTAssertTrue(recorder.snapshot.allSatisfy { $0.status == "error" })

    gate.releaseWorker.signal()
    let originalShouldQuit = await originalRoute.value
    XCTAssertFalse(originalShouldQuit)
    let reachedThreeResponses = await recorder.waitForCount(3)
    XCTAssertTrue(reachedThreeResponses)
    XCTAssertEqual(recorder.snapshot.first(where: { $0.requestId == "transcribe-valid" })?.status, "completed")
    let statusShouldQuit = await dispatcher.route(request(id: "after-invalid-quit", op: "status"))
    XCTAssertFalse(statusShouldQuit)
    let reachedFourResponses = await recorder.waitForCount(4)
    XCTAssertTrue(reachedFourResponses)

    let idleRecorder = ASRDispatchResponseRecorder()
    let idleRuntime = ASRDispatchFixtureRuntime(gate: ASRDispatchWorkerGate())
    let idleDispatcher = ASRRequestDispatcher(runtime: idleRuntime, responseWriter: idleRecorder.append)
    let idleInvalidQuit = await idleDispatcher.route(quit(requestId: "", protocolVersion: sidecarProtocolVersion))
    XCTAssertFalse(idleInvalidQuit)
    let idleStatusShouldQuit = await idleDispatcher.route(request(id: "still-open", op: "status"))
    XCTAssertFalse(idleStatusShouldQuit)
    let idleValidQuitShouldQuit = await idleDispatcher.route(quit(requestId: "valid-quit"))
    XCTAssertTrue(idleValidQuitShouldQuit)
    XCTAssertEqual(idleRecorder.snapshot.map(\.status), ["error", "ready", "ready"])
  }

  func testValidQuitAndEOFJoinHeldWorkerBeforeShutdown() async {
    for useQuit in [false, true] {
      let gate = ASRDispatchWorkerGate()
      defer {
        gate.releaseWorker.signal()
        gate.releaseSynchronization.signal()
      }
      let recorder = ASRDispatchResponseRecorder()
      let runtime = ASRDispatchFixtureRuntime(gate: gate)
      let dispatcher = ASRRequestDispatcher(runtime: runtime, responseWriter: recorder.append)
      let activeQuitRequest = quit(requestId: "valid-quit-active")
      let originalShouldQuit = await dispatcher.route(request(id: "transcribe-shutdown", op: "transcribe"))
      XCTAssertFalse(originalShouldQuit)
      XCTAssertEqual(gate.workerStarted.wait(timeout: .now() + 2), .success)
      let settlement: Task<Bool?, Never>
      if useQuit {
        settlement = Task { await dispatcher.route(activeQuitRequest) }
      } else {
        settlement = Task {
          await dispatcher.finishInput()
          return nil
        }
      }
      let cancellationDeadline = Date().addingTimeInterval(2)
      while !runtime.cancellationRequested && Date() < cancellationDeadline {
        try? await Task.sleep(for: .milliseconds(10))
      }
      XCTAssertTrue(runtime.cancellationRequested)
      XCTAssertTrue(recorder.snapshot.isEmpty, "shutdown must wait for the held actual worker")
      gate.releaseWorker.signal()
      XCTAssertEqual(gate.synchronizationEntered.wait(timeout: .now() + 2), .success)
      XCTAssertTrue(recorder.snapshot.isEmpty, "shutdown terminal must wait for actual device synchronization")
      XCTAssertEqual(runtime.shutdownReleaseCount, 0)
      gate.releaseSynchronization.signal()
      XCTAssertEqual(gate.workerSynchronized.wait(timeout: .now() + 2), .success)
      if useQuit {
        let shouldQuit = await settlement.value
        XCTAssertEqual(shouldQuit, true)
        XCTAssertEqual(recorder.snapshot.map(\.requestId), ["transcribe-shutdown", "valid-quit-active"])
        XCTAssertEqual(recorder.snapshot.map(\.status), ["cancelled", "ready"])
        await dispatcher.finishInput()
      } else {
        let eofResult = await settlement.value
        XCTAssertNil(eofResult)
        XCTAssertEqual(recorder.snapshot.map(\.requestId), ["transcribe-shutdown"])
        XCTAssertEqual(recorder.snapshot.map(\.status), ["cancelled"])
      }
      XCTAssertEqual(runtime.shutdownReleaseCount, 1)
    }
  }

  func testCancelMustMatchActiveTranscriptionIdentityAndBeFresh() {
    let original = SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: "transcribe-1",
      sessionId: "session-1",
      modelVersion: "revision-1",
      op: "transcribe",
      modelId: "model-1",
      audioPath: "/private/audio.wav",
      audioMaxBytes: 4096,
      maxTokensPerChunk: 512,
      maxTokensTotal: 1024
    )
    let cancellation = ASRRequestCancellation(original: original)

    XCTAssertFalse(cancellation.accept(cancel(requestId: "transcribe-1")))
    XCTAssertFalse(cancellation.isCancellationRequested)
    XCTAssertFalse(cancellation.accept(cancel(requestId: "cancel-1", sessionId: "stale-session")))
    XCTAssertFalse(cancellation.accept(cancel(requestId: "cancel-2", modelVersion: "stale-revision")))
    XCTAssertFalse(cancellation.accept(cancel(requestId: "cancel-3", modelId: "stale-model")))
    XCTAssertFalse(cancellation.accept(cancel(requestId: "cancel-4", audioPath: "/tmp/other.wav")))
    XCTAssertFalse(cancellation.isCancellationRequested)

    let accepted = cancel(requestId: "cancel-5")
    XCTAssertTrue(cancellation.accept(accepted))
    XCTAssertTrue(cancellation.isCancellationRequested)
    XCTAssertEqual(cancellation.acceptedCancelRequest?.requestId, accepted.requestId)
    XCTAssertFalse(cancellation.accept(cancel(requestId: "cancel-6")))
  }

  func testCancelMustBindARealSessionAndTranscribeRequest() {
    let noSession = SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: "transcribe-1",
      sessionId: nil,
      modelVersion: "revision-1",
      op: "transcribe",
      modelId: "model-1"
    )
    XCTAssertFalse(ASRRequestCancellation(original: noSession).accept(cancel(requestId: "cancel-1")))

    let notTranscribe = SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: "load-1",
      sessionId: "session-1",
      modelVersion: "revision-1",
      op: "load",
      modelId: "model-1"
    )
    XCTAssertFalse(ASRRequestCancellation(original: notTranscribe).accept(cancel(requestId: "cancel-2")))
  }

  private func cancel(
    requestId: String,
    sessionId: String = "session-1",
    modelVersion: String = "revision-1",
    modelId: String = "model-1",
    audioPath: String? = nil
  ) -> SidecarRequest {
    SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: requestId,
      sessionId: sessionId,
      modelVersion: modelVersion,
      op: "cancel",
      modelId: modelId,
      audioPath: audioPath
    )
  }

  private func request(id: String, op: String) -> SidecarRequest {
    SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: id,
      sessionId: "session-1",
      modelVersion: "revision-1",
      op: op,
      modelId: "model-1"
    )
  }

  private func quit(requestId: String, protocolVersion: Int = sidecarProtocolVersion) -> SidecarRequest {
    SidecarRequest(
      protocolVersion: protocolVersion,
      requestId: requestId,
      sessionId: nil,
      modelVersion: nil,
      op: "quit"
    )
  }
}

private final class ASRDispatchWorkerGate: @unchecked Sendable {
  let workerStarted = DispatchSemaphore(value: 0)
  let workerSynchronized = DispatchSemaphore(value: 0)
  let synchronizationEntered = DispatchSemaphore(value: 0)
  let releaseSynchronization = DispatchSemaphore(value: 0)
  let releaseWorker = DispatchSemaphore(value: 0)
  let workerEntryHeld = DispatchSemaphore(value: 0)
  let releaseWorkerEntry = DispatchSemaphore(value: 0)
  let terminalWritten = DispatchSemaphore(value: 0)
  let releaseTerminal = DispatchSemaphore(value: 0)
  let installationEntered = DispatchSemaphore(value: 0)
  let releaseInstallation = DispatchSemaphore(value: 0)
}

private final class ASRDispatchResponseRecorder: @unchecked Sendable {
  private let lock = NSLock()
  private var responses: [SidecarResponse] = []

  var snapshot: [SidecarResponse] {
    lock.lock()
    defer { lock.unlock() }
    return responses
  }

  func append(_ response: SidecarResponse) {
    lock.lock()
    responses.append(response)
    lock.unlock()
  }

  func waitForCount(_ count: Int) async -> Bool {
    let deadline = Date().addingTimeInterval(2)
    while Date() < deadline {
      if snapshot.count >= count { return true }
      try? await Task.sleep(for: .milliseconds(10))
    }
    return snapshot.count >= count
  }
}

private final class ASRDispatchFixtureRuntime: ASRRuntimeHandling, @unchecked Sendable {
  private let gate: ASRDispatchWorkerGate
  private let holdBeforeWorkerEntry: Bool
  private let modelOwner: ASRModelRuntime
  private let lock = NSLock()
  private var transcriptions = 0
  private var operations = 0
  private var shutdownReleases = 0
  private var lastCancellation: ASRRequestCancellation?

  init(gate: ASRDispatchWorkerGate, holdBeforeWorkerEntry: Bool = false) {
    self.gate = gate
    self.holdBeforeWorkerEntry = holdBeforeWorkerEntry
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent("voiceflow-asr-owner-\(UUID().uuidString)", isDirectory: true)
    self.modelOwner = ASRModelRuntime(
      modelsRootPath: root.path,
      audioRootPath: root.appendingPathComponent("audio-tmp", isDirectory: true).path
    )
  }

  var transcriptionCount: Int {
    lock.lock()
    defer { lock.unlock() }
    return transcriptions
  }

  var operationCount: Int {
    lock.lock()
    defer { lock.unlock() }
    return operations
  }

  var shutdownReleaseCount: Int {
    lock.lock()
    defer { lock.unlock() }
    return shutdownReleases
  }

  var cancellationRequested: Bool {
    lock.lock()
    let cancellation = lastCancellation
    lock.unlock()
    return cancellation?.isCancellationRequested ?? false
  }

  func releaseForShutdown() async {
    lock.withLock { shutdownReleases += 1 }
    await modelOwner.releaseForShutdown()
  }

  func handleControl(_ request: SidecarRequest) async -> SidecarResponse {
    SidecarResponse(request: request, status: "ready", platformSupported: true)
  }

  func transcribe(_ request: SidecarRequest, cancellation: ASRRequestCancellation) async -> SidecarResponse {
    lock.withLock {
      transcriptions += 1
      lastCancellation = cancellation
    }
    if holdBeforeWorkerEntry {
      await withCheckedContinuation { continuation in
        DispatchQueue.global().async {
          self.gate.workerEntryHeld.signal()
          self.gate.releaseWorkerEntry.wait()
          continuation.resume()
        }
      }
    }
    do {
      let status = try await modelOwner.runSynchronousWorker(
        cancellation: cancellation,
        synchronize: {
          if request.requestId == "transcribe-1" || request.requestId == "transcribe-shutdown" {
            self.gate.synchronizationEntered.signal()
            self.gate.releaseSynchronization.wait()
          }
          self.gate.workerSynchronized.signal()
        },
        operation: {
          self.lock.withLock { self.operations += 1 }
          if request.requestId == "transcribe-1"
            || request.requestId == "transcribe-complete"
            || request.requestId == "transcribe-preinstall"
            || request.requestId == "transcribe-valid"
            || request.requestId == "transcribe-shutdown" {
            self.gate.workerStarted.signal()
            self.gate.releaseWorker.wait()
          }
          try cancellation.check()
          return "completed"
        }
      )
      return SidecarResponse(request: request, status: status, platformSupported: true)
    } catch {
      return SidecarResponse(request: request, status: "cancelled", platformSupported: true)
    }
  }
}
