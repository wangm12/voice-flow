import Foundation
import XCTest

@testable import VoiceFlowMLXSidecarCore

final class CleanupResponseSizeTests: XCTestCase {
    func testOversizedCleanupResponseReturnsError() throws {
        let request = SidecarRequest(
            protocolVersion: sidecarProtocolVersion,
            requestId: "cleanup-request-1",
            sessionId: "cleanup-session-1",
            modelVersion: "cleanup-model-revision",
            op: "cleanup"
        )
        let cleanupResult = CleanupResult(
            text: String(repeating: "\u{0001}", count: 20_000),
            promptTokens: 12,
            generationTokens: 20_000,
            promptTimeSeconds: 0.1,
            generationTimeSeconds: 0.2,
            completion: "complete"
        )
        let completed = SidecarResponse(
            request: request,
            status: "completed",
            platformSupported: true,
            loadedModelId: "local-cleanup-model",
            loadedModelVersion: "cleanup-model-revision",
            cleanupResult: cleanupResult
        )
        let maximumLineBytes = SidecarIO.maximumCleanupResponseLineBytes

        let bounded = try CleanupResponseSizeGuard.responseOrSizeError(
            completed,
            maximumLineBytes: maximumLineBytes,
            platformSupported: true
        )

        XCTAssertEqual(bounded.status, "error")
        XCTAssertEqual(bounded.error?.code, "response_too_large")
        XCTAssertEqual(bounded.requestId, request.requestId)
        XCTAssertEqual(bounded.sessionId, request.sessionId)
        XCTAssertEqual(bounded.modelVersion, request.modelVersion)
        XCTAssertEqual(bounded.op, request.op)
        XCTAssertNil(bounded.cleanupResult)
        let frame = try SidecarIO.encodedResponseFrame(bounded, maximumLineBytes: maximumLineBytes)
        XCTAssertEqual(frame.last, 0x0A)
        XCTAssertLessThanOrEqual(frame.count, maximumLineBytes)
    }

    func testCleanupResponseFrameBoundaryAndNormalFieldsArePreserved() throws {
        let request = SidecarRequest(
            protocolVersion: sidecarProtocolVersion,
            requestId: "cleanup-request-2",
            sessionId: "cleanup-session-2",
            modelVersion: "cleanup-model-revision",
            op: "cleanup"
        )
        let response = SidecarResponse(
            request: request,
            status: "completed",
            platformSupported: true,
            cleanupResult: CleanupResult(
                text: "line\n\"quoted\" ✓",
                promptTokens: 12,
                generationTokens: 8,
                promptTimeSeconds: 0.1,
                generationTimeSeconds: 0.2,
                completion: "complete"
            )
        )
        let frame = try SidecarIO.encodedResponseFrame(
            response,
            maximumLineBytes: SidecarIO.maximumCleanupResponseLineBytes
        )
        let exactLimitResponse = try CleanupResponseSizeGuard.responseOrSizeError(
            response,
            maximumLineBytes: frame.count,
            platformSupported: true
        )
        let tooSmallResponse = try CleanupResponseSizeGuard.responseOrSizeError(
            response,
            maximumLineBytes: frame.count - 1,
            platformSupported: true
        )
        let encoded = try SidecarIO.encodedResponseFrame(
            exactLimitResponse,
            maximumLineBytes: frame.count
        )

        XCTAssertEqual(exactLimitResponse.status, "completed")
        XCTAssertEqual(exactLimitResponse.requestId, request.requestId)
        XCTAssertEqual(exactLimitResponse.cleanupResult?.text, "line\n\"quoted\" ✓")
        XCTAssertEqual(encoded.count, frame.count)
        XCTAssertEqual(encoded.last, 0x0A)
        XCTAssertEqual(tooSmallResponse.status, "error")
        XCTAssertEqual(tooSmallResponse.error?.code, "response_too_large")
    }

    func testProductionCleanupWireBoundaryAccepts65536AndRejects65537Bytes() throws {
        let request = SidecarRequest(
            protocolVersion: sidecarProtocolVersion,
            requestId: "cleanup-boundary-request",
            sessionId: "cleanup-boundary-session",
            modelVersion: "cleanup-model-revision",
            op: "cleanup"
        )
        let maximumLineBytes = SidecarIO.maximumCleanupResponseLineBytes
        XCTAssertEqual(maximumLineBytes, 65_536)

        func response(text: String) -> SidecarResponse {
            SidecarResponse(
                request: request,
                status: "completed",
                platformSupported: true,
                loadedModelId: "local-cleanup-model",
                loadedModelVersion: "cleanup-model-revision",
                cleanupResult: CleanupResult(
                    text: text,
                    promptTokens: 12,
                    generationTokens: 20_000,
                    promptTimeSeconds: 0.1,
                    generationTimeSeconds: 0.2,
                    completion: "complete"
                )
            )
        }

        let emptyTextFrame = try SidecarIO.encodedResponseFrame(response(text: ""), maximumLineBytes: 70_000)
        let exactResponse = response(text: String(repeating: "x", count: maximumLineBytes - emptyTextFrame.count))
        let exactFrame = try SidecarIO.encodedResponseFrame(exactResponse, maximumLineBytes: maximumLineBytes)
        XCTAssertEqual(exactFrame.count, 65_536)
        XCTAssertEqual(exactFrame.last, 0x0A)

        let boundedExact = try CleanupResponseSizeGuard.responseOrSizeError(
            exactResponse,
            maximumLineBytes: maximumLineBytes,
            platformSupported: true
        )
        XCTAssertEqual(boundedExact.status, "completed")
        XCTAssertEqual(boundedExact.requestId, request.requestId)
        XCTAssertEqual(boundedExact.sessionId, request.sessionId)
        XCTAssertEqual(boundedExact.modelVersion, request.modelVersion)
        XCTAssertEqual(boundedExact.op, request.op)

        let oversizedResponse = response(text: String(repeating: "x", count: maximumLineBytes - emptyTextFrame.count + 1))
        let oversizedFrame = try SidecarIO.encodedResponseFrame(oversizedResponse, maximumLineBytes: 70_000)
        XCTAssertEqual(oversizedFrame.count, 65_537)
        let boundedOversized = try CleanupResponseSizeGuard.responseOrSizeError(
            oversizedResponse,
            maximumLineBytes: maximumLineBytes,
            platformSupported: true
        )
        XCTAssertEqual(boundedOversized.status, "error")
        XCTAssertEqual(boundedOversized.error?.code, "response_too_large")
        XCTAssertEqual(boundedOversized.requestId, request.requestId)
        XCTAssertEqual(boundedOversized.sessionId, request.sessionId)
        XCTAssertEqual(boundedOversized.modelVersion, request.modelVersion)
        XCTAssertEqual(boundedOversized.op, request.op)
    }
}
