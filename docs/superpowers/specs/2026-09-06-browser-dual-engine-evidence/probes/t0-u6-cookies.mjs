// U6: which Network.setCookies fields survive a round-trip on each engine?
// usage: node t0-u6-cookies.mjs <chrome|obscura>
import { launchEngine, newPage, tryCall, parentUrl, serveStatic, PROBE_DIR, PARENT_PORT, emit } from "./t0-lib.mjs";

const engine = process.argv[2] ?? "obscura";
const server = await serveStatic(PROBE_DIR, PARENT_PORT);
const e = await launchEngine(engine);
const { c, sessionId } = await newPage(e.wsUrl, parentUrl());

const URL_ = parentUrl();
const EXPIRES = Math.floor(Date.now() / 1000) + 3600;
const SENT = [
  { name: "t0_plain",      value: "v1", url: URL_ },
  { name: "t0_lax",        value: "v2", url: URL_, sameSite: "Lax" },
  { name: "t0_strict",     value: "v3", url: URL_, sameSite: "Strict" },
  { name: "t0_expires",    value: "v4", url: URL_, expires: EXPIRES },
  { name: "t0_httponly",   value: "v5", url: URL_, httpOnly: true },
  { name: "t0_path",       value: "v6", url: URL_, path: "/sub" },
  { name: "t0_domainpath", value: "v7", domain: "127.0.0.1", path: "/" },
  // deliberately self-contradictory on an http origin: SameSite=None requires Secure.
  { name: "t0_none_insecure", value: "v8", url: URL_, sameSite: "None" },
];
const out = { u: "U6", engine, sent: SENT, expiresSent: EXPIRES };
out.setCookies = await tryCall(c, "Network.setCookies", { cookies: SENT }, sessionId, 30000);
const all = await tryCall(c, "Network.getAllCookies", {}, sessionId, 30000);
out.getAllOk = all.ok;
if (!all.ok) out.getAllError = all.error.message;
const got = all.ok ? (all.result.cookies ?? []) : [];
out.received = got.filter((k) => k.name.startsWith("t0_"));
const byName = Object.fromEntries(out.received.map((k) => [k.name, k]));
out.perCookie = SENT.map((s) => {
  const g = byName[s.name];
  if (!g) return { name: s.name, present: false };
  return {
    name: s.name, present: true, value: g.value, domain: g.domain, path: g.path,
    expires: g.expires, httpOnly: g.httpOnly, secure: g.secure, sameSite: g.sameSite,
    sameSiteKept: s.sameSite === undefined ? null : g.sameSite === s.sameSite,
    expiresKept: s.expires === undefined ? null : Math.abs((g.expires ?? -1) - s.expires) <= 1,
    keys: Object.keys(g),
  };
});
const kept = out.perCookie.filter((p) => p.present).length;
const ss = out.perCookie.filter((p) => p.sameSiteKept === true).map((p) => p.name);
const ssLost = out.perCookie.filter((p) => p.sameSiteKept === false).map((p) => p.name);
const expOk = out.perCookie.find((p) => p.name === "t0_expires")?.expiresKept;
out.verdict = !out.setCookies.ok
  ? engine + ": `Network.setCookies` refused the batch — " + out.setCookies.error.message
    + " ⇒ migration must set cookies one at a time and report per-cookie failures."
  : engine + ": " + kept + "/" + SENT.length + " cookies came back; sameSite kept for ["
    + ss.join(",") + "]" + (ssLost.length ? ", LOST for [" + ssLost.join(",") + "]" : "")
    + "; `expires` " + (expOk === true ? "round-trips" : expOk === false ? "does NOT round-trip" : "unmeasured")
    + "; fields present on a returned cookie: "
    + JSON.stringify(out.perCookie.find((p) => p.present)?.keys ?? []) + ".";
emit(out);
c.close(); e.kill(); server.close();
