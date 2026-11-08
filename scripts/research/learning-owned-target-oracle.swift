#if TARGET_ORACLE_ONLY
import AppKit
import ApplicationServices
import Darwin
import Foundation

private let ownedTargetBundleId = "local.voiceflow.learning-oracle-fixture.v1"
private let ownedTargetFixtureId = "90000000-0000-4000-8000-000000000002"
private let ownedTargetRunId = "90000000-0000-4000-8000-000000000001"
private let ownedTargetWindowTitle = "Voice harness owned fixture 90000000-0000-4000-8000-000000000002"
private let ownedTargetControlId = "vf-fixture-90000000-0000-4000-8000-000000000002-target"
private let ownedTargetRecipeSha256 = "1d2f3f722c0232bc78e5896fe80c4a328c800e6813e34ead983167ab0a27ce15"
private let ownedTargetInfoPlistSha256 = "8b7bee050e73472f890e0767f9e036c023d712fb9239ff7d41ca6b18613b74d5"
private let ownedTargetSourceManifestSha256 = "4e143f250d877feebe36098020df5545cf200d6273e95a84eb4af16656d6668f"
private let ownedTargetExpectedTextSha256 = "e7b17776dd843cfaa300566ee48aaa3a260364bb8edaebe7401ae749c1e9bfe1"
private let ownedTargetExpectedTextByteLength = 29
private let ownedTargetSource = "AXTargetReader_owned_declared_target_v1"
private let targetReadMaximumNs: UInt64 = 500_000_000
private let setupAndReadReserveNs: UInt64 = 750_000_000

private struct PreparedManifest: Decodable {
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
    let expectedValueSha256: String
    let expectedValueByteLength: Int
    let hostBundleIdentifier: String
    let helperPath: String
    let hostAppPath: String
    let hostExecutablePath: String
    let recipePath: String
    let sourceManifestPath: String
    let infoPlistPath: String
}

private struct NativeTargetSetup: Encodable {
    let state: String
    let receiptSha256: String?
    let receipt: HostReceipt?
    let reason: String?
}

private struct NativeTargetActualSample: Encodable {
    let state: String
    let source: String
    let sampledUptimeNs: String?
    let value: String?
    let valueSha256: String?
    let valueByteLength: Int?
    let axReadback: AXReadback?
    let processIdentityBefore: ProcessIdentityObservation?
    let processIdentityAfter: ProcessIdentityObservation?
    let bindingMode: String
}

private struct NativeTargetCleanup: Encodable {
    let state: String
    let childPid: Int32?
    let terminationStatus: Int32?
    let exitConfirmed: Bool
    let reason: String?
}

private struct NativeTargetOutcome: Encodable {
    let artifactType: String
    let schemaVersion: Int
    let planId: String
    let phaseId: String
    let operationId: String
    let sessionId: String
    let helperPid: Int32
    let overallState: String
    let sourceManifestSha256: String
    let helperExecutableSha256: String
    let hostExecutableSha256: String
    let hostBundleIdentifier: String
    let recipeSha256: String
    let setup: NativeTargetSetup
    let actualSample: NativeTargetActualSample
    let cleanup: NativeTargetCleanup
    let expectedComparison: String
    let targetTerminal: String
    let unknownReason: String?
}

private struct NativeTargetRunState {
    var child: Process?
    var childStarted = false
    var owner: OwnedHostProcess?
    var hostIdentity: ProcessIdentityObservation?
    var setupReceipt: HostReceipt?
    var setupReceiptSha256: String?
    var setupReason: String?
    var axReadback: AXReadback?
    var actualValue: String?
    var sampleUptimeNs: String?
    var identityObservations: [ProcessIdentityObservation] = []
    var sampleReason: String?
    var terminalReason: String?
    var childStdoutHandle: FileHandle?
    var childStderrHandle: FileHandle?
}

@main private struct OwnedTargetOracleCLI {
    @MainActor static func main() {
        do {
            let args = try Arguments(
                ["--plan-id", "--phase-id", "--operation-id", "--session-id", "--remaining-ms"],
                switches: []
            )
            let outcome = try runOwnedTargetSample(args)
            var data = try encoder.encode(outcome)
            data.append(0x0a)
            FileHandle.standardOutput.write(data)
        } catch {
            failCLI(error)
        }
    }

