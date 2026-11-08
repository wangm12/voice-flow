import Foundation
import MachO
import XCTest

@testable import VoiceFlowMLXSidecarCore

final class SidecarExecutableIdentityTests: XCTestCase {
    func testCurrentExecutableURLMatchesLoadedMainImagePath() throws {
        guard let mainImageName = _dyld_get_image_name(0) else {
            XCTFail("The test process main image path should be available.")
            return
        }
        let expected = URL(fileURLWithPath: String(cString: mainImageName))
            .standardizedFileURL
            .resolvingSymlinksInPath()

        XCTAssertEqual(try SidecarExecutableIdentity.currentExecutableURL().path, expected.path)
    }

    func testPackagedHelperResolvesMetalResourcesBesideHelperExecutable() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let appExecutable = root.appendingPathComponent("Contents/MacOS/voiceflow")
        let helperExecutable = root.appendingPathComponent("Contents/Resources/voiceflow-mlx-sidecar")
        try writeFixture(at: appExecutable, contents: "VOICEFLOW_APP_BINARY_DECOY_V1")
        try writeFixture(at: helperExecutable, contents: "VOICEFLOW_HELPER_BINARY_FIXTURE_V1")

        let helperURL = try SidecarExecutableIdentity.canonicalExecutableURL(path: helperExecutable.path)
        let helperResourceRoot = try SidecarExecutableIdentity.resourceRoot(for: helperURL)
        XCTAssertEqual(helperResourceRoot.path, helperExecutable.deletingLastPathComponent().path)
        XCTAssertNotEqual(helperResourceRoot.path, appExecutable.deletingLastPathComponent().path)

        let helperMetalOne = root.appendingPathComponent("Contents/Resources/mlx.metallib")
        let helperMetalTwo = root.appendingPathComponent(
            "Contents/Resources/mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib"
        )
        try writeFixture(at: helperMetalOne, contents: "HELPER_METAL_RESOURCE_FIXTURE_ONE")
        try writeFixture(at: helperMetalTwo, contents: "HELPER_METAL_RESOURCE_FIXTURE_TWO")

        // Distinct files at the former Bundle.main root must not be selected.
        try writeFixture(
            at: root.appendingPathComponent("Contents/MacOS/mlx.metallib"),
            contents: "APP_MACOS_METAL_DECOY_ONE"
        )
        try writeFixture(
            at: root.appendingPathComponent(
                "Contents/MacOS/mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib"
            ),
            contents: "APP_MACOS_METAL_DECOY_TWO"
        )

