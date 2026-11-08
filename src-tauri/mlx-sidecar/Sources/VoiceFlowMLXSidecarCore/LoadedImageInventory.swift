import Foundation

// Bounds the complete dyld path|UUID inventory before it enters the attestation wire response.
enum LoadedImageInventoryError: Error, Equatable {
    case imageCountExceeded(count: Int)
    case imageNameUnavailable(index: Int)
    case imageNameTooLong(index: Int, bytes: Int)
    case imageHeaderUnavailable(index: Int)
    case invalidImageHeader(index: Int)
    case malformedLoadCommands(index: Int)
    case missingUUID(index: Int)
    case imageEntryTooLarge(index: Int, bytes: Int)
    case totalBytesExceeded(count: Int, bytes: Int)

    var diagnosticCode: String {
        switch self {
        case .imageCountExceeded(let count):
            return "model_attestation_images_count_\(count)"
        case .imageNameUnavailable(let index):
            return "model_attestation_images_name_\(index)"
        case .imageNameTooLong(let index, let bytes):
            return "model_attestation_images_name_\(index)_\(bytes)"
        case .imageHeaderUnavailable(let index):
            return "model_attestation_images_header_\(index)"
        case .invalidImageHeader(let index):
            return "model_attestation_images_magic_\(index)"
        case .malformedLoadCommands(let index):
            return "model_attestation_images_commands_\(index)"
        case .missingUUID(let index):
            return "model_attestation_images_uuid_\(index)"
        case .imageEntryTooLarge(let index, let bytes):
            return "model_attestation_images_entry_\(index)_\(bytes)"
        case .totalBytesExceeded(let count, let bytes):
            return "model_attestation_images_total_\(count)_\(bytes)"
        }
    }
}

enum LoadedImageInventory {
    static let maximumImageCount = 4_096
    static let maximumEntryUTF8Bytes = 8_192
    static let maximumTotalUTF8Bytes = 256 * 1_024

    static func validateImageCount(_ count: Int) throws {
        guard count >= 0, count <= maximumImageCount else {
            throw LoadedImageInventoryError.imageCountExceeded(count: count)
        }
    }

    static func append(
        _ image: String,
        index: Int,
        to images: inout [String],
        totalUTF8Bytes: inout Int
    ) throws {
        let (nextCount, countOverflow) = images.count.addingReportingOverflow(1)
        guard !countOverflow, nextCount <= maximumImageCount else {
            throw LoadedImageInventoryError.imageCountExceeded(count: nextCount)
        }

        let entryBytes = image.utf8.count
        guard entryBytes <= maximumEntryUTF8Bytes else {
            throw LoadedImageInventoryError.imageEntryTooLarge(index: index, bytes: entryBytes)
        }
        let (nextTotalBytes, totalOverflow) = totalUTF8Bytes.addingReportingOverflow(entryBytes)
        guard !totalOverflow, nextTotalBytes <= maximumTotalUTF8Bytes else {
            throw LoadedImageInventoryError.totalBytesExceeded(
                count: nextCount,
                bytes: totalOverflow ? Int.max : nextTotalBytes
            )
        }

        images.append(image)
        totalUTF8Bytes = nextTotalBytes
    }
}
