import CryptoKit
import Darwin
import Dispatch
import Foundation
import MLX
import MLXHuggingFace
import MLXLLM
import MLXLMCommon
import Tokenizers

enum PromptPrefixBoundary {
  static let step = 512
  static let minimumMemoryOnlyPrefix = 128

  static func utf8Prefix(_ text: String, byteCount: Int) -> String? {
    guard byteCount >= 0, byteCount <= text.utf8.count else { return nil }
    let bytes = Array(text.utf8.prefix(byteCount))
    guard let prefix = String(bytes: bytes, encoding: .utf8), prefix.utf8.count == byteCount else {
      return nil
    }
    return prefix
  }

  static func alignedCommonPrefixLength(actual: [Int], probes: [[Int]]) -> Int? {
    alignedCommonPrefixLength(actual: actual, probes: probes, maximumTokenCount: nil)
  }

  static func commonPrefixLength(_ sequences: [[Int]]) -> Int? {
    guard sequences.count >= 2, let first = sequences.first, !first.isEmpty else { return nil }
    var common = first.count
    for sequence in sequences.dropFirst() {
      common = min(common, sequence.count)
      var index = 0
      while index < common, first[index] == sequence[index] {
        index += 1
      }
      common = index
    }
    return common
  }

  static func shortSystemOnlyPrefixLength(actual: [Int], probes: [[Int]]) -> Int? {
    guard probes.count == 2,
      let common = commonPrefixLength([actual] + probes),
      common >= minimumMemoryOnlyPrefix,
      common < step,
      common < actual.count
    else {
      return nil
    }
    return common
  }

  static func alignedCommonPrefixLength(
    actual: [Int],
    probes: [[Int]],
    maximumTokenCount: Int?
  ) -> Int? {
    guard !probes.isEmpty, !actual.isEmpty else { return nil }
    var common = actual.count
    for probe in probes {
      common = min(common, probe.count)
      var index = 0
      while index < common, actual[index] == probe[index] {
        index += 1
      }
      common = index
    }
    if let maximumTokenCount {
      guard maximumTokenCount >= 0 else { return nil }
      common = min(common, maximumTokenCount)
    }
    let aligned = (common / step) * step
    guard aligned >= step, aligned < actual.count else { return nil }
    return aligned
  }
}

enum PromptCacheTokenInput {
  /// MLXLMCommon.generate and LanguageModel.prepare accept the package's
  /// one-dimensional token stream. Adding a batch axis here changes the
  /// official 512-token prefill chunking contract.
  static func make(_ tokens: [Int]) -> LMInput {
    LMInput(tokens: MLXArray(tokens))
  }

  static func suffix(_ tokens: [Int], after prefixTokenCount: Int) -> [Int]? {
    guard prefixTokenCount >= 0, prefixTokenCount < tokens.count else { return nil }
    return Array(tokens.dropFirst(prefixTokenCount))
  }
}

struct PromptCacheCandidate: Sendable {
  let tokenCount: Int
  let key: String
  let tokenDigest: String

  static func make(prefixTokens: [Int], keyIdentity: String) -> Self? {
    guard !prefixTokens.isEmpty else { return nil }
    var keyData = Data(keyIdentity.utf8)
    for token in prefixTokens {
      var encoded = UInt64(bitPattern: Int64(token)).littleEndian
      withUnsafeBytes(of: &encoded) { keyData.append(contentsOf: $0) }
    }
    return Self(
      tokenCount: prefixTokens.count,
      key: PromptPrefixCache.sha256(keyData),
      tokenDigest: PromptPrefixCache.digestTokens(prefixTokens)
    )
  }
}

enum PromptCacheSDKIdentity {
  static let current = versions(
    mlxSwift: "0.31.4",
    mlxSwiftLM: "3.31.4",
    swiftTransformers: "1.3.4"
  )

  static func versions(mlxSwift: String, mlxSwiftLM: String, swiftTransformers: String) -> String {
    "mlx-swift=\(mlxSwift)|mlx-swift-lm=\(mlxSwiftLM)|swift-transformers=\(swiftTransformers)"
  }
}

struct PromptCacheMetrics: Sendable {
  var status: String
  var reason: String?
  var cachedPrefixTokens: Int
  var computedSuffixTokens: Int
  var snapshotBytes: UInt64
  var lookupSeconds: Double?
  var buildSeconds: Double?
  var copySeconds: Double?
  var restoreSeconds: Double?

  static func bypassed(_ reason: String, promptTokens: Int) -> Self {
    Self(
      status: "bypassed",
      reason: reason,
      cachedPrefixTokens: 0,
      computedSuffixTokens: promptTokens,
      snapshotBytes: 0,
      lookupSeconds: nil,
      buildSeconds: nil,
      copySeconds: nil,
      restoreSeconds: nil
    )
  }
}

struct PromptCacheGeneration: Sendable {
  let stream: AsyncStream<Generation>
  let worker: Task<Void, Never>
  let metrics: PromptCacheMetrics
  let mlxGenerateCallSeconds: Double?

  init(
    stream: AsyncStream<Generation>,
    worker: Task<Void, Never>,
    metrics: PromptCacheMetrics,
    mlxGenerateCallSeconds: Double? = nil
  ) {
    self.stream = stream
    self.worker = worker
    self.metrics = metrics
    self.mlxGenerateCallSeconds = mlxGenerateCallSeconds
  }
}

enum PromptCacheGenerationFactory {
  static func make(
    context: ModelContext,
    input: LMInput,
    cache: [KVCache]?,
    parameters: GenerateParameters,
    metrics: PromptCacheMetrics,
    lifetime: CleanupGenerationLifetime
  ) throws -> PromptCacheGeneration {
    guard !lifetime.isCancelled else { throw CancellationError() }
    let generateStarted = ProcessInfo.processInfo.systemUptime
    let iterator = try TokenIterator(
      input: input,
      model: context.model,
      cache: cache,
      parameters: parameters
    )
    let (stream, worker) = MLXLMCommon.generateTask(
      promptTokenCount: input.text.tokens.size,
      modelConfiguration: context.configuration,
      tokenizer: context.tokenizer,
      iterator: iterator
    )
    lifetime.install(worker)
    return PromptCacheGeneration(
      stream: stream,
      worker: worker,
      metrics: metrics,
      mlxGenerateCallSeconds: max(0, ProcessInfo.processInfo.systemUptime - generateStarted)
    )
  }
}

private enum CleanupInputEvent: Sendable {
  case line(Data)
  case eof
  case lineTooLarge
  case readFailed
}

private final class CleanupInputOverflow: @unchecked Sendable {
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

/// Owns the underlying MLX generation task independently of its stream consumer.
/// Cancellation may arrive while iterator construction is still in progress, so
/// installation also observes and applies an earlier cancellation request.
final class CleanupGenerationLifetime: @unchecked Sendable {
  private let lock = NSLock()
  private var cancelled = false
  private var worker: Task<Void, Never>?

  var isCancelled: Bool {
    lock.lock()
    defer { lock.unlock() }
    return cancelled
  }

  func cancel() {
    lock.lock()
    cancelled = true
    let worker = self.worker
    lock.unlock()
    worker?.cancel()
  }

  func install(_ worker: Task<Void, Never>) {
    lock.lock()
    self.worker = worker
    let cancelImmediately = cancelled
    lock.unlock()
    if cancelImmediately { worker.cancel() }
  }

  func settle() async {
    await currentWorker()?.value
  }

  private func currentWorker() -> Task<Void, Never>? {
    lock.lock()
    defer { lock.unlock() }
    return worker
  }
}

enum CleanupCancelAcceptance: Equatable {
  case accepted
  case duplicate
  case rejected
  case limitReached
}

/// Shared admission, identity binding, and terminal-settlement owner used by
/// the JSONL server and its controllable-worker tests.
final class CleanupActiveOperation: @unchecked Sendable {
  let id: String
  let request: SidecarRequest
  let lifetime: CleanupGenerationLifetime
  let task: Task<SidecarResponse, Never>
  private let lock = NSLock()
  private var cancelAcknowledgements: [SidecarRequest] = []
  private var cancelRequestIds: Set<String> = []
  private var terminalCommitted = false

  init(
    id: String,
    request: SidecarRequest,
    lifetime: CleanupGenerationLifetime,
    task: Task<SidecarResponse, Never>
  ) {
    self.id = id
    self.request = request
    self.lifetime = lifetime
    self.task = task
  }

  func acceptCancel(_ request: SidecarRequest, modelId: String, modelRevision: String)
    -> CleanupCancelAcceptance
  {
    guard Self.validIdentifier(request.requestId),
      let sessionId = request.sessionId,
      Self.validIdentifier(sessionId),
      request.op == "cancel",
      request.protocolVersion == sidecarProtocolVersion,
      let activeSession = self.request.sessionId,
      sessionId == activeSession,
      request.requestId != self.request.requestId,
      request.modelId == modelId,
      request.modelId == self.request.modelId,
      request.modelVersion == modelRevision,
      request.modelVersion == self.request.modelVersion,
      self.request.op == "cleanup" || self.request.op == "warmup"
    else {
      return .rejected
    }
    lock.lock()
    guard !terminalCommitted else {
      lock.unlock()
      return .rejected
    }
    if cancelRequestIds.contains(request.requestId) {
      lock.unlock()
      return .duplicate
    }
    guard cancelAcknowledgements.count < 8 else {
      lock.unlock()
      return .limitReached
    }
    cancelRequestIds.insert(request.requestId)
    cancelAcknowledgements.append(request)
    lifetime.cancel()
    task.cancel()
    lock.unlock()
    return .accepted
  }

  func settleWorker() async {
    await lifetime.settle()
  }

  func commitTerminal() -> (cancelled: Bool, acknowledgements: [SidecarRequest]) {
    lock.lock()
    defer { lock.unlock() }
    terminalCommitted = true
    return (lifetime.isCancelled, cancelAcknowledgements)
  }

  private static func validIdentifier(_ value: String) -> Bool {
    !value.isEmpty && value.utf8.count <= 128
      && value.utf8.allSatisfy {
        ($0 >= 48 && $0 <= 57) || ($0 >= 65 && $0 <= 90) || ($0 >= 97 && $0 <= 122)
          || $0 == 45 || $0 == 46 || $0 == 58 || $0 == 95
      }
  }
}

enum PromptCachePrepareProbeKind: Sendable {
  case stableUserPrefix
  case systemOnly
}

struct PromptCacheCandidateProbeTimings: Sendable {
  var stableUserPrefixPrepareCount = 0
  var stableUserPrefixPrepareSeconds = 0.0
  var systemOnlyPrepareCount = 0
  var systemOnlyPrepareSeconds = 0.0

  mutating func record(_ kind: PromptCachePrepareProbeKind, seconds: Double) {
    switch kind {
    case .stableUserPrefix:
      stableUserPrefixPrepareCount += 1
      stableUserPrefixPrepareSeconds += seconds
    case .systemOnly:
      systemOnlyPrepareCount += 1
      systemOnlyPrepareSeconds += seconds
    }
  }

  func diagnostic(
    performToClosureEntrySeconds: Double? = nil,
    mlxGenerateCallSeconds: Double? = nil
  ) -> CleanupCachePlanTimings {
    let stableUserPrefixPrepareMilliseconds = stableUserPrefixPrepareSeconds * 1_000
    let systemOnlyPrepareMilliseconds = systemOnlyPrepareSeconds * 1_000
    return CleanupCachePlanTimings(
      candidatePrepareCount: stableUserPrefixPrepareCount + systemOnlyPrepareCount,
      candidatePrepareMilliseconds:
        stableUserPrefixPrepareMilliseconds + systemOnlyPrepareMilliseconds,
      stableUserPrefixPrepareCount: stableUserPrefixPrepareCount,
      stableUserPrefixPrepareMilliseconds: stableUserPrefixPrepareMilliseconds,
      systemOnlyPrepareCount: systemOnlyPrepareCount,
      systemOnlyPrepareMilliseconds: systemOnlyPrepareMilliseconds,
      performToClosureEntryMilliseconds: performToClosureEntrySeconds.map { $0 * 1_000 },
      mlxGenerateCallMilliseconds: mlxGenerateCallSeconds.map { $0 * 1_000 }
    )
  }
}

struct PromptCacheCandidateResult: Sendable {
  let candidate: PromptCacheCandidate?
  let reason: String?
  let probeTimings: PromptCacheCandidateProbeTimings
}

private struct PromptCacheSnapshotManifest: Codable {
  let schemaVersion: Int
  let key: String
  let prefixTokenCount: Int
  let prefixTokenSHA256: String
  let stateSignature: String
  let stateBytes: UInt64
  let snapshotBytes: UInt64
  let snapshotSHA256: String
}

/// Holds at most two immutable system-prefix caches. Each operation gets an
/// official MLX cache copy; user/transcript tokens are never part of saved state.
final class PromptPrefixCache: @unchecked Sendable {
  static let schemaVersion = 2
  static let maximumResidentCount = 2
  private static let maximumSnapshotBytes: UInt64 = 512 * 1_024 * 1_024
  private static let maximumResidentStateBytes = maximumSnapshotBytes - 1_048_576
  private static let manifestMaximumBytes = 64 * 1_024
  private static let snapshotName = "prefix.safetensors"
  private static let manifestName = "manifest.json"