        XCTAssertEqual(
            try SidecarExecutableIdentity.digestRegularFile(at: helperURL),
            "2b8eb2f77b23e9d8b54fca94bcc645a42be1a602a241a49d86d5b3603140f775"
        )
        XCTAssertEqual(
            try SidecarExecutableIdentity.metalDigests(for: helperURL),
            [
                "mlx.metallib": "06cb8b59fa915d8415b6cfe5073cd554c8f2120ec75b3d229d6b6881e279b673",
                "mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib":
                    "71f96aaa01c18b93943749adb172fd5be57e41b8c0685aafcd669c0115d053bc",
            ]
        )
    }

    func testMacOSMetalDecoysCannotSatisfyMissingHelperResources() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let helperExecutable = root.appendingPathComponent("Contents/Resources/voiceflow-mlx-sidecar")
        try writeFixture(at: helperExecutable, contents: "VOICEFLOW_HELPER_BINARY_FIXTURE_V1")
        try writeFixture(
            at: root.appendingPathComponent("Contents/MacOS/mlx.metallib"),
            contents: "APP_MACOS_METAL_DECOY_ONE"
        )
        try writeFixture(
            at: root.appendingPathComponent(
                "Contents/MacOS/mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib"
            ),
            contents: "APP_MACOS_METAL_DECOY_TWO"
        )

        let helperURL = try SidecarExecutableIdentity.canonicalExecutableURL(path: helperExecutable.path)
        XCTAssertThrowsError(try SidecarExecutableIdentity.metalDigests(for: helperURL)) { error in
            XCTAssertEqual(error as? SidecarExecutableIdentityError, .some(.invalidArtifact))
        }
    }

    func testStandaloneHelperUsesItsSiblingMetalResources() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let helperExecutable = root.appendingPathComponent(".build/release/voiceflow-mlx-sidecar")
        try writeFixture(at: helperExecutable, contents: "VOICEFLOW_HELPER_BINARY_FIXTURE_V1")
        let helperURL = try SidecarExecutableIdentity.canonicalExecutableURL(path: helperExecutable.path)
        let resourceRoot = try SidecarExecutableIdentity.resourceRoot(for: helperURL)
        XCTAssertEqual(resourceRoot.path, helperExecutable.deletingLastPathComponent().path)

        try writeFixture(
            at: root.appendingPathComponent(".build/release/mlx.metallib"),
            contents: "HELPER_METAL_RESOURCE_FIXTURE_ONE"
        )
        try writeFixture(
            at: root.appendingPathComponent(
                ".build/release/mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib"
            ),
            contents: "HELPER_METAL_RESOURCE_FIXTURE_TWO"
        )
        XCTAssertEqual(
            try SidecarExecutableIdentity.metalDigests(for: helperURL),
            [
                "mlx.metallib": "06cb8b59fa915d8415b6cfe5073cd554c8f2120ec75b3d229d6b6881e279b673",
                "mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib":
                    "71f96aaa01c18b93943749adb172fd5be57e41b8c0685aafcd669c0115d053bc",
            ]
        )
    }

    func testExecutableSymlinkUsesCanonicalTargetResourceRoot() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let helperExecutable = root.appendingPathComponent("Contents/Resources/voiceflow-mlx-sidecar")
        try writeFixture(at: helperExecutable, contents: "VOICEFLOW_HELPER_BINARY_FIXTURE_V1")
        let alias = root.appendingPathComponent("helper-alias")
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: helperExecutable)

        let canonicalURL = try SidecarExecutableIdentity.canonicalExecutableURL(path: alias.path)
        XCTAssertEqual(canonicalURL.path, helperExecutable.path)
        XCTAssertEqual(
            try SidecarExecutableIdentity.resourceRoot(for: canonicalURL).path,
            helperExecutable.deletingLastPathComponent().path
        )
    }

    func testRelativePathAndMissingOrNonRegularExecutableCannotBeAttested() throws {
        XCTAssertThrowsError(
            try SidecarExecutableIdentity.canonicalExecutableURL(path: "voiceflow-mlx-sidecar")
        ) { error in
            XCTAssertEqual(error as? SidecarExecutableIdentityError, .some(.invalidExecutablePath))
        }

        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let missingExecutable = try SidecarExecutableIdentity.canonicalExecutableURL(
            path: root.appendingPathComponent("missing-sidecar").path
        )
        XCTAssertThrowsError(try SidecarExecutableIdentity.digestRegularFile(at: missingExecutable)) { error in
            XCTAssertEqual(error as? SidecarExecutableIdentityError, .some(.invalidArtifact))
        }

        let nonRegularExecutablePath = root.appendingPathComponent("not-an-executable")
        try FileManager.default.createDirectory(at: nonRegularExecutablePath, withIntermediateDirectories: true)
        let nonRegularExecutable = try SidecarExecutableIdentity.canonicalExecutableURL(
            path: nonRegularExecutablePath.path
        )
        XCTAssertThrowsError(try SidecarExecutableIdentity.digestRegularFile(at: nonRegularExecutable)) { error in
            XCTAssertEqual(error as? SidecarExecutableIdentityError, .some(.invalidArtifact))
        }
    }

    func testExecutablePathLimitRejectsOversizedInput() {
        let oversizedPath = "/" + String(repeating: "x", count: 32 * 1_024)
        XCTAssertThrowsError(
            try SidecarExecutableIdentity.canonicalExecutableURL(path: oversizedPath)
        ) { error in
            XCTAssertEqual(error as? SidecarExecutableIdentityError, .some(.invalidExecutablePath))
        }
    }

    func testMissingOrNonRegularMetalResourcesCannotBeAttested() throws {
        let root = try temporaryDirectory()
        defer { try? FileManager.default.removeItem(at: root) }

        let helperExecutable = root.appendingPathComponent("Resources/voiceflow-mlx-sidecar")
        try writeFixture(at: helperExecutable, contents: "VOICEFLOW_HELPER_BINARY_FIXTURE_V1")
        let helperURL = try SidecarExecutableIdentity.canonicalExecutableURL(path: helperExecutable.path)
        let metalURLs = try SidecarExecutableIdentity.metalResourceURLs(for: helperURL)

        XCTAssertThrowsError(try SidecarExecutableIdentity.metalDigests(for: helperURL)) { error in
            XCTAssertEqual(error as? SidecarExecutableIdentityError, .some(.invalidArtifact))
        }

        try writeFixture(at: metalURLs[0].url)
        try FileManager.default.createDirectory(at: metalURLs[1].url, withIntermediateDirectories: true)
        XCTAssertThrowsError(try SidecarExecutableIdentity.metalDigests(for: helperURL)) { error in
            XCTAssertEqual(error as? SidecarExecutableIdentityError, .some(.invalidArtifact))
        }
    }

    private func temporaryDirectory() throws -> URL {
        let directory = FileManager.default.temporaryDirectory
            .appendingPathComponent("voiceflow-sidecar-identity-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        return directory.standardizedFileURL.resolvingSymlinksInPath()
    }

    private func writeFixture(at url: URL, contents: String = "VOICEFLOW_FIXTURE_V1") throws {
        try FileManager.default.createDirectory(
            at: url.deletingLastPathComponent(),
            withIntermediateDirectories: true
        )
        try Data(contents.utf8).write(to: url, options: .atomic)
    }
}
