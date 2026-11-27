function installAlephShellBridge(origin, prefix) {
  if (window !== window.top || location.origin !== origin) return;

  const documentId = Array.from(crypto.getRandomValues(new Uint8Array(16)),
    byte => byte.toString(16).padStart(2, "0")).join("");
  let pending = null;
  let statusPending = null;
  let ready = false;
  let closed = false;
  let nextRequest = 0;
  const failure = () => new Error("native credential bridge rejected or unavailable");
  const keys = (value, names) => value !== null && typeof value === "object"
    && Object.keys(value).length === names.length
    && names.every(name => Object.hasOwn(value, name));
  const leaseId = value => Number.isSafeInteger(value) && value > 0;
  const callFor = (command, args) => {
    if (command === "panel_credential_load" && (args === null || args === undefined)) {
      return { operation: "load" };
    }
    if (command === "panel_credential_delete" && keys(args, ["lease_id"]) && leaseId(args.lease_id)) {
      return { operation: "delete", lease_id: args.lease_id };
    }
    if (command === "panel_credential_save" && keys(args, ["lease_id", "credential"])
      && leaseId(args.lease_id) && keys(args.credential, ["device_id", "device_token"])
      && typeof args.credential.device_id === "string" && args.credential.device_id.trim() !== ""
      && typeof args.credential.device_token === "string" && args.credential.device_token.trim() !== "") {
      return { operation: "save", lease_id: args.lease_id, credential: args.credential };
    }
    throw failure();
  };
  const finish = (error, value) => {
    if (!pending) return;
    const entry = pending;
    pending = null;
    clearTimeout(entry.timer);
    if (error) entry.reject(failure());
    else entry.resolve(value);
  };
  const sendPending = () => {
    if (!ready || !pending || pending.sent || closed) return;
    const payload = JSON.stringify({ kind: "credential-request", request: pending.request,
      document: documentId, call: pending.call });
    if (payload.length > 16384) { finish(true); return; }
    pending.sent = true;
    try { window.nsh.postMessage(prefix + payload); }
    catch (_) { finish(true); }
  };
  const finishStatus = (error, value) => {
    if (!statusPending) return;
    const entry = statusPending;
    statusPending = null;
    clearTimeout(entry.timer);
    if (error) entry.reject(new Error("native autostart status rejected or unavailable"));
    else entry.resolve(value);
  };
  // One autostart read or write at a time; both are answered by
  // `autostart-reply` with the status the host read.
  const sendStatus = () => {
    if (!ready || !statusPending || statusPending.sent || closed) return;
    statusPending.sent = true;
    try { window.nsh.postMessage(prefix + JSON.stringify({ ...statusPending.message,
      request: statusPending.request, document: documentId })); }
    catch (_) { finishStatus(true); }
  };
  const askStatus = (message, resolve, reject) => {
    statusPending = { message, request: (nextRequest = (nextRequest + 1) >>> 0), resolve, reject,
      sent: false, timer: setTimeout(() => finishStatus(true), 15000) };
    sendStatus();
  };
  const validStatus = value => (
    keys(value, ["status"]) && ["enabled", "disabled", "requires_approval", "failed"].includes(value.status)
  ) || (keys(value, ["status", "reason"]) && value.status === "unavailable"
    && ["unsupported_platform", "not_app_bundle", "identity_mismatch", "api_unavailable", "not_found"].includes(value.reason));
  window.addEventListener("message", event => {
    if (closed || event.source !== null || event.origin !== "") return;
    if (typeof event.data !== "string" || event.data.length > 16384) return;
    let message;
    try { message = JSON.parse(event.data); } catch (_) { return; }
    if (message?.document !== documentId) return;
    if (message.kind === "credentials-ready") {
      ready = true;
      sendPending();
      sendStatus();
      return;
    }
    if (message.kind === "autostart-reply") {
      if (!statusPending || message.request !== statusPending.request) return;
      finishStatus(!keys(message, ["kind", "request", "document", "status"]) || !validStatus(message.status), message.status);
      return;
    }
    if (message.kind !== "credential-reply" || !pending || message.request !== pending.request) return;
    const reply = message.reply;
    if (pending.call.operation === "load" && keys(reply, ["outcome", "lease"]) && reply.outcome === "loaded") {
      finish(false, reply.lease);
      return;
    }
    if (pending.call.operation === "save" && keys(reply, ["outcome"]) && reply.outcome === "saved") {
      finish(false, null);
      return;
    }
    if (pending.call.operation === "delete" && keys(reply, ["outcome", "existed"])
      && reply.outcome === "deleted" && typeof reply.existed === "boolean") {
      finish(false, reply.existed);
      return;
    }
    finish(true);
  });
  window.addEventListener("pagehide", () => { closed = true; finish(true); finishStatus(true); });
  // The window holds one Panel page and has no back button: a link off the
  // Panel's origin (or to the server's artifact bytes, which are documents)
  // goes to the host, which decides again and opens it in the system.
  // Capture phase, so it runs before the page's own handlers.
  document.addEventListener("click", event => {
    if (closed || event.defaultPrevented) return;
    const anchor = event.target?.closest?.("a[href]");
    if (!anchor) return;
    let url;
    try { url = new URL(anchor.href, location.href); } catch (_) { return; }
    if (url.origin === location.origin && !url.pathname.startsWith("/artifact/")) return;
    event.preventDefault();
    const payload = JSON.stringify({ kind: "open-external", document: documentId, url: url.href });
    if (payload.length > 16384) return;
    try { window.nsh.postMessage(prefix + payload); } catch (_) {}
  }, true);
  Object.defineProperty(window, "__ALEPH_PANEL_KEYRING__", { value: true });
  const mark = () => document.documentElement?.setAttribute("data-shell", "aleph-nsh");
  if (document.documentElement) mark();
  else document.addEventListener("DOMContentLoaded", mark, { once: true });
  Object.defineProperty(window, "__alephShellBridge", {
    value: Object.freeze({
      document: documentId,
      reply(payload) {
        if (!closed && typeof payload === "string" && payload.length <= 16384) {
          window.nsh.postMessage(prefix + payload);
        }
      },
      autostartStatus(...args) {
        return new Promise((resolve, reject) => {
          if (closed || statusPending || args.length !== 0) {
            reject(new Error("native autostart status rejected or unavailable")); return;
          }
          askStatus({ kind: "autostart-request" }, resolve, reject);
        });
      },
      autostartSet(...args) {
        return new Promise((resolve, reject) => {
          if (closed || statusPending || args.length !== 1 || typeof args[0] !== "boolean") {
            reject(new Error("native autostart status rejected or unavailable")); return;
          }
          askStatus({ kind: "autostart-set", enabled: args[0] }, resolve, reject);
        });
      },
      invoke(command, args) {
        return new Promise((resolve, reject) => {
          if (closed || pending) { reject(failure()); return; }
          let call;
          try { call = callFor(command, args); } catch (_) { reject(failure()); return; }
          pending = { request: (nextRequest = (nextRequest + 1) >>> 0), call,
            resolve, reject, sent: false, timer: setTimeout(() => finish(true), 15000) };
          sendPending();
        });
      },
    }),
  });
}
