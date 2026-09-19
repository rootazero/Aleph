#!/usr/bin/env python3
"""Stage drivers for qa/browser_dual. One websocket, `tools.invoke`, and
out-of-band oracles (`pgrep`/`ps`, `lsof`, the sidecar JSON, the installed file)
for everything the code under test would otherwise be asked to confirm about
itself."""
import argparse
import asyncio
import glob
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import time

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "browser_managed"))
from qa_rpc import Ledger, Rpc, ran, ws_connect, http_json  # noqa: E402

# The obscura build every dated claim in this file is dated TO. A claim about a
# gap between the engines is a MEASUREMENT, and a measurement carries the build
# it was taken on exactly as it carries the commit (判据 §18). On any other
# build such a claim is UNKNOWN — never PASS, because nothing measured it, and
# never FAIL, because nothing says it should still hold.
MEASURED_ON = "0.2.2"


def ps_lines(pattern):
    """Full command lines of live processes matching `pattern`.

    `pgrep -f -- "<pat>"`: the `--` is required, because BSD pgrep reads a
    leading `-` in the pattern as one of its own options and then matches
    nothing — which reads exactly like "the process is gone" and would make
    every death assertion below pass for free."""
    out = subprocess.run(["pgrep", "-f", "--", pattern], capture_output=True, text=True)
    pids = [p for p in out.stdout.split() if p.strip()]
    lines = []
    for pid in pids:
        ps = subprocess.run(["ps", "-p", pid, "-o", "command="], capture_output=True, text=True)
        if ps.returncode == 0 and ps.stdout.strip():
            lines.append((int(pid), ps.stdout.strip()))
    return lines


def token_processes(token):
    """Processes whose argv contains `token` as a WHOLE WORD.

    `pgrep -f` matches a substring of the joined command line, so a pattern of
    `--storage-dir=<X>` also matches `--storage-dir=<X>-neighbour`. That is the
    exact prefix confusion the `reap` stage's control exists to detect, and on
    its first run this fixture fell for it: `the obscura orphan was swept`
    reported the NEIGHBOUR's pid as the un-swept orphan, so a correct sweep
    read as a broken one. 判据 §12, inside the instrument — the same string
    compared two different ways in two places.

    `argv_names_dir` compares whole argv words for the same reason, so this is
    the fixture asking the question the way the code under test answers it
    rather than a second, looser way."""
    return [(pid, line) for pid, line in ps_lines(token) if token in line.split()]


def main_processes(token):
    """`token_processes`, minus a browser's own helper processes.

    A Chromium launch is ELEVEN processes on this machine, and every one of
    them inherits `--user-data-dir=<ours>` — GPU, network, storage and seven
    renderers. Two consequences, and both bite:

    * the argv claims would read whichever of the eleven `pgrep` happened to
      list first (it was the main process only because pids sort that way);
    * `reap` asserts a browser is GONE, and a helper that outlives its parent
      by a second turns "the orphan was swept" into a coin flip.

    A helper is named by its own argv: Chrome gives every child a `--type=`
    switch, and the main process has none. obscura is a single process and is
    unaffected either way."""
    return [(pid, line) for pid, line in token_processes(token)
            if not any(w.startswith("--type=") for w in line.split())]


def argv_of(pid):
    """One live process's argv as a VECTOR, not a joined line.

    `ps -o command=` joins with spaces and there is no way back — a
    `--storage-dir=/a b/c` would split into two words that match nothing. The
    kernel already split it, so read it that way: on macOS and Linux
    `/proc`-free, `ps -ww -o args=` is still joined, so the vector comes from
    `ps -o command=` split only where this fixture's own paths cannot contain
    a space (QA_ROOT is `mktemp -d` under $TMPDIR). Stated rather than assumed:
    if that ever stops being true this helper is the one place to fix.
    """
    ps = subprocess.run(["ps", "-p", str(pid), "-o", "command="], capture_output=True, text=True)
    if ps.returncode != 0:
        return []
    return ps.stdout.strip().split()


def sidecars(aleph_home):
    """Every engine sidecar record, whatever leaf directory holds them.

    Globbed rather than hard-coded: the registry leaf is `engine/process.rs`'s
    business (it is the frozen literal `chromium` for BOTH engines today) and
    this fixture must not be a second place that decides it. An empty result is
    reported as a FAILED claim, never skipped — "I looked nowhere" and "there is
    nothing" are different answers."""
    out = {}
    for path in glob.glob(os.path.join(aleph_home, "data", "browser", "*", "*.json")):
        try:
            with open(path) as fh:
                out[path] = json.load(fh)
        except (OSError, ValueError):
            out[path] = None
    return out


def engine_record_items(aleph_home, engine):
    """`(path, record)` for every sidecar naming `engine`.

    The path is not decoration: anything this fixture must write ALONGSIDE a
    genuine record has to take the leaf directory from one, not spell it — see
    `sidecars()` on why this file must not be a second decider for that leaf."""
    return [(p, r) for p, r in sidecars(aleph_home).items()
            if r and r.get("engine") == engine]


def engine_records(aleph_home, engine):
    return [r for _, r in engine_record_items(aleph_home, engine)]


def server_log(a):
    """Everything this run's server wrote, from both places it writes."""
    body = ""
    try:
        with open(os.path.join(a.qa_root, "server.log")) as fh:
            body += fh.read()
    except OSError:
        pass
    for path in sorted(glob.glob(os.path.join(a.aleph_home, "logs", "aleph-server.log*"))):
        try:
            with open(path, errors="replace") as fh:
                body += fh.read()
        except OSError:
            pass
    return body


def sha256_file(path):
    h = hashlib.sha256()
    with open(path, "rb") as fh:
        for chunk in iter(lambda: fh.read(1 << 20), b""):
            h.update(chunk)
    return h.hexdigest()


def obscura_version(binary):
    """`obscura --version`'s version token, or None.

    `None` is UNKNOWN and is spent as UNKNOWN by every caller: a dated claim
    whose date cannot be read is not thereby a claim about today."""
    try:
        out = subprocess.run([binary, "--version"], capture_output=True, text=True, timeout=60)
    except (OSError, subprocess.SubprocessError):
        return None
    m = re.search(r"(\d+\.\d+\.\d+)", out.stdout + out.stderr)
    return m.group(1) if m else None


