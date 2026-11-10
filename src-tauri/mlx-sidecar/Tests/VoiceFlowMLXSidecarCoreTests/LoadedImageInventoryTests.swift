import Foundation
import XCTest

@testable import VoiceFlowMLXSidecarCore

final class LoadedImageInventoryTests: XCTestCase {
    func testCountAndPerEntryBoundariesAreInclusive() throws {
        XCTAssertNoThrow(try LoadedImageInventory.validateImageCount(4_096))
        XCTAssertThrowsError(try LoadedImageInventory.validateImageCount(4_097)) { error in
            XCTAssertEqual(error as? LoadedImageInventoryError, .imageCountExceeded(count: 4_097))
        }

        var images: [String] = []
        var totalBytes = 0
        let exactEntry = String(repeating: "x", count: LoadedImageInventory.maximumEntryUTF8Bytes)
        try LoadedImageInventory.append(exactEntry, index: 0, to: &images, totalUTF8Bytes: &totalBytes)
        XCTAssertEqual(totalBytes, LoadedImageInventory.maximumEntryUTF8Bytes)

        let overEntry = String(repeating: "x", count: LoadedImageInventory.maximumEntryUTF8Bytes + 1)
        XCTAssertThrowsError(
            try LoadedImageInventory.append(overEntry, index: 1, to: &images, totalUTF8Bytes: &totalBytes)
        ) { error in
            XCTAssertEqual(
                error as? LoadedImageInventoryError,
                .imageEntryTooLarge(index: 1, bytes: LoadedImageInventory.maximumEntryUTF8Bytes + 1)
            )
        }
        XCTAssertEqual(images, [exactEntry])
        XCTAssertEqual(totalBytes, LoadedImageInventory.maximumEntryUTF8Bytes)
    }

    func testTotalUTF8ByteBudgetAcceptsExactLimitAndRejectsNextByte() throws {
        var images: [String] = []
        var totalBytes = 0
        for index in 0..<32 {
            let image = String(format: "%02d", index)
                + String(repeating: "x", count: LoadedImageInventory.maximumEntryUTF8Bytes - 2)
            try LoadedImageInventory.append(image, index: index, to: &images, totalUTF8Bytes: &totalBytes)
        }
        XCTAssertEqual(totalBytes, LoadedImageInventory.maximumTotalUTF8Bytes)

        XCTAssertThrowsError(
            try LoadedImageInventory.append("y", index: 32, to: &images, totalUTF8Bytes: &totalBytes)
        ) { error in
            XCTAssertEqual(
                error as? LoadedImageInventoryError,
                .totalBytesExceeded(count: 33, bytes: LoadedImageInventory.maximumTotalUTF8Bytes + 1)
            )
        }
        XCTAssertEqual(images.count, 32)
        XCTAssertEqual(totalBytes, LoadedImageInventory.maximumTotalUTF8Bytes)
    }

    func testBudgetCountsUTF8BytesRatherThanCharacters() {
        let image = String(repeating: "é", count: LoadedImageInventory.maximumEntryUTF8Bytes / 2 + 1)
        var images: [String] = []
        var totalBytes = 0
        XCTAssertThrowsError(
            try LoadedImageInventory.append(image, index: 0, to: &images, totalUTF8Bytes: &totalBytes)
        ) { error in
            XCTAssertEqual(
                error as? LoadedImageInventoryError,
                .imageEntryTooLarge(index: 0, bytes: LoadedImageInventory.maximumEntryUTF8Bytes + 2)
            )
        }
    }

    func testCompleteInventoryAboveOldLimitIsPreserved() throws {
        let images = (0..<1_145).map { index in
            "/System/Library/Frameworks/Fixture\(String(format: "%04d", index))/Versions/A/Fixture"
                + "|00000000-0000-0000-0000-\(String(format: "%012X", index))"
        }
        var collected: [String] = []
        var totalBytes = 0
        try LoadedImageInventory.validateImageCount(images.count)
        for (index, image) in images.enumerated() {
            try LoadedImageInventory.append(image, index: index, to: &collected, totalUTF8Bytes: &totalBytes)
        }

        XCTAssertEqual(collected, images)
        XCTAssertEqual(collected.count, 1_145)
        XCTAssertGreaterThan(totalBytes, 65_535)
        XCTAssertLessThanOrEqual(totalBytes, LoadedImageInventory.maximumTotalUTF8Bytes)
    }

