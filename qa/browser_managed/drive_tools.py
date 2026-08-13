#!/usr/bin/env python3
"""Per-verb real-machine QA for the managed browser stack.

Why a second driver
-------------------
`drive_browser.py` answers "does a browser come up, with the config Aleph
generated". That took four rounds to get right, but it only ever exercised four
of the twenty-six browser tools. The other twenty-two had unit tests whose green
is a property of the machine — they assert the *degraded* branch, the one taken
when no browser is reachable. Sealing those (round 4) stopped them from silently
becoming machine-dependent; it did not make them cover anything.

So every claim here is an **effect** claim, read back out of the live page:
`browser_click` is not "the tool returned success", it is "`#clicked` now says
`yes` when JavaScript is asked". A tool that returns success and does nothing
fails here, which is the entire point.

Scenarios
---------
* ``tools``  — drive every remaining browser verb against one interactive page.
* ``frames`` — a genuinely cross-origin iframe (second http server, second
  port). Answers empirically whether the accessibility snapshot and the click
  path reach into an out-of-process frame, instead of inferring it from docs.
  The cross-origin-ness is proven first; a same-origin iframe would make every
  later claim vacuous.
* ``reap``   — the idle reaper actually closes a managed session. `idle_timeout_secs`
  is per-profile config, so this costs one sweep (~60 s), not the 30-minute
  default. A second profile with a far-future timeout is the control: without
  it, "the idle one was closed" and "everything was closed" look identical.
* ``pdf``    — `pdf_generate`'s browser engine, which drives its own
  `playwright-cli` session rather than the profile-configured one.
"""
import argparse
import asyncio
import base64
import json
import os
import re
import sys
import time

import websockets

from qa_rpc import Ledger, Rpc, cli_sessions, open_session_count, session_status

# The two drivers print element handles differently, and the handle is what the
# model passes back as `ref_id`:
#   managed          `- textbox "Full name" [ref=e3]`
#   existing-session `uid=1_2 textbox "Full name"`
# A fixture that knew only the first found nothing on the second and reported
# "no control is addressable" for a driver whose refs were right there.
REF_RE = re.compile(r"\[ref=([A-Za-z0-9_.:-]+)\]|\buid=([A-Za-z0-9_.:-]+)")


def ref_for(snapshot, needle):
    """The `[ref=eNN]` on the first snapshot line mentioning `needle`.

    The fixture page gives every interactive element an `aria-label`, because
    that is what turns into the accessible name the snapshot prints — and the
    accessible name is the only handle the model has. An unnamed element is
    unaddressable, which is a fixture bug rather than a product one, so a
    missing ref is reported as such by the caller.
    """
    for line in snapshot.splitlines():
        if needle in line:
            m = REF_RE.search(line)
            if m:
                return m.group(1) or m.group(2)
    return None


FENCE_LINE = ("<<<EXTERNAL_UNTRUSTED_CONTENT", "<<<END_EXTERNAL_UNTRUSTED_CONTENT")


def unfence(payload):
    """Strip the untrusted-content fence and JSON-decode what is inside.

    Everything a browser read hands back is wrapped by `redact_wrap`, which is
    correct for the model and wrong for an oracle: asserting against the fenced
    blob compares page data with Aleph's framing. Returns the decoded value when
    the payload is JSON, otherwise the unfenced text.
    """
    if not isinstance(payload, str):
        return payload
    inner = "\n".join(
        ln for ln in payload.splitlines() if not ln.lstrip().startswith(FENCE_LINE)
    ).strip()
    try:
        return json.loads(inner)
    except (ValueError, TypeError):
        return inner


class Page:
    """Read/act helpers bound to one profile."""

    def __init__(self, rpc, led, profile="default"):
        self.rpc = rpc
        self.led = led
        self.profile = profile

    async def call(self, tool, **kw):
        kw.setdefault("profile", self.profile)
        return await self.rpc.invoke(tool, kw)

    async def js(self, expr):
        """Evaluate `expr` in the page and return `(ok, value)`.

        Every effect assertion in this file goes through here: reading the DOM
        back is the only way to tell a verb that worked from a verb that merely
        returned success.

        The value arrives wrapped in the untrusted-content fence every browser
        read carries, so the fence is stripped and the payload JSON-decoded —
        an assertion made against the fenced blob would compare page data with
        Aleph's own framing, and `scrollY > 0` would be a string comparison.
        Non-JSON payloads (`undefined`, an `### Error` transcript) come back as
        raw text, which is what the caller should see.
        """
        ok, res = await self.call("browser_evaluate", script=f"() => ({expr})")
        if not ok or not isinstance(res, dict) or not res.get("success"):
            return False, json.dumps(res)[:300]
        return True, unfence(res.get("result"))

    async def dom(self, selector, prop="textContent"):
        return await self.js(f"document.querySelector({selector!r}).{prop}")

    async def snapshot(self, max_chars=None):
        kw = {} if max_chars is None else {"max_chars": max_chars}
        ok, res = await self.call("browser_snapshot", **kw)
        text = (res or {}).get("snapshot") or "" if isinstance(res, dict) else ""
        return ok and isinstance(res, dict) and res.get("success"), text, res

    async def effect(self, claim, selector, expected, prop="textContent"):
        """Assert a DOM read-back. The workhorse of this file."""
        ok, val = await self.dom(selector, prop)
        return self.led.check(
            claim, ok and expected in str(val), f"{selector}.{prop} = {str(val)[:140]}"
        )


