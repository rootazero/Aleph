//
//  HaloState.swift
//  Aether
//
//  State machine for Halo overlay animations.
//

import SwiftUI

enum HaloState: Equatable {
    case idle
    case listening
    case retrievingMemory  // Phase 9: Retrieving memories from database
    case processingWithAI(providerColor: Color, providerName: String?)  // Phase 9: AI provider is processing
    case processing(providerColor: Color, streamingText: String? = nil)  // Generic processing (backward compatibility)
    case typewriting(progress: Float)  // Phase 7.2: Typewriter animation in progress
    case success(finalText: String? = nil)
    case error(type: ErrorType, message: String)

    // Equatable conformance
    static func == (lhs: HaloState, rhs: HaloState) -> Bool {
        switch (lhs, rhs) {
        case (.idle, .idle):
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
        case (.error(let type1, let msg1), .error(let type2, let msg2)):
            return type1 == type2 && msg1 == msg2
        default:
            return false
        }
    }
}