    func testEncodedResponseAcceptsCompleteInventoryOverFormerResponseLimit() throws {
        let images = (0..<1_145).map { index in
            "/System/Library/Frameworks/Fixture\(String(format: "%04d", index))/Versions/A/Fixture"
                + "|00000000-0000-0000-0000-\(String(format: "%012X", index))"
        }
        let attestation = LoadedAttestation(
            schemaVersion: 1,
            executableSHA256: String(repeating: "a", count: 64),
            metalSHA256: [
                "mlx.metallib": String(repeating: "b", count: 64),
                "mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib": String(repeating: "c", count: 64),
            ],
            loadedImages: images,
            parameterLayoutSHA256: String(repeating: "d", count: 64),
            parameterCount: 1,
            parameterDtypes: ["float32": 1],
            decoderRecipe: "voiceflow-mlx-decoder-v1:chunks30s/min1s;temperature0;context-empty;qwen-repetition1.05/window32;inner-chunk1200s;ordered-trim-language-join"
        )
        let request = SidecarRequest(
            protocolVersion: sidecarProtocolVersion,
            requestId: "fixture",
            sessionId: "fixture-session",
            modelVersion: "fixture-revision",
            op: "load"
        )
        let response = SidecarResponse(
            request: request,
            status: "loaded",
            platformSupported: true,
            loadedModelId: "qwen3-asr-0.6b",
            loadedModelVersion: "fixture-revision",
            loadedAttestation: attestation
        )

        let encoded = try SidecarIO.encodedResponsePayload(
            response,
            maximumBytes: SidecarIO.maximumASRResponsePayloadBytes
        )
        XCTAssertGreaterThan(encoded.count, 65_535)
        XCTAssertLessThanOrEqual(encoded.count, SidecarIO.maximumASRResponsePayloadBytes)
    }

    func testEncodedEscapedResponseUsesPayloadPlusNewlineWireBoundary() throws {
        let payloadLimit = SidecarIO.maximumASRResponsePayloadBytes
        func response(text: String) -> SidecarResponse {
            let request = SidecarRequest(
                protocolVersion: sidecarProtocolVersion,
                requestId: "fixture",
                sessionId: "fixture-session",
                modelVersion: "fixture-revision",
                op: "transcribe"
            )
            let result = TranscriptionResult(
                text: text,
                language: nil,
                generationTokens: 1,
                coveredSamples: 1,
                budgetExhausted: false,
                chunksCompleted: 1,
                chunksTotal: 1
            )
            return SidecarResponse(
                request: request,
                status: "completed",
                platformSupported: true,
                result: result
            )
        }

        var lower = 0
        var upper = payloadLimit / 6 + 1
        while lower < upper {
            let middle = (lower + upper) / 2
            let encoded = try SidecarProtocolCodec.encodeResponse(
                response(text: String(repeating: "\u{0001}", count: middle))
            )
            if encoded.count <= payloadLimit {
                lower = middle + 1
            } else {
                upper = middle
            }
        }
        let controlCount = lower - 1
        let controlText = String(repeating: "\u{0001}", count: controlCount)
        let controlPayload = try SidecarProtocolCodec.encodeResponse(response(text: controlText))
        let paddingCount = payloadLimit - controlPayload.count
        let exactResponse = response(text: controlText + String(repeating: "x", count: paddingCount))
        let exactPayload = try SidecarIO.encodedResponsePayload(exactResponse, maximumBytes: payloadLimit)
        XCTAssertEqual(exactPayload.count, payloadLimit)
        var exactWireLine = exactPayload
        exactWireLine.append(0x0A)
        XCTAssertEqual(exactWireLine.count, SidecarIO.maximumASRResponseLineBytes)

        let overResponse = response(text: controlText + String(repeating: "x", count: paddingCount + 1))
        XCTAssertThrowsError(
            try SidecarIO.encodedResponsePayload(overResponse, maximumBytes: payloadLimit)
        ) { error in
            XCTAssertEqual(error as? SidecarIOError, .responseTooLarge)
        }
    }
}
