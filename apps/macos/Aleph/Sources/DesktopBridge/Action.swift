// Action.swift
// Provides mouse, keyboard, app launch, and window management capabilities for DesktopBridge.

import AppKit
import CoreGraphics
import Foundation

/// Provides action capabilities: mouse events, keyboard events, app launch, window management.
final class Action: @unchecked Sendable {
    static let shared = Action()

    // MARK: - Ref Resolution

    /// Resolve a target from either a ref ID or explicit coordinates.
    private func resolveTarget(refId: String?, x: Double?, y: Double?) -> Result<CGPoint, Error> {
        if let refId = refId {
            switch RefStore.shared.resolve(refId) {
            case .success(let point):
                return .success(point)
            case .failure(let err):
                return .failure(err)
            }
        }
        if let x = x, let y = y {
            return .success(CGPoint(x: x, y: y))
        }
        return .failure(NSError(domain: "Action", code: 10,
                                userInfo: [NSLocalizedDescriptionKey: "Either ref or (x, y) coordinates required"]))
    }

    // MARK: - Mouse

    func click(x: Double, y: Double, button: String) async -> Result<Any, Error> {
        let point = CGPoint(x: x, y: y)
        let (downType, upType, cgButton): (CGEventType, CGEventType, CGMouseButton)
        switch button.lowercased() {
        case "right":
            downType = .rightMouseDown; upType = .rightMouseUp; cgButton = .right
        case "middle":
            downType = .otherMouseDown; upType = .otherMouseUp; cgButton = .center
        default:
            downType = .leftMouseDown; upType = .leftMouseUp; cgButton = .left
        }

        guard let source = CGEventSource(stateID: .hidSystemState),
              let down = CGEvent(mouseEventSource: source, mouseType: downType,
                                 mouseCursorPosition: point, mouseButton: cgButton),
              let up = CGEvent(mouseEventSource: source, mouseType: upType,
                               mouseCursorPosition: point, mouseButton: cgButton)
        else {
            return .failure(NSError(domain: "Action", code: 1,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create mouse events"]))
        }

        down.post(tap: .cghidEventTap)
        try? await Task.sleep(nanoseconds: 50_000_000) // 50ms hold
        up.post(tap: .cghidEventTap)

        return .success(["clicked": true, "x": x, "y": y, "button": button] as [String: Any])
    }

    /// Click with ref-based or coordinate-based targeting.
    func clickTarget(refId: String?, x: Double?, y: Double?, button: String) async -> Result<Any, Error> {
        switch resolveTarget(refId: refId, x: x, y: y) {
        case .success(let point):
            return await click(x: point.x, y: point.y, button: button)
        case .failure(let err):
            return .failure(err)
        }
    }

    // MARK: - Double Click

    func doubleClick(refId: String?, x: Double?, y: Double?, button: String) async -> Result<Any, Error> {
        switch resolveTarget(refId: refId, x: x, y: y) {
        case .failure(let err):
            return .failure(err)
        case .success(let point):
            let (downType, upType, cgButton): (CGEventType, CGEventType, CGMouseButton)
            switch button.lowercased() {
            case "right":
                downType = .rightMouseDown; upType = .rightMouseUp; cgButton = .right
            case "middle":
                downType = .otherMouseDown; upType = .otherMouseUp; cgButton = .center
            default:
                downType = .leftMouseDown; upType = .leftMouseUp; cgButton = .left
            }

            guard let source = CGEventSource(stateID: .hidSystemState),
                  let down = CGEvent(mouseEventSource: source, mouseType: downType,
                                     mouseCursorPosition: point, mouseButton: cgButton),
                  let up = CGEvent(mouseEventSource: source, mouseType: upType,
                                   mouseCursorPosition: point, mouseButton: cgButton)
            else {
                return .failure(NSError(domain: "Action", code: 1,
                                       userInfo: [NSLocalizedDescriptionKey: "Failed to create mouse events"]))
            }

            // First click
            down.setIntegerValueField(.mouseEventClickState, value: 1)
            up.setIntegerValueField(.mouseEventClickState, value: 1)
            down.post(tap: .cghidEventTap)
            try? await Task.sleep(nanoseconds: 50_000_000)
            up.post(tap: .cghidEventTap)
            try? await Task.sleep(nanoseconds: 50_000_000)

            // Second click
            guard let down2 = CGEvent(mouseEventSource: source, mouseType: downType,
                                      mouseCursorPosition: point, mouseButton: cgButton),
                  let up2 = CGEvent(mouseEventSource: source, mouseType: upType,
                                    mouseCursorPosition: point, mouseButton: cgButton)
            else {
                return .failure(NSError(domain: "Action", code: 1,
                                       userInfo: [NSLocalizedDescriptionKey: "Failed to create mouse events"]))
            }
            down2.setIntegerValueField(.mouseEventClickState, value: 2)
            up2.setIntegerValueField(.mouseEventClickState, value: 2)
            down2.post(tap: .cghidEventTap)
            try? await Task.sleep(nanoseconds: 50_000_000)
            up2.post(tap: .cghidEventTap)

            return .success(["double_clicked": true, "x": point.x, "y": point.y, "button": button] as [String: Any])
        }
    }

    // MARK: - Scroll

    func scroll(refId: String?, x: Double?, y: Double?, deltaX: Double, deltaY: Double) async -> Result<Any, Error> {
        if refId != nil || (x != nil && y != nil) {
            switch resolveTarget(refId: refId, x: x, y: y) {
            case .success(let point):
                guard let source = CGEventSource(stateID: .hidSystemState),
                      let moveEvent = CGEvent(mouseEventSource: source, mouseType: .mouseMoved,
                                              mouseCursorPosition: point, mouseButton: .left)
                else {
                    return .failure(NSError(domain: "Action", code: 8,
                                           userInfo: [NSLocalizedDescriptionKey: "Failed to create move event"]))
                }
                moveEvent.post(tap: .cghidEventTap)
                try? await Task.sleep(nanoseconds: 10_000_000)
            case .failure(let err):
                return .failure(err)
            }
        }

        guard let source = CGEventSource(stateID: .hidSystemState),
              let scrollEvent = CGEvent(scrollWheelEvent2Source: source,
                                        units: .pixel,
                                        wheelCount: 2,
                                        wheel1: Int32(deltaY),
                                        wheel2: Int32(deltaX))
        else {
            return .failure(NSError(domain: "Action", code: 9,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create scroll event"]))
        }
        scrollEvent.post(tap: .cghidEventTap)

        return .success(["scrolled": true, "delta_x": deltaX, "delta_y": deltaY] as [String: Any])
    }

    // MARK: - Hover

    func hover(refId: String?, x: Double?, y: Double?) async -> Result<Any, Error> {
        switch resolveTarget(refId: refId, x: x, y: y) {
        case .failure(let err):
            return .failure(err)
        case .success(let point):
            guard let source = CGEventSource(stateID: .hidSystemState),
                  let moveEvent = CGEvent(mouseEventSource: source, mouseType: .mouseMoved,
                                          mouseCursorPosition: point, mouseButton: .left)
            else {
                return .failure(NSError(domain: "Action", code: 11,
                                       userInfo: [NSLocalizedDescriptionKey: "Failed to create move event"]))
            }
            moveEvent.post(tap: .cghidEventTap)
            return .success(["hovered": true, "x": point.x, "y": point.y] as [String: Any])
        }
    }

    // MARK: - Drag

    func drag(startRefId: String?, startX: Double?, startY: Double?,
              endRefId: String?, endX: Double?, endY: Double?,
              durationMs: UInt64?) async -> Result<Any, Error> {
        let startPoint: CGPoint
        switch resolveTarget(refId: startRefId, x: startX, y: startY) {
        case .success(let p): startPoint = p
        case .failure(let err): return .failure(err)
        }

        let endPoint: CGPoint
        switch resolveTarget(refId: endRefId, x: endX, y: endY) {
        case .success(let p): endPoint = p
        case .failure(let err): return .failure(err)
        }

        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return .failure(NSError(domain: "Action", code: 12,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create event source"]))
        }

        let duration = durationMs ?? 300
        let steps = max(Int(duration / 16), 5)
        let sleepPerStep = UInt64(duration) * 1_000_000 / UInt64(steps)

        guard let down = CGEvent(mouseEventSource: source, mouseType: .leftMouseDown,
                                 mouseCursorPosition: startPoint, mouseButton: .left)
        else {
            return .failure(NSError(domain: "Action", code: 12,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create mouse down"]))
        }
        down.post(tap: .cghidEventTap)
        try? await Task.sleep(nanoseconds: 20_000_000)

        for i in 1...steps {
            let t = Double(i) / Double(steps)
            let ix = startPoint.x + (endPoint.x - startPoint.x) * t
            let iy = startPoint.y + (endPoint.y - startPoint.y) * t
            let pt = CGPoint(x: ix, y: iy)

            guard let dragEvent = CGEvent(mouseEventSource: source, mouseType: .leftMouseDragged,
                                          mouseCursorPosition: pt, mouseButton: .left)
            else { continue }
            dragEvent.post(tap: .cghidEventTap)
            try? await Task.sleep(nanoseconds: sleepPerStep)
        }

        guard let up = CGEvent(mouseEventSource: source, mouseType: .leftMouseUp,
                               mouseCursorPosition: endPoint, mouseButton: .left)
        else {
            return .failure(NSError(domain: "Action", code: 12,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create mouse up"]))
        }
        up.post(tap: .cghidEventTap)

        return .success([
            "dragged": true,
            "start": ["x": startPoint.x, "y": startPoint.y],
            "end": ["x": endPoint.x, "y": endPoint.y],
        ] as [String: Any])
    }

    // MARK: - Keyboard

    func typeText(_ text: String) async -> Result<Any, Error> {
        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return .failure(NSError(domain: "Action", code: 2,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create event source"]))
        }

        for scalar in text.unicodeScalars {
            guard let event = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: true) else { continue }
            // keyboardSetUnicodeString requires UniChar (UInt16); encode scalar as UTF-16
            let utf16Units = Character(scalar).utf16.map { UniChar($0) }
            var units = utf16Units
            event.keyboardSetUnicodeString(stringLength: utf16Units.count, unicodeString: &units)
            event.post(tap: .cghidEventTap)

            if let up = CGEvent(keyboardEventSource: source, virtualKey: 0, keyDown: false) {
                up.post(tap: .cghidEventTap)
            }

            try? await Task.sleep(nanoseconds: 10_000_000) // 10ms between chars
        }

        return .success(["typed": text.count] as [String: Any])
    }

    func keyCombo(keys: [String]) async -> Result<Any, Error> {
        guard let source = CGEventSource(stateID: .hidSystemState) else {
            return .failure(NSError(domain: "Action", code: 3,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create event source"]))
        }

        var flags: CGEventFlags = []
        var mainKey: CGKeyCode = 0

        for key in keys {
            switch key.lowercased() {
            case "cmd", "command":      flags.insert(.maskCommand)
            case "shift":               flags.insert(.maskShift)
            case "opt", "alt":          flags.insert(.maskAlternate)
            case "ctrl", "control":     flags.insert(.maskControl)
            default:
                mainKey = keyNameToCode(key)
            }
        }

        guard let down = CGEvent(keyboardEventSource: source, virtualKey: mainKey, keyDown: true),
              let up = CGEvent(keyboardEventSource: source, virtualKey: mainKey, keyDown: false)
        else {
            return .failure(NSError(domain: "Action", code: 4,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create key events"]))
        }

        down.flags = flags
        up.flags = flags
        down.post(tap: .cghidEventTap)
        try? await Task.sleep(nanoseconds: 50_000_000)
        up.post(tap: .cghidEventTap)

        return .success(["keys": keys] as [String: Any])
    }

    // MARK: - Paste

    func paste(text: String) async -> Result<Any, Error> {
        let pasteboard = NSPasteboard.general
        pasteboard.clearContents()
        pasteboard.setString(text, forType: .string)
        return await keyCombo(keys: ["cmd", "v"])
    }

    /// Type text with optional ref-based focus.
    func typeTextTarget(refId: String?, text: String) async -> Result<Any, Error> {
        if let refId = refId {
            let clickResult = await clickTarget(refId: refId, x: nil, y: nil, button: "left")
            if case .failure(let err) = clickResult {
                return .failure(err)
            }
            try? await Task.sleep(nanoseconds: 100_000_000)
        }
        return await typeText(text)
    }

    private func keyNameToCode(_ name: String) -> CGKeyCode {
        let map: [String: CGKeyCode] = [
            "a": 0, "s": 1, "d": 2, "f": 3, "h": 4, "g": 5, "z": 6, "x": 7,
            "c": 8, "v": 9, "b": 11, "q": 12, "w": 13, "e": 14, "r": 15, "y": 16,
            "t": 17, "1": 18, "2": 19, "3": 20, "4": 21, "6": 22, "5": 23,
            "=": 24, "9": 25, "7": 26, "-": 27, "8": 28, "0": 29, "]": 30,
            "o": 31, "u": 32, "[": 33, "i": 34, "p": 35, "l": 37, "j": 38,
            "'": 39, "k": 40, ";": 41, "\\": 42, ",": 43, "/": 44, "n": 45,
            "m": 46, ".": 47, "tab": 48, "space": 49, "`": 50, "delete": 51,
            "return": 36, "enter": 76, "escape": 53, "esc": 53,
            "f1": 122, "f2": 120, "f3": 99, "f4": 118, "f5": 96, "f6": 97,
            "f7": 98, "f8": 100, "f9": 101, "f10": 109, "f11": 103, "f12": 111,
            "left": 123, "right": 124, "down": 125, "up": 126,
            "home": 115, "end": 119, "pageup": 116, "pagedown": 121,
        ]
        return map[name.lowercased()] ?? 0
    }

    // MARK: - App and Window Management

    func launchApp(bundleId: String) async -> Result<Any, Error> {
        guard let appURL = NSWorkspace.shared.urlForApplication(withBundleIdentifier: bundleId) else {
            return .failure(NSError(domain: "Action", code: 5,
                                   userInfo: [NSLocalizedDescriptionKey: "App not found: \(bundleId)"]))
        }
        do {
            let config = NSWorkspace.OpenConfiguration()
            let app = try await NSWorkspace.shared.openApplication(at: appURL, configuration: config)
            return .success(["launched": bundleId, "pid": app.processIdentifier] as [String: Any])
        } catch {
            return .failure(error)
        }
    }

    func windowList() async -> Result<Any, Error> {
        guard let windows = CGWindowListCopyWindowInfo(
            [.optionOnScreenOnly, .excludeDesktopElements], kCGNullWindowID
        ) as? [[String: Any]] else {
            return .failure(NSError(domain: "Action", code: 6,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to list windows"]))
        }

        let list: [[String: Any]] = windows.compactMap { info in
            guard let id = info[kCGWindowNumber as String] as? Int else { return nil }
            return [
                "id": id,
                "title": info[kCGWindowName as String] as? String ?? "",
                "owner": info[kCGWindowOwnerName as String] as? String ?? "",
                "pid": info[kCGWindowOwnerPID as String] as? Int ?? 0,
                "bounds": info[kCGWindowBounds as String] as? [String: Any] ?? [:],
            ]
        }

        return .success(["windows": list] as [String: Any])
    }

    func focusWindow(id: UInt32) async -> Result<Any, Error> {
        // Query window info for this specific window ID
        guard let windowList = CGWindowListCopyWindowInfo([.optionAll], CGWindowID(id)) as? [[String: Any]],
              let info = windowList.first,
              let pid = info[kCGWindowOwnerPID as String] as? Int32
        else {
            return .failure(NSError(domain: "Action", code: 7,
                                   userInfo: [NSLocalizedDescriptionKey: "Window \(id) not found"]))
        }

        let app = NSRunningApplication(processIdentifier: pid)
        app?.activate()

        return .success(["focused": id] as [String: Any])
    }
}
