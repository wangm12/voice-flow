import Foundation

public let sidecarProtocolVersion = 1
public let sidecarRuntimeVersion = "1.1.0"

// Measured after the loader has installed the actual parameter arrays. This is
// a representation inventory, not a claim that every computation uses one dtype.
public struct LoadedAttestation: Encodable, Sendable {
  public let schemaVersion: Int
  public let executableSHA256: String
  public let metalSHA256: [String: String]
  public let loadedImages: [String]
  public let parameterLayoutSHA256: String
  public let parameterCount: Int
  public let parameterDtypes: [String: Int]
  public let decoderRecipe: String
}

public struct SidecarRequest: Decodable, Sendable {
  public let protocolVersion: Int
  public let requestId: String
  public let sessionId: String?
  public let modelVersion: String?
  public let op: String
  public let modelId: String?
  public let modelDirectory: String?
  public let audioPath: String?
  public let language: String?
  public let audioMaxBytes: Int?
  public let maxTokensPerChunk: Int?
  public let maxTokensTotal: Int?
  public let messages: [SidecarChatMessage]?
  public let maxOutputTokens: Int?
  public let stablePrefixUserBytes: Int?

  public init(
    protocolVersion: Int,
    requestId: String,
    sessionId: String?,
    modelVersion: String?,
    op: String,
    modelId: String? = nil,
    modelDirectory: String? = nil,
    audioPath: String? = nil,
    language: String? = nil,
    audioMaxBytes: Int? = nil,
    maxTokensPerChunk: Int? = nil,
    maxTokensTotal: Int? = nil,
    messages: [SidecarChatMessage]? = nil,
    maxOutputTokens: Int? = nil,
    stablePrefixUserBytes: Int? = nil
  ) {
    self.protocolVersion = protocolVersion
    self.requestId = requestId
    self.sessionId = sessionId
    self.modelVersion = modelVersion
    self.op = op
    self.modelId = modelId
    self.modelDirectory = modelDirectory
    self.audioPath = audioPath
    self.language = language
    self.audioMaxBytes = audioMaxBytes
    self.maxTokensPerChunk = maxTokensPerChunk
    self.maxTokensTotal = maxTokensTotal
    self.messages = messages
    self.maxOutputTokens = maxOutputTokens
    self.stablePrefixUserBytes = stablePrefixUserBytes
  }
}

public struct SidecarChatMessage: Codable, Sendable {
  public let role: String
  public let content: String

  public init(role: String, content: String) {
    self.role = role
    self.content = content
  }
}

public struct SidecarFailure: Encodable, Sendable {
  public let code: String
  public let message: String
  public let promptTokens: Int?
  public let requestedOutputTokens: Int?
  public let requestedTotalTokens: Int?
  public let maxTotalTokens: Int?
  public let promptCacheStatus: String?
  public let promptCacheReason: String?
  public let cachedPrefixTokens: Int?
  public let computedSuffixTokens: Int?
  public let promptCacheSnapshotBytes: UInt64?
  public let promptCacheLookupSeconds: Double?
  public let promptCacheBuildSeconds: Double?
  public let promptCacheCopySeconds: Double?
  public let promptCacheRestoreSeconds: Double?

  public init(
    code: String,
    message: String,
    promptTokens: Int? = nil,
    requestedOutputTokens: Int? = nil,
    requestedTotalTokens: Int? = nil,
    maxTotalTokens: Int? = nil,
    promptCacheStatus: String? = nil,
    promptCacheReason: String? = nil,
    cachedPrefixTokens: Int? = nil,
    computedSuffixTokens: Int? = nil,
    promptCacheSnapshotBytes: UInt64? = nil,
    promptCacheLookupSeconds: Double? = nil,
    promptCacheBuildSeconds: Double? = nil,
    promptCacheCopySeconds: Double? = nil,
    promptCacheRestoreSeconds: Double? = nil
  ) {
    self.code = code
    self.message = message
    self.promptTokens = promptTokens
    self.requestedOutputTokens = requestedOutputTokens
    self.requestedTotalTokens = requestedTotalTokens
    self.maxTotalTokens = maxTotalTokens
    self.promptCacheStatus = promptCacheStatus
    self.promptCacheReason = promptCacheReason
    self.cachedPrefixTokens = cachedPrefixTokens
    self.computedSuffixTokens = computedSuffixTokens
    self.promptCacheSnapshotBytes = promptCacheSnapshotBytes
    self.promptCacheLookupSeconds = promptCacheLookupSeconds
    self.promptCacheBuildSeconds = promptCacheBuildSeconds
    self.promptCacheCopySeconds = promptCacheCopySeconds
    self.promptCacheRestoreSeconds = promptCacheRestoreSeconds
  }
}

