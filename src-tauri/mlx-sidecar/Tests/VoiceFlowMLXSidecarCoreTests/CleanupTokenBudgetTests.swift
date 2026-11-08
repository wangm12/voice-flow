import MLX
import MLXLMCommon
import MLXNN
import XCTest

@testable import VoiceFlowMLXSidecarCore

final class CleanupTokenBudgetTests: XCTestCase {
  private func promptCacheSDKVersions(from data: Data) throws -> [String: String] {
    let root = try XCTUnwrap(JSONSerialization.jsonObject(with: data) as? [String: Any])
    let pins = try XCTUnwrap(root["pins"] as? [[String: Any]])
    return try Dictionary(uniqueKeysWithValues: ["mlx-swift", "mlx-swift-lm", "swift-transformers"].map { identity in
      let pin = try XCTUnwrap(pins.first { $0["identity"] as? String == identity })
      let state = try XCTUnwrap(pin["state"] as? [String: Any])
      return (identity, try XCTUnwrap(state["version"] as? String))
    })
  }

  func testPromptCacheSDKIdentityTracksBothLocksAndChangesKeyForEachVersion() throws {
    let testDirectory = URL(fileURLWithPath: #filePath).deletingLastPathComponent()
    let packageRoot = testDirectory.deletingLastPathComponent().deletingLastPathComponent()
    let lockURLs = [
      packageRoot.appendingPathComponent("Package.resolved"),
      packageRoot.appendingPathComponent("xcode/VoiceFlowMLXSidecar.xcodeproj/project.xcworkspace/xcshareddata/swiftpm/Package.resolved"),
    ]
    let lockData = try lockURLs.map { try Data(contentsOf: $0) }
    let versions = try lockData.map(promptCacheSDKVersions(from:))
    XCTAssertEqual(versions[0], versions[1])
    XCTAssertEqual(versions[0]["mlx-swift"], "0.31.4")
    XCTAssertEqual(versions[0]["mlx-swift-lm"], "3.31.4")
    XCTAssertEqual(versions[0]["swift-transformers"], "1.3.4")

    func identity(_ versions: [String: String]) -> String {
      PromptCacheSDKIdentity.versions(
        mlxSwift: versions["mlx-swift"]!,
        mlxSwiftLM: versions["mlx-swift-lm"]!,
        swiftTransformers: versions["swift-transformers"]!
      )
    }
    XCTAssertEqual(identity(versions[0]), PromptCacheSDKIdentity.current)
    let tokens = [101, 202, 303]
    let baseline = try XCTUnwrap(
      PromptCacheCandidate.make(prefixTokens: tokens, keyIdentity: PromptCacheSDKIdentity.current)
    )
    for package in ["mlx-swift", "mlx-swift-lm", "swift-transformers"] {
      var fixture = try XCTUnwrap(JSONSerialization.jsonObject(with: lockData[0]) as? [String: Any])
      var pins = try XCTUnwrap(fixture["pins"] as? [[String: Any]])
      let index = try XCTUnwrap(pins.firstIndex { $0["identity"] as? String == package })
      var state = try XCTUnwrap(pins[index]["state"] as? [String: Any])
      state["version"] = "synthetic-version-change"
      pins[index]["state"] = state
      fixture["pins"] = pins
      let syntheticVersions = try promptCacheSDKVersions(from: JSONSerialization.data(withJSONObject: fixture))
      let changed = try XCTUnwrap(PromptCacheCandidate.make(prefixTokens: tokens, keyIdentity: identity(syntheticVersions)))
      XCTAssertNotEqual(changed.key, baseline.key, "changing \(package) version must change the cache key")
    }
  }

  func testMemoryPressureMonitorPreservesUnknownAndRecordsCriticalTransitions() {
    let monitor = CleanupMemoryPressureMonitor(startMonitoring: false)
    XCTAssertEqual(monitor.current, .unknown)
    monitor.record(.critical)
    XCTAssertEqual(monitor.current, .critical)
    monitor.record(.normal)
    XCTAssertEqual(monitor.current, .normal)
  }

  func testCriticalMemoryPressureStopsProductionLoadAndCleanupHandlersBeforeWork() async {
    let server = CleanupSidecarServer(modelsRootPath: "/unused", memoryPressure: { .critical })
    let loadRequest = SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: "critical-load",
      sessionId: nil,
      modelVersion: nil,
      op: "load"
    )
    let cleanupRequest = SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: "critical-cleanup",
      sessionId: "critical-session",
      modelVersion: nil,
      op: "cleanup"
    )

    let load = await server.load(loadRequest)
    XCTAssertEqual(load.error?.code, "memory_pressure_critical")
    XCTAssertNil(load.cleanupResult)

