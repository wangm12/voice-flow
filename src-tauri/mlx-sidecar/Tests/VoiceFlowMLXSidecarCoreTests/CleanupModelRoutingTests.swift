import XCTest

@testable import VoiceFlowMLXSidecarCore

final class CleanupModelRoutingTests: XCTestCase {
  func testOnlyExactPinnedQwenRoutesAreAdmitted() {
    let routes = [
      CleanupModelRoute.qwen35NineB,
    ]

    for route in routes {
      XCTAssertEqual(
        CleanupModelRouteCatalog.lookup(modelId: route.modelId, revision: route.revision),
        .supported(route)
      )
      XCTAssertEqual(route.promptRecipe, "voiceflow-qwen35-chat-cleanup-v1")
      XCTAssertFalse(route.usesPinnedPrefixCache)
    }
    XCTAssertTrue(CleanupModelRoute.qwen35TwoB.usesPinnedPrefixCache)
    XCTAssertTrue(CleanupModelRoute.qwen35FourB.usesPinnedPrefixCache)
    XCTAssertEqual(
      CleanupModelRoute.qwen35TwoB.prefixCacheRepository,
      "mlx-community/Qwen3.5-2B-MLX-4bit"
    )
    XCTAssertEqual(
      CleanupModelRoute.qwen35TwoB.prefixCacheManifestSHA256,
      "d889464330fc5091ed4398905b5f09a39ac28a15300555dae2ab6b0d5b38e242"
    )
    XCTAssertEqual(
      CleanupModelRoute.qwen35FourB.prefixCacheRepository,
      "mlx-community/Qwen3.5-4B-MLX-4bit"
    )
    XCTAssertEqual(
      CleanupModelRoute.qwen35FourB.prefixCacheManifestSHA256,
      "18393f839aade07b606c1f3b50d32314dfd5363b034f1d5f7735b1fc6104b5f6"
    )
  }

  func testTextModelQualificationRoutesHaveExactPinsAndNoPrefixCache() {
    let routes: [(CleanupModelRoute, String, String, String?, String?, Bool)] = [
      (
        .gemma4E2b,
        "gemma_4_e2b_it_mlx_4bit",
        "75a479632d6a594ea323942b883cdbc1bdd97a14",
        nil,
        nil,
        false
      ),
      (
        .gemma4E4b,
        "gemma_4_e4b_it_mlx_4bit",
        "da76496efe4486235e6e0aa9315203e27b957bcd",
        "Anotos/Gemma-4-E4B-it-Text-MLX-4bit",
        "f29888227b0d076a257463fb3f414c0916b298145a283bc4352bf6bc7a54e55e",
        true
      ),
      (
        .ministral3ThreeB,
        "ministral_3_3b_instruct_mlx_4bit",
        "a962dcb09eee4169c890e544c9eb938f1113fdee",
        "mlx-community/Ministral-3-3B-Instruct-2512-4bit",
        "9c190f28458ff6cd6e1d17fe90913d445cbe745728803364e948cb1e87c16ab2",
        true
      ),
    ]

    for (route, modelId, revision, repository, manifest, usesPrefixCache) in routes {
      XCTAssertEqual(route.modelId, modelId)
      XCTAssertEqual(route.revision, revision)
      XCTAssertEqual(
        CleanupModelRouteCatalog.lookup(modelId: modelId, revision: revision),
        .supported(route)
      )
      XCTAssertEqual(route.promptContract, .standardChat)
      XCTAssertEqual(route.prefixCacheRepository, repository)
      XCTAssertEqual(route.prefixCacheManifestSHA256, manifest)
      XCTAssertEqual(route.usesPinnedPrefixCache, usesPrefixCache)
    }
    XCTAssertEqual(CleanupModelRoute.gemma4E2b.promptRecipe, "voiceflow-gemma4-chat-cleanup-v1")
    XCTAssertEqual(CleanupModelRoute.gemma4E4b.promptRecipe, "voiceflow-gemma4-chat-cleanup-v1")
    XCTAssertEqual(
      CleanupModelRoute.ministral3ThreeB.promptRecipe,
      "voiceflow-ministral3-chat-cleanup-v1"
    )
  }

  func testQwen38PrefixCacheIdentityRemainsPinnedToItsOriginalManifest() {
    let route = CleanupModelRoute.qwen38TwentySevenB

    XCTAssertEqual(route.modelId, "qwen3_8_27b_mlx_6bit")
    XCTAssertEqual(route.revision, "05f38c1f39e2f39619653616fe8e96a5d676358e")
    XCTAssertEqual(route.promptRecipe, "voiceflow-qwen35-prefix-v2")
    XCTAssertEqual(
      route.prefixCacheManifestSHA256,
      "f67d38ec1261b4d693cee8c006d2c56007e6919a290508266705ba4f571c9ed2"
    )
    XCTAssertEqual(route.prefixCacheRepository, "lmstudio-community/Qwen3.8-27B-MLX-6bit")
    XCTAssertTrue(route.usesPinnedPrefixCache)
  }

