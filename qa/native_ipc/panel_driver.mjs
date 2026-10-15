// Task #56 panel_driver: JS-ONLY self-test harness for the QA monitor/driver script that
// `desktop/shell/src/main.rs` injects (`QA_MONITOR_JS`). Pure Node stdlib; importing this
// module has NO side effects (no I/O, no sockets, no processes). `--self-test` (when run
// directly) reads main.rs as text, extracts the script and runs it inside `node:vm` against
// MOCKS (WebSocket / document / window / Event / timers). Nothing here opens a socket,
// launches the shell, touches a keyring, or reads any credential.
//
// WHAT THIS PROVES (against the SOURCE TEXT of the script, in a mock page):
//   - the nonce in `hello.auth` is compared exactly (and Origin) by the contract validator;
//   - unknown / malformed / replayed / wrong-run / extra-key commands are refused (ack
//     `refused`, ok:false) and trigger NO DOM action;
//   - the ticket never appears on any outgoing event and is not retained after use;
//   - absent / disabled targets give `ok:false` (never a silent success);
//   - the script never writes `__TAURI_INTERNALS__` / `invoke` (the mock models the REAL immutable
//     shape: a frozen object with a non-writable, non-configurable `invoke`) and emits no invoke
//     events; credential execution evidence is the native trace (monitor_contract.mjs), not this
//     script. A mock-only check: it is NOT evidence about the real Tauri IPC.
// WHAT IT DOES NOT PROVE: that the Rust compiles, that the real WebView runs the script, that
// the real Leptos DOM has these `data-testid`s, or anything about the real IPC. Those need a
// native run (see task56-auth-monitor.md).

import fs from "node:fs";
import path from "node:path";
import vm from "node:vm";
import { fileURLToPath } from "node:url";
import {
  ACTIONS,
  DOM_STATE_KEYS,
  MAX_COMMAND_CHARS,
  MAX_TICKET_CHARS,
  StreamValidator,
  authEquals,
  buildCommand,
  validateEvent,
} from "./monitor_contract.mjs";

const HERE = path.dirname(fileURLToPath(import.meta.url));
const MAIN_RS = path.join(HERE, "..", "..", "desktop", "shell", "src", "main.rs");
const SCRIPT_OPEN = 'const QA_MONITOR_JS: &str = r#"';
const SCRIPT_CLOSE = '"#;';
const PORT_PLACEHOLDER = "__QA_MONITOR_PORT__";
const NONCE_PLACEHOLDER = "__QA_MONITOR_NONCE__";

/** Extract the raw (still templated) monitor script from the text of main.rs. */
export function extractMonitorScript(mainRsText) {
  const text = String(mainRsText).replace(/\r\n/g, "\n");
  const start = text.indexOf(SCRIPT_OPEN);
  if (start < 0 || text.indexOf(SCRIPT_OPEN, start + 1) >= 0) throw new Error("script_open_not_unique");
  const bodyStart = start + SCRIPT_OPEN.length;
  const end = text.indexOf(SCRIPT_CLOSE, bodyStart);
  if (end < 0) throw new Error("script_close_missing");
  return text.slice(bodyStart, end);
}

/** Mirror of `qa_monitor_script` in main.rs: validated nonce as a JSON string literal. */
export function renderMonitorScript(template, { port, nonce }) {
  if (!Number.isInteger(port) || port < 1 || port > 65535) throw new Error("bad_port");
  if (typeof nonce !== "string" || !/^[0-9a-fA-F]{16,128}$/.test(nonce)) throw new Error("bad_nonce");
  if (template.split(PORT_PLACEHOLDER).length !== 2 || template.split(NONCE_PLACEHOLDER).length !== 2) {
    throw new Error("placeholder_count");
  }
  return template.replace(PORT_PLACEHOLDER, () => String(port)).replace(NONCE_PLACEHOLDER, () => JSON.stringify(nonce));
}

const TID = (id) => `[data-testid=${id}]`;

/**
 * A mock page. `dom` maps the exact selector strings the script uses to element mocks (or is
 * missing => `querySelector` returns null). Everything is recorded for assertions.
 */