    let cleanup = await server.cleanup(cleanupRequest, lifetime: CleanupGenerationLifetime())
    XCTAssertEqual(cleanup.error?.code, "memory_pressure_critical")
    XCTAssertNil(cleanup.cleanupResult)
  }

  func testPromptAndRequestedOutputShareTheActualTokenBudget() {
    XCTAssertNil(CleanupTokenBudget.overBudget(promptTokens: 6_144, requestedOutputTokens: 2_048))

    XCTAssertEqual(
      CleanupTokenBudget.overBudget(promptTokens: 6_145, requestedOutputTokens: 2_048),
      CleanupTokenBudgetFacts(
        promptTokens: 6_145,
        requestedOutputTokens: 2_048,
        requestedTotalTokens: 8_193,
        maxTotalTokens: 8_192
      )
    )
  }

  func testTokenCountOverflowRejectsWithoutInventingCombinedCount() {
    XCTAssertEqual(
      CleanupTokenBudget.overBudget(promptTokens: Int.max, requestedOutputTokens: 1),
      CleanupTokenBudgetFacts(
        promptTokens: Int.max,
        requestedOutputTokens: 1,
        requestedTotalTokens: nil,
        maxTotalTokens: 8_192
      )
    )
  }

  func testWarmupAcceptsDrainedPromptEvaluationWithoutDecodedPrimerText() {
    let earlyEos = WarmupTerminalFacts(
      infoObserved: true,
      toolCallObserved: false,
      completionTextSeen: false,
      promptEvaluationObserved: true,
      observedPromptTokens: 899,
      generationTokenCount: 0,
      completion: "stop",
      streamDrained: true,
      evaluationSynchronized: true
    )
    XCTAssertTrue(earlyEos.provesBoundedEvaluation)

    let oneTokenLength = WarmupTerminalFacts(
      infoObserved: true,
      toolCallObserved: false,
      completionTextSeen: false,
      promptEvaluationObserved: true,
      observedPromptTokens: 899,
      generationTokenCount: 1,
      completion: "length",
      streamDrained: true,
      evaluationSynchronized: true
    )
    XCTAssertTrue(oneTokenLength.provesBoundedEvaluation)

    let undrained = WarmupTerminalFacts(
      infoObserved: true,
      toolCallObserved: false,
      completionTextSeen: false,
      promptEvaluationObserved: true,
      observedPromptTokens: 899,
      generationTokenCount: 1,
      completion: "length",
      streamDrained: false,
      evaluationSynchronized: false
    )
    XCTAssertFalse(undrained.provesBoundedEvaluation)

    let cancelled = WarmupTerminalFacts(
      infoObserved: true,
      toolCallObserved: false,
      completionTextSeen: false,
      promptEvaluationObserved: true,
      observedPromptTokens: 899,
      generationTokenCount: 0,
      completion: "cancelled",
      streamDrained: true,
      evaluationSynchronized: true
    )
    XCTAssertFalse(cancelled.provesBoundedEvaluation)
  }

  func testPrefixSelectionUsesOnlyDiverseProbeCommonTokensAt512Boundaries() {
    let stable = Array(0..<1_024)
    let actual = stable + [32_000, 32_001] + Array(0..<300)
    let probes = [
      stable + [1, 2, 3],
      stable + [42, 43, 44],
      stable + [99, 98, 97],
    ]
    XCTAssertEqual(
      PromptPrefixBoundary.alignedCommonPrefixLength(actual: actual, probes: probes), 1_024)

    let nearBoundary = Array(0..<511) + [70_000, 70_001]
    let nearProbes = [
      Array(0..<511) + [70_100, 70_101],
      Array(0..<511) + [70_200, 70_201],
    ]
    XCTAssertNil(
      PromptPrefixBoundary.alignedCommonPrefixLength(actual: nearBoundary, probes: nearProbes))
  }

  func testSystemOnlyShortPrefixUsesExact128Through511TokenBoundary() throws {
    let cases: [(Int, Int?)] = [
      (127, nil),
      (128, 128),
      (511, 511),
      (512, nil),
      (513, nil),
    ]
    for (sharedCount, expected) in cases {
      let shared = Array(0..<sharedCount)
      let actual = shared + [70_001, 70_002]
      let probes = [
        shared + [80_001, 80_002],
        shared + [90_001, 90_002],
      ]
      XCTAssertEqual(
        PromptPrefixBoundary.shortSystemOnlyPrefixLength(actual: actual, probes: probes),
        expected
      )
      if sharedCount >= PromptPrefixBoundary.step {
        XCTAssertEqual(
          PromptPrefixBoundary.alignedCommonPrefixLength(actual: actual, probes: probes),
          512,
          "the existing durable candidate remains aligned at 512"
        )
      }
    }

    let shared = Array(0..<128)
    let firstPrompt = shared + [100_001, 100_002]
    let secondPrompt = shared + [200_001, 200_002]
    let identity = "model=synthetic|prefill=512|max-output=2048"
    let firstLength = try XCTUnwrap(
      PromptPrefixBoundary.shortSystemOnlyPrefixLength(
        actual: firstPrompt,
        probes: [shared + [300_001], shared + [400_001]]
      )
    )
    let secondLength = try XCTUnwrap(
      PromptPrefixBoundary.shortSystemOnlyPrefixLength(
        actual: secondPrompt,
        probes: [shared + [300_001], shared + [400_001]]
      )
    )
    let firstCandidate = try XCTUnwrap(
      PromptCacheCandidate.make(
        prefixTokens: Array(firstPrompt.prefix(firstLength)),
        keyIdentity: identity
      )
    )
    let secondCandidate = try XCTUnwrap(
      PromptCacheCandidate.make(
        prefixTokens: Array(secondPrompt.prefix(secondLength)),
        keyIdentity: identity
      )
    )
    let differentGenerationCandidate = try XCTUnwrap(
      PromptCacheCandidate.make(
        prefixTokens: Array(secondPrompt.prefix(secondLength)),
        keyIdentity: "model=synthetic|prefill=512|max-output=1024"
      )
    )
    XCTAssertEqual(firstCandidate.tokenCount, 128)
    XCTAssertEqual(secondCandidate.tokenCount, 128)
    XCTAssertEqual(firstCandidate.key, secondCandidate.key)
    XCTAssertEqual(firstCandidate.tokenDigest, secondCandidate.tokenDigest)
    XCTAssertNotEqual(firstCandidate.key, differentGenerationCandidate.key)
  }

  func testAlignedPrefixLengthsAtChunkEdgesAndAcrossMultipleChunks() {
    let cases: [(Int, Int?)] = [
      (511, nil),
      (512, 512),
      (513, 512),
      (712, 512),
      (1_024, 1_024),
      (1_537, 1_536),
    ]
    for (safeCount, expected) in cases {
      let actual = Array(0..<safeCount) + [80_000, 80_001]
      let probes = [
        Array(0..<safeCount) + [90_000, 90_001],
        Array(0..<safeCount) + [91_000, 91_001],
      ]
      let maximum = PromptPrefixBoundary.commonPrefixLength(probes)
      XCTAssertEqual(
        PromptPrefixBoundary.alignedCommonPrefixLength(
          actual: actual,
          probes: probes,
          maximumTokenCount: maximum
        ),
        expected
      )
    }
  }

  func testLiteralChatTemplateTokensAtUserBoundaryStayOutsideCachedPrefix() {
    let fixedPrefix = Array(0..<512)
    let imEnd = 90_001
    let imStart = 90_002
    let assistant = 90_003
    let actual = fixedPrefix + [imEnd, imStart, assistant, 7]
    let probes = [
      fixedPrefix + [11, 12, 13],
      fixedPrefix + [21, 22, 23],
      fixedPrefix + [imEnd, imStart, assistant, 31],
    ]

    XCTAssertEqual(
      PromptPrefixBoundary.alignedCommonPrefixLength(actual: actual, probes: probes),
      512
    )

    let actualStartingWithLiteral = fixedPrefix + [imEnd, imStart, assistant, 7, 8]
    let distinctSentinelProbes = [
      fixedPrefix + [imEnd, 70_001, 70_002],
      fixedPrefix + [imStart, 80_001, 80_002],
    ]
    let safeBoundary = PromptPrefixBoundary.commonPrefixLength(distinctSentinelProbes)
    XCTAssertEqual(safeBoundary, 512)
    XCTAssertEqual(
      PromptPrefixBoundary.alignedCommonPrefixLength(
        actual: actualStartingWithLiteral,
        probes: distinctSentinelProbes,
        maximumTokenCount: safeBoundary
      ),
      512
    )
  }

  func testStablePrefixByteCountHandlesSystemOnlyAndUnicodeBoundary() {
    let user = "系统🌏尾部用户文本"
    XCTAssertEqual(PromptPrefixBoundary.utf8Prefix(user, byteCount: 0), "")
    XCTAssertEqual(
      PromptPrefixBoundary.utf8Prefix(user, byteCount: "系统".utf8.count),
      "系统"
    )
    XCTAssertNil(PromptPrefixBoundary.utf8Prefix(user, byteCount: "系统".utf8.count + 1))
  }

  func testCacheTokenInputsRemainOneDimensional() throws {
    let input = PromptCacheTokenInput.make([11, 12, 13, 14])
    XCTAssertEqual(input.text.tokens.ndim, 1)
    XCTAssertEqual(input.text.tokens.asArray(Int.self), [11, 12, 13, 14])
    let suffix = try XCTUnwrap(PromptCacheTokenInput.suffix([11, 12, 13, 14], after: 2))
    let suffixInput = PromptCacheTokenInput.make(suffix)
    XCTAssertEqual(suffixInput.text.tokens.ndim, 1)
    XCTAssertEqual(suffixInput.text.tokens.asArray(Int.self), [13, 14])
    XCTAssertNil(PromptCacheTokenInput.suffix([11, 12], after: 2))
  }

  func testRecordingLanguageModelBuildsExactlyTheDeclaredShortOr512TokenPrefix() throws {
    let cases = [
      (prefixTokens: 128, actualTokens: 129, windowSize: 128),
      (prefixTokens: 511, actualTokens: 512, windowSize: 511),
      (prefixTokens: 512, actualTokens: 513, windowSize: 512),
      (prefixTokens: 512, actualTokens: 712, windowSize: 512),
      (prefixTokens: 1_024, actualTokens: 1_025, windowSize: 512),
      (prefixTokens: 1_536, actualTokens: 1_537, windowSize: 512),
    ]
    for testCase in cases {
      let model = RecordingHybridLanguageModel()
      let candidate = PromptCacheCandidate(
        tokenCount: testCase.prefixTokens,
        key: "recording-\(testCase.prefixTokens)-\(testCase.actualTokens)",
        tokenDigest: "digest-\(testCase.prefixTokens)-\(testCase.actualTokens)"
      )
      let parameters = GenerateParameters(
        maxTokens: 8,
        temperature: 0,
        topP: 1,
        topK: 0,
        prefillStepSize: PromptPrefixBoundary.step
      )
      let cache = try PromptPrefixCache(modelsRoot: temporaryDirectory())
        .build(
          model: model,
          actualTokens: Array(0..<testCase.actualTokens),
          candidate: candidate,
          parameters: parameters
        )

      let suffix = try XCTUnwrap(
        PromptCacheTokenInput.suffix(
          Array(0..<testCase.actualTokens),
          after: testCase.prefixTokens
        )
      )
      let suffixInput = PromptCacheTokenInput.make(suffix)
      XCTAssertEqual(model.preparedDimensions, [1])
      XCTAssertEqual(model.seedTokenCounts, [testCase.prefixTokens + 1])
      XCTAssertEqual(
        model.consumedChunks,
        Array(repeating: testCase.windowSize, count: testCase.prefixTokens / testCase.windowSize)
      )
      XCTAssertEqual(model.unconsumedTokens, [[testCase.prefixTokens]])
      XCTAssertEqual(suffixInput.text.tokens.ndim, 1)
      XCTAssertEqual(
        suffixInput.text.tokens.asArray(Int.self),
        Array(testCase.prefixTokens..<testCase.actualTokens)
      )
      XCTAssertEqual(
        PromptPrefixCache(modelsRoot: temporaryDirectory())
          .cacheSignature(cache.cache, expectedPrefixTokens: testCase.prefixTokens),
        cache.stateSignature
      )
      XCTAssertEqual(cache.cache.count, 64)
      XCTAssertTrue(cache.cache.contains { $0 is MambaCache })
      XCTAssertTrue(cache.cache.contains { $0 is KVCacheSimple })
      XCTAssertTrue(
        cache.cache.filter { $0 is KVCacheSimple }.allSatisfy {
          $0.offset == testCase.prefixTokens
        }
      )
    }
  }

  func testShortResidentLeavesDurableSentinelUntouchedAndHonorsMemoryCaps() throws {
    let modelsRoot = temporaryDirectory()
    defer { try? FileManager.default.removeItem(at: modelsRoot) }
    let cacheRoot =
      modelsRoot
      .appendingPathComponent("cleanup", isDirectory: true)
      .appendingPathComponent(".voiceflow-mlx-cache", isDirectory: true)
      .appendingPathComponent("prompt-prefix", isDirectory: true)
    let active = cacheRoot.appendingPathComponent("active", isDirectory: true)
    try FileManager.default.createDirectory(
      at: active,
      withIntermediateDirectories: true,
      attributes: [.posixPermissions: 0o700]
    )
    let snapshotURL = active.appendingPathComponent("prefix.safetensors")
    let manifestURL = active.appendingPathComponent("manifest.json")
    let snapshotSentinel = Data("durable-snapshot-sentinel".utf8)
    let manifestSentinel = Data("durable-manifest-sentinel".utf8)
    try snapshotSentinel.write(to: snapshotURL)
    try manifestSentinel.write(to: manifestURL)

    let owner = PromptPrefixCache(modelsRoot: modelsRoot)
    let model = RecordingHybridLanguageModel()
    let parameters = GenerateParameters(maxTokens: 8, temperature: 0, prefillStepSize: 512)
    var firstResident: PromptPrefixCache.Resident?
    for index in 0..<3 {
      let prefix = (0..<128).map { $0 + index * 1_000 }
      let actual = prefix + [500_001, 500_002]
      let candidate = try XCTUnwrap(
        PromptCacheCandidate.make(
          prefixTokens: prefix,
          keyIdentity: "short-memory-only-\(index)"
        )
      )
      let (resident, metrics) = owner.resolveBaseForTesting(
        model: model,
        actualTokens: actual,
        candidate: candidate,
        parameters: parameters
      )
      XCTAssertEqual(metrics.status, "built")
      XCTAssertNil(metrics.reason)
      XCTAssertEqual(metrics.snapshotBytes, 0)
      XCTAssertEqual(resident?.snapshotBytes, 0)
      if index == 0 {
        firstResident = resident
      }
    }

    XCTAssertEqual(owner.residentCountForTesting, PromptPrefixCache.maximumResidentCount)
    XCTAssertEqual(try Data(contentsOf: snapshotURL), snapshotSentinel)
    XCTAssertEqual(try Data(contentsOf: manifestURL), manifestSentinel)
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: cacheRoot.appendingPathComponent("stage").path))
    XCTAssertFalse(
      FileManager.default.fileExists(atPath: cacheRoot.appendingPathComponent("previous").path))
    XCTAssertTrue(
      PromptPrefixCache.permitsResidentState(bytes: 512 * 1_024 * 1_024 - 1_048_576)
    )
    XCTAssertFalse(
      PromptPrefixCache.permitsResidentState(bytes: 512 * 1_024 * 1_024 - 1_048_575)
    )

    let resident = try XCTUnwrap(firstResident)
    let originalKV = try XCTUnwrap(resident.cache.compactMap { $0 as? KVCacheSimple }.first)
    let originalMamba = try XCTUnwrap(resident.cache.compactMap { $0 as? MambaCache }.first)
    let originalKVValue = originalKV.state[0][0, 0, 0, 0].item(Float.self)
    let originalMambaValue = originalMamba.state[0][0, 0].item(Float.self)
    let copied = resident.cache.map { $0.copy() }
    let copiedKV = try XCTUnwrap(copied.compactMap { $0 as? KVCacheSimple }.first)
    let copiedMamba = try XCTUnwrap(copied.compactMap { $0 as? MambaCache }.first)
    copiedKV.state = [
      MLXArray.ones(copiedKV.state[0].shape) * MLXArray(Float(9)),
      MLXArray.ones(copiedKV.state[1].shape) * MLXArray(Float(9)),
    ]
    copiedMamba.state = [
      MLXArray.ones(copiedMamba.state[0].shape) * MLXArray(Float(9)),
      MLXArray.ones(copiedMamba.state[1].shape) * MLXArray(Float(9)),
    ]
    XCTAssertEqual(originalKV.state[0][0, 0, 0, 0].item(Float.self), originalKVValue)
    XCTAssertEqual(originalMamba.state[0][0, 0].item(Float.self), originalMambaValue)
  }

  func testHybridCacheCopyAndDurableRestoreAreIsolatedAcrossCacheOwners() throws {
    let modelsRoot = temporaryDirectory()
    defer { try? FileManager.default.removeItem(at: modelsRoot) }
    let candidate = PromptCacheCandidate(
      tokenCount: 512,
      key: "cache-roundtrip-key",
      tokenDigest: "cache-roundtrip-digest"
    )
    let model = RecordingHybridLanguageModel()
    let parameters = GenerateParameters(maxTokens: 4, temperature: 0, prefillStepSize: 512)
    let firstOwner = PromptPrefixCache(modelsRoot: modelsRoot)
    let resident = try firstOwner.build(
      model: model,
      actualTokens: Array(0..<530),
      candidate: candidate,
      parameters: parameters
    )

    let copied = resident.cache.map { $0.copy() }
    let originalKV = try XCTUnwrap(resident.cache.compactMap { $0 as? KVCacheSimple }.first)
    let copiedKV = try XCTUnwrap(copied.compactMap { $0 as? KVCacheSimple }.first)
    let originalMamba = try XCTUnwrap(resident.cache.compactMap { $0 as? MambaCache }.first)
    let copiedMamba = try XCTUnwrap(copied.compactMap { $0 as? MambaCache }.first)
    let originalValue = originalKV.state[0][0, 0, 0, 0].item(Float.self)
    let originalMambaValue = originalMamba.state[0][0, 0].item(Float.self)
    copiedKV.state = [
      MLXArray.ones(copiedKV.state[0].shape) * MLXArray(Float(9)),
      MLXArray.ones(copiedKV.state[1].shape) * MLXArray(Float(9)),
    ]
    copiedMamba.state = [
      MLXArray.ones(copiedMamba.state[0].shape) * MLXArray(Float(9)),
      MLXArray.ones(copiedMamba.state[1].shape) * MLXArray(Float(9)),
    ]
    XCTAssertEqual(originalKV.state[0][0, 0, 0, 0].item(Float.self), originalValue)
    XCTAssertEqual(originalMamba.state[0][0, 0].item(Float.self), originalMambaValue)

    let savedBytes = try firstOwner.persist(resident)
    XCTAssertGreaterThan(savedBytes, 0)
    XCTAssertLessThanOrEqual(savedBytes, 512 * 1_024 * 1_024)
    let secondOwner = PromptPrefixCache(modelsRoot: modelsRoot)
    let restored = try XCTUnwrap(
      try secondOwner.restore(candidate, requestStarted: ProcessInfo.processInfo.systemUptime))
    XCTAssertEqual(restored.key, resident.key)
    XCTAssertEqual(restored.tokenCount, 512)
    XCTAssertEqual(restored.stateSignature, resident.stateSignature)
    XCTAssertEqual(restored.stateBytes, resident.stateBytes)
    XCTAssertEqual(
      secondOwner.cacheSignature(restored.cache, expectedPrefixTokens: 512),
      resident.stateSignature
    )

    let cacheParent =
      modelsRoot
      .appendingPathComponent("cleanup", isDirectory: true)
      .appendingPathComponent(".voiceflow-mlx-cache", isDirectory: true)
    let promptCacheRoot = cacheParent.appendingPathComponent("prompt-prefix", isDirectory: true)
    let active = promptCacheRoot.appendingPathComponent("active", isDirectory: true)
    let snapshot = active.appendingPathComponent("prefix.safetensors")
    let manifest = active.appendingPathComponent("manifest.json")
    XCTAssertEqual(
      try XCTUnwrap(
        FileManager.default.attributesOfItem(atPath: cacheParent.path)[.posixPermissions]
          as? NSNumber
      ).intValue,
      0o700
    )
    XCTAssertEqual(
      try XCTUnwrap(
        FileManager.default.attributesOfItem(atPath: promptCacheRoot.path)[.posixPermissions]
          as? NSNumber
      ).intValue,
      0o700
    )
    XCTAssertEqual(
      try XCTUnwrap(
        FileManager.default.attributesOfItem(atPath: active.path)[.posixPermissions] as? NSNumber
      ).intValue,
      0o700
    )
    XCTAssertEqual(
      try XCTUnwrap(
        FileManager.default.attributesOfItem(atPath: snapshot.path)[.posixPermissions] as? NSNumber
      ).intValue,
      0o600
    )
    XCTAssertEqual(
      try XCTUnwrap(
        FileManager.default.attributesOfItem(atPath: manifest.path)[.posixPermissions] as? NSNumber
      ).intValue,
      0o600
    )
  }

  func testInterruptedSnapshotPublishRestoresPreviousAndRemovesStage() throws {
    let modelsRoot = temporaryDirectory()
    defer { try? FileManager.default.removeItem(at: modelsRoot) }
    let candidate = PromptCacheCandidate(
      tokenCount: 512,
      key: "interrupted-publish-key",
      tokenDigest: "interrupted-publish-digest"
    )
    let firstOwner = PromptPrefixCache(modelsRoot: modelsRoot)
    let resident = try firstOwner.build(
      model: RecordingHybridLanguageModel(),
      actualTokens: Array(0..<530),
      candidate: candidate,
      parameters: GenerateParameters(maxTokens: 4, temperature: 0, prefillStepSize: 512)
    )
    _ = try firstOwner.persist(resident)

    let promptCacheRoot =
      modelsRoot
      .appendingPathComponent("cleanup", isDirectory: true)
      .appendingPathComponent(".voiceflow-mlx-cache", isDirectory: true)
      .appendingPathComponent("prompt-prefix", isDirectory: true)
    let active = promptCacheRoot.appendingPathComponent("active", isDirectory: true)
    let previous = promptCacheRoot.appendingPathComponent("previous", isDirectory: true)
    let stage = promptCacheRoot.appendingPathComponent("stage", isDirectory: true)
    try FileManager.default.moveItem(at: active, to: previous)
    try FileManager.default.createDirectory(
      at: stage,
      withIntermediateDirectories: false,
      attributes: [.posixPermissions: 0o700]
    )

    let nextOwner = PromptPrefixCache(modelsRoot: modelsRoot)
    XCTAssertNil(nextOwner.recoverInterruptedSwap())
    XCTAssertTrue(FileManager.default.fileExists(atPath: active.path))
    XCTAssertFalse(FileManager.default.fileExists(atPath: previous.path))
    XCTAssertFalse(FileManager.default.fileExists(atPath: stage.path))
    XCTAssertNotNil(
      try nextOwner.restore(candidate, requestStarted: ProcessInfo.processInfo.systemUptime)
    )
  }

  func testSnapshotDigestMutationIsRejectedAsCacheMiss() throws {
    let modelsRoot = temporaryDirectory()
    defer { try? FileManager.default.removeItem(at: modelsRoot) }
    let candidate = PromptCacheCandidate(
      tokenCount: 512,
      key: "corrupt-snapshot-key",
      tokenDigest: "corrupt-snapshot-digest"
    )
    let owner = PromptPrefixCache(modelsRoot: modelsRoot)
    let resident = try owner.build(
      model: RecordingHybridLanguageModel(),
      actualTokens: Array(0..<530),
      candidate: candidate,
      parameters: GenerateParameters(maxTokens: 4, temperature: 0, prefillStepSize: 512)
    )
    _ = try owner.persist(resident)
    let snapshot =
      modelsRoot
      .appendingPathComponent("cleanup", isDirectory: true)
      .appendingPathComponent(".voiceflow-mlx-cache", isDirectory: true)
      .appendingPathComponent("prompt-prefix", isDirectory: true)
      .appendingPathComponent("active", isDirectory: true)
      .appendingPathComponent("prefix.safetensors")
    let originalBytes = try Data(contentsOf: snapshot)
    let originalLastByte = try XCTUnwrap(originalBytes.last)
    let handle = try FileHandle(forUpdating: snapshot)
    defer { try? handle.close() }
    let fileLength = try handle.seekToEnd()
    XCTAssertGreaterThan(fileLength, 0)
    try handle.seek(toOffset: fileLength - 1)
    try handle.write(contentsOf: Data([originalLastByte ^ 0xFF]))
    try handle.synchronize()
    try handle.close()

    XCTAssertThrowsError(
      try PromptPrefixCache(modelsRoot: modelsRoot).restore(
        candidate,
        requestStarted: ProcessInfo.processInfo.systemUptime
      )
    )
  }

  func testMalformedHybridSnapshotHeaderIsRejectedBeforeOfficialLoader() throws {
    let root = temporaryDirectory()
    defer { try? FileManager.default.removeItem(at: root) }
    try FileManager.default.createDirectory(
      at: root,
      withIntermediateDirectories: true,
      attributes: [.posixPermissions: 0o700]
    )
    let url = root.appendingPathComponent("malformed.safetensors")
    let array = MLXArray.ones([1, 1, 512, 1])
    let cache = KVCacheSimple()
    cache.state = [array, array]
    try savePromptCache(
      url: url,
      cache: [cache],
      metadata: [
        "voiceflow_schema": String(PromptPrefixCache.schemaVersion),
        "voiceflow_key": "test-key",
        "voiceflow_prefix_tokens": "512",
        "voiceflow_prefix_token_sha256": "test-digest",
      ]
    )
    let bytes = try XCTUnwrap(
      try FileManager.default.attributesOfItem(atPath: url.path)[.size] as? NSNumber
    )
    XCTAssertFalse(
      PromptPrefixCache.validPromptCacheHeader(
        url,
        expectedKey: "test-key",
        expectedTokenCount: 512,
        expectedTokenDigest: "test-digest",
        expectedSnapshotBytes: bytes.uint64Value
      )
    )
    XCTAssertFalse(
      PromptPrefixCache.validPromptCacheHeader(
        root.appendingPathComponent("not-created.safetensors"),
        expectedKey: "test-key",
        expectedTokenCount: 512,
        expectedTokenDigest: "test-digest",
        expectedSnapshotBytes: 512 * 1_024 * 1_024 + 1
      )
    )
  }

  func testCleanupResponseCanEncodeBoundedContentFreeRuntimeFacts() throws {
    let request = SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: "12345678-1234-1234-1234-123456789abc",
      sessionId: nil,
      modelVersion: nil,
      op: "cleanup"
    )
    let sample = { (phase: CleanupRuntimePhase, elapsed: Double) in
      CleanupRuntimeSample(
        phase: phase,
        elapsedSeconds: elapsed,
        mlxActiveMemoryBytes: 1_024,
        mlxCacheMemoryBytes: 2_048,
        mlxPeakMemoryBytes: 4_096,
        mlxMemoryLimitBytes: 8_192,
        device: .gpu,
        taskPriority: .userInitiated,
        threadQos: .userInitiated,
        thermalState: .nominal,
        processUserCPUSeconds: 1.25,
        processSystemCPUSeconds: 0.25,
        processMinorPageFaults: 12,
        processMajorPageFaults: 1,
        processPageIns: 3,
        processPhysicalFootprintBytes: 16_384
      )
    }
    var probeTimings = PromptCacheCandidateProbeTimings()
    probeTimings.record(.stableUserPrefix, seconds: 0.01)
    probeTimings.record(.stableUserPrefix, seconds: 0.02)
    probeTimings.record(.systemOnly, seconds: 0.03)
    probeTimings.record(.systemOnly, seconds: 0.04)
    let diagnostics = CleanupRuntimeDiagnostics(
      modelPrepareSeconds: 1.5,
      cachePlanSeconds: 0.25,
      phases: [
        sample(.beforePrepare, 0),
        sample(.afterPrepare, 1.5),
        sample(.afterPlan, 1.75),
        sample(.terminal, 2.5),
      ],
      cachePlanTimings: probeTimings.diagnostic(
        performToClosureEntrySeconds: 0.005,
        mlxGenerateCallSeconds: 0.05
      )
    )
    let response = SidecarResponse(
      request: request,
      status: "completed",
      platformSupported: true,
      cleanupDiagnostics: diagnostics
    )
    let encoded = try SidecarProtocolCodec.encodeResponse(response)
    XCTAssertLessThan(encoded.count, 4_096)
    let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
    let cleanupDiagnostics = try XCTUnwrap(object["cleanup_diagnostics"] as? [String: Any])
    XCTAssertEqual(cleanupDiagnostics["model_prepare_seconds"] as? Double, 1.5)
    let cachePlanTimings = try XCTUnwrap(
      cleanupDiagnostics["cache_plan_timings"] as? [String: Any]
    )
    XCTAssertEqual(cachePlanTimings["candidate_prepare_count"] as? Int, 4)
    XCTAssertEqual(cachePlanTimings["candidate_prepare_milliseconds"] as? Double, 100.0)
    XCTAssertEqual(cachePlanTimings["stable_user_prefix_prepare_count"] as? Int, 2)
    XCTAssertEqual(cachePlanTimings["system_only_prepare_count"] as? Int, 2)
    XCTAssertEqual(cachePlanTimings["perform_to_closure_entry_milliseconds"] as? Double, 5.0)
    XCTAssertEqual(cachePlanTimings["mlx_generate_call_milliseconds"] as? Double, 50.0)
    let phases = try XCTUnwrap(cleanupDiagnostics["phases"] as? [[String: Any]])
    XCTAssertEqual(
      phases.compactMap { $0["phase"] as? String },
      [
        "before_prepare", "after_prepare", "after_plan", "terminal",
      ])
    XCTAssertEqual(phases[0]["mlx_active_memory_bytes"] as? Int, 1_024)
    XCTAssertEqual(phases[0]["process_physical_footprint_bytes"] as? Int, 16_384)
    for forbidden in ["transcript", "prompt", "secret", "api_key"] {
      XCTAssertFalse(String(decoding: encoded, as: UTF8.self).contains(forbidden))
    }

    let withoutDiagnostics = SidecarResponse(
      request: request,
      status: "ready",
      platformSupported: true
    )
    let legacyEncoded = try SidecarProtocolCodec.encodeResponse(withoutDiagnostics)
    let legacyObject = try XCTUnwrap(
      JSONSerialization.jsonObject(with: legacyEncoded) as? [String: Any]
    )
    XCTAssertNil(legacyObject["cleanup_diagnostics"])
  }

  func testInvalidCachePlanTimingIsOmittedWithoutDroppingCompletedResponse() throws {
    let request = SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: "12345678-1234-1234-1234-123456789abc",
      sessionId: nil,
      modelVersion: nil,
      op: "cleanup"
    )
    let invalidTimings = CleanupCachePlanTimings(
      candidatePrepareCount: 5,
      candidatePrepareMilliseconds: .infinity,
      stableUserPrefixPrepareCount: Int.max,
      stableUserPrefixPrepareMilliseconds: 10,
      systemOnlyPrepareCount: 2,
      systemOnlyPrepareMilliseconds: 10,
      performToClosureEntryMilliseconds: .nan,
      mlxGenerateCallMilliseconds: .infinity
    )
    let diagnostics = CleanupRuntimeDiagnostics(
      modelPrepareSeconds: 1,
      cachePlanSeconds: 2,
      phases: [],
      cachePlanTimings: invalidTimings
    )
    let response = SidecarResponse(
      request: request,
      status: "completed",
      platformSupported: true,
      cleanupResult: CleanupResult(
        text: "safe result",
        promptTokens: 12,
        generationTokens: 3,
        promptTimeSeconds: 0.1,
        generationTimeSeconds: 0.2,
        completion: "stop"
      ),
      cleanupDiagnostics: diagnostics
    )

    let encoded = try SidecarProtocolCodec.encodeResponse(response)
    XCTAssertLessThan(encoded.count, 4_096)
    let object = try XCTUnwrap(JSONSerialization.jsonObject(with: encoded) as? [String: Any])
    XCTAssertEqual(object["status"] as? String, "completed")
    let result = try XCTUnwrap(object["cleanup_result"] as? [String: Any])
    XCTAssertEqual(result["text"] as? String, "safe result")
    let cleanupDiagnostics = try XCTUnwrap(
      object["cleanup_diagnostics"] as? [String: Any]
    )
    XCTAssertEqual(cleanupDiagnostics["cache_plan_seconds"] as? Double, 2)
    XCTAssertNil(cleanupDiagnostics["cache_plan_timings"])
  }

  private func temporaryDirectory() -> URL {
    FileManager.default.temporaryDirectory
      .appendingPathComponent("voiceflow-prefix-cache-test-\(UUID().uuidString)", isDirectory: true)
  }
}

