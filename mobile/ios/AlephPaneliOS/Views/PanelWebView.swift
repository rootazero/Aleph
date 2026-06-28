import SwiftUI
import WebKit

/// Full-screen `WKWebView` hosting the Aleph WASM panel.
struct PanelWebView: UIViewRepresentable {
    let url: URL
    var onLoadFailure: (String) -> Void = { _ in }

    func makeCoordinator() -> Coordinator {
        Coordinator(onLoadFailure: onLoadFailure)
    }

    func makeUIView(context: Context) -> WKWebView {
        let config = WKWebViewConfiguration()

        // The panel's static viewport meta omits `viewport-fit=cover`, so iOS
        // reports zero safe-area insets and the phone shell's
        // `env(safe-area-inset-*)` padding collapses. Rewrite the meta at
        // document end so the insets resolve correctly.
        let coverJS = """
        (function () {
          var m = document.querySelector('meta[name=viewport]');
          var v = 'width=device-width, initial-scale=1, viewport-fit=cover';
          if (m) { m.setAttribute('content', v); }
          else {
            m = document.createElement('meta');
            m.name = 'viewport'; m.content = v;
            document.head.appendChild(m);
          }
        })();
        """
        config.userContentController.addUserScript(
            WKUserScript(source: coverJS, injectionTime: .atDocumentEnd, forMainFrameOnly: true)
        )
        config.allowsInlineMediaPlayback = true

        let webView = WKWebView(frame: .zero, configuration: config)
        webView.navigationDelegate = context.coordinator
        webView.scrollView.contentInsetAdjustmentBehavior = .never
        webView.scrollView.bounces = false
        webView.isInspectable = true
        webView.load(URLRequest(url: url))
        return webView
    }

    func updateUIView(_ webView: WKWebView, context: Context) {}

    final class Coordinator: NSObject, WKNavigationDelegate {
        let onLoadFailure: (String) -> Void

        init(onLoadFailure: @escaping (String) -> Void) {
            self.onLoadFailure = onLoadFailure
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            onLoadFailure(error.localizedDescription)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            onLoadFailure(error.localizedDescription)
        }
    }
}