# --------------------------------------------------------------------------
# scenario: tools
# --------------------------------------------------------------------------
async def scenario_tools(rpc, led, args):
    p = Page(rpc, led)

    led.log("\n--- open the interactive fixture ---")
    ok, res = await p.call("browser_open", url=args.page_url)
    led.check("browser_open succeeds", ok and res.get("success"), json.dumps(res)[:200])
    first_tab = (res or {}).get("tab_id")

    # ---- wait_for: a genuine wait, then a genuine absence -----------------
    led.log("\n--- browser_wait_for ---")
    # The fixture writes LATE_MARKER 1.5 s after load, so this is a real wait
    # rather than a lookup of something already on the page.
    t0 = time.monotonic()
    ok, res = await p.call("browser_wait_for", text=args.late_marker, timeout_ms=8000)
    led.check(
        "wait_for finds text that appears only after a delay",
        ok and res.get("success") and res.get("found"),
        f"elapsed={time.monotonic() - t0:.2f}s result={json.dumps(res)[:180]}",
    )
    ok, res = await p.call("browser_wait_for", text="NEVER_APPEARS_" + args.marker, timeout_ms=1500)
    # Absence is an answer, not a failure: the tool must succeed and report
    # `found: false`. A tool that errors here would push the model into
    # error-recovery over a perfectly ordinary negative result.
    led.check(
        "wait_for reports absence as success+found:false, not as an error",
        ok and res.get("success") and res.get("found") is False,
        json.dumps(res)[:180],
    )

    # ---- snapshot + refs ---------------------------------------------------
    led.log("\n--- browser_snapshot gives addressable refs ---")
    ok, snap, _ = await p.snapshot()
    led.check("browser_snapshot succeeds", ok, snap[:120])
    led.check("the snapshot carries the page marker", args.marker in snap, snap[:200])
    refs = {
        name: ref_for(snap, name)
        for name in ("Go button", "Full name", "Email address", "Pick one",
                     "Hover target", "Drag source", "Drop target", "Upload file",
                     "Alert button")
    }
    missing = [k for k, v in refs.items() if not v]
    led.check("every labelled control resolved to a ref", not missing, f"missing={missing}")

    # ---- click -------------------------------------------------------------
    led.log("\n--- browser_click / type / fill_form / select / hover / press_key ---")
    await p.effect("baseline: the click target has not fired yet", "#clicked", "clicked:no")
    ok, res = await p.call("browser_click", ref_id=refs["Go button"])
    led.check("browser_click returns success", ok and res.get("success"), json.dumps(res)[:180])
    await p.effect("…and the page's click handler actually ran", "#clicked", "clicked:yes")

    # ---- type --------------------------------------------------------------
    typed = "TYPED_" + args.marker
    ok, res = await p.call("browser_type", ref_id=refs["Full name"], text=typed)
    led.check("browser_type returns success", ok and res.get("success"), json.dumps(res)[:180])
    await p.effect("…and the text landed in that input", "#name", typed, prop="value")

    # ---- fill_form ---------------------------------------------------------
    ok, res = await p.call(
        "browser_fill_form",
        fields=[
            {"ref_id": refs["Full name"], "value": "FILLED_NAME"},
            {"ref_id": refs["Email address"], "value": "QA"},
        ],
    )
    led.check("browser_fill_form returns success", ok and res.get("success"), json.dumps(res)[:180])
    await p.effect("…field 1 was filled", "#name", "FILLED_NAME", prop="value")
    await p.effect("…field 2 was filled (both, not just the first)", "#email", "QA", prop="value")

    # ---- select ------------------------------------------------------------
    ok, res = await p.call("browser_select", ref_id=refs["Pick one"], value="b")
    led.check("browser_select returns success", ok and res.get("success"), json.dumps(res)[:180])
    await p.effect("…and the change event fired with the new value", "#picked", "picked:b")

    # ---- hover -------------------------------------------------------------
    ok, res = await p.call("browser_hover", ref_id=refs["Hover target"])
    led.check("browser_hover returns success", ok and res.get("success"), json.dumps(res)[:180])
    await p.effect("…and the page saw a real mouseover", "#hovered", "hovered:yes")

    # ---- press_key ---------------------------------------------------------
    # `End` then `Backspace` on a focused field: both keys are unambiguous, and
    # the resulting value ("Q") is one character the fixture never wrote itself.
    await p.call("browser_click", ref_id=refs["Email address"])
    await p.call("browser_press_key", key="End")
    ok, res = await p.call("browser_press_key", key="Backspace")
    led.check("browser_press_key returns success", ok and res.get("success"), json.dumps(res)[:180])
    ok, val = await p.dom("#email", "value")
    led.check("…and the keystroke edited the focused field", ok and val == "Q", f"value={val!r}")

    # ---- drag --------------------------------------------------------------
    led.log("\n--- browser_drag ---")
    ok, res = await p.call("browser_drag", from_ref=refs["Drag source"], to_ref=refs["Drop target"])
    led.check("browser_drag returns success", ok and res.get("success"), json.dumps(res)[:180])
    # The fixture records WHICH mechanism fired (html5 `drop` vs a bare
    # `mouseup`), so a partial result is legible instead of just "no".
    ok, val = await p.dom("#dropped")
    led.check("…and the drop target observed the gesture", ok and val != "dropped:no", f"state={val!r}")

    # ---- scroll ------------------------------------------------------------
    led.log("\n--- browser_scroll / resize / screenshot ---")
    ok, res = await p.call("browser_scroll", direction="down")
    led.check("browser_scroll returns success", ok and res.get("success"), json.dumps(res)[:180])
    ok, val = await p.js("window.scrollY")
    led.check("…and the viewport actually moved", ok and isinstance(val, (int, float)) and val > 0,
              f"scrollY={val!r}")

    # ---- resize ------------------------------------------------------------
    ok, res = await p.call("browser_resize", width=900, height=600)
    led.check("browser_resize returns success", ok and res.get("success"), json.dumps(res)[:180])
    ok, val = await p.js("[window.innerWidth, window.innerHeight].join('x')")
    led.check("…and the viewport is the size we asked for", ok and val == "900x600", f"inner={val!r}")

    # ---- screenshot --------------------------------------------------------
    ok, res = await p.call("browser_screenshot")
    b64 = (res or {}).get("image_base64") if isinstance(res, dict) else None
    led.check("browser_screenshot returns success", ok and res.get("success"), json.dumps(res)[:180])
    png = False
    if b64:
        try:
            png = base64.b64decode(b64)[:8] == b"\x89PNG\r\n\x1a\n"
        except Exception:  # noqa: BLE001 - a malformed payload is the failure
            png = False
    # A base64 field that is present but not a PNG is exactly the shape a
    # never-decoded screenshot has; the magic bytes are what separate the two.
    led.check("…and the payload decodes to a real PNG", png, f"len={len(b64 or '')}")

    # ---- console -----------------------------------------------------------
    led.log("\n--- browser_console / browser_network ---")
    ok, res = await p.call("browser_console")
    msgs = (res or {}).get("messages") or "" if isinstance(res, dict) else ""
    led.check(
        "browser_console carries the page's own console.log",
        ok and res.get("success") and args.console_marker in msgs,
        f"messages[:200]={msgs[:200]!r}",
    )

    # ---- network -----------------------------------------------------------
    ok, res = await p.call("browser_network")
    reqs = (res or {}).get("requests") or "" if isinstance(res, dict) else ""
    led.check(
        "browser_network carries the subresource the page fetched",
        ok and res.get("success") and "net-probe.json" in reqs,
        f"requests[:200]={reqs[:200]!r}",
    )

    # ---- emulate -----------------------------------------------------------
    led.log("\n--- browser_emulate ---")
    # The managed driver supports exactly one emulation axis. Both halves are
    # asserted: the supported one by effect, the unsupported one by the shape of
    # its refusal — a tool that quietly accepted an override it cannot apply
    # would be the worse failure, and only the negative case can catch that.
    ok, val = await p.js("navigator.onLine")
    led.check("baseline: the page believes it is online", ok and val is True, f"onLine={val!r}")
    ok, res = await p.call("browser_emulate", network_condition="offline")
    led.check("browser_emulate(network_condition) returns success",
              ok and res.get("success"), json.dumps(res)[:200])
    ok, val = await p.js("navigator.onLine")
    led.check("…and the page really went offline", ok and val is False, f"onLine={val!r}")
    await p.call("browser_emulate", network_condition="online")

    ok, res = await p.call("browser_emulate", color_scheme="dark")
    blob = json.dumps(res)
    led.check(
        "an emulation axis this driver cannot apply is refused, not silently dropped",
        not (ok and res.get("success")) and "network_condition" in blob,
        blob[:220],
    )

    # ---- cookies -----------------------------------------------------------
    led.log("\n--- browser_cookies ---")
    cname, cval = "qa_cookie", "qa_" + args.marker
    ok, res = await p.call("browser_cookies", action="set", name=cname, value=cval, path="/")
    led.check("browser_cookies set returns success", ok and res.get("success"), json.dumps(res)[:180])
    ok, val = await p.js("document.cookie")
    led.check("…and the browser really holds it", ok and cval in str(val), f"document.cookie={str(val)[:160]}")
    ok, res = await p.call("browser_cookies", action="list")
    listing = (res or {}).get("cookies") or "" if isinstance(res, dict) else ""
    led.check("browser_cookies list shows it", ok and cname in listing, listing[:200])
    ok, res = await p.call("browser_cookies", action="delete", name=cname)
    led.check("browser_cookies delete returns success", ok and res.get("success"), json.dumps(res)[:180])
    ok, val = await p.js("document.cookie")
    led.check("…and the browser no longer holds it", ok and cval not in str(val), f"document.cookie={str(val)[:160]}")

    # ---- tabs --------------------------------------------------------------
    led.log("\n--- browser_tabs ---")
    ok, res = await p.call("browser_tabs", action="list")
    tabs = (res or {}).get("tabs") or [] if isinstance(res, dict) else []
    led.check("browser_tabs list returns the open tabs", ok and len(tabs) >= 1, json.dumps(res)[:220])
    before = len(tabs)
    ok, res = await p.call("browser_open", url=args.page_url)
    second_tab = (res or {}).get("tab_id")
    ok, res = await p.call("browser_tabs", action="list")
    tabs2 = (res or {}).get("tabs") or [] if isinstance(res, dict) else []
    led.check("…and a newly opened tab shows up in the listing", len(tabs2) == before + 1,
              f"{before} -> {len(tabs2)}")
    # `TabAction` is externally tagged: `{"switch": {"tab_id": ...}}`, not a
    # sibling `tab_id` key. Getting this wrong yields a deserialization error,
    # which is a fixture bug wearing the costume of a product one.
    ok, res = await p.call("browser_tabs", action={"switch": {"tab_id": str(first_tab)}})
    led.check("browser_tabs switch returns success", ok and res.get("success"), json.dumps(res)[:180])
    ok, res = await p.call("browser_tabs", action={"close": {"tab_id": str(second_tab)}})
    led.check("browser_tabs close returns success", ok and res.get("success"), json.dumps(res)[:180])
    ok, res = await p.call("browser_tabs", action="list")
    tabs3 = (res or {}).get("tabs") or [] if isinstance(res, dict) else []
    led.check("…and the closed tab is gone from the listing", len(tabs3) == before, f"{len(tabs2)} -> {len(tabs3)}")

    # ---- navigate ----------------------------------------------------------
    led.log("\n--- browser_navigate goto / back / refresh ---")
    ok, res = await p.call("browser_navigate", action={"goto": {"url": args.second_url}})
    led.check("browser_navigate goto returns success", ok and res.get("success"), json.dumps(res)[:180])
    ok, val = await p.js("location.href")
    led.check("…and the tab is on the new URL", ok and "second" in str(val), f"href={val!r}")
    ok, res = await p.call("browser_navigate", action="back")
    led.check("browser_navigate back returns success", ok and res.get("success"), json.dumps(res)[:180])
    ok, val = await p.js("location.href")
    led.check("…and the tab went back", ok and "second" not in str(val), f"href={val!r}")

    # ---- browser_pdf -------------------------------------------------------
    led.log("\n--- browser_pdf ---")
    pdf_path = os.path.join(args.out_dir, "browser_pdf.pdf")
    ok, res = await p.call("browser_pdf", output_path=pdf_path)
    led.check("browser_pdf returns success", ok and res.get("success"), json.dumps(res)[:200])
    head = b""
    if os.path.exists(pdf_path):
        with open(pdf_path, "rb") as fh:
            head = fh.read(5)
    # "the tool said ok" and "a PDF exists" are different claims; only the
    # second one is about the browser having rendered anything.
    led.check("…and the file on disk is a PDF", head == b"%PDF-", f"{pdf_path} head={head!r}")

    # ---- session state -----------------------------------------------------
    led.log("\n--- browser_session save/load ---")
    ok, res = await p.call("browser_session", action="save", name="qastate")
    saved = (res or {}).get("path") if isinstance(res, dict) else None
    led.check("browser_session save returns success", ok and res.get("success"), json.dumps(res)[:200])
    led.check("…and it names a file that exists", bool(saved) and os.path.exists(saved), f"path={saved}")
    ok, res = await p.call("browser_session", action="load", name="qastate")
    led.check("browser_session load returns success", ok and res.get("success"), json.dumps(res)[:200])

    # ---- profile -----------------------------------------------------------
    led.log("\n--- browser_profile ---")
    ok, res = await rpc.invoke("browser_profile", {"action": "list"})
    profiles = (res or {}).get("profiles") or [] if isinstance(res, dict) else []
    names = [x.get("name") for x in profiles if isinstance(x, dict)]
    led.check("browser_profile list reports the configured profiles", ok and "default" in names,
              f"names={names}")

    # ---- exec --------------------------------------------------------------
    led.log("\n--- browser_exec (multi-step procedure) ---")
    ok, res = await p.call(
        "browser_exec",
        actions=[
            {"action": "navigate", "url": args.page_url},
            {"action": "wait", "text": args.late_marker, "timeout_ms": 8000},
            {"action": "click", "ref_id": "#go"},
            {"action": "snapshot"},
        ],
    )
    led.check(
        "browser_exec ran every step",
        ok and res.get("success") and res.get("completed") == res.get("total") and res.get("failed_at") is None,
        json.dumps({k: res.get(k) for k in ("success", "total", "completed", "failed_at", "message")})[:240],
    )
    await p.effect("…and the click step really clicked", "#clicked", "clicked:yes")

    # A snapshot step cut by its own budget: the note the model is handed must
    # not point at a lever that cannot recover the tail. `browser_exec` is a
    # PROCEDURE — by the time the model could take a standalone snapshot, later
    # steps have moved the page, so "take a standalone browser_snapshot" is
    # advice about a different page than the one that was cut.
    ok, res = await p.call(
        "browser_exec",
        actions=[{"action": "snapshot", "max_chars": 1000}],
    )
    step_text = ""
    if isinstance(res, dict):
        step_text = json.dumps(res.get("results") or [])
    tail = step_text[-420:]
    led.log(f"  (truncated-step tail: {tail})")
    # Two acceptable outcomes, and the fixture must not care which: with a
    # result store in scope the tail is offloaded and the model gets a persist
    # marker; over bare `tools.invoke` there is no call id, so the honest answer
    # is that the tail is gone. What is NOT acceptable is the third answer this
    # shipped with — pointing at a standalone `browser_snapshot`, which in a
    # procedure re-reads a page the later steps have already left.
    recoverable = "Full output persisted" in step_text
    honest = "not recoverable" in step_text
    led.check("a budget-cut exec snapshot accounts for the dropped tail",
              recoverable or honest, tail)
    led.check(
        "…and does NOT tell the model to re-snapshot a page the procedure has left",
        "standalone browser_snapshot" not in step_text,
        tail,
    )

    # ---- upload (modal-risky, so late) -------------------------------------
    led.log("\n--- browser_upload ---")
    ok, snap, _ = await p.snapshot()
    fref = ref_for(snap, "Upload file")
    ok, res = await p.call("browser_upload", paths=[args.upload_file], ref_id=fref)
    led.check("browser_upload returns success", ok and res.get("success"), json.dumps(res)[:220])
    await p.effect("…and the page's file input holds the file",
                   "#filename", os.path.basename(args.upload_file))

    # ---- dialog (blocks the page, so last) ---------------------------------
    led.log("\n--- browser_dialog ---")
    ok, snap, _ = await p.snapshot()
    aref = ref_for(snap, "Alert button")
    ok, res = await p.call("browser_click", ref_id=aref)
    led.log(f"  (click on the alert button returned: {json.dumps(res)[:200]})")
    ok, res = await p.call("browser_dialog", action="accept")
    led.check("browser_dialog accept returns success", ok and res.get("success"), json.dumps(res)[:220])
    # The claim that matters is not "accept returned ok" but "the page is
    # usable again" — an unhandled modal wedges every later verb.
    ok, val = await p.dom("#marker")
    led.check("…and the page is responsive again afterwards", ok and args.marker in str(val),
              f"marker={str(val)[:120]}")