def dated(led, build, claim, ok, detail=""):
    """A claim about a GAP between the engines, dated to the build that was
    measured.

    Three states, not two. On `MEASURED_ON` it is an ordinary check. On any
    other build — or on none that could be read — it prints UNKNOWN and settles
    nothing: reporting PASS for "the gap is still there" on a build nobody has
    measured would be asserting something this run did not observe, and
    reporting FAIL would be blaming a build for not matching a note."""
    if build == MEASURED_ON:
        return led.check(f"{claim} [measured on obscura {MEASURED_ON}]", ok, detail)
    Ledger.log(
        f"  [UNKNOWN] {claim} — this harness found obscura {build or 'unreadable'}, "
        f"and the expectation is dated to {MEASURED_ON}. Re-measure before "
        f"believing either answer. (observed: {detail})"
    )
    return None


def unfence(text):
    """The body inside the untrusted-content fence.

    Every snapshot the model receives — text tree AND `format="json"` — is
    wrapped in `<<<EXTERNAL_UNTRUSTED_CONTENT …>` markers, because a page
    chooses the strings inside it. That is correct and the fixture asserts it;
    it also means `json.loads` on the raw body fails, and a `except ValueError:
    state = {}` then turns a perfectly good tree into "no nodes" — which this
    stage read as three missing elements on its first run."""
    return "\n".join(
        ln for ln in text.splitlines()
        if not ln.startswith("<<<EXTERNAL_UNTRUSTED_CONTENT")
        and not ln.startswith("<<<END_EXTERNAL_UNTRUSTED_CONTENT")
    )


async def snapshot_text(rpc, profile="default"):
    ok, body = await rpc.invoke("browser_snapshot", {"profile": profile})
    return ok, body or {}, (body or {}).get("snapshot") or ""


async def snapshot_state(rpc, led, profile="default"):
    """The JSON face of the same observation — the only one carrying geometry.

    `format="json"` is the CDP driver's own tree (`PageState`), so a node's
    `rect` here is what Aleph itself believes about the element's box. A tree
    that does not parse is reported, never swallowed: an empty `nodes` list and
    an unparsed body are different answers and only one of them is about the
    page."""
    ok, body = await rpc.invoke("browser_snapshot", {"profile": profile, "format": "json"})
    raw = unfence((body or {}).get("snapshot") or "")
    try:
        state = json.loads(raw or "{}")
    except ValueError as exc:
        led.check("the JSON snapshot parses", False, f"{exc}: {raw[:200]}")
        return ok, body or {}, {}
    led.check("the JSON snapshot parses and carries nodes",
              bool(state.get("nodes")), f"{len(state.get('nodes', []))} nodes")
    return ok, body or {}, state


def node_named(state, needle, want_rect=False):
    """The node this page's text belongs to — and, with `want_rect`, the one
    that owns its BOX.

    A `<div onclick>` whose only content is text renders as two nodes: a
    `generic` with the rect and an accessible name of `""`, and a `text` leaf
    under it carrying the words. Matching on the words alone therefore answers
    the leaf, whose `rect` is `null` — and this stage read that as "the block
    clickable has no rect" on its first run, against a text tree that was
    printing `@8,97 220x44` two lines above. A box belongs to the element, so
    when a box is what the caller wants, climb to the element that has one."""
    nodes = state.get("nodes", [])
    hits = [i for i, n in enumerate(nodes)
            if needle in (n.get("name") or "") or needle in (n.get("text") or "")]
    if not hits:
        return None
    if not want_rect:
        return nodes[hits[0]]
    for i in hits:
        if nodes[i].get("rect"):
            return nodes[i]
    for i in hits:
        parent = nodes[i].get("parent")
        while parent is not None and 0 <= parent < len(nodes):
            if nodes[parent].get("rect"):
                return nodes[parent]
            parent = nodes[parent].get("parent")
    return nodes[hits[0]]


async def goto(rpc, url, profile="default"):
    """`browser_navigate`'s `goto` arm. The tool takes an `action` enum, not a
    bare `url` — `{"goto": {"url": …}}` is the wire shape serde reads."""
    return await rpc.invoke("browser_navigate", {"profile": profile, "action": {"goto": {"url": url}}})


# --------------------------------------------------------------------------
# provision
# --------------------------------------------------------------------------


