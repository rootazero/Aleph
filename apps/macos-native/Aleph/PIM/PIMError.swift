import Foundation

// MARK: - PIM Error Types

/// Errors specific to PIM (Personal Information Management) operations.
///
/// Maps to JSON-RPC error codes via `PIMErrorCode` for wire transport.
enum PIMError: Error, LocalizedError {
    case permissionDenied(String)
    case notFound(String)
    case scriptError(String)

    var errorDescription: String? {
        switch self {
        case .permissionDenied(let msg): return msg
        case .notFound(let msg): return msg
        case .scriptError(let msg): return msg
        }
    }
}

/// JSON-RPC error codes for PIM operations.
///
/// Uses the application-specific range (-32001 to -32099) to avoid
/// collision with standard JSON-RPC codes (-32600 to -32700).
enum PIMErrorCode {
    static let permissionDenied: Int32 = -32001
    static let notFound: Int32 = -32002
    static let validationFailed: Int32 = -32003
    static let scriptError: Int32 = -32004
}
