/* Debug QA observer shared by both native hosts. It never supplies RPC results. */
function installAlephPanelQa(settings, deliver) {
  "use strict";
  if (location.protocol !== "http:" || location.hostname !== "127.0.0.1") return;
  const NativeSocket = window.WebSocket;
  let epoch = 0;
  let session = null;
  let run = null;
  let queued = null;
  let active = null;
  let submitted = false;
  let completed = false;
  let challenging = false;
  let completionFrame = null;
  let priorSocket = null;
  let invalidated = false;
  let artifactEvidence = null;
  const pending = new Map();
  const snapshots = new Set();
  // WebKit converts the monotonic origin to wall time on each getter call.
  // Capture one document identity and share it with the production QA witness.
  const browserTimeOrigin = performance.timeOrigin;
  Object.defineProperty(window, "__alephPanelQaTimeOrigin", {value: browserTimeOrigin});
  const emit = (kind, payload = {}, identity = {}) => deliver({nonce: settings.nonce,
    qa_run_id: settings.qa_run_id, kind, browser_at_ms: performance.now(), session_key: session, run_id: run,
    connection_epoch: epoch, browser_time_origin_ms: browserTimeOrigin, payload, ...identity});
  let reconnectRequestEpoch = null;
  window.__alephPanelQaOnReconnecting = witness => {
    if (reconnectRequestEpoch == null || witness.connection_epoch !== reconnectRequestEpoch) return;
    emit("disconnected", {production_state: true, marked_epoch: witness.connection_epoch},
      {browser_at_ms: witness.browser_at_ms, browser_time_origin_ms: witness.browser_time_origin_ms});
    reconnectRequestEpoch = null;
  };
  setTimeout(() => emit("qa-diagnostic", {path: location.pathname, ready: document.readyState,
    body: document.body?.innerText?.slice(0, 1200), composer: !!document.querySelector(".aleph-composer textarea"),
    artifacts: !!document.querySelector('[data-testid="workspace-tab-artifacts"]')}), 15000);
  const nextFrame = () => new Promise(resolve => requestAnimationFrame(() => requestAnimationFrame(resolve)));
  function projection() {
    const chat = document.querySelector(".chat-scroll-fade");
    if (!chat) throw new Error("Production Chat projection was not rendered");
    // Compare rendered business values, excluding animation and elapsed-time attributes.
    return JSON.stringify({messages: chat.textContent, artifacts: Array.from(document.querySelectorAll('a[href*="/artifact/"]')).map(a => [a.textContent, a.href])});
  }
  async function challenge(socket, reason, frame, evidence = {}) {
    const before = projection();
    challenging = true;
    try { socket.dispatchEvent(new MessageEvent("message", {data: JSON.stringify(frame)})); }
    finally { challenging = false; }
    await nextFrame();
    const after = projection();
    emit(before === after ? "rejected" : "projection-violation", {reason, before, after, ...evidence});
  }
  async function reconnectChallenges() {
    if (!priorSocket || !completionFrame) return;
    const ingress = window.__alephPanelQaIngress;
    if (!Array.isArray(ingress) || ingress.length !== 2 || ingress.some(receive => typeof receive !== "function")) {
      emit("qa-error", {message: "runtime-qa Panel ingress was not installed for both receipts"});
      return;
    }
    const before = projection();
    const currentAccepted = ingress[1](JSON.stringify({method: "event", params: {topic: "qa.phase9.control", data: {}}}));
    const staleAccepted = ingress[0](JSON.stringify({method: "stream.response_chunk", params: {run_id: run, content: "PHASE9_OLD_EPOCH_MUST_NOT_RENDER", chunk_index: 999999, is_final: false}}));
    await nextFrame();
    const after = projection();
    emit(before === after && currentAccepted === true && staleAccepted === false ? "rejected" : "projection-violation",
      {reason: "old_epoch", before, after, boundary: "panel_ingress", handler_invoked: true,
        current_control_accepted: currentAccepted, stale_frame_accepted: staleAccepted});
    await challenge(active, "wrong_session", {jsonrpc: "2.0", method: "stream.run_accepted", params: {run_id: "phase9-foreign-run", session_key: "agent:phase9-foreign:main", accepted_at: new Date().toISOString()}});
    await challenge(active, "malformed_topic", {jsonrpc: "2.0", method: "event", params: {topic: {invalid: true}, data: {content: "PHASE9_MALFORMED_MUST_NOT_RENDER"}}});
    await challenge(active, "duplicate", completionFrame);
    emit("close-requested");
  }
  function observe(socket, generation, raw) {
    if (challenging || socket !== active || generation !== epoch) return;
    let frame;
    try { frame = JSON.parse(raw); } catch (_) { return; }
    if (Object.hasOwn(frame, "id")) {
      const request = pending.get(frame.id);
      const method = request?.method;
      pending.delete(frame.id);
      if (frame.error) { emit("rpc-error", {method, code: frame.error.code}); return; }
      if (method === "connect" && frame.result?.authorized === true) emit("hello");
      if (method === "chat.send" && frame.result?.run_id && frame.result?.session_key
          && (request.params?.session_key == null || request.params.session_key === frame.result.session_key)) {
        emit("send-received", {request_id: frame.id, request_session: request.params?.session_key ?? null},
          {run_id: frame.result.run_id, session_key: frame.result.session_key});
      }
      if (["sessions.list", "chat.history", "artifacts.list", "gateway.metrics.run_concurrency"].includes(method)) {
        emit("qa-diagnostic", {rpc: method, request_session: request?.params?.session_key,
          observed_session: session, result_keys: Object.keys(frame.result || {})});
      }
      const addressed = request?.params?.session_key === session;
      if (method === "sessions.list" || (addressed && ["chat.history", "artifacts.list"].includes(method))) snapshots.add(method);
      if (method === "artifacts.list" && addressed && invalidated && frame.result?.items?.length) {
        artifactEvidence = {refetched: true, count: frame.result.items.length};
        if (completed) emit("artifact-refreshed", artifactEvidence);
      }
      if (epoch > 1 && snapshots.size === 3) {
        emit("snapshot", {sessions: true, history: true, artifacts: true});
        snapshots.clear();
        const replayEpoch = epoch;
        const deadline = Date.now() + 15000;
        (async () => {
          while (epoch === replayEpoch) {
            const applied = window.__alephPanelQaHistoryApplied;
            if (applied?.connection_epoch === replayEpoch && applied.session_key === session) break;
            if (Date.now() >= deadline) throw new Error("Production history replay did not apply");
            await nextFrame();
          }
          if (epoch !== replayEpoch) return;
          await nextFrame();
          emit("reconnected", {rendered: true, applied_epoch: replayEpoch, projection: projection()});
          setTimeout(() => reconnectChallenges().catch(error => emit("qa-error", {message: String(error)})), 250);
        })().catch(error => emit("qa-error", {message: String(error)}));
      }
      return;
    }
    const data = frame.params || {};
    if (frame.method === "event" && data.topic === "session.artifact" && data.data?.session_key === session) invalidated = true;
    if (frame.method === "stream.run_queued" && submitted && !run && data.run_id && data.session_key) {
      queued = {run_id: data.run_id, session_key: data.session_key};
      emit("queued", {ahead: data.ahead}, queued);
    }
    if (frame.method === "stream.run_accepted" && submitted && !run) {
      if (queued && (queued.run_id !== data.run_id || queued.session_key !== data.session_key)) {
        emit("qa-error", {message: "queued run/session differs from acceptance"});
        return;
      }
      session = data.session_key;
      run = data.run_id;
      emit("accepted");
    }
    if (!run || data.run_id !== run || (data.session_key && data.session_key !== session)) return;
    if (frame.method === "stream.response_chunk") emit("streamed");
    if (frame.method === "stream.run_complete") {
      completed = true;
      completionFrame = frame;
      priorSocket = socket;
      emit("complete");
      if (artifactEvidence) emit("artifact-refreshed", artifactEvidence);
      setTimeout(() => {
        reconnectRequestEpoch = epoch;
        socket.close(1000, "owned QA reconnect exercise");
      }, 1000);
    }
  }
  class ObservedSocket extends NativeSocket {
    constructor(...args) {
      super(...args);
      active = this;
      const generation = ++epoch;
      pending.clear();
      snapshots.clear();
      this.addEventListener("message", event => observe(this, generation, event.data));
    }
    send(data) {
      try {
        const frame = JSON.parse(data);
        if (this === active && frame.id != null) pending.set(frame.id, {method: frame.method, params: frame.params});
      } catch (_) { /* Binary and non-JSON data remain the transport's concern. */ }
      return super.send(data);
    }
  }
  window.WebSocket = ObservedSocket;
  function rendered() {
    const input = document.querySelector(".aleph-composer textarea");
    if (!input || !input.getClientRects().length) { requestAnimationFrame(rendered); return; }
    requestAnimationFrame(() => {
      emit("rendered");
      const original = location.pathname;
      // Exercise the browser router used by the canonical Panel. Some shipped
      // layouts have no mounted section menu, so drive an actual popstate path.
      history.pushState(null, "", "/dashboard");
      window.dispatchEvent(new PopStateEvent("popstate", {state: null}));
      const returnToChat = () => {
        if (location.pathname !== "/dashboard" || document.querySelector(".aleph-composer textarea")?.getClientRects().length) {
          requestAnimationFrame(returnToChat); return;
        }
        requestAnimationFrame(() => {
          const returned = () => {
            if (location.pathname !== original) return;
            window.removeEventListener("popstate", returned, true);
            emit("route-requested", {from: "/dashboard", to: original});
            const awaitChat = () => {
              const composer = document.querySelector(".aleph-composer textarea");
              if (!composer?.getClientRects().length) { requestAnimationFrame(awaitChat); return; }
              requestAnimationFrame(() => {
              emit("route-ready", {from: "/dashboard", via: "/dashboard", to: location.pathname});
              const pane = document.querySelector('[data-testid="workspace-panel"]');
              if (!pane?.getClientRects().length) document.querySelector('[data-testid="layout-toggle"]')?.click();
              document.querySelector('[data-testid="workspace-tab-artifacts"]')?.click();
              if (!settings.message) return;
              const setter = Object.getOwnPropertyDescriptor(HTMLTextAreaElement.prototype, "value").set;
              setter.call(composer, settings.message);
              submitted = true;
              emit("input");
              composer.dispatchEvent(new Event("input", {bubbles: true}));
              composer.dispatchEvent(new KeyboardEvent("keydown", {key: "Enter", code: "Enter", bubbles: true}));
              });
            };
            requestAnimationFrame(awaitChat);
          };
          window.addEventListener("popstate", returned, true);
          history.back();
        });
      };
      requestAnimationFrame(returnToChat);
    });
  }
  emit("ready");
  requestAnimationFrame(rendered);
  // Closing is requested by the collector only after it has checked its gates.
  window.__alephQaRequestClose = () => emit("close-requested");
}
