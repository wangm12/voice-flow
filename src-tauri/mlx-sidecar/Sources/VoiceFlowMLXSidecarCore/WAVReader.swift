import Darwin
import Foundation

public struct ValidatedWAV {
    public let samples: [Float]
    public let sampleCount: Int
    public let durationSeconds: Double

    public init(samples: [Float], sampleCount: Int, durationSeconds: Double) {
        self.samples = samples
        self.sampleCount = sampleCount
        self.durationSeconds = durationSeconds
    }
}

public enum WAVValidationError: Error {
    case invalid
    case tooLarge
    case tooLong
}

public enum PrivateAudioFileReader {
    public static let sampleRate = 16_000
    public static let maximumFileBytes: off_t = 32 * 1024 * 1024
    public static let maximumDurationSeconds = 15 * 60

    public static func readValidatedWAV(
        at rawPath: String,
        audioRoot: URL,
        maximumBytes: Int
    ) throws -> ValidatedWAV {
        let fileName = URL(fileURLWithPath: rawPath).lastPathComponent
        guard rawPath.utf8.count <= 4_096,
              rawPath.hasPrefix("/"),
              maximumBytes >= 44,
              maximumBytes <= Int(maximumFileBytes),
              !fileName.isEmpty,
              fileName.lowercased().hasSuffix(".wav"),
              fileName.utf8.allSatisfy({
                  ($0 >= 48 && $0 <= 57) || ($0 >= 65 && $0 <= 90) || ($0 >= 97 && $0 <= 122)
                      || $0 == 45 || $0 == 46 || $0 == 95
              }) else {
            throw WAVValidationError.invalid
        }

        let expectedRoot = audioRoot.standardizedFileURL.resolvingSymlinksInPath()
        let requestedURL = URL(fileURLWithPath: rawPath).standardizedFileURL
        let requestedParent = requestedURL.deletingLastPathComponent().resolvingSymlinksInPath()
        var rootStatus = stat()
        guard lstat(expectedRoot.path, &rootStatus) == 0,
              (rootStatus.st_mode & mode_t(S_IFMT)) == mode_t(S_IFDIR),
              rootStatus.st_uid == geteuid(),
              (rootStatus.st_mode & mode_t(0o777)) == mode_t(0o700),
              requestedParent.path == expectedRoot.path,
              requestedURL.lastPathComponent == fileName else {
            throw WAVValidationError.invalid
        }

        var fileStatus = stat()
        guard lstat(requestedURL.path, &fileStatus) == 0,
              (fileStatus.st_mode & mode_t(S_IFMT)) == mode_t(S_IFREG),
              fileStatus.st_uid == geteuid(),
              (fileStatus.st_mode & mode_t(0o777)) == mode_t(0o600),
              fileStatus.st_size >= 44 else {
            throw WAVValidationError.invalid
        }
        guard fileStatus.st_size <= maximumFileBytes,
              fileStatus.st_size <= off_t(maximumBytes) else {
            throw WAVValidationError.tooLarge
        }

        let descriptor = open(requestedURL.path, O_RDONLY | O_CLOEXEC | O_NOFOLLOW)
        guard descriptor >= 0 else {
            throw WAVValidationError.invalid
        }
        defer { _ = close(descriptor) }

        var openedStatus = stat()
        guard fstat(descriptor, &openedStatus) == 0,
              openedStatus.st_dev == fileStatus.st_dev,
              openedStatus.st_ino == fileStatus.st_ino,
              openedStatus.st_uid == geteuid(),
              (openedStatus.st_mode & mode_t(0o077)) == 0,
              openedStatus.st_size == fileStatus.st_size else {
            throw WAVValidationError.invalid
        }

        let fileData = try readExactly(descriptor, byteCount: Int(openedStatus.st_size))
        var trailingByte: UInt8 = 0
        guard read(descriptor, &trailingByte, 1) == 0 else {
            throw WAVValidationError.invalid
        }

        return try parsePCM16WAV(fileData)
    }

    private static func readExactly(_ descriptor: Int32, byteCount: Int) throws -> Data {
        guard byteCount >= 0 else { throw WAVValidationError.invalid }
        var data = Data(count: byteCount)
        let completed = data.withUnsafeMutableBytes { rawBuffer -> Bool in
            guard let base = rawBuffer.baseAddress else { return byteCount == 0 }
            var offset = 0
            while offset < byteCount {
                let result = read(descriptor, base.advanced(by: offset), byteCount - offset)
                if result < 0 {
                    if errno == EINTR { continue }
                    return false
                }
                if result == 0 { return false }
                offset += result
            }
            return true
        }
        guard completed else { throw WAVValidationError.invalid }
        return data
    }

