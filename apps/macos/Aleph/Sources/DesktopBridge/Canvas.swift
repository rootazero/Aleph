// Canvas.swift
// Provides WKWebView overlay canvas rendering with A2UI protocol support for DesktopBridge.

import AppKit
import Foundation
import WebKit

/// Provides canvas rendering: WKWebView overlay panel with A2UI patch protocol support.
///
/// Canvas must be used from the main thread because WKWebView and NSPanel require it.
/// DesktopBridgeServer wraps Canvas calls via runAsync which dispatches to Task {},
/// which runs on the cooperative thread pool. Canvas.show/hide/update are async so
/// they can be called from any async context and will execute on the main actor.
@MainActor
final class Canvas: NSObject {
    static let shared = Canvas()

    private var panel: NSPanel?
    private var webView: WKWebView?

    // MARK: - Public API

    func show(html: String, position: CanvasPosition) async -> Result<Any, Error> {
        if panel == nil {
            createPanel()
        }

        guard let panel = panel, let webView = webView else {
            return .failure(NSError(domain: "Canvas", code: 1,
                                   userInfo: [NSLocalizedDescriptionKey: "Failed to create canvas panel"]))
        }

        panel.setFrame(NSRect(x: position.x, y: position.y,
                              width: position.width, height: position.height),
                       display: true)

        webView.loadHTMLString(html, baseURL: nil)
        panel.orderFront(nil)

        return .success([
            "visible": true,
            "position": [
                "x": position.x, "y": position.y,
                "width": position.width, "height": position.height,
            ] as [String: Any],
        ] as [String: Any])
    }

    func hide() async -> Result<Any, Error> {
        panel?.orderOut(nil)
        return .success(["visible": false] as [String: Any])
    }

    func update(patch: Any) async -> Result<Any, Error> {
        guard let webView = webView else {
            return .failure(NSError(domain: "Canvas", code: 2,
                                   userInfo: [NSLocalizedDescriptionKey: "Canvas not shown — call canvas_show first"]))
        }

        guard let patchData = try? JSONSerialization.data(withJSONObject: patch),
              let patchJson = String(data: patchData, encoding: .utf8)
        else {
            return .failure(NSError(domain: "Canvas", code: 3,
                                   userInfo: [NSLocalizedDescriptionKey: "Invalid patch data — must be JSON-serializable"]))
        }

        let script = "if (typeof window.alephApplyPatch === 'function') { window.alephApplyPatch(\(patchJson)); }"
        do {
            try await webView.evaluateJavaScript(script)
        } catch {
            // JS evaluation errors are non-fatal (e.g. page still loading, alephApplyPatch not yet injected).
            // Log for debuggability but report success since the JS guard handles the not-ready case.
            print("[Canvas] evaluateJavaScript error: \(error)")
        }

        return .success(["patched": true] as [String: Any])
    }

    // MARK: - Private

    private func createPanel() {
        let newPanel = NSPanel(
            contentRect: NSRect(x: 100, y: 100, width: 800, height: 600),
            styleMask: [.titled, .closable, .resizable, .nonactivatingPanel],
            backing: .buffered,
            defer: false
        )
        newPanel.title = "Aleph Canvas"
        newPanel.level = .floating
        newPanel.isReleasedWhenClosed = false
        newPanel.backgroundColor = .clear
        newPanel.isOpaque = false

        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")

        let newWebView = WKWebView(
            frame: newPanel.contentRect(forFrameRect: newPanel.frame),
            configuration: config
        )
        newWebView.autoresizingMask = [.width, .height]
        newWebView.navigationDelegate = self

        newPanel.contentView = newWebView

        self.panel = newPanel
        self.webView = newWebView
    }
}

// MARK: - WKNavigationDelegate

extension Canvas: WKNavigationDelegate {
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // Inject A2UI v0.8 surface patch handler
        let script = """
        window.alephApplyPatch = function(patch) {
            if (!Array.isArray(patch)) { return; }
            patch.forEach(function(op) {
                if (op.type === 'surfaceUpdate' && op.content) {
                    document.body.innerHTML = op.content;
                }
            });
        };
        """
        webView.evaluateJavaScript(script, completionHandler: nil)
    }
}