  struct Resident {
    let key: String
    let tokenCount: Int
    let tokenDigest: String
    let stateSignature: String
    let stateBytes: UInt64
    let snapshotBytes: UInt64
    let lookupSeconds: Double?
    let restoreSeconds: Double?
    let cache: [KVCache]
  }

  private let root: URL
  private let lock = NSLock()
  private var residents: [Resident] = []

  var residentCountForTesting: Int {
    lock.lock()
    defer { lock.unlock() }
    return residents.count
  }

  static func permitsResidentState(bytes: UInt64) -> Bool {
    bytes <= maximumResidentStateBytes
  }

  init(modelsRoot: URL) {
    root =
      modelsRoot
      .appendingPathComponent("cleanup", isDirectory: true)
      .appendingPathComponent(".voiceflow-mlx-cache", isDirectory: true)
      .appendingPathComponent("prompt-prefix", isDirectory: true)
  }

  func clearResident() {
    lock.lock()
    residents.removeAll()
    lock.unlock()
  }

  func generate(
    context: ModelContext,
    prepared: LMInput,
    actualTokens: [Int],
    candidate: PromptCacheCandidate,
    parameters: GenerateParameters,
    lifetime: CleanupGenerationLifetime
  ) throws -> PromptCacheGeneration {
    lock.lock()
    defer { lock.unlock() }
    let (base, resolvedMetrics) = resolveBase(
      model: context.model,
      actualTokens: actualTokens,
      candidate: candidate,
      parameters: parameters
    )
    var metrics = resolvedMetrics
    guard let base else {
      let generateStarted = ProcessInfo.processInfo.systemUptime
      let generation = try PromptCacheGenerationFactory.make(
        context: context,
        input: prepared,
        cache: nil,
        parameters: parameters,
        metrics: metrics,
        lifetime: lifetime
      )
      return PromptCacheGeneration(
        stream: generation.stream,
        worker: generation.worker,
        metrics: generation.metrics,
        mlxGenerateCallSeconds: max(0, ProcessInfo.processInfo.systemUptime - generateStarted)
      )
    }

    let copyStarted = ProcessInfo.processInfo.systemUptime
    let requestCache = base.cache.map { $0.copy() }
    metrics.copySeconds = max(0, ProcessInfo.processInfo.systemUptime - copyStarted)
    guard let suffix = PromptCacheTokenInput.suffix(actualTokens, after: candidate.tokenCount)
    else {
      let generateStarted = ProcessInfo.processInfo.systemUptime
      let generation = try PromptCacheGenerationFactory.make(
        context: context,
        input: prepared,
        cache: nil,
        parameters: parameters,
        metrics: .bypassed("unsupported_cache_state", promptTokens: actualTokens.count),
        lifetime: lifetime
      )
      return PromptCacheGeneration(
        stream: generation.stream,
        worker: generation.worker,
        metrics: generation.metrics,
        mlxGenerateCallSeconds: max(0, ProcessInfo.processInfo.systemUptime - generateStarted)
      )
    }
    let generateStarted = ProcessInfo.processInfo.systemUptime
    let generation = try PromptCacheGenerationFactory.make(
      context: context,
      input: PromptCacheTokenInput.make(suffix),
      cache: requestCache,
      parameters: parameters,
      metrics: metrics,
      lifetime: lifetime
    )
    return PromptCacheGeneration(
      stream: generation.stream,
      worker: generation.worker,
      metrics: generation.metrics,
      mlxGenerateCallSeconds: max(0, ProcessInfo.processInfo.systemUptime - generateStarted)
    )
  }

  /// Performs a bounded internal evaluation against the same production
  /// prefix identity and 2048-token cache configuration. Only the copied
  /// request cache sees the synthetic suffix; it is never persisted.
  func warm(
    context: ModelContext,
    prepared: LMInput,
    preparedHasMask: Bool,
    actualTokens: [Int],
    candidate: PromptCacheCandidate?,
    candidateReason: String?,
    productionParameters: GenerateParameters,
    evaluationParameters: GenerateParameters,
    lifetime: CleanupGenerationLifetime
  ) throws -> PromptCacheGeneration {
    lock.lock()
    defer { lock.unlock() }

    guard let candidate, !preparedHasMask else {
      return try PromptCacheGenerationFactory.make(
        context: context,
        input: prepared,
        cache: nil,
        parameters: evaluationParameters,
        metrics: .bypassed(
          candidateReason
            ?? (preparedHasMask ? "unsupported_cache_state" : "prefix_below_prefill_step"),
          promptTokens: actualTokens.count
        ),
        lifetime: lifetime
      )
    }
    let (base, metrics) = resolveBase(
      model: context.model,
      actualTokens: actualTokens,
      candidate: candidate,
      parameters: productionParameters
    )
    guard let base,
      let suffix = PromptCacheTokenInput.suffix(actualTokens, after: candidate.tokenCount)
    else {
      return try PromptCacheGenerationFactory.make(
        context: context,
        input: prepared,
        cache: nil,
        parameters: evaluationParameters,
        metrics: metrics,
        lifetime: lifetime
      )
    }
    let copyStarted = ProcessInfo.processInfo.systemUptime
    let requestCache = base.cache.map { $0.copy() }
    var warmMetrics = metrics
    warmMetrics.copySeconds = max(0, ProcessInfo.processInfo.systemUptime - copyStarted)
    return try PromptCacheGenerationFactory.make(
      context: context,
      input: PromptCacheTokenInput.make(suffix),
      cache: requestCache,
      parameters: evaluationParameters,
      metrics: warmMetrics,
      lifetime: lifetime
    )
  }

  /// Caller holds `lock`; durable candidates retain the existing
  /// lookup/restore/build/persist path, while short candidates use residents.
  private func resolveBase(
    model: any LanguageModel,
    actualTokens: [Int],
    candidate: PromptCacheCandidate,
    parameters: GenerateParameters
  ) -> (Resident?, PromptCacheMetrics) {
    let lookupStarted = ProcessInfo.processInfo.systemUptime
    var metrics = PromptCacheMetrics.bypassed("snapshot_missing", promptTokens: actualTokens.count)
    let memoryOnly = candidate.tokenCount < PromptPrefixBoundary.step
    var base = residents.first(where: { $0.key == candidate.key })

    if let resident = base {
      metrics.status = "memory_hit"
      metrics.reason = nil
      metrics.snapshotBytes = resident.snapshotBytes
      metrics.lookupSeconds = max(0, ProcessInfo.processInfo.systemUptime - lookupStarted)
      metrics.cachedPrefixTokens = candidate.tokenCount
      metrics.computedSuffixTokens = actualTokens.count - candidate.tokenCount
    } else if memoryOnly {
      // Short prefixes are transient residents only. Do not inspect, recover,
      // or replace the durable 512-token snapshot for this request.
      metrics.reason = nil
      metrics.snapshotBytes = 0
    } else if let recoveryReason = recoverInterruptedSwap() {
      metrics.reason = recoveryReason
    } else {
      do {
        base = try restore(candidate, requestStarted: lookupStarted)
        if let restored = base {
          installResident(restored)
          metrics.status = "snapshot_hit"
          metrics.reason = nil
          metrics.snapshotBytes = restored.snapshotBytes
          metrics.lookupSeconds = restored.lookupSeconds
          metrics.restoreSeconds = restored.restoreSeconds
          metrics.cachedPrefixTokens = candidate.tokenCount
          metrics.computedSuffixTokens = actualTokens.count - candidate.tokenCount
        } else {
          metrics.reason = "snapshot_missing"
        }
      } catch SnapshotError.miss(let reason) {
        metrics.reason = reason
        metrics.snapshotBytes = 0
      } catch {
        metrics.reason = "snapshot_corrupt"
        metrics.snapshotBytes = 0
      }
    }
    if metrics.lookupSeconds == nil {
      metrics.lookupSeconds = max(0, ProcessInfo.processInfo.systemUptime - lookupStarted)
    }

    if base == nil {
      let buildStarted = ProcessInfo.processInfo.systemUptime
      do {
        let built = try build(
          model: model,
          actualTokens: actualTokens,
          candidate: candidate,
          parameters: parameters
        )
        if memoryOnly {
          guard Self.permitsResidentState(bytes: built.stateBytes) else {
            throw SnapshotError.tooLarge(built.stateBytes)
          }
          metrics.status = "built"
          metrics.reason = nil
          metrics.cachedPrefixTokens = 0
          metrics.computedSuffixTokens = actualTokens.count - candidate.tokenCount
          metrics.snapshotBytes = 0
          installResident(built)
          base = built
        } else {
          metrics.status = "built"
          metrics.cachedPrefixTokens = 0
          metrics.computedSuffixTokens = actualTokens.count - candidate.tokenCount
          metrics.snapshotBytes = built.stateBytes
          var stored = built
          do {
            let persistedBytes = try persist(built)
            stored = Resident(
              key: built.key,
              tokenCount: built.tokenCount,
              tokenDigest: built.tokenDigest,
              stateSignature: built.stateSignature,
              stateBytes: built.stateBytes,
              snapshotBytes: persistedBytes,
              lookupSeconds: nil,
              restoreSeconds: nil,
              cache: built.cache
            )
            metrics.snapshotBytes = persistedBytes
          } catch SnapshotError.tooLarge(let bytes) {
            metrics.reason = "snapshot_too_large"
            metrics.snapshotBytes = bytes
          } catch {
            metrics.reason = "snapshot_write_failed"
          }
          installResident(stored)
          base = stored
        }
      } catch SnapshotError.tooLarge(let bytes) where memoryOnly {
        metrics.status = "bypassed"
        metrics.reason = "snapshot_too_large"
        metrics.cachedPrefixTokens = 0
        metrics.computedSuffixTokens = actualTokens.count
        metrics.snapshotBytes = 0
      } catch {
        metrics.status = "bypassed"
        metrics.reason = "unsupported_cache_state"
        if memoryOnly {
          metrics.cachedPrefixTokens = 0
          metrics.computedSuffixTokens = actualTokens.count
          metrics.snapshotBytes = 0
        }
      }
      metrics.buildSeconds = max(0, ProcessInfo.processInfo.systemUptime - buildStarted)
    }
    return (base, metrics)
  }

  func resolveBaseForTesting(
    model: any LanguageModel,
    actualTokens: [Int],
    candidate: PromptCacheCandidate,
    parameters: GenerateParameters
  ) -> (Resident?, PromptCacheMetrics) {
    lock.lock()
    defer { lock.unlock() }
    return resolveBase(
      model: model,
      actualTokens: actualTokens,
      candidate: candidate,
      parameters: parameters
    )
  }

  private func installResident(_ resident: Resident) {
    residents.removeAll { $0.key == resident.key }
    residents.append(resident)
    if residents.count > Self.maximumResidentCount {
      residents.removeFirst(residents.count - Self.maximumResidentCount)
    }
  }

  private enum SnapshotError: Error {
    case tooLarge(UInt64)
    case miss(String)
  }

  func build(
    model: any LanguageModel,
    actualTokens: [Int],
    candidate: PromptCacheCandidate,
    parameters: GenerateParameters
  ) throws -> Resident {
    let isShortMemoryOnly =
      (PromptPrefixBoundary.minimumMemoryOnlyPrefix..<PromptPrefixBoundary.step)
      .contains(candidate.tokenCount)
    let isAlignedDurable =
      candidate.tokenCount >= PromptPrefixBoundary.step
      && candidate.tokenCount % PromptPrefixBoundary.step == 0
    guard candidate.tokenCount > 0,
      isShortMemoryOnly || isAlignedDurable,
      candidate.tokenCount < actualTokens.count
    else {
      throw SnapshotError.miss("unsupported_cache_state")
    }
    var cache = model.newCache(parameters: parameters)
    // The short in-memory path uses its exact prefix as the prepare window;
    // durable prefixes retain the existing 512-token chunking unchanged.
    // One actual next token remains unconsumed so the state lands exactly on
    // the declared prefix boundary in both cases.
    let seedTokens = Array(actualTokens.prefix(candidate.tokenCount + 1))
    _ = try model.prepare(
      PromptCacheTokenInput.make(seedTokens),
      cache: cache,
      windowSize: isShortMemoryOnly ? candidate.tokenCount : PromptPrefixBoundary.step
    )
    eval(cache.flatMap(\.state))
    guard let signature = cacheSignature(cache, expectedPrefixTokens: candidate.tokenCount),
      let stateBytes = stateByteCount(cache)
    else {
      throw SnapshotError.miss("unsupported_cache_state")
    }
    return Resident(
      key: candidate.key,
      tokenCount: candidate.tokenCount,
      tokenDigest: candidate.tokenDigest,
      stateSignature: signature,
      stateBytes: stateBytes,
      snapshotBytes: 0,
      lookupSeconds: nil,
      restoreSeconds: nil,
      cache: cache
    )
  }