    @MainActor private static func runOwnedTargetSample(_ args: Arguments) throws -> NativeTargetOutcome {
        let planId = try args.need("--plan-id")
        let phaseId = try args.need("--phase-id")
        let operationId = try args.need("--operation-id")
        let sessionId = try args.need("--session-id")
        let remainingMsText = try args.need("--remaining-ms")
        try require(uuid(planId) && uuid(phaseId) && uuid(operationId), "arguments", "canonical plan/phase/operation UUIDs required")
        try require(sessionId == "\(planId)-\(phaseId)-\(operationId)", "arguments", "session identity must derive from registered request IDs")
        guard let remainingMs = UInt64(remainingMsText), String(remainingMs) == remainingMsText,
              (1...1_200_000).contains(remainingMs) else {
            throw HarnessFailure(code: "phase_deadline", detail: "positive remaining active phase lease required before launch")
        }

        let helperPath = URL(fileURLWithPath: CommandLine.arguments[0]).standardizedFileURL.resolvingSymlinksInPath()
        let cacheRoot = helperPath.deletingLastPathComponent().deletingLastPathComponent()
        let manifestPath = cacheRoot.appendingPathComponent("prepared.json")
        let manifestData = try boundedData(manifestPath.path, limit: 32_768)
        let manifest = try decoder.decode(PreparedManifest.self, from: manifestData)
        try validatePreparedManifest(manifest, helperPath: helperPath, cacheRoot: cacheRoot)

        let sessionRoot = cacheRoot.appendingPathComponent("sessions", isDirectory: true)
        try requireDirectory(sessionRoot, "owned_session_root")
        let session = sessionRoot.appendingPathComponent(sessionId, isDirectory: true)
        let sessionValues = try session.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        try require(sessionValues.isDirectory == true && sessionValues.isSymbolicLink != true,
                    "owned_session", "precreated unique nonsymlink session required")
        let cancelPath = session.appendingPathComponent("cancel.json").path
        let runDeadline = DispatchTime.now().uptimeNanoseconds + remainingMs * 1_000_000

        var state = NativeTargetRunState()
        var overallState = "unknown"
        var unknownReason: String?
        var cleanupState = NativeTargetCleanup(state: "not_started", childPid: nil, terminationStatus: nil, exitConfirmed: true, reason: nil)
        var actualValid = false

        if FileManager.default.fileExists(atPath: cancelPath) {
            unknownReason = "cancelled_before_host_launch"
        } else if !AXIsProcessTrusted() {
            unknownReason = "accessibility_reader_permission_unavailable_no_prompt_requested"
        } else {
            do {
                try launchAndRead(
                    manifest: manifest,
                    session: session,
                    cancelPath: cancelPath,
                    runDeadline: runDeadline,
                    state: &state
                )
                actualValid = isValidActualSample(state)
                if !actualValid {
                    unknownReason = state.sampleReason ?? state.setupReason ?? "independent_ax_sample_unknown"
                }
            } catch {
                unknownReason = String(describing: error)
            }
        }

        cleanupState = stopAndWait(state: &state, session: session)
        let wasCancelled = FileManager.default.fileExists(atPath: cancelPath)
        let isLate = DispatchTime.now().uptimeNanoseconds >= runDeadline
        if let cleanupReason = cleanupState.reason {
            actualValid = false
            unknownReason = cleanupReason
        } else if !cleanupState.exitConfirmed {
            actualValid = false
            unknownReason = cleanupState.reason ?? "owned_host_exit_unconfirmed"
        } else if cleanupState.terminationStatus != 0, state.childStarted {
            actualValid = false
            unknownReason = "owned_host_exited_nonzero_\(cleanupState.terminationStatus ?? -1)"
        } else if wasCancelled {
            actualValid = false
            unknownReason = "phase_cancelled_owned_host_closed"
        } else if isLate {
            actualValid = false
            unknownReason = "phase_deadline_expired_owned_host_closed"
        }

        if actualValid {
            overallState = "observed_sample"
        } else if unknownReason == nil {
            unknownReason = "independent_ax_sample_unknown"
        }

        let setup = NativeTargetSetup(
            state: state.setupReceipt == nil ? "unknown" : "host_setup_received_only",
            receiptSha256: state.setupReceiptSha256,
            receipt: state.setupReceipt,
            reason: state.setupReason
        )
        let actual = NativeTargetActualSample(
            state: actualSampleState(state, valid: actualValid),
            source: ownedTargetSource,
            sampledUptimeNs: state.sampleUptimeNs,
            value: actualValid ? state.actualValue : nil,
            valueSha256: actualValid ? state.actualValue.map { digest(Data($0.utf8)) } : nil,
            valueByteLength: actualValid ? state.actualValue.map { $0.utf8.count } : nil,
            axReadback: state.axReadback,
            processIdentityBefore: state.identityObservations.first,
            processIdentityAfter: state.identityObservations.last,
            bindingMode: "same_owned_control_identifier_for_reader_pair; target_AXValue_queried_twice; second_control_value_not_exported"
        )

        return NativeTargetOutcome(
            artifactType: "voice_flow_owned_native_target_oracle",
            schemaVersion: 1,
            planId: planId,
            phaseId: phaseId,
            operationId: operationId,
            sessionId: sessionId,
            helperPid: ProcessInfo.processInfo.processIdentifier,
            overallState: overallState,
            sourceManifestSha256: manifest.sourceManifestSha256,
            helperExecutableSha256: manifest.helperExecutableSha256,
            hostExecutableSha256: manifest.hostExecutableSha256,
            hostBundleIdentifier: manifest.hostBundleIdentifier,
            recipeSha256: manifest.recipeSha256,
            setup: setup,
            actualSample: actual,
            cleanup: cleanupState,
            expectedComparison: "receiver_independent_not_performed_by_helper",
            targetTerminal: "not_probed_component_sample_only",
            unknownReason: unknownReason
        )
    }

