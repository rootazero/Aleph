/* Fixed QA case: real Panel pairing and reload, followed by explicit isolated cleanup. */
function installAlephPanelPairingQa(settings, deliver) {
  "use strict";
  const key = `phase11-protected:${settings.qa_run_id}`;
  const ticket = JSON.parse(settings.message.slice("phase11-pairing:".length)).ticket;
  const prior = JSON.parse(sessionStorage.getItem(key) || "null");
  const effects = prior?.effects || Object.fromEntries([
    "initial_empty", "local_without_prompt", "ticket_submitted", "pair_authorized", "save_ack", "reload_stored",
    "reload_protected", "no_reload_ticket", "cleanup_deleted", "cleanup_empty"
  ].map(name => [name, false]));
  const documents = prior?.documents || [];
  const stage = prior ? 1 : 0;
  const NativeSocket = window.WebSocket;
  const sockets = new Set();
  const pending = new Map();
  let documentId = null;
  let loaded = false;
  let storedId = null;
  let submitted = false;
  let moving = false;
  let finishing = false;
  let reloadTicket = false;
  let cleanupStage = "not_started";
  let cleanupError = null;
  let openedSettingsMenu = false;
  let localAuthorized = false;
  let pairingOpened = false;
  window.addEventListener("pagehide", () => { for (const socket of sockets) socket.close(1000, "QA navigation"); });
  const emit = (kind, payload = {}) => deliver({nonce: settings.nonce, qa_run_id: settings.qa_run_id, kind, ...payload});
  window.addEventListener("message", event => {
    if (event.source !== null || event.origin !== "" || typeof event.data !== "string") return;
    let value;
    try { value = JSON.parse(event.data); } catch (_) { return; }
    if (value.kind !== "credential-reply" || !/^[a-f0-9]{32}$/.test(value.document || "")) return;
    if (value.reply?.outcome === "loaded" && !loaded) {
      loaded = true;
      documentId = value.document;
      documents.push(documentId);
      const credential = value.reply.lease?.credential;
      if (stage === 0) effects.initial_empty = credential === null;
      if (stage === 1) {
        effects.reload_stored = !!credential && typeof credential.device_token === "string" && credential.device_token.length > 0;
        storedId = credential?.device_id;
      }
    }
    if (stage === 0 && value.document === documentId && value.reply?.outcome === "saved") effects.save_ack = true;
  });
  class ObservedSocket extends NativeSocket {
    constructor(...args) {
      super(...args);
      sockets.add(this);
      this.addEventListener("message", event => {
        let frame;
        try { frame = JSON.parse(event.data); } catch (_) { return; }
        const request = pending.get(frame.id);
        if (!request) return;
        pending.delete(frame.id);
        const result = frame.result;
        if (stage === 0 && request.mode === "none" && result?.authorized === true) localAuthorized = true;
        if (result?.authorized !== true || result.user_id !== "alice" || result.device_id !== request.device) return;
        if (stage === 0 && request.mode === "bootstrap") {
          effects.pair_authorized = typeof result.device_token === "string" && result.device_token.length > 0 && result.device_token !== ticket;
        }
        if (stage === 1 && request.mode === "stored" && request.device === storedId) {
          effects.reload_protected = true;
          effects.no_reload_ticket = !reloadTicket;
        }
      });
    }
    send(data) {
      let frame;
      try { frame = JSON.parse(data); } catch (_) { return super.send(data); }
      if (frame.method === "connect" && frame.id != null) {
        const params = frame.params || {};
        const mode = Object.hasOwn(params, "bootstrap_ticket") ? "bootstrap" : Object.hasOwn(params, "device_token") ? "stored" : "none";
        if (stage === 1 && mode === "bootstrap") { reloadTicket = true; effects.no_reload_ticket = false; }
        pending.set(frame.id, {mode, device: params.device_id});
      }
      return super.send(data);
    }
  }
  window.WebSocket = ObservedSocket;
  async function closeSockets() {
    await Promise.all([...sockets].map(socket => socket.readyState === NativeSocket.CLOSED ? Promise.resolve() : new Promise(resolve => {
      const timer = setTimeout(resolve, 2000);
      socket.addEventListener("close", () => { clearTimeout(timer); resolve(); }, {once: true});
      socket.close(1000, "QA complete");
    })));
  }
  async function finish() {
    if (finishing) return;
    finishing = true;
    try {
      const bridge = window.__alephShellBridge;
      cleanupStage = "load";
      const lease = await bridge.invoke("panel_credential_load", null);
      cleanupStage = "delete";
      effects.cleanup_deleted = await bridge.invoke("panel_credential_delete", {lease_id: lease.lease_id}) === true;
      cleanupStage = "verify";
      effects.cleanup_empty = (await bridge.invoke("panel_credential_load", null)).credential === null;
      cleanupStage = "complete";
    } catch (error) { cleanupError = error?.name || "cleanup_failed"; }
    await closeSockets();
    sessionStorage.removeItem(key);
    emit("protected-panel-fixture", {documents, user: effects.reload_protected ? "alice" : null, effects, cleanup_stage: cleanupStage, cleanup_error: cleanupError});
    emit("close-requested");
  }
  function inspect() {
    if (finishing || moving) return;
    if (stage === 0 && effects.initial_empty && localAuthorized && !submitted) {
      if (!pairingOpened && document.querySelector('[data-testid="token-wall"]')?.getClientRects().length) {
        void finish();
        return;
      }
      if (location.pathname !== "/settings/general") {
        const general = document.querySelector('a[href="/settings/general"]');
        const settings = document.querySelector('[data-panel-nav="/settings"]');
        const menu = document.querySelector('[data-panel-nav-toggle]');
        if (general) general.click();
        else if (settings) settings.click();
        else if (menu && !openedSettingsMenu) {
          openedSettingsMenu = true;
          menu.click();
        }
        requestAnimationFrame(inspect);
        return;
      }
      const pair = document.querySelector('[data-testid="pair-this-device"]');
      if (!pair?.getClientRects().length) {
        requestAnimationFrame(inspect);
        return;
      }
      if (!pairingOpened) {
        effects.local_without_prompt = true;
        pairingOpened = true;
        pair.click();
      }
      const input = document.querySelector('[data-testid="token-wall-input"]');
      const button = document.querySelector('[data-testid="token-wall-submit"]');
      if (input?.getClientRects().length && button?.getClientRects().length && typeof ticket === "string" && ticket.length > 0) {
        submitted = true;
        Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value").set.call(input, ticket);
        input.dispatchEvent(new Event("input", {bubbles: true}));
        requestAnimationFrame(() => {
          if (!button.disabled) { effects.ticket_submitted = true; button.click(); }
        });
      }
    }
    if (stage === 0 && effects.pair_authorized && effects.save_ack) {
      moving = true;
      sessionStorage.setItem(key, JSON.stringify({documents, effects}));
      closeSockets().then(() => requestAnimationFrame(() => requestAnimationFrame(() => location.reload())));
      return;
    }
    if (stage === 1 && effects.reload_stored && effects.reload_protected && effects.no_reload_ticket) {
      requestAnimationFrame(() => requestAnimationFrame(finish));
      return;
    }
    requestAnimationFrame(inspect);
  }
  requestAnimationFrame(inspect);
  setTimeout(finish, 50000);
}
