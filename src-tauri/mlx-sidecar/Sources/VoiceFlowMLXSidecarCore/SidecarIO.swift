import Darwin
import Foundation

enum JSONLineReaderError: Error {
    case lineTooLarge
    case readFailed
}

final class JSONLineReader {
    private let maximumLineBytes: Int
    private var buffer = [UInt8](repeating: 0, count: 8_192)
    private var bufferIndex = 0
    private var bufferCount = 0

    init(maximumLineBytes: Int) {
        self.maximumLineBytes = maximumLineBytes
    }

    func nextLine() throws -> Data? {
        var line = [UInt8]()
        line.reserveCapacity(min(maximumLineBytes, 8_192))

        while true {
            if bufferIndex == bufferCount {
                let count = buffer.withUnsafeMutableBytes { rawBuffer -> Int in
                    guard let base = rawBuffer.baseAddress else { return -1 }
                    while true {
                        let result = Darwin.read(STDIN_FILENO, base, rawBuffer.count)
                        if result < 0 && errno == EINTR { continue }
                        return result
                    }
                }
                if count < 0 { throw JSONLineReaderError.readFailed }
                if count == 0 {
                    return line.isEmpty ? nil : Data(line)
                }
                bufferIndex = 0
                bufferCount = count
            }

            let byte = buffer[bufferIndex]
            bufferIndex += 1
            if byte == 0x0A {
                if line.last == 0x0D { line.removeLast() }
                return Data(line)
            }
            guard line.count < maximumLineBytes else {
                throw JSONLineReaderError.lineTooLarge
            }
            line.append(byte)
        }
    }
}

public enum SidecarIO {
    private static let protocolDescriptor = dup(STDOUT_FILENO)
    private static let diagnosticDescriptor = dup(STDERR_FILENO)

    public static func silenceThirdPartyOutput() -> Bool {
        guard protocolDescriptor >= 0, diagnosticDescriptor >= 0 else { return false }
        let nullDescriptor = open("/dev/null", O_WRONLY | O_CLOEXEC)
        guard nullDescriptor >= 0 else { return false }
        defer { _ = close(nullDescriptor) }
        return dup2(nullDescriptor, STDOUT_FILENO) == STDOUT_FILENO
            && dup2(nullDescriptor, STDERR_FILENO) == STDERR_FILENO
    }

    public static func writeResponse(_ response: SidecarResponse, maximumBytes: Int) {
        do {
            let encoded = try SidecarProtocolCodec.encodeResponse(response)
            guard encoded.count <= maximumBytes else {
                writeDiagnostic(code: "response_too_large")
                return
            }
            var line = encoded
            line.append(0x0A)
            writeAll(line, to: protocolDescriptor, failureCode: "protocol_write_failed")
        } catch {
            writeDiagnostic(code: "response_encoding_failed")
        }
    }

    public static func writeDiagnostic(code: String) {
        let safeCode = String(code.utf8.prefix(48).map { byte -> Character in
            if (byte >= 48 && byte <= 57) || (byte >= 65 && byte <= 90) || (byte >= 97 && byte <= 122) {
                return Character(UnicodeScalar(byte))
            }
            return "_"
        })
        let message = Data("voiceflow-mlx-sidecar: \(safeCode)\n".utf8)
        writeAll(message, to: diagnosticDescriptor, failureCode: nil)
    }

    private static func writeAll(_ data: Data, to descriptor: Int32, failureCode: String?) {
        guard descriptor >= 0 else { return }
        let success = data.withUnsafeBytes { rawBuffer -> Bool in
            guard let base = rawBuffer.baseAddress else { return data.isEmpty }
            var offset = 0
            while offset < rawBuffer.count {
                let written = Darwin.write(descriptor, base.advanced(by: offset), rawBuffer.count - offset)
                if written < 0 {
                    if errno == EINTR { continue }
                    return false
                }
                if written == 0 { return false }
                offset += written
            }
            return true
        }
        if !success, let failureCode, failureCode != "protocol_write_failed" {
            writeDiagnostic(code: failureCode)
        }
    }
}