  func restore(
    _ candidate: PromptCacheCandidate,
    requestStarted: TimeInterval
  ) throws -> Resident? {
    let active = root.appendingPathComponent("active", isDirectory: true)
    let snapshot = active.appendingPathComponent(Self.snapshotName)
    let manifestURL = active.appendingPathComponent(Self.manifestName)
    let cacheParent = root.deletingLastPathComponent()
    guard fileExists(cacheParent) else { throw SnapshotError.miss("snapshot_missing") }
    guard validPrivateDirectory(cacheParent) else { throw SnapshotError.miss("snapshot_corrupt") }
    guard fileExists(root) else { throw SnapshotError.miss("snapshot_missing") }
    guard validPrivateDirectory(root) else { throw SnapshotError.miss("snapshot_corrupt") }
    guard fileExists(active) else { throw SnapshotError.miss("snapshot_missing") }
    guard validPrivateDirectory(active) else { throw SnapshotError.miss("snapshot_corrupt") }
    guard fileExists(manifestURL) else { throw SnapshotError.miss("snapshot_missing") }
    guard validPrivateFile(manifestURL),
      (try? fileSize(manifestURL)).map({ $0 <= Self.manifestMaximumBytes }) == true,
      let manifestData = try? Data(contentsOf: manifestURL),
      let manifest = try? JSONDecoder().decode(
        PromptCacheSnapshotManifest.self, from: manifestData),
      manifest.schemaVersion == Self.schemaVersion
    else {
      throw SnapshotError.miss("snapshot_corrupt")
    }
    guard manifest.key == candidate.key,
      manifest.prefixTokenCount == candidate.tokenCount,
      manifest.prefixTokenSHA256 == candidate.tokenDigest
    else {
      throw SnapshotError.miss("cache_key_changed")
    }
    guard validPrivateFile(snapshot) else { throw SnapshotError.miss("snapshot_corrupt") }
    let snapshotBytes = try fileSize(snapshot)
    guard snapshotBytes == manifest.snapshotBytes,
      snapshotBytes <= Self.maximumSnapshotBytes,
      try sha256File(snapshot) == manifest.snapshotSHA256
    else {
      throw SnapshotError.miss("snapshot_corrupt")
    }
    let lookupSeconds = max(0, ProcessInfo.processInfo.systemUptime - requestStarted)
    let restoreStarted = ProcessInfo.processInfo.systemUptime
    guard
      Self.validPromptCacheHeader(
        snapshot,
        expectedKey: candidate.key,
        expectedTokenCount: candidate.tokenCount,
        expectedTokenDigest: candidate.tokenDigest,
        expectedSnapshotBytes: snapshotBytes
      )
    else {
      throw SnapshotError.miss("snapshot_corrupt")
    }
    let (cache, metadata) = try loadPromptCache(url: snapshot)
    guard metadata["voiceflow_schema"] == String(Self.schemaVersion),
      metadata["voiceflow_key"] == candidate.key,
      metadata["voiceflow_prefix_tokens"] == String(candidate.tokenCount),
      metadata["voiceflow_prefix_token_sha256"] == candidate.tokenDigest,
      cacheSignature(cache, expectedPrefixTokens: candidate.tokenCount) == manifest.stateSignature,
      stateByteCount(cache) == manifest.stateBytes
    else {
      throw SnapshotError.miss("snapshot_corrupt")
    }
    return Resident(
      key: candidate.key,
      tokenCount: candidate.tokenCount,
      tokenDigest: candidate.tokenDigest,
      stateSignature: manifest.stateSignature,
      stateBytes: manifest.stateBytes,
      snapshotBytes: manifest.snapshotBytes,
      lookupSeconds: lookupSeconds,
      restoreSeconds: max(0, ProcessInfo.processInfo.systemUptime - restoreStarted),
      cache: cache
    )
  }

  func persist(_ resident: Resident) throws -> UInt64 {
    guard Self.permitsResidentState(bytes: resident.stateBytes) else {
      throw SnapshotError.tooLarge(resident.stateBytes)
    }
    try ensurePrivateDirectory(root.deletingLastPathComponent())
    try ensurePrivateDirectory(root)
    let stage = root.appendingPathComponent("stage", isDirectory: true)
    let active = root.appendingPathComponent("active", isDirectory: true)
    let previous = root.appendingPathComponent("previous", isDirectory: true)
    try removeIfPresent(stage)
    try FileManager.default.createDirectory(
      at: stage,
      withIntermediateDirectories: false,
      attributes: [.posixPermissions: 0o700]
    )
    var published = false
    defer {
      if !published {
        try? removeIfPresent(stage)
      }
    }
    guard chmod(stage.path, 0o700) == 0 else {
      throw posixError("chmod prompt cache staging directory")
    }
    let snapshot = stage.appendingPathComponent(Self.snapshotName)
    let metadata = [
      "voiceflow_schema": String(Self.schemaVersion),
      "voiceflow_key": resident.key,
      "voiceflow_prefix_tokens": String(resident.tokenCount),
      "voiceflow_prefix_token_sha256": resident.tokenDigest,
    ]
    guard
      FileManager.default.createFile(
        atPath: snapshot.path,
        contents: Data(),
        attributes: [.posixPermissions: 0o600]
      )
    else {
      throw posixError("create private prompt cache snapshot")
    }
    try savePromptCache(url: snapshot, cache: resident.cache, metadata: metadata)
    guard chmod(snapshot.path, 0o600) == 0 else {
      throw posixError("chmod prompt cache snapshot")
    }
    let snapshotBytes = try fileSize(snapshot)
    guard snapshotBytes <= Self.maximumSnapshotBytes else {
      try removeIfPresent(stage)
      throw SnapshotError.tooLarge(snapshotBytes)
    }
    let manifest = PromptCacheSnapshotManifest(
      schemaVersion: Self.schemaVersion,
      key: resident.key,
      prefixTokenCount: resident.tokenCount,
      prefixTokenSHA256: resident.tokenDigest,
      stateSignature: resident.stateSignature,
      stateBytes: resident.stateBytes,
      snapshotBytes: snapshotBytes,
      snapshotSHA256: try sha256File(snapshot)
    )
    let encoder = JSONEncoder()
    encoder.outputFormatting = [.sortedKeys]
    let manifestData = try encoder.encode(manifest)
    guard manifestData.count <= Self.manifestMaximumBytes else {
      try removeIfPresent(stage)
      throw SnapshotError.tooLarge(snapshotBytes)
    }
    let manifestURL = stage.appendingPathComponent(Self.manifestName)
    try manifestData.write(to: manifestURL, options: .atomic)
    guard chmod(manifestURL.path, 0o600) == 0 else {
      throw posixError("chmod prompt cache manifest")
    }
    try removeIfPresent(previous)
    if fileExists(active) {
      try FileManager.default.moveItem(at: active, to: previous)
    }
    do {
      try FileManager.default.moveItem(at: stage, to: active)
      published = true
    } catch {
      if fileExists(previous), !fileExists(active) {
        try? FileManager.default.moveItem(at: previous, to: active)
      }
      throw error
    }
    try removeIfPresent(previous)
    return snapshotBytes
  }

  func recoverInterruptedSwap() -> String? {
    let parent = root.deletingLastPathComponent()
    guard fileExists(parent) else { return nil }
    guard validPrivateDirectory(parent) else { return "snapshot_corrupt" }
    guard fileExists(root) else { return nil }
    guard validPrivateDirectory(root) else { return "snapshot_corrupt" }

    let active = root.appendingPathComponent("active", isDirectory: true)
    let stage = root.appendingPathComponent("stage", isDirectory: true)
    let previous = root.appendingPathComponent("previous", isDirectory: true)
    do {
      if !fileExists(active), fileExists(previous) {
        guard validPrivateDirectory(previous) else { return "snapshot_corrupt" }
        try FileManager.default.moveItem(at: previous, to: active)
      } else if fileExists(active), fileExists(previous) {
        try removeIfPresent(previous)
      }
      try removeIfPresent(stage)
      return nil
    } catch {
      return "snapshot_corrupt"
    }
  }

  func cacheSignature(_ cache: [KVCache], expectedPrefixTokens: Int) -> String? {
    guard cache.count == 64 else { return nil }
    var entries: [String] = []
    entries.reserveCapacity(cache.count)
    for layer in cache {
      let kind: String
      let expectedOffset: Int
      if layer is MambaCache {
        kind = "mamba"
        expectedOffset = 0
      } else if layer is KVCacheSimple {
        kind = "kv_simple"
        expectedOffset = expectedPrefixTokens
      } else {
        return nil
      }
      guard layer.offset == expectedOffset, layer.state.count == 2 else { return nil }
      let state = layer.state
      guard state.allSatisfy({ !$0.shape.isEmpty && $0.nbytes > 0 }) else { return nil }
      let shapeAndDtype = state.map { array in
        "\(array.dtype):\(array.shape.map(String.init).joined(separator: "x"))"
      }.joined(separator: ",")
      entries.append("\(kind):\(layer.offset):\(shapeAndDtype)")
    }
    return Self.sha256(Data(entries.joined(separator: "|").utf8))
  }

  func stateByteCount(_ cache: [KVCache]) -> UInt64? {
    var total: UInt64 = 0
    for array in cache.flatMap(\.state) {
      guard array.nbytes >= 0 else { return nil }
      let (sum, overflow) = total.addingReportingOverflow(UInt64(array.nbytes))
      guard !overflow else { return nil }
      total = sum
    }
    return total
  }

  private func ensurePrivateDirectory(_ url: URL) throws {
    if !fileExists(url) {
      try FileManager.default.createDirectory(
        at: url,
        withIntermediateDirectories: true,
        attributes: [.posixPermissions: 0o700]
      )
    }
    guard ownedRealDirectory(url), chmod(url.path, 0o700) == 0, validPrivateDirectory(url) else {
      throw posixError("create private prompt cache directory")
    }
  }

  private func ownedRealDirectory(_ url: URL) -> Bool {
    var info = stat()
    return lstat(url.path, &info) == 0
      && (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR)
      && info.st_uid == geteuid()
  }

  private func validPrivateDirectory(_ url: URL) -> Bool {
    var info = stat()
    return lstat(url.path, &info) == 0
      && (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR)
      && info.st_uid == geteuid()
      && (info.st_mode & 0o777) == 0o700
  }

  private func validPrivateFile(_ url: URL) -> Bool {
    var info = stat()
    return lstat(url.path, &info) == 0
      && (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG)
      && info.st_uid == geteuid()
      && (info.st_mode & 0o777) == 0o600
  }

