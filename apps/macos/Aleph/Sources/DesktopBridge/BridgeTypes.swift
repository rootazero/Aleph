//
//  BridgeTypes.swift
//  Aleph
//
//  Minimal type definitions for the Desktop Bridge JSON-RPC server.
//

import Foundation

struct ScreenRegion: Codable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

struct CanvasPosition: Codable {
    let x: Double
    let y: Double
    let width: Double
    let height: Double
}

enum MouseButtonType: String, Codable {
    case left, right, middle
}