    @MainActor private static func validatePreparedManifest(
        _ manifest: PreparedManifest,
        helperPath: URL,
        cacheRoot: URL
    ) throws {
        let expectedHelper = cacheRoot.appendingPathComponent("bin/OwnedTargetOracle").standardizedFileURL
        let expectedHostApp = cacheRoot.appendingPathComponent("FixtureHost.app", isDirectory: true).standardizedFileURL
        let expectedHost = expectedHostApp.appendingPathComponent("Contents/MacOS/FixtureHost").standardizedFileURL
        let expectedRecipe = cacheRoot.appendingPathComponent("Recipe.json").standardizedFileURL
        let expectedManifest = cacheRoot.appendingPathComponent("source-manifest.txt").standardizedFileURL
        let expectedInfo = expectedHostApp.appendingPathComponent("Contents/Info.plist").standardizedFileURL
        try require(manifest.artifactType == "voice_flow_owned_target_oracle_prepared" && manifest.schemaVersion == 1,
                    "prepared_manifest", "registered native target build manifest required")
        try require(helperPath == expectedHelper && manifest.helperPath == expectedHelper.path,
                    "prepared_manifest", "helper must be the registered cache executable")
        try require(manifest.hostAppPath == expectedHostApp.path && manifest.hostExecutablePath == expectedHost.path
                    && manifest.recipePath == expectedRecipe.path && manifest.sourceManifestPath == expectedManifest.path
                    && manifest.infoPlistPath == expectedInfo.path,
                    "prepared_manifest", "registered cache paths required")
        try require(manifest.sourceManifestSha256 == ownedTargetSourceManifestSha256
                    && manifest.recipeSha256 == ownedTargetRecipeSha256
                    && manifest.hostInfoPlistSha256 == ownedTargetInfoPlistSha256
                    && manifest.hostBundleIdentifier == ownedTargetBundleId
                    && manifest.expectedValueSha256 == ownedTargetExpectedTextSha256
                    && manifest.expectedValueByteLength == ownedTargetExpectedTextByteLength,
                    "prepared_manifest", "fixed target registry identity mismatch")
        let cacheIdentity = [
            manifest.sourceManifestSha256,
            manifest.helperSourceSha256,
            manifest.buildScriptSha256,
            manifest.recipeSha256,
            manifest.hostInfoPlistSha256
        ].joined(separator: "\n")
        try require(manifest.cacheId == "v1-" + digest(Data(cacheIdentity.utf8)),
                    "prepared_manifest", "content-addressed cache identity mismatch")
        for sha in [manifest.helperExecutableSha256, manifest.hostExecutableSha256, manifest.helperSourceSha256, manifest.buildScriptSha256] {
            try require(nativeHash(sha), "prepared_manifest", "canonical lowercase SHA-256 required")
        }
        let helperBytes = try boundedData(expectedHelper.path, limit: 67_108_864)
        let hostBytes = try boundedData(expectedHost.path, limit: 67_108_864)
        let recipeBytes = try boundedData(expectedRecipe.path, limit: 1_048_576)
        let sourceManifestBytes = try boundedData(expectedManifest.path, limit: 16_384)
        let infoBytes = try boundedData(expectedInfo.path, limit: 16_384)
        try require(digest(helperBytes) == manifest.helperExecutableSha256
                    && digest(hostBytes) == manifest.hostExecutableSha256
                    && digest(recipeBytes) == manifest.recipeSha256
                    && digest(sourceManifestBytes) == manifest.sourceManifestSha256
                    && digest(infoBytes) == manifest.hostInfoPlistSha256,
                    "prepared_manifest", "prepared executable or fixed input hash changed")
        try require(FileManager.default.isExecutableFile(atPath: expectedHost.path)
                    && FileManager.default.isExecutableFile(atPath: expectedHelper.path),
                    "prepared_manifest", "prepared helper and Host executables required")
        let (recipe, recipeSha) = try loadRecipe(expectedRecipe.path)
        try require(recipeSha == ownedTargetRecipeSha256 && recipe.runId == ownedTargetRunId
                    && recipe.fixtureId == ownedTargetFixtureId && recipe.title == ownedTargetWindowTitle
                    && recipe.control == ownedTargetControlId && recipe.trigger.kind == "fixture"
                    && recipe.fixtureAction == "no_write" && recipe.readOnly && recipe.stress.isEmpty,
                    "prepared_recipe", "fixed read-only owned target recipe required")
    }