public struct TranscriptionResult: Encodable, Sendable {
  public let text: String
  public let language: String?
  public let generationTokens: Int
  public let coveredSamples: Int
  public let completion: String
  public let chunksCompleted: Int
  public let chunksTotal: Int

  public init(
    text: String,
    language: String?,
    generationTokens: Int,
    coveredSamples: Int,
    budgetExhausted: Bool,
    chunksCompleted: Int,
    chunksTotal: Int
  ) {
    self.text = text
    self.language = language
    self.generationTokens = generationTokens
    self.coveredSamples = coveredSamples
    self.completion = budgetExhausted ? "budget_exhausted" : "complete"
    self.chunksCompleted = chunksCompleted
    self.chunksTotal = chunksTotal
  }
}

public struct CleanupResult: Encodable, Sendable {
  public let text: String
  public let promptTokens: Int
  public let generationTokens: Int
  public let promptTimeSeconds: Double
  public let generationTimeSeconds: Double
  public let prepareTimeSeconds: Double?
  public let firstVisibleChunkSeconds: Double?
  public let generationDrainSeconds: Double?
  public let promptCacheStatus: String?
  public let promptCacheReason: String?
  public let cachedPrefixTokens: Int?
  public let computedSuffixTokens: Int?
  public let promptCacheSnapshotBytes: UInt64?
  public let promptCacheLookupSeconds: Double?
  public let promptCacheBuildSeconds: Double?
  public let promptCacheCopySeconds: Double?
  public let promptCacheRestoreSeconds: Double?
  public let promptTimeScope: String?
  public let completion: String

  public init(
    text: String,
    promptTokens: Int,
    generationTokens: Int,
    promptTimeSeconds: Double,
    generationTimeSeconds: Double,
    completion: String,
    prepareTimeSeconds: Double? = nil,
    firstVisibleChunkSeconds: Double? = nil,
    generationDrainSeconds: Double? = nil,
    promptCacheStatus: String? = nil,
    promptCacheReason: String? = nil,
    cachedPrefixTokens: Int? = nil,
    computedSuffixTokens: Int? = nil,
    promptCacheSnapshotBytes: UInt64? = nil,
    promptCacheLookupSeconds: Double? = nil,
    promptCacheBuildSeconds: Double? = nil,
    promptCacheCopySeconds: Double? = nil,
    promptCacheRestoreSeconds: Double? = nil,
    promptTimeScope: String? = nil
  ) {
    self.text = text
    self.promptTokens = promptTokens
    self.generationTokens = generationTokens
    self.promptTimeSeconds = promptTimeSeconds
    self.generationTimeSeconds = generationTimeSeconds
    self.prepareTimeSeconds = prepareTimeSeconds
    self.firstVisibleChunkSeconds = firstVisibleChunkSeconds
    self.generationDrainSeconds = generationDrainSeconds
    self.promptCacheStatus = promptCacheStatus
    self.promptCacheReason = promptCacheReason
    self.cachedPrefixTokens = cachedPrefixTokens
    self.computedSuffixTokens = computedSuffixTokens
    self.promptCacheSnapshotBytes = promptCacheSnapshotBytes
    self.promptCacheLookupSeconds = promptCacheLookupSeconds
    self.promptCacheBuildSeconds = promptCacheBuildSeconds
    self.promptCacheCopySeconds = promptCacheCopySeconds
    self.promptCacheRestoreSeconds = promptCacheRestoreSeconds
    self.promptTimeScope = promptTimeScope
    self.completion = completion
  }
}

