import Darwin
import Foundation
import MLX
import MLXNN
import CryptoKit
import MachO
import MLXAudioCore
import MLXAudioSTT

private enum ModelID: String, Equatable {
    case qwen06 = "qwen3-asr-0.6b"
    case qwen17 = "qwen3-asr-1.7b"
    case cohere2 = "cohere-transcribe-2b"

    var revision: String {
        switch self {
        case .qwen06:
            return "eae2b51f96265328f1e7beced788adb0e4536f92"
        case .qwen17:
            return "e1f6c266914abc5a46e8756e02580f834a6cf8a7"
        case .cohere2:
            return "06bbe0d853a93cbbf7c1f40cffccd4c5481459ef"
        }
    }
}

private enum LoadedModel {
    case qwen(ModelID, Qwen3ASRModel)
    case cohere(ModelID, CohereTranscribeModel)

    var id: ModelID {
        switch self {
        case .qwen(let id, _), .cohere(let id, _): return id
        }
    }
}

private enum RequestFailure: Error {
    case invalidRequest
    case unsupportedPlatform
    case invalidModel
    case versionMismatch
    case modelUnavailable
    case modelLoadFailed
    case modelNotLoaded
    case audioInvalid
    case audioTooLarge
    case audioTooLong
    case unsupportedLanguage
    case generationFailed
    case responseTooLarge

    var response: SidecarFailure {
        switch self {
        case .invalidRequest:
            return SidecarFailure(code: "invalid_request", message: "The sidecar request is invalid.")
        case .unsupportedPlatform:
            return SidecarFailure(code: "unsupported_platform", message: "Local MLX requires Apple silicon and macOS 14 or later.")
        case .invalidModel:
            return SidecarFailure(code: "invalid_model", message: "The requested local model is not supported.")
        case .versionMismatch:
            return SidecarFailure(code: "model_version_mismatch", message: "The requested model revision is not supported.")
        case .modelUnavailable:
            return SidecarFailure(code: "model_unavailable", message: "Verified local model files are unavailable.")
        case .modelLoadFailed:
            return SidecarFailure(code: "model_load_failed", message: "The local model could not be loaded.")
        case .modelNotLoaded:
            return SidecarFailure(code: "model_not_loaded", message: "The requested local model is not loaded.")
        case .audioInvalid:
            return SidecarFailure(code: "invalid_audio_file", message: "The private WAV file is invalid.")
        case .audioTooLarge:
            return SidecarFailure(code: "audio_too_large", message: "The private WAV file exceeds the supported size.")
        case .audioTooLong:
            return SidecarFailure(code: "audio_too_long", message: "The private WAV file exceeds the supported duration.")
        case .unsupportedLanguage:
            return SidecarFailure(code: "unsupported_language", message: "This model requires an explicitly supported language.")
        case .generationFailed:
            return SidecarFailure(code: "generation_failed", message: "Local transcription did not complete.")
        case .responseTooLarge:
            return SidecarFailure(code: "response_too_large", message: "The local transcription response exceeds the supported size.")
        }
    }
}

private enum AttestationFailure: Error {
    case parameters
    case executableIdentity
    case parameterLayout
    case metalResources
    case loadedImages(String)
    case executableHash

    var diagnosticCode: String {
        switch self {
        case .parameters: return "model_attestation_parameters_failed"
        case .executableIdentity: return "model_attestation_identity_failed"
        case .parameterLayout: return "model_attestation_layout_failed"
        case .metalResources: return "model_attestation_metal_failed"
        case .loadedImages(let diagnosticCode): return diagnosticCode
        case .executableHash: return "model_attestation_executable_hash_failed"
        }
    }
}

private struct GeneratedChunk {
    let text: String
    let language: String?
    let generationTokens: Int
    let mayHaveReachedGenerationLimit: Bool
}

private enum ASRCancellationEffectError: Error {
    case cancelled
}

private protocol ASRCancellableEffect: Sendable {
    func check() throws
}

final class ASRRequestCancellation: ASRCancellableEffect, @unchecked Sendable {
    private let lock = NSLock()
    private let original: SidecarRequest
    private var acceptedRequest: SidecarRequest?
    private var cancellationRequested = false

    init(original: SidecarRequest) {
        self.original = original
    }

    var acceptedCancelRequest: SidecarRequest? {
        lock.lock()
        defer { lock.unlock() }
        return acceptedRequest
    }