# --------------------------------------------------------------------------
# scenario: frames
# --------------------------------------------------------------------------
async def scenario_frames(rpc, led, args):
    p = Page(rpc, led)

    led.log("\n--- open a page holding a cross-origin iframe ---")
    ok, res = await p.call("browser_open", url=args.page_url)
    led.check("browser_open succeeds", ok and res.get("success"), json.dumps(res)[:200])

    # CONTROL GROUP. Everything below is a claim about an out-of-process frame,
    # and a same-origin iframe would satisfy all of it while proving nothing.
    # Two independent proofs, because either alone can be true by accident:
    led.log("\n--- control: the iframe really is cross-origin ---")
    ok, val = await p.dom("#probe")
    led.check(
        "the parent document cannot reach into the child (same-origin policy)",
        ok and "CROSS_ORIGIN" in str(val),
        f"probe={val!r}",
    )
    ok, val = await p.js("document.querySelector('iframe').src")
    led.check(
        "…and the frame's origin differs from the parent's",
        ok and args.child_origin in str(val) and args.parent_origin not in str(val),
        f"src={val!r} parent={args.parent_origin} child={args.child_origin}",
    )
    # Third leg, read live rather than from a value the page recorded earlier:
    # the page's own probe is only as good as its timing (it originally ran at
    # parse, against the initial `about:blank`, and reported same-origin).
    ok, val = await p.js("document.querySelector('iframe').contentDocument === null")
    led.check(
        "…and the parent still cannot reach in at the moment we act",
        ok and val is True,
        f"contentDocument === null -> {val!r}",
    )

    led.log("\n--- does the accessibility snapshot reach into the frame? ---")
    ok, snap, _ = await p.snapshot()
    led.check("browser_snapshot succeeds", ok, snap[:120])
    led.check("the snapshot carries the PARENT's text", args.marker in snap, snap[:200])
    sees_child = args.child_marker in snap
    led.check(
        "the snapshot carries the CROSS-ORIGIN CHILD's text",
        sees_child,
        f"child marker {args.child_marker!r} " + ("found" if sees_child else "absent from the tree"),
    )

    led.log("\n--- can a control inside the frame be acted on? ---")
    cref = ref_for(snap, "Child frame button")
    if not led.check("a ref inside the frame is addressable from the snapshot", bool(cref),
                     f"ref={cref!r}"):
        return
    ok, res = await p.call("browser_click", ref_id=cref)
    led.check("clicking a ref inside the frame returns success", ok and res.get("success"),
              json.dumps(res)[:200])
    # "returned success" is not the claim; "the child's handler ran" is. The
    # parent cannot read into the frame (that is the control above), and
    # `browser_evaluate` runs in the parent — so the only oracle for the child's
    # state is the snapshot itself, which is the capability under test reading
    # back its own effect.
    ok, snap2, _ = await p.snapshot()
    led.check(
        "…and the child frame's own click handler really ran",
        ok and "child:yes" in snap2,
        f"child state line: {[l for l in snap2.splitlines() if 'child:' in l][:2]}",
    )


