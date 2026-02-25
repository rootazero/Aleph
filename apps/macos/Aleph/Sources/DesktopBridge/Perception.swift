// Perception.swift
// Provides screenshot, OCR, and accessibility tree capabilities for DesktopBridge.

import AppKit
import CoreGraphics
import Foundation
import ScreenCaptureKit
import Vision

/// Provides perception capabilities: screenshot, OCR, accessibility tree.
final class Perception: @unchecked Sendable {
    static let shared = Perception()

    // MARK: - Internal helpers

    /// Captures the primary display as a CGImage without any encoding step.
    /// Shared by `screenshot` and `ocr` to avoid a PNG round-trip when no
    /// caller-supplied image is present.
    private func captureCurrentScreen() async -> Result<CGImage, Error> {
        do {
            let content = try await SCShareableContent.current
            guard let display = content.displays.first else {
                throw NSError(domain: "Perception", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "No display found"])
            }
            let filter = SCContentFilter(display: display, excludingApplications: [], exceptingWindows: [])
            let config = SCStreamConfiguration()
            config.pixelFormat = kCVPixelFormatType_32BGRA
            let image = try await SCScreenshotManager.captureImage(contentFilter: filter,
                                                                   configuration: config)
            return .success(image)
        } catch {
            return .failure(error)
        }
    }

    private func imageToBase64PNG(_ image: CGImage) -> String? {
        let nsImage = NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
        guard let tiff = nsImage.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let pngData = bitmap.representation(using: .png, properties: [:])
        else { return nil }
        return pngData.base64EncodedString()
    }

    // MARK: - Screenshot

    func screenshot(region: ScreenRegion?) async -> Result<Any, Error> {
        do {
            let content = try await SCShareableContent.current
            guard let display = content.displays.first else {
                throw NSError(domain: "Perception", code: 1,
                              userInfo: [NSLocalizedDescriptionKey: "No display found"])
            }

            let filter = SCContentFilter(display: display, excludingApplications: [], exceptingWindows: [])
            let config = SCStreamConfiguration()
            if let r = region {
                config.sourceRect = CGRect(x: r.x, y: r.y, width: r.width, height: r.height)
            }
            config.pixelFormat = kCVPixelFormatType_32BGRA

            let image = try await SCScreenshotManager.captureImage(contentFilter: filter,
                                                                   configuration: config)
            guard let data = imageToBase64PNG(image) else {
                throw NSError(domain: "Perception", code: 2,
                              userInfo: [NSLocalizedDescriptionKey: "Failed to encode screenshot"])
            }

            return .success([
                "image_base64": data,
                "width": image.width,
                "height": image.height,
                "format": "png",
            ] as [String: Any])
        } catch {
            return .failure(error)
        }
    }

    // MARK: - OCR

    func ocr(imageBase64: String?) async -> Result<Any, Error> {
        let cgImage: CGImage
        if let b64 = imageBase64 {
            guard let data = Data(base64Encoded: b64),
                  let nsImage = NSImage(data: data),
                  let cg = nsImage.cgImage(forProposedRect: nil, context: nil, hints: nil)
            else {
                return .failure(NSError(domain: "Perception", code: 5,
                                       userInfo: [NSLocalizedDescriptionKey: "Invalid image data"]))
            }
            cgImage = cg
        } else {
            // Capture screen directly as CGImage — no Base64 round-trip
            switch await captureCurrentScreen() {
            case .success(let img): cgImage = img
            case .failure(let err): return .failure(err)
            }
        }

        return await recognizeText(in: cgImage)
    }

    private func recognizeText(in image: CGImage) async -> Result<Any, Error> {
        await withCheckedContinuation { continuation in
            let request = VNRecognizeTextRequest { request, error in
                if let error = error {
                    continuation.resume(returning: .failure(error))
                    return
                }
                let observations = request.results as? [VNRecognizedTextObservation] ?? []
                let lines: [[String: Any]] = observations.compactMap { obs in
                    guard let top = obs.topCandidates(1).first else { return nil }
                    return [
                        "text": top.string,
                        "confidence": top.confidence,
                        "bounds": [
                            "x": obs.boundingBox.origin.x,
                            "y": obs.boundingBox.origin.y,
                            "width": obs.boundingBox.width,
                            "height": obs.boundingBox.height,
                        ] as [String: Any],
                    ]
                }
                let fullText = lines.compactMap { $0["text"] as? String }.joined(separator: "\n")
                continuation.resume(returning: .success([
                    "text": fullText,
                    "lines": lines,
                ] as [String: Any]))
            }
            request.recognitionLevel = .accurate
            request.recognitionLanguages = ["zh-Hans", "zh-Hant", "en-US"]
            request.usesLanguageCorrection = true

            let handler = VNImageRequestHandler(cgImage: image, options: [:])
            do {
                try handler.perform([request])
            } catch {
                continuation.resume(returning: .failure(error))
            }
        }
    }

    // MARK: - Accessibility Tree

    func axTree(appBundleId: String?) async -> Result<Any, Error> {
        let axApp: AXUIElement
        if let bundleId = appBundleId {
            guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundleId).first else {
                return .failure(NSError(domain: "Perception", code: 6,
                                      userInfo: [NSLocalizedDescriptionKey: "App not running: \(bundleId)"]))
            }
            axApp = AXUIElementCreateApplication(app.processIdentifier)
        } else {
            guard let frontmost = NSWorkspace.shared.frontmostApplication else {
                return .failure(NSError(domain: "Perception", code: 7,
                                      userInfo: [NSLocalizedDescriptionKey: "No frontmost application"]))
            }
            axApp = AXUIElementCreateApplication(frontmost.processIdentifier)
        }

        let tree = axElementToDict(axApp, depth: 0, maxDepth: 5)
        return .success(tree)
    }

    private func axElementToDict(_ element: AXUIElement, depth: Int, maxDepth: Int) -> [String: Any] {
        guard depth < maxDepth else { return ["truncated": true] }

        var result: [String: Any] = [:]

        var roleValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &roleValue)
        result["role"] = (roleValue as? String) ?? "unknown"

        var titleValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXTitleAttribute as CFString, &titleValue)
        if let title = titleValue as? String, !title.isEmpty {
            result["title"] = title
        }

        var valueValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXValueAttribute as CFString, &valueValue)
        if let v = valueValue as? String, !v.isEmpty {
            result["value"] = v
        }

        // AXValue is a CoreFoundation type and cannot be cast with 'as?'.
        // Use CFGetTypeID comparison to safely extract position and size values.
        var positionValue: AnyObject?
        var sizeValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &positionValue)
        AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &sizeValue)
        if let rawPos = positionValue, let rawSz = sizeValue,
           CFGetTypeID(rawPos as CFTypeRef) == AXValueGetTypeID(),
           CFGetTypeID(rawSz as CFTypeRef) == AXValueGetTypeID() {
            // swiftlint:disable:next force_cast
            let pos = rawPos as! AXValue
            // swiftlint:disable:next force_cast
            let sz = rawSz as! AXValue
            var point = CGPoint.zero
            var size = CGSize.zero
            AXValueGetValue(pos, .cgPoint, &point)
            AXValueGetValue(sz, .cgSize, &size)
            result["frame"] = [
                "x": point.x, "y": point.y,
                "width": size.width, "height": size.height,
            ] as [String: Any]
        }

        var childrenValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &childrenValue)
        if let children = childrenValue as? [AXUIElement] {
            result["children"] = children.map { axElementToDict($0, depth: depth + 1, maxDepth: maxDepth) }
        }

        return result
    }

    // MARK: - UI Snapshot (ref-based)

    /// Roles considered interactive (ref-targetable).
    private static let interactiveRoles: Set<String> = [
        "AXButton", "AXTextField", "AXTextArea", "AXCheckBox", "AXSlider",
        "AXPopUpButton", "AXMenuItem", "AXLink", "AXTab", "AXRadioButton",
        "AXComboBox", "AXScrollArea", "AXTable", "AXList", "AXIncrementor",
        "AXDisclosureTriangle", "AXColorWell", "AXMenuButton",
    ]

    /// Capture a structured UI snapshot with ref IDs for interactive elements.
    func snapshot(appBundleId: String?, maxDepth: Int, includeNonInteractive: Bool) async -> Result<Any, Error> {
        let axApp: AXUIElement
        let resolvedBundleId: String
        let appName: String

        if let bundleId = appBundleId {
            guard let app = NSRunningApplication.runningApplications(withBundleIdentifier: bundleId).first else {
                return .failure(NSError(domain: "Perception", code: 6,
                                       userInfo: [NSLocalizedDescriptionKey: "App not running: \(bundleId)"]))
            }
            axApp = AXUIElementCreateApplication(app.processIdentifier)
            resolvedBundleId = bundleId
            appName = app.localizedName ?? bundleId
        } else {
            guard let frontmost = NSWorkspace.shared.frontmostApplication else {
                return .failure(NSError(domain: "Perception", code: 7,
                                       userInfo: [NSLocalizedDescriptionKey: "No frontmost application"]))
            }
            axApp = AXUIElementCreateApplication(frontmost.processIdentifier)
            resolvedBundleId = frontmost.bundleIdentifier ?? "unknown"
            appName = frontmost.localizedName ?? "Unknown"
        }

        var refCounter = 0
        var refs: [String: ResolvedElement] = [:]
        var interactiveRefs: [String] = []
        var totalElements = 0

        let tree = buildSnapshotTree(
            element: axApp,
            depth: 0,
            maxDepth: maxDepth,
            indent: "",
            refCounter: &refCounter,
            refs: &refs,
            interactiveRefs: &interactiveRefs,
            totalElements: &totalElements,
            includeNonInteractive: includeNonInteractive
        )

        // Update the shared RefStore
        RefStore.shared.update(newRefs: refs)

        // Build refs dict for JSON response
        let refsDict: [String: Any] = refs.mapValues { element in
            var entry: [String: Any] = [
                "role": element.role,
                "frame": [
                    "x": element.frame.origin.x,
                    "y": element.frame.origin.y,
                    "w": element.frame.size.width,
                    "h": element.frame.size.height,
                ] as [String: Any],
            ]
            if let label = element.label {
                entry["label"] = label
            }
            return entry
        }

        return .success([
            "app_bundle_id": resolvedBundleId,
            "app_name": appName,
            "tree": tree,
            "refs": refsDict,
            "interactive": interactiveRefs,
            "stats": [
                "total_elements": totalElements,
                "interactive": interactiveRefs.count,
                "max_depth": maxDepth,
            ] as [String: Any],
        ] as [String: Any])
    }

    private func buildSnapshotTree(
        element: AXUIElement,
        depth: Int,
        maxDepth: Int,
        indent: String,
        refCounter: inout Int,
        refs: inout [String: ResolvedElement],
        interactiveRefs: inout [String],
        totalElements: inout Int,
        includeNonInteractive: Bool
    ) -> String {
        guard depth < maxDepth else { return "" }

        totalElements += 1

        // Extract role
        var roleValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXRoleAttribute as CFString, &roleValue)
        let role = (roleValue as? String) ?? "AXUnknown"

        // Extract label (title or description)
        var titleValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXTitleAttribute as CFString, &titleValue)
        let title = titleValue as? String

        var descValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXDescriptionAttribute as CFString, &descValue)
        let desc = descValue as? String

        let label = title.flatMap({ $0.isEmpty ? nil : $0 }) ?? desc.flatMap({ $0.isEmpty ? nil : $0 })

        // Extract frame
        var positionValue: AnyObject?
        var sizeValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXPositionAttribute as CFString, &positionValue)
        AXUIElementCopyAttributeValue(element, kAXSizeAttribute as CFString, &sizeValue)

        var frame: CGRect?
        if let rawPos = positionValue, let rawSz = sizeValue,
           CFGetTypeID(rawPos as CFTypeRef) == AXValueGetTypeID(),
           CFGetTypeID(rawSz as CFTypeRef) == AXValueGetTypeID() {
            // swiftlint:disable:next force_cast
            let pos = rawPos as! AXValue
            // swiftlint:disable:next force_cast
            let sz = rawSz as! AXValue
            var point = CGPoint.zero
            var size = CGSize.zero
            AXValueGetValue(pos, .cgPoint, &point)
            AXValueGetValue(sz, .cgSize, &size)
            frame = CGRect(origin: point, size: size)
        }

        let isInteractive = Self.interactiveRoles.contains(role)

        // Build line
        var line = indent

        if isInteractive, let f = frame {
            refCounter += 1
            let rid = "e\(refCounter)"

            let resolved = ResolvedElement(
                refId: rid,
                role: role,
                label: label,
                frame: f
            )
            refs[rid] = resolved
            interactiveRefs.append(rid)

            let labelStr = label.map { " '\($0)'" } ?? ""
            let frameStr = "(\(Int(f.origin.x)),\(Int(f.origin.y)) \(Int(f.size.width))x\(Int(f.size.height)))"
            line += "[\(rid)] \(role)\(labelStr) \(frameStr)"
        } else if includeNonInteractive, let f = frame {
            refCounter += 1
            let rid = "e\(refCounter)"

            let resolved = ResolvedElement(
                refId: rid,
                role: role,
                label: label,
                frame: f
            )
            refs[rid] = resolved

            let labelStr = label.map { " '\($0)'" } ?? ""
            let frameStr = "(\(Int(f.origin.x)),\(Int(f.origin.y)) \(Int(f.size.width))x\(Int(f.size.height)))"
            line += "[\(rid)] \(role)\(labelStr) \(frameStr)"
        } else {
            let labelStr = label.map { " '\($0)'" } ?? ""
            line += "\(role)\(labelStr)"
        }

        // Recurse into children
        var childrenValue: AnyObject?
        AXUIElementCopyAttributeValue(element, kAXChildrenAttribute as CFString, &childrenValue)
        var childLines = ""
        if let children = childrenValue as? [AXUIElement] {
            for child in children {
                let childTree = buildSnapshotTree(
                    element: child,
                    depth: depth + 1,
                    maxDepth: maxDepth,
                    indent: indent + "  ",
                    refCounter: &refCounter,
                    refs: &refs,
                    interactiveRefs: &interactiveRefs,
                    totalElements: &totalElements,
                    includeNonInteractive: includeNonInteractive
                )
                if !childTree.isEmpty {
                    childLines += "\n" + childTree
                }
            }
        }

        return line + childLines
    }
}