    var isCancellationRequested: Bool {
        lock.lock()
        defer { lock.unlock() }
        return cancellationRequested
    }

    func accept(_ request: SidecarRequest) -> Bool {
        lock.lock()
        defer { lock.unlock() }
        guard acceptedRequest == nil,
              original.op == "transcribe",
              original.protocolVersion == sidecarProtocolVersion,
              original.sessionId.map(validSidecarIdentifier) == true,
              original.modelId != nil,
              original.modelVersion != nil,
              request.op == "cancel",
              request.protocolVersion == original.protocolVersion,
              request.requestId != original.requestId,
              validSidecarIdentifier(request.requestId),
              request.sessionId == original.sessionId,
              request.modelId == original.modelId,
              request.modelVersion == original.modelVersion,
              request.modelDirectory == nil,
              request.audioPath == nil,
              request.language == nil,
              request.audioMaxBytes == nil,
              request.maxTokensPerChunk == nil,
              request.maxTokensTotal == nil,
              request.messages == nil,
              request.maxOutputTokens == nil,
              request.stablePrefixUserBytes == nil else {
            return false
        }
        acceptedRequest = request
        cancellationRequested = true
        return true
    }

    func cancelForShutdown() {
        lock.lock()
        cancellationRequested = true
        lock.unlock()
    }

    func check() throws {
        if isCancellationRequested { throw ASRCancellationEffectError.cancelled }
    }
}

func validSidecarIdentifier(_ value: String) -> Bool {
    !value.isEmpty && value.utf8.count <= 128 && value.utf8.allSatisfy {
        ($0 >= 48 && $0 <= 57) || ($0 >= 65 && $0 <= 90) || ($0 >= 97 && $0 <= 122)
            || $0 == 45 || $0 == 46 || $0 == 58 || $0 == 95
    }
}

func runASRSynchronousWorker<Result: Sendable>(
    cancellation: ASRRequestCancellation,
    synchronize: @Sendable () -> Void,
    operation: () throws -> Result
) throws -> Result {
    defer { synchronize() }
    try cancellation.check()
    let result = try operation()
    try cancellation.check()
    return result
}