async def stage_provision(rpc, led, a):
    """The ledger install, against the REAL release.

    There is no local fixture release this stage could use, and the reason is
    the product's trust model rather than an omission:
    `ReleaseSource::for_runtime` pins the release METADATA — and therefore the
    expected sha256 — at `api.github.com` and never at the operator's mirror,
    and `configured_host` discards any `download_host` that is not `https://`.
    So a `python3 -m http.server` on loopback can supply neither half.

    What that costs is named rather than papered over: the tampered-digest
    refusal is UNRUN here. What it buys is asserted instead — the two-host
    split is a real-machine claim no unit test makes, and the second half of
    this stage measures it.
    """
    installed = os.path.join(a.aleph_home, "runtimes", "obscura", a.release_tag, "obscura")
    led.check("no obscura is installed before the stage starts", not os.path.exists(installed), installed)

    ok, body = await rpc.invoke("runtime_manage", {"action": "install", "capability": "obscura"})
    led.check("runtime_manage accepts capability=obscura", ran(ok, body), json.dumps(body)[:300])
    message = (body or {}).get("message", "")
    # The job id has no JSON key of its own — `RuntimeManageOutput` is
    # `{ok, message, runtimes}` and the id is interpolated into `message`
    # (`runtime_manage.rs:333-347`). Read it from where it is, and say so if it
    # is not there: "the install ran inline" and "the id moved" are different
    # answers and this stage must not guess between them.
    m = re.search(r"process_id:(\d+)", message) or re.search(r"as job (\d+)", message)
    led.check("the install reported a pollable job id in its message", bool(m), message[:300])
    if m:
        await rpc.invoke("bash", {"process_action": "wait", "process_id": int(m.group(1))})

    for _ in range(300):
        if os.path.exists(installed):
            break
        time.sleep(1)
    led.check("the binary landed at the ledger's tag-named path", os.path.exists(installed), installed)
    if not os.path.exists(installed):
        led.check("(the rest of this stage needs that binary)", False, server_log(a)[-1500:])
        return
    led.check("it is executable", os.access(installed, os.X_OK))
    led.check("`--version` reports the pinned tag", obscura_version(installed) == MEASURED_ON,
              str(obscura_version(installed)))
    led.check(
        "obscura-worker was NOT installed beside it (one member, not the archive)",
        not os.path.exists(os.path.join(os.path.dirname(installed), "obscura-worker")),
        str(sorted(os.listdir(os.path.dirname(installed)))),
    )
    led.check("and no partial download was left behind",
              not glob.glob(os.path.join(os.path.dirname(installed), "*.tmp"))
              and not glob.glob(os.path.join(os.path.dirname(installed), "*.part")),
              str(sorted(os.listdir(os.path.dirname(installed)))))
    # The bytes that came off the socket, against a copy that was already on
    # this machine before the stage ran. Not a restatement of the installer's
    # own check — that one compares the download against the metadata, and this
    # one compares it against an independently-obtained file.
    want = sha256_file(a.obscura_binary)
    got = sha256_file(installed)
    led.check("the installed bytes match the obscura this host already had",
              want == got, f"{got[:16]}… vs {want[:16]}…")

    _, listing = await rpc.invoke("runtime_manage", {"action": "list"})
    rows = [r for r in (listing or {}).get("runtimes", []) if r.get("name") == "obscura"]
    led.check("runtime_manage{list} shows exactly one obscura row", len(rows) == 1, str(rows))
    # `status` is `format!("{:?}")` of `CapabilityStatus`, so the word is the
    # PascalCase variant name and not a serde spelling.
    led.check("and it reports Ready", bool(rows) and rows[0].get("status", "").startswith("Ready"), str(rows))

    # --- the two-host split, measured ---------------------------------------
    # Point the ASSET mirror at a host nothing answers on and install again.
    # `configured_host` calls `Config::load()` at install time, so rewriting the
    # file is enough — no restart, and the RED control lives in this fixture
    # rather than in someone's working copy.
    Ledger.log(f"  … re-pointing [general.browser.obscura] download_host at {a.dead_mirror}")
    # Section-scoped, in `add_browser_config.py::set_key`'s idiom. A file-wide
    # `re.sub` for `^download_host` would also strip
    # `[general.browser.runtime] download_host` — the Playwright CDN mirror,
    # a different key for a different subsystem — and this stage would then be
    # quietly editing something it never meant to touch.
    with open(a.config) as fh:
        lines = fh.read().splitlines()
    out, in_section, inserted = [], False, False
    for line in lines:
        if line.strip().startswith("["):
            in_section = line.strip() == "[general.browser.obscura]"
            out.append(line)
            if in_section:
                out.append(f'download_host = "{a.dead_mirror}"')
                inserted = True
            continue
        if in_section and re.match(r"^\s*download_host\s*=", line):
            continue  # superseded by the line inserted at the header
        out.append(line)
    led.check("the config has an [general.browser.obscura] section to edit", inserted)
    with open(a.config, "w") as fh:
        fh.write("\n".join(out) + "\n")
    # The EFFECT, not the edit: the claim is that the installer will read this,
    # and the only evidence for that is the bytes on disk.
    led.check("the dead mirror is in the config the installer will read",
              f'download_host = "{a.dead_mirror}"' in open(a.config).read())
    shutil.rmtree(os.path.dirname(installed), ignore_errors=True)
    before = len(server_log(a))

    ok, body = await rpc.invoke("runtime_manage", {"action": "install", "capability": "obscura"})
    m = re.search(r"process_id:(\d+)", (body or {}).get("message", "")) \
        or re.search(r"as job (\d+)", (body or {}).get("message", ""))
    if m:
        await rpc.invoke("bash", {"process_action": "wait", "process_id": int(m.group(1))})
    time.sleep(5)
    tail = server_log(a)[before:]

    led.check("an unreachable asset mirror installs NOTHING", not os.path.exists(installed), installed)
    led.check("and the failure names the mirror it could not reach",
              a.dead_mirror in tail, tail[-600:])
    # The half that makes the first half mean something. `Fetch::Checksum`'s own
    # words appear only in a failure of the METADATA fetch; their absence, with
    # the asset fetch having failed, is what says the checksum request went to
    # api.github.com and did not follow the mirror.
    led.check("the release asset bytes are what failed",
              "the release asset bytes" in tail, tail[-600:])
    led.check("the CHECKSUM fetch did not follow the mirror",
              "always read from GitHub's API" not in tail, tail[-600:])

    Ledger.log(
        "  [UNRUN] a tampered sha256 refuses and leaves nothing — not runnable on a real "
        "machine from this fixture: the expected digest comes from api.github.com "
        "(ReleaseSource::for_runtime) and a non-https mirror is discarded "
        "(configured_host), so there is no loopback host that can serve either half. "
        "That claim is owned by github_release.rs::a_digest_mismatch_installs_nothing."
    )


# --------------------------------------------------------------------------
# open
# --------------------------------------------------------------------------


