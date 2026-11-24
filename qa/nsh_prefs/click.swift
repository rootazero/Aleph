// Clicks the left mouse button at a screen point (points, top-left origin):
// `click <x> <y>`. System Events' `click at` goes through accessibility and
// does not reach NSH views; a posted mouse event does.
import CoreGraphics
import Foundation

let arguments = CommandLine.arguments
guard arguments.count == 3, let x = Double(arguments[1]), let y = Double(arguments[2]) else {
    FileHandle.standardError.write("usage: click <x> <y>\n".data(using: .utf8)!)
    exit(2)
}
let point = CGPoint(x: x, y: y)
for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
    let event = CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)
    event?.post(tap: .cghidEventTap)
    usleep(80_000)
}
