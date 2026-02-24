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

    // MARK: - Screenshot

    func screenshot(region: ScreenRegion?) async -> Result<Any, Error> {
        return await screenshotSCK(region: region)
    }

    private func screenshotSCK(region: ScreenRegion?) async -> Result<Any, Error> {
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

    private func imageToBase64PNG(_ image: CGImage) -> String? {
        let nsImage = NSImage(cgImage: image, size: NSSize(width: image.width, height: image.height))
        guard let tiff = nsImage.tiffRepresentation,
              let bitmap = NSBitmapImageRep(data: tiff),
              let pngData = bitmap.representation(using: .png, properties: [:])
        else { return nil }
        return pngData.base64EncodedString()
    }

    // MARK: - OCR

    func ocr(imageBase64: String?) async -> Result<Any, Error> {
        let imageData: Data?
        if let b64 = imageBase64 {
            imageData = Data(base64Encoded: b64)
        } else {
            // Capture current screen first
            let result = await screenshot(region: nil)
            switch result {
            case .success(let dict):
                let d = dict as? [String: Any]
                let b64 = d?["image_base64"] as? String
                imageData = b64.flatMap { Data(base64Encoded: $0) }
            case .failure(let e):
                return .failure(e)
            }
        }

        guard let data = imageData,
              let nsImage = NSImage(data: data),
              let cgImage = nsImage.cgImage(forProposedRect: nil, context: nil, hints: nil)
        else {
            return .failure(NSError(domain: "Perception", code: 5,
                                   userInfo: [NSLocalizedDescriptionKey: "Invalid image data"]))
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
}
