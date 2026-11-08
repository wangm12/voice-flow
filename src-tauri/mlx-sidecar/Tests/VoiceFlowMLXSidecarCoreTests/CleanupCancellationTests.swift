import Darwin
import Foundation
import XCTest

@testable import VoiceFlowMLXSidecarCore

final class CleanupCancellationTests: XCTestCase {
  func testCancelJoinsOwnedGenerationWorkerBeforeTerminalCommit() async {
    let lifetime = CleanupGenerationLifetime()
    let barrier = CancellationWorkerBarrier()
    let (stream, continuation) = AsyncStream<Int>.makeStream()
    let worker = Task {
      await barrier.run()
      continuation.finish()
    }
    lifetime.install(worker)
    let original = makeRequest(requestId: "cleanup-1", op: "cleanup")
    let operation = makeOperation(original, lifetime: lifetime)

    let consumer = Task {
      for await _ in stream {}
    }
    continuation.finish()
    await consumer.value

    XCTAssertEqual(
      operation.acceptCancel(
        makeRequest(requestId: "cancel-1", op: "cancel"),
        modelId: "model",
        modelRevision: "rev"
      ),
      .accepted
    )
    guard await barrier.waitForCancellation() else {
      await barrier.release()
      await operation.settleWorker()
      XCTFail("worker did not observe cancellation")
      return
    }
    let joined = expectation(description: "underlying worker joined")
    joined.isInverted = true
    let joinTask = Task {
      await operation.settleWorker()
      _ = operation.commitTerminal()
      joined.fulfill()
    }
    await fulfillment(of: [joined], timeout: 0.05)

    await barrier.release()
    await joinTask.value
    await operation.settleWorker()
    let settlement = operation.commitTerminal()
    XCTAssertTrue(settlement.cancelled)
    XCTAssertEqual(settlement.acknowledgements.map(\.requestId), ["cancel-1"])
  }

  func testCancellationBeforeWorkerInstallationCancelsInstalledWorker() async {
    let lifetime = CleanupGenerationLifetime()
    let barrier = CancellationWorkerBarrier()
    let original = makeRequest(requestId: "warmup-1", op: "warmup")
    let operation = makeOperation(original, lifetime: lifetime)
    XCTAssertEqual(
      operation.acceptCancel(
        makeRequest(requestId: "cancel-2", op: "cancel"),
        modelId: "model",
        modelRevision: "rev"
      ),
      .accepted
    )

    let worker = Task { await barrier.run() }
    lifetime.install(worker)
    guard await barrier.waitForCancellation() else {
      await barrier.release()
      await lifetime.settle()
      XCTFail("late-installed worker did not observe cancellation")
      return
    }
    await barrier.release()
    await operation.settleWorker()
    let settlement = operation.commitTerminal()
    XCTAssertTrue(settlement.cancelled)
    XCTAssertEqual(settlement.acknowledgements.map(\.requestId), ["cancel-2"])
  }

