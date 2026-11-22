/* Observe the real Panel. Never supply RPC replies or invoke native APIs. */
function installAlephPanelQa(settings, deliver) {
  "use strict";
  if (window.top !== window || location.protocol !== "http:" || location.hostname !== "127.0.0.1") return;
  if (settings.message.startsWith("phase11-pairing:")) {
    installAlephPanelPairingQa(settings, deliver);
    return;
  }
  const labels = JSON.parse(settings.status_labels);
  const NativeSocket = window.WebSocket;
  const pending = new Set();
  let credentialDocument = null;
  let statusDocument = null;
  let nativeStatus = null;
  let empty = false;
  let authorized = false;
  let routed = false;
  let finished = false;
  const diagnostics = {errors: [], probes: 0, ready: 0, credential_replies: 0, credential_failed: false};
  window.addEventListener("error", event => diagnostics.errors.push(event.error?.name || "script_error"));
  window.addEventListener("unhandledrejection", event => diagnostics.errors.push(event.reason?.name || "rejection"));
  const emit = (kind, payload = {}) => deliver({nonce: settings.nonce, qa_run_id: settings.qa_run_id, kind, ...payload});
  window.addEventListener("message", event => {
    if (event.source !== null || event.origin !== "" || typeof event.data !== "string") return;
    let value;
    try { value = JSON.parse(event.data); } catch (_) { return; }
    if (value.kind === "probe") diagnostics.probes++;
    if (value.kind === "credentials-ready") diagnostics.ready++;
    if (value.kind === "credential-reply") {
      diagnostics.credential_replies++;
      diagnostics.credential_failed ||= value.reply?.outcome === "failed";
    }
    if (!/^[a-f0-9]{32}$/.test(value.document || "")) return;
    if (value.kind === "credential-reply" && value.reply?.outcome === "loaded") {
      credentialDocument = value.document;
      empty = value.reply.lease?.credential === null;
    }
    if (value.kind === "autostart-reply") {
      statusDocument = value.document;
      nativeStatus = value.status;
    }
  });
  class ObservedSocket extends NativeSocket {
    constructor(...args) {
      super(...args);
      this.addEventListener("message", event => {
        let frame;
        try { frame = JSON.parse(event.data); } catch (_) { return; }
        if (pending.delete(frame.id) && frame.result?.authorized === true) authorized = true;
      });
    }
    send(data) {
      let frame;
      try { frame = JSON.parse(data); } catch (_) { return super.send(data); }
      if (frame.method === "connect" && frame.id != null
          && !["device_token", "bootstrap_ticket", "token"].some(key => Object.hasOwn(frame.params || {}, key))) {
        pending.add(frame.id);
      }
      return super.send(data);
    }
  }
  window.WebSocket = ObservedSocket;
  function observation() {
    const section = document.querySelector('[data-testid="desktop-autostart-section"]');
    const status = section?.querySelector('[data-testid="desktop-autostart-status"]');
    const paragraphs = status?.querySelectorAll("p");
    return {document: statusDocument, route: location.pathname, native_status: nativeStatus,
      diagnostics: {...diagnostics, ready_state: document.readyState, body_children: document.body?.children.length,
        composer: !!document.querySelector(".aleph-composer textarea"),
        resources: performance.getEntriesByType("resource").flatMap(entry => {
          const name = new URL(entry.name).pathname;
          return ["/aleph_panel.js", "/aleph_panel_bg.wasm", "/tailwind.css"].includes(name)
            ? [{name, bytes: entry.decodedBodySize}] : [];
        })},
      effects: {
        native_marker: document.documentElement?.getAttribute("data-shell") === "aleph-nsh",
        credential_load_empty: empty,
        connect_authorized_without_credential: authorized,
        component_visible: !!section?.getClientRects().length && !!status?.getClientRects().length,
        status_text_correct: !!paragraphs?.[0] && labels.some(value => paragraphs[0].textContent.trim() === value.status_unavailable),
        readonly_text_correct: !!paragraphs?.[1] && labels.some(value => paragraphs[1].textContent.trim() === value.read_only),
        toggle_absent: !!section && section.querySelectorAll('input[type="checkbox"]').length === 0,
        document_bound: !!credentialDocument && credentialDocument === statusDocument,
      }};
  }
  function finish() {
    if (finished) return;
    finished = true;
    emit("panel-status-fixture", observation());
    emit("close-requested");
  }
  function inspect() {
    if (finished) return;
    if (!routed && empty && authorized && document.querySelector(".aleph-composer textarea")?.getClientRects().length) {
      routed = true;
      history.pushState(null, "", "/settings/general");
      window.dispatchEvent(new PopStateEvent("popstate", {state: null}));
    }
    const value = observation();
    if (routed && nativeStatus && Object.values(value.effects).every(effect => effect === true)) {
      requestAnimationFrame(() => requestAnimationFrame(finish));
      return;
    }
    requestAnimationFrame(inspect);
  }
  requestAnimationFrame(inspect);
  setTimeout(finish, 45000); // An incomplete observation fails the evaluator.
}
