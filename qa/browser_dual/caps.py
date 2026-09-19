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
