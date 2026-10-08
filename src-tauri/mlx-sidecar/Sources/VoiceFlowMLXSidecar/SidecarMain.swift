import Darwin
import Foundation
import VoiceFlowMLXSidecarCore

private struct SidecarArguments {
    let modelsRoot: String
    let audioRoot: String

    static func parse(_ arguments: [String]) -> SidecarArguments? {
        var values: [String: String] = [:]
        var index = 1
        while index < arguments.count {
            let key = arguments[index]
            guard (key == "--models-root" || key == "--audio-root"),
                  index + 1 < arguments.count,
                  values[key] == nil else {
                return nil
            }
            values[key] = arguments[index + 1]
            index += 2
        }
        guard let modelsRoot = values["--models-root"],
              let audioRoot = values["--audio-root"],
              modelsRoot.hasPrefix("/"),
              audioRoot.hasPrefix("/") else {
            return nil
        }
        return SidecarArguments(modelsRoot: modelsRoot, audioRoot: audioRoot)
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

        let server = SidecarServer(
            modelsRootPath: arguments.modelsRoot,
            audioRootPath: arguments.audioRoot
        )
        await server.run()
    }
}
