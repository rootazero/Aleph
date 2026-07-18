import SwiftUI
import WebKit
import Security

/// Full-screen `WKWebView` hosting the Aleph WASM panel.
struct PanelWebView: UIViewRepresentable {
    let url: URL
    var onLoadFailure: (String) -> Void = { _ in }
    /// Surfaced when a self-signed host needs TOFU approval — the host view
    /// presents `CertTrustSheet` from this. Default no-op for previews/tests.
    var onCertPrompt: (CertPromptRequest) -> Void = { _ in }
    /// Pinned-cert store consulted by the TLS-challenge handler. Injectable for
    /// tests; production uses the Keychain-backed store.
    var certStore: any CertStore = KeychainCertStore()

    func makeCoordinator() -> Coordinator {
        Coordinator(onLoadFailure: onLoadFailure, onCertPrompt: onCertPrompt, certStore: certStore)
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
        let onCertPrompt: (CertPromptRequest) -> Void
        let certStore: any CertStore

        /// Reason string surfaced on the approval sheet, matching the desktop
        /// adapters' `REASON`.
        private static let reason = "self-signed / untrusted issuer"

        init(
            onLoadFailure: @escaping (String) -> Void,
            onCertPrompt: @escaping (CertPromptRequest) -> Void,
            certStore: any CertStore
        ) {
            self.onLoadFailure = onLoadFailure
            self.onCertPrompt = onCertPrompt
            self.certStore = certStore
        }

        func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
            onLoadFailure(error.localizedDescription)
        }

        func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
            onLoadFailure(error.localizedDescription)
        }

        /// Server-trust TOFU — the iOS counterpart of the desktop adapters'
        /// TLS-error hooks (`adapter_macos`/`_linux`/`_windows`). WKWebView runs
        /// default system-trust validation; a CA-valid cert passes here and never
        /// prompts. Only a bare-IP self-signed host fails validation and reaches
        /// the shared decision (`certDecision`): a pinned match is accepted
        /// silently, an unknown/changed cert defers the challenge behind an
        /// approval sheet. Fail-closed on every abnormal path (cancel the
        /// challenge → the load fails, never a blanket accept).
        func webView(
            _ webView: WKWebView,
            didReceive challenge: URLAuthenticationChallenge,
            completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
        ) {
            guard challenge.protectionSpace.authenticationMethod == NSURLAuthenticationMethodServerTrust,
                  let trust = challenge.protectionSpace.serverTrust else {
                completionHandler(.performDefaultHandling, nil)
                return
            }
            // Default validation first: a CA-valid cert (domain deployment) passes
            // and takes the silent default path — only self-signed reaches TOFU.
            var evalError: CFError?
            if SecTrustEvaluateWithError(trust, &evalError) {
                completionHandler(.performDefaultHandling, nil)
                return
            }
            // Leaf DER (chain index 0). Fail closed if it can't be read.
            guard let leaf = Self.leafInfo(from: trust) else {
                completionHandler(.cancelAuthenticationChallenge, nil)
                return
            }
            let host = "\(challenge.protectionSpace.host):\(challenge.protectionSpace.port)"
            let fp = sha256Fingerprint(leaf.der)

            switch certDecision(host: host, presentedFP: fp, store: certStore) {
            case .allow:
                completionHandler(.useCredential, URLCredential(trust: trust))
            case .promptUnknown:
                prompt(host: host, fp: fp, subject: leaf.subject, changedFrom: nil,
                       trust: trust, completionHandler: completionHandler)
            case .warnChanged(let old, _, _):
                prompt(host: host, fp: fp, subject: leaf.subject, changedFrom: old,
                       trust: trust, completionHandler: completionHandler)
            }
        }

        /// Build the one-shot approval request and hand it to the host view. The
        /// `decide` closure resolves the held challenge exactly once (a boxed
        /// `resolved` latch guards against a double tap): trust → pin the
        /// fingerprint + accept this connection's cert; cancel → reject (the load
        /// then fails, staying fail-closed). Unlike the desktop reroute dance,
        /// iOS can hold the challenge open and resolve it in place.
        private func prompt(
            host: String,
            fp: String,
            subject: String,
            changedFrom: String?,
            trust: SecTrust,
            completionHandler: @escaping (URLSession.AuthChallengeDisposition, URLCredential?) -> Void
        ) {
            let store = certStore
            var resolved = false
            let request = CertPromptRequest(
                host: host,
                fingerprint: fp,
                subject: subject,
                sans: [],
                reason: Self.reason,
                changedFrom: changedFrom
            ) { approved in
                guard !resolved else { return }
                resolved = true
                if approved {
                    store.pin(host, fp)
                    completionHandler(.useCredential, URLCredential(trust: trust))
                } else {
                    completionHandler(.cancelAuthenticationChallenge, nil)
                }
            }
            onCertPrompt(request)
        }

        /// Leaf cert facts: DER bytes (for the fingerprint) + a subject summary
        /// for display. iOS has no lightweight SAN parser in Security.framework,
        /// so SANs are left to the caller (empty on this platform); the SHA-256
        /// fingerprint is the actual trust anchor and is always shown.
        private static func leafInfo(from trust: SecTrust) -> (der: Data, subject: String)? {
            guard let chain = SecTrustCopyCertificateChain(trust) as? [SecCertificate],
                  let leaf = chain.first else {
                return nil
            }
            let der = SecCertificateCopyData(leaf) as Data
            let subject = (SecCertificateCopySubjectSummary(leaf) as String?) ?? ""
            return (der, subject)
        }
    }
}