async def stage_open(rpc, led, a):
    ok, body = await rpc.invoke("browser_open", {"profile": "default", "url": a.page_url})
    led.check("browser_open on the default profile succeeds", ran(ok, body), json.dumps(body)[:300])

    procs = token_processes(f"--storage-dir={a.obscura_storage}")
    led.check("an obscura is running on this run's storage dir", len(procs) == 1,
              str([p for p, _ in procs]))
    if not procs:
        return
    argv = argv_of(procs[0][0])
    led.check("its storage-dir is under the scratch root, as ONE argv token",
              f"--storage-dir={a.obscura_storage}" in argv, str(argv))
    led.check("it binds loopback explicitly", "--host=127.0.0.1" in argv, str(argv))
    led.check("it was NOT given --allow-file-access", "--allow-file-access" not in argv, str(argv))
    # Not decoration: obscura refuses to navigate to 127.0.0.1 without it
    # ("Access to private/internal IP address 127.0.0.1 is not allowed",
    # measured), so the page below could not have loaded if this were missing —
    # and the flag is passed only because this profile's own network policy
    # already permits private ranges.
    led.check("private ranges were opened deliberately, by flag",
              "--allow-private-network" in argv, str(argv))

    cars = sidecars(a.aleph_home)
    led.check("a sidecar record exists", bool(cars), str(list(cars)))
    ours = engine_records(a.aleph_home, "obscura")
    led.check("and it records engine=obscura", len(ours) == 1, json.dumps(list(cars.values()))[:400])
    if not ours:
        return
    led.check("its pid is the process we just found", ours[0].get("pid") == procs[0][0],
              f"sidecar pid {ours[0].get('pid')} vs live pid {procs[0][0]}")
    # `user_data_dir` is the wire key; `data_dir` is the Rust field name behind a
    # `#[serde(rename)]`. Reading the Rust name here would answer `None` for a
    # record that is perfectly correct.
    led.check("and the storage dir it recorded is the one on the command line",
              ours[0].get("user_data_dir") == a.obscura_storage, str(ours[0].get("user_data_dir")))
    http = ours[0].get("http_url")
    led.check("with an endpoint (the second sidecar write happened)", bool(http), str(http))
    if not http:
        return

    port = int(http.rsplit(":", 1)[1])
    try:
        status, _ = http_json(port, "/json/version")
    except Exception as exc:  # noqa: BLE001 — the claim IS the absence of one
        status = repr(exc)
    led.check("/json/version answers on that endpoint", status == 200, str(status))
    # The readiness gate's OTHER half: the engine navigated. `/json/version`
    # alone is the sentinel plan 1 recorded as insufficient — Chrome answered it
    # while every first navigation silently died.
    ok, body = await rpc.invoke("browser_evaluate", {"profile": "default", "script": "document.title"})
    led.check("and the page really navigated", ran(ok, body) and "dual-engine QA" in json.dumps(body),
              json.dumps(body)[:300])

    owner = subprocess.run(["lsof", "-a", "-nP", f"-iTCP:{port}", "-sTCP:LISTEN", "-Fp"],
                           capture_output=True, text=True)
    owner_pid = next((int(l[1:]) for l in owner.stdout.splitlines() if l.startswith("p")), None)
    led.check("the obscura we launched owns the port", owner_pid == procs[0][0],
              f"listener pid {owner_pid}, obscura pid {procs[0][0]}")

    # --- the OTHER engine's gate (spec §7.4: 就绪门对两引擎都成立) ------------
    ok, body = await rpc.invoke("browser_open", {"profile": "escape", "url": a.page_url})
    led.check("browser_open on the chromium profile succeeds", ran(ok, body), json.dumps(body)[:300])
    chrome = main_processes(f"--user-data-dir={a.chromium_udd}")
    led.check("a chromium is running (exactly one main process, helpers filtered)",
              len(chrome) == 1, str([p for p, _ in chrome]))
    chrome_argv = argv_of(chrome[0][0]) if chrome else []
    # Round-7's headline defect: without this switch Chrome answers
    # /json/version and looks healthy while every first navigation per page
    # silently dies.
    led.check("its argv still carries --use-mock-keychain",
              "--use-mock-keychain" in chrome_argv, " ".join(chrome_argv)[:300])
    ok, body = await rpc.invoke("browser_evaluate", {"profile": "escape", "script": "document.title"})
    led.check("and the chromium profile really navigated",
              ran(ok, body) and "dual-engine QA" in json.dumps(body), json.dumps(body)[:300])
    led.check("its sidecar records engine=chromium",
              len(engine_records(a.aleph_home, "chromium")) == 1,
              json.dumps(list(sidecars(a.aleph_home).values()))[:400])


# --------------------------------------------------------------------------
# snapshot
# --------------------------------------------------------------------------


async def stage_snapshot(rpc, led, a):
    await rpc.invoke("browser_open", {"profile": "default", "url": a.page_url})
    ok, body, text = await snapshot_text(rpc)
    led.check("browser_snapshot succeeds", ran(ok, body), json.dumps(body)[:300])
    led.check("it returned a body", bool(text), json.dumps(body)[:300])
    if not text:
        return
    # The body reaches the model wrapped in an untrusted-content fence, so
    # line 0 is the fence and not the header — this stage read it as the header
    # on its first run and failed three claims that were in fact true. Assert
    # the fence, then find the header by the token that identifies it.
    led.check("the tree is fenced as external untrusted content",
              "EXTERNAL_UNTRUSTED_CONTENT" in text.splitlines()[0], text.splitlines()[0][:200])
    head = next((ln for ln in text.splitlines() if ln.startswith("# engine=")), "")
    led.check("there is a header line", bool(head), text[:400])
    led.check("the header names the engine", "engine=obscura" in head, head)
    led.check("the header names the url", "index.html" in head, head)
    led.check("the header reports the no-box count", "no_box=" in head, head)
    led.check("the link has a ref", "[ref=" in text and "go by ref" in text, text[:800])
    led.check("the block clickable is offered too", "go by coordinates" in text, text[:800])
    # The two elements obscura CAN see as hidden, via the computed flags rather
    # than via a box (measured: an inline element's box is zero either way, so a
    # box test could not tell the ghost from the link).
    led.check("a display:none element is not offered", f"{a.marker}-hidden" not in text, text[:800])
    led.check("an opacity:0 element is not offered", f"{a.marker}-ghost" not in text, text[:800])
    # `ref_count agrees with the rendered refs` stood here and could not go red:
    # on the TEXT face `ref_count` IS `text.matches("[ref=").count()`
    # (`snapshot.rs`), so this was the tool's own derivation recomputed over the
    # tool's own string — 判据 §2, a predicate with no failing case. The claim
    # belongs to `snapshot.rs`'s tests. The cross-check worth having is against the
    # `format="json"` face, where `ref_count` is `snap.ref_count` and the two
    # derivations really are independent.
    led.check("the snapshot was not truncated (so ref_count is the page total here)",
              body.get("truncated") is False, str(body.get("truncated")))


# --------------------------------------------------------------------------
# click
# --------------------------------------------------------------------------


