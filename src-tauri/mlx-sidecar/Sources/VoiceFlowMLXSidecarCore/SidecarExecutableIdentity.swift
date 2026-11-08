import CryptoKit
import Darwin
import Foundation
import MachO

enum SidecarExecutableIdentityError: Error, Equatable {
    case executablePathUnavailable
    case invalidExecutablePath
    case invalidArtifact
}

enum SidecarExecutableIdentity {
    private static let maximumPathBytes = 32 * 1_024
    private static let maximumArtifactBytes: UInt64 = 256 * 1_024 * 1_024

    static let metalResourceNames = [
        "mlx.metallib",
        "mlx-swift_Cmlx.bundle/Contents/Resources/default.metallib",
    ]

    static func currentExecutableURL() throws -> URL {
        var probe = [CChar](repeating: 0, count: 1)
        var requiredSize: UInt32 = 1
        let probeResult = probe.withUnsafeMutableBufferPointer { buffer -> Int32 in
            guard let baseAddress = buffer.baseAddress else { return Int32.min }
            return _NSGetExecutablePath(baseAddress, &requiredSize)
        }
        guard probeResult == -1,
              requiredSize > UInt32(probe.count),
              requiredSize <= UInt32(maximumPathBytes) else {
            throw SidecarExecutableIdentityError.executablePathUnavailable
        }

        var pathBuffer = [CChar](repeating: 0, count: Int(requiredSize))
        var pathBufferSize = requiredSize
        let pathResult = pathBuffer.withUnsafeMutableBufferPointer { buffer -> Int32 in
            guard let baseAddress = buffer.baseAddress else { return Int32.min }
            return _NSGetExecutablePath(baseAddress, &pathBufferSize)
        }
        guard pathResult == 0,
              pathBufferSize > 1,
              pathBufferSize <= requiredSize,
              let terminator = pathBuffer.firstIndex(of: 0), terminator > 0 else {
            throw SidecarExecutableIdentityError.executablePathUnavailable
        }

        let pathBytes = pathBuffer[..<terminator].map { UInt8(bitPattern: $0) }
        guard let path = String(bytes: pathBytes, encoding: .utf8) else {
            throw SidecarExecutableIdentityError.executablePathUnavailable
        }
        return try canonicalExecutableURL(path: path)
    }

    static func canonicalExecutableURL(path: String) throws -> URL {
        guard !path.isEmpty,
              path.utf8.count <= maximumPathBytes,
              path.hasPrefix("/") else {
            throw SidecarExecutableIdentityError.invalidExecutablePath
        }

        let executable = URL(fileURLWithPath: path, isDirectory: false)
            .standardizedFileURL
            .resolvingSymlinksInPath()
        guard executable.isFileURL,
              executable.path.hasPrefix("/"),
              !executable.path.isEmpty,
              executable.path.utf8.count <= maximumPathBytes else {
            throw SidecarExecutableIdentityError.invalidExecutablePath
        }
        return executable
    }

    static func resourceRoot(for executableURL: URL) throws -> URL {
        guard executableURL.isFileURL,
              executableURL.path.hasPrefix("/"),
              !executableURL.path.isEmpty,
              executableURL.path.utf8.count <= maximumPathBytes else {
            throw SidecarExecutableIdentityError.invalidExecutablePath
        }

        let canonicalExecutable = try canonicalExecutableURL(path: executableURL.path)
        let root = canonicalExecutable.deletingLastPathComponent()
        guard root.isFileURL, root.path.hasPrefix("/") else {
            throw SidecarExecutableIdentityError.invalidExecutablePath
        }
        return root
    }

    static func metalResourceURLs(for executableURL: URL) throws -> [(name: String, url: URL)] {
        let root = try resourceRoot(for: executableURL)
        return metalResourceNames.map { name in
            (name: name, url: root.appendingPathComponent(name))
        }
    }

    static func metalDigests(for executableURL: URL) throws -> [String: String] {
        var digests: [String: String] = [:]
        for resource in try metalResourceURLs(for: executableURL) {
            digests[resource.name] = try digestRegularFile(at: resource.url)
        }
        return digests
    }

    static func digestRegularFile(at url: URL) throws -> String {
        guard url.isFileURL, url.path.hasPrefix("/") else {
            throw SidecarExecutableIdentityError.invalidArtifact
        }

        do {
            let attributes = try FileManager.default.attributesOfItem(atPath: url.path)
            guard attributes[.type] as? FileAttributeType == .typeRegular,
                  let size = attributes[.size] as? NSNumber,
                  size.uint64Value > 0,
                  size.uint64Value <= maximumArtifactBytes else {
                throw SidecarExecutableIdentityError.invalidArtifact
            }
            let data = try Data(contentsOf: url, options: [.mappedIfSafe])
            let after = try FileManager.default.attributesOfItem(atPath: url.path)
            guard data.count == size.intValue,
                  after[.size] as? NSNumber == size,
                  after[.modificationDate] as? Date == attributes[.modificationDate] as? Date,
                  after[.systemFileNumber] as? NSNumber == attributes[.systemFileNumber] as? NSNumber,
                  after[.systemNumber] as? NSNumber == attributes[.systemNumber] as? NSNumber else {
                throw SidecarExecutableIdentityError.invalidArtifact
            }
            return SHA256.hash(data: data).map { String(format: "%02x", $0) }.joined()
        } catch let error as SidecarExecutableIdentityError {
            throw error
        } catch {
            throw SidecarExecutableIdentityError.invalidArtifact
        }
    }
}