  private func fileSize(_ url: URL) throws -> UInt64 {
    var info = stat()
    guard lstat(url.path, &info) == 0,
      (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
      info.st_uid == geteuid()
    else {
      throw posixError("inspect prompt cache file")
    }
    return UInt64(info.st_size)
  }

  private func sha256File(_ url: URL) throws -> String {
    let handle = try FileHandle(forReadingFrom: url)
    defer { try? handle.close() }
    var hasher = SHA256()
    while let data = try handle.read(upToCount: 65_536), !data.isEmpty {
      hasher.update(data: data)
    }
    return Self.hex(Array(hasher.finalize()))
  }

  static func sha256(_ data: Data) -> String {
    hex(Array(SHA256.hash(data: data)))
  }

  static func digestTokens(_ tokens: [Int]) -> String {
    var data = Data(capacity: tokens.count * MemoryLayout<UInt64>.size)
    for token in tokens {
      var encoded = UInt64(bitPattern: Int64(token)).littleEndian
      withUnsafeBytes(of: &encoded) { data.append(contentsOf: $0) }
    }
    return sha256(data)
  }

  /// Validate the bounded safetensors header before calling MLXLMCommon's
  /// loader. That loader reconstructs cache classes and some state setters
  /// use fatalError for malformed array counts, so hashes alone are not a
  /// sufficient guard against a malformed private snapshot.
  static func validPromptCacheHeader(
    _ url: URL,
    expectedKey: String,
    expectedTokenCount: Int,
    expectedTokenDigest: String,
    expectedSnapshotBytes: UInt64
  ) -> Bool {
    let maximumHeaderBytes = 1_048_576
    guard expectedSnapshotBytes >= 8,
      expectedSnapshotBytes <= maximumSnapshotBytes,
      let handle = try? FileHandle(forReadingFrom: url)
    else { return false }
    defer { try? handle.close() }
    guard let prefix = try? handle.read(upToCount: 8), prefix.count == 8 else {
      return false
    }
    var headerLength: UInt64 = 0
    for (index, byte) in prefix.enumerated() {
      headerLength |= UInt64(byte) << (index * 8)
    }
    guard headerLength > 0,
      headerLength <= UInt64(maximumHeaderBytes),
      headerLength <= expectedSnapshotBytes - 8,
      let headerData = try? handle.read(upToCount: Int(headerLength)),
      headerData.count == Int(headerLength),
      let root = try? JSONSerialization.jsonObject(with: headerData) as? [String: Any],
      let rawMetadata = root["__metadata__"] as? [String: String]
    else { return false }

    let expectedMetadata = [
      "1.voiceflow_schema": String(schemaVersion),
      "1.voiceflow_key": expectedKey,
      "1.voiceflow_prefix_tokens": String(expectedTokenCount),
      "1.voiceflow_prefix_token_sha256": expectedTokenDigest,
    ]
    let userMetadataKeys = Set(rawMetadata.keys.filter { $0.hasPrefix("1.") })
    guard userMetadataKeys == Set(expectedMetadata.keys),
      expectedMetadata.allSatisfy({ rawMetadata[$0.key] == $0.value })
    else { return false }

    var classes: [Int: String] = [:]
    var cacheInfo: [Int: [Int: String]] = [:]
    for (key, value) in rawMetadata {
      let parts = key.split(separator: ".", omittingEmptySubsequences: false)
      guard parts.first == "1" || parts.first == "2" || parts.first == "0" else { return false }
      if parts.first == "2", parts.count == 2, let index = Int(parts[1]), (0..<64).contains(index) {
        classes[index] = value
      } else if parts.first == "0", parts.count == 3,
        let layer = Int(parts[1]), (0..<64).contains(layer),
        let slot = Int(parts[2]), (0..<2).contains(slot)
      {
        cacheInfo[layer, default: [:]][slot] = value
      } else if parts.first != "1" {
        return false
      }
    }
    guard classes.count == 64, cacheInfo.count == 64 else { return false }
    for layer in 0..<64 {
      guard let kind = classes[layer],
        let info = cacheInfo[layer]
      else { return false }
      if kind == "KVCache" || kind == "KVCacheSimple" {
        guard info.count == 1, info[0] == "" else { return false }
      } else if kind == "MambaCache" {
        guard info.count == 2, info[0] == "2", info[1] == "0,1" else { return false }
      } else {
        return false
      }
    }

    struct TensorRange {
      let start: UInt64
      let end: UInt64
    }
    var ranges: [TensorRange] = []
    var shapesByLayer: [Int: [[Int]]] = [:]
    var tensorCount = 0
    let payloadBytes = expectedSnapshotBytes - 8 - headerLength
    for layer in 0..<64 {
      guard let kind = classes[layer] else { return false }
      for slot in 0..<2 {
        let tensorKey = "\(layer).\(slot)"
        guard let descriptor = root[tensorKey] as? [String: Any],
          let dtype = descriptor["dtype"] as? String,
          let bytesPerElement = safeTensorBytesPerElement(dtype),
          let shapeNumbers = descriptor["shape"] as? [NSNumber],
          !shapeNumbers.isEmpty,
          let offsets = descriptor["data_offsets"] as? [NSNumber], offsets.count == 2,
          let start = exactUnsigned(offsets[0]),
          let end = exactUnsigned(offsets[1]), start < end,
          end <= payloadBytes
        else { return false }
        var elements: UInt64 = 1
        var shape: [Int] = []
        for number in shapeNumbers {
          guard let dimension = exactUnsigned(number), dimension > 0,
            dimension <= UInt64(Int.max)
          else { return false }
          let (product, overflow) = elements.multipliedReportingOverflow(by: dimension)
          guard !overflow else { return false }
          elements = product
          shape.append(Int(dimension))
        }
        let (expectedBytes, overflow) = elements.multipliedReportingOverflow(by: bytesPerElement)
        guard !overflow, expectedBytes == end - start else { return false }
        if kind == "KVCache" || kind == "KVCacheSimple" {
          guard shape.count == 4, shape[0] == 1, shape[2] == expectedTokenCount else {
            return false
          }
        } else if shape.count < 2 {
          return false
        }
        shapesByLayer[layer, default: []].append(shape)
        ranges.append(TensorRange(start: start, end: end))
        tensorCount += 1
      }
    }
    let allowedKeys = Set(["__metadata__"] + (0..<64).flatMap { ["\($0).0", "\($0).1"] })
    guard Set(root.keys) == allowedKeys, tensorCount == 128 else { return false }
    for layer in 0..<64 where classes[layer] == "KVCache" || classes[layer] == "KVCacheSimple" {
      guard let shapes = shapesByLayer[layer], shapes.count == 2, shapes[0] == shapes[1] else {
        return false
      }
    }
    ranges.sort { $0.start < $1.start }
    var cursor: UInt64 = 0
    for range in ranges {
      guard range.start == cursor else { return false }
      cursor = range.end
    }
    return cursor == payloadBytes
  }

  private static func exactUnsigned(_ number: NSNumber) -> UInt64? {
    let value = number.doubleValue
    guard value.isFinite, value >= 0, value.rounded(.towardZero) == value,
      value <= Double(UInt64.max)
    else { return nil }
    return number.uint64Value
  }

  private static func safeTensorBytesPerElement(_ dtype: String) -> UInt64? {
    switch dtype {
    case "F16", "BF16": return 2
    case "F32": return 4
    default: return nil
    }
  }

  private static func hex(_ bytes: [UInt8]) -> String {
    bytes.map { String(format: "%02x", $0) }.joined()
  }

  private func fileExists(_ url: URL) -> Bool {
    var info = stat()
    return lstat(url.path, &info) == 0
  }

  private func removeIfPresent(_ url: URL) throws {
    guard fileExists(url) else { return }
    try FileManager.default.removeItem(at: url)
  }

  private func posixError(_ operation: String) -> NSError {
    NSError(
      domain: NSPOSIXErrorDomain, code: Int(errno), userInfo: [NSLocalizedDescriptionKey: operation]
    )
  }
}

struct WarmupTerminalFacts {
  let infoObserved: Bool
  let toolCallObserved: Bool
  let completionTextSeen: Bool
  let promptEvaluationObserved: Bool
  let observedPromptTokens: Int?
  let generationTokenCount: Int?
  let completion: String?
  let streamDrained: Bool
  let evaluationSynchronized: Bool

  var provesBoundedEvaluation: Bool {
    guard infoObserved,
      !toolCallObserved,
      promptEvaluationObserved,
      let observedPromptTokens,
      observedPromptTokens > 0,
      let generationTokenCount,
      let completion,
      (0...1).contains(generationTokenCount),
      streamDrained,
      evaluationSynchronized
    else {
      return false
    }

    switch completion {
    case "stop": return true
    case "length": return generationTokenCount == 1
    default: return false
    }
  }
}

private enum CleanupFailure: Error, Sendable {
  case invalidRequest
  case unsupportedPlatform
  case invalidModel
  case versionMismatch
  case modelUnavailable
  case modelLoadFailed
  case modelNotLoaded
  case criticalMemoryPressure
  case generationFailed
  case warmupIncomplete
  case incompleteGeneration
  case responseTooLarge
  case inputTooLarge(CleanupTokenBudgetFacts)
  case specializedInputTooLarge

  var response: SidecarFailure {
    switch self {
    case .invalidRequest:
      SidecarFailure(code: "invalid_request", message: "The cleanup request is invalid.")
    case .unsupportedPlatform:
      SidecarFailure(
        code: "unsupported_platform",
        message: "Local MLX requires Apple silicon and macOS 14 or later.")
    case .invalidModel:
      SidecarFailure(
        code: "invalid_model", message: "The requested local cleanup model is not supported.")
    case .versionMismatch:
      SidecarFailure(
        code: "model_version_mismatch",
        message: "The requested cleanup model revision is not supported.")
    case .modelUnavailable:
      SidecarFailure(
        code: "model_unavailable", message: "Verified local cleanup model files are unavailable.")
    case .modelLoadFailed:
      SidecarFailure(
        code: "model_load_failed", message: "The local cleanup model could not be loaded.")
    case .modelNotLoaded:
      SidecarFailure(
        code: "model_not_loaded", message: "The requested cleanup model is not loaded.")
    case .criticalMemoryPressure:
      SidecarFailure(
        code: "memory_pressure_critical",
        message: "Local cleanup was skipped because system memory pressure is critical.")
    case .generationFailed:
      SidecarFailure(code: "generation_failed", message: "Local cleanup did not complete.")
    case .warmupIncomplete:
      SidecarFailure(
        code: "warmup_incomplete", message: "The bounded local warmup did not complete.")
    case .incompleteGeneration:
      SidecarFailure(
        code: "generation_incomplete", message: "Local cleanup stopped before a complete response.")
    case .responseTooLarge:
      SidecarFailure(
        code: "response_too_large",
        message: "The local cleanup response exceeds the supported size.")
    case .inputTooLarge(let facts):
      SidecarFailure(
        code: "input_too_large",
        message:
          "The tokenized cleanup prompt and requested output exceed the supported token budget.",
        promptTokens: facts.promptTokens,
        requestedOutputTokens: facts.requestedOutputTokens,
        requestedTotalTokens: facts.requestedTotalTokens,
        maxTotalTokens: facts.maxTotalTokens
      )
    case .specializedInputTooLarge:
      SidecarFailure(
        code: "model_input_too_large",
        message: "S1-mini by Superwhisper accepts transcript prompts up to roughly 1,000 tokens."
      )
    }
  }
}

enum CleanupMemoryPressureLevel: Equatable, Sendable {
  case unknown
  case normal
  case warning
  case critical
}

final class CleanupMemoryPressureMonitor: @unchecked Sendable {
  static let shared = CleanupMemoryPressureMonitor()

  private let lock = NSLock()
  private var level: CleanupMemoryPressureLevel = .unknown
  private let source: (any DispatchSourceMemoryPressure)?

  init(startMonitoring: Bool = true) {
    guard startMonitoring else {
      self.source = nil
      return
    }
    let source = DispatchSource.makeMemoryPressureSource(eventMask: .all, queue: .global(qos: .utility))
    self.source = source
    source.setEventHandler { [weak self] in
      self?.record(source.data)
    }
    source.activate()
  }

  var current: CleanupMemoryPressureLevel {
    lock.lock()
    defer { lock.unlock() }
    return level
  }

  func record(_ events: DispatchSource.MemoryPressureEvent) {
    let next: CleanupMemoryPressureLevel
    if events.contains(.critical) {
      next = .critical
    } else if events.contains(.warning) {
      next = .warning
    } else if events.contains(.normal) {
      next = .normal
    } else {
      return
    }
    lock.lock()
    level = next
    lock.unlock()
  }
}

struct CleanupTokenBudgetFacts: Equatable, Sendable {
  let promptTokens: Int
  let requestedOutputTokens: Int
  let requestedTotalTokens: Int?
  let maxTotalTokens: Int
}

enum CleanupTokenBudget {
  static let maximumTotalTokens = 8_192

