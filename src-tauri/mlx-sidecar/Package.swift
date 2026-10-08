// swift-tools-version: 6.2
import PackageDescription

let package = Package(
    name: "VoiceFlowMLXSidecar",
    platforms: [.macOS(.v14)],
    products: [
        .library(name: "VoiceFlowMLXSidecarCore", targets: ["VoiceFlowMLXSidecarCore"]),
        .executable(name: "voiceflow-mlx-sidecar", targets: ["VoiceFlowMLXSidecar"]),
    ],
    dependencies: [
        .package(
            url: "https://github.com/Blaizzy/mlx-audio-swift.git",
            revision: "01dec7c9bdce3088a6b6b7ab9f2e403458195efb"
        ),
        .package(
            url: "https://github.com/ml-explore/mlx-swift.git",
            exact: "0.31.4"
        ),
    ],
    targets: [
        .target(
            name: "VoiceFlowMLXSidecarCore",
            dependencies: [
                .product(name: "MLXAudioSTT", package: "mlx-audio-swift"),
                .product(name: "MLX", package: "mlx-swift"),
            ],
            path: "Sources/VoiceFlowMLXSidecarCore"
        ),
        .executableTarget(
            name: "VoiceFlowMLXSidecar",
            dependencies: ["VoiceFlowMLXSidecarCore"],
            path: "Sources/VoiceFlowMLXSidecar"
        ),
    ]
)
