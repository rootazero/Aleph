//! macOS WKWebView TLS-challenge adapter (reference platform).
//!
//! wry's `WKNavigationDelegate` (`WryNavigationDelegate`, defined via objc2
//! `define_class!`) implements the navigation/download callbacks but **not**
//! `webView:didReceiveAuthenticationChallenge:completionHandler:`. Without it,
//! WKWebView does default system-trust validation, so a self-signed Panel host
//! is silently rejected. We inject the missing selector into wry's delegate
//! class at runtime (`class_addMethod`) — a pure ADD (the selector is absent),
//! no swizzle-save-original needed. The injected method runs TOFU trust for
//! server-trust challenges only; every other challenge (and any failure to read
//! the trust/cert/state) falls through to `.performDefaultHandling`. It never
//! blanket-accepts.
//!
//! The class is resolved from the **live `navigationDelegate` instance** (not by
//! name): objc2 0.6's `define_class!` registers classes under an auto-generated,
//! version-stamped name, so a name lookup would miss. By install time (after the
//! webview is built) the delegate is set, and adding a method to its class
//! affects all instances — harmless: the hook only fires on TLS errors, and the
//! full app connects to loopback with no TLS.

use std::ffi::c_void;
use std::sync::Once;

use block2::Block;
use core_foundation::base::TCFType;
use objc2::runtime::{AnyClass, AnyObject, Imp, Sel};
use objc2::{msg_send, sel, ClassType};
use objc2_foundation::{
    NSString, NSURLAuthenticationChallenge, NSURLAuthenticationMethodServerTrust, NSURLCredential,
    NSURLSessionAuthChallengeDisposition,
};
use security_framework::trust::SecTrust;

use crate::cert_trust::install::{resolve, HookAction, CERT_TRUST_APP};

/// Default Panel listen port. The TOFU store keys on `host:port` (Task 4); a
/// protection space with no explicit port (0) is normalized to this.
const DEFAULT_PORT: isize = 18790;

/// Reason string surfaced on the approval page for a rejected server cert.
const REASON: &str = "self-signed / untrusted issuer";

/// The auth-challenge completion block: `(disposition, credential) -> void`,
/// matching `NSURLSessionAuthChallengeDisposition`-based WKWebView challenges.
type ChallengeCompletion =
    Block<dyn Fn(NSURLSessionAuthChallengeDisposition, *mut NSURLCredential)>;

/// Inject the challenge handler into wry's navigation-delegate class, once.
/// `webview` is the `WKWebView` pointer from Tauri's `PlatformWebview::inner()`.
pub(crate) fn install(webview: *mut c_void) {
    static ONCE: Once = Once::new();
    ONCE.call_once(|| match try_inject(webview) {
        Ok(()) => tracing::info!("cert-trust: WKWebView challenge hook installed"),
        Err(e) => tracing::warn!("cert-trust: WKWebView challenge hook not installed: {e}"),
    });
}

fn try_inject(webview: *mut c_void) -> Result<(), &'static str> {
    if webview.is_null() {
        return Err("null WKWebView");
    }

    // Resolve wry's delegate class from the live instance, then class_addMethod.
    // SAFETY: `webview` is the live `WKWebView` from Tauri's PlatformWebview,
    // valid for this call (we run inside `with_webview` on the main thread). We
    // only send it `-navigationDelegate` and read the delegate's class; both are
    // ordinary object messages. `class_addMethod` adds the absent selector to the
    // (registered) delegate class, and `did_receive_challenge` has exactly the C
    // ABI the runtime invokes for this selector (`id self, SEL _cmd, id webView,
    // id challenge, id block`), so transmuting the typed fn item to the erased
    // `Imp` is sound.
    let added = unsafe {
        let wk: &AnyObject = &*(webview as *const AnyObject);
        let delegate: *mut AnyObject = msg_send![wk, navigationDelegate];
        if delegate.is_null() {
            return Err("webview has no navigationDelegate yet");
        }
        let class: &'static AnyClass = (*delegate).class();
        let class_ptr = (class as *const AnyClass).cast_mut();

        let sel = sel!(webView:didReceiveAuthenticationChallenge:completionHandler:);
        // Type encoding: void ret; self(@), _cmd(:), webView(@), challenge(@),
        // completionHandler block(@?).
        let types = c"v@:@@@?";
        let imp: Imp = std::mem::transmute(
            did_receive_challenge
                as unsafe extern "C-unwind" fn(
                    *mut AnyObject,
                    Sel,
                    *mut AnyObject,
                    *mut AnyObject,
                    *mut AnyObject,
                ),
        );
        let ok = objc2::ffi::class_addMethod(class_ptr, sel, imp, types.as_ptr());
        if ok.as_bool() {
            // WKWebView caches which optional navigation-delegate methods exist
            // when `-setNavigationDelegate:` is first called — during webview
            // construction, before this injection. Re-assign the same delegate so
            // WebKit re-runs `respondsToSelector:` and starts routing the auth
            // challenge to the method we just added; without this, the handler is
            // present on the class but never invoked (self-signed load silently
            // fails). wry owns a strong reference to the delegate, so it stays
            // alive across the momentary nil assignment.
            let nil: *mut AnyObject = std::ptr::null_mut();
            let _: () = msg_send![wk, setNavigationDelegate: nil];
            let _: () = msg_send![wk, setNavigationDelegate: delegate];
        }
        ok
    };

    if added.as_bool() {
        Ok(())
    } else {
        // NO means the selector is already present (e.g. a wry version that grew
        // its own handler) — leave that one in place rather than fighting it.
        Err("class_addMethod returned NO (selector already present)")
    }
}

