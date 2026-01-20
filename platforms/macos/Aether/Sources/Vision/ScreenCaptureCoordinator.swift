import AppKit
import Combine
import ScreenCaptureKit
import UserNotifications
import os.log

// Debug file logger for crash investigation
#if DEBUG
private func debugLog(_ message: String) {
    let timestamp = ISO8601DateFormatter().string(from: Date())
    let logMessage = "[\(timestamp)] \(message)\n"
    let logPath = NSHomeDirectory() + "/Desktop/aether_debug.log"

    if let data = logMessage.data(using: .utf8) {
        if FileManager.default.fileExists(atPath: logPath) {
            if let handle = FileHandle(forWritingAtPath: logPath) {
                handle.seekToEndOfFile()
                handle.write(data)
                handle.closeFile()
            }
        } else {
            FileManager.default.createFile(atPath: logPath, contents: data)
        }
    }
    NSLog("[ScreenCapture] \(message)")
}
#else
private func debugLog(_ message: String) {
    // No-op in release builds
}
#endif

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

    /// Flag to prevent reentry during dismissal
    private var isDismissing = false

    /// Shared overlay window - created once, reused for all captures
    /// Using lazy initialization to defer creation until first use
    private var _sharedWindow: NSWindow?
    private var _sharedView: ScreenCaptureOverlayView?

    /// Get or create the shared overlay window
    private var sharedWindow: NSWindow {
        if let existing = _sharedWindow {
            return existing
        }
        debugLog(" [WindowReuse] Creating shared window...")
        let window = createOverlayWindow()
        _sharedWindow = window
        return window
    }

    /// Get or create the shared overlay view
    private var sharedView: ScreenCaptureOverlayView {
        if let existing = _sharedView {
            return existing
        }
        debugLog(" [WindowReuse] Creating shared view...")
        let view = ScreenCaptureOverlayView(frame: .zero)
        _sharedView = view
        return view
    }

    // MARK: - Singleton

    static let shared = ScreenCaptureCoordinator()

    private init() {}

    // MARK: - Window Factory

    /// Create the overlay window with proper configuration
    private func createOverlayWindow() -> NSWindow {
        guard let screen = NSScreen.main else {
            // Fallback to a default frame if no screen available
            return NSWindow(
                contentRect: CGRect(x: 0, y: 0, width: 800, height: 600),
                styleMask: .borderless,
                backing: .buffered,
                defer: false
            )
        }

        let window = NSWindow(
            contentRect: screen.frame,
            styleMask: .borderless,
            backing: .buffered,
            defer: false
        )

        // Configure window properties (done once)
        window.level = .screenSaver
        window.backgroundColor = .clear
        window.isOpaque = false
        window.ignoresMouseEvents = false
        window.collectionBehavior = [.canJoinAllSpaces, .fullScreenAuxiliary]

        return window
    }

    // MARK: - Public Methods

    /// Start screen capture with the specified mode
    func startCapture(mode: ScreenCaptureMode) {
        debugLog(">>> startCapture() ENTRY mode=\(mode)")

        // Prevent reentry - if already capturing, ignore the request
        guard !isCapturing else {
            debugLog("Capture already in progress, ignoring request")
            return
        }

        // Permission pre-check: Verify Screen Recording permission BEFORE creating overlay
        // This provides better UX by showing error immediately instead of after user selection
        debugLog("Checking Screen Recording permission...")
        guard PermissionChecker.hasScreenRecordingPermission() else {
            debugLog("Screen Recording permission NOT granted")
            showPermissionRequiredError()
            return
        }
        debugLog("Permission OK, setting isCapturing=true")

        // CRITICAL: Set isCapturing IMMEDIATELY after guard to prevent race conditions
        // This must happen before any async work or UI setup to prevent double-entry
        // when hotkey is pressed rapidly
        isCapturing = true

        captureMode = mode
        lastResult = nil
        lastError = nil

        debugLog(" Dispatching to mode handler...")
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
        debugLog(" >>> showRegionSelector() START [WindowReuse]")

        guard let screen = NSScreen.main else {
            lastError = "No screen available"
            isCapturing = false  // Reset flag on failure
            debugLog(" showRegionSelector() FAILED - no screen")
            return
        }

        // Get or create shared window and view (reused across captures)
        let window = sharedWindow
        let view = sharedView

        debugLog(" [WindowReuse] Using shared window and view")

        // Update window frame to match current screen (in case screen changed)
        window.setFrame(screen.frame, display: false)

        // Reset view state for new capture session
        debugLog(" [WindowReuse] Resetting view state...")
        view.reset()

        // Set callbacks for this capture session
        view.setCallbacks(
            onComplete: { [weak self] rect in
                debugLog(" onComplete callback triggered rect=\(rect)")
                self?.captureRegion(rect)
            },
            onCancel: { [weak self] in
                debugLog(" onCancel callback triggered")
                self?.cancelCapture()
            }
        )

        // Ensure view is set as contentView (first time or after any edge case)
        if window.contentView !== view {
            debugLog(" [WindowReuse] Setting contentView (first use or reconnection)")
            window.contentView = view
        }

        // Update view frame to match window
        view.frame = window.contentView?.bounds ?? screen.frame

        debugLog(" [WindowReuse] Showing window...")
        window.makeKeyAndOrderFront(nil)
        window.makeFirstResponder(view)

        debugLog(" >>> showRegionSelector() COMPLETE - window visible [WindowReuse]")
        // Note: isCapturing was already set to true in startCapture()
    }

    private func captureRegion(_ rect: CGRect) {
        debugLog(" captureRegion() START rect=\(rect)")

        // CRITICAL FIX: Delay ALL cleanup until AFTER mouseUp event fully completes
        // The crash occurs because AppKit's event dispatch has autoreleased objects
        // that reference the view. We must let the current event fully complete first.
        DispatchQueue.main.async { [weak self] in
            guard let self = self else { return }

            debugLog(" [Deferred] Now dismissing overlay...")
            self.dismissOverlay()

            // Additional delay for ScreenCaptureKit
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.1) { [weak self] in
                guard let self = self else { return }

                debugLog(" Starting ScreenCaptureKit capture...")

            Task {
                do {
                    // Use ScreenCaptureKit to capture the region
                    let content = try await SCShareableContent.excludingDesktopWindows(false, onScreenWindowsOnly: true)

                    guard let display = content.displays.first else {
                        self.lastError = "No display found"
                        self.isCapturing = false
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

                    self.processCapture(image)
                } catch {
                    self.lastError = "Failed to capture screen: \(error.localizedDescription)"
                    self.isCapturing = false
                }
            }
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

        // Show Halo with processing spinner (same as single-turn dialog mode)
        showHaloProcessing()

        // Process through Rust vision service
        Task {
            do {
                let result = try await extractTextFromImage(pngData: pngData)

                // Check if OCR returned empty or whitespace-only result
                let trimmedResult = result.trimmingCharacters(in: .whitespacesAndNewlines)
                if trimmedResult.isEmpty {
                    // No text found - show brief error then hide
                    lastError = L("ocr.no_text_found")
                    isCapturing = false
                    debugLog("OCR: No text found")
                    hideHaloWithDelay()
                    return
                }

                // Copy OCR result text to clipboard (NOT the image)
                NSPasteboard.general.clearContents()
                NSPasteboard.general.setString(trimmedResult, forType: .string)

                lastResult = trimmedResult
                isCapturing = false

                // Show success checkmark ✅ then auto-hide
                showHaloSuccess()
                debugLog("OCR SUCCESS: \(trimmedResult.count) characters")

            } catch {
                lastError = error.localizedDescription
                isCapturing = false
                debugLog("OCR ERROR: \(error.localizedDescription)")
                // Hide halo on error (no toast, just hide)
                hideHaloWithDelay()
            }
        }
    }

    /// Extract text from PNG image data using Rust vision service
    private func extractTextFromImage(pngData: Data) async throws -> String {
        debugLog("[OCR] extractTextFromImage START: dataSize=\(pngData.count) bytes")

        // Get AetherCore instance from AppDelegate
        guard let appDelegate = NSApplication.shared.delegate as? AppDelegate else {
            debugLog("[OCR] ERROR: AppDelegate not found")
            throw NSError(
                domain: "ScreenCapture",
                code: -1,
                userInfo: [NSLocalizedDescriptionKey: "AppDelegate not found"]
            )
        }

        guard let core = appDelegate.core else {
            debugLog("[OCR] ERROR: AetherCore not initialized")
            throw NSError(
                domain: "ScreenCapture",
                code: -2,
                userInfo: [NSLocalizedDescriptionKey: "AetherCore not initialized"]
            )
        }

        debugLog("[OCR] Calling core.extractText()...")
        let startTime = CFAbsoluteTimeGetCurrent()

        do {
            // Call Rust extract_text method (is synchronous)
            let imageBytes = Array(pngData)
            let result = try core.extractText(imageData: imageBytes)

            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            debugLog("[OCR] SUCCESS: \(result.count) chars in \(String(format: "%.2f", elapsed))s")
            debugLog("[OCR] Result preview: \(String(result.prefix(100)))...")

            return result
        } catch {
            let elapsed = CFAbsoluteTimeGetCurrent() - startTime
            debugLog("[OCR] EXCEPTION after \(String(format: "%.2f", elapsed))s: \(error)")
            throw error
        }
    }

    // MARK: - HaloWindow Feedback

    /// Show HaloWindow with processing spinner (same as single-turn dialog mode)
    private func showHaloProcessing() {
        guard let appDelegate = NSApplication.shared.delegate as? AppDelegate,
              let haloWindow = appDelegate.getHaloWindow()
        else {
            debugLog("[Halo] Cannot get HaloWindow for processing state")
            return
        }

        debugLog("[Halo] Showing processing spinner")
        haloWindow.updateState(.processing(streamingText: nil))
        haloWindow.showCentered()
    }

    /// Show HaloWindow success checkmark ✅ then auto-hide
    private func showHaloSuccess() {
        guard let appDelegate = NSApplication.shared.delegate as? AppDelegate,
              let haloWindow = appDelegate.getHaloWindow()
        else {
            debugLog("[Halo] Cannot get HaloWindow for success state")
            return
        }

        debugLog("[Halo] Showing success checkmark")
        haloWindow.updateState(.success(message: nil))

        // Auto-hide after 0.8 seconds
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) {
            haloWindow.hide()
            debugLog("[Halo] Auto-hidden after success")
        }
    }

    /// Hide HaloWindow with a brief delay
    private func hideHaloWithDelay() {
        guard let appDelegate = NSApplication.shared.delegate as? AppDelegate,
              let haloWindow = appDelegate.getHaloWindow()
        else {
            return
        }

        DispatchQueue.main.asyncAfter(deadline: .now() + 0.3) {
            haloWindow.hide()
            debugLog("[Halo] Hidden after delay")
        }
    }

    /// Show permission required error and open System Settings
    private func showPermissionRequiredError() {
        guard let appDelegate = NSApplication.shared.delegate as? AppDelegate,
              let haloWindow = appDelegate.getHaloWindow()
        else {
            return
        }

        // Show error toast with guidance
        haloWindow.showToast(
            type: .error,
            title: L("ocr.permission_required_title"),
            message: L("ocr.permission_required_message"),
            autoDismiss: false  // Keep visible so user can read
        )

        // Open System Settings to Screen Recording permission page
        PermissionChecker.openSystemSettings(for: .screenRecording)
    }

    // MARK: - Helper Methods

    private func dismissOverlay() {
        debugLog(" >>> dismissOverlay() START [WindowReuse]")

        // Reentry protection - prevent multiple dismissal attempts
        guard !isDismissing else {
            debugLog(" dismissOverlay() reentry prevented")
            return
        }
        isDismissing = true

        // Step 1: Prepare view for dismissal (stops callbacks, removes tracking areas)
        // This is safe because we're reusing the view - it will be reset() on next use
        debugLog(" Step 1: prepareForDismissal on shared view...")
        _sharedView?.prepareForDismissal()

        // Step 2: Hide the window (NOT close - we're reusing it)
        debugLog(" Step 2: orderOut (hide, not close)...")
        _sharedWindow?.orderOut(nil)

        // Note: We do NOT clear references or close the window
        // The window and view remain alive for reuse
        // No memory accumulation because we're always using the same instances

        debugLog(" [WindowReuse] Window hidden, ready for next capture")

        // Reset flag
        isDismissing = false
        debugLog(" >>> dismissOverlay() COMPLETE [WindowReuse]")
    }
}