async def stage_click(rpc, led, a):
    """Both click paths, on the element that HAS a box — and the fail-closed
    refusal on the one that does not.

    An earlier draft aimed the by-ref half at the inline `<a>`, on the premise
    that a ref click is box-free. It is not: `OCCLUSION_JS` hit-tests the
    resolved element and refuses anything whose `getBoundingClientRect()` is
    zero-sized, which is right — an element with no box cannot receive a click
    from anyone. On obscura v0.2.2 that makes EVERY inline element unclickable
    through Aleph, by ref as well as by coordinates, and that consequence is
    asserted below rather than left as a surprise.
    """
    build = obscura_version(a.obscura_binary)
    await rpc.invoke("browser_open", {"profile": "default", "url": a.page_url})
    _, _, text = await snapshot_text(rpc)
    ref = next((ln.split("[ref=")[1].split("]")[0] for ln in text.splitlines()
                if "[ref=" in ln and "go by coordinates" not in ln and "@" in ln), None)
    led.check("the block clickable's ref is in the snapshot", ref is not None, text[:800])
    if ref is None:
        return

    ok, body = await rpc.invoke("browser_click", {"profile": "default", "ref_id": ref})
    led.check("click by ref succeeds", ran(ok, body), json.dumps(body)[:300])
    _, ev = await rpc.invoke("browser_evaluate", {"profile": "default", "script": "location.pathname"})
    led.check("…and the URL changed", "second" in json.dumps(ev), json.dumps(ev)[:300])

    # The stale half: that ref was minted against the previous document, and no
    # snapshot has been taken since.
    ok, body = await rpc.invoke("browser_click", {"profile": "default", "ref_id": ref})
    blob = json.dumps(body)
    # The SAFETY claim, and the one that must never go green by accident: a ref
    # from a dead document must not act on the live one. Stated as "refused AND
    # nothing moved", never as "the message said stale" — a message test alone
    # would pass for a dead engine, a refused connection or a denied approval,
    # which is 判据 §2's question with only one answer left.
    led.check("the pre-navigation ref does NOT act on the new document",
              not ran(ok, body), blob[:400])
    _, ev = await rpc.invoke("browser_evaluate",
                             {"profile": "default", "script": "location.pathname"})
    led.check("…and the page did not move", "second" in json.dumps(ev), json.dumps(ev)[:300])
    # The QUALITY half. This was a DATED KNOWN GAP until Task 18b, and it is now
    # an ordinary claim — that flip is the whole of 18b seen from here.
    #
    # The gap was: Aleph classified a dead ref by matching the ENGINE's own error
    # prose (`"No node with given id"`), which is a sentence obscura never emits
    # — and, measured on Chrome 152, one Chromium does not emit for this hazard
    # either (it says `Node with given id does not belong to the document`). So
    # the refusal arrived three calls later as `DOM.scrollIntoViewIfNeeded
    # failed: -32601`, a protocol error where the model needed the one sentence
    # naming its next move (判据 §17).
    #
    # It is refused by Aleph's own bookkeeping now: the event pump folds
    # `Page.frameNavigated` into `RefTable::reset_for_document`, so by the time
    # this ref is used the tab knows its document changed and `resolve` answers
    # `Navigated` before anything reaches the wire.
    #
    # NOT `dated()`, deliberately. A dated claim says "this is what this build of
    # the other project does"; this sentence is produced by Aleph before the
    # engine is consulted at all. What is still build-dependent is that the
    # engine ANNOUNCES its navigations — and a build that stopped doing so must
    # turn this RED, not print `[UNKNOWN]`, because the refusal would silently
    # fall back to the engine accidents this task removed.
    #
    # Stated POSITIVELY, which also closes a blindness the previous version had:
    # the old negative form ("no 'stale', no 'browser_snapshot'") stayed green
    # under the fix-r1 mutation that made this very click report
    # `{"success": true}` — a string test that cannot tell "refused with the
    # wrong words" from "not refused at all" (判据 §2).
    led.check("…and the refusal uses the ref vocabulary, not a protocol error",
              "is stale" in blob and "browser_snapshot" in blob
              and "-32601" not in blob,
              blob[:400])

    # WHICH of the two layers answered, and why this stage does not assert one.
    #
    #   * the DOCUMENT layer — the event pump folds `Page.frameNavigated` into
    #     `RefTable::reset_for_document`, so `resolve` answers `Navigated`
    #     before anything reaches the wire. Cheapest, and the better sentence.
    #   * the NODE layer — `resolve_target` asks the resolved object whether it
    #     is still connected. Costs a round trip, needs no event to have
    #     arrived.
    #
    # On obscura 0.2.2 the document layer CANNOT see this particular
    # navigation, and the reason is the engine's, not Aleph's — measured
    # 2026-09-19 with `<scratchpad>/t18b_probe4.py`, both routes in one run:
    #
    #   dispatched mouse click on <div onclick="location.href=…">
    #       -> ONE Page.frameNavigated, carrying the OLD loaderId,
    #          and Page.getFrameTree afterwards still reports the OLD loaderId
    #   Runtime.evaluate("location.href=…")
    #       -> Page.frameNavigated with a NEW loaderId, then loadEventFired
    #          and frameStoppedLoading; getFrameTree reports the new one
    #
    # So for a click-driven navigation the loader never turns over on this
    # engine, and NOTHING loader-based can fire: not the pump, not
    # `PageState::build`'s reset, not `navigate()`'s boundary. The node layer is
    # the only thing that can answer, which is why it exists and why it is not
    # an optional extra on top of a document comparison.
    #
    # DATED, because it is a statement about what THIS build of the other
    # project does: on a build that turned the loader over here, the sentence
    # would legitimately become "the page navigated", and this harness must say
    # UNKNOWN rather than fail a build for being better.
    dated(led, build,
          "KNOWN GAP: a CLICK-driven navigation does not turn obscura's main "
          "loaderId over, so the refusal names the node rather than the "
          "navigation — the document layer is blind to it on this engine",
          "no longer in the page" in blob, blob[:400])

    # …and the document layer, on the route where this engine DOES turn the
    # loader over. Aleph issues no `navigate` here — the page moves itself, from
    # a script — so the only thing that can have retired the ref is the pump
    # folding `Page.frameNavigated` in. Delete that arm and this goes red while
    # every safety claim above stays green: the difference between "refused" and
    # "refused for the right reason" (判据 §17).
    #
    # The half second is a margin, not a guess: the fold is one lock and one
    # map clear, measured at under a millisecond in the server log.
    await goto(rpc, a.page_url)
    _, _, text_sc = await snapshot_text(rpc)
    sc_ref = next((ln.split("[ref=")[1].split("]")[0] for ln in text_sc.splitlines()
                   if "[ref=" in ln and "go by coordinates" not in ln and "@" in ln), None)
    led.check("the block clickable has a ref for the script-navigation case",
              sc_ref is not None, text_sc[:800])
    if sc_ref is not None:
        await rpc.invoke("browser_evaluate",
                         {"profile": "default", "script": "location.href='/second.html'"})
        time.sleep(0.5)
        _, ev = await rpc.invoke("browser_evaluate",
                                 {"profile": "default", "script": "location.pathname"})
        led.check("the page moved itself, with no browser_navigate",
                  "second" in json.dumps(ev), json.dumps(ev)[:300])
        ok, body = await rpc.invoke("browser_click", {"profile": "default", "ref_id": sc_ref})
        moved = json.dumps(body)
        led.check("a ref from before a navigation the PAGE started is refused, "
                  "and the refusal names the navigation",
                  (not ran(ok, body)) and "the page navigated" in moved, moved[:400])

    # Coordinates, on the same BLOCK element. Not on the link: measured on
    # obscura v0.2.2 an inline element reports a zero box from every route, so a
    # coordinate click on it would be a click at (0, 0) — a stage that aimed
    # there would be testing the wrong thing whichever way it came out.
    await goto(rpc, a.page_url)
    _, _, state = await snapshot_state(rpc, led)
    blk = node_named(state, "go by coordinates", want_rect=True)
    led.check("the block clickable is in the JSON face", blk is not None, json.dumps(state)[:400])
    led.check("and it has a rect", bool(blk and blk.get("rect")), json.dumps(blk)[:400])
    if blk and blk.get("rect"):
        rect = blk["rect"]
        ok, body = await rpc.invoke("browser_click", {
            "profile": "default",
            "x": rect["x"] + rect["w"] / 2,
            "y": rect["y"] + rect["h"] / 2,
        })
        led.check("click by coordinates succeeds", ran(ok, body), json.dumps(body)[:300])
        _, ev = await rpc.invoke("browser_evaluate",
                                 {"profile": "default", "script": "location.pathname"})
        led.check("…and the URL changed", "second" in json.dumps(ev), json.dumps(ev)[:300])

    # The measured gap, asserted so it cannot improve silently — and DATED, so
    # the day this harness meets another obscura it says UNKNOWN rather than
    # vouching for a reading it never took.
    await goto(rpc, a.page_url)
    _, _, state = await snapshot_state(rpc, led)
    lnk = node_named(state, "go by ref")
    dated(led, build,
          "KNOWN GAP (see T0 U9): this inline link has no rect — obscura gives "
          "<a> a zero box from getBoxModel, getContentQuads and "
          "getBoundingClientRect alike",
          lnk is not None and lnk.get("rect") is None, json.dumps(lnk)[:400])
    # …and what that costs, which is more than the missing coordinates: the
    # element cannot be clicked AT ALL. Asserted as a REFUSAL, because the one
    # answer that would be worse than "no" here is a reported success for a
    # click that reached nothing (判据 §11).
    lnk_ref = next((ln.split("[ref=")[1].split("]")[0] for ln in text.splitlines()
                    if "[ref=" in ln and "go by ref" in ln), None)
    led.check("the inline link is still offered a ref", lnk_ref is not None, text[:800])
    if lnk_ref is not None:
        _, _, fresh = await snapshot_text(rpc)
        lnk_ref = next((ln.split("[ref=")[1].split("]")[0] for ln in fresh.splitlines()
                        if "[ref=" in ln and "go by ref" in ln), lnk_ref)
        ok, body = await rpc.invoke("browser_click", {"profile": "default", "ref_id": lnk_ref})
        dated(led, build,
              "a boxless element REFUSES the click rather than reporting a "
              "no-op success, and says the box is the reason",
              (not ran(ok, body)) and "zero-sized box" in json.dumps(body),
              json.dumps(body)[:400])
        _, ev = await rpc.invoke("browser_evaluate",
                                 {"profile": "default", "script": "location.pathname"})
        led.check("…and nothing navigated", "index" in json.dumps(ev), json.dumps(ev)[:300])

    # --- the OTHER staleness: same document, the node left it ---------------
    #
    # The navigation case above is decided from Aleph's own ref table. This one
    # cannot be — the document has not changed, so no bookkeeping Aleph keeps
    # can know the element is gone; the page is the only thing that knows. It is
    # the case the deleted prose classifier was written for, and the case that
    # engine never had an answer to: obscura's `DOM.resolveNode` succeeds on a
    # dead `backendNodeId`, `DOM.getBoxModel` invents a box for it, and what
    # used to refuse the click was `OCCLUSION_JS` reporting a "zero-sized box" —
    # a label describing the wrong fact about an element that is not in the page
    # at all (判据 §17).
    #
    # `resolve_target` now asks the node itself (`this.isConnected`), one
    # question in one place, the same JS on both engines. This is that answer
    # measured on the real default engine.
    await goto(rpc, a.page_url)
    _, _, text2 = await snapshot_text(rpc)
    gone_ref = next((ln.split("[ref=")[1].split("]")[0] for ln in text2.splitlines()
                     if "[ref=" in ln and "go by coordinates" not in ln and "@" in ln), None)
    led.check("the block clickable has a ref for the detach case",
              gone_ref is not None, text2[:800])
    if gone_ref is not None:
        # ONE expression, not two statements: `browser_evaluate` on obscura
        # rejects `a(); b` with `SyntaxError: Unexpected token ';'`, and the
        # first draft of this stage did exactly that — the removal never
        # happened and the click below succeeded, correctly.
        ok, ev = await rpc.invoke("browser_evaluate", {
            "profile": "default",
            "script": "document.getElementById('blk').remove()",
        })
        led.check("the remove() call itself ran",
                  ran(ok, ev) and "threw" not in json.dumps(ev), json.dumps(ev)[:300])
        # The removal is asserted, not assumed: if the element were still there
        # the refusal below would be about something else entirely, and the
        # claim would be green for the wrong reason.
        #
        # A SENTINEL, not a bare boolean: the first draft asserted
        # `"true" in json.dumps(ev)` and was green against a body that carried
        # `"success": true` from the ENVELOPE while the script had thrown — a
        # predicate that could not go red (判据 §2). `BLK-GONE` appears nowhere
        # else, and its opposite is spelled out so the two are distinguishable.
        ok, ev = await rpc.invoke("browser_evaluate", {
            "profile": "default",
            "script": "document.getElementById('blk') === null "
                      "? 'BLK-GONE' : 'BLK-STILL-HERE'",
        })
        led.check("the element really was removed, same document",
                  "BLK-GONE" in json.dumps(ev), json.dumps(ev)[:300])
        ok, body = await rpc.invoke("browser_click", {"profile": "default", "ref_id": gone_ref})
        blob2 = json.dumps(body)
        led.check("a ref whose node left the document REFUSES the click",
                  not ran(ok, body), blob2[:400])
        led.check("…and says the element is no longer in the page, not that it "
                  "has an odd box",
                  "no longer in the page" in blob2 and "browser_snapshot" in blob2
                  and "zero-sized box" not in blob2,
                  blob2[:400])
        _, ev = await rpc.invoke("browser_evaluate",
                                 {"profile": "default", "script": "location.pathname"})
        led.check("…and the page did not move for it",
                  "index" in json.dumps(ev), json.dumps(ev)[:300])


