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
    case processing(providerColor: Color)
    case success
    case error

    // Equatable conformance
    static func == (lhs: HaloState, rhs: HaloState) -> Bool {
        switch (lhs, rhs) {
        case (.idle, .idle):
            return true
        case (.listening, .listening):
            return true
        case (.processing(let color1), .processing(let color2)):
            return color1 == color2
        case (.success, .success):
            return true
        case (.error, .error):
            return true
        default:
            return false
        }
    }
}