/// Metadata only: the internal startup primer output is deliberately omitted.
public struct WarmupResult: Encodable, Sendable {
  public let status: String
  public let promptTokens: Int
  public let generationTokens: Int?
  public let completion: String?
  public let prepareTimeSeconds: Double
  public let evaluationTimeSeconds: Double
  public let promptTimeSeconds: Double?
  public let generationTimeSeconds: Double?
  public let generationDrainSeconds: Double
  public let infoObserved: Bool
  public let toolCallObserved: Bool
  public let completionTextSeen: Bool
  public let promptEvaluationObserved: Bool
  public let observedPromptTokens: Int?
  public let streamDrained: Bool
  public let evaluationSynchronized: Bool
  public let promptCacheStatus: String
  public let promptCacheReason: String?
  public let cachedPrefixTokens: Int
  public let computedSuffixTokens: Int
  public let promptCacheSnapshotBytes: UInt64
  public let promptCacheLookupSeconds: Double?
  public let promptCacheBuildSeconds: Double?
  public let promptCacheCopySeconds: Double?
  public let promptCacheRestoreSeconds: Double?

  public init(
    status: String,
    promptTokens: Int,
    generationTokens: Int?,
    completion: String?,
    prepareTimeSeconds: Double,
    evaluationTimeSeconds: Double,
    promptTimeSeconds: Double?,
    generationTimeSeconds: Double?,
    generationDrainSeconds: Double,
    infoObserved: Bool,
    toolCallObserved: Bool,
    completionTextSeen: Bool,
    promptEvaluationObserved: Bool,
    observedPromptTokens: Int?,
    streamDrained: Bool,
    evaluationSynchronized: Bool,
    promptCacheStatus: String,
    promptCacheReason: String?,
    cachedPrefixTokens: Int,
    computedSuffixTokens: Int,
    promptCacheSnapshotBytes: UInt64,
    promptCacheLookupSeconds: Double?,
    promptCacheBuildSeconds: Double?,
    promptCacheCopySeconds: Double?,
    promptCacheRestoreSeconds: Double?
  ) {
    self.status = status
    self.promptTokens = promptTokens
    self.generationTokens = generationTokens
    self.completion = completion
    self.prepareTimeSeconds = prepareTimeSeconds
    self.evaluationTimeSeconds = evaluationTimeSeconds
    self.promptTimeSeconds = promptTimeSeconds
    self.generationTimeSeconds = generationTimeSeconds
    self.generationDrainSeconds = generationDrainSeconds
    self.infoObserved = infoObserved
    self.toolCallObserved = toolCallObserved
    self.completionTextSeen = completionTextSeen
    self.promptEvaluationObserved = promptEvaluationObserved
    self.observedPromptTokens = observedPromptTokens
    self.streamDrained = streamDrained
    self.evaluationSynchronized = evaluationSynchronized
    self.promptCacheStatus = promptCacheStatus
    self.promptCacheReason = promptCacheReason
    self.cachedPrefixTokens = cachedPrefixTokens
    self.computedSuffixTokens = computedSuffixTokens
    self.promptCacheSnapshotBytes = promptCacheSnapshotBytes
    self.promptCacheLookupSeconds = promptCacheLookupSeconds
    self.promptCacheBuildSeconds = promptCacheBuildSeconds
    self.promptCacheCopySeconds = promptCacheCopySeconds
    self.promptCacheRestoreSeconds = promptCacheRestoreSeconds
  }
}

public enum CleanupRuntimePhase: String, Encodable, Sendable {
  case beforePrepare = "before_prepare"
  case afterPrepare = "after_prepare"
  case afterPlan = "after_plan"
  case terminal
}

public enum CleanupRuntimeDevice: String, Encodable, Sendable {
  case cpu
  case gpu
  case unknown
}

public enum CleanupRuntimeTaskPriority: String, Encodable, Sendable {
  case userInitiated = "user_initiated"
  case high
  case medium
  case low
  case utility
  case background
  case other
}