    @MainActor private static func launchAndRead(
        manifest: PreparedManifest,
        session: URL,
        cancelPath: String,
        runDeadline: UInt64,
        state: inout NativeTargetRunState
    ) throws {
        let (recipe, recipeSha) = try loadRecipe(manifest.recipePath)
        try require(recipeSha == manifest.recipeSha256, "prepared_recipe", "recipe changed after preparation")
        let hostURL = URL(fileURLWithPath: manifest.hostExecutablePath).standardizedFileURL.resolvingSymlinksInPath()
        let executableBytes = try boundedData(hostURL.path, limit: 67_108_864)
        try require(digest(executableBytes) == manifest.hostExecutableSha256,
                    "host_identity", "Host executable changed after preparation")

        let child = Process()
        child.executableURL = hostURL
        child.arguments = ["--recipe", manifest.recipePath, "--directory", session.path]
        let hostStdout = session.appendingPathComponent("host.stdout.log")
        let hostStderr = session.appendingPathComponent("host.stderr.log")
        try require(!FileManager.default.fileExists(atPath: hostStdout.path)
                    && !FileManager.default.fileExists(atPath: hostStderr.path),
                    "owned_session", "fresh Host diagnostic paths required")
        try require(FileManager.default.createFile(atPath: hostStdout.path, contents: Data(), attributes: [.posixPermissions: 0o600])
                    && FileManager.default.createFile(atPath: hostStderr.path, contents: Data(), attributes: [.posixPermissions: 0o600]),
                    "owned_session", "Host diagnostics could not be created")
        guard let stdoutHandle = FileHandle(forWritingAtPath: hostStdout.path),
              let stderrHandle = FileHandle(forWritingAtPath: hostStderr.path) else {
            throw HarnessFailure(code: "owned_session", detail: "Host diagnostics could not be opened")
        }
        state.childStdoutHandle = stdoutHandle
        state.childStderrHandle = stderrHandle
        child.standardOutput = stdoutHandle
        child.standardError = stderrHandle
        state.child = child
        try require(!FileManager.default.fileExists(atPath: session.appendingPathComponent("ready.json").path),
                    "owned_session", "fresh Host receipt path required")
        try child.run()
        state.childStarted = true
        try require(child.isRunning, "process_identity_unknown", "owned Host exited before identity observation")
        let spawn = observeProcessIdentity(child.processIdentifier, cocoaLaunchDate: nil, cocoaSource: "not_queried_owned_spawn_kernel_only")
        state.hostIdentity = spawn
        let owner = try OwnedHostProcess(child, observation: spawn, executable: hostURL)
        state.owner = owner

        let readyPath = session.appendingPathComponent("ready.json").path
        let setupDeadline = runDeadline > setupAndReadReserveNs ? runDeadline - setupAndReadReserveNs : runDeadline
        while !FileManager.default.fileExists(atPath: readyPath)
            && child.isRunning
            && DispatchTime.now().uptimeNanoseconds < setupDeadline
            && !FileManager.default.fileExists(atPath: cancelPath) {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        if FileManager.default.fileExists(atPath: cancelPath) {
            state.setupReason = "phase_cancelled_before_host_setup"
            return
        }
        try require(FileManager.default.fileExists(atPath: readyPath), "host_setup_unknown", "owned Host setup receipt not observed before deadline")
        let hostReceiptBytes = try boundedData(readyPath, limit: 32_768)
        let receipt = try loadHostReceipt(readyPath)
        state.setupReceipt = receipt
        state.setupReceiptSha256 = digest(hostReceiptBytes)
        try require(receipt.runId == recipe.runId && receipt.caseId == recipe.caseId
                    && receipt.fixtureId == recipe.fixtureId && receipt.recipeSha256 == recipeSha
                    && receipt.title == recipe.title && receipt.control == recipe.control,
                    "host_identity", "Host receipt differs from fixed registered target recipe")
        try require(receipt.otherControl == recipe.otherControl,
                    "host_identity", "Host target metadata differs from its declared fixed fixture")
        guard let setup = receipt.setupObservation else {
            throw HarnessFailure(code: "host_setup_unknown", detail: "Host local setup receipt missing")
        }
        try require(setup.independentAxReadiness == "not_observed",
                    "host_setup", "Host setup must remain distinct from independent AX readiness")
        try require(setup.targetFirstResponder,
                    "host_setup", "registered fixture target was not the locally declared first responder")
        guard let native = receipt.nativeApplication else {
            throw HarnessFailure(code: "host_bundle_identity_unknown", detail: "Host Bundle.main observation missing")
        }
        try require(native.source == "FixtureHost_Bundle.main" && native.state == "observed_app_bundle"
                    && native.bundlePath == manifest.hostAppPath
                    && native.bundleIdentifier == manifest.hostBundleIdentifier
                    && native.executablePath == manifest.hostExecutablePath
                    && native.infoPlistSha256 == manifest.hostInfoPlistSha256,
                    "host_bundle_identity", "Host Bundle.main identity differs from the registered fixture app")
        try owner.requireMatchingHost(receipt.processIdentity)
        state.setupReason = nil

        let reader = AXTargetReader(
            application: AXUIElementCreateApplication(owner.identity.pid),
            title: receipt.title,
            control: receipt.control,
            otherControl: receipt.control,
            source: nativeAXRead
        )
        let sampleStarted = DispatchTime.now().uptimeNanoseconds
        let readDeadline = min(runDeadline - 250_000_000, sampleStarted + targetReadMaximumNs)
        if sampleStarted >= readDeadline {
            state.sampleReason = "phase_deadline_reserve_exhausted_before_ax_read"
            return
        }
        let read = reader.sample(deadline: readDeadline) {
            let observation = observeProcessIdentity(owner.identity.pid, cocoaLaunchDate: nil, cocoaSource: "not_queried_AX_sample_kernel_only")
            state.identityObservations.append(observation)
            try owner.requireMatchingHost(observation)
        }
        state.axReadback = read.evidence
        state.sampleUptimeNs = String(DispatchTime.now().uptimeNanoseconds)
        state.sampleReason = read.evidence.reason.rawValue
        state.actualValue = read.values.value
        if FileManager.default.fileExists(atPath: cancelPath) {
            state.sampleReason = "phase_cancelled_during_ax_sample"
        }
        if DispatchTime.now().uptimeNanoseconds >= runDeadline {
            state.sampleReason = "phase_deadline_expired_during_ax_sample"
        }
    }

    private static func actualSampleState(_ state: NativeTargetRunState, valid: Bool) -> String {
        if valid { return "observed" }
        if state.axReadback?.applicability == .changed || state.axReadback?.reason == .targetChanged {
            return "changed"
        }
        return "unknown"
    }

    @MainActor private static func isValidActualSample(_ state: NativeTargetRunState) -> Bool {
        guard let evidence = state.axReadback,
              evidence.source == ownedTargetSource,
              evidence.applicability == .matched,
              evidence.reason == .observed,
              state.identityObservations.count == 2,
              let owner = state.owner,
              let value = state.actualValue,
              value.utf8.count <= 65_536 else {
            return false
        }
        return state.identityObservations.allSatisfy {
            $0.kernelState == "observed" && $0.kernel == owner.identity
        }
    }

    @MainActor private static func stopAndWait(
        state: inout NativeTargetRunState,
        session: URL
    ) -> NativeTargetCleanup {
        defer {
            try? state.childStdoutHandle?.close()
            try? state.childStderrHandle?.close()
            state.childStdoutHandle = nil
            state.childStderrHandle = nil
        }
        guard let child = state.child else {
            return NativeTargetCleanup(state: "not_started", childPid: nil, terminationStatus: nil, exitConfirmed: true, reason: nil)
        }
        let pid = state.hostIdentity?.kernel?.pid ?? child.processIdentifier
        guard state.childStarted else {
            return NativeTargetCleanup(state: "not_started", childPid: nil, terminationStatus: nil, exitConfirmed: true, reason: state.terminalReason)
        }
        let stopPath = session.appendingPathComponent("stop-\(ownedTargetRunId).json").path
        if child.isRunning && !FileManager.default.fileExists(atPath: stopPath) {
            do {
                try writeNew(["run_id": ownedTargetRunId], stopPath)
            } catch {
                state.terminalReason = "owned_stop_marker_failed:\(error)"
            }
        }
        let gracefulDeadline = DispatchTime.now().uptimeNanoseconds + 250_000_000
        while child.isRunning && DispatchTime.now().uptimeNanoseconds < gracefulDeadline {
            RunLoop.current.run(until: Date().addingTimeInterval(0.01))
        }
        if child.isRunning {
            child.terminate()
            state.terminalReason = state.terminalReason ?? "owned_child_terminate_requested_after_stop_timeout"
            let terminateDeadline = DispatchTime.now().uptimeNanoseconds + 500_000_000
            while child.isRunning && DispatchTime.now().uptimeNanoseconds < terminateDeadline {
                RunLoop.current.run(until: Date().addingTimeInterval(0.01))
            }
        }
        if child.isRunning {
            _ = Darwin.kill(pid, SIGKILL)
            state.terminalReason = state.terminalReason ?? "owned_child_sigkill_requested_after_terminate_timeout"
        }
        while child.isRunning {
            RunLoop.current.run(until: Date().addingTimeInterval(0.02))
        }
        child.waitUntilExit()
        return NativeTargetCleanup(
            state: "owned_child_exit_confirmed",
            childPid: pid,
            terminationStatus: child.terminationStatus,
            exitConfirmed: !child.isRunning,
            reason: state.terminalReason
        )
    }

    private static func requireDirectory(_ url: URL, _ code: String) throws {
        let values = try url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
        try require(values.isDirectory == true && values.isSymbolicLink != true,
                    code, "owned nonsymlink directory required")
    }
}
#endif
