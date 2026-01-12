import AppKit
import Combine
import Foundation
import ScreenCaptureKit
import UserNotifications

/// Capture mode for screen capture
enum ScreenCaptureMode {
    /// User-selected region
    case region
    /// Active window capture
    case window
    /// Full screen capture
    case fullScreen
}

/// Coordinator for screen capture operations
///
/// Manages the screen capture workflow including:
/// - Displaying the region selection overlay
/// - Capturing windows and full screen using ScreenCaptureKit
/// - Processing captured images through Rust vision service
/// - Showing HaloWindow feedback during OCR processing
@MainActor
final class ScreenCaptureCoordinator: ObservableObject {
    // MARK: - Published Properties

    @Published var isCapturing = false
    @Published var captureMode: ScreenCaptureMode = .region
    @Published var lastResult: String?
    @Published var lastError: String?

    // MARK: - Private Properties

    private var overlayWindow: NSWindow?
    private var overlayView: ScreenCaptureOverlayView?

    /// Flag to prevent reentry during dismissal
    private var isDismissing = false

    // MARK: - Singleton

    static let shared = ScreenCaptureCoordinator()

    private init() {}

    // MARK: - Public Methods

    /// Start screen capture with the specified mode
    func startCapture(mode: ScreenCaptureMode) {
        // Prevent reentry - if already capturing, ignore the request
        guard !isCapturing else {
            print("[ScreenCaptureCoordinator] Capture already in progress, ignoring request")
            return
        }

        // CRITICAL: Set isCapturing IMMEDIATELY after guard to prevent race conditions
        // This must happen before any async work or UI setup to prevent double-entry
        // when hotkey is pressed rapidly
        isCapturing = true

        captureMode = mode
        lastResult = nil
        lastError = nil

        switch mode {
        case .region:
            showRegionSelector()
        case .window:
            captureActiveWindow()
        case .fullScreen:
            captureFullScreen()
        }
    }

    /// Cancel the current capture operation
    func cancelCapture() {
        dismissOverlay()
        isCapturing = false
    }

    // MARK: - Region Selection

    private func showRegionSelector() {
        guard let screen = NSScreen.main else {
            lastError = "No screen available"
            isCapturing = false  // Reset flag on failure
            return
        }

        // Create full-screen transparent window
        overlayWindow = NSWindow(
            contentRect: screen.frame,
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        guard let window = overlayWindow else {
            isCapturing = false  // Reset flag on failure
            return
        }

        // Configure window properties
        window.level = .screenSaver
        window.backgroundColor = .clear
        window.isOpaque = false
        window.ignoresMouseEvents = false
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]

        // Create overlay view
        overlayView = ScreenCaptureOverlayView(
            onComplete: { [weak self] rect in
                self?.captureRegion(rect)
            },
            onCancel: { [weak self] in
                self?.cancelCapture()
            }
        )

        window.contentView = overlayView
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(overlayView)
        // Note: isCapturing was already set to true in startCapture()
    }