# --------------------------------------------------------------------------
# scenario: reap
# --------------------------------------------------------------------------
async def scenario_reap(rpc, led, args):
    p = Page(rpc, led)
    ctl = Page(rpc, led, profile=args.control_profile)

    led.log("\n--- bring up two managed profiles ---")
    ok, res = await p.call("browser_open", url=args.page_url)
    led.check("the idle-candidate profile opened", ok and res.get("success"), json.dumps(res)[:180])
    ok, res = await ctl.call("browser_open", url=args.page_url)
    led.check("the control profile opened", ok and res.get("success"), json.dumps(res)[:180])

    # The control profile also carries `max_tabs_per_profile = 2`, so opening a
    # third tab makes the LRU cap the one reaper behaviour that needs no idle
    # wait at all.
    for _ in range(2):
        await ctl.call("browser_open", url=args.page_url)
    ok, res = await ctl.call("browser_tabs", action="list")
    ctl_tabs_before = len((res or {}).get("tabs") or [])
    led.check("the control profile is over its tab cap", ctl_tabs_before > args.control_max_tabs,
              f"{ctl_tabs_before} tabs, cap {args.control_max_tabs}")

    listing = cli_sessions(args.cli, args.home)
    led.check("the CLI oracle sees both sessions open", session_status(listing) ==
              {"default": "open", args.control_profile: "open"},
              f"status={session_status(listing)}")

    led.log(f"\n--- wait {args.wait_secs}s for a reaper sweep (idle_timeout={args.idle_secs}s, sweep every 60s) ---")
    # Deliberately NOT polling the product for the answer: the oracle is the
    # CLI's own session list, read out of band.
    deadline = time.monotonic() + args.wait_secs
    while time.monotonic() < deadline:
        await asyncio.sleep(5)
        if session_status(cli_sessions(args.cli, args.home)).get("default") != "open":
            break
    final = cli_sessions(args.cli, args.home)
    status = session_status(final)
    led.log(f"  final status: {status}")

    led.check(
        "the idle managed session was closed by the reaper",
        status.get("default") == "closed",
        f"status={status}",
    )
    # The control: a profile whose timeout is far in the future must survive the
    # SAME sweep. Without this, a reaper that closes everything scores identical
    # to one that closes the right thing.
    led.check(
        "…and the non-idle control profile survived that same sweep",
        status.get(args.control_profile) == "open",
        f"status={status}",
    )

    ok, res = await ctl.call("browser_tabs", action="list")
    ctl_tabs_after = len((res or {}).get("tabs") or [])
    led.check(
        "…and the over-cap profile was trimmed to its tab cap",
        ok and ctl_tabs_after <= args.control_max_tabs,
        f"{ctl_tabs_before} -> {ctl_tabs_after} tabs, cap {args.control_max_tabs}",
    )

    # A reaped profile must still be usable: the next call re-launches lazily.
    ok, res = await p.call("browser_open", url=args.page_url)
    led.check("a reaped profile relaunches on the next call", ok and res.get("success"),
              json.dumps(res)[:200])


