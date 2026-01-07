//
//  HaloState.swift
//  Aether
//
//  State machine for Halo overlay animations.
//

import SwiftUI

/// Toast notification types for Halo overlay
enum ToastType: Equatable {
    case info      // Blue accent, info.circle icon - for success/confirmation
    case warning   // Orange accent, exclamationmark.triangle icon - for warnings
    case error     // Red accent, xmark.circle icon - for errors

    /// SF Symbol icon name for each toast type
    var iconName: String {
        switch self {
        case .info:
            return "info.circle.fill"
        case .warning:
            return "exclamationmark.triangle.fill"
        case .error:
            return "xmark.circle.fill"
        }
    }

    /// Accent color for each toast type
    var accentColor: Color {
        switch self {
        case .info:
            return Color(red: 0, green: 0.478, blue: 1.0)  // #007AFF
        case .warning:
            return Color(red: 1.0, green: 0.584, blue: 0)  // #FF9500
        case .error:
            return Color(red: 1.0, green: 0.231, blue: 0.188)  // #FF3B30
        }
    }

    /// Display name for accessibility
    var displayName: String {
        switch self {
        case .info:
            return "Information"
        case .warning:
            return "Warning"
        case .error:
            return "Error"
        }
    }
}

enum HaloState: Equatable {
    case idle
    case listening
    case commandMode  // Command completion mode (add-command-completion-system)
    case retrievingMemory  // Retrieving memories from database
    case processingWithAI(providerColor: Color, providerName: String?)  // AI provider is processing
    case processing(providerColor: Color, streamingText: String? = nil)  // Generic processing
    case typewriting(progress: Float)  // Typewriter animation in progress
    case success(finalText: String? = nil)
    case error(type: ErrorType, message: String, suggestion: String? = nil)  // Error with optional suggestion
    case permissionRequired(type: PermissionType)  // Permission prompt
    case toast(type: ToastType, title: String, message: String, autoDismiss: Bool, onDismiss: (() -> Void)?)  // Toast notification
    case clarification(request: ClarificationRequest)  // Phantom Flow clarification (add-phantom-flow-interaction)

    // Equatable conformance
    static func == (lhs: HaloState, rhs: HaloState) -> Bool {
        switch (lhs, rhs) {
        case (.idle, .idle):
            return true
        case (.listening, .listening):
            return true
        case (.commandMode, .commandMode):
            return true
        case (.retrievingMemory, .retrievingMemory):
            return true
        case (.processingWithAI(let color1, let name1), .processingWithAI(let color2, let name2)):
            return color1 == color2 && name1 == name2
        case (.processing(let color1, let text1), .processing(let color2, let text2)):
            return color1 == color2 && text1 == text2
        case (.typewriting(let progress1), .typewriting(let progress2)):
            return progress1 == progress2
        case (.success(let text1), .success(let text2)):
            return text1 == text2
        case (.error(let type1, let msg1, let sug1), .error(let type2, let msg2, let sug2)):
            return type1 == type2 && msg1 == msg2 && sug1 == sug2
        case (.permissionRequired(let type1), .permissionRequired(let type2)):
            return type1 == type2
        case (.toast(let type1, let title1, let msg1, let auto1, _), .toast(let type2, let title2, let msg2, let auto2, _)):
            // Closures can't be compared, so we compare other fields
            return type1 == type2 && title1 == title2 && msg1 == msg2 && auto1 == auto2
        case (.clarification(let req1), .clarification(let req2)):
            return req1 == req2
        default:
            return false
        }
    }
}