# --------------------------------------------------------------------------
# stall
# --------------------------------------------------------------------------


async def wedge(rpc, led, a, tool, args):
    """Load the spinning page, then fire one verb into the block it causes.

    Returns `(body_json, seconds_waited)`. The page arms its loop 300 ms after
    load and then blocks EVERY command on the connection — measured at ~4.3 s
    on this machine, with a raw `Target.getTargets` (the verb the source survey
    lists as lock-free) taking 4.32 s to answer. So the call has to be issued
    inside that window and the configured budget has to expire inside it too."""
    await goto(rpc, a.stall_url)
    time.sleep(0.9)
    t0 = time.time()
    ok, body = await rpc.invoke(tool, args)
    waited = time.time() - t0
    led.check(f"the stalled {tool} was refused, not hung forever", waited < 60, f"{waited:.1f}s")
    led.check(f"…and {tool} waited about the configured timeout, not zero",
              waited >= a.stall_timeout * 0.5, f"{waited:.1f}s vs {a.stall_timeout}s")
    led.check(f"…and {tool} did not report success", not ran(ok, body), json.dumps(body)[:300])
    return json.dumps(body), waited


async def stage_stall(rpc, led, a):
    """A genuinely wedged engine, on the two paths that answer it differently.

    `map_cdp_err` turns a CDP timeout into `BrowserError::EngineBusy`, whose
    sentence tells the model to retry or to switch engines. Every ACTION verb
    goes through it. The SNAPSHOT path does not, and says something else — see
    the dated claim at the end.
    """
    build = obscura_version(a.obscura_binary)
    await rpc.invoke("browser_open", {"profile": "default", "url": a.page_url})
    before = token_processes(f"--storage-dir={a.obscura_storage}")
    led.check("obscura is running before the stall", bool(before), str([p for p, _ in before]))

    # --- the action path, which is where EngineBusy lives -------------------
    blob, _ = await wedge(rpc, led, a, "browser_evaluate",
                          {"profile": "default", "script": "1+1"})
    # `EngineBusy`'s Display never spells its own variant name, and it says
    # `did not answer … within Ns` rather than `waited`. Asserting the variant
    # name would be asserting a word that is not on the wire; these are.
    led.check("the refusal says the engine did not answer",
              "did not answer" in blob and "obscura" in blob, blob[:400])
    led.check("and it says how long it gave it",
              f"within {int(a.stall_timeout)}s" in blob, blob[:400])
    led.check("and it names the way across", "switch_engine" in blob, blob[:400])
    led.check("and it says the request was queued, not refused by the page",
              "still" in blob and "queued" in blob, blob[:400])

    after = token_processes(f"--storage-dir={a.obscura_storage}")
    led.check("the engine is STILL ALIVE afterwards",
              {p for p, _ in after} == {p for p, _ in before},
              f"{[p for p, _ in before]} -> {[p for p, _ in after]}")
    # Measured: obscura ends the page's own task at its autonomous budget, so
    # the engine recovers by itself. A stage that stopped at the refusal would
    # not notice a driver that had left the connection wedged.
    time.sleep(10)
    ok, body = await rpc.invoke("browser_evaluate", {"profile": "default", "script": "1+1"})
    led.check("the engine answers again once the page stops spinning",
              ran(ok, body), json.dumps(body)[:300])
    ok, title = await rpc.invoke("browser_evaluate", {"profile": "default", "script": "document.title"})
    led.check("and the page's own script was terminated, not completed",
              ran(ok, title) and "done" not in json.dumps(title), json.dumps(title)[:300])

    # --- the snapshot path, which does not ----------------------------------
    # A SECOND stall rather than a second verb inside the first: each probe
    # spends the whole configured budget, and two of them back to back would
    # have the later one landing outside the ~4.3 s window — a stage whose
    # verdict depended on that margin would be measuring this machine's load.
    blob, _ = await wedge(rpc, led, a, "browser_snapshot", {"profile": "default"})
    dated(led, build,
          "KNOWN GAP: the snapshot path calls a BUSY engine 'stopped' and does "
          "not produce the EngineBusy sentence — same runtime fact, second "
          "vocabulary (判据 §9), and the engine this run just watched recover "
          "had not stopped at all (判据 §17)",
          "stopped" in blob and "did not answer" not in blob and "switch_engine" not in blob,
          blob[:400])
    led.check("…though it does name the method and the budget it spent",
              "Page.getLayoutMetrics" in blob and "timed out after" in blob, blob[:400])
    still = token_processes(f"--storage-dir={a.obscura_storage}")
    led.check("…and the engine it called 'stopped' is still running",
              {p for p, _ in still} == {p for p, _ in before},
              f"{[p for p, _ in before]} -> {[p for p, _ in still]}")