public enum CleanupRuntimeThreadQoS: String, Encodable, Sendable {
  case userInteractive = "user_interactive"
  case userInitiated = "user_initiated"
  case utility
  case background
  case maintenance
  case unspecified
  case other
}

public enum CleanupRuntimeThermalState: String, Encodable, Sendable {
  case nominal
  case fair
  case serious
  case critical
  case unknown
}

/// Bounded, content-free process and MLX facts sampled at cleanup phase boundaries.
public struct CleanupRuntimeSample: Encodable, Sendable {
  public let phase: CleanupRuntimePhase
  public let elapsedSeconds: Double
  public let mlxActiveMemoryBytes: Int
  public let mlxCacheMemoryBytes: Int
  /// MLX's peak counter is process-global and is intentionally not reset per request.
  public let mlxPeakMemoryBytes: Int
  public let mlxMemoryLimitBytes: Int
  public let device: CleanupRuntimeDevice
  public let taskPriority: CleanupRuntimeTaskPriority
  public let threadQos: CleanupRuntimeThreadQoS
  public let thermalState: CleanupRuntimeThermalState
  public let processUserCPUSeconds: Double?
  public let processSystemCPUSeconds: Double?
  public let processMinorPageFaults: UInt64?
  public let processMajorPageFaults: UInt64?
  public let processPageIns: UInt64?
  public let processPhysicalFootprintBytes: UInt64?

  public init(
    phase: CleanupRuntimePhase,
    elapsedSeconds: Double,
    mlxActiveMemoryBytes: Int,
    mlxCacheMemoryBytes: Int,
    mlxPeakMemoryBytes: Int,
    mlxMemoryLimitBytes: Int,
    device: CleanupRuntimeDevice,
    taskPriority: CleanupRuntimeTaskPriority,
    threadQos: CleanupRuntimeThreadQoS,
    thermalState: CleanupRuntimeThermalState,
    processUserCPUSeconds: Double?,
    processSystemCPUSeconds: Double?,
    processMinorPageFaults: UInt64?,
    processMajorPageFaults: UInt64?,
    processPageIns: UInt64?,
    processPhysicalFootprintBytes: UInt64?
  ) {
    self.phase = phase
    self.elapsedSeconds = elapsedSeconds
    self.mlxActiveMemoryBytes = mlxActiveMemoryBytes
    self.mlxCacheMemoryBytes = mlxCacheMemoryBytes
    self.mlxPeakMemoryBytes = mlxPeakMemoryBytes
    self.mlxMemoryLimitBytes = mlxMemoryLimitBytes
    self.device = device
    self.taskPriority = taskPriority
    self.threadQos = threadQos
    self.thermalState = thermalState
    self.processUserCPUSeconds = processUserCPUSeconds
    self.processSystemCPUSeconds = processSystemCPUSeconds
    self.processMinorPageFaults = processMinorPageFaults
    self.processMajorPageFaults = processMajorPageFaults
    self.processPageIns = processPageIns
    self.processPhysicalFootprintBytes = processPhysicalFootprintBytes
  }
}

/// Content-free timings nested inside the existing cache-plan interval.
/// Probe durations are sums of the measured prepare calls, not disjoint from
/// `cachePlanSeconds`; the MLX call may synchronously prefill the suffix.
public struct CleanupCachePlanTimings: Encodable, Sendable {
  public let candidatePrepareCount: Int
  public let candidatePrepareMilliseconds: Double
  public let stableUserPrefixPrepareCount: Int
  public let stableUserPrefixPrepareMilliseconds: Double
  public let systemOnlyPrepareCount: Int
  public let systemOnlyPrepareMilliseconds: Double
  /// Wall time from calling ModelContainer.perform until its closure begins.
  /// This includes scheduling/actor admission and is not a pure actor-wait timer.
  public let performToClosureEntryMilliseconds: Double?
  /// Duration of the existing MLXLMCommon.generate call. In the pinned SDK
  /// this can include synchronous TokenIterator setup and suffix evaluation.
  public let mlxGenerateCallMilliseconds: Double?

