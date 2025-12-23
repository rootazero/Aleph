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
    case processing(providerColor: Color, streamingText: String? = nil)
    case success(finalText: String? = nil)
    case error(type: ErrorType, message: String)

    // Equatable conformance
    static func == (lhs: HaloState, rhs: HaloState) -> Bool {
        switch (lhs, rhs) {
        case (.idle, .idle):
            return true
        case (.listening, .listening):
            return true
        case (.processing(let color1, let text1), .processing(let color2, let text2)):
            return color1 == color2 && text1 == text2
        case (.success(let text1), .success(let text2)):
            return text1 == text2
        case (.error(let type1, let msg1), .error(let type2, let msg2)):
            return type1 == type2 && msg1 == msg2
        default:
            return false
        }
    }
}
