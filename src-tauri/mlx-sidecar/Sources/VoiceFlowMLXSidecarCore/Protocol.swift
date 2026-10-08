import Foundation

public let sidecarProtocolVersion = 1
public let sidecarRuntimeVersion = "1.0.0"

public struct SidecarRequest: Decodable {
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
        maxTokensTotal: Int? = nil
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
    }
}

public struct SidecarFailure: Encodable {
    public let code: String
    public let message: String

    public init(code: String, message: String) {
        self.code = code
        self.message = message
    }
}

public struct TranscriptionResult: Encodable {
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

public struct SidecarResponse: Encodable {
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
    public let result: TranscriptionResult?
    public let error: SidecarFailure?

    public init(
        request: SidecarRequest,
        status: String,
        platformSupported: Bool,
        loadedModelId: String? = nil,
        loadedModelVersion: String? = nil,
        result: TranscriptionResult? = nil,
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
        self.result = result
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
        case result
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
        try container.encode(result, forKey: .result)
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