export function createMockPage({ script, internals = "default", bytes = 7 } = {}) {
  const log = { queried: [], clicks: [], nativeSets: [], ownSets: [], inputEvents: [], reloads: 0, sockets: [], timers: [] };
  const dom = new Map();

  class HTMLInputElement {
    constructor() {
      this._v = "";
      this.tagName = "INPUT";
      this.type = "password";
      this.onInput = null;
      // A framework-style own accessor trap: assigning `input.value = x` directly would hit
      // this and be recorded as `own`; the script must use the prototype's native setter.
      Object.defineProperty(this, "value", {
        configurable: true,
        get: () => this._v,
        set: (x) => {
          log.ownSets.push("own");
          this._v = x;
        },
      });
    }
    dispatchEvent(ev) {
      log.inputEvents.push({ type: ev.type, bubbles: ev.bubbles === true, value: this._v });
      if (this.onInput) this.onInput(this);
      return true;
    }
  }
  Object.defineProperty(HTMLInputElement.prototype, "value", {
    configurable: true,
    get() {
      return this._v;
    },
    set(x) {
      log.nativeSets.push(x);
      this._v = x;
    },
  });
  class Event {
    constructor(type, init) {
      this.type = type;
      this.bubbles = !!(init && init.bubbles);
    }
  }
  class MockWebSocket {
    constructor(url) {
      this.url = url;
      this.sent = [];
      this.closed = false;
      this.onopen = null;
      this.onmessage = null;
      this.onclose = null;
      this.onerror = null;
      log.sockets.push(this);
    }
    send(s) {
      this.sent.push(s);
    }
    close() {
      this.closed = true;
    }
  }
  const timers = [];
  const win = {
    HTMLInputElement,
    location: {
      reload() {
        log.reloads += 1;
      },
    },
    crypto: {
      getRandomValues(a) {
        for (let i = 0; i < a.length; i++) a[i] = (bytes + i * 13) & 0xff;
        return a;
      },
    },
    listeners: {},
    addEventListener(t, f) {
      (this.listeners[t] ||= []).push(f);
    },
  };
  const invokeLog = { calls: [], thisSeen: [] };
  const ti = { name: "ti" };
  if (internals === "default") {
    // Models real Tauri: the internals object is frozen and `invoke` is a non-writable, non-configurable
    // data property, and the global itself cannot be reassigned. A script that tried to wrap it would
    // fail (strict mode: TypeError), exactly like the real Native run's `not_writable`.
    Object.defineProperty(ti, "invoke", {
      value: function invoke(cmd, args) {
        invokeLog.calls.push({ cmd, args });
        invokeLog.thisSeen.push(this);
        return Promise.resolve(null);
      },
      writable: false,
      configurable: false,
      enumerable: true,
    });
    Object.freeze(ti);
    Object.defineProperty(win, "__TAURI_INTERNALS__", { value: ti, writable: false, configurable: false, enumerable: true });
  } else if (internals === "no_invoke") {
    win.__TAURI_INTERNALS__ = {};
  } else if (internals === "none") {
    win.__TAURI_INTERNALS__ = null;
  }
  const document = {
    querySelector(sel) {
      log.queried.push(sel);
      return dom.has(sel) ? dom.get(sel) : null;
    },
  };
  const sandbox = {
    window: win,
    document,
    WebSocket: MockWebSocket,
    Event,
    setTimeout(fn, ms) {
      timers.push({ fn, ms });
      log.timers.push(ms);
      return timers.length;
    },
  };
  // Make `window.X` and free `X` agree for the identifiers the script uses.
  const ctx = vm.createContext(sandbox);
  vm.runInContext(script, ctx, { filename: "QA_MONITOR_JS", timeout: 2000 });

  const page = {
    log,
    dom,
    win,
    ti,
    invokeLog,
    get ws() {
      return log.sockets[0] || null;
    },
    el(sel, props = {}) {
      const el = {
        tagName: "BUTTON",
        disabled: false,
        attrs: {},
        getAttribute(n) {
          return Object.hasOwn(this.attrs, n) ? this.attrs[n] : null;
        },
        click() {
          log.clicks.push(sel);
          if (this.onClick) this.onClick(this);
        },
        ...props,
      };
      dom.set(sel, el);
      return el;
    },
    input(sel = TID("token-wall-input")) {
      const el = new HTMLInputElement();
      dom.set(sel, el);
      return el;
    },
    open() {
      if (this.ws && this.ws.onopen) this.ws.onopen();
    },
    deliver(raw) {
      if (this.ws && this.ws.onmessage) this.ws.onmessage({ data: raw });
    },
    /** Run all queued timers (including ones queued while running), bounded. */
    flush(limit = 200) {
      let n = 0;
      while (timers.length && n < limit) {
        const t = timers.shift();
        t.fn();
        n += 1;
      }
      return n;
    },
    pendingTimers: () => timers.length,
    events() {
      return (this.ws ? this.ws.sent : []).map((s) => JSON.parse(s));
    },
    rawSent() {
      return this.ws ? this.ws.sent.slice() : [];
    },
  };
  return page;
}

// ---------------------------------------------------------------- self test