# --------------------------------------------------------------------------
# reap
# --------------------------------------------------------------------------


async def stage_reap(rpc, led, a):
    """The boot sweep, with a negative half that can actually go red.

    `reap_orphans` decides per RECORD: a sidecar's pid whose argv names that
    sidecar's own `data_dir`, under that record's own engine flag, is killed.
    So both engines' orphans are swept and neither is the other's control — an
    earlier draft of this stage expected a live chromium orphan to be SPARED,
    which is simply not what the sweep does.

    The control that does work is a process the registry never recorded, whose
    argv is a PREFIX-NEIGHBOUR of one it did: `<storage>-neighbour` against a
    record of `<storage>`. `argv_names_dir` compares whole argv words, so it
    must survive; an implementation that joined the argv and used `contains`
    kills it. That is the `work` / `work-archive` case `argv_names_dir`'s own
    doc names, and it is the only shape here whose verdict distinguishes the
    two implementations.
    """
    await rpc.invoke("browser_open", {"profile": "default", "url": a.page_url})
    await rpc.invoke("browser_open", {"profile": "escape", "url": a.page_url})
    obs = token_processes(f"--storage-dir={a.obscura_storage}")
    chrome = main_processes(f"--user-data-dir={a.chromium_udd}")
    led.check("an obscura is running", len(obs) == 1, str([p for p, _ in obs]))
    led.check("a chromium is running too", len(chrome) == 1, str([p for p, _ in chrome]))
    led.check("the registry recorded both", len(engine_records(a.aleph_home, "obscura")) == 1
              and len(engine_records(a.aleph_home, "chromium")) == 1,
              json.dumps(list(sidecars(a.aleph_home).values()))[:400])

    neighbour_dir = a.obscura_storage + "-neighbour"
    os.makedirs(neighbour_dir, exist_ok=True)
    neighbour = subprocess.Popen(
        [a.obscura_binary, "serve", "--host=127.0.0.1", "--port=0",
         f"--storage-dir={neighbour_dir}"],
        stdout=open(os.path.join(a.qa_root, "neighbour.log"), "w"),
        stderr=subprocess.STDOUT,
    )
    time.sleep(3)
    neighbour_pids = [p for p, _ in token_processes(f"--storage-dir={neighbour_dir}")]
    led.check("a prefix-neighbour obscura is running", len(neighbour_pids) == 1, str(neighbour_pids))
    led.check("…and its directory really is a prefix-extension of the recorded one",
              neighbour_dir.startswith(a.obscura_storage) and neighbour_dir != a.obscura_storage,
              f"{a.obscura_storage} vs {neighbour_dir}")

    os.kill(int(a.server_pid), 9)
    time.sleep(3)

    # --- the control, and it has to be THIS shape to discriminate anything ---
    #
    # A neighbour with no sidecar proves only that the sweep is registry-scoped,
    # because `reap_orphans` iterates SIDECARS and reads each record's pid — a
    # process nothing recorded is never looked at. Measured: with
    # `argv_names_dir` mutated to `argv.join(" ").contains(&joined)`, the stage
    # stayed fully GREEN. 判据 §3 — the control only covered a shape the hazard
    # does not take.
    #
    # The hazard `argv_names_dir`'s own doc names is a RECORD whose pid now
    # belongs to a process whose argv names a NEIGHBOURING directory ("a
    # substring test therefore kills the neighbouring profile's browser"). So
    # the fixture writes exactly that record: dir = the recorded one, pid = the
    # neighbour's. Whole-word matching takes the recycled-pid arm — drop the
    # record, kill nothing. Substring matching finds `--storage-dir=<X>` inside
    # `--storage-dir=<X>-neighbour` and kills a browser that was never ours.
    #
    # Written after the server is dead so nothing overwrites it, and alongside
    # the genuine record rather than replacing it, so the positive claims below
    # still have their own orphan to sweep.
    items = engine_record_items(a.aleph_home, "obscura")
    genuine = [r for _, r in items]
    led.check("the genuine obscura record is readable before the control is added",
              len(items) == 1, json.dumps(genuine)[:300])
    # The leaf is taken from the genuine record's OWN path, never spelled. It is
    # the frozen literal `chromium` for both engines today, but that is
    # `engine/process.rs`'s decision and a rename is a permitted act with a
    # migration — on that day a hard-coded leaf would drop this control into a
    # directory the sweep never walks, and the stage would stay green while
    # proving less (判据 §3). No record, no control: the leaf is then a thing
    # this fixture does not know, and inventing one would be reading an absent
    # answer as a value (判据 §8), so the claim below goes red instead.
    forged_path = (os.path.join(os.path.dirname(items[0][0]), "neighbour-probe.json")
                   if items else None)
    if forged_path:
        with open(forged_path, "w") as fh:
            json.dump({
                "engine": "obscura",
                "pid": neighbour_pids[0] if neighbour_pids else 0,
                "http_url": None,
                "user_data_dir": a.obscura_storage,
                "aleph_version": genuine[0].get("aleph_version"),
            }, fh)
    led.check("the control record names the recorded dir but the neighbour's pid",
              bool(forged_path) and os.path.exists(forged_path) and bool(neighbour_pids),
              f"path={forged_path} pid={neighbour_pids} dir={a.obscura_storage}")
    led.check("both engines survived the server's death (they are orphans now)",
              bool(token_processes(f"--storage-dir={a.obscura_storage}"))
              and bool(main_processes(f"--user-data-dir={a.chromium_udd}")),
              f"obscura={[p for p, _ in token_processes(f'--storage-dir={a.obscura_storage}')]} "
              f"chromium={[p for p, _ in main_processes(f'--user-data-dir={a.chromium_udd}')]}")

    proc = subprocess.Popen([a.server_bin, "start"], cwd=a.server_cwd,
                            stdout=open(os.path.join(a.qa_root, "server2.log"), "w"),
                            stderr=subprocess.STDOUT)
    # Recorded BEFORE the health wait, and on disk rather than returned: this
    # process outlives the driver, and `run.sh`'s trap is the only thing that
    # can reap it. Writing it after the wait — or only on a clean exit — leaks
    # a live server bound to the gateway port whenever the stage fails, and the
    # NEXT run then dies at `no config generated` with `Address already in
    # use`, which reads like a port-choice problem three steps away from the
    # cause. Measured: that is exactly how this stage's first red run ended.
    with open(os.path.join(a.qa_root, "server2.pid"), "w") as fh:
        fh.write(str(proc.pid))
    up = False
    for _ in range(90):
        try:
            http_json(int(a.gateway_port), "/health")
            up = True
            break
        except Exception:  # noqa: BLE001
            time.sleep(1)
    led.check("the replacement server came up", up, f"pid {proc.pid}")
    time.sleep(6)

    led.check("the obscura orphan was swept",
              not token_processes(f"--storage-dir={a.obscura_storage}"),
              str([p for p, _ in token_processes(f"--storage-dir={a.obscura_storage}")]))
    led.check("the chromium orphan was swept too (the census reaches both engines)",
              not main_processes(f"--user-data-dir={a.chromium_udd}"),
              str([l for _, l in main_processes(f"--user-data-dir={a.chromium_udd}")])[:300])
    # The half that makes the other two mean something. A sweep that killed
    # every obscura would pass both of those on its own, and so would one that
    # matched its recorded directory as a SUBSTRING of the argv — which is what
    # the control record above is there to catch.
    led.check("the prefix-neighbour was NOT killed, though a record named its pid",
              bool(token_processes(f"--storage-dir={neighbour_dir}")),
              str([p for p, _ in token_processes(f"--storage-dir={neighbour_dir}")]))
    led.check("and the swept records' sidecars are gone",
              not engine_records(a.aleph_home, "obscura")
              and not engine_records(a.aleph_home, "chromium"),
              json.dumps(list(sidecars(a.aleph_home).values()))[:400])
    neighbour.kill()
    print(f"restarted server pid {proc.pid}", flush=True)


STAGES = {"provision": stage_provision, "open": stage_open, "snapshot": stage_snapshot,
          "click": stage_click, "stall": stage_stall, "reap": stage_reap}


async def main():
    p = argparse.ArgumentParser()
    p.add_argument("ws")
    p.add_argument("stage")
    p.add_argument("--page-url")
    p.add_argument("--stall-url")
    p.add_argument("--second-url")
    p.add_argument("--marker")
    p.add_argument("--qa-root")
    p.add_argument("--aleph-home")
    p.add_argument("--config")
    p.add_argument("--release-repo")
    p.add_argument("--release-tag")
    p.add_argument("--dead-mirror")
    p.add_argument("--obscura-binary")
    p.add_argument("--server-bin")
    p.add_argument("--server-cwd")
    p.add_argument("--gateway-port")
    p.add_argument("--server-pid")
    p.add_argument("--obscura-storage")
    p.add_argument("--chromium-udd")
    p.add_argument("--stall-timeout", type=float, default=2.0)
    a = p.parse_args()
    led = Ledger()
    async with ws_connect(a.ws) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-browser-dual")
        await STAGES[a.stage](rpc, led, a)
    return led.verdict()


if __name__ == "__main__":
    sys.exit(asyncio.run(main()))
