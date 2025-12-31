//
//  HaloState.swift
//  Aether
//
//  State machine for Halo overlay animations.
//

import SwiftUI

/// Input mode selection for user choice before AI processing
enum InputModeChoice {
    case replace  // Cut original text, replace with AI response
    case append   // Copy original text, append AI response after it
}

enum HaloState: Equatable {
    case idle
    case awaitingInputMode(onSelect: (InputModeChoice) -> Void)  // Waiting for user to select input mode
    case listening
    case retrievingMemory  // Phase 9: Retrieving memories from database
    case processingWithAI(providerColor: Color, providerName: String?)  // Phase 9: AI provider is processing
    case processing(providerColor: Color, streamingText: String? = nil)  // Generic processing (backward compatibility)
    case typewriting(progress: Float)  // Phase 7.2: Typewriter animation in progress
    case success(finalText: String? = nil)
    case error(type: ErrorType, message: String, suggestion: String? = nil)  // Phase 7.4: Error with optional suggestion
    case permissionRequired(type: PermissionType)  // Permission prompt (replaces system NSAlert)

    // Equatable conformance
    static func == (lhs: HaloState, rhs: HaloState) -> Bool {
        switch (lhs, rhs) {
        case (.idle, .idle):
            return true
        case (.awaitingInputMode, .awaitingInputMode):
            // Closures can't be compared, so we just check if both are awaiting
            return true
        case (.listening, .listening):
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
        default:
            return false
        }
    }
}
