// OCR and drive the fixture's own Aleph window using normalized screenshot coordinates.
import CoreGraphics
import Foundation
import Vision

let arguments = CommandLine.arguments
if arguments.count == 3 && arguments[1] == "read" {
    let request = VNRecognizeTextRequest()
    request.recognitionLevel = .accurate
    request.usesLanguageCorrection = false
    try VNImageRequestHandler(url: URL(fileURLWithPath: arguments[2])).perform([request])
    let rows: [[String: Any]] = (request.results ?? []).compactMap { observation in
        guard let text = observation.topCandidates(1).first?.string else { return nil }
        let bounds = observation.boundingBox
        return ["text": text, "x": bounds.midX, "y": 1 - bounds.midY]
    }
    print(String(decoding: try JSONSerialization.data(withJSONObject: rows), as: UTF8.self))
} else {
    guard arguments.count == 5, ["click", "up"].contains(arguments[1]),
          let pid = Int32(arguments[2]), let x = Double(arguments[3]), let y = Double(arguments[4]),
          (0...1).contains(x), (0...1).contains(y),
          let windows = CGWindowListCopyWindowInfo([.optionOnScreenOnly], kCGNullWindowID) as? [[String: Any]],
          let window = windows.first(where: {
              $0[kCGWindowOwnerPID as String] as? Int32 == pid &&
              $0[kCGWindowLayer as String] as? Int == 0 &&
              $0[kCGWindowName as String] as? String == "Aleph"
          }), let rawBounds = window[kCGWindowBounds as String] as? [String: Any],
          let bounds = CGRect(dictionaryRepresentation: rawBounds as CFDictionary)
    else {
        FileHandle.standardError.write(Data("usage: native-view read <image> | click/up <fixture pid> <x fraction> <y fraction>\n".utf8))
        exit(2)
    }
    let point = CGPoint(x: bounds.minX + x * bounds.width, y: bounds.minY + y * bounds.height)
    if arguments[1] == "click" {
        for type in [CGEventType.mouseMoved, .leftMouseDown, .leftMouseUp] {
            CGEvent(mouseEventSource: nil, mouseType: type, mouseCursorPosition: point, mouseButton: .left)?.post(tap: .cghidEventTap)
            usleep(80_000)
        }
    } else {
        let event = CGEvent(scrollWheelEvent2Source: nil, units: .pixel, wheelCount: 1, wheel1: 5000, wheel2: 0, wheel3: 0)
        event?.location = point
        event?.post(tap: .cghidEventTap)
    }
}
