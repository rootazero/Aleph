//
//  HexSegment.swift
//  Aether
//
//  Hexagonal segment shape for Jarvis theme
//

import SwiftUI

/// Hexagonal segment that can be rotated and animated
struct HexSegment: Shape {
    let index: Int

    func path(in rect: CGRect) -> Path {
        var path = Path()
        let center = CGPoint(x: rect.midX, y: rect.midY)
        let radius = min(rect.width, rect.height) / 2
        let innerRadius = radius * 0.6

        // Calculate angles for this segment (60 degree segment)
        let startAngle = CGFloat(index) * .pi / 3.0 - .pi / 2.0
        let endAngle = startAngle + .pi / 3.0

        // Outer edge
        path.move(to: CGPoint(
            x: center.x + radius * cos(startAngle),
            y: center.y + radius * sin(startAngle)
        ))
        path.addLine(to: CGPoint(
            x: center.x + radius * cos(endAngle),
            y: center.y + radius * sin(endAngle)
        ))

        // Inner edge
        path.addLine(to: CGPoint(
            x: center.x + innerRadius * cos(endAngle),
            y: center.y + innerRadius * sin(endAngle)
        ))
        path.addLine(to: CGPoint(
            x: center.x + innerRadius * cos(startAngle),
            y: center.y + innerRadius * sin(startAngle)
        ))

        path.closeSubpath()
        return path
    }
}
