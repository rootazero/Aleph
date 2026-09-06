// shared CDP helper (throwaway)
export const sleep = (ms) => new Promise((r) => setTimeout(r, ms));
export const now = () => performance.now();
export const errStr = (e) => String(e?.message ?? e);
export class Cdp {
  constructor(url, name = "c") { this.url = url; this.name = name; this.id = 0; this.pending = new Map(); this.listeners = []; }
  async connect() {
    this.ws = new WebSocket(this.url);
    await new Promise((res, rej) => { this.ws.addEventListener("open", res); this.ws.addEventListener("error", () => rej(new Error(`${this.name}: ws error`))); });
    this.ws.addEventListener("message", (ev) => {
      const m = JSON.parse(ev.data);
      if (m.id && this.pending.has(m.id)) { const p = this.pending.get(m.id); this.pending.delete(m.id); m.error ? p.reject(new Error(`${p.method}: ${m.error.message}`)) : p.resolve(m.result); }
      else if (m.method) { for (const l of this.listeners) l(m); }
    });
  }
  call(method, params = {}, sessionId, timeoutMs = 60000) {
    return new Promise((resolve, reject) => {
      const id = ++this.id;
      const t = setTimeout(() => { if (this.pending.has(id)) { this.pending.delete(id); reject(new Error(`${method}: timeout ${timeoutMs}ms`)); } }, timeoutMs);
      this.pending.set(id, { method, resolve: (v) => { clearTimeout(t); resolve(v); }, reject: (e) => { clearTimeout(t); reject(e); } });
      const msg = { id, method, params }; if (sessionId) msg.sessionId = sessionId;
      this.ws.send(JSON.stringify(msg));
    });
  }
  on(fn) { this.listeners.push(fn); return () => { this.listeners = this.listeners.filter((l) => l !== fn); }; }
  waitEvent(method, sessionId, timeoutMs = 45000) {
    return new Promise((resolve, reject) => {
      const t = setTimeout(() => { off(); reject(new Error(`timeout waiting ${method}`)); }, timeoutMs);
      const off = this.on((m) => { if (m.method === method && (!sessionId || m.sessionId === sessionId)) { clearTimeout(t); off(); resolve(m.params); } });
    });
  }
  close() { try { this.ws.close(); } catch {} }
}
export const q = (a) => { const s = [...a].sort((x, y) => x - y); return { min: s[0], med: s[Math.floor(s.length / 2)], max: s[s.length - 1] }; };