private final class RecordingHybridLanguageModel: Module, LanguageModel {
  private(set) var preparedDimensions: [Int] = []
  private(set) var seedTokenCounts: [Int] = []
  private(set) var consumedChunks: [Int] = []
  private(set) var unconsumedTokens: [[Int]] = []

  func newCache(parameters: GenerateParameters?) -> [KVCache] {
    var caches: [KVCache] = []
    for index in 0..<64 {
      if index.isMultiple(of: 4) {
        caches.append(MambaCache())
      } else {
        caches.append(KVCacheSimple())
      }
    }
    return caches
  }

  func prepare(_ input: LMInput, cache: [KVCache], windowSize: Int?) throws -> PrepareResult {
    let tokens = input.text.tokens.asArray(Int.self)
    preparedDimensions.append(input.text.tokens.ndim)
    seedTokenCounts.append(tokens.count)
    let step = windowSize ?? 512
    var consumed = 0
    while tokens.count - consumed > step {
      consumedChunks.append(step)
      let nextOffset = consumed + step
      for layer in cache {
        if let kv = layer as? KVCacheSimple {
          kv.state = [
            MLXArray.ones([1, 1, nextOffset, 1]),
            MLXArray.ones([1, 1, nextOffset, 1]) * MLXArray(Float(2)),
          ]
        } else if let mamba = layer as? MambaCache {
          mamba.state = [
            MLXArray.ones([1, 2]),
            MLXArray.ones([1, 2]) * MLXArray(Float(3)),
          ]
        }
      }
      consumed = nextOffset
    }
    let remaining = Array(tokens.dropFirst(consumed))
    unconsumedTokens.append(remaining)
    return .tokens(LMInput.Text(tokens: MLXArray(remaining)))
  }

  func callAsFunction(_ input: LMInput.Text, cache: [KVCache]?, state: LMOutput.State?) -> LMOutput
  {
    LMOutput(logits: callAsFunction(input.tokens, cache: cache))
  }

  func callAsFunction(_ inputs: MLXArray, cache: [KVCache]?) -> MLXArray {
    MLXArray.zeros([1, inputs.size, 1])
  }
}