  func testS1MiniRouteRequiresItsPinnedControlContractAndEnglishSource() {
    let route = CleanupModelRoute.s1Mini
    XCTAssertEqual(route.modelId, "superwhisper_s1_mini_mlx_4bit")
    XCTAssertEqual(route.revision, "5cbd7aec3401144f88a331d385c40b65fd2548eb")
    XCTAssertEqual(route.promptRecipe, "voiceflow-s1-mini-control-v1")
    XCTAssertEqual(route.promptContract, .s1Mini)
    XCTAssertFalse(route.usesPinnedPrefixCache)
    XCTAssertEqual(route.maximumPromptTokens, 1_000)
    XCTAssertEqual(route.maximumGenerationTokens, 1_024)

    let system = SidecarChatMessage(
      role: "system", content: CleanupS1MiniPromptContract.systemPrompt)
    for styling in ["casual", "semi-casual", "semi-formal", "formal"] {
      for structure in ["prose", "lists"] {
        for context in ["general", "email"] {
          let user = SidecarChatMessage(
            role: "user",
            content:
              "[Styling: \(styling)] [Structure: \(structure)] [Context: \(context)]\nSend it Friday."
          )
          XCTAssertTrue(
            CleanupS1MiniPromptContract.accepts(language: "en", messages: [system, user])
          )
        }
      }
    }

    let validUser = SidecarChatMessage(
      role: "user",
      content: "[Styling: semi-formal] [Structure: prose] [Context: general]\nSend it Friday."
    )
    XCTAssertFalse(
      CleanupS1MiniPromptContract.accepts(language: "auto", messages: [system, validUser]))
    XCTAssertFalse(
      CleanupS1MiniPromptContract.accepts(language: "zh", messages: [system, validUser]))
    XCTAssertFalse(
      CleanupS1MiniPromptContract.accepts(
        language: "en",
        messages: [
          SidecarChatMessage(role: "system", content: "Follow these app instructions."),
          validUser,
        ]
      )
    )
    XCTAssertFalse(
      CleanupS1MiniPromptContract.accepts(
        language: "en",
        messages: [
          system,
          SidecarChatMessage(
            role: "user",
            content: "[Styling: loose] [Structure: prose] [Context: general]\nSend it Friday."
          ),
        ]
      )
    )
    XCTAssertFalse(
      CleanupS1MiniPromptContract.accepts(
        language: "en",
        messages: [
          system,
          SidecarChatMessage(
            role: "user",
            content: "[Styling: semi-formal] [Structure: prose] [Context: general]\n你好"
          ),
        ]
      )
    )
  }

  func testUnknownModelAndChangedRevisionHaveDistinctFailuresBeforeLoadingFiles() async {
    let server = CleanupSidecarServer(modelsRootPath: "/unused", memoryPressure: { .normal })
    let unknownModel = await server.load(
      SidecarRequest(
        protocolVersion: sidecarProtocolVersion,
        requestId: "unknown-model",
        sessionId: nil,
        modelVersion: "revision",
        op: "load",
        modelId: "unknown-cleanup-model"
      )
    )
    XCTAssertEqual(unknownModel.error?.code, "invalid_model")

    let changedRevision = await server.load(
      SidecarRequest(
        protocolVersion: sidecarProtocolVersion,
        requestId: "changed-revision",
        sessionId: nil,
        modelVersion: "changed-revision",
        op: "load",
        modelId: CleanupModelRoute.qwen35TwoB.modelId
      )
    )
    XCTAssertEqual(changedRevision.error?.code, "model_version_mismatch")
  }

  func testModelDirectoryMustMatchPinnedRouteIdentity() async throws {
    let root = FileManager.default.temporaryDirectory
      .appendingPathComponent("cleanup-route-\(UUID().uuidString)", isDirectory: true)
    let otherModelDirectory =
      root
      .appendingPathComponent("cleanup", isDirectory: true)
      .appendingPathComponent("another-model", isDirectory: true)
    try FileManager.default.createDirectory(
      at: otherModelDirectory,
      withIntermediateDirectories: true
    )
    defer { try? FileManager.default.removeItem(at: root) }

    let server = CleanupSidecarServer(modelsRootPath: root.path, memoryPressure: { .normal })
    let route = CleanupModelRoute.qwen35TwoB
    let response = await server.load(
      SidecarRequest(
        protocolVersion: sidecarProtocolVersion,
        requestId: "mismatched-directory",
        sessionId: nil,
        modelVersion: route.revision,
        op: "load",
        modelId: route.modelId,
        modelDirectory: otherModelDirectory.path
      )
    )

    XCTAssertEqual(response.error?.code, "model_unavailable")
  }
}