actor ASRModelRuntime: ASRRuntimeHandling {
    nonisolated let executor = DispatchSerialQueue(label: "com.voiceflow.asr.model-owner")
    nonisolated var unownedExecutor: UnownedSerialExecutor {
        executor.asUnownedSerialExecutor()
    }

    static let maximumInputBytes = 1_048_576
    static let maximumResponseBytes = SidecarIO.maximumASRResponsePayloadBytes
    private static let maximumTranscriptionBytes = 48 * 1_024
    private static let chunkDurationSeconds: Float = 30
    private static let maximumTokensPerChunk = 4_096
    private static let maximumTokensPerAudio = 131_072

    private let modelsRoot: URL
    private let audioRoot: URL
    private var loadedModel: LoadedModel?
    private var loadedAttestation: LoadedAttestation?

    init(modelsRootPath: String, audioRootPath: String) {
        self.modelsRoot = URL(fileURLWithPath: modelsRootPath, isDirectory: true)
            .standardizedFileURL.resolvingSymlinksInPath()
        self.audioRoot = URL(fileURLWithPath: audioRootPath, isDirectory: true)
            .standardizedFileURL.resolvingSymlinksInPath()
    }

    func releaseForShutdown() async {
        releaseModel()
    }

    func runSynchronousWorker<Result: Sendable>(
        cancellation: ASRRequestCancellation,
        synchronize: @Sendable () -> Void = {
            Stream.defaultStream(.gpu).synchronize()
            Stream.defaultStream(.cpu).synchronize()
        },
        operation: () throws -> Result
    ) throws -> Result {
        try runASRSynchronousWorker(
            cancellation: cancellation,
            synchronize: synchronize,
            operation: operation
        )
    }

    func handleControl(_ request: SidecarRequest) async -> SidecarResponse {
        guard validSidecarIdentifier(request.requestId),
              request.sessionId.map(validSidecarIdentifier) ?? true else {
            return response(request, status: "error", failure: .invalidRequest)
        }
        guard request.protocolVersion == sidecarProtocolVersion else {
            return response(request, status: "error", failure: .invalidRequest)
        }
        guard Self.platformSupported else {
            return response(request, status: "unavailable", failure: .unsupportedPlatform)
        }

        switch request.op {
        case "hello", "status":
            return response(request, status: "ready")
        case "load":
            return await load(request)
        case "unload":
            releaseModel()
            return response(request, status: "unloaded")
        case "transcribe":
            return response(request, status: "error", failure: .invalidRequest)
        case "cancel":
            return response(request, status: "error", failure: .invalidRequest)
        case "quit":
            releaseModel()
            return response(request, status: "completed")
        default:
            return response(request, status: "error", failure: .invalidRequest)
        }
    }

    private func load(_ request: SidecarRequest) async -> SidecarResponse {
        guard let modelName = request.modelId,
              let model = ModelID(rawValue: modelName) else {
            return response(request, status: "error", failure: .invalidModel)
        }
        guard request.modelVersion == model.revision else {
            return response(request, status: "error", failure: .versionMismatch)
        }
        guard let modelDirectoryPath = request.modelDirectory,
              let modelDirectory = validatedModelDirectory(modelDirectoryPath, for: model) else {
            return response(request, status: "error", failure: .modelUnavailable)
        }

        releaseModel()
        do {
            switch model {
            case .qwen06, .qwen17:
                try regenerateQwenTokenizerJSON(in: modelDirectory)
                let loaded = try await Qwen3ASRModel.fromModelDirectory(modelDirectory)
                loadedModel = .qwen(model, loaded)
            case .cohere2:
                let loaded = try CohereTranscribeModel.fromDirectory(modelDirectory)
                loadedModel = .cohere(model, loaded)
            }
            do {
                loadedAttestation = try attestLoadedModel()
            } catch let failure as AttestationFailure {
                loadedAttestation = nil
                SidecarIO.writeDiagnostic(code: failure.diagnosticCode)
            } catch {
                loadedAttestation = nil
                SidecarIO.writeDiagnostic(code: "model_attestation_unclassified_failed")
            }
            return response(request, status: "loaded")
        } catch {
            releaseModel()
            SidecarIO.writeDiagnostic(code: "model_load_failed")
            return response(request, status: "error", failure: .modelLoadFailed)
        }
    }

    func transcribe(
        _ request: SidecarRequest,
        cancellation: ASRRequestCancellation
    ) async -> SidecarResponse {
        let transcriptionActivity = ProcessInfo.processInfo.beginActivity(
            options: [.userInitiated],
            reason: "Foreground local dictation transcription"
        )
        defer { ProcessInfo.processInfo.endActivity(transcriptionActivity) }
        do {
            return try runSynchronousWorker(cancellation: cancellation) {
                let device = Device.defaultDevice()
                return try Device.withDefaultDevice(device) {
                    try transcribeValidated(request, cancellation: cancellation)
                }
            }
        } catch ASRCancellationEffectError.cancelled {
            Memory.clearCache()
            return response(request, status: "cancelled")
        } catch let failure as RequestFailure {
            if cancellation.isCancellationRequested {
                Memory.clearCache()
                return response(request, status: "cancelled")
            }
            SidecarIO.writeDiagnostic(code: failure.response.code)
            return response(request, status: "error", failure: failure)
        } catch {
            if cancellation.isCancellationRequested {
                Memory.clearCache()
                return response(request, status: "cancelled")
            }
            SidecarIO.writeDiagnostic(code: "generation_failed")
            return response(request, status: "error", failure: .generationFailed)
        }
    }

    private func transcribeValidated(
        _ request: SidecarRequest,
        cancellation: ASRRequestCancellation
    ) throws -> SidecarResponse {
        guard validSidecarIdentifier(request.requestId),
              request.sessionId.map(validSidecarIdentifier) ?? true else {
            return response(request, status: "error", failure: .invalidRequest)
        }
        guard request.protocolVersion == sidecarProtocolVersion else {
            return response(request, status: "error", failure: .invalidRequest)
        }
        guard Self.platformSupported else {
            return response(request, status: "unavailable", failure: .unsupportedPlatform)
        }
        guard let model = loadedModel,
              request.modelId == model.id.rawValue,
              request.modelVersion == model.id.revision else {
            return response(request, status: "error", failure: .modelNotLoaded)
        }
        guard let rawAudioPath = request.audioPath,
              let maximumBytes = request.audioMaxBytes,
              maximumBytes >= 44,
              maximumBytes <= Int(PrivateAudioFileReader.maximumFileBytes),
              let requestedPerChunk = request.maxTokensPerChunk,
              requestedPerChunk > 0,
              requestedPerChunk <= Self.maximumTokensPerChunk,
              let requestedTotal = request.maxTokensTotal,
              requestedTotal > 0,
              requestedTotal <= Self.maximumTokensPerAudio,
              audioRoot.path == modelsRoot.appendingPathComponent("audio-tmp", isDirectory: true).standardizedFileURL.path else {
            return response(request, status: "error", failure: .invalidRequest)
        }

        let language: String?
        do {
            language = try validatedLanguage(request.language, for: model.id)
        } catch {
            return response(request, status: "error", failure: .unsupportedLanguage)
        }

        let wav: ValidatedWAV
        do {
            try cancellation.check()
            wav = try PrivateAudioFileReader.readValidatedWAV(
                at: rawAudioPath,
                audioRoot: audioRoot,
                maximumBytes: maximumBytes
            )
        } catch WAVValidationError.tooLarge {
            return response(request, status: "error", failure: .audioTooLarge)
        } catch WAVValidationError.tooLong {
            return response(request, status: "error", failure: .audioTooLong)
        } catch {
            return response(request, status: "error", failure: .audioInvalid)
        }
        try cancellation.check()

        let result = try generate(
            wav,
            language: language,
            maxTokensPerChunk: requestedPerChunk,
            maxTokensTotal: requestedTotal,
            with: model,
            cancellation: cancellation
        )
        try cancellation.check()
        let completed = SidecarResponse(
            request: request,
            status: "completed",
            platformSupported: Self.platformSupported,
            loadedModelId: model.id.rawValue,
            loadedModelVersion: model.id.revision,
            loadedAttestation: loadedAttestation,
            result: result
        )
        guard (try? SidecarIO.encodedResponsePayload(
            completed,
            maximumBytes: Self.maximumResponseBytes
        )) != nil else {
            return response(request, status: "error", failure: .responseTooLarge)
        }
        return completed
    }

    private func generate(
        _ wav: ValidatedWAV,
        language: String?,
        maxTokensPerChunk: Int,
        maxTokensTotal: Int,
        with loaded: LoadedModel,
        cancellation: ASRCancellableEffect
    ) throws -> TranscriptionResult {
        let audio = MLXArray(wav.samples)
        let chunks = try splitAudioIntoChunks(
            audio,
            sampleRate: PrivateAudioFileReader.sampleRate,
            chunkDuration: Self.chunkDurationSeconds,
            minChunkDuration: 1.0,
            checkCancellation: { try cancellation.check() }
        )
        guard !chunks.isEmpty else { throw RequestFailure.generationFailed }

        var textParts: [String] = []
        var detectedLanguages: [String] = []
        var generationTokens = 0
        var coveredSamples = 0
        var chunksCompleted = 0
        var budgetExhausted = false
        let chunksTotal = chunks.count

        // `splitAudioIntoChunks` emits ordered contiguous integer slices, with
        // only a short final slice padded to its minimum input length. Count
        // original samples cumulatively and clip that padding off here.
        for (chunkAudio, _) in chunks {
            try cancellation.check()
            guard generationTokens < maxTokensTotal else {
                budgetExhausted = true
                break
            }
            let tokenLimit = min(
                maxTokensPerChunk,
                maxTokensTotal - generationTokens
            )
            guard tokenLimit > 0 else {
                budgetExhausted = true
                break
            }

            let output: GeneratedChunk
            switch loaded {
            case .qwen(_, let model):
                try cancellation.check()
                let result = try model.generateCancellable(
                    audio: chunkAudio,
                    maxTokens: tokenLimit,
                    temperature: 0,
                    context: "",
                    language: language,
                    chunkDuration: 1_200,
                    minChunkDuration: 1,
                    repetitionPenalty: 1.05,
                    repetitionContextSize: 32,
                    checkCancellation: { try cancellation.check() }
                )
                try cancellation.check()
                output = GeneratedChunk(
                    text: result.text,
                    language: language == nil ? result.language : nil,
                    generationTokens: result.generationTokens,
                    mayHaveReachedGenerationLimit: false
                )
            case .cohere(_, let model):
                guard let language else { throw RequestFailure.unsupportedLanguage }
                let parameters = STTGenerateParameters(
                    maxTokens: tokenLimit,
                    temperature: 0,
                    topP: 1,
                    topK: 0,
                    verbose: false,
                    language: language,
                    chunkDuration: 1_200,
                    minChunkDuration: 1
                )
                try cancellation.check()
                let result = try model.generate(
                    audio: chunkAudio,
                    generationParameters: parameters,
                    checkCancellation: { try cancellation.check() }
                )
                try cancellation.check()
                let effectiveTokenLimit = min(
                    tokenLimit,
                    max(0, model.config.decoder.maxSequenceLength - result.promptTokens)
                )
                output = GeneratedChunk(
                    text: result.text,
                    language: nil,
                    generationTokens: result.generationTokens,
                    // The pinned Cohere API strips a trailing EOS token and has no
                    // finish reason. Treat the final allowed token as ambiguous.
                    mayHaveReachedGenerationLimit: effectiveTokenLimit == 0
                        || result.generationTokens >= max(0, effectiveTokenLimit - 1)
                )
            }

            generationTokens += output.generationTokens
            let effectiveChunkSamples = min(chunkAudio.dim(0), wav.sampleCount - coveredSamples)
            guard effectiveChunkSamples > 0 else { throw RequestFailure.generationFailed }
            coveredSamples += effectiveChunkSamples

            if output.generationTokens >= tokenLimit || output.mayHaveReachedGenerationLimit {
                budgetExhausted = true
                break
            }
            chunksCompleted += 1

            let trimmedText = output.text.trimmingCharacters(in: .whitespacesAndNewlines)
            if !trimmedText.isEmpty {
                textParts.append(trimmedText)
                if textParts.reduce(0, { $0 + $1.utf8.count }) > Self.maximumTranscriptionBytes {
                    throw RequestFailure.responseTooLarge
                }
            }
            if let detected = output.language?.trimmingCharacters(in: .whitespacesAndNewlines),
               !detected.isEmpty,
               !detectedLanguages.contains(detected) {
                detectedLanguages.append(detected)
            }
            Memory.clearCache()
        }

        if chunksCompleted != chunksTotal || coveredSamples != wav.sampleCount {
            budgetExhausted = true
        }

        let separator = joinedTextSeparator(for: loaded.id, language: language, detectedLanguages: detectedLanguages)
        // Do not expose a partial transcript as if it were a complete result.
        let text = budgetExhausted ? "" : textParts.joined(separator: separator)
        guard text.utf8.count <= Self.maximumTranscriptionBytes else {
            throw RequestFailure.responseTooLarge
        }

        let detectedLanguage = loaded.id == .cohere2 || language != nil || detectedLanguages.count != 1
            ? nil
            : detectedLanguages.first
        return TranscriptionResult(
            text: text,
            language: detectedLanguage,
            generationTokens: generationTokens,
            coveredSamples: coveredSamples,
            budgetExhausted: budgetExhausted,
            chunksCompleted: chunksCompleted,
            chunksTotal: chunksTotal
        )
    }

    private func joinedTextSeparator(for model: ModelID, language: String?, detectedLanguages: [String]) -> String {
        if language == "zh" || (language == nil && detectedLanguages == ["Chinese"]) {
            return ""
        }
        if model == .cohere2 && language == "zh" {
            return ""
        }
        return " "
    }

    private func validatedLanguage(_ raw: String?, for model: ModelID) throws -> String? {
        let normalized = raw?.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if model == .cohere2 {
            guard let normalized, normalized == "zh" || normalized == "en" else {
                throw RequestFailure.unsupportedLanguage
            }
            return normalized
        }
        guard normalized == nil || normalized == "auto" || normalized == "zh" || normalized == "en" else {
            throw RequestFailure.unsupportedLanguage
        }
        return normalized == nil || normalized == "auto" ? nil : normalized
    }

    private func validatedModelDirectory(_ rawPath: String, for model: ModelID) -> URL? {
        guard validDirectory(modelsRoot) else { return nil }
        guard rawPath.utf8.count <= 4_096, rawPath.hasPrefix("/") else { return nil }
        let directory = URL(fileURLWithPath: rawPath, isDirectory: true).standardizedFileURL
        let expectedDirectory = modelsRoot.appendingPathComponent(model.rawValue, isDirectory: true)
            .standardizedFileURL
        guard directory.path == expectedDirectory.path else { return nil }
        guard directory.deletingLastPathComponent().path == modelsRoot.path,
              validDirectory(directory),
              directory.resolvingSymlinksInPath().path == directory.path,
              containsNoLinksOutsideModelDirectory(directory),
              containsOnlyPinnedWeightFiles(directory) else {
            return nil
        }
        return directory
    }

    private func containsOnlyPinnedWeightFiles(_ directory: URL) -> Bool {
        var enumerationFailed = false
        guard let enumerator = FileManager.default.enumerator(
            at: directory,
            includingPropertiesForKeys: nil,
            options: [],
            errorHandler: { _, _ in
                enumerationFailed = true
                return false
            }
        ) else { return false }

        var foundPinnedWeights = false
        let directoryPath = directory.standardizedFileURL.path
        let expectedWeights = directory.appendingPathComponent("model.safetensors")
            .standardizedFileURL.path
        while let entry = enumerator.nextObject() as? URL {
            let entryPath = entry.standardizedFileURL.path
            guard entryPath.hasPrefix(directoryPath + "/") else { return false }
            guard entry.pathExtension.lowercased() == "safetensors" else { continue }
            guard entryPath == expectedWeights else { return false }
            var info = stat()
            guard lstat(entry.path, &info) == 0,
                  (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
                  info.st_uid == geteuid(),
                  info.st_size > 0 else {
                return false
            }
            foundPinnedWeights = true
        }
        return !enumerationFailed && foundPinnedWeights
    }

    private func regenerateQwenTokenizerJSON(in directory: URL) throws {
        let path = directory.appendingPathComponent("tokenizer.json", isDirectory: false).path
        var info = stat()
        if lstat(path, &info) == 0 {
            guard (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
                  info.st_uid == geteuid(),
                  unlink(path) == 0 else {
                throw RequestFailure.modelLoadFailed
            }
        } else if errno != ENOENT {
            throw RequestFailure.modelLoadFailed
        }
        // The pinned upstream loader deterministically regenerates this file
        // from the Rust-verified vocab.json, merges.txt, and tokenizer_config.json.
    }

    private func containsNoLinksOutsideModelDirectory(_ directory: URL) -> Bool {
        let directoryPath = directory.standardizedFileURL.path
        guard let enumerator = FileManager.default.enumerator(
            at: directory,
            includingPropertiesForKeys: nil,
            options: [],
            errorHandler: { _, _ in false }
        ) else {
            return false
        }
        while let entry = enumerator.nextObject() as? URL {
            let entryPath = entry.standardizedFileURL.path
            var info = stat()
            guard entryPath.hasPrefix(directoryPath + "/"),
                  lstat(entry.path, &info) == 0 else {
                return false
            }
            let kind = info.st_mode & mode_t(S_IFMT)
            guard kind == mode_t(S_IFDIR) || kind == mode_t(S_IFREG) else {
                return false
            }
        }
        return true
    }

    private func validDirectory(_ directory: URL) -> Bool {
        var info = stat()
        return lstat(directory.path, &info) == 0 && (info.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR)
    }

    private func attestLoadedModel() throws -> LoadedAttestation {
        guard let loadedModel else { throw AttestationFailure.parameters }
        let parameters: [(String, MLXArray)]
        switch loadedModel {
        case .qwen(_, let model): parameters = model.parameters().flattened()
        case .cohere(_, let model): parameters = model.parameters().flattened()
        }
        guard !parameters.isEmpty, parameters.count <= 100_000 else {
            throw AttestationFailure.parameters
        }
        let executable: URL
        do {
            executable = try SidecarExecutableIdentity.currentExecutableURL()
        } catch {
            throw AttestationFailure.executableIdentity
        }
        var dtypes: [String: Int] = [:]
        var layout: [[String]] = []
        for (name, array) in parameters.sorted(by: { $0.0 < $1.0 }) {
            let dtype = String(describing: array.dtype)
            dtypes[dtype, default: 0] += 1
            layout.append([name, dtype, array.shape.map { String($0) }.joined(separator: ",")])
        }
        let layoutData: Data
        do {
            layoutData = try JSONSerialization.data(withJSONObject: layout, options: [.sortedKeys])
        } catch {
            throw AttestationFailure.parameterLayout
        }
        let metal: [String: String]
        do {
            metal = try SidecarExecutableIdentity.metalDigests(for: executable)
        } catch {
            throw AttestationFailure.metalResources
        }
        // Dyld's loaded image UUIDs describe the actual mapped dependency set,
        // including shared-cache libraries for which an on-disk hash is unavailable.
        let images: [String]
        do {
            images = try loadedImageUUIDs()
        } catch let failure as LoadedImageInventoryError {
            throw AttestationFailure.loadedImages(failure.diagnosticCode)
        } catch {
            throw AttestationFailure.loadedImages("model_attestation_images_enumeration_failed")
        }
        let executableHash: String
        do {
            executableHash = try SidecarExecutableIdentity.digestRegularFile(at: executable)
        } catch {
            throw AttestationFailure.executableHash
        }
        return LoadedAttestation(
            schemaVersion: 1,
            executableSHA256: executableHash,
            metalSHA256: metal,
            loadedImages: images,
            parameterLayoutSHA256: SHA256.hash(data: layoutData).map { String(format: "%02x", $0) }.joined(),
            parameterCount: parameters.count, parameterDtypes: dtypes,
            decoderRecipe: "voiceflow-mlx-decoder-v1:chunks30s/min1s;temperature0;context-empty;qwen-repetition1.05/window32;inner-chunk1200s;cohere-topP1/topK0/vad-nil;ordered-trim-language-join"
        )
    }

    private func loadedImageUUIDs() throws -> [String] {
        let imageCount = Int(_dyld_image_count())
        try LoadedImageInventory.validateImageCount(imageCount)

        var images: [String] = []
        images.reserveCapacity(imageCount)
        var totalUTF8Bytes = 0
        for index in 0..<imageCount {
            let dyldIndex = UInt32(index)
            guard let header = _dyld_get_image_header(dyldIndex) else {
                throw LoadedImageInventoryError.imageHeaderUnavailable(index: index)
            }
            guard let name = _dyld_get_image_name(dyldIndex) else {
                throw LoadedImageInventoryError.imageNameUnavailable(index: index)
            }
            guard header.pointee.magic == MH_MAGIC_64 else {
                throw LoadedImageInventoryError.invalidImageHeader(index: index)
            }
            let nameBytes = strnlen(name, LoadedImageInventory.maximumEntryUTF8Bytes + 1)
            guard nameBytes <= LoadedImageInventory.maximumEntryUTF8Bytes else {
                throw LoadedImageInventoryError.imageNameTooLong(index: index, bytes: Int(nameBytes))
            }

            let wide = UnsafeRawPointer(header).assumingMemoryBound(to: mach_header_64.self).pointee
            var cursor = UnsafeRawPointer(header).advanced(by: MemoryLayout<mach_header_64>.size)
            var remaining = Int(wide.sizeofcmds)
            var uuid: String?
            for _ in 0..<wide.ncmds {
                guard remaining >= MemoryLayout<load_command>.size else {
                    throw LoadedImageInventoryError.malformedLoadCommands(index: index)
                }
                let command = cursor.load(as: load_command.self)
                guard Int(command.cmdsize) >= MemoryLayout<load_command>.size,
                      Int(command.cmdsize) <= remaining else {
                    throw LoadedImageInventoryError.malformedLoadCommands(index: index)
                }
                if command.cmd == LC_UUID {
                    guard Int(command.cmdsize) >= MemoryLayout<uuid_command>.size else {
                        throw LoadedImageInventoryError.malformedLoadCommands(index: index)
                    }
                    uuid = UUID(uuid: cursor.load(as: uuid_command.self).uuid).uuidString
                }
                cursor = cursor.advanced(by: Int(command.cmdsize))
                remaining -= Int(command.cmdsize)
            }
            guard let uuid else {
                throw LoadedImageInventoryError.missingUUID(index: index)
            }
            let image = String(cString: name) + "|" + uuid
            try LoadedImageInventory.append(
                image,
                index: index,
                to: &images,
                totalUTF8Bytes: &totalUTF8Bytes
            )
        }
        return images.sorted()
    }

    private func releaseModel() {
        loadedModel = nil
        loadedAttestation = nil
        Memory.clearCache()
    }

    private func response(
        _ request: SidecarRequest,
        status: String,
        failure: RequestFailure? = nil
    ) -> SidecarResponse {
        SidecarResponse(
            request: request,
            status: status,
            platformSupported: Self.platformSupported,
            loadedModelId: loadedModel?.id.rawValue,
            loadedModelVersion: loadedModel?.id.revision,
            loadedAttestation: loadedAttestation,
            error: failure?.response
        )
    }

    static var platformSupported: Bool {
        #if arch(arm64)
        let version = ProcessInfo.processInfo.operatingSystemVersion
        return version.majorVersion >= 14
        #else
        return false
        #endif
    }
}
