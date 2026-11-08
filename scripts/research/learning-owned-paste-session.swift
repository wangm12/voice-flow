#if TARGET_ORACLE_ONLY
import AppKit
import ApplicationServices
import Darwin
import Foundation

private let artifactType = "voice_flow_owned_paste_session_prepared"
private let eventType = "voice_flow_owned_paste_session_event"
private let schemaVersion = 1
private let bundleIdentifier = "local.voiceflow.learning-oracle-fixture.v1"
private let runId = "90000000-0000-4000-8000-000000000011"
private let caseId = "90000000-0000-4000-8000-000000000012"
private let fixtureId = "90000000-0000-4000-8000-000000000013"
private let windowTitle = "Voice harness owned fixture 90000000-0000-4000-8000-000000000013"
private let controlIdentifier = "vf-fixture-90000000-0000-4000-8000-000000000013-target"
private let otherControlIdentifier = "vf-fixture-90000000-0000-4000-8000-000000000013-other"
private let operationFixtureId = "90000000-0000-4000-8000-000000000016"
private let termFixtureId = "90000000-0000-4000-8000-000000000017"
private let insertedText = "VoiceFlo"
private let postInsertText = "Type: VoiceFlo"
private let settledText = "Type: VoiceFlow"
private let maxPhaseMs: UInt64 = 30_000
private let maxCommandBytes = 8_192
private let maxCommands = 4

private struct PreparedPasteManifest: Decodable {
    let artifactType: String
    let schemaVersion: Int
    let cacheId: String
    let sourceManifestSha256: String
    let helperSourceSha256: String
    let buildScriptSha256: String
    let helperExecutableSha256: String
    let hostExecutableSha256: String
    let recipeSha256: String
    let hostInfoPlistSha256: String
    let insertedTextSha256: String
    let insertedTextByteLength: Int
    let expectedPostInsertSha256: String
    let expectedPostInsertByteLength: Int
    let expectedSettledSha256: String
    let expectedSettledByteLength: Int
    let hostBundleIdentifier: String
    let helperPath: String
    let hostAppPath: String
    let hostExecutablePath: String
    let recipePath: String
    let sourceManifestPath: String
    let infoPlistPath: String
}

private struct SessionState {
    var child: Process?
    var childStarted = false
    var owner: OwnedHostProcess?
    var receipt: HostReceipt?
    var receiptSha256: String?
    var editAttempted = false
    var editCompleted = false
}

private enum InboxItem {
    case line(Data)
    case overlong
    case commandOverflow
    case closed
}

private final class BoundedCommandInbox: @unchecked Sendable {
    private let lock = NSLock()
    private var items: [InboxItem] = []
    private var partial = Data()
    private var overlong = false
    private var inputClosed = false
    private var commandOverflow = false

    init() {
        DispatchQueue.global(qos: .userInitiated).async { [weak self] in
            guard let self else { return }
            let input = FileHandle.standardInput
            do {
                while let chunk = try input.read(upToCount: 4_096), !chunk.isEmpty {
                    self.append(chunk)
                }
            } catch {
                self.closeInput()
                return
            }
            self.closeInput()
        }
    }

    func take() -> InboxItem? {
        lock.lock()
        defer { lock.unlock() }
        guard !items.isEmpty else { return nil }
        return items.removeFirst()
    }

    private func append(_ bytes: Data) {
        lock.lock()
        defer { lock.unlock() }
        for byte in bytes {
            if commandOverflow {
                continue
            }
            if byte == 0x0a {
                items.append(overlong ? .overlong : .line(partial))
                partial.removeAll(keepingCapacity: true)
                overlong = false
                if items.count > maxCommands {
                    items = [.commandOverflow]
                    commandOverflow = true
                    return
                }
            } else if !overlong {
                if partial.count < maxCommandBytes {
                    partial.append(byte)
                } else {
                    partial.removeAll(keepingCapacity: false)
                    overlong = true
                }
            }
        }
    }

    private func closeInput() {
        lock.lock()
        defer { lock.unlock() }
        if !partial.isEmpty || overlong {
            items.append(overlong ? .overlong : .line(partial))
            partial.removeAll(keepingCapacity: false)
            overlong = false
        }
        if !inputClosed {
            items.append(.closed)
            inputClosed = true
        }
    }
}

