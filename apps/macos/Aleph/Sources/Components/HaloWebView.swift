//
//  HaloWebView.swift
//  Aleph
//
//  WKWebView wrapper that loads the Leptos /halo route.
//  Handles transparent background and server-unavailable detection.

import AppKit
import WebKit

/// WKWebView that hosts the Leptos Halo chat UI.
final class HaloWebView: WKWebView {

    private static let haloURL = URL(string: "http://127.0.0.1:18790/halo")!

    init() {
        let config = WKWebViewConfiguration()
        config.preferences.setValue(true, forKey: "drawsBackground")

        #if DEBUG
        config.preferences.setValue(true, forKey: "developerExtrasEnabled")
        #endif

        super.init(frame: .zero, configuration: config)

        // Transparent background so NSWindow transparency works
        setValue(false, forKey: "drawsBackground")

        #if DEBUG
        isInspectable = true
        #endif

        navigationDelegate = self
        load(URLRequest(url: Self.haloURL))
    }

    @available(*, unavailable)
    required init?(coder: NSCoder) { fatalError() }

    /// Reload the Halo page (e.g., after server restart).
    func reload() {
        load(URLRequest(url: Self.haloURL))
    }
}

extension HaloWebView: WKNavigationDelegate {
    func webView(
        _ webView: WKWebView,
        didFailProvisionalNavigation navigation: WKNavigation!,
        withError error: Error
    ) {
        let nsError = error as NSError
        if nsError.domain == NSURLErrorDomain &&
           (nsError.code == NSURLErrorCannotConnectToHost ||
            nsError.code == NSURLErrorTimedOut) {
            print("[HaloWebView] Server unreachable — will retry on next show()")
        }
    }
}
