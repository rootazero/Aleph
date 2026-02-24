//
//  SettingsWebView.swift
//  Aleph
//
//  WebView wrapper for loading Control Plane settings UI.
//

import SwiftUI
import WebKit

/// WebView that loads the Leptos Control Plane settings UI from the local server.
///
/// Handles:
/// - Loading the settings URL from localhost
/// - Server-not-running error detection
/// - Reload support
struct SettingsWebView: NSViewRepresentable {

    /// The URL to load in the WebView
    let url: URL

    /// Callback when the server is unreachable
    var onServerUnavailable: (() -> Void)?

    func makeNSView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()
        #if DEBUG
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")
        #endif

        let webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = context.coordinator
        #if DEBUG
        webView.isInspectable = true
        #endif
        webView.load(URLRequest(url: url))
        return webView
    }

    func updateNSView(_ webView: WKWebView, context: Context) {
        // Only reload if URL changed
        if webView.url != url {
            webView.load(URLRequest(url: url))
        }
    }

    func makeCoordinator() -> Coordinator {
        Coordinator(onServerUnavailable: onServerUnavailable)
    }

    final class Coordinator: NSObject, WKNavigationDelegate, @unchecked Sendable {
        let onServerUnavailable: (() -> Void)?

        init(onServerUnavailable: (() -> Void)?) {
            self.onServerUnavailable = onServerUnavailable
        }

        nonisolated func webView(
            _ webView: WKWebView,
            didFailProvisionalNavigation navigation: WKNavigation!,
            withError error: Error
        ) {
            let nsError = error as NSError
            // NSURLErrorCannotConnectToHost covers connection refused
            if nsError.domain == NSURLErrorDomain &&
               (nsError.code == NSURLErrorCannotConnectToHost ||
                nsError.code == NSURLErrorTimedOut) {
                Task { @MainActor in
                    onServerUnavailable?()
                }
            }
        }
    }
}