@main private struct OwnedPasteSessionCLI {
    @MainActor static func main() {
        do {
            let args = try Arguments(
                ["--plan-id", "--phase-id", "--operation-id", "--session-id", "--remaining-ms"],
                switches: []
            )
            try run(args)
        } catch {
            failCLI(error)
        }
    }

    @MainActor private static func run(_ args: Arguments) throws {
        let phaseStartedAt = clockNS()
        let planId = try args.need("--plan-id")
        let phaseId = try args.need("--phase-id")
        let operationId = try args.need("--operation-id")
        let sessionId = try args.need("--session-id")
        let remainingText = try args.need("--remaining-ms")
        try require(uuid(planId) && uuid(phaseId) && uuid(operationId), "arguments", "canonical plan/phase/operation UUIDs required")
        try require(sessionId == "\(planId)-\(phaseId)-\(operationId)", "arguments", "session identity must derive from registered request IDs")
        guard let remainingMs = UInt64(remainingText), String(remainingMs) == remainingText,
              (1...maxPhaseMs).contains(remainingMs) else {
            throw HarnessFailure(code: "phase_deadline", detail: "bounded remaining phase lease required")
        }
        let runDeadline = phaseStartedAt + remainingMs * 1_000_000

        let helperURL = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL.resolvingSymlinksInPath()
        let cacheRoot = helperURL.deletingLastPathComponent().deletingLastPathComponent()
        let manifestPath = cacheRoot.appendingPathComponent("prepared.json")
        let manifestData = try readHeldData(manifestPath.path, limit: 32_768)
        let manifest = try decoder.decode(PreparedPasteManifest.self, from: manifestData)
        try validatePreparedManifest(manifest, helperURL: helperURL, cacheRoot: cacheRoot)
        let (recipe, recipeDigest) = try loadRecipe(manifest.recipePath)
        try validateFixedRecipe(recipe, digest: recipeDigest, manifest: manifest)
        try require(remainingMs <= UInt64(recipe.hostLifetimeMs), "phase_deadline", "phase lease must fit the fixed Host lifetime")

        let sessionsRoot = cacheRoot.appendingPathComponent("sessions", isDirectory: true)
        try ensurePrivateDirectory(sessionsRoot)
        try require(clockNS() < runDeadline, "phase_deadline", "phase lease expired while validating prepared artifacts")
        let session = sessionsRoot.appendingPathComponent(sessionId, isDirectory: true)
        try FileManager.default.createDirectory(
            at: session,
            withIntermediateDirectories: false,
            attributes: [.posixPermissions: 0o700]
        )
        try requireRealDirectory(session, "owned_session")
        try require(clockNS() < runDeadline, "phase_deadline", "phase lease expired before launching the owned Host")

        var state = SessionState()
        defer {
            if state.childStarted, state.child?.isRunning == true {
                _ = stopAndWait(state: &state, session: session)
            }
        }

        try launchAndAwaitSetup(manifest: manifest, recipe: recipe, session: session, deadline: runDeadline, state: &state)
        emit([
            "artifact_type": eventType,
            "schema_version": schemaVersion,
            "event": "session_ready",
            "plan_id": planId,
            "phase_id": phaseId,
            "operation_id": operationId,
            "session_id": sessionId,
            "helper_pid": getpid(),
            "source_manifest_sha256": manifest.sourceManifestSha256,
            "helper_executable_sha256": manifest.helperExecutableSha256,
            "host_executable_sha256": manifest.hostExecutableSha256,
            "host_bundle_identifier": manifest.hostBundleIdentifier,
            "recipe_sha256": manifest.recipeSha256,
            "host_receipt_sha256": state.receiptSha256 as Any? ?? NSNull(),
            "host_receipt": try encodedObject(state.receipt),
            "host_kernel_identity": try encodedObject(state.owner?.identity),
            "target_scope": ["bundle_id": bundleIdentifier, "window_title": windowTitle, "control_identifier": controlIdentifier],
            "paste_input": ["sha256": manifest.insertedTextSha256, "byte_length": manifest.insertedTextByteLength],
            "declared_settled_value": ["sha256": manifest.expectedSettledSha256, "byte_length": manifest.expectedSettledByteLength],
            "learning": "not_run",
            "sql": "not_run",
            "settings": "not_run"
        ])

        let inbox = BoundedCommandInbox()
        var commandCount = 0
        while clockNS() < runDeadline {
            if state.child?.isRunning != true {
                emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "session_unexpected_child_exit", "plan_id": planId, "phase_id": phaseId, "operation_id": operationId, "learning": "not_run"])
                return
            }
            guard let item = inbox.take() else {
                Thread.sleep(forTimeInterval: 0.01)
                continue
            }
            if case .closed = item {
                let cleanup = stopAndWait(state: &state, session: session)
                emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "session_closed", "plan_id": planId, "phase_id": phaseId, "operation_id": operationId, "cleanup": cleanup, "reason": "stdin_closed", "learning": "not_run"])
                return
            }
            commandCount += 1
            if commandCount > maxCommands {
                emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "command_rejected", "reason": "command_count_exceeded", "learning": "not_run"])
                continue
            }
            switch item {
            case .overlong:
                emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "command_rejected", "reason": "command_line_over_budget", "learning": "not_run"])
            case .commandOverflow:
                emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "command_rejected", "reason": "command_count_exceeded", "learning": "not_run"])
            case .closed:
                let cleanup = stopAndWait(state: &state, session: session)
                emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "session_closed", "plan_id": planId, "phase_id": phaseId, "operation_id": operationId, "cleanup": cleanup, "reason": "stdin_closed", "learning": "not_run"])
                return
            case .line(let bytes):
                try handleCommand(
                    bytes,
                    planId: planId,
                    phaseId: phaseId,
                    operationId: operationId,
                    sessionId: sessionId,
                    manifest: manifest,
                    recipe: recipe,
                    session: session,
                    deadline: runDeadline,
                    state: &state
                )
                if !state.childStarted || state.child?.isRunning != true {
                    return
                }
            }
        }
        let cleanup = stopAndWait(state: &state, session: session)
        emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "session_closed", "plan_id": planId, "phase_id": phaseId, "operation_id": operationId, "cleanup": cleanup, "reason": "phase_deadline_expired", "learning": "not_run"])
    }

    @MainActor private static func validatePreparedManifest(
        _ manifest: PreparedPasteManifest,
        helperURL: URL,
        cacheRoot: URL
    ) throws {
        let expectedRoot = cacheRoot.standardizedFileURL
        let expectedHelper = expectedRoot.appendingPathComponent("bin/OwnedPasteSession").standardizedFileURL
        let expectedHostApp = expectedRoot.appendingPathComponent("FixtureHost.app", isDirectory: true).standardizedFileURL
        let expectedHost = expectedHostApp.appendingPathComponent("Contents/MacOS/FixtureHost").standardizedFileURL
        let expectedRecipe = expectedRoot.appendingPathComponent("Recipe.json").standardizedFileURL
        let expectedManifest = expectedRoot.appendingPathComponent("source-manifest.txt").standardizedFileURL
        let expectedInfo = expectedHostApp.appendingPathComponent("Contents/Info.plist").standardizedFileURL
        try require(manifest.artifactType == artifactType && manifest.schemaVersion == schemaVersion,
                    "prepared_manifest", "registered paste-session artifact required")
        try require(manifest.sourceManifestSha256 == OwnedPasteBuildPins.sourceManifestSha256
                    && manifest.helperSourceSha256 == OwnedPasteBuildPins.helperSourceSha256
                    && manifest.buildScriptSha256 == OwnedPasteBuildPins.buildScriptSha256
                    && manifest.recipeSha256 == OwnedPasteBuildPins.recipeSha256
                    && manifest.hostInfoPlistSha256 == OwnedPasteBuildPins.hostInfoPlistSha256
                    && manifest.insertedTextSha256 == OwnedPasteBuildPins.insertedTextSha256
                    && manifest.insertedTextByteLength == OwnedPasteBuildPins.insertedTextByteLength
                    && manifest.expectedPostInsertSha256 == OwnedPasteBuildPins.expectedPostInsertSha256
                    && manifest.expectedPostInsertByteLength == OwnedPasteBuildPins.expectedPostInsertByteLength
                    && manifest.expectedSettledSha256 == OwnedPasteBuildPins.expectedSettledSha256
                    && manifest.expectedSettledByteLength == OwnedPasteBuildPins.expectedSettledByteLength,
                    "prepared_manifest", "prepared fields differ from the compiled source pins")
        try require(helperURL == expectedHelper && manifest.helperPath == expectedHelper.path,
                    "prepared_manifest", "helper must be the registered cache executable")
        try require(manifest.hostAppPath == expectedHostApp.path && manifest.hostExecutablePath == expectedHost.path
                    && manifest.recipePath == expectedRecipe.path && manifest.sourceManifestPath == expectedManifest.path
                    && manifest.infoPlistPath == expectedInfo.path,
                    "prepared_manifest", "registered cache paths required")
        try require(manifest.hostBundleIdentifier == bundleIdentifier
                    && manifest.insertedTextSha256 == digest(Data(insertedText.utf8))
                    && manifest.insertedTextByteLength == insertedText.utf8.count
                    && manifest.expectedPostInsertSha256 == digest(Data(postInsertText.utf8))
                    && manifest.expectedPostInsertByteLength == postInsertText.utf8.count
                    && manifest.expectedSettledSha256 == digest(Data(settledText.utf8))
                    && manifest.expectedSettledByteLength == settledText.utf8.count,
                    "prepared_manifest", "fixed paste-session text identity mismatch")
        for value in [manifest.sourceManifestSha256, manifest.helperSourceSha256, manifest.buildScriptSha256,
                      manifest.helperExecutableSha256, manifest.hostExecutableSha256, manifest.recipeSha256,
                      manifest.hostInfoPlistSha256, manifest.insertedTextSha256, manifest.expectedPostInsertSha256,
                      manifest.expectedSettledSha256] {
            try require(nativeHash(value), "prepared_manifest", "lowercase SHA-256 required")
        }
        let identity = [manifest.sourceManifestSha256, manifest.helperSourceSha256, manifest.buildScriptSha256,
                        manifest.recipeSha256, manifest.hostInfoPlistSha256].joined(separator: "\n")
        try require(manifest.cacheId == "v1-" + digest(Data(identity.utf8)),
                    "prepared_manifest", "content-addressed cache identity mismatch")
        try require(cacheRoot.lastPathComponent == manifest.cacheId
                    && cacheRoot.deletingLastPathComponent().lastPathComponent == "learning-native-paste-session",
                    "prepared_manifest", "cache root is not the registered paste-session cache")
        try requireRealDirectory(expectedRoot, "prepared_cache")
        try requireRealDirectory(expectedRoot.appendingPathComponent("bin", isDirectory: true), "prepared_cache")
        try requireRealDirectory(expectedHostApp, "prepared_cache")
        try requireRealDirectory(expectedHostApp.appendingPathComponent("Contents", isDirectory: true), "prepared_cache")
        try requireRealDirectory(expectedHostApp.appendingPathComponent("Contents/MacOS", isDirectory: true), "prepared_cache")
        try require(try fileDigest(expectedHelper.path, limit: 67_108_864) == manifest.helperExecutableSha256
                    && fileDigest(expectedHost.path, limit: 67_108_864) == manifest.hostExecutableSha256
                    && fileDigest(expectedRecipe.path, limit: 1_048_576) == manifest.recipeSha256
                    && fileDigest(expectedManifest.path, limit: 16_384) == manifest.sourceManifestSha256
                    && fileDigest(expectedInfo.path, limit: 16_384) == manifest.hostInfoPlistSha256,
                    "prepared_manifest", "prepared artifact hash mismatch")
        for executable in [expectedHelper, expectedHost] {
            let attributes = try FileManager.default.attributesOfItem(atPath: executable.path)
            guard let mode = attributes[.posixPermissions] as? NSNumber else {
                throw HarnessFailure(code: "prepared_manifest", detail: "executable mode unavailable")
            }
            try require(mode.intValue & 0o111 != 0, "prepared_manifest", "prepared executable is not executable")
        }
    }

    @MainActor private static func validateFixedRecipe(_ recipe: Recipe, digest recipeDigest: String, manifest: PreparedPasteManifest) throws {
        try require(recipeDigest == manifest.recipeSha256 && recipe.runId == runId && recipe.caseId == caseId
                    && recipe.fixtureId == fixtureId && recipe.title == windowTitle
                    && recipe.control == controlIdentifier && recipe.otherControl == otherControlIdentifier,
                    "prepared_recipe", "fixed recipe identity mismatch")
        try require(recipe.before == "Type: " && recipe.inserted == insertedText
                    && recipe.selection.location == 6 && recipe.selection.length == 0
                    && recipe.fixtureAction == "no_write" && recipe.readOnly == false
                    && recipe.trigger.kind == "research_stdio" && recipe.stress.count == 1
                    && recipe.stress[0].atMs == 0 && recipe.stress[0].kind == "learning_edit"
                    && recipe.pollMs == 10 && recipe.deadlineMs == 20_000
                    && recipe.hostLifetimeMs == 30_000 && recipe.fixtureDelayMs == 0,
                    "prepared_recipe", "single declared same-target paste/edit recipe required")
        guard let consent = recipe.researchSession?.learning else {
            throw HarnessFailure(code: "prepared_recipe", detail: "explicit fixture edit authority required")
        }
        try require(consent.editFixtureId == operationFixtureId && consent.purpose == "correction"
                    && consent.mappingId == "research-owned-native-oracle-host-v1"
                    && consent.bundleId == bundleIdentifier && consent.observerEnabled
                    && consent.postInsertField == postInsertText && consent.settledField == settledText
                    && consent.terms.count == 1 && consent.terms[0].fixtureId == termFixtureId
                    && consent.terms[0].before == insertedText && consent.terms[0].after == "VoiceFlow",
                    "prepared_recipe", "fixed synthetic programmatic edit must match the verified paste baseline")
        try require(consent.postInsertField.utf8.count == manifest.expectedPostInsertByteLength
                    && digest(Data(consent.postInsertField.utf8)) == manifest.expectedPostInsertSha256
                    && consent.settledField.utf8.count == manifest.expectedSettledByteLength
                    && digest(Data(consent.settledField.utf8)) == manifest.expectedSettledSha256,
                    "prepared_recipe", "declared text bytes differ from registered hashes")
    }

    @MainActor private static func launchAndAwaitSetup(
        manifest: PreparedPasteManifest,
        recipe: Recipe,
        session: URL,
        deadline: UInt64,
        state: inout SessionState
    ) throws {
        let hostURL = URL(fileURLWithPath: manifest.hostExecutablePath).standardizedFileURL.resolvingSymlinksInPath()
        try require(try fileDigest(hostURL.path, limit: 67_108_864) == manifest.hostExecutableSha256,
                    "host_identity", "registered Host executable changed")
        let child = Process()
        child.executableURL = hostURL
        child.arguments = ["--recipe", manifest.recipePath, "--directory", session.path]
        child.standardOutput = FileHandle.nullDevice
        child.standardError = FileHandle.nullDevice
        state.child = child
        let readyURL = session.appendingPathComponent("ready.json")
        try require(!FileManager.default.fileExists(atPath: readyURL.path), "owned_session", "fresh Host setup receipt required")
        try require(clockNS() < deadline, "phase_deadline", "phase lease expired before launching the owned Host")
        try child.run()
        state.childStarted = true
        try require(child.isRunning, "process_identity_unknown", "Host exited before process ownership was established")
        let spawn = observeProcessIdentity(child.processIdentifier, cocoaLaunchDate: nil, cocoaSource: "not_queried_owned_spawn_kernel_only")
        let owner = try OwnedHostProcess(child, observation: spawn, executable: hostURL)
        state.owner = owner

        let setupDeadline = deadline > 1_000_000_000 ? deadline - 1_000_000_000 : deadline
        while !FileManager.default.fileExists(atPath: readyURL.path)
            && child.isRunning && clockNS() < setupDeadline {
            Thread.sleep(forTimeInterval: 0.01)
        }
        try require(FileManager.default.fileExists(atPath: readyURL.path), "host_setup_unknown", "Host setup receipt not observed before reserve")
        let receiptBytes = try readHeldData(readyURL.path, limit: 32_768)
        let receipt = try loadHostReceipt(readyURL.path)
        state.receipt = receipt
        state.receiptSha256 = digest(receiptBytes)
        try require(receipt.runId == recipe.runId && receipt.caseId == recipe.caseId
                    && receipt.fixtureId == recipe.fixtureId && receipt.recipeSha256 == manifest.recipeSha256
                    && receipt.title == windowTitle && receipt.control == controlIdentifier
                    && receipt.otherControl == otherControlIdentifier,
                    "host_identity", "Host receipt differs from the registered target")
        try owner.requireMatchingHost(receipt.processIdentity)
        guard let setup = receipt.setupObservation,
              let native = receipt.nativeApplication else {
            throw HarnessFailure(code: "host_setup_unknown", detail: "Host setup/bundle identity facts unavailable")
        }
        try require(setup.independentAxReadiness == "not_observed" && setup.targetFirstResponder
                    && native.source == "FixtureHost_Bundle.main" && native.state == "observed_app_bundle"
                    && native.bundlePath == manifest.hostAppPath && native.bundleIdentifier == manifest.hostBundleIdentifier
                    && native.executablePath == manifest.hostExecutablePath
                    && native.infoPlistSha256 == manifest.hostInfoPlistSha256,
                    "host_identity", "owned Host setup differs from the registered app")
        try owner.requireLiveIdentity()
    }

    @MainActor private static func handleCommand(
        _ bytes: Data,
        planId: String,
        phaseId: String,
        operationId: String,
        sessionId: String,
        manifest: PreparedPasteManifest,
        recipe: Recipe,
        session: URL,
        deadline: UInt64,
        state: inout SessionState
    ) throws {
        let object = try strictObject(try parseJSON(bytes), allowed: ["request_id", "command"], required: ["request_id", "command"])
        guard let requestId = object["request_id"] as? String,
              let command = object["command"] as? String,
              uuid(requestId) else {
            throw HarnessFailure(code: "command_schema", detail: "bounded command UUID and kind required")
        }
        switch command {
        case "apply_declared_edit":
            try require(!state.editAttempted && !state.editCompleted, "operation_duplicate", "declared edit is single-use")
            state.editAttempted = true
            try require(clockNS() < deadline && state.child?.isRunning == true,
                        "phase_deadline", "Host session expired before declared edit")
            guard let owner = state.owner else {
                throw HarnessFailure(code: "process_identity_unknown", detail: "held Host owner unavailable")
            }
            try owner.requireLiveIdentity()
            let marker = session.appendingPathComponent("stress-\(recipe.runId)-0.json")
            let seen = session.appendingPathComponent("stress-seen-\(recipe.runId)-0.json")
            let editReceipt = session.appendingPathComponent("learning-edit.json")
            try require(!FileManager.default.fileExists(atPath: marker.path)
                        && !FileManager.default.fileExists(atPath: seen.path)
                        && !FileManager.default.fileExists(atPath: editReceipt.path),
                        "owned_session", "fresh single-use declared action files required")
            try writeNew(["run_id": recipe.runId, "index": "0"], marker.path)
            let editDeadline = min(deadline - 250_000_000, clockNS() + 2_000_000_000)
            while (!FileManager.default.fileExists(atPath: seen.path)
                   || !FileManager.default.fileExists(atPath: editReceipt.path))
                && state.child?.isRunning == true && clockNS() < editDeadline {
                try owner.requireLiveIdentity()
                Thread.sleep(forTimeInterval: 0.01)
            }
            guard FileManager.default.fileExists(atPath: seen.path),
                  FileManager.default.fileExists(atPath: editReceipt.path) else {
                emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "programmatic_edit_unknown", "request_id": requestId, "plan_id": planId, "phase_id": phaseId, "operation_id": operationId, "session_id": sessionId, "reason": "host_edit_receipt_unobserved", "learning": "not_run", "sql": "not_run", "settings": "not_run"])
                return
            }
            try validateActionReceipts(seenPath: seen.path, editPath: editReceipt.path, recipe: recipe)
            let sample = try readFinalAXSample(session: session, deadline: deadline, owner: owner)
            state.editCompleted = true
            emit([
                "artifact_type": eventType,
                "schema_version": schemaVersion,
                "event": "programmatic_edit_and_final_ax",
                "request_id": requestId,
                "plan_id": planId,
                "phase_id": phaseId,
                "operation_id": operationId,
                "session_id": sessionId,
                "host_pid": owner.identity.pid,
                "host_kernel_identity": try encodedObject(owner.identity),
                "programmatic_edit": ["state": "observed", "actor": "owned_fixture_host_programmatic_setter", "receipt_sha256": sample.editReceiptSha256],
                "actual_sample": ["state": sample.state, "source": sample.source, "sha256": sample.valueSha256 as Any? ?? NSNull(), "byte_length": sample.valueByteLength as Any? ?? NSNull(), "ax_readback": try encodedObject(sample.axReadback), "process_identity_before": try encodedObject(sample.identityBefore), "process_identity_after": try encodedObject(sample.identityAfter)],
                "same_host_still_live": state.child?.isRunning == true,
                "target_terminal": "not_product_target_terminal",
                "learning": "not_run",
                "sql": "not_run",
                "settings": "not_run"
            ])
        case "close":
            let cleanup = stopAndWait(state: &state, session: session)
            emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "session_closed", "request_id": requestId, "plan_id": planId, "phase_id": phaseId, "operation_id": operationId, "session_id": sessionId, "cleanup": cleanup, "learning": "not_run", "sql": "not_run", "settings": "not_run"])
        default:
            emit(["artifact_type": eventType, "schema_version": schemaVersion, "event": "command_rejected", "request_id": requestId, "reason": "unsupported_command", "learning": "not_run"])
        }
    }

    private struct FinalSample {
        let state: String
        let source: String
        let valueSha256: String?
        let valueByteLength: Int?
        let axReadback: AXReadback?
        let identityBefore: ProcessIdentityObservation?
        let identityAfter: ProcessIdentityObservation?
        let editReceiptSha256: String
    }

    @MainActor private static func readFinalAXSample(
        session: URL,
        deadline: UInt64,
        owner: OwnedHostProcess
    ) throws -> FinalSample {
        try owner.requireLiveIdentity()
        let application = AXUIElementCreateApplication(owner.identity.pid)
        let reader = AXTargetReader(
            application: application,
            title: windowTitle,
            control: controlIdentifier,
            otherControl: controlIdentifier,
            source: nativeAXRead
        )
        var identities: [ProcessIdentityObservation] = []
        let now = clockNS()
        let readDeadline = min(deadline > 250_000_000 ? deadline - 250_000_000 : deadline, now + 500_000_000)
        guard now < readDeadline else {
            return FinalSample(state: "unknown", source: "AXTargetReader_owned_declared_target_v1", valueSha256: nil, valueByteLength: nil, axReadback: nil, identityBefore: nil, identityAfter: nil, editReceiptSha256: try hashActionReceipt(session: session))
        }
        let result = reader.sample(deadline: readDeadline) {
            try require(NSWorkspace.shared.frontmostApplication?.processIdentifier == owner.identity.pid,
                        "target_focus_changed", "registered Host must remain frontmost during final AX sampling")
            let observation = observeProcessIdentity(owner.identity.pid, cocoaLaunchDate: nil, cocoaSource: "not_queried_AX_sample_kernel_only")
            identities.append(observation)
            try owner.requireMatchingHost(observation)
        }
        try owner.requireLiveIdentity()
        try require(NSWorkspace.shared.frontmostApplication?.processIdentifier == owner.identity.pid,
                    "target_focus_changed", "registered Host must remain frontmost through final AX sampling")
        let editReceiptSha256 = try hashActionReceipt(session: session)
        guard result.evidence.applicability == .matched,
              result.evidence.reason == .observed,
              identities.count == 2,
              let value = result.values.value,
              value.utf8.count <= 65_536,
              identities.allSatisfy({ $0.kernelState == "observed" && $0.kernel == owner.identity }) else {
            return FinalSample(state: result.evidence.applicability == .changed ? "changed" : "unknown", source: result.evidence.source, valueSha256: nil, valueByteLength: nil, axReadback: result.evidence, identityBefore: identities.first, identityAfter: identities.last, editReceiptSha256: editReceiptSha256)
        }
        return FinalSample(state: "observed", source: result.evidence.source, valueSha256: digest(Data(value.utf8)), valueByteLength: value.utf8.count, axReadback: result.evidence, identityBefore: identities.first, identityAfter: identities.last, editReceiptSha256: editReceiptSha256)
    }

    private static func validateActionReceipts(seenPath: String, editPath: String, recipe: Recipe) throws {
        let seenData = try readHeldData(seenPath, limit: 8_192)
        let seen = try strictObject(try parseJSON(seenData), allowed: ["run_id", "index", "reason"], required: ["run_id", "index"])
        try require(seen["run_id"] as? String == recipe.runId && seen["index"] as? String == "0",
                    "programmatic_edit_receipt", "Host did not consume the exact declared action")
        if let reason = seen["reason"] as? String {
            throw HarnessFailure(code: "programmatic_edit_not_applied", detail: reason)
        }
        let editData = try readHeldData(editPath, limit: 8_192)
        let edit = try strictObject(try parseJSON(editData), allowed: ["run_id", "case_id", "edit_fixture_id", "actor", "applied_uptime_ns", "product_observation"], required: ["run_id", "case_id", "edit_fixture_id", "actor", "applied_uptime_ns", "product_observation"])
        try require(edit["run_id"] as? String == recipe.runId
                    && edit["case_id"] as? String == recipe.caseId
                    && edit["edit_fixture_id"] as? String == operationFixtureId
                    && edit["actor"] as? String == "owned_fixture_host_programmatic_setter"
                    && edit["product_observation"] as? String == "unknown_until_producer_receipt",
                    "programmatic_edit_receipt", "Host action receipt differs from the declared programmatic edit")
        guard let appliedTime = edit["applied_uptime_ns"] as? String,
              let parsed = UInt64(appliedTime), String(parsed) == appliedTime,
              parsed <= clockNS() else {
            throw HarnessFailure(code: "programmatic_edit_receipt", detail: "canonical Host monotonic action time required")
        }
    }

    private static func hashActionReceipt(session: URL) throws -> String {
        try fileDigest(session.appendingPathComponent("learning-edit.json").path, limit: 8_192)
    }

    @MainActor private static func stopAndWait(state: inout SessionState, session: URL) -> [String: Any] {
        guard let child = state.child else {
            return ["state": "not_started", "exit_confirmed": true, "termination_status": NSNull()]
        }
        guard state.childStarted else {
            return ["state": "not_started", "exit_confirmed": true, "termination_status": NSNull()]
        }
        let pid = state.owner?.identity.pid ?? child.processIdentifier
        let stopPath = session.appendingPathComponent("stop-\(runId).json").path
        var reason: String?
        if child.isRunning && !FileManager.default.fileExists(atPath: stopPath) {
            do { try writeNew(["run_id": runId], stopPath) }
            catch { reason = "owned_stop_marker_failed" }
        }
        let gracefulDeadline = clockNS() + 500_000_000
        while child.isRunning && clockNS() < gracefulDeadline { Thread.sleep(forTimeInterval: 0.01) }
        if child.isRunning {
            child.terminate()
            reason = reason ?? "owned_child_terminate_requested_after_stop_timeout"
            let terminateDeadline = clockNS() + 1_000_000_000
            while child.isRunning && clockNS() < terminateDeadline { Thread.sleep(forTimeInterval: 0.01) }
        }
        if child.isRunning {
            _ = Darwin.kill(pid, SIGKILL)
            reason = reason ?? "owned_child_sigkill_requested_after_terminate_timeout"
        }
        while child.isRunning { Thread.sleep(forTimeInterval: 0.02) }
        child.waitUntilExit()
        return ["state": "owned_host_exit_confirmed", "child_pid": pid, "termination_status": child.terminationStatus,
                "exit_confirmed": !child.isRunning, "reason": reason as Any? ?? NSNull()]
    }

    private static func emit(_ object: [String: Any]) {
        guard JSONSerialization.isValidJSONObject(object),
              let encoded = try? JSONSerialization.data(withJSONObject: object, options: [.sortedKeys]) else {
            return
        }
        var line = encoded
        line.append(0x0a)
        FileHandle.standardOutput.write(line)
    }

    private static func encodedObject<T: Encodable>(_ value: T?) throws -> Any {
        guard let value else { return NSNull() }
        let bytes = try encoder.encode(value)
        return try JSONSerialization.jsonObject(with: bytes, options: [.fragmentsAllowed])
    }

    private static func readHeldData(_ path: String, limit: Int) throws -> Data {
        let held = try HeldNativeAppObject(path: path)
        return try held.data(limit: limit)
    }

    private static func fileDigest(_ path: String, limit: Int) throws -> String {
        digest(try readHeldData(path, limit: limit))
    }

    private static func ensurePrivateDirectory(_ url: URL) throws {
        if !FileManager.default.fileExists(atPath: url.path) {
            try FileManager.default.createDirectory(at: url, withIntermediateDirectories: false, attributes: [.posixPermissions: 0o700])
        }
        try requireRealDirectory(url, "owned_session_root")
        try FileManager.default.setAttributes([.posixPermissions: 0o700], ofItemAtPath: url.path)
    }

    private static func requireRealDirectory(_ url: URL, _ code: String) throws {
        let values = try url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        try require(values.isDirectory == true && values.isSymbolicLink != true, code, "owned nonsymlink directory required")
    }
}
#endif