  public init(
    candidatePrepareCount: Int,
    candidatePrepareMilliseconds: Double,
    stableUserPrefixPrepareCount: Int,
    stableUserPrefixPrepareMilliseconds: Double,
    systemOnlyPrepareCount: Int,
    systemOnlyPrepareMilliseconds: Double,
    performToClosureEntryMilliseconds: Double?,
    mlxGenerateCallMilliseconds: Double?
  ) {
    self.candidatePrepareCount = candidatePrepareCount
    self.candidatePrepareMilliseconds = candidatePrepareMilliseconds
    self.stableUserPrefixPrepareCount = stableUserPrefixPrepareCount
    self.stableUserPrefixPrepareMilliseconds = stableUserPrefixPrepareMilliseconds
    self.systemOnlyPrepareCount = systemOnlyPrepareCount
    self.systemOnlyPrepareMilliseconds = systemOnlyPrepareMilliseconds
    self.performToClosureEntryMilliseconds = performToClosureEntryMilliseconds
    self.mlxGenerateCallMilliseconds = mlxGenerateCallMilliseconds
  }

  fileprivate var isSafeToEncode: Bool {
    let (expectedCount, countOverflow) = stableUserPrefixPrepareCount.addingReportingOverflow(
      systemOnlyPrepareCount
    )
    let expectedMilliseconds =
      stableUserPrefixPrepareMilliseconds + systemOnlyPrepareMilliseconds
    return !countOverflow
      && candidatePrepareCount >= 0
      && candidatePrepareCount <= 4
      && stableUserPrefixPrepareCount >= 0
      && stableUserPrefixPrepareCount <= 2
      && systemOnlyPrepareCount >= 0
      && systemOnlyPrepareCount <= 2
      && candidatePrepareCount == expectedCount
      && candidatePrepareMilliseconds.isFinite
      && candidatePrepareMilliseconds >= 0
      && stableUserPrefixPrepareMilliseconds.isFinite
      && stableUserPrefixPrepareMilliseconds >= 0
      && systemOnlyPrepareMilliseconds.isFinite
      && systemOnlyPrepareMilliseconds >= 0
      && expectedMilliseconds.isFinite
      && candidatePrepareMilliseconds == expectedMilliseconds
      && (stableUserPrefixPrepareCount != 0 || stableUserPrefixPrepareMilliseconds == 0)
      && (systemOnlyPrepareCount != 0 || systemOnlyPrepareMilliseconds == 0)
      && (performToClosureEntryMilliseconds.map { $0.isFinite && $0 >= 0 } ?? true)
      && (mlxGenerateCallMilliseconds.map { $0.isFinite && $0 >= 0 } ?? true)
  }
}

public struct CleanupRuntimeDiagnostics: Encodable, Sendable {
  public let modelPrepareSeconds: Double?
  public let cachePlanSeconds: Double?
  public let phases: [CleanupRuntimeSample]
  public let cachePlanTimings: CleanupCachePlanTimings?

  public init(
    modelPrepareSeconds: Double?,
    cachePlanSeconds: Double?,
    phases: [CleanupRuntimeSample],
    cachePlanTimings: CleanupCachePlanTimings? = nil
  ) {
    self.modelPrepareSeconds = modelPrepareSeconds
    self.cachePlanSeconds = cachePlanSeconds
    self.phases = phases
    self.cachePlanTimings = cachePlanTimings
  }

  private enum CodingKeys: String, CodingKey {
    case modelPrepareSeconds
    case cachePlanSeconds
    case phases
    case cachePlanTimings
  }

  public func encode(to encoder: Encoder) throws {
    var container = encoder.container(keyedBy: CodingKeys.self)
    try container.encodeIfPresent(modelPrepareSeconds, forKey: .modelPrepareSeconds)
    try container.encodeIfPresent(cachePlanSeconds, forKey: .cachePlanSeconds)
    try container.encode(phases, forKey: .phases)
    if let cachePlanTimings, cachePlanTimings.isSafeToEncode {
      try container.encode(cachePlanTimings, forKey: .cachePlanTimings)
    }
  }
}