# --------------------------------------------------------------------------
# scenario: pdf  (pdf_generate's browser engine)
# --------------------------------------------------------------------------
async def scenario_pdf(rpc, led, args):
    led.log("\n--- pdf_generate render_engine=browser ---")
    out = os.path.join(args.out_dir, "pdf_generate_browser.pdf")
    ok, res = await rpc.invoke(
        "pdf_generate",
        {
            "content": f"# {args.marker}\n\nHello from the QA fixture.\n\n- one\n- two\n",
            "format": "markdown",
            "render_engine": "browser",
            "output_path": out,
        },
    )
    led.check("pdf_generate(browser) returns success", ok, json.dumps(res)[:400])
    # The engine that ran is legible from the message, and it is the claim: the
    # scenario runs with `playwright-cli` off the server's PATH, so reaching the
    # browser engine at all proves the pinned `binary_path` was consulted.
    led.check(
        "…via playwright-cli, resolved from the pinned binary_path (not PATH)",
        "playwright-cli" in json.dumps(res),
        json.dumps(res)[:300],
    )
    head = b""
    size = 0
    if os.path.exists(out):
        size = os.path.getsize(out)
        with open(out, "rb") as fh:
            head = fh.read(5)
    led.check("…and wrote a real PDF", head == b"%PDF-" and size > 1000, f"{out} size={size} head={head!r}")

    # The engine drives its OWN playwright-cli session (`aleph-pdf-gen`), built
    # from `PlaywrightCliConfig::default()` rather than the configured browser
    # config. The scenario pins `binary_path` to a CLI that is deliberately NOT
    # the one bare `which` would find first, so this is the claim that separates
    # "the engine uses the operator's configuration" from "it happened to find a
    # binary on PATH".
    listing = cli_sessions(args.cli, args.home)
    led.log(f"  cli sessions after the render:\n{listing.strip()[:400]}")
    led.check(
        "the engine's session is visible to the same scratch HOME as the tools",
        "aleph-pdf-gen" in listing or "(no browsers)" in listing,
        listing.strip()[:200],
    )

    led.log("\n--- pdf_generate render_engine=auto ---")
    out2 = os.path.join(args.out_dir, "pdf_generate_auto.pdf")
    ok, res = await rpc.invoke(
        "pdf_generate",
        {
            "content": f"# {args.marker} auto\n\nauto-engine body\n",
            "format": "markdown",
            "render_engine": "auto",
            "output_path": out2,
        },
    )
    led.check("pdf_generate(auto) returns success", ok, json.dumps(res)[:300])
    led.check("…and wrote a file", os.path.exists(out2) and os.path.getsize(out2) > 1000,
              f"{out2} size={os.path.getsize(out2) if os.path.exists(out2) else 0}")
    # `auto` picks its engine from an availability probe. With the CLI off PATH
    # and only `binary_path` pinned, a probe that asks `which` answers "Chrome
    # not available" and silently downgrades to the native renderer — a
    # lower-fidelity PDF with no error anywhere. The message names the engine.
    led.check(
        "…using the browser engine, i.e. the probe consulted the pin too",
        "playwright-cli" in json.dumps(res),
        json.dumps(res)[:300],
    )