    private static func parsePCM16WAV(_ data: Data) throws -> ValidatedWAV {
        guard data.count >= 44,
              fourCC(data, at: 0) == "RIFF",
              fourCC(data, at: 8) == "WAVE",
              let riffLength = uint32LE(data, at: 4),
              UInt64(riffLength) + 8 == UInt64(data.count) else {
            throw WAVValidationError.invalid
        }

        var offset = 12
        var format: (encoding: UInt16, channels: UInt16, sampleRate: UInt32, byteRate: UInt32, blockAlign: UInt16, bits: UInt16)?
        var pcmRange: Range<Int>?

        while offset <= data.count - 8 {
            guard let chunkLength = uint32LE(data, at: offset + 4) else {
                throw WAVValidationError.invalid
            }
            let length = Int(chunkLength)
            let contentStart = offset + 8
            let (contentEnd, overflow) = contentStart.addingReportingOverflow(length)
            guard !overflow, contentEnd <= data.count else {
                throw WAVValidationError.invalid
            }

            switch fourCC(data, at: offset) {
            case "fmt ":
                guard format == nil,
                      length >= 16,
                      let encoding = uint16LE(data, at: contentStart),
                      let channels = uint16LE(data, at: contentStart + 2),
                      let sampleRate = uint32LE(data, at: contentStart + 4),
                      let byteRate = uint32LE(data, at: contentStart + 8),
                      let blockAlign = uint16LE(data, at: contentStart + 12),
                      let bits = uint16LE(data, at: contentStart + 14) else {
                    throw WAVValidationError.invalid
                }
                format = (encoding, channels, sampleRate, byteRate, blockAlign, bits)
            case "data":
                guard pcmRange == nil, length > 0 else {
                    throw WAVValidationError.invalid
                }
                pcmRange = contentStart..<contentEnd
            default:
                break
            }

            let paddedEnd = contentEnd + (length & 1)
            guard paddedEnd <= data.count else {
                throw WAVValidationError.invalid
            }
            offset = paddedEnd
        }

        guard offset == data.count,
              let format,
              let pcmRange,
              format.encoding == 1,
              format.channels == 1,
              format.sampleRate == UInt32(sampleRate),
              format.byteRate == UInt32(sampleRate * 2),
              format.blockAlign == 2,
              format.bits == 16,
              pcmRange.count.isMultiple(of: 2) else {
            throw WAVValidationError.invalid
        }

        let sampleCount = pcmRange.count / 2
        guard sampleCount > 0 else { throw WAVValidationError.invalid }
        guard sampleCount <= sampleRate * maximumDurationSeconds else {
            throw WAVValidationError.tooLong
        }

        var samples = [Float]()
        samples.reserveCapacity(sampleCount)
        var byteOffset = pcmRange.lowerBound
        while byteOffset < pcmRange.upperBound {
            let bits = UInt16(data[byteOffset]) | (UInt16(data[byteOffset + 1]) << 8)
            samples.append(Float(Int16(bitPattern: bits)) / 32_768.0)
            byteOffset += 2
        }

        return ValidatedWAV(
            samples: samples,
            sampleCount: sampleCount,
            durationSeconds: Double(sampleCount) / Double(sampleRate)
        )
    }

    private static func fourCC(_ data: Data, at offset: Int) -> String? {
        guard offset >= 0, offset <= data.count - 4 else { return nil }
        return String(bytes: data[offset..<(offset + 4)], encoding: .ascii)
    }

    private static func uint16LE(_ data: Data, at offset: Int) -> UInt16? {
        guard offset >= 0, offset <= data.count - 2 else { return nil }
        return UInt16(data[offset]) | (UInt16(data[offset + 1]) << 8)
    }

    private static func uint32LE(_ data: Data, at offset: Int) -> UInt32? {
        guard offset >= 0, offset <= data.count - 4 else { return nil }
        return UInt32(data[offset])
            | (UInt32(data[offset + 1]) << 8)
            | (UInt32(data[offset + 2]) << 16)
            | (UInt32(data[offset + 3]) << 24)
    }
}
