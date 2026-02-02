//
//  HaloState.swift
//  Aether
//
//  Simplified Halo state enum (V2 refactored).
//  8 unified states for cleaner state management.
//

import SwiftUI

// MARK: - HaloState (Simplified State Model)

/// Simplified Halo overlay states
/// Reduces complex state variants to 7 unified states for cleaner state management.
enum HaloState: Equatable {
    /// Hidden - Halo is not visible
    case idle

    /// Listening for clipboard/input
    case listening

    /// AI streaming response (covers: processing, retrievingMemory, planProgress)
    case streaming(StreamingContext)

    /// User confirmation required (covers: toolConfirmation, planConfirmation, taskGraphConfirmation)
    case confirmation(ConfirmationContext)

    /// Operation completed (covers: success, typewriting)
    case result(ResultContext)

    /// Error state
    case error(ErrorContext)

    /// History list view (// command)
    case historyList(HistoryListContext)

    /// Command list view (/ command)
    case commandList(CommandListContext)
}

// MARK: - HaloState State Query Helpers

extension HaloState {
    /// Check if state is idle
    var isIdle: Bool {
        if case .idle = self { return true }
        return false
    }

    /// Check if state is listening
    var isListening: Bool {
        if case .listening = self { return true }
        return false
    }

    /// Check if state is streaming
    var isStreaming: Bool {
        if case .streaming = self { return true }
        return false
    }

    /// Check if state is confirmation
    var isConfirmation: Bool {
        if case .confirmation = self { return true }
        return false
    }

    /// Check if state is result
    var isResult: Bool {
        if case .result = self { return true }
        return false
    }

    /// Check if state is error
    var isError: Bool {
        if case .error = self { return true }
        return false
    }

    /// Check if state is history list
    var isHistoryList: Bool {
        if case .historyList = self { return true }
        return false
    }

    /// Check if state is command list
    var isCommandList: Bool {
        if case .commandList = self { return true }
        return false
    }

    /// Check if state requires user interaction
    var isInteractive: Bool {
        switch self {
        case .confirmation, .error, .historyList, .commandList:
            return true
        default:
            return false
        }
    }
}

// MARK: - HaloState Window Size

extension HaloState {
    /// Computed window size based on current state
    var windowSize: NSSize {
        switch self {
        case .idle:
            return NSSize(width: 0, height: 0)
        case .listening:
            return NSSize(width: 80, height: 60)
        case .streaming(let ctx):
            switch ctx.phase {
            case .thinking:
                return NSSize(width: 80, height: 60)
            case .responding:
                return NSSize(width: 320, height: 100)
            case .toolExecuting:
                return NSSize(width: 280, height: 120)
            }
        case .confirmation:
            return NSSize(width: 340, height: 280)
        case .result:
            return NSSize(width: 280, height: 80)
        case .error:
            return NSSize(width: 320, height: 200)
        case .historyList:
            return NSSize(width: 380, height: 420)
        case .commandList:
            return NSSize(width: 380, height: 420)
        }
    }
}

// MARK: - Toast Type

/// Toast notification types (used by EventHandler and HaloToastView)
enum ToastType: Equatable {
    case info
    case warning
    case error

    /// Display name for accessibility
    var displayName: String {
        switch self {
        case .info: return L("toast.type.info")
        case .warning: return L("toast.type.warning")
        case .error: return L("toast.type.error")
        }
    }

    /// SF Symbol icon name
    var iconName: String {
        switch self {
        case .info: return "info.circle.fill"
        case .warning: return "exclamationmark.triangle.fill"
        case .error: return "xmark.circle.fill"
        }
    }

    /// Accent color for the toast type
    var accentColor: Color {
        switch self {
        case .info: return .blue
        case .warning: return .orange
        case .error: return .red
        }
    }
}

// MARK: - ErrorType to HaloErrorType Conversion

extension ErrorType {
    /// Convert FFI ErrorType to HaloErrorType
    func toHaloErrorType() -> HaloErrorType {
        switch self {
        case .network:
            return .network
        case .permission:
            return .provider  // Map permission to provider (closest match)
        case .quota:
            return .provider  // Map quota to provider (closest match)
        case .timeout:
            return .timeout
        case .unknown:
            return .unknown
        }
    }
}