export function selfTest() {
  const results = [];
  const t = (name, fn) => {
    try {
      const r = fn();
      results.push({ name, ok: r === undefined || r === true });
    } catch (e) {
      results.push({ name, ok: false, error: String(e && e.message) });
    }
  };
  const eq = (a, b) => {
    if (JSON.stringify(a) !== JSON.stringify(b)) throw new Error(`expected ${JSON.stringify(b)} got ${JSON.stringify(a)}`);
  };
  const truthy = (v, msg) => {
    if (!v) throw new Error(msg || "expected truthy");
  };
  const throws = (fn, code) => {
    try {
      fn();
    } catch (e) {
      if (code !== undefined && e.message !== code) throw new Error(`expected throw ${code} got ${e.message}`);
      return;
    }
    throw new Error("expected throw");
  };

  const NONCE = "0123456789abcdef0123456789abcdef";
  const ORIGIN = "http://127.0.0.1:18789";
  const TICKET = "TICKET-SENTINEL-9f2c7d41";
  const mainText = fs.readFileSync(MAIN_RS, "utf8");
  const template = extractMonitorScript(mainText);
  const script = renderMonitorScript(template, { port: 18790, nonce: NONCE });

  const SEL = {
    wall: TID("token-wall"),
    input: TID("token-wall-input"),
    submit: TID("token-wall-submit"),
    rejected: TID("token-wall-instruction-rejected"),
    notice: TID("pairing-notice"),
    pair: TID("pairing-notice-pair-desktop"),
    decision: TID("browser-live-decision"),
    launch: TID("browser-live-launch"),
    frame: TID("browser-live-frame"),
    status: TID("browser-live-status"),
    workspace: TID("workspace-panel"),
    toggle: TID("layout-toggle"),
    browserTab: TID("workspace-tab-browser"),
  };

  /** Open a page + harness-side validator pair, hello consumed and authenticated. */
  function connect(opts = {}) {
    const page = createMockPage({ script, ...opts });
    page.open();
    const validator = new StreamValidator({ expectedAuth: NONCE, expectedOrigin: ORIGIN, requestOrigin: ORIGIN });
    const h = {
      page,
      validator,
      consumed: 0,
      nextId: 0,
      pull() {
        const ev = page.events();
        const fresh = ev.slice(this.consumed);
        this.consumed = ev.length;
        const out = [];
        for (const e of fresh) {
          const r = validator.push(e);
          out.push({ e, r });
        }
        return out;
      },
      /** Harness: expectCommand + buildCommand + deliver + timers, returns pushed events. */
      drive(action, extra = {}) {
        const r = validator.expectCommand(action);
        if (!r.ok) throw new Error(`expect:${r.error}`);
        const raw = buildCommand({ run: this.run(), id: this.nextId, action, ...extra });
        page.deliver(raw);
        this.nextId += 1;
        page.flush();
        return this.pull();
      },
      run() {
        return page.events()[0].run;
      },
    };
    const first = h.pull();
    h.first = first;
    return h;
  }
  const ackOf = (pulled) => pulled.filter((x) => x.e.type === "command_ack").map((x) => x.e)[0];
  const allOk = (pulled) => pulled.every((x) => x.r.ok);
  /** Deliver an arbitrary RAW string with no harness bookkeeping; return fresh events. */
  function rawCommand(h, raw) {
    h.page.deliver(raw);
    h.page.flush();
    const ev = h.page.events();
    const fresh = ev.slice(h.consumed);
    h.consumed = ev.length;
    return fresh;
  }
  const env = (h, over = {}, drop = []) => {
    const o = { v: 1, type: "command", run: h.run(), id: h.nextId, command: "inspect", ...over };
    for (const k of drop) delete o[k];
    return JSON.stringify(o);
  };

  // ---- extraction / rendering
  t("extract: script found once, both placeholders present exactly once, no raw-string terminator", () => {
    eq(template.split(PORT_PLACEHOLDER).length, 2);
    eq(template.split(NONCE_PLACEHOLDER).length, 2);
    eq(template.includes('"#'), false);
    truthy(template.trim().startsWith("(function () {"));
  });
  t("render: nonce as JSON literal, port as digits; bad port/nonce refused", () => {
    truthy(script.includes(`var AUTH = "${NONCE}";`));
    truthy(script.includes("var PORT = 18790;"));
    eq(script.includes("__QA_MONITOR"), false);
    throws(() => renderMonitorScript(template, { port: 0, nonce: NONCE }), "bad_port");
    throws(() => renderMonitorScript(template, { port: 18790, nonce: "zz".repeat(16) }), "bad_nonce");
    throws(() => renderMonitorScript(template, { port: 18790, nonce: "ab".repeat(8) + '";alert(1);"' }), "bad_nonce");
    throws(() => renderMonitorScript(template, { port: 18790, nonce: "abc" }), "bad_nonce");
  });
  t("static: script has no eval/Function/innerHTML/fetch/XHR/storage/cookie/arbitrary selector input", () => {
    for (const bad of ["eval(", "Function(", "innerHTML", "fetch(", "XMLHttpRequest", "localStorage", "sessionStorage", "document.cookie", "import(", "postMessage", "console."]) {
      eq(script.includes(bad), false);
    }
    // The only DOM lookups use the compile-time SEL table; `q(` is never called with data.
    const qCalls = [...script.matchAll(/\bq\(([^)]*)\)/g)].map((m) => m[1]);
    truthy(qCalls.length >= 8);
    for (const a of qCalls) truthy(/^(sel|SEL\.[A-Za-z]+)$/.test(a), `q arg ${a}`);
    // ticket only ever read from the parsed command into the local `act`, never emitted.
    eq(/emit\([^;]*ticket/i.test(script), false);
    eq(/ws\.send\([^;]*ticket/i.test(script), false);
  });

  // ---- hello / nonce
  t("hello: first event is hello with contract + exact nonce in auth; run token differs from nonce", () => {
    const h = connect();
    const [e0] = h.page.events();
    eq(Object.keys(e0), ["v", "type", "run", "seq", "contract", "auth"]);
    eq(e0.type, "hello");
    eq(e0.seq, 0);
    eq(e0.contract, "aleph-qa-monitor-v1");
    eq(e0.auth, NONCE);
    truthy(e0.run !== NONCE && /^[0-9a-f]{32}$/.test(e0.run));
    eq(validateEvent(e0).ok, true);
    eq(allOk(h.first), true);
    eq(h.validator.finish({ closedByHarness: true }).authenticated, true);
    eq(h.page.ws.url, "ws://127.0.0.1:18790/qa");
  });
  t("nonce compare: exact match only (wrong, near, short, long, case-changed, missing)", () => {
    const hello = (auth) => ({ v: 1, type: "hello", run: "ab".repeat(16), seq: 0, contract: "aleph-qa-monitor-v1", ...(auth === undefined ? {} : { auth }) });
    const mk = (over = {}) => new StreamValidator({ expectedAuth: NONCE, expectedOrigin: ORIGIN, requestOrigin: ORIGIN, ...over });
    eq(mk().push(hello(NONCE)).ok, true);
    const flip = NONCE.slice(0, -1) + (NONCE.endsWith("f") ? "e" : "f");
    eq(mk().push(hello(flip)), { ok: false, error: "auth_mismatch" });
    eq(mk().push(hello(NONCE.slice(1))), { ok: false, error: "auth_mismatch" });
    eq(mk().push(hello(NONCE + "0")), { ok: false, error: "auth_mismatch" });
    eq(mk().push(hello(NONCE.toUpperCase())), { ok: false, error: "auth_mismatch" });
    eq(mk().push(hello(undefined)), { ok: false, error: "missing_key" });
    eq(mk().push(hello(12345)), { ok: false, error: "bad_field" });
    eq(authEquals(NONCE, NONCE), true);
    eq(authEquals(NONCE, flip), false);
    eq(authEquals(NONCE, 5), false);
  });
  t("origin compare + fail-closed when expectations are not configured", () => {
    const hello = { v: 1, type: "hello", run: "ab".repeat(16), seq: 0, contract: "aleph-qa-monitor-v1", auth: NONCE };
    eq(new StreamValidator({ expectedAuth: NONCE, expectedOrigin: ORIGIN, requestOrigin: "http://127.0.0.1:18788" }).push(hello), { ok: false, error: "origin_mismatch" });
    eq(new StreamValidator({ expectedAuth: NONCE, expectedOrigin: ORIGIN, requestOrigin: ORIGIN + "/" }).push(hello), { ok: false, error: "origin_mismatch" });
    eq(new StreamValidator({ expectedAuth: NONCE, expectedOrigin: ORIGIN }).push(hello), { ok: false, error: "origin_not_configured" });
    eq(new StreamValidator({ expectedAuth: NONCE, requestOrigin: ORIGIN }).push(hello), { ok: false, error: "origin_not_configured" });
    eq(new StreamValidator({ expectedOrigin: ORIGIN, requestOrigin: ORIGIN }).push(hello), { ok: false, error: "auth_not_configured" });
    eq(new StreamValidator({ credentialNonce: NONCE }).push(hello), { ok: false, error: "auth_not_configured" });
    eq(new StreamValidator({ expectedAuth: "not hex", expectedOrigin: ORIGIN, requestOrigin: ORIGIN }).push(hello), { ok: false, error: "auth_not_configured" });
    eq(new StreamValidator({ expectedAuth: NONCE, expectedOrigin: ORIGIN, requestOrigin: ORIGIN }).push({ ...hello, run: NONCE }), { ok: false, error: "run_equals_auth" });
  });
  t("nonce appears ONLY in hello.auth: never in any later event (even after commands)", () => {
    const h = connect();
    h.drive("inspect");
    h.page.el(SEL.pair);
    h.drive("ask_pair");
    const ev = h.page.events();
    eq(ev.filter((e) => JSON.stringify(e).includes(NONCE)).map((e) => e.type), ["hello"]);
  });
  t("install_failed: no hello, no auth on the wire, untrusted, and commands are IGNORED", () => {
    const page = createMockPage({ script, internals: "none" });
    page.open();
    const ev = page.events();
    eq(ev.map((e) => e.type), ["install_failed"]);
    eq(JSON.stringify(ev).includes(NONCE), false);
    page.el(SEL.pair);
    page.deliver(JSON.stringify({ v: 1, type: "command", run: ev[0].run, id: 0, command: "ask_pair" }));
    page.flush();
    eq(page.log.clicks, []);
    eq(page.events().length, 1);
    eq(page.ws.closed, true);
    const v = new StreamValidator({ expectedAuth: NONCE, expectedOrigin: ORIGIN, requestOrigin: ORIGIN });
    eq(v.push(ev[0]).ok, true);
    eq(v.finish({ closedByHarness: true }).trusted, false);
    eq(v.expectCommand("inspect"), { ok: false, error: "not_authenticated" });
  });

  // ---- command refusal
  t("commands: unknown / camelCase / eval-ish names are refused with ack refused,false and no DOM action", () => {
    const h = connect();
    h.page.el(SEL.pair);
    h.page.input();
    h.page.el(SEL.submit);
    for (const command of ["eval", "askPair", "ASK_PAIR", "click", "invoke", "navigate", "", "ask_pair ", "__proto__", "constructor", 5, null, ["inspect"]]) {
      const fresh = rawCommand(h, env(h, { command }));
      eq(fresh.map((e) => [e.type, e.action, e.ok]), [["command_ack", "refused", false]]);
    }
    eq(h.page.log.clicks, []);
    eq(h.page.log.nativeSets, []);
    eq(h.page.log.reloads, 0);
  });
  t("commands: extra/missing key, wrong run, wrong v/type, bad id, non-JSON, oversize, non-string => refused", () => {
    const h = connect();
    h.page.el(SEL.pair);
    const refused = (raw) => {
      const fresh = rawCommand(h, raw);
      eq(fresh.map((e) => [e.type, e.action, e.ok]), [["command_ack", "refused", false]]);
    };
    refused(env(h, { selector: "#x" }));
    refused(env(h, { script: "alert(1)" }));
    refused(env(h, { ticket: "x" })); // ticket is for submit_ticket only
    refused(env(h, {}, ["id"]));
    refused(env(h, {}, ["run"]));
    refused(env(h, { run: "0".repeat(32) }));
    refused(env(h, { run: undefined }));
    refused(env(h, { v: 2 }));
    refused(env(h, { type: "cmd" }));
    refused(env(h, { id: 1 })); // skipped id
    refused(env(h, { id: -1 }));
    refused(env(h, { id: 0.5 }));
    refused(env(h, { id: "0" }));
    refused(env(h, { id: 99999 }));
    refused("not json");
    refused("[]");
    refused("null");
    refused('"inspect"');
    refused(JSON.stringify({ v: 1, type: "command", run: h.run(), id: 0, command: "inspect", pad: "x".repeat(MAX_COMMAND_CHARS) }));
    h.page.deliver({ not: "a string" });
    h.page.deliver(12);
    h.page.deliver(null);
    const ev = h.page.events().slice(h.consumed);
    h.consumed += ev.length;
    eq(ev.length, 3);
    for (const e of ev) eq([e.type, e.action, e.ok], ["command_ack", "refused", false]);
    eq(h.page.log.clicks, []);
    // A refused command does not consume an id: the correct id 0 still works afterwards.
    const ok = h.drive("inspect");
    eq(ackOf(ok).ok, true);
  });
  t("commands: replayed id refused; ids must be strictly sequential", () => {
    const h = connect();
    const a = h.drive("inspect");
    eq(ackOf(a), { v: 1, type: "command_ack", run: h.run(), seq: ackOf(a).seq, action: "inspect", ok: true });
    const replay = rawCommand(h, JSON.stringify({ v: 1, type: "command", run: h.run(), id: 0, command: "inspect" }));
    eq(replay.map((e) => [e.action, e.ok]), [["refused", false]]);
    const b = h.drive("inspect");
    eq(ackOf(b).ok, true);
  });
  t("commands: ticket validation (missing/empty/non-string/too long/extra on others) refused, no DOM action", () => {
    const h = connect();
    const inp = h.page.input();
    h.page.el(SEL.submit);
    const refused = (raw) => eq(rawCommand(h, raw).map((e) => [e.action, e.ok]), [["refused", false]]);
    refused(env(h, { command: "submit_ticket" }));
    refused(env(h, { command: "submit_ticket", ticket: "" }));
    refused(env(h, { command: "submit_ticket", ticket: 5 }));
    refused(env(h, { command: "submit_ticket", ticket: null }));
    refused(env(h, { command: "submit_ticket", ticket: "x".repeat(MAX_TICKET_CHARS + 1) }));
    refused(env(h, { command: "submit_ticket", ticket: "x", extra: 1 }));
    refused(env(h, { command: "reload", ticket: "x" }));
    eq(h.page.log.nativeSets, []);
    eq(inp.value, "");
    eq(h.page.log.clicks, []);
    eq(h.page.log.reloads, 0);
  });
  t("commands: refused events never echo any command field (incl. a would-be ticket)", () => {
    const h = connect();
    rawCommand(h, env(h, { command: "submit_ticket", ticket: TICKET, extra: TICKET }));
    rawCommand(h, env(h, { command: TICKET }));
    rawCommand(h, TICKET);
    eq(h.page.rawSent().some((s) => s.includes(TICKET)), false);
  });
  t("commands: one in flight; a second command while busy is refused", () => {
    const h = connect();
    h.page.input();
    const submit = h.page.el(SEL.submit, { disabled: true });
    h.validator.expectCommand("submit_ticket");
    h.page.deliver(buildCommand({ run: h.run(), id: 0, action: "submit_ticket", ticket: TICKET }));
    h.nextId = 1;
    // poll timer pending (submit disabled): command still in flight
    truthy(h.page.pendingTimers() > 0);
    h.page.deliver(buildCommand({ run: h.run(), id: 1, action: "inspect" })); // no timer flush: still busy
    const during = h.page.events().slice(h.consumed);
    h.consumed += during.length;
    eq(during.map((e) => [e.action, e.ok]), [["refused", false]]);
    submit.disabled = false;
    h.page.flush();
    const rest = h.page.events().slice(h.consumed);
    eq(rest.map((e) => [e.type, e.action, e.ok]), [["command_ack", "submit_ticket", true]]);
    eq(h.page.log.clicks, [SEL.submit]);
  });

  // ---- ask_pair
  t("ask_pair: absent => ack false (no silent success), disabled => false, aria-disabled => false, non-button => false", () => {
    const h = connect();
    eq(ackOf(h.drive("ask_pair")).ok, false);
    h.page.el(SEL.pair, { disabled: true });
    eq(ackOf(h.drive("ask_pair")).ok, false);
    h.page.el(SEL.pair, { attrs: { "aria-disabled": "true" } });
    eq(ackOf(h.drive("ask_pair")).ok, false);
    h.page.el(SEL.pair, { tagName: "DIV" });
    eq(ackOf(h.drive("ask_pair")).ok, false);
    eq(h.page.log.clicks, []);
    eq(h.validator.failed, null);
  });
  t("ask_pair: enabled real button clicked exactly once, ack ok:true, only the fixed Pair selector clicked", () => {
    const h = connect();
    h.page.el(SEL.pair);
    const pulled = h.drive("ask_pair");
    eq(ackOf(pulled).action, "ask_pair");
    eq(ackOf(pulled).ok, true);
    eq(h.page.log.clicks, [SEL.pair]);
    eq(allOk(pulled), true);
  });

  // ---- submit_ticket
  t("submit_ticket: native prototype setter + bubbling input event, then real submit click; ack true; ticket not on wire", () => {
    const h = connect();
    const inp = h.page.input();
    const submit = h.page.el(SEL.submit, { disabled: true });
    inp.onInput = (i) => {
      submit.disabled = i.value === "";
    };
    const pulled = h.drive("submit_ticket", { ticket: TICKET });
    eq(h.page.log.nativeSets, [TICKET]);
    eq(h.page.log.ownSets, []); // never assigned through the framework-visible own accessor
    eq(h.page.log.inputEvents.map((x) => [x.type, x.bubbles, x.value === TICKET]), [["input", true, true]]);
    eq(h.page.log.clicks, [SEL.submit]);
    const ack = ackOf(pulled);
    eq([ack.action, ack.ok], ["submit_ticket", true]);
    eq(allOk(pulled), true);
    eq(h.page.rawSent().some((s) => s.includes(TICKET)), false);
    eq(JSON.stringify(h.page.events()).includes(TICKET), false);
  });
  t("submit_ticket: absent input / absent submit / non-password input => ack false and the ticket is never written", () => {
    const h = connect();
    eq(ackOf(h.drive("submit_ticket", { ticket: TICKET })).ok, false); // nothing present
    const inp = h.page.input();
    eq(ackOf(h.drive("submit_ticket", { ticket: TICKET })).ok, false); // no submit button
    h.page.el(SEL.submit);
    inp.type = "text";
    eq(ackOf(h.drive("submit_ticket", { ticket: TICKET })).ok, false); // not a password input
    eq(h.page.log.nativeSets, []);
    eq(h.page.log.clicks, []);
    eq(h.page.rawSent().some((s) => s.includes(TICKET)), false);
  });
  t("submit_ticket: submit stays disabled => bounded polling then ack false, input reset to empty, no click", () => {
    const h = connect();
    const inp = h.page.input();
    h.page.el(SEL.submit, { disabled: true });
    const pulled = h.drive("submit_ticket", { ticket: TICKET });
    eq(ackOf(pulled).ok, false);
    eq(h.page.log.clicks, []);
    eq(inp.value, ""); // ticket erased from the DOM on failure
    eq(h.page.log.nativeSets, [TICKET, ""]);
    truthy(h.page.log.timers.length <= 22, "poll bound");
    eq(h.page.rawSent().some((s) => s.includes(TICKET)), false);
  });
  t("submit_ticket: no secret serialization: ticket is in no emitted event across success/failure/refusal", () => {
    const h = connect();
    h.page.input();
    h.page.el(SEL.submit);
    h.drive("submit_ticket", { ticket: TICKET });
    h.drive("inspect");
    rawCommand(h, env(h, { command: "submit_ticket", ticket: TICKET, id: 99 }));
    const blob = JSON.stringify(h.page.events());
    eq(blob.includes(TICKET), false);
    eq(/"ticket"s*:/.test(blob), false); // no ticket key on any event (the action enum may name submit_ticket)
    for (const e of h.page.events()) eq(validateEvent(e).ok, true);
  });

  // ---- reveal_browser (fixed, bounded: layout-toggle -> workspace-tab-browser)
  t("reveal_browser: toggle absent => ack false, nothing clicked, no polling", () => {
    const h = connect();
    const pulled = h.drive("reveal_browser");
    eq(ackOf(pulled).action, "reveal_browser");
    eq(ackOf(pulled).ok, false);
    eq(h.page.log.clicks, []);
    eq(h.page.log.timers, []);
    eq(allOk(pulled), true);
  });
  t("reveal_browser: toggle disabled => ack false, no click", () => {
    const h = connect();
    h.page.el(SEL.toggle, { disabled: true });
    eq(ackOf(h.drive("reveal_browser")).ok, false);
    h.page.el(SEL.toggle, { attrs: { "aria-disabled": "true" } });
    eq(ackOf(h.drive("reveal_browser")).ok, false);
    h.page.el(SEL.toggle, { tagName: "DIV" });
    eq(ackOf(h.drive("reveal_browser")).ok, false);
    eq(h.page.log.clicks, []);
  });
  t("reveal_browser: clicks toggle, then the enabled browser tab, ack true; only the two fixed selectors clicked", () => {
    const h = connect();
    h.page.el(SEL.toggle, {
      onClick: () => {
        h.page.el(SEL.workspace, { tagName: "ASIDE" });
        h.page.el(SEL.browserTab);
      },
    });
    const pulled = h.drive("reveal_browser");
    eq(ackOf(pulled).ok, true);
    eq(h.page.log.clicks, [SEL.toggle, SEL.browserTab]);
    eq(allOk(pulled), true);
    eq(h.drive("inspect")[0].e.browserTab, true);
  });
  t("reveal_browser: workspace already open => toggle NOT clicked (it would close the pane); tab clicked", () => {
    const h = connect();
    h.page.el(SEL.workspace, { tagName: "ASIDE" });
    h.page.el(SEL.toggle);
    h.page.el(SEL.browserTab);
    eq(ackOf(h.drive("reveal_browser")).ok, true);
    eq(h.page.log.clicks, [SEL.browserTab]);
  });
  t("reveal_browser: collapsed pane (workspace-collapsed class) counts as NOT open => toggle clicked", () => {
    const h = connect();
    h.page.el(SEL.workspace, { tagName: "ASIDE", classList: { contains: (c) => c === "workspace-collapsed" } });
    h.page.el(SEL.toggle, { onClick: () => h.page.el(SEL.browserTab) });
    eq(ackOf(h.drive("reveal_browser")).ok, true);
    eq(h.page.log.clicks, [SEL.toggle, SEL.browserTab]);
  });
  t("reveal_browser: tab appears late => polled then clicked; tab never enabled => bounded poll (<=500ms), ack false, tab not clicked", () => {
    let h = connect();
    let reads = 0;
    h.page.el(SEL.workspace, { tagName: "ASIDE" });
    const late = h.page.el(SEL.browserTab);
    // spread in el() would freeze a getter, so define it on the created element
    Object.defineProperty(late, "disabled", {
      get() {
        reads += 1;
        return reads < 4;
      },
    });
    eq(ackOf(h.drive("reveal_browser")).ok, true);
    eq(h.page.log.clicks, [SEL.browserTab]);
    h = connect();
    h.page.el(SEL.workspace, { tagName: "ASIDE" });
    h.page.el(SEL.browserTab, { disabled: true });
    eq(ackOf(h.drive("reveal_browser")).ok, false);
    eq(h.page.log.clicks, []);
    truthy(h.page.log.timers.length <= 20, "poll bound");
    truthy(h.page.log.timers.every((ms) => ms <= 25), "poll step");
    truthy(h.page.log.timers.reduce((a, b) => a + b, 0) <= 500, "500ms budget");
    eq(h.page.pendingTimers(), 0);
    h = connect();
    h.page.el(SEL.workspace, { tagName: "ASIDE" });
    eq(ackOf(h.drive("reveal_browser")).ok, false); // tab absent
    eq(h.page.log.clicks, []);
  });
  t("reveal_browser: schema stays exact: extra keys / ticket / selector refused, nothing clicked, ticket not on wire", () => {
    const h = connect();
    h.page.el(SEL.toggle);
    h.page.el(SEL.browserTab);
    for (const extra of [{ ticket: TICKET }, { selector: "#evil" }, { script: "1" }]) {
      const ev = rawCommand(h, env(h, { command: "reveal_browser", ...extra }));
      eq(ev.filter((e) => e.type === "command_ack").map((e) => [e.action, e.ok]), [["refused", false]]);
    }
    eq(rawCommand(h, env(h, { command: "reveal_browser " })).map((e) => e.ok), [false]);
    eq(rawCommand(h, env(h, { command: "revealBrowser" })).map((e) => e.ok), [false]);
    eq(h.page.log.clicks, []);
    eq(h.page.rawSent().some((s) => s.includes(TICKET)), false);
    throws(() => buildCommand({ run: h.run(), id: 0, action: "reveal_browser", ticket: TICKET }), "unexpected_ticket");
  });

  // ---- reload / inspect
  t("reload: ack true first, reload only after the delay timer; stale timers cannot reload twice", () => {
    const h = connect();
    h.validator.expectCommand("reload");
    h.page.deliver(buildCommand({ run: h.run(), id: 0, action: "reload" }));
    h.nextId = 1;
    eq(h.page.log.reloads, 0);
    const ack = h.pull().map((x) => x.e).find((e) => e.type === "command_ack");
    eq([ack.action, ack.ok], ["reload", true]);
    eq(h.page.log.timers, [100]);
    h.page.flush();
    eq(h.page.log.reloads, 1);
    // pagehide => monitor_end page_unload (the validator then completes the stream)
    for (const f of h.page.win.listeners.pagehide || []) f();
    h.pull();
    eq(h.validator.finish().complete, true);
  });
  t("inspect: dom_state is exactly the fixed boolean keys, emitted before the ack; validator accepts", () => {
    const h = connect();
    h.page.el(SEL.wall, { tagName: "DIV" });
    h.page.input();
    h.page.el(SEL.submit);
    h.page.el(SEL.notice, { tagName: "DIV" });
    h.page.el(SEL.pair);
    const pulled = h.drive("inspect");
    eq(pulled.map((x) => x.e.type), ["dom_state", "command_ack"]);
    const st = pulled[0].e;
    eq(Object.keys(st).slice(4), DOM_STATE_KEYS);
    for (const k of DOM_STATE_KEYS) eq(typeof st[k], "boolean");
    eq([st.tokenWall, st.tokenInput, st.tokenSubmitEnabled, st.pairingNotice, st.pairDesktop, st.pairDesktopEnabled], [true, true, true, true, true, true]);
    eq([st.tokenRejected, st.paired, st.resource, st.live, st.liveStatus], [false, false, false, false, false]);
    eq(allOk(pulled), true);
  });
  t("inspect: paired = decision/launch present AND pairing notice absent; disabled buttons report not-enabled", () => {
    const h = connect();
    h.page.el(SEL.decision, { tagName: "DIV" });
    h.page.el(SEL.frame, { tagName: "DIV" });
    h.page.el(SEL.submit, { disabled: true });
    const st = h.drive("inspect")[0].e;
    eq([st.paired, st.resource, st.live, st.tokenSubmitEnabled, st.pairingNotice], [true, true, true, false, false]);
    h.page.el(SEL.notice, { tagName: "DIV" });
    eq(h.drive("inspect")[0].e.paired, false);
  });
  t("selectors: only the fixed SEL table is ever queried (no harness-supplied selector)", () => {
    const h = connect();
    h.page.input();
    h.page.el(SEL.submit);
    h.page.el(SEL.pair);
    h.drive("inspect");
    h.drive("ask_pair");
    h.drive("submit_ticket", { ticket: TICKET });
    h.page.el(SEL.toggle);
    h.page.el(SEL.browserTab);
    h.drive("reveal_browser");
    rawCommand(h, env(h, { selector: "#evil", command: "inspect" }));
    const allowed = new Set(Object.values(SEL));
    for (const q of h.page.log.queried) truthy(allowed.has(q), `unexpected selector ${q}`);
  });

  // ---- validator sequencing
  t("validator: ack/dom_state require a pending harness command; mismatches fail closed", () => {
    const base = (h) => ({ v: 1, run: h.run(), seq: h.page.events().length });
    let h = connect();
    eq(h.validator.push({ ...base(h), type: "command_ack", action: "inspect", ok: true }), { ok: false, error: "unsolicited_ack" });
    h = connect();
    h.validator.expectCommand("ask_pair");
    eq(h.validator.push({ ...base(h), type: "command_ack", action: "reload", ok: true }), { ok: false, error: "ack_action_mismatch" });
    h = connect();
    h.validator.expectCommand("inspect");
    eq(h.validator.push({ ...base(h), type: "command_ack", action: "inspect", ok: true }), { ok: false, error: "inspect_without_state" });
    h = connect();
    eq(h.validator.push({ ...base(h), type: "dom_state", ...Object.fromEntries(DOM_STATE_KEYS.map((k) => [k, false])) }), { ok: false, error: "unsolicited_dom_state" });
    h = connect();
    h.validator.expectCommand("ask_pair");
    eq(h.validator.expectCommand("inspect"), { ok: false, error: "command_in_flight" });
    eq(h.validator.finish({ closedByHarness: true }).complete, false); // pending => not complete
    eq(h.validator.expectCommand("bogus"), { ok: false, error: "unknown_action" });
  });
  t("validator: refused ack must be ok:false; non-boolean / unknown action rejected by schema", () => {
    const h = connect();
    const e = { v: 1, type: "command_ack", run: h.run(), seq: 1, action: "refused", ok: true };
    eq(validateEvent(e), { ok: false, error: "bad_field" });
    eq(validateEvent({ ...e, ok: false }).ok, true);
    eq(validateEvent({ ...e, action: "eval", ok: false }), { ok: false, error: "bad_field" });
    eq(validateEvent({ ...e, action: "ask_pair", ok: "yes" }), { ok: false, error: "bad_field" });
    eq(validateEvent({ ...e, action: "ask_pair", ok: true, ticket: "x" }), { ok: false, error: "unknown_key" });
    eq(validateEvent({ v: 1, type: "dom_state", run: h.run(), seq: 1, ...Object.fromEntries(DOM_STATE_KEYS.map((k) => [k, "true"])) }), { ok: false, error: "bad_field" });
  });
  t("buildCommand: exact envelope, ticket only for submit_ticket, fixed errors never echo the ticket", () => {
    const run = "ab".repeat(16);
    eq(JSON.parse(buildCommand({ run, id: 0, action: "inspect" })), { v: 1, type: "command", run, id: 0, command: "inspect" });
    eq(Object.keys(JSON.parse(buildCommand({ run, id: 3, action: "submit_ticket", ticket: "t" }))).length, 6);
    for (const a of ACTIONS) truthy(typeof buildCommand({ run, id: 1, action: a, ...(a === "submit_ticket" ? { ticket: "t" } : {}) }) === "string");
    throws(() => buildCommand({ run, id: 0, action: "eval" }), "bad_action");
    throws(() => buildCommand({ run, id: 0, action: "inspect", ticket: "x" }), "unexpected_ticket");
    throws(() => buildCommand({ run, id: 0, action: "submit_ticket" }), "bad_ticket");
    throws(() => buildCommand({ run, id: 0, action: "submit_ticket", ticket: "" }), "bad_ticket");
    throws(() => buildCommand({ run, id: -1, action: "inspect" }), "bad_id");
    throws(() => buildCommand({ run: "short", id: 0, action: "inspect" }), "bad_run");
    try {
      buildCommand({ run, id: 0, action: "submit_ticket", ticket: TICKET.repeat(200) });
    } catch (e) {
      eq(e.message.includes("TICKET"), false);
    }
  });

  // ---- immutable Tauri internals: the script observes nothing on invoke and never writes it
  t("immutable internals (real-Tauri shape): hello still sent, descriptor untouched, invoke calls emit NOTHING", () => {
    const h = connect();
    const ti = h.page.ti;
    const d0 = Object.getOwnPropertyDescriptor(ti, "invoke");
    eq([d0.writable, d0.configurable, Object.isFrozen(ti)], [false, false, true]);
    const gd = Object.getOwnPropertyDescriptor(h.page.win, "__TAURI_INTERNALS__");
    eq([gd.writable, gd.configurable], [false, false]);
    const before = h.page.events().length;
    ti.invoke("panel_credential_save", { credential: "CRED-SENTINEL", lease_id: "LEASE-SENTINEL" });
    ti.invoke("panel_credential_load", {});
    eq(h.page.invokeLog.calls.length, 2);
    eq(Object.getOwnPropertyDescriptor(ti, "invoke"), d0); // same function, same attributes: never replaced
    eq(h.page.events().length, before); // no event for any invoke
    const types = h.page.events().map((e) => e.type);
    eq(types.includes("invoke_start") || types.includes("invoke_end"), false);
    eq(types[0], "hello");
    const blob = JSON.stringify(h.page.events());
    for (const x of ["CRED-SENTINEL", "LEASE-SENTINEL"]) eq(blob.includes(x), false, x);
  });
  t("immutable internals: the script source contains no invoke wrapping / invoke event vocabulary", () => {
    for (const bad of ["invoke_start", "invoke_end", "not_writable", "ti.invoke =", "wrapped ="]) eq(script.includes(bad), false, bad);
  });
  t("immutable internals: a hand-written wrapper attempt on the real shape fails, as in the real Native run", () => {
    const h = connect();
    let threw = false;
    try {
      "use strict";
      Object.defineProperty(h.page.ti, "invoke", { value: () => null });
    } catch {
      threw = true;
    }
    eq(threw, true);
  });

  return Promise.resolve({
    mode: "panel-driver-self-test",
    total: results.length,
    passed: results.filter((r) => r.ok).length,
    failed: results.filter((r) => !r.ok),
    scope: "JS-only vm mock of QA_MONITOR_JS source text; no sockets, no Cargo, no shell launch; not evidence about the real Tauri IPC",
  });
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url) && process.argv.includes("--self-test")) {
  selfTest().then((r) => {
    process.stdout.write(`${JSON.stringify(r)}\n`);
    process.exit(r.failed.length === 0 ? 0 : 1);
  });
}