  static func overBudget(promptTokens: Int, requestedOutputTokens: Int) -> CleanupTokenBudgetFacts?
  {
    guard promptTokens >= 0, requestedOutputTokens > 0 else { return nil }
    let (total, overflow) = promptTokens.addingReportingOverflow(requestedOutputTokens)
    guard overflow || total > maximumTotalTokens else { return nil }
    return CleanupTokenBudgetFacts(
      promptTokens: promptTokens,
      requestedOutputTokens: requestedOutputTokens,
      requestedTotalTokens: overflow ? nil : total,
      maxTotalTokens: maximumTotalTokens
    )
  }
}

private enum CleanupRuntimeSampler {
  static func sample(
    phase: CleanupRuntimePhase,
    elapsedSeconds: Double
  ) -> CleanupRuntimeSample {
    let processUsage = processUsageSnapshot()
    let taskVM = taskVMSnapshot()
    let taskEvents = taskEventsSnapshot()
    let device: CleanupRuntimeDevice
    switch Device.defaultDevice().deviceType?.rawValue {
    case "cpu": device = .cpu
    case "gpu": device = .gpu
    default: device = .unknown
    }
    let taskPriority: CleanupRuntimeTaskPriority
    switch Task.currentPriority {
    case .userInitiated: taskPriority = .userInitiated
    case .high: taskPriority = .high
    case .medium: taskPriority = .medium
    case .low: taskPriority = .low
    case .utility: taskPriority = .utility
    case .background: taskPriority = .background
    default: taskPriority = .other
    }
    let threadQoS: CleanupRuntimeThreadQoS
    switch Thread.current.qualityOfService {
    case .userInteractive: threadQoS = .userInteractive
    case .userInitiated: threadQoS = .userInitiated
    case .utility: threadQoS = .utility
    case .background: threadQoS = .background
    case .default: threadQoS = .unspecified
    @unknown default: threadQoS = .other
    }
    let thermalState: CleanupRuntimeThermalState
    switch ProcessInfo.processInfo.thermalState {
    case .nominal: thermalState = .nominal
    case .fair: thermalState = .fair
    case .serious: thermalState = .serious
    case .critical: thermalState = .critical
    @unknown default: thermalState = .unknown
    }
    return CleanupRuntimeSample(
      phase: phase,
      elapsedSeconds: max(0, elapsedSeconds),
      mlxActiveMemoryBytes: max(0, Memory.activeMemory),
      mlxCacheMemoryBytes: max(0, Memory.cacheMemory),
      mlxPeakMemoryBytes: max(0, Memory.peakMemory),
      mlxMemoryLimitBytes: max(0, Memory.memoryLimit),
      device: device,
      taskPriority: taskPriority,
      threadQos: threadQoS,
      thermalState: thermalState,
      processUserCPUSeconds: processUsage?.userCPUSeconds,
      processSystemCPUSeconds: processUsage?.systemCPUSeconds,
      processMinorPageFaults: processUsage?.minorPageFaults,
      processMajorPageFaults: processUsage?.majorPageFaults,
      processPageIns: taskEvents?.pageIns,
      processPhysicalFootprintBytes: taskVM?.physicalFootprintBytes
    )
  }

  private static func processUsageSnapshot() -> ProcessUsageSnapshot? {
    var usage = rusage()
    guard getrusage(RUSAGE_SELF, &usage) == 0 else { return nil }
    return ProcessUsageSnapshot(
      userCPUSeconds: max(
        0,
        Double(usage.ru_utime.tv_sec) + Double(usage.ru_utime.tv_usec) / 1_000_000
      ),
      systemCPUSeconds: max(
        0,
        Double(usage.ru_stime.tv_sec) + Double(usage.ru_stime.tv_usec) / 1_000_000
      ),
      minorPageFaults: UInt64(max(0, usage.ru_minflt)),
      majorPageFaults: UInt64(max(0, usage.ru_majflt))
    )
  }

  private static func taskVMSnapshot() -> TaskVMSnapshot? {
    var info = task_vm_info_data_t()
    var count = mach_msg_type_number_t(
      MemoryLayout<task_vm_info_data_t>.size / MemoryLayout<integer_t>.size
    )
    let result = withUnsafeMutablePointer(to: &info) { pointer in
      pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
        task_info(mach_task_self_, task_flavor_t(TASK_VM_INFO), $0, &count)
      }
    }
    guard result == KERN_SUCCESS else { return nil }
    return TaskVMSnapshot(
      physicalFootprintBytes: UInt64(max(0, info.phys_footprint))
    )
  }

  private static func taskEventsSnapshot() -> TaskEventsSnapshot? {
    var info = task_events_info_data_t()
    var count = mach_msg_type_number_t(
      MemoryLayout<task_events_info_data_t>.size / MemoryLayout<integer_t>.size
    )
    let result = withUnsafeMutablePointer(to: &info) { pointer in
      pointer.withMemoryRebound(to: integer_t.self, capacity: Int(count)) {
        task_info(mach_task_self_, task_flavor_t(TASK_EVENTS_INFO), $0, &count)
      }
    }
    guard result == KERN_SUCCESS else { return nil }
    return TaskEventsSnapshot(pageIns: UInt64(max(0, info.pageins)))
  }

  private struct ProcessUsageSnapshot {
    let userCPUSeconds: Double
    let systemCPUSeconds: Double
    let minorPageFaults: UInt64
    let majorPageFaults: UInt64
  }

  private struct TaskVMSnapshot {
    let physicalFootprintBytes: UInt64
  }

  private struct TaskEventsSnapshot {
    let pageIns: UInt64
  }
}

enum CleanupResponseSizeGuard {
  static func responseOrSizeError(
    _ response: SidecarResponse,
    maximumLineBytes: Int,
    platformSupported: Bool
  ) throws -> SidecarResponse {
    do {
      _ = try SidecarIO.encodedResponseFrame(response, maximumLineBytes: maximumLineBytes)
      return response
    } catch SidecarIOError.responseTooLarge {
      return SidecarResponse(
        request: SidecarRequest(
          protocolVersion: response.protocolVersion,
          requestId: response.requestId,
          sessionId: response.sessionId,
          modelVersion: response.modelVersion,
          op: response.op
        ),
        status: "error",
        platformSupported: platformSupported,
        error: CleanupFailure.responseTooLarge.response
      )
    }
  }
}

enum CleanupPromptContract: Equatable, Sendable {
  case standardChat
  case s1Mini
}

struct CleanupModelRoute: Equatable, Sendable {
  let modelId: String
  let revision: String
  let promptRecipe: String
  let promptContract: CleanupPromptContract
  let prefixCacheManifestSHA256: String?
  let maximumPromptTokens: Int?
  let maximumGenerationTokens: Int?

  var usesPinnedPrefixCache: Bool { prefixCacheManifestSHA256 != nil }

  static let qwen38TwentySevenBManifestSHA256 =
    "f67d38ec1261b4d693cee8c006d2c56007e6919a290508266705ba4f571c9ed2"
  static let qwen38TwentySevenB = Self(
    modelId: "qwen3_8_27b_mlx_6bit",
    revision: "05f38c1f39e2f39619653616fe8e96a5d676358e",
    promptRecipe: "voiceflow-qwen35-prefix-v2",
    promptContract: .standardChat,
    prefixCacheManifestSHA256: qwen38TwentySevenBManifestSHA256,
    maximumPromptTokens: nil,
    maximumGenerationTokens: nil
  )
  static let qwen35TwoB = Self(
    modelId: "qwen3_5_2b_mlx_4bit",
    revision: "93760be4f1f69842a46bc13dbdc0f19e291392a3",
    promptRecipe: "voiceflow-qwen35-chat-cleanup-v1",
    promptContract: .standardChat,
    prefixCacheManifestSHA256: nil,
    maximumPromptTokens: nil,
    maximumGenerationTokens: nil
  )
  static let qwen35FourB = Self(
    modelId: "qwen3_5_4b_mlx_4bit",
    revision: "32f3e8ecf65426fc3306969496342d504bfa13f3",
    promptRecipe: "voiceflow-qwen35-chat-cleanup-v1",
    promptContract: .standardChat,
    prefixCacheManifestSHA256: nil,
    maximumPromptTokens: nil,
    maximumGenerationTokens: nil
  )
  static let qwen35NineB = Self(
    modelId: "qwen3_5_9b_mlx_4bit",
    revision: "938d8919941c6e7efd3c7150eff7fe9d12afa631",
    promptRecipe: "voiceflow-qwen35-chat-cleanup-v1",
    promptContract: .standardChat,
    prefixCacheManifestSHA256: nil,
    maximumPromptTokens: nil,
    maximumGenerationTokens: nil
  )

  static let gemma4E2b = Self(
    modelId: "gemma_4_e2b_it_mlx_4bit",
    revision: "75a479632d6a594ea323942b883cdbc1bdd97a14",
    promptRecipe: "voiceflow-gemma4-chat-cleanup-v1",
    promptContract: .standardChat,
    prefixCacheManifestSHA256: nil,
    maximumPromptTokens: nil,
    maximumGenerationTokens: nil
  )
  static let gemma4E4b = Self(
    modelId: "gemma_4_e4b_it_mlx_4bit",
    revision: "da76496efe4486235e6e0aa9315203e27b957bcd",
    promptRecipe: "voiceflow-gemma4-chat-cleanup-v1",
    promptContract: .standardChat,
    prefixCacheManifestSHA256: nil,
    maximumPromptTokens: nil,
    maximumGenerationTokens: nil
  )
  static let ministral3ThreeB = Self(
    modelId: "ministral_3_3b_instruct_mlx_4bit",
    revision: "a962dcb09eee4169c890e544c9eb938f1113fdee",
    promptRecipe: "voiceflow-ministral3-chat-cleanup-v1",
    promptContract: .standardChat,
    prefixCacheManifestSHA256: nil,
    maximumPromptTokens: nil,
    maximumGenerationTokens: nil
  )
  static let s1Mini = Self(
    modelId: "superwhisper_s1_mini_mlx_4bit",
    revision: "5cbd7aec3401144f88a331d385c40b65fd2548eb",
    promptRecipe: CleanupS1MiniPromptContract.promptRecipe,
    promptContract: .s1Mini,
    prefixCacheManifestSHA256: nil,
    maximumPromptTokens: 1_000,
    maximumGenerationTokens: 1_024
  )

  /// Runtime pin allowlist only; the Rust catalogue controls product availability.
  static let supported: [Self] = [
    qwen38TwentySevenB,
    qwen35TwoB,
    qwen35FourB,
    qwen35NineB,
    gemma4E2b,
    gemma4E4b,
    ministral3ThreeB,
    s1Mini,
  ]
}

enum CleanupS1MiniPromptContract {
  static let systemPrompt =
    "You are a text normalizer for speech-to-text transcripts. The input begins with a control line specifying the styling, structure, and context settings; clean the transcript to match those settings and output only the cleaned text."
  static let promptRecipe = "voiceflow-s1-mini-control-v1"

  private static let supportedControlLines: Set<String> = {
    let stylings = ["casual", "semi-casual", "semi-formal", "formal"]
    let structures = ["prose", "lists"]
    let contexts = ["general", "email"]
    return Set(
      stylings.flatMap { styling in
        structures.flatMap { structure in
          contexts.map { context in
            "[Styling: \(styling)] [Structure: \(structure)] [Context: \(context)]"
          }
        }
      }
    )
  }()

