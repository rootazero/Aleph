#!/usr/bin/env python3
"""Probe the REAL obscura for each row of the capability table and diff against
`browser_session{action:"capabilities"}`.

A name list only covers the world as it was on the day it was written (判据 §5).
This is that list's expiry check, and it is deliberately built out of EFFECTS:
`Input.insertText` on obscura answers `{}` whether or not anything was typed, so
a probe that only checked for a protocol error would report as supported a verb
that had done nothing. Every row below fires the verb at the page and reads the
state it should have changed.

**It probes the obscura ALEPH LAUNCHED**, reached through the engine sidecar,
and not one this script starts. That is not convenience: `file_upload` is
`unsupported` *because Aleph never passes `--allow-file-access`*
(`DOM.setFileInputFiles is disabled. Restart with obscura serve
--allow-file-access to enable local file uploads.`), so the capability is a
property of the argv as much as of the binary. A probe against its own launch
would be measuring a different configuration and calling the table wrong.
"""
import argparse
import asyncio
import base64
import glob
import json
import os
import re
import subprocess
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "browser_managed"))
from qa_rpc import Ledger, Rpc, ran, ws_connect  # noqa: E402
import websockets  # noqa: E402

MEASURED_ON = "v0.2.2"


class Raw:
    """A raw CDP connection, so the probes reach verbs Aleph's tools do not."""

    def __init__(self, ws):
        self.ws, self._id = ws, 0

    async def call(self, method, params=None, session=None, timeout=90):
        self._id += 1
        msg = {"id": self._id, "method": method, "params": params or {}}
        if session:
            msg["sessionId"] = session
        await self.ws.send(json.dumps(msg))
        while True:
            m = json.loads(await asyncio.wait_for(self.ws.recv(), timeout=timeout))
            if m.get("id") == self._id:
                return m


def find_node(node, tag, ident):
    if node.get("nodeName", "").lower() == tag:
        attrs = node.get("attributes", [])
        if dict(zip(attrs[::2], attrs[1::2])).get("id") == ident:
            return node["backendNodeId"]
    for child in node.get("children", []):
        got = find_node(child, tag, ident)
        if got is not None:
            return got
    return None