  func testCancelIdentityIsBoundAndDuplicatesAreIdempotent() async {
    let request = makeRequest(requestId: "cleanup-3", op: "cleanup")
    let operation = makeOperation(request, lifetime: CleanupGenerationLifetime())
    let wrongSession = SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: "cancel-wrong",
      sessionId: "stale-session",
      modelVersion: "rev",
      op: "cancel",
      modelId: "model"
    )
    XCTAssertEqual(
      operation.acceptCancel(wrongSession, modelId: "model", modelRevision: "rev"),
      .rejected
    )
    let cancel = makeRequest(requestId: "cancel-3", op: "cancel")
    XCTAssertEqual(
      operation.acceptCancel(cancel, modelId: "model", modelRevision: "rev"),
      .accepted
    )
    XCTAssertEqual(
      operation.acceptCancel(cancel, modelId: "model", modelRevision: "rev"),
      .duplicate
    )
    await operation.settleWorker()
    let settlement = operation.commitTerminal()
    XCTAssertEqual(settlement.acknowledgements.map(\.requestId), ["cancel-3"])
    XCTAssertEqual(
      operation.acceptCancel(
        makeRequest(requestId: "cancel-late", op: "cancel"),
        modelId: "model",
        modelRevision: "rev"
      ),
      .rejected
    )
  }

  func testInjectedJSONLineReaderPreservesSplitLinesCRLFAndPartialEOF() async throws {
    var descriptors = [Int32](repeating: 0, count: 2)
    guard pipe(&descriptors) == 0 else {
      XCTFail("pipe setup failed")
      return
    }
    let readDescriptor = descriptors[0]
    let writeDescriptor = descriptors[1]
    let writer = Task.detached {
      for bytes in [Data("first\r".utf8), Data("\nsecond".utf8)] {
        _ = bytes.withUnsafeBytes { rawBuffer in
          _ = Darwin.write(writeDescriptor, rawBuffer.baseAddress, rawBuffer.count)
        }
      }
      _ = Darwin.close(writeDescriptor)
    }
    let reader = JSONLineReader(
      maximumLineBytes: 32,
      descriptor: readDescriptor,
      ownsDescriptor: true
    )
    XCTAssertEqual(try reader.nextLine(), Data("first".utf8))
    XCTAssertEqual(try reader.nextLine(), Data("second".utf8))
    XCTAssertNil(try reader.nextLine())
    reader.cancel()
    reader.closeOwnedDescriptor()
    await writer.value
  }

  func testInjectedWriterSerializesConcurrentWholeJSONLFrames() async throws {
    let fileURL = FileManager.default.temporaryDirectory
      .appendingPathComponent("voiceflow-sidecar-frames-\(UUID().uuidString).jsonl")
    let descriptor = Darwin.open(
      fileURL.path,
      O_RDWR | O_CREAT | O_EXCL | O_CLOEXEC,
      mode_t(0o600)
    )
    guard descriptor >= 0 else {
      XCTFail("temporary JSONL file setup failed")
      return
    }
    defer {
      _ = Darwin.close(descriptor)
      try? FileManager.default.removeItem(at: fileURL)
    }
    let responses = (0..<8).map { index in
      let request = SidecarRequest(
        protocolVersion: sidecarProtocolVersion,
        requestId: "writer-\(index)",
        sessionId: "writer-session",
        modelVersion: "rev",
        op: "cleanup"
      )
      let result = CleanupResult(
        text: String(repeating: "x", count: 12_000),
        promptTokens: 1,
        generationTokens: 1,
        promptTimeSeconds: 0.1,
        generationTimeSeconds: 0.1,
        completion: "stop"
      )
      return SidecarResponse(
        request: request,
        status: "completed",
        platformSupported: true,
        cleanupResult: result
      )
    }
    await withTaskGroup(of: Void.self) { group in
      for response in responses {
        group.addTask {
          SidecarIO.writeResponse(response, maximumBytes: 65_535, to: descriptor)
        }
      }
    }
    XCTAssertEqual(Darwin.lseek(descriptor, 0, SEEK_SET), 0)
    let lineReader = JSONLineReader(maximumLineBytes: 65_535, descriptor: descriptor)
    var lines: [Data] = []
    while true {
      let nextLine: Data?
      do { nextLine = try lineReader.nextLine() }
      catch { break }
      guard let nextLine else { break }
      lines.append(nextLine)
    }

    XCTAssertEqual(lines.count, responses.count)
    let requestIds = try lines.map { line -> String in
      let object = try XCTUnwrap(JSONSerialization.jsonObject(with: line) as? [String: Any])
      return try XCTUnwrap(object["request_id"] as? String)
    }
    XCTAssertEqual(Set(requestIds), Set((0..<8).map { "writer-\($0)" }))
  }

  private func makeRequest(requestId: String, op: String) -> SidecarRequest {
    SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: requestId,
      sessionId: "session-a",
      modelVersion: "rev",
      op: op,
      modelId: "model"
    )
  }

  private func makeOperation(
    _ request: SidecarRequest,
    lifetime: CleanupGenerationLifetime
  ) -> CleanupActiveOperation {
    let task = Task {
      SidecarResponse(request: request, status: "completed", platformSupported: true)
    }
    return CleanupActiveOperation(
      id: "test-operation",
      request: request,
      lifetime: lifetime,
      task: task
    )
  }
}

private actor CancellationWorkerBarrier {
  private var cancelled = false
  private var released = false
  private var cancellationWaiters: [(UUID, CheckedContinuation<Bool, Never>)] = []
  private var releaseWaiters: [CheckedContinuation<Void, Never>] = []

  func run() async {
    await withTaskCancellationHandler {
      await withCheckedContinuation { continuation in
        if released { continuation.resume() }
        else { releaseWaiters.append(continuation) }
      }
    } onCancel: {
      Task { await self.observeCancellation() }
    }
  }

  func waitForCancellation() async -> Bool {
    if cancelled { return true }
    let id = UUID()
    return await withCheckedContinuation { continuation in
      cancellationWaiters.append((id, continuation))
      Task {
        try? await Task.sleep(nanoseconds: 1_000_000_000)
        await self.timeoutCancellationWaiter(id)
      }
    }
  }

  func release() {
    released = true
    let pending = releaseWaiters
    releaseWaiters.removeAll()
    for waiter in pending { waiter.resume() }
  }

  private func observeCancellation() {
    cancelled = true
    let pending = cancellationWaiters
    cancellationWaiters.removeAll()
    for (_, waiter) in pending { waiter.resume(returning: true) }
  }

  private func timeoutCancellationWaiter(_ id: UUID) {
    guard let index = cancellationWaiters.firstIndex(where: { $0.0 == id }) else { return }
    let (_, waiter) = cancellationWaiters.remove(at: index)
    waiter.resume(returning: false)
  }
}