/// Injected `webView:didReceiveAuthenticationChallenge:completionHandler:`.
///
/// Fail-closed on every abnormal path: `.performDefaultHandling` (which lets
/// WKWebView reject an untrusted cert — the load fails, never a silent accept).
///
/// # Safety
/// Invoked by the Objective-C runtime with the exact argument shape declared in
/// [`try_inject`]'s type encoding. `challenge` and `completion` are the runtime's
/// live objects for this challenge; `completion` is a block called exactly once.
unsafe extern "C-unwind" fn did_receive_challenge(
    _this: *mut AnyObject,
    _cmd: Sel,
    _webview: *mut AnyObject,
    challenge: *mut AnyObject,
    completion: *mut AnyObject,
) {
    // Call the completion block once. Fail-closed if it's somehow null.
    let complete = |disposition: NSURLSessionAuthChallengeDisposition,
                    credential: *mut NSURLCredential| {
        if completion.is_null() {
            return;
        }
        // SAFETY: `completion` is the non-null challenge completion block with the
        // documented `(disposition, credential)` signature; called once, here.
        unsafe {
            let block = &*(completion as *const ChallengeCompletion);
            block.call((disposition, credential));
        }
    };
    let default = || {
        complete(
            NSURLSessionAuthChallengeDisposition::PerformDefaultHandling,
            std::ptr::null_mut(),
        )
    };

    if challenge.is_null() {
        default();
        return;
    }
    // SAFETY: non-null challenge pointer of the delegate method's declared type.
    let challenge: &NSURLAuthenticationChallenge =
        unsafe { &*(challenge as *const NSURLAuthenticationChallenge) };
    let space = challenge.protectionSpace();

    // Only server-trust challenges are ours; everything else → default handling.
    // SAFETY: reading the framework-provided constant NSString symbol.
    let server_trust_method: &NSString = unsafe { NSURLAuthenticationMethodServerTrust };
    if space.authenticationMethod().to_string() != server_trust_method.to_string() {
        default();
        return;
    }

    // The SecTrustRef for this server-trust space (not in the generated bindings).
    // SAFETY: `-serverTrust` returns the `SecTrustRef` for a server-trust space.
    let trust_ref: *mut c_void = unsafe { msg_send![&*space, serverTrust] };
    if trust_ref.is_null() {
        default();
        return;
    }
    // SAFETY: `trust_ref` is a live `SecTrustRef`; wrap under the *get* rule — we
    // borrow it (WKWebView owns it), so CF retain/release stays balanced. `.cast()`
    // infers `SecTrustRef` from `wrap_under_get_rule`'s parameter type.
    let trust = unsafe { SecTrust::wrap_under_get_rule(trust_ref.cast()) };

    // Step 1: default system-trust eval. If it passes, this is a CA-valid cert —
    // silent default path, no prompt.
    if trust.evaluate_with_error().is_ok() {
        default();
        return;
    }

    // Step 2: extract the leaf DER (chain index 0). `evaluate_with_error` ran
    // above, satisfying the deprecated accessor's "evaluate first" precondition.
    #[allow(deprecated)]
    let leaf_der = match trust.certificate_at_index(0) {
        Some(cert) => cert.to_der(),
        None => {
            default();
            return;
        }
    };

    // Store key: `host:port` (port 0/absent → default, matching Task 4's keys).
    let host = space.host().to_string();
    let port = space.port();
    let port = if port <= 0 { DEFAULT_PORT } else { port };
    let host_key = format!("{host}:{port}");

    let Some(app) = CERT_TRUST_APP.get() else {
        tracing::warn!("cert-trust: no AppHandle — failing {host_key} closed");
        default();
        return;
    };

    let action = resolve(app, &host_key, &leaf_der, REASON);
    tracing::info!(
        "cert-trust: TLS challenge {host_key} -> {}",
        match action {
            HookAction::Allow => "ALLOW (pinned)",
            HookAction::Reject => "PROMPT (unknown/changed)",
        }
    );
    match action {
        HookAction::Allow => {
            // SAFETY: `+[NSURLCredential credentialForTrust:]` builds an autoreleased
            // credential from the SecTrustRef; objc2 retains it per the `none` method
            // family. An `Option` return avoids objc2's non-nil-return panic (which would
            // unwind across the FFI boundary and leave the completion block uncalled);
            // a nil credential fails closed to default handling instead.
            let cred: Option<objc2::rc::Retained<NSURLCredential>> =
                unsafe { msg_send![NSURLCredential::class(), credentialForTrust: trust_ref] };
            match cred {
                Some(cred) => complete(
                    NSURLSessionAuthChallengeDisposition::UseCredential,
                    objc2::rc::Retained::as_ptr(&cred).cast_mut(),
                ),
                None => default(),
            }
        }
        HookAction::Reject => {
            // The approval prompt is now showing; fail THIS load. On approval the
            // reroute re-triggers this hook and the pinned cert yields Allow.
            complete(
                NSURLSessionAuthChallengeDisposition::CancelAuthenticationChallenge,
                std::ptr::null_mut(),
            );
        }
    }
}