  static func accepts(language: String?, messages: [SidecarChatMessage]) -> Bool {
    guard language == "en",
      messages.count == 2,
      messages[0].role == "system",
      messages[0].content == systemPrompt,
      messages[1].role == "user",
      let newline = messages[1].content.firstIndex(of: "\n")
    else {
      return false
    }

    let controlLine = String(messages[1].content[..<newline])
    guard supportedControlLines.contains(controlLine) else { return false }
    let transcript = messages[1].content[messages[1].content.index(after: newline)...]
    guard !transcript.isEmpty else { return false }

    // Any non-ASCII codepoint makes the route fall back. This deliberately
    // rejects accented English too, keeping mixed-script input off S1-mini.
    return transcript.utf8.allSatisfy { $0 < 0x80 }
  }
}

enum CleanupModelRouteLookup: Equatable, Sendable {
  case unsupportedModel
  case revisionMismatch
  case supported(CleanupModelRoute)
}

enum CleanupModelRouteCatalog {
  static func lookup(modelId: String?, revision: String?) -> CleanupModelRouteLookup {
    guard let modelId,
      let route = CleanupModelRoute.supported.first(where: { $0.modelId == modelId })
    else {
      return .unsupportedModel
    }
    guard route.revision == revision else { return .revisionMismatch }
    return .supported(route)
  }
}

/// Cleanup has its own process and model slot so loading or cancelling this
/// language model cannot replace or terminate the ASR sidecar.
public actor CleanupSidecarServer {
  private static let modelID = CleanupModelRoute.qwen38TwentySevenB.modelId
  private static let modelRevision = CleanupModelRoute.qwen38TwentySevenB.revision
  private static let modelManifestSHA256 = CleanupModelRoute.qwen38TwentySevenBManifestSHA256
  private static let maximumInputBytes = 1_048_576
  private static let maximumResponseBytes = SidecarIO.maximumCleanupResponsePayloadBytes
  private static let maximumMessageBytes = 512 * 1_024
  private static let maximumOutputBytes = 48 * 1_024
  private static let maximumOutputTokens = 2_048

  private let modelsRoot: URL
  private let promptPrefixCache: PromptPrefixCache
  private let memoryPressure: @Sendable () -> CleanupMemoryPressureLevel
  private var modelContainer: ModelContainer?
  private var loadedModelRoute: CleanupModelRoute?
  private var shouldQuit = false
  private var activeOperation: CleanupActiveOperation?
  private var activeWaiters: [CheckedContinuation<Void, Never>] = []

  public init(modelsRootPath: String) {
    self.modelsRoot = URL(fileURLWithPath: modelsRootPath, isDirectory: true)
      .standardizedFileURL.resolvingSymlinksInPath()
    self.promptPrefixCache = PromptPrefixCache(modelsRoot: self.modelsRoot)
    self.memoryPressure = { CleanupMemoryPressureMonitor.shared.current }
  }

  init(
    modelsRootPath: String,
    memoryPressure: @escaping @Sendable () -> CleanupMemoryPressureLevel
  ) {
    self.modelsRoot = URL(fileURLWithPath: modelsRootPath, isDirectory: true)
      .standardizedFileURL.resolvingSymlinksInPath()
    self.promptPrefixCache = PromptPrefixCache(modelsRoot: self.modelsRoot)
    self.memoryPressure = memoryPressure
  }

  public func run() async {
    let inputDescriptor = dup(STDIN_FILENO)
    guard inputDescriptor >= 0 else {
      SidecarIO.writeDiagnostic(code: "protocol_io_failed")
      releaseModel()
      return
    }
    let input = JSONLineReader(
      maximumLineBytes: Self.maximumInputBytes,
      descriptor: inputDescriptor,
      ownsDescriptor: true
    )
    let (lines, continuation) = AsyncStream<CleanupInputEvent>.makeStream(
      bufferingPolicy: .bufferingOldest(4)
    )
    let overflow = CleanupInputOverflow()
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

    for await event in lines {
      switch event {
      case .line(let line):
        do {
          let request = try SidecarProtocolCodec.decodeRequest(line)
          await handle(request)
        } catch {
          SidecarIO.writeResponse(
            Self.malformedRequestResponse(), maximumBytes: Self.maximumResponseBytes)
        }
      case .eof:
        break
      case .lineTooLarge:
        SidecarIO.writeResponse(
          Self.malformedRequestResponse(), maximumBytes: Self.maximumResponseBytes)
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
    if activeOperation != nil {
      cancelActiveOperation()
      await waitForActiveOperation()
    }
    releaseModel()
  }

  private func handle(_ request: SidecarRequest) async {
    guard validIdentifier(request.requestId), request.sessionId.map(validIdentifier) ?? true,
      request.protocolVersion == sidecarProtocolVersion
    else {
      writeResponse(response(request, status: "error", failure: .invalidRequest))
      return
    }
    guard Self.platformSupported else {
      writeResponse(response(request, status: "unavailable", failure: .unsupportedPlatform))
      return
    }
    switch request.op {
    case "hello", "status":
      writeResponse(response(request, status: "ready"))
    case "load":
      guard activeOperation == nil else {
        writeResponse(response(request, status: "error", failure: .invalidRequest))
        return
      }
      writeResponse(await load(request))
    case "unload":
      guard activeOperation == nil else {
        writeResponse(response(request, status: "error", failure: .invalidRequest))
        return
      }
      releaseModel()
      writeResponse(response(request, status: "unloaded"))
    case "cleanup":
      beginOperation(request)
    case "warmup":
      beginOperation(request)
    case "cancel":
      await cancel(request)
    case "quit":
      if activeOperation != nil {
        cancelActiveOperation()
        await waitForActiveOperation()
      }
      releaseModel()
      shouldQuit = true
      writeResponse(response(request, status: "completed"))
    default:
      writeResponse(response(request, status: "error", failure: .invalidRequest))
    }
  }

  private func beginOperation(_ request: SidecarRequest) {
    guard activeOperation == nil,
      let sessionId = request.sessionId,
      validIdentifier(sessionId)
    else {
      writeResponse(response(request, status: "error", failure: .invalidRequest))
      return
    }
    guard
      case .supported(let route) = CleanupModelRouteCatalog.lookup(
        modelId: request.modelId,
        revision: request.modelVersion
      ),
      loadedModelRoute == route,
      modelContainer != nil
    else {
      writeResponse(response(request, status: "error", failure: .modelNotLoaded))
      return
    }
    guard !(route.promptContract == .s1Mini && request.op == "warmup") else {
      writeResponse(response(request, status: "error", failure: .invalidRequest))
      return
    }
    let id = UUID().uuidString
    let lifetime = CleanupGenerationLifetime()
    let taskPriority: TaskPriority = request.op == "warmup" ? .utility : .userInitiated
    let task = Task(priority: taskPriority) {
      switch request.op {
      case "cleanup": return await cleanup(request, lifetime: lifetime)
      case "warmup": return await warmup(request, lifetime: lifetime)
      default: return response(request, status: "error", failure: .invalidRequest)
      }
    }
    let active = CleanupActiveOperation(
      id: id,
      request: request,
      lifetime: lifetime,
      task: task
    )
    activeOperation = active
    Task(priority: taskPriority) { [weak self] in
      let result = await task.value
      await self?.finishOperation(id: id, result: result)
    }
  }

  private func cancel(_ request: SidecarRequest) async {
    guard let active = activeOperation else {
      writeResponse(response(request, status: "error", failure: .invalidRequest))
      return
    }
    guard let modelId = active.request.modelId,
      let modelRevision = active.request.modelVersion
    else {
      writeResponse(response(request, status: "error", failure: .invalidRequest))
      return
    }
    switch active.acceptCancel(request, modelId: modelId, modelRevision: modelRevision) {
    case .accepted:
      return
    case .duplicate:
      return
    case .rejected, .limitReached:
      writeResponse(response(request, status: "error", failure: .invalidRequest))
    }
  }

  private func cancelActiveOperation() {
    guard let active = activeOperation else { return }
    active.lifetime.cancel()
    active.task.cancel()
  }

  private func finishOperation(id: String, result: SidecarResponse) async {
    guard let active = activeOperation, active.id == id else { return }
    await active.settleWorker()
    guard let current = activeOperation, current.id == id else { return }
    let settlement = current.commitTerminal()
    let terminal =
      settlement.cancelled
      ? response(current.request, status: "cancelled")
      : result
    writeResponse(terminal)
    for acknowledgement in settlement.acknowledgements {
      writeResponse(response(acknowledgement, status: "cancelled"))
    }
    activeOperation = nil
    let waiters = activeWaiters
    activeWaiters.removeAll()
    for waiter in waiters { waiter.resume() }
  }

  private func waitForActiveOperation() async {
    guard activeOperation != nil else { return }
    await withCheckedContinuation { continuation in
      if activeOperation == nil {
        continuation.resume()
      } else {
        activeWaiters.append(continuation)
      }
    }
  }

  private func writeResponse(_ response: SidecarResponse) {
    do {
      let bounded = try CleanupResponseSizeGuard.responseOrSizeError(
        response,
        maximumLineBytes: Self.maximumResponseBytes + 1,
        platformSupported: Self.platformSupported
      )
      SidecarIO.writeResponse(bounded, maximumBytes: Self.maximumResponseBytes)
    } catch {
      SidecarIO.writeDiagnostic(code: "response_encoding_failed")
    }
  }

  func load(_ request: SidecarRequest) async -> SidecarResponse {
    guard memoryPressure() != .critical else {
      return response(request, status: "error", failure: .criticalMemoryPressure)
    }
    let route: CleanupModelRoute
    switch CleanupModelRouteCatalog.lookup(
      modelId: request.modelId,
      revision: request.modelVersion
    ) {
    case .unsupportedModel:
      return response(request, status: "error", failure: .invalidModel)
    case .revisionMismatch:
      return response(request, status: "error", failure: .versionMismatch)
    case .supported(let supportedRoute):
      route = supportedRoute
    }
    guard let rawPath = request.modelDirectory,
      let directory = validatedModelDirectory(rawPath, route: route)
    else {
      return response(request, status: "error", failure: .modelUnavailable)
    }

    releaseModel()
    do {
      // This overload uses only local model and tokenizer files. It does not
      // create a Hub downloader or fall back to network access.
      modelContainer = try await LLMModelFactory.shared.loadContainer(
        from: directory,
        using: #huggingFaceTokenizerLoader()
      )
      loadedModelRoute = route
      return response(request, status: "loaded")
    } catch {
      releaseModel()
      SidecarIO.writeDiagnostic(code: "cleanup_model_load_failed")
      return response(request, status: "error", failure: .modelLoadFailed)
    }
  }

  func cleanup(
    _ request: SidecarRequest,
    lifetime: CleanupGenerationLifetime
  ) async -> SidecarResponse {
    guard memoryPressure() != .critical else {
      return response(request, status: "error", failure: .criticalMemoryPressure)
    }
    guard let loadedModelRoute,
      loadedModelRoute.modelId == request.modelId,
      loadedModelRoute.revision == request.modelVersion,
      modelContainer != nil
    else {
      return response(request, status: "error", failure: .modelNotLoaded)
    }
    guard let messages = request.messages,
      messages.count == 2,
      messages[0].role == "system",
      messages[1].role == "user",
      messages.allSatisfy({
        !$0.content.isEmpty && $0.content.utf8.count <= Self.maximumMessageBytes
      }),
      let maxTokens = request.maxOutputTokens,
      maxTokens > 0,
      maxTokens
        <= min(
          Self.maximumOutputTokens,
          loadedModelRoute.maximumGenerationTokens ?? Self.maximumOutputTokens
        )
    else {
      return response(request, status: "error", failure: .invalidRequest)
    }
    if loadedModelRoute.promptContract == .s1Mini,
      !CleanupS1MiniPromptContract.accepts(language: request.language, messages: messages)
    {
      return response(request, status: "error", failure: .invalidRequest)
    }
    guard !lifetime.isCancelled else { return response(request, status: "cancelled") }
    guard let modelContainer else {
      return response(request, status: "error", failure: .modelNotLoaded)
    }
    guard !lifetime.isCancelled else { return response(request, status: "cancelled") }

    let cleanupActivity = ProcessInfo.processInfo.beginActivity(
      options: [.userInitiatedAllowingIdleSystemSleep],
      reason: "Foreground local dictation cleanup"
    )
    defer { ProcessInfo.processInfo.endActivity(cleanupActivity) }

    let requestProcessingStarted = ProcessInfo.processInfo.systemUptime
    let prepareStarted = requestProcessingStarted
    var phaseSamples = [CleanupRuntimeSample]()
    phaseSamples.append(
      CleanupRuntimeSampler.sample(
        phase: .beforePrepare,
        elapsedSeconds: 0
      )
    )
    var modelPrepareSeconds: Double?
    var cachePlanSeconds: Double?
    var cachePlanTimings: CleanupCachePlanTimings?
    func runtimeDiagnostics() -> CleanupRuntimeDiagnostics {
      var samples = phaseSamples
      samples.append(
        CleanupRuntimeSampler.sample(
          phase: .terminal,
          elapsedSeconds: max(
            0,
            ProcessInfo.processInfo.systemUptime - requestProcessingStarted
          )
        )
      )
      return CleanupRuntimeDiagnostics(
        modelPrepareSeconds: modelPrepareSeconds,
        cachePlanSeconds: cachePlanSeconds,
        phases: samples,
        cachePlanTimings: cachePlanTimings
      )
    }

    do {
      let input = UserInput(
        chat: [
          .system(messages[0].content),
          .user(messages[1].content),
        ],
        additionalContext: ["enable_thinking": false]
      )
      let prepared = try await modelContainer.prepare(input: input)
      guard !lifetime.isCancelled else { return response(request, status: "cancelled") }
      modelPrepareSeconds = max(
        0,
        ProcessInfo.processInfo.systemUptime - prepareStarted
      )
      phaseSamples.append(
        CleanupRuntimeSampler.sample(
          phase: .afterPrepare,
          elapsedSeconds: max(
            0,
            ProcessInfo.processInfo.systemUptime - requestProcessingStarted
          )
        )
      )
      let preparedPromptTokenCount = prepared.text.tokens.size
      if let maximumPromptTokens = loadedModelRoute.maximumPromptTokens,
        preparedPromptTokenCount > maximumPromptTokens
      {
        return response(
          request,
          status: "error",
          failure: .specializedInputTooLarge,
          cleanupDiagnostics: runtimeDiagnostics()
        )
      }
      if let facts = CleanupTokenBudget.overBudget(
        promptTokens: preparedPromptTokenCount,
        requestedOutputTokens: maxTokens
      ) {
        return response(
          request,
          status: "error",
          failure: .inputTooLarge(facts),
          cleanupDiagnostics: runtimeDiagnostics()
        )
      }
      let fullPromptTokens = prepared.text.tokens.asArray(Int.self)
      let preparedHasMask = prepared.text.mask != nil
      let parameters = GenerateParameters(
        maxTokens: maxTokens,
        temperature: 0,
        topP: 1,
        topK: 0,
        prefillStepSize: PromptPrefixBoundary.step
      )
      let cachePlanStarted = ProcessInfo.processInfo.systemUptime
      let candidateResult = await promptCacheCandidate(
        request: request,
        system: messages[0].content,
        user: messages[1].content,
        preparedHasMask: preparedHasMask,
        fullPromptTokens: fullPromptTokens,
        modelContainer: modelContainer,
        maxOutputTokens: maxTokens
      )
      cachePlanTimings = candidateResult.probeTimings.diagnostic()
      let generationPlan: PromptCacheGeneration
      if let candidate = candidateResult.candidate, !preparedHasMask {
        let prefixCache = promptPrefixCache
        let performStarted = ProcessInfo.processInfo.systemUptime
        let performResult = try await modelContainer.perform(nonSendable: prepared) {
          context, prepared in
          let closureEntrySeconds = max(
            0,
            ProcessInfo.processInfo.systemUptime - performStarted
          )
          let plan = try prefixCache.generate(
            context: context,
            prepared: prepared,
            actualTokens: fullPromptTokens,
            candidate: candidate,
            parameters: parameters,
            lifetime: lifetime
          )
          return (plan, closureEntrySeconds)
        }
        generationPlan = performResult.0
        cachePlanTimings = candidateResult.probeTimings.diagnostic(
          performToClosureEntrySeconds: performResult.1,
          mlxGenerateCallSeconds: performResult.0.mlxGenerateCallSeconds
        )
      } else {
        generationPlan = try await modelContainer.perform(nonSendable: prepared) {
          context, prepared in
          try PromptCacheGenerationFactory.make(
            context: context,
            input: prepared,
            cache: nil,
            parameters: parameters,
            metrics: .bypassed(
              candidateResult.reason ?? "unsupported_cache_state",
              promptTokens: preparedPromptTokenCount
            ),
            lifetime: lifetime
          )
        }
      }
      cachePlanSeconds = max(
        0,
        ProcessInfo.processInfo.systemUptime - cachePlanStarted
      )
      phaseSamples.append(
        CleanupRuntimeSampler.sample(
          phase: .afterPlan,
          elapsedSeconds: max(
            0,
            ProcessInfo.processInfo.systemUptime - requestProcessingStarted
          )
        )
      )
      let prepareSeconds = max(0, ProcessInfo.processInfo.systemUptime - prepareStarted)
      // This scope measures only stream consumption. First-visible also
      // includes prepare, cache lookup/restore/build, and prefill below.
      let streamStarted = ProcessInfo.processInfo.systemUptime
      var text = ""
      var promptTokens = 0
      var generationTokens = 0
      var promptTime = 0.0
      var generationTime = 0.0
      var firstVisibleChunkSeconds: Double?
      var completion: String?
      var responseTooLarge = false
      var toolCallObserved = false
      for await event in generationPlan.stream {
        switch event {
        case .chunk(let chunk):
          if firstVisibleChunkSeconds == nil, !chunk.isEmpty {
            firstVisibleChunkSeconds = max(
              0,
              ProcessInfo.processInfo.systemUptime - requestProcessingStarted
            )
          }
          if !responseTooLarge {
            text.append(chunk)
            if text.utf8.count > Self.maximumOutputBytes {
              responseTooLarge = true
              text = ""
            }
          }
        case .info(let info):
          promptTokens = info.promptTokenCount
          generationTokens = info.generationTokenCount
          promptTime = info.promptTime
          generationTime = info.generateTime
          switch info.stopReason {
          case .stop: completion = "stop"
          case .length: completion = "length"
          case .cancelled: completion = "cancelled"
          }
        case .toolCall:
          toolCallObserved = true
        }
      }
      await generationPlan.worker.value
      guard !lifetime.isCancelled else { return response(request, status: "cancelled") }
      let generationDrainSeconds = max(
        0,
        ProcessInfo.processInfo.systemUptime - streamStarted
      )
      // Always drain the generation stream before returning an error.
      // MLX can continue its task after the consumer stops iterating.
      if responseTooLarge {
        return response(
          request,
          status: "error",
          failure: .responseTooLarge,
          cleanupDiagnostics: runtimeDiagnostics(),
          promptCacheMetrics: generationPlan.metrics
        )
      }
      if toolCallObserved {
        return response(
          request,
          status: "error",
          failure: .generationFailed,
          cleanupDiagnostics: runtimeDiagnostics(),
          promptCacheMetrics: generationPlan.metrics
        )
      }
      guard completion == "stop", !text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
      else {
        return response(
          request,
          status: "error",
          failure: .incompleteGeneration,
          cleanupDiagnostics: runtimeDiagnostics(),
          promptCacheMetrics: generationPlan.metrics
        )
      }
      let promptTimeScope: String
      switch generationPlan.metrics.status {
      case "memory_hit", "snapshot_hit":
        promptTimeScope = "computed_suffix"
      case "built":
        promptTimeScope = "suffix_after_prefix_build"
      default:
        promptTimeScope = "full_prompt"
      }
      let result = CleanupResult(
        text: text,
        promptTokens: preparedPromptTokenCount,
        generationTokens: generationTokens,
        promptTimeSeconds: promptTime,
        generationTimeSeconds: generationTime,
        completion: completion ?? "unknown",
        prepareTimeSeconds: prepareSeconds,
        firstVisibleChunkSeconds: firstVisibleChunkSeconds,
        generationDrainSeconds: generationDrainSeconds,
        promptCacheStatus: generationPlan.metrics.status,
        promptCacheReason: generationPlan.metrics.reason,
        cachedPrefixTokens: generationPlan.metrics.cachedPrefixTokens,
        computedSuffixTokens: generationPlan.metrics.computedSuffixTokens,
        promptCacheSnapshotBytes: generationPlan.metrics.snapshotBytes,
        promptCacheLookupSeconds: generationPlan.metrics.lookupSeconds,
        promptCacheBuildSeconds: generationPlan.metrics.buildSeconds,
        promptCacheCopySeconds: generationPlan.metrics.copySeconds,
        promptCacheRestoreSeconds: generationPlan.metrics.restoreSeconds,
        promptTimeScope: promptTimeScope
      )
      return response(
        request,
        status: "completed",
        cleanupResult: result,
        cleanupDiagnostics: runtimeDiagnostics()
      )
    } catch {
      if lifetime.isCancelled { return response(request, status: "cancelled") }
      SidecarIO.writeDiagnostic(code: "cleanup_generation_failed")
      return response(
        request,
        status: "error",
        failure: .generationFailed,
        cleanupDiagnostics: runtimeDiagnostics()
      )
    }
  }

  private func warmup(
    _ request: SidecarRequest,
    lifetime: CleanupGenerationLifetime
  ) async -> SidecarResponse {
    guard memoryPressure() != .critical else {
      return response(request, status: "error", failure: .criticalMemoryPressure)
    }
    guard let modelContainer,
      let loadedModelRoute,
      loadedModelRoute.promptContract != .s1Mini,
      loadedModelRoute.modelId == request.modelId,
      loadedModelRoute.revision == request.modelVersion
    else {
      return response(request, status: "error", failure: .modelNotLoaded)
    }
    guard let messages = request.messages,
      messages.count == 2,
      messages[0].role == "system",
      messages[1].role == "user",
      messages.allSatisfy({
        !$0.content.isEmpty && $0.content.utf8.count <= Self.maximumMessageBytes
      }),
      request.maxOutputTokens == Self.maximumOutputTokens
    else {
      return response(request, status: "error", failure: .invalidRequest)
    }

    guard !lifetime.isCancelled else { return response(request, status: "cancelled") }
    let started = ProcessInfo.processInfo.systemUptime
    do {
      let input = UserInput(
        chat: [
          .system(messages[0].content),
          .user(messages[1].content),
        ],
        additionalContext: ["enable_thinking": false]
      )
      let prepared = try await modelContainer.prepare(input: input)
      guard !lifetime.isCancelled else { return response(request, status: "cancelled") }
      let fullPromptTokens = prepared.text.tokens.asArray(Int.self)
      if let facts = CleanupTokenBudget.overBudget(
        promptTokens: fullPromptTokens.count,
        requestedOutputTokens: Self.maximumOutputTokens
      ) {
        return response(request, status: "error", failure: .inputTooLarge(facts))
      }

      let preparedHasMask = prepared.text.mask != nil
      let productionParameters = GenerateParameters(
        maxTokens: Self.maximumOutputTokens,
        temperature: 0,
        topP: 1,
        topK: 0,
        prefillStepSize: PromptPrefixBoundary.step
      )
      let evaluationParameters = GenerateParameters(
        maxTokens: 1,
        temperature: 0,
        topP: 1,
        topK: 0,
        prefillStepSize: PromptPrefixBoundary.step
      )
      let candidateResult = await promptCacheCandidate(
        request: request,
        system: messages[0].content,
        user: messages[1].content,
        preparedHasMask: preparedHasMask,
        fullPromptTokens: fullPromptTokens,
        modelContainer: modelContainer,
        maxOutputTokens: Self.maximumOutputTokens
      )
      let planReadyAt = ProcessInfo.processInfo.systemUptime
      let generationPlan: PromptCacheGeneration
      if !preparedHasMask {
        let prefixCache = promptPrefixCache
        generationPlan = try await modelContainer.perform(nonSendable: prepared) {
          context, prepared in
          try prefixCache.warm(
            context: context,
            prepared: prepared,
            preparedHasMask: preparedHasMask,
            actualTokens: fullPromptTokens,
            candidate: candidateResult.candidate,
            candidateReason: candidateResult.reason,
            productionParameters: productionParameters,
            evaluationParameters: evaluationParameters,
            lifetime: lifetime
          )
        }
      } else {
        generationPlan = try await modelContainer.perform(nonSendable: prepared) {
          context, prepared in
          try PromptCacheGenerationFactory.make(
            context: context,
            input: prepared,
            cache: nil,
            parameters: evaluationParameters,
            metrics: .bypassed(
              candidateResult.reason ?? "unsupported_cache_state",
              promptTokens: fullPromptTokens.count
            ),
            lifetime: lifetime
          )
        }
      }

      let evaluationStarted = ProcessInfo.processInfo.systemUptime
      var generationTokens = 0
      var observedPromptTokens: Int?
      var promptTime = 0.0
      var generationTime = 0.0
      var completion: String?
      var infoObserved = false
      var toolCallObserved = false
      var completionTextSeen = false
      for await event in generationPlan.stream {
        switch event {
        case .chunk(let chunk):
          // The bounded primer output is intentionally never retained.
          completionTextSeen = completionTextSeen || !chunk.isEmpty
        case .info(let info):
          infoObserved = true
          observedPromptTokens = info.promptTokenCount
          generationTokens = info.generationTokenCount
          promptTime = info.promptTime
          generationTime = info.generateTime
          switch info.stopReason {
          case .stop: completion = "stop"
          case .length: completion = "length"
          case .cancelled: completion = "cancelled"
          }
        case .toolCall:
          toolCallObserved = true
        }
      }
      await generationPlan.worker.value
      guard !lifetime.isCancelled else { return response(request, status: "cancelled") }
      // mlx-swift-lm finishes its generation stream only after synchronizing
      // the default MLX stream. Reaching this point means both facts hold.
      let generationStreamDrained = true
      let evaluationSynchronized = generationStreamDrained
      let evaluationSeconds = max(
        0,
        ProcessInfo.processInfo.systemUptime - evaluationStarted
      )
      let promptEvaluationObserved =
        infoObserved
        && observedPromptTokens.map { $0 > 0 && $0 <= fullPromptTokens.count } == true
        && promptTime.isFinite
        && promptTime > 0
      let terminalFacts = WarmupTerminalFacts(
        infoObserved: infoObserved,
        toolCallObserved: toolCallObserved,
        completionTextSeen: completionTextSeen,
        promptEvaluationObserved: promptEvaluationObserved,
        observedPromptTokens: observedPromptTokens,
        generationTokenCount: infoObserved ? generationTokens : nil,
        completion: completion,
        streamDrained: generationStreamDrained,
        evaluationSynchronized: evaluationSynchronized
      )
      let evaluated = terminalFacts.provesBoundedEvaluation
      let result = WarmupResult(
        status: evaluated ? "evaluated" : "incomplete",
        promptTokens: fullPromptTokens.count,
        generationTokens: terminalFacts.generationTokenCount,
        completion: terminalFacts.completion,
        prepareTimeSeconds: max(0, planReadyAt - started),
        evaluationTimeSeconds: evaluationSeconds,
        promptTimeSeconds: infoObserved ? promptTime : nil,
        generationTimeSeconds: infoObserved ? generationTime : nil,
        generationDrainSeconds: evaluationSeconds,
        infoObserved: terminalFacts.infoObserved,
        toolCallObserved: terminalFacts.toolCallObserved,
        completionTextSeen: terminalFacts.completionTextSeen,
        promptEvaluationObserved: terminalFacts.promptEvaluationObserved,
        observedPromptTokens: terminalFacts.observedPromptTokens,
        streamDrained: terminalFacts.streamDrained,
        evaluationSynchronized: terminalFacts.evaluationSynchronized,
        promptCacheStatus: generationPlan.metrics.status,
        promptCacheReason: generationPlan.metrics.reason,
        cachedPrefixTokens: generationPlan.metrics.cachedPrefixTokens,
        computedSuffixTokens: generationPlan.metrics.computedSuffixTokens,
        promptCacheSnapshotBytes: generationPlan.metrics.snapshotBytes,
        promptCacheLookupSeconds: generationPlan.metrics.lookupSeconds,
        promptCacheBuildSeconds: generationPlan.metrics.buildSeconds,
        promptCacheCopySeconds: generationPlan.metrics.copySeconds,
        promptCacheRestoreSeconds: generationPlan.metrics.restoreSeconds
      )
      return response(
        request,
        status: evaluated ? "completed" : "error",
        failure: evaluated ? nil : .warmupIncomplete,
        warmupResult: result,
        warmupMetrics: generationPlan.metrics
      )
    } catch {
      if lifetime.isCancelled { return response(request, status: "cancelled") }
      SidecarIO.writeDiagnostic(code: "cleanup_warmup_failed")
      return response(request, status: "error", failure: .warmupIncomplete)
    }
  }

  private func validatedModelDirectory(_ rawPath: String, route: CleanupModelRoute) -> URL? {
    guard rawPath.utf8.count <= 4_096, rawPath.hasPrefix("/"), validDirectory(modelsRoot) else {
      return nil
    }
    let directory = URL(fileURLWithPath: rawPath, isDirectory: true).standardizedFileURL
    let prefix = modelsRoot.path.hasSuffix("/") ? modelsRoot.path : modelsRoot.path + "/"
    let expectedDirectory =
      modelsRoot
      .appendingPathComponent("cleanup", isDirectory: true)
      .appendingPathComponent(route.modelId, isDirectory: true)
      .standardizedFileURL
    guard directory.path.hasPrefix(prefix),
      directory.path == expectedDirectory.path,
      directory.resolvingSymlinksInPath().path == directory.path,
      validDirectory(directory), containsNoLinks(directory)
    else { return nil }
    return directory
  }

  private func containsNoLinks(_ directory: URL) -> Bool {
    var failed = false
    guard
      let enumerator = FileManager.default.enumerator(
        at: directory,
        includingPropertiesForKeys: nil,
        options: [],
        errorHandler: { _, _ in
          failed = true
          return false
        }
      )
    else { return false }
    while let entry = enumerator.nextObject() as? URL {
      var info = stat()
      guard entry.path.hasPrefix(directory.path + "/"), lstat(entry.path, &info) == 0 else {
        return false
      }
      let kind = info.st_mode & mode_t(S_IFMT)
      guard kind == mode_t(S_IFDIR) || kind == mode_t(S_IFREG) else { return false }
    }
    return !failed
  }

  private func validDirectory(_ directory: URL) -> Bool {
    var info = stat()
    return lstat(directory.path, &info) == 0 && (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR)
  }

  private func releaseModel() {
    modelContainer = nil
    loadedModelRoute = nil
    promptPrefixCache.clearResident()
    Memory.clearCache()
  }

  private func promptCacheCandidate(
    request: SidecarRequest,
    system: String,
    user: String,
    preparedHasMask: Bool,
    fullPromptTokens: [Int],
    modelContainer: ModelContainer,
    maxOutputTokens: Int
  ) async -> PromptCacheCandidateResult {
    var probeTimings = PromptCacheCandidateProbeTimings()
    guard loadedModelRoute?.usesPinnedPrefixCache == true else {
      return PromptCacheCandidateResult(
        candidate: nil,
        reason: "model_cache_disabled",
        probeTimings: probeTimings
      )
    }
    guard let byteCount = request.stablePrefixUserBytes,
      let stableUserPrefix = PromptPrefixBoundary.utf8Prefix(user, byteCount: byteCount)
    else {
      return PromptCacheCandidateResult(
        candidate: nil,
        reason: "boundary_not_provided",
        probeTimings: probeTimings
      )
    }
    guard !preparedHasMask else {
      return PromptCacheCandidateResult(
        candidate: nil,
        reason: "unsupported_cache_state",
        probeTimings: probeTimings
      )
    }

    // Diverse suffix probes make the common token prefix stop before any
    // user-specific continuation, including literal chat-template tokens.
    // These deliberately begin with distinct tokenizer special tokens.
    // Their common prefix identifies the token boundary before any probe
    // suffix; it also prevents prefix-only template closing/header tokens
    // or a user transcript beginning with literal special-token text from
    // being admitted into the reusable state.
    let probes = ["<|im_end|>VfBoundary-A9x", "<|im_start|>🧪-boundary-probe"]
    var probeTokenIds: [[Int]] = []
    for probe in probes {
      let probeInput = UserInput(
        chat: [
          .system(system),
          .user(stableUserPrefix + probe),
        ],
        additionalContext: ["enable_thinking": false]
      )
      let prepareStarted = ProcessInfo.processInfo.systemUptime
      do {
        let tokenizedProbe = try await modelContainer.prepare(input: probeInput)
        probeTimings.record(
          .stableUserPrefix,
          seconds: max(0, ProcessInfo.processInfo.systemUptime - prepareStarted)
        )
        guard tokenizedProbe.text.mask == nil else {
          return PromptCacheCandidateResult(
            candidate: nil,
            reason: "unsupported_cache_state",
            probeTimings: probeTimings
          )
        }
        probeTokenIds.append(tokenizedProbe.text.tokens.asArray(Int.self))
      } catch {
        probeTimings.record(
          .stableUserPrefix,
          seconds: max(0, ProcessInfo.processInfo.systemUptime - prepareStarted)
        )
        return PromptCacheCandidateResult(
          candidate: nil,
          reason: "unsupported_cache_state",
          probeTimings: probeTimings
        )
      }
    }
    let sdkIdentity = PromptCacheSDKIdentity.current
    let keyIdentity =
      "voiceflow-qwen35-prefix-v2|model=\(Self.modelID)|revision=\(Self.modelRevision)|manifest=\(Self.modelManifestSHA256)|runtime=\(sidecarRuntimeVersion)|\(sdkIdentity)|template=chat-enable-thinking-false|prefill=512|max-output=\(maxOutputTokens)|temperature=0|top-p=1|top-k=0|min-p=0|repetition=none|presence=none|frequency=none|max-kv=none|kv-bits=none|"
    if let safeBoundary = PromptPrefixBoundary.commonPrefixLength(probeTokenIds),
      let prefixLength = PromptPrefixBoundary.alignedCommonPrefixLength(
        actual: fullPromptTokens,
        probes: probeTokenIds,
        maximumTokenCount: safeBoundary
      )
    {
      let prefixTokens = Array(fullPromptTokens.prefix(prefixLength))
      return PromptCacheCandidateResult(
        candidate: PromptCacheCandidate.make(
          prefixTokens: prefixTokens,
          keyIdentity: keyIdentity
        ),
        reason: nil,
        probeTimings: probeTimings
      )
    }

    // The durable 512-token candidate above keeps its original user-boundary
    // probes and key. If it is ineligible, these two empty-user probes bound
    // only the prefix shared by the actual full prompt and fixed system
    // instructions, so context-dependent user preambles cannot enter a short
    // reusable cache.
    var systemOnlyProbeTokenIds: [[Int]] = []
    for probe in probes {
      let probeInput = UserInput(
        chat: [
          .system(system),
          .user(probe),
        ],
        additionalContext: ["enable_thinking": false]
      )
      let prepareStarted = ProcessInfo.processInfo.systemUptime
      do {
        let tokenizedProbe = try await modelContainer.prepare(input: probeInput)
        probeTimings.record(
          .systemOnly,
          seconds: max(0, ProcessInfo.processInfo.systemUptime - prepareStarted)
        )
        guard tokenizedProbe.text.mask == nil else {
          return PromptCacheCandidateResult(
            candidate: nil,
            reason: "unsupported_cache_state",
            probeTimings: probeTimings
          )
        }
        systemOnlyProbeTokenIds.append(tokenizedProbe.text.tokens.asArray(Int.self))
      } catch {
        probeTimings.record(
          .systemOnly,
          seconds: max(0, ProcessInfo.processInfo.systemUptime - prepareStarted)
        )
        return PromptCacheCandidateResult(
          candidate: nil,
          reason: "unsupported_cache_state",
          probeTimings: probeTimings
        )
      }
    }
    guard
      let prefixLength = PromptPrefixBoundary.shortSystemOnlyPrefixLength(
        actual: fullPromptTokens,
        probes: systemOnlyProbeTokenIds
      )
    else {
      return PromptCacheCandidateResult(
        candidate: nil,
        reason: "prefix_below_prefill_step",
        probeTimings: probeTimings
      )
    }
    let prefixTokens = Array(fullPromptTokens.prefix(prefixLength))
    return PromptCacheCandidateResult(
      candidate: PromptCacheCandidate.make(
        prefixTokens: prefixTokens,
        keyIdentity: keyIdentity
      ),
      reason: nil,
      probeTimings: probeTimings
    )
  }

  private func response(
    _ request: SidecarRequest,
    status: String,
    failure: CleanupFailure? = nil,
    cleanupResult: CleanupResult? = nil,
    cleanupDiagnostics: CleanupRuntimeDiagnostics? = nil,
    warmupResult: WarmupResult? = nil,
    warmupMetrics: PromptCacheMetrics? = nil,
    promptCacheMetrics: PromptCacheMetrics? = nil
  ) -> SidecarResponse {
    let cacheMetrics = promptCacheMetrics ?? warmupMetrics
    return SidecarResponse(
      request: request,
      status: status,
      platformSupported: Self.platformSupported,
      loadedModelId: modelContainer == nil ? nil : loadedModelRoute?.modelId,
      loadedModelVersion: modelContainer == nil ? nil : loadedModelRoute?.revision,
      cleanupResult: cleanupResult,
      warmupResult: warmupResult,
      cleanupDiagnostics: cleanupDiagnostics,
      error: failure.map { base in
        guard let metrics = cacheMetrics else { return base.response }
        let original = base.response
        return SidecarFailure(
          code: original.code,
          message: original.message,
          promptTokens: original.promptTokens,
          requestedOutputTokens: original.requestedOutputTokens,
          requestedTotalTokens: original.requestedTotalTokens,
          maxTotalTokens: original.maxTotalTokens,
          promptCacheStatus: metrics.status,
          promptCacheReason: metrics.reason,
          cachedPrefixTokens: metrics.cachedPrefixTokens,
          computedSuffixTokens: metrics.computedSuffixTokens,
          promptCacheSnapshotBytes: metrics.snapshotBytes,
          promptCacheLookupSeconds: metrics.lookupSeconds,
          promptCacheBuildSeconds: metrics.buildSeconds,
          promptCacheCopySeconds: metrics.copySeconds,
          promptCacheRestoreSeconds: metrics.restoreSeconds
        )
      }
    )
  }

  private static func malformedRequestResponse() -> SidecarResponse {
    let request = SidecarRequest(
      protocolVersion: sidecarProtocolVersion,
      requestId: "invalid",
      sessionId: nil,
      modelVersion: nil,
      op: "invalid"
    )
    return SidecarResponse(
      request: request,
      status: "error",
      platformSupported: platformSupported,
      error: CleanupFailure.invalidRequest.response
    )
  }

  private func validIdentifier(_ value: String) -> Bool {
    !value.isEmpty && value.utf8.count <= 128
      && value.utf8.allSatisfy {
        ($0 >= 48 && $0 <= 57) || ($0 >= 65 && $0 <= 90) || ($0 >= 97 && $0 <= 122)
          || $0 == 45 || $0 == 46 || $0 == 58 || $0 == 95
      }
  }

  private static var platformSupported: Bool {
    #if arch(arm64)
      ProcessInfo.processInfo.operatingSystemVersion.majorVersion >= 14
    #else
      false
    #endif
  }
}