    private func captureRegion(_ rect: CGRect) {
        dismissOverlay()

        Task {
            do {
                // Use ScreenCaptureKit to capture the region
                let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)

                guard let display = content.displays.first else {
                    lastError = "No display found"
                    isCapturing = false
                    return
                }

                // Create a filter for the display
                let filter = SCContentFilter(display: display, excludingWindows: [])

                // Configure the capture
                let config = SCStreamConfiguration()
                config.sourceRect = rect
                config.width = Int(rect.width) * 2 // Retina
                config.height = Int(rect.height) * 2
                config.scalesToFit = true

                // Capture a single frame
                let image = try await SCScreenshotManager.captureImage(
                    contentFilter: filter,
                    configuration: config
                )

                processCapture(image)
            } catch {
                lastError = "Failed to capture screen: \(error.localizedDescription)"
                isCapturing = false
            }
        }
    }

    // MARK: - Window Capture

    private func captureActiveWindow() {
        Task {
            do {
                let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)

                // Find the frontmost window
                guard let frontApp = NSWorkspace.shared.frontmostApplication,
                      let targetWindow = content.windows.first(where: {
                          $0.owningApplication?.processID == frontApp.processIdentifier
                      })
                else {
                    lastError = "No active window found"
                    return
                }

                // Create a filter for the window
                let filter = SCContentFilter(desktopIndependentWindow: targetWindow)

                // Configure the capture
                let config = SCStreamConfiguration()
                config.width = Int(targetWindow.frame.width) * 2
                config.height = Int(targetWindow.frame.height) * 2
                config.scalesToFit = true

                // Capture a single frame
                let image = try await SCScreenshotManager.captureImage(
                    contentFilter: filter,
                    configuration: config
                )

                processCapture(image)
            } catch {
                lastError = "Failed to capture window: \(error.localizedDescription)"
            }
        }
    }

    // MARK: - Full Screen Capture

    private func captureFullScreen() {
        Task {
            do {
                let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)

                guard let display = content.displays.first else {
                    lastError = "No display found"
                    return
                }

                // Create a filter for the display
                let filter = SCContentFilter(display: display, excludingWindows: [])

                // Configure the capture
                let config = SCStreamConfiguration()
                config.width = Int(display.width) * 2
                config.height = Int(display.height) * 2

                // Capture a single frame
                let image = try await SCScreenshotManager.captureImage(
                    contentFilter: filter,
                    configuration: config
                )

                processCapture(image)
            } catch {
                lastError = "Failed to capture screen: \(error.localizedDescription)"
            }
        }
    }

    // MARK: - Image Processing

    private func processCapture(_ cgImage: CGImage) {
        // Convert CGImage to PNG data
        let bitmap = NSBitmapImageRep(cgImage: cgImage)
        guard let pngData = bitmap.representation(using: .png, properties: [:]) else {
            lastError = "Failed to convert image to PNG"
            isCapturing = false
            return
        }

        // Show HaloWindow with processing state
        showHaloProcessing()

        // Process through Rust vision service
        Task {
            do {
                let result = try await extractTextFromImage(pngData: pngData)

                // Check if OCR returned empty or whitespace-only result
                let trimmedResult = result.trimmingCharacters(in: .whitespacesAndNewlines)
                if trimmedResult.isEmpty {
                    // No text found
                    showHaloError(message: L("ocr.no_text_found"))
                    lastError = L("ocr.no_text_found")
                    isCapturing = false
                    return
                }

                // Copy OCR result text to clipboard (NOT the image)
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(trimmedResult, forType: .string)

                lastResult = trimmedResult
                isCapturing = false

                // Show success feedback via HaloWindow
                showHaloSuccess(characterCount: trimmedResult.count)

            } catch {
                lastError = error.localizedDescription
                isCapturing = false
                showHaloError(message: error.localizedDescription)
            }
        }
    }

    /// Extract text from PNG image data using Rust vision service
    private func extractTextFromImage(pngData: Data) async throws -> String {
        // Get AetherCore instance from AppDelegate
        guard let appDelegate = NSApplication.shared.delegate as? AppDelegate,
              let core = appDelegate.core
        else {
            throw NSError(
                domain: "ScreenCapture",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "AetherCore not initialized"]
            )
        }

        // Call Rust extract_text method
        let imageBytes = Array(pngData)
        return try await core.extractText(imageData: imageBytes)
    }

    // MARK: - HaloWindow Feedback

    /// Show HaloWindow with processing state for OCR
    private func showHaloProcessing() {
        guard let appDelegate = NSApplication.shared.delegate as? AppDelegate,
              let haloWindow = appDelegate.getHaloWindow()
        else {
            return
        }

        haloWindow.updateState(.processing(streamingText: L("ocr.processing")))
        haloWindow.showCentered()
    }

    /// Show HaloWindow success toast
    private func showHaloSuccess(characterCount: Int) {
        guard let appDelegate = NSApplication.shared.delegate as? AppDelegate,
              let haloWindow = appDelegate.getHaloWindow()
        else {
            return
        }

        let message = String(format: L("ocr.success_message"), characterCount)
        haloWindow.showToast(
            type: .info,
            title: L("ocr.success_title"),
            message: message,
            autoDismiss: true
        )
    }

    /// Show HaloWindow error toast
    private func showHaloError(message: String) {
        guard let appDelegate = NSApplication.shared.delegate as? AppDelegate,
              let haloWindow = appDelegate.getHaloWindow()
        else {
            return
        }

        haloWindow.showToast(
            type: .error,
            title: L("ocr.error_title"),
            message: message,
            autoDismiss: true
        )
    }

    // MARK: - Helper Methods

    private func dismissOverlay() {
        // Reentry protection - prevent multiple dismissal attempts
        guard !isDismissing else {
            print("[ScreenCaptureCoordinator] dismissOverlay() reentry prevented")
            return
        }
        isDismissing = true
        defer { isDismissing = false }

        // Step 1: Prepare overlay view for dismissal
        // This stops callbacks and removes tracking areas to break retain cycles
        overlayView?.prepareForDismissal()

        // Step 2: Order the window out (removes from screen, stops rendering)
        // This is safer than immediate close() which can trigger autorelease issues
        overlayWindow?.orderOut(nil)

        // Step 3: Store reference and clear our properties
        // Important: clear overlayView BEFORE closing window to break retain cycles
        let windowToClose = overlayWindow
        overlayView = nil
        overlayWindow = nil

        // Step 4: Close the window asynchronously
        // This allows the current autorelease pool to drain cleanly
        // before the window is fully deallocated
        if let window = windowToClose {
            DispatchQueue.main.async {
                // The window is no longer in our view hierarchy
                // Safe to close now
                window.close()
            }
        }
    }
}
