import Darwin
import Foundation
import VoiceFlowMLXSidecarCore

private struct SidecarArguments {
    let modelsRoot: String
    let audioRoot: String?
    let mode: String

    static func parse(_ arguments: [String]) -> SidecarArguments? {
        var values: [String: String] = [:]
        var index = 1
        while index < arguments.count {
            let key = arguments[index]
            guard (key == "--models-root" || key == "--audio-root" || key == "--mode"),
                  index + 1 < arguments.count,
                  values[key] == nil else {
                return nil
            }
            values[key] = arguments[index + 1]
            index += 2
        }
        guard let modelsRoot = values["--models-root"],
              modelsRoot.hasPrefix("/") else {
            return nil
        }
        let mode = values["--mode"] ?? "asr"
        guard mode == "asr" || mode == "cleanup" else { return nil }
        let audioRoot = values["--audio-root"]
        guard mode == "cleanup" || audioRoot?.hasPrefix("/") == true else { return nil }
        guard mode == "asr" || audioRoot == nil else { return nil }
        return SidecarArguments(modelsRoot: modelsRoot, audioRoot: audioRoot, mode: mode)
    }
}

@main
private struct VoiceFlowMLXSidecarMain {
    static func main() async {
        guard let arguments = SidecarArguments.parse(CommandLine.arguments) else {
            SidecarIO.writeDiagnostic(code: "invalid_arguments")
            exit(64)
        }
        guard SidecarIO.silenceThirdPartyOutput() else {
            SidecarIO.writeDiagnostic(code: "output_setup_failed")
            exit(70)
        }

        switch arguments.mode {
        case "asr":
            guard let audioRoot = arguments.audioRoot else {
                SidecarIO.writeDiagnostic(code: "invalid_arguments")
                exit(64)
            }
            await SidecarServer(
                modelsRootPath: arguments.modelsRoot,
                audioRootPath: audioRoot
            ).run()
        case "cleanup":
            await CleanupSidecarServer(modelsRootPath: arguments.modelsRoot).run()
        default:
            SidecarIO.writeDiagnostic(code: "invalid_arguments")
            exit(64)
        }
    }
}