async def probe_obscura(ws_url, page_url, led):
    """What the binary DOES, one row per capability, by effect."""
    async with websockets.connect(ws_url, max_size=None, ping_interval=None) as ws:
        cdp = Raw(ws)
        made = await cdp.call("Target.createTarget", {"url": "about:blank"})
        tid = made["result"]["targetId"]
        att = await cdp.call("Target.attachToTarget", {"targetId": tid, "flatten": True})
        s = att["result"]["sessionId"]
        await cdp.call("Page.enable", {}, s)
        await cdp.call("Runtime.enable", {}, s)
        nav = await cdp.call("Page.navigate", {"url": page_url}, s)
        led.check("the probe's own page loaded", "error" not in nav, json.dumps(nav)[:300])
        await asyncio.sleep(2.5)

        async def ev(expr):
            r = await cdp.call("Runtime.evaluate", {"expression": expr, "returnByValue": True}, s)
            return r.get("result", {}).get("result", {}).get("value")

        led.check("…and it is the page this probe serves", await ev("document.title") == "caps",
                  str(await ev("document.title")))

        found = {}

        # js_dialogs. Probed by dispatch rather than by effect on purpose: an
        # engine with no dialogs never opens one, so there is no state to read.
        # The refusal IS the observation here, and the next claim checks that
        # the engine does not merely refuse the handler but never blocks either.
        r = await cdp.call("Page.handleJavaScriptDialog", {"accept": True}, s)
        found["js_dialogs"] = "unsupported" if "error" in r else "supported"

        r = await cdp.call("Input.dispatchDragEvent",
                           {"type": "dragOver", "x": 10, "y": 10, "data": {"items": []}}, s)
        found["drag"] = "supported" if "error" not in r and await ev("window.__dragged") else "unsupported"

        # `touch`, `screencast`, `network_interception` and `multi_connection`
        # are NOT probed here, and their absence is the point: R39 cut them from
        # the capability table because no `browser_*` verb dispatches them. A QA
        # stage that kept probing them would be measuring something the table no
        # longer claims — and the diff below would fail on the prober's surplus
        # rather than on a wrong claim.
        doc = await cdp.call("DOM.getDocument", {"depth": -1}, s)
        root = doc["result"]["root"]
        file_id = find_node(root, "input", "file")
        txt_id = find_node(root, "input", "txt")
        led.check("the probe found both inputs it fires at",
                  file_id is not None and txt_id is not None, f"file={file_id} txt={txt_id}")

        tmp = os.path.join(os.path.dirname(os.path.abspath(__file__)), ".caps-upload.txt")
        with open(tmp, "w") as fh:
            fh.write("caps")
        try:
            await cdp.call("DOM.setFileInputFiles", {"backendNodeId": file_id, "files": [tmp]}, s)
            found["file_upload"] = "supported" if await ev(
                "document.getElementById('file').files.length === 1") else "unsupported"
        finally:
            os.remove(tmp)

        r = await cdp.call("Page.printToPDF", {}, s)
        data = r.get("result", {}).get("data", "")
        found["pdf"] = "supported" if data and base64.b64decode(data)[:4] == b"%PDF" else "unsupported"

        await cdp.call("DOM.focus", {"backendNodeId": txt_id}, s)
        await cdp.call("Input.insertText", {"text": "caps-probe"}, s)
        found["insert_text"] = "supported" if await ev(
            "document.getElementById('txt').value === 'caps-probe'") else "unsupported"

        # The carried finding, measured here because this is the connection that
        # can measure it: obscura emits no dialog event AND does not block its
        # renderer on `alert()`. A Chromium blocks until the dialog is handled.
        await cdp.call("Runtime.evaluate",
                       {"expression": "setTimeout(function(){alert('caps')},50)",
                        "returnByValue": True}, s)
        await asyncio.sleep(1.0)
        answered = await ev("1+1")
        led.check("obscura does not block its renderer on alert() — so there is "
                  "nothing for a dialog verb to answer", answered == 2, str(answered))

        # effect_probe. The table row was fail-closed on "no obscura binary on
        # the implementing machine" (capability.rs, T3); this probe is the
        # measurement that row asked for. THREE premises, each asserted on its
        # own, because the production probe (`cdp_backend::actions::
        # effect_probe_install_js` + `conclude_effect_probe`) rests on all
        # three:
        #   1. a window global set by one `Runtime.evaluate` is visible to the
        #      next one — the probe's hit flag lives on `window` (obscura mints
        #      a FRESH JS wrapper per `DOM.resolveNode`, so node expandos do
        #      not persist; `window` must);
        #   2. a listener ON THE TARGET NODE observes a synthetic
        #      `Input.dispatchMouseEvent`. Node-level, not window-level, on
        #      purpose: that is where the production probe installs, and the
        #      first draft of this probe (window-level capture) measured a
        #      DIFFERENT fact — obscura v0.2.2 delivers synthetic mouse events
        #      to the target node's listeners but does NOT propagate them to
        #      window-level capture listeners (measured 2026-09-26, x86_64-
        #      linux build). Reading the effect is the whole point either way:
        #      `Input.insertText` answers `{}` whether or not anything was
        #      typed, so a protocol-error check alone would report a verb that
        #      did nothing as "supported".
        #   3. is below, after the dispatch — it needs the setup above first.
        await ev("window.__x = 1")
        persist = await ev("window.__x") == 1
        led.log(f"  effect_probe premise 1 (window global persists across Runtime.evaluate): {persist}")
        await ev("window.__capHit = false; "
                 "document.getElementById('txt').addEventListener("
                 "'click', function(){ window.__capHit = true; }, true)")
        rect = await ev("(function(){ var r = document.getElementById('txt').getBoundingClientRect(); "
                        "return {x: r.x + r.width / 2, y: r.y + r.height / 2}; })()")
        delivered = False
        if rect:
            for mtype, cc in (("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 1)):
                r = await cdp.call("Input.dispatchMouseEvent",
                                   {"type": mtype, "x": rect["x"], "y": rect["y"],
                                    "button": "left", "clickCount": cc}, s)
                if "error" in r:
                    led.log(f"  Input.dispatchMouseEvent {mtype} errored: {json.dumps(r)[:200]}")
            await asyncio.sleep(0.5)
            delivered = bool(await ev("window.__capHit"))
        led.log(f"  effect_probe premise 2 (node-level listener observes synthetic "
                f"Input.dispatchMouseEvent): {delivered} (rect={rect})")
        # premise 3 — the navigation race the first two premises cannot see:
        # the probe's read-back runs AFTER a dispatch that may itself
        # navigate, and the hit flag lives on `window`, which a navigation
        # destroys. A SOUND engine answers that read-back with the flag (the
        # evaluate won the race into the old context) or with an error (the
        # context is gone — `ProbeReadback::Unknown`, the action stands);
        # chromium does the former kinds. obscura instead answers a CLEAN
        # null evaluated on the NEW document, which the probe cannot tell
        # from "never landed" — measured 2026-09-26 (v0.2.2, x86_64-linux):
        # a click on a real link navigated (the default action fires) and the
        # read-back came back null, i.e. arming the probe on this engine
        # turns every navigating click into a false `EffectNotDelivered`.
        # `qa/browser_dual`'s `click` stage is the end-to-end witness.
        await ev("var a = document.createElement('a'); a.id = 'navlink';"
                 "a.href = 'index.html'; a.textContent = 'nav';"
                 "a.style.display = 'block'; a.style.width = '100px'; a.style.height = '30px';"
                 "document.body.appendChild(a);"
                 "window.__navHit = null;"
                 "a.addEventListener('click', function(){ window.__navHit = 'click'; }, true)")
        navrect = await ev("(function(){ var r = document.getElementById('navlink').getBoundingClientRect(); "
                           "return {x: r.x + r.width / 2, y: r.y + r.height / 2}; })()")
        nav_sound = False
        if navrect:
            for mtype, cc in (("mouseMoved", 0), ("mousePressed", 1), ("mouseReleased", 1)):
                await cdp.call("Input.dispatchMouseEvent",
                               {"type": mtype, "x": navrect["x"], "y": navrect["y"],
                                "button": "left", "clickCount": cc}, s)
            await asyncio.sleep(1.5)
            r = await cdp.call("Runtime.evaluate",
                               {"expression": "window.__navHit", "returnByValue": True}, s)
            landed = await cdp.call("Runtime.evaluate",
                                    {"expression": "location.pathname", "returnByValue": True}, s)
            landed_path = landed.get("result", {}).get("result", {}).get("value")
            navhit = r.get("result", {}).get("result", {}).get("value") if "error" not in r else "<error>"
            # Fixture integrity, asserted: if the click did NOT navigate, the
            # premise measured nothing and "unsound" would be the wrong
            # conclusion (判据 §2 — a red for the wrong reason is the shape
            # this stage exists to refuse).
            led.check("premise 3's integrity: the link click actually navigated",
                      bool(landed_path) and "index" in str(landed_path), str(landed_path))
            led.log(f"  effect_probe premise 3 (read-back after a self-navigating click): "
                    f"navHit={navhit!r} — "
                    + ("carried the flag or refused to answer (sound)"
                       if ("error" in r or navhit == "click")
                       else "a CLEAN NULL on the new document — indistinguishable from "
                            "'never landed' (unsound: a landed click reads as NoHit)"))
            nav_sound = "error" in r or navhit == "click"
        found["effect_probe"] = (
            "supported" if persist and delivered and nav_sound else "unsupported")

        return found


def obscura_version(binary):
    try:
        out = subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return None
    m = re.search(r"(\d+\.\d+\.\d+)", out.stdout + out.stderr)
    return f"v{m.group(1)}" if m else None


async def main():
    p = argparse.ArgumentParser()
    p.add_argument("ws")
    p.add_argument("--page-url", required=True)
    p.add_argument("--aleph-home", required=True)
    p.add_argument("--obscura-binary", required=True)
    a = p.parse_args()
    led = Ledger()

    async with ws_connect(a.ws) as gw:
        rpc = Rpc(gw)
        await rpc.connect("qa-browser-dual-caps")
        ok, body = await rpc.invoke("browser_open", {"profile": "default", "url": a.page_url})
        led.check("browser_open on the default profile succeeds", ran(ok, body), json.dumps(body)[:300])
        ok, body = await rpc.invoke("browser_session", {"action": "capabilities"})
        led.check("browser_session{capabilities} answers", ran(ok, body), json.dumps(body)[:300])
        table = (body or {}).get("capabilities") or {}
        led.check("it carries both engines", set(table) >= {"obscura", "chromium"}, str(list(table)))

        # --- the tool face of an unsupported verb ---------------------------
        # 判据 §11, on the default engine: a verb the table says this engine
        # cannot do must REFUSE, not report success having done nothing. The
        # gate is `cdp_backend::require`, one layer below the tool, and this is
        # the only place it is exercised against a real engine.
        ok, body = await rpc.invoke("browser_dialog", {"profile": "default", "action": "accept"})
        blob = json.dumps(body)
        led.check("browser_dialog on obscura REFUSES rather than reporting success",
                  not ran(ok, body), blob[:400])
        led.check("…and the refusal names the engine that cannot do it",
                  "obscura" in blob and "handle_dialog" in blob, blob[:400])
        led.check("…and names the engine that can, and the verb that moves there",
                  "chromium" in blob and "switch_engine" in blob, blob[:400])

        http = None
        for path in glob.glob(os.path.join(a.aleph_home, "data", "browser", "*", "*.json")):
            try:
                with open(path) as fh:
                    rec = json.load(fh)
            except (OSError, ValueError):
                continue
            if rec.get("engine") == "obscura":
                http = rec.get("http_url")
        led.check("the obscura endpoint is known from its sidecar", bool(http), str(http))
        if not http:
            return led.verdict()
        port = int(http.rsplit(":", 1)[1])

        measured = await probe_obscura(f"ws://127.0.0.1:{port}/devtools/browser", a.page_url, led)

        # --- ref_precheck (round-1 B4), probed at the GATEWAY layer ---------
        # The check lives in `browser_tools::mod::precheck_ref`, one layer ABOVE
        # the engine: a ref minted before a navigation must be refused before
        # any side effect. It is probed here rather than in `probe_obscura`
        # because the raw CDP connection has no refs at all — minting is a tool
        # behaviour. Runs AFTER probe_obscura on purpose: it navigates the
        # default profile's only tab away from the caps page, and the raw
        # probes above still needed that page where it was.
        ok, body = await rpc.invoke("browser_snapshot", {"profile": "default", "max_chars": 4000})
        blob = json.dumps(body)
        m = re.search(r"\[ref=(e\d+)\]", blob)
        led.check("the default profile's snapshot minted a ref to play staleness against",
                  ran(ok, body) and bool(m), blob[:200])
        if m:
            stale_ref = m.group(1)
            away = a.page_url.replace("caps.html", "index.html")
            ok, body = await rpc.invoke(
                "browser_navigate",
                {"profile": "default", "action": {"goto": {"url": away}}})
            led.check("the tab navigated away from the page the ref was minted on",
                      ran(ok, body), json.dumps(body)[:200])
            ok, body = await rpc.invoke(
                "browser_click", {"profile": "default", "ref_id": stale_ref})
            message = (body or {}).get("message") or ""
            refused = not ran(ok, body)
            led.check("clicking a pre-navigation ref is REFUSED before any side effect",
                      refused, message[:300])
            # The refusal's own words, not any refusal: `BrowserError::StaleRef`
            # with `StaleReason::Navigated` renders "ref eN is stale — the page
            # navigated. …" (src/browser/error.rs).
            led.check("…and the refusal says the ref is stale because the page navigated",
                      refused and "stale" in message and "the page navigated" in message,
                      message[:300])
            trailer = next(
                (ln for ln in message.splitlines() if ln.startswith("recovery: ")), "")
            category = ""
            if trailer:
                try:
                    category = json.loads(trailer[len("recovery: "):]).get("category", "")
                except ValueError:
                    category = "<unparseable>"
            led.check("…and carries the recovery trailer naming the stale_ref category",
                      category == "stale_ref", trailer[:200])
            measured["ref_precheck"] = (
                "supported" if refused and category == "stale_ref" else "unsupported")
        else:
            measured["ref_precheck"] = "unsupported"

    claimed = table.get("obscura", {})
    for name, got in sorted(measured.items()):
        led.check(f"obscura.{name}: the table says what the binary does",
                  claimed.get(name) == got, f"table={claimed.get(name)} measured={got}")
    # Both directions. A row the prober does not cover is a claim nobody
    # checked; a probe with no row is the fixture measuring something the table
    # stopped claiming, which is the direction that matters after R39.
    led.check("every claimed row was probed",
              set(claimed) - {"measured_on"} == set(measured),
              f"claimed={sorted(set(claimed) - {'measured_on'})} probed={sorted(measured)}")
    # The row's own date, against the binary this run actually launched. A table
    # dated to one build and a binary of another is not a disagreement about a
    # capability — it is a table describing something else.
    build = obscura_version(a.obscura_binary)
    led.check("the obscura row says what it was measured on",
              claimed.get("measured_on") == MEASURED_ON, str(claimed.get("measured_on")))
    led.check("…and that is the build this run probed",
              build == claimed.get("measured_on"),
              f"binary={build} table={claimed.get('measured_on')}")
    # Chromium's row is UNRUN here, and says so rather than passing: probing it
    # needs a second launch, and an unrun claim reported as a pass is the
    # failure mode qa/README.md records for spend_budget on Windows.
    Ledger.log("  [UNRUN] chromium.* — not probed by this stage; the chromium row is "
               "asserted in src/browser/engine/capability.rs and exercised by "
               "qa/browser_managed under ALEPH_QA_DRIVER=cdp")
    return led.verdict()


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
