// U1 — CONFIRMATION of a ruling already issued, not an open question.
//
// R14 settled §6.2 from Part 5's real-machine run: obscura's stdout banner is printed BEFORE the
// socket is bound, and with `--port 0` both the banner and `/json/version` report
// `ws://127.0.0.1:0` verbatim. Spec §11's U1 row already carries that conclusion, and 备选① (read
// the banner) no longer exists. This probe re-measures the same thing on this machine so the
// conclusion has a second observation behind it, and its verdict is allowed to say only what an
// OWNED, LISTENING socket proves — never what a banner says. A confidently wrong port-strategy
// label is more expensive than a missing one (判据 §17).
import { launchObscura, jsonVersion, listenerPids, freePort, sleep, emit } from "./t0-lib.mjs";

const out = { u: "U1", question: "obscura --port 0: is there ever an endpoint we can own?" };

// --- (a) `--port 0` -----------------------------------------------------------------------------
// `launchObscura` already refuses to hand back a wsUrl unless /json/version answered 200 on the
// port it passed, so with `--port 0` there is nothing to poll and `a.wsUrl` is null by
// construction. The banner is recorded as evidence, and separately checked against reality.
const a = await launchObscura({ port: 0, waitMs: 8000 });
await sleep(1500);
out.port_zero = {
  argv: a.argv, exit: a.exit, wsUrl: a.wsUrl, banner: a.banner,
  stdout: a.stdout.slice(0, 2000), stderr: a.stderr.slice(0, 2000),
};
// Does the port the banner names actually listen, and is it ours? R14 says the banner says 0.
const announced = a.banner?.port ?? null;
out.port_zero.announcedPort = announced;
out.port_zero.bannerNamesAUsablePort = Number.isInteger(announced) && announced > 0;
out.port_zero.jsonVersion = out.port_zero.bannerNamesAUsablePort ? await jsonVersion(announced) : null;
out.port_zero.listenerPids = out.port_zero.bannerNamesAUsablePort ? listenerPids(announced) : [];
out.port_zero.ownedByLaunchedPid = out.port_zero.listenerPids.includes(a.pid);
// The single predicate the verdict is allowed to branch on.
out.port_zero.usableEndpoint = Boolean(
  out.port_zero.bannerNamesAUsablePort
  && out.port_zero.jsonVersion?.status === 200
  && out.port_zero.ownedByLaunchedPid,
);
a.kill();
await sleep(300);

// --- (b) the strategy §6.2 actually kept: Aleph picks the port, then verifies ownership ---------
const fixed = await freePort();
const b = await launchObscura({ port: fixed, waitMs: 15000 });
out.fixed_port = {
  port: fixed, pid: b.pid, wsUrl: b.wsUrl, exit: b.exit,
  banner: b.banner,
  stdout: b.stdout.slice(0, 800),
  jsonVersion: b.jsonVersion ?? (await jsonVersion(fixed)),
  listenerPids: b.listenerPids,
  ownedByLaunchedPid: b.ownedByLaunchedPid,
};
out.fixed_port.usableEndpoint = Boolean(b.wsUrl && b.ownedByLaunchedPid);
// Whether the banner agrees with reality on THIS run, recorded because it is the fact R14 rests on.
out.fixed_port.bannerAgreesWithTheRequestedPort = b.banner?.port === fixed;
b.kill();

out.verdict = out.port_zero.usableEndpoint
  ? "UNEXPECTED — `--port 0` produced a listening socket on port "
    + out.port_zero.announcedPort + " owned by the launched pid and answering /json/version 200. "
    + "This CONTRADICTS R14; do not act on it, re-run and report to the controller before any "
    + "task changes its port strategy."
  : out.fixed_port.usableEndpoint
    ? "confirms R14: `--port 0` yields no ownable endpoint (banner said "
      + JSON.stringify(out.port_zero.banner?.url ?? null)
      + ", which names port " + out.port_zero.announcedPort + " — "
      + (out.port_zero.bannerNamesAUsablePort ? "a port that does not answer" : "not a port at all")
      + "), while an Aleph-picked port (" + fixed + ") answers /json/version "
      + (out.fixed_port.jsonVersion?.status ?? out.fixed_port.jsonVersion?.error)
      + " and its listener pid IS the launched pid. The banner is evidence, never an endpoint "
      + "(it also " + (out.fixed_port.bannerAgreesWithTheRequestedPort ? "did" : "did NOT")
      + " name the requested port on the fixed-port run) ⇒ §6.2 keeps only «Aleph allocates the "
      + "port + verifies ownership»."
    : "U1 UNMEASURED: neither `--port 0` nor an Aleph-picked port (" + fixed
      + ") produced an ownable endpoint — obscura did not start here (exit "
      + JSON.stringify(out.fixed_port.exit) + "). This says nothing about the port strategy; fix "
      + "the launch first.";
emit(out);