public struct SidecarResponse: Encodable, Sendable {
  public let protocolVersion: Int
  public let requestId: String
  public let sessionId: String?
  public let modelVersion: String?
  public let op: String
  public let status: String
  public let platformSupported: Bool
  public let runtimeVersion: String
  public let loadedModelId: String?
  public let loadedModelVersion: String?
  public let loadedAttestation: LoadedAttestation?
  public let result: TranscriptionResult?
  public let cleanupResult: CleanupResult?
  public let warmupResult: WarmupResult?
  public let cleanupDiagnostics: CleanupRuntimeDiagnostics?
  public let error: SidecarFailure?

  public init(
    request: SidecarRequest,
    status: String,
    platformSupported: Bool,
    loadedModelId: String? = nil,
    loadedModelVersion: String? = nil,
    loadedAttestation: LoadedAttestation? = nil,
    result: TranscriptionResult? = nil,
    cleanupResult: CleanupResult? = nil,
    warmupResult: WarmupResult? = nil,
    cleanupDiagnostics: CleanupRuntimeDiagnostics? = nil,
    error: SidecarFailure? = nil
  ) {
    self.protocolVersion = sidecarProtocolVersion
    self.requestId = request.requestId
    self.sessionId = request.sessionId
    self.modelVersion = request.modelVersion
    self.op = request.op
    self.status = status
    self.platformSupported = platformSupported
    self.runtimeVersion = sidecarRuntimeVersion
    self.loadedModelId = loadedModelId
    self.loadedModelVersion = loadedModelVersion
    self.loadedAttestation = loadedAttestation
    self.result = result
    self.cleanupResult = cleanupResult
    self.warmupResult = warmupResult
    self.cleanupDiagnostics = cleanupDiagnostics
    self.error = error
  }

  private enum CodingKeys: String, CodingKey {
    case protocolVersion
    case requestId
    case sessionId
    case modelVersion
    case op
    case status
    case platformSupported
    case runtimeVersion
    case loadedModelId
    case loadedModelVersion
    case loadedAttestation
    case result
    case cleanupResult
    case warmupResult
    case cleanupDiagnostics
    case error
  }

  public func encode(to encoder: Encoder) throws {
    var container = encoder.container(keyedBy: CodingKeys.self)
    try container.encode(protocolVersion, forKey: .protocolVersion)
    try container.encode(requestId, forKey: .requestId)
    try container.encode(sessionId, forKey: .sessionId)
    try container.encode(modelVersion, forKey: .modelVersion)
    try container.encode(op, forKey: .op)
    try container.encode(status, forKey: .status)
    try container.encode(platformSupported, forKey: .platformSupported)
    try container.encode(runtimeVersion, forKey: .runtimeVersion)
    try container.encode(loadedModelId, forKey: .loadedModelId)
    try container.encode(loadedModelVersion, forKey: .loadedModelVersion)
    try container.encodeIfPresent(loadedAttestation, forKey: .loadedAttestation)
    try container.encode(result, forKey: .result)
    try container.encode(cleanupResult, forKey: .cleanupResult)
    try container.encode(warmupResult, forKey: .warmupResult)
    try container.encodeIfPresent(cleanupDiagnostics, forKey: .cleanupDiagnostics)
    try container.encode(error, forKey: .error)
  }
}

public enum SidecarProtocolCodec {
  public static func decodeRequest(_ data: Data) throws -> SidecarRequest {
    let decoder = JSONDecoder()
    decoder.keyDecodingStrategy = .convertFromSnakeCase
    return try decoder.decode(SidecarRequest.self, from: data)
  }

  public static func encodeResponse(_ response: SidecarResponse) throws -> Data {
    let encoder = JSONEncoder()
    encoder.keyEncodingStrategy = .convertToSnakeCase
    encoder.outputFormatting = [.sortedKeys, .withoutEscapingSlashes]
    return try encoder.encode(response)
  }
}