# --------------------------------------------------------------------------
# scenario: existing  (the OTHER driver — Chrome DevTools MCP)
# --------------------------------------------------------------------------
async def scenario_existing(rpc, led, args):
    """Real-machine coverage for `driver = "existing_session"`.

    Narrower than `tools` on purpose. The claims that only this driver can
    settle are the ones where the two drivers answer the same question
    differently:

    * `wait_for` — this backend overrides the `Text` arm with a native tool and
      falls back to the SHARED `wait_probe` for `selector` / `url_contains`.
      That shared probe searches `evaluate`'s return for a sentinel that is a
      literal in the probe's own source, which is sound only if `evaluate`
      returns a value rather than a transcript. The managed driver returned a
      transcript and every wait there was a lie; whether this one does is a
      question no unit test can answer, because the fake backend returns
      exactly what the code hopes for.
    * the read path reaching a real page at all.
    """
    p = Page(rpc, led, profile=args.existing_profile)

    led.log("\n--- open a page through the existing-session driver ---")
    ok, res = await p.call("browser_open", url=args.page_url)
    if not led.check("browser_open succeeds", ok and res.get("success"), json.dumps(res)[:300]):
        return

    led.log("\n--- the read path ---")
    ok, snap, _ = await p.snapshot()
    led.check("browser_snapshot succeeds", ok, snap[:200])
    led.check("the snapshot carries the page marker", args.marker in snap, snap[:300])

    ok, val = await p.dom("#clicked")
    led.check("browser_evaluate reads the DOM", ok and "clicked:no" in str(val), f"value={val!r}")

    led.log("\n--- wait_for: the native Text arm ---")
    ok, res = await p.call("browser_wait_for", text=args.late_marker, timeout_ms=8000)
    led.check("wait_for(text) finds a delayed string",
              ok and res.get("success") and res.get("found"), json.dumps(res)[:200])
    ok, res = await p.call("browser_wait_for", text="NEVER_APPEARS_" + args.marker, timeout_ms=2000)
    led.check("wait_for(text) reports a genuine absence",
              ok and res.get("success") and res.get("found") is False, json.dumps(res)[:200])

    led.log("\n--- wait_for: the SHARED evaluate-probe arms ---")
    # The pair that matters. A backend whose `evaluate` echoed the script would
    # answer "found" to both, which is indistinguishable from working unless the
    # negative case is asserted alongside.
    ok, res = await p.call("browser_wait_for", selector="#clicked", timeout_ms=5000)
    led.check("wait_for(selector) finds an element that is present",
              ok and res.get("success") and res.get("found"), json.dumps(res)[:200])
    ok, res = await p.call("browser_wait_for", selector="#no-such-element", timeout_ms=2000)
    led.check("wait_for(selector) reports an element that is absent",
              ok and res.get("success") and res.get("found") is False, json.dumps(res)[:200])
    ok, res = await p.call("browser_wait_for", url_contains="tools.html", timeout_ms=5000)
    led.check("wait_for(url_contains) matches the current URL",
              ok and res.get("success") and res.get("found"), json.dumps(res)[:200])
    ok, res = await p.call("browser_wait_for", url_contains="definitely-not-in-the-url",
                           timeout_ms=2000)
    led.check("wait_for(url_contains) reports a URL that does not match",
              ok and res.get("success") and res.get("found") is False, json.dumps(res)[:200])

    led.log("\n--- the act path ---")
    cref = ref_for(snap, "Go button")
    if led.check("a labelled control resolved to a ref", bool(cref), f"ref={cref!r}"):
        ok, res = await p.call("browser_click", ref_id=cref)
        led.check("browser_click returns success", ok and res.get("success"), json.dumps(res)[:200])
        await p.effect("…and the page's click handler actually ran", "#clicked", "clicked:yes")


