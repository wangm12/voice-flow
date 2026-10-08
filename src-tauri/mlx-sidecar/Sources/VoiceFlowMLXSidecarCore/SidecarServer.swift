import Darwin
import Foundation
import MLX
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

private struct GeneratedChunk {
    let text: String
    let language: String?
    let generationTokens: Int
    let mayHaveReachedGenerationLimit: Bool
}

public final class SidecarServer {
    private static let maximumInputBytes = 1_048_576
    private static let maximumResponseBytes = 65_535
    private static let maximumTranscriptionBytes = 48 * 1_024
    private static let chunkDurationSeconds: Float = 30
    private static let maximumTokensPerChunk = 4_096
    private static let maximumTokensPerAudio = 131_072

    private let modelsRoot: URL
    private let audioRoot: URL
    private var loadedModel: LoadedModel?
    private var shouldQuit = false

    public init(modelsRootPath: String, audioRootPath: String) {
        self.modelsRoot = URL(fileURLWithPath: modelsRootPath, isDirectory: true)
            .standardizedFileURL.resolvingSymlinksInPath()
        self.audioRoot = URL(fileURLWithPath: audioRootPath, isDirectory: true)
            .standardizedFileURL.resolvingSymlinksInPath()
    }

    public func run() async {
        let input = JSONLineReader(maximumLineBytes: Self.maximumInputBytes)
        while !shouldQuit {
            do {
                guard let line = try input.nextLine() else { break }
                let request: SidecarRequest
                do {
                    request = try SidecarProtocolCodec.decodeRequest(line)
                } catch {
                    SidecarIO.writeResponse(
                        Self.malformedRequestResponse(),
                        maximumBytes: Self.maximumResponseBytes
                    )
                    continue
                }

                let response = await handle(request)
                SidecarIO.writeResponse(response, maximumBytes: Self.maximumResponseBytes)
            } catch JSONLineReaderError.lineTooLarge {
                SidecarIO.writeResponse(
                    Self.malformedRequestResponse(),
                    maximumBytes: Self.maximumResponseBytes
                )
                break
            } catch {
                SidecarIO.writeDiagnostic(code: "protocol_io_failed")
                break
            }
        }
    }

    private func handle(_ request: SidecarRequest) async -> SidecarResponse {
        guard validIdentifier(request.requestId),
              request.sessionId.map(validIdentifier) ?? true else {
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
            return transcribe(request)
        case "cancel":
            // Active MLX generation is synchronous. The caller terminates and
            // restarts this child to cancel an in-flight request.
            return response(request, status: "cancelled")
        case "quit":
            releaseModel()
            shouldQuit = true
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
            return response(request, status: "loaded")
        } catch {
            releaseModel()
            SidecarIO.writeDiagnostic(code: "model_load_failed")
            return response(request, status: "error", failure: .modelLoadFailed)
        }
    }

    private func transcribe(_ request: SidecarRequest) -> SidecarResponse {
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

        do {
            let result = try generate(
                wav,
                language: language,
                maxTokensPerChunk: requestedPerChunk,
                maxTokensTotal: requestedTotal,
                with: model
            )
            let completed = SidecarResponse(
                request: request,
                status: "completed",
                platformSupported: Self.platformSupported,
                loadedModelId: model.id.rawValue,
                loadedModelVersion: model.id.revision,
                result: result
            )
            guard let encoded = try? SidecarProtocolCodec.encodeResponse(completed),
                  encoded.count <= Self.maximumResponseBytes else {
                return response(request, status: "error", failure: .responseTooLarge)
            }
            return completed
        } catch let failure as RequestFailure {
            SidecarIO.writeDiagnostic(code: failure.response.code)
            return response(request, status: "error", failure: failure)
        } catch {
            SidecarIO.writeDiagnostic(code: "generation_failed")
            return response(request, status: "error", failure: .generationFailed)
        }
    }

    private func generate(
        _ wav: ValidatedWAV,
        language: String?,
        maxTokensPerChunk: Int,
        maxTokensTotal: Int,
        with loaded: LoadedModel
    ) throws -> TranscriptionResult {
        let audio = MLXArray(wav.samples)
        let chunks = splitAudioIntoChunks(
            audio,
            sampleRate: PrivateAudioFileReader.sampleRate,
            chunkDuration: Self.chunkDurationSeconds,
            minChunkDuration: 1.0
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
                let result = model.generate(
                    audio: chunkAudio,
                    maxTokens: tokenLimit,
                    temperature: 0,
                    context: "",
                    language: language,
                    chunkDuration: 1_200,
                    minChunkDuration: 1,
                    repetitionPenalty: 1.05,
                    repetitionContextSize: 32
                )
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
                let result = model.generate(audio: chunkAudio, generationParameters: parameters, vad: nil)
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
        let expectedWeights = directory.appendingPathComponent("model.safetensors").path
        while let entry = enumerator.nextObject() as? URL {
            guard entry.path.hasPrefix(directory.path + "/") else { return false }
            guard entry.pathExtension.lowercased() == "safetensors" else { continue }
            guard entry.path == expectedWeights else { return false }
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
        guard let enumerator = FileManager.default.enumerator(
            at: directory,
            includingPropertiesForKeys: nil,
            options: [],
            errorHandler: { _, _ in false }
        ) else {
            return false
        }
        while let entry = enumerator.nextObject() as? URL {
            var info = stat()
            guard entry.path.hasPrefix(directory.path + "/"),
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

    private func releaseModel() {
        loadedModel = nil
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
            error: failure?.response
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
            error: RequestFailure.invalidRequest.response
        )
    }

    private func validIdentifier(_ value: String) -> Bool {
        !value.isEmpty && value.utf8.count <= 128 && value.utf8.allSatisfy {
            ($0 >= 48 && $0 <= 57) || ($0 >= 65 && $0 <= 90) || ($0 >= 97 && $0 <= 122)
                || $0 == 45 || $0 == 46 || $0 == 58 || $0 == 95
        }
    }

    private static var platformSupported: Bool {
        #if arch(arm64)
        let version = ProcessInfo.processInfo.operatingSystemVersion
        return version.majorVersion >= 14
        #else
        return false
        #endif
    }
}
