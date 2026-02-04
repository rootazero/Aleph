//
//  ErrorType+Extensions.swift
//  Aether
//
//  UI extensions for Rust-generated ErrorType enum.
//

import SwiftUI

// MARK: - ErrorType UI Extensions

extension ErrorType {

    /// System symbol name for the error type
    var iconName: String {
        switch self {
        case .network:
            return "wifi.exclamationmark"
        case .permission:
            return "lock.shield.fill"
        case .quota:
            return "clock.badge.exclamationmark"
        case .timeout:
            return "hourglass.bottomhalf.filled"
        case .unknown:
            return "questionmark.circle.fill"
        }
    }

    /// Localized display name for the error type
    var displayName: String {
        switch self {
        case .network:
            return L("error.type.network")
        case .permission:
            return L("error.type.permission")
        case .quota:
            return L("error.type.quota")
        case .timeout:
            return L("error.type.timeout")
        case .unknown:
            return L("error.type.unknown")
        }
    }
}