SCENARIOS = {
    "tools": scenario_tools,
    "frames": scenario_frames,
    "reap": scenario_reap,
    "pdf": scenario_pdf,
    "existing": scenario_existing,
}


def parse_args():
    ap = argparse.ArgumentParser()
    ap.add_argument("url")
    ap.add_argument("scenario", choices=sorted(SCENARIOS))
    ap.add_argument("--page-url", required=True)
    ap.add_argument("--second-url", default="")
    ap.add_argument("--marker", required=True)
    ap.add_argument("--console-marker", default="")
    ap.add_argument("--late-marker", default="")
    ap.add_argument("--child-marker", default="")
    ap.add_argument("--parent-origin", default="")
    ap.add_argument("--child-origin", default="")
    ap.add_argument("--home", required=True)
    ap.add_argument("--cli", required=True)
    ap.add_argument("--out-dir", default="/tmp")
    ap.add_argument("--upload-file", default="")
    ap.add_argument("--control-profile", default="control")
    ap.add_argument("--control-user-data-dir", default="")
    ap.add_argument("--control-max-tabs", type=int, default=2)
    ap.add_argument("--existing-profile", default="existing")
    ap.add_argument("--idle-secs", type=int, default=5)
    ap.add_argument("--wait-secs", type=int, default=150)
    return ap.parse_args()


async def main():
    args = parse_args()
    led = Ledger()
    async with websockets.connect(args.url, max_size=None) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-browser-tools")
        await SCENARIOS[args.scenario](rpc, led, args)
    return led.verdict()


sys.exit(asyncio.run(main()))
