#!/usr/bin/env python3
"""Prove spec §5.3/§5.5 on two real engines: the login survives the switch.

Every claim here is one a fake engine cannot make. The load-bearing one is the
LAST pair: the source engine's process is gone and a chromium is in its place.
A switch that leaves the old browser running looks identical to a correct one
from every RPC angle.
"""
import argparse
import asyncio
import json
import os
import sys
from collections import Counter

sys.path.insert(0, os.path.join(os.path.dirname(__file__), "..", "browser_managed"))
from qa_rpc import Ledger, Rpc, ws_connect  # noqa: E402

# `token_processes` and `main_processes` carry two pieces of reasoning this
# file must not re-derive (判据 §1): the `--` that BSD pgrep needs before a
# pattern starting with `-` (without it it matches nothing, which reads exactly
# like "the process is gone"), and the whole-word match that stops
# `--storage-dir=<X>` from also finding `--storage-dir=<X>-neighbour`.
sys.path.insert(0, os.path.dirname(__file__))
from drive import (  # noqa: E402
    MEASURED_ON,
    main_processes,
    node_named,
    obscura_version,
    snapshot_state,
    snapshot_text,
    token_processes,
)

ap = argparse.ArgumentParser()
ap.add_argument("ws")
ap.add_argument("--page-url", required=True)
ap.add_argument("--aleph-home", required=True)
ap.add_argument("--obscura-storage", required=True)
# Read for its VERSION, not to launch it: the F2 pin below is dated to a build,
# and a dated expectation that never looks at the build is a date in a comment.
ap.add_argument("--obscura-binary", required=True)
args = ap.parse_args()

led = Ledger()
log, check = Ledger.log, led.check
COOKIE = "aleph-qa-switch"
VALUE = "carried-%d" % os.getpid()
# The obscura the `default` profile runs: the fixture names that directory
# explicitly (run.sh's `--user-data-dir "$OBSCURA_STORAGE"`), so this is the
# path the launcher actually passes and not one this script guessed.
OBSCURA_PAT = f"--storage-dir={args.obscura_storage}"
# The chromium the switch starts, on the other hand, is NOT that directory: a
# `user_data_dir` written for obscura is in obscura's format, so
# `ProfileManager::request_from` hands the target engine the managed path under
# its own `data_subdir` instead. That derivation is
# `browser_state_dir("chromium")/sanitize_session_key("default")`.
CHROMIUM_UDD = os.path.join(args.aleph_home, "data", "browser", "chromium", "default")

# The orphan-reap registry. `sidecar_registry_dir()` is
# `browser_state_dir(SIDECAR_REGISTRY_LEAF)`, and that leaf is the string
# "chromium" for BOTH engines — frozen deliberately, because renaming it would
# strand every existing record (the constant's own doc says so). So the
# directory below is not the chromium data dir above despite sharing a name, and
# the file names inside it are `<engine>-<profile>.json`.
SIDECAR_DIR = os.path.join(args.aleph_home, "data", "browser", "chromium")
OBSCURA_RECORD = os.path.join(SIDECAR_DIR, "obscura-default.json")
CHROMIUM_RECORD = os.path.join(SIDECAR_DIR, "chromium-default.json")


def sidecar_listing():
    """What is actually in the registry, for a failure message that names it."""
    try:
        return sorted(os.listdir(SIDECAR_DIR))
    except OSError as e:
        return f"<unreadable: {e}>"


# ---------------------------------------------------------------------------
# F2 — the two-engine addressable-ref comparison
# ---------------------------------------------------------------------------
#
# The branch's own declared merge blocker: *same page, both engines, compare the
# set of addressable refs.* Both prerequisites landed in Tasks 17 and 5; the
# comparison itself had never been made anywhere — not in QA, not in a fixture
# pair, not in a unit test. This is it.
#
# **WHAT IS COMPARED, and why a comparison has to say so.** Two browser engines
# will not produce byte-identical trees, so an assertion over the raw text has
# exactly one possible outcome and measures nothing (判据 §2). The unit compared
# here is therefore the thing an agent actually addresses an element BY:
#
#     key = (role, accessible name)          for an element line
#     key = ("text:", rendered content)      for a text leaf
#
# as a MULTISET, not a set. Multiset because absorption
# (`render::payload_flags` / `name_covers_all_text`) is decided by the name: if
# one engine's name covers a text child and the other's does not, one tree
# prints one line and the other prints two, and the model is looking at a
# different number of addressable things. A set would hide exactly that.
#
# **WHAT IS EXCLUDED, each for a stated reason — this is the tolerance, and it
# is the part worth attacking:**
#
#   * the `[ref=eN]` id itself. Refs are minted per capture
#     (`PageState::build`), so two snapshots on the SAME engine already disagree
#     about them. Comparing ids would be 判据 §2's 恒红 face: an assertion that
#     can only fail.
#   * `@x,y wxh` geometry. It is a measurement, not an address, and obscura
#     v0.2.2 is already MEASURED to report a zero box for inline elements where
#     Chromium reports a real one. Comparing geometry would answer "do the two
#     engines lay this page out identically", which is a different and much
#     weaker question than F2's. The header's own `no_box=n/m` token is where
#     that difference is reported, and the `snapshot` stage already asserts it
#     is printed.
#   * state tokens (`[checked]` / `[disabled]` / …). A state is something the
#     model reads off an element it has already addressed. Task 20 recorded a
#     stale `[checked]` on the default engine, which would make this go red for
#     a reason that is not about addressability.
#   * the header lines (`# engine=`, `# INCOMPLETE:`). `engine=`, `gen=`,
#     `fetch=…ms` and `no_box=` differ BY CONSTRUCTION.
#
# **What is NOT excluded, and so can still go red:** which elements earn a ref
# at all, what each one's role is, what each one's accessible name is, how many
# lines each element produces, and the exact text of every addressable text
# leaf. That is the whole of what an agent's plan is written against.
#
# ⚠️ If this comes back with a difference, that is a FINDING and must be
# reported as one. Widening the key until it passes would convert the one
# measurement this branch is named after into a tolerance wide enough to accept
# anything.
#
# ---------------------------------------------------------------------------
# IT CAME BACK WITH A DIFFERENCE, AND THE DIFFERENCE IS PINNED BELOW.
#
# MECHANISM, established rather than guessed. `roles::is_interactive` is a
# six-way OR. Five of the six read attributes or computed styles BOTH engines
# supply. The sixth is `clickable_hint == Some(true)`, fed from exactly one
# place — `fetch_chromium.rs`'s `DOMSnapshot.isClickable` — and
# `fetch_obscura.rs` leaves it `None` deliberately, with a comment arguing
# (correctly) that faking it from `cursor_pointer` would be worse. So an element
# whose ONLY evidence of clickability is a script-attached listener is
# interactive on chromium and not on obscura; on obscura it earns no line at
# all, and the model sees only its text leaf.
#
# FALSIFIED, not merely read: giving such an element `style="cursor:pointer"` —
# a signal both engines report, reaching the FIFTH disjunct — made obscura mint
# the same node and the symmetric difference went empty, every other key
# unchanged. So this assertion has been observed BOTH red and green on this
# fixture, which is the only useful answer to "in what circumstance does it go
# the other way?" (判据 §2).
#
# WHY A PIN AND NOT A RED. Leaving the claim red would make this stage
# permanently 17 PASS / 1 FAIL, and the next REAL divergence would arrive
# hidden behind a failure everybody had learned to skip — 判据 §2's 恒红 face.
# Widening the key would be the 恒绿 face. The third shape is to assert the
# measured symmetric difference ITSELF, so that the gap growing and the gap
# CLOSING are both red, for different reasons and with different sentences.
# ---------------------------------------------------------------------------

# The pin. Version and date live HERE, beside the fact they date, not in a
# comment somewhere else that can rot on its own (判据 §1).
#
# `PIN_OBSCURA_BUILD` is imported from `drive.py` rather than written again:
# "which obscura build was measured" already has an author on this branch, and a
# second copy of it is the defect this file spends forty lines avoiding.
PIN_MEASURED_AT = "2026-09-20"
PIN_OBSCURA_BUILD = MEASURED_ON
# What chromium offers and obscura does not, exactly. Two instances of ONE
# class, kept distinguishable on purpose:
#   * `generic ""`                    — `<label for="txt">`, the instance the
#                                       first F2 run caught by accident;
#   * `generic "JS listener only"`    — `<div>` + `addEventListener`, the class's
#                                       representative shape, added so the pin
#                                       names a class rather than an accident.
PIN_CHROMIUM_ONLY = Counter({("generic", ""): 1, ("generic", "JS listener only"): 1})
PIN_OBSCURA_ONLY = Counter()
# The element whose geometry the model is told to fall back on. Block-level, so
# obscura's zero-box-on-inline behaviour is not what is being measured here.
PIN_DOOR_NAME = "JS listener only"


def _quoted_at(line, start):
    """Read one `quote()`-produced string beginning at `line[start]`.

    `page_state::render::quote` escapes `"` `\\` `\n` `\r` `\t` and C0 as
    `\\u{..}`, and deliberately does NOT escape brackets — so a name containing
    " [" is legal and a naive `split(" [")` would cut a name in half and then
    report the two halves as an engine disagreement. Reading the string the way
    it was written is the only parse that cannot invent one.
    """
    if start >= len(line) or line[start] != '"':
        return None, start
    out = []
    i = start + 1
    while i < len(line):
        c = line[i]
        if c == "\\" and i + 1 < len(line):
            out.append(line[i : i + 2])
            i += 2
            continue
        if c == '"':
            return "".join(out), i + 1
        out.append(c)
        i += 1
    # Unterminated: the renderer cannot produce this, so say so rather than
    # returning a plausible prefix.
    return None, len(line)


def addressable_keys(text):
    """The multiset of addressable (role, name) keys in one rendered tree.

    Returns `(Counter, [unparsed lines])`. The second half is not decoration: a
    ref line this function cannot read is not "no difference", it is "I could
    not look" — and an unknown may only say so (判据 §8). Every `[ref=` line has
    a quoted string by construction (`render::line` quotes a text leaf's content
    and quotes an element's name whenever it is non-empty OR the node is
    interactive, and `build` mints refs only for interactive-or-text nodes), so
    an unparsed line means the line shape changed and this parser is stale.
    """
    keys = Counter()
    unparsed = []
    for raw in text.splitlines():
        line = raw.strip()
        if "[ref=" not in line:
            continue
        if not line.startswith("- "):
            unparsed.append(line)
            continue
        rest = line[2:]
        if rest.startswith("text: "):
            value, _ = _quoted_at(rest, len("text: "))
            if value is None:
                unparsed.append(line)
                continue
            keys[("text:", value)] += 1
            continue
        role, _, after = rest.partition(" ")
        value, _ = _quoted_at(after, 0)
        if value is None:
            unparsed.append(line)
            continue
        keys[(role, value)] += 1
    return keys, unparsed


def describe(counter):
    """A stable, readable rendering of one side, for the failure message."""
    return "; ".join(
        f"{role} {name!r}" + (f" x{n}" if n > 1 else "")
        for (role, name), n in sorted(counter.items())
    )


async def main():
    async with ws_connect(args.ws) as ws:
        rpc = Rpc(ws)
        await rpc.connect("qa-browser-dual-switch")

        ok, res = await rpc.invoke(
            "browser_open", {"url": args.page_url, "profile": "default"}
        )
        check(
            "browser_open on the default profile succeeds",
            ok and res.get("success") is True,
            json.dumps(res)[:200],
        )
        check(
            "browser_open reports obscura",
            res.get("engine") == "obscura",
            str(res.get("engine")),
        )

        before = token_processes(OBSCURA_PAT)
        check("an obscura owns our storage dir before the switch", bool(before), str(before))
        check(
            "and the orphan-reap registry has a record for it",
            os.path.exists(OBSCURA_RECORD),
            f"{OBSCURA_RECORD} in {sidecar_listing()}",
        )

        # F2, first half: the page as obscura sees it. Taken here, while the
        # source engine is still the live one, because after the switch it is
        # gone and there is no second chance.
        ok, body, obscura_tree = await snapshot_text(rpc)
        check(
            "browser_snapshot on obscura returned a tree",
            ok and bool(obscura_tree),
            json.dumps(body)[:200],
        )

        # THE DOOR, measured in the same run that records the gap.
        #
        # `browser_snapshot`'s DESCRIPTION tells the model that an element with
        # no ref on this engine can still be found in the `format="json"` face
        # and clicked by coordinates. That sentence is only allowed to exist
        # while these two claims are green — a named door that does not open is
        # fail-dead, not fail-closed (判据 §14), and a DESCRIPTION promising one
        # would be the same expensive-direction lie W7 was.
        _, _, obscura_state = await snapshot_state(rpc, led)
        door = node_named(obscura_state, PIN_DOOR_NAME, want_rect=True)
        check(
            "the ref-less element is still IN the json face",
            door is not None,
            json.dumps(door)[:200]
            if door is not None
            else f"NO node named {PIN_DOOR_NAME!r} among "
            f"{len(obscura_state.get('nodes', []))} nodes",
        )
        if door is not None:
            rect = door.get("rect") or {}
            check(
                "…and it carries a rect the model can click by coordinates",
                bool(rect) and (rect.get("w") or 0) > 0 and (rect.get("h") or 0) > 0,
                json.dumps(door)[:300],
            )
            # Outcome-dependent text, for the same reason the pin pair's is.
            # Found by mutation N-D: when obscura DOES start seeing this
            # element, the gap has closed and this claim goes red too — and its
            # first wording ("which is the whole reason the door is needed")
            # read as a breakage, so the operator got two reds where the
            # situation calls for one instruction. Good news must not arrive
            # wearing a failure's label (判据 §17).
            no_ref = door.get("ref") is None
            check(
                "…while having no ref, which is the whole reason the door is needed",
                no_ref,
                json.dumps(door)[:300]
                if no_ref
                else f"this element now HAS a ref ({door.get('ref')}) on obscura — the gap "
                f"closed for it, which is the SAME event the F2 pin reports below. "
                f"Nothing is broken; follow the pin's expiry instructions and this "
                f"claim goes with it",
            )

        ok, res = await rpc.invoke(
            "browser_cookies",
            {
                "profile": "default",
                "action": "set",
                "name": COOKIE,
                "value": VALUE,
                "domain": "127.0.0.1",
                "path": "/",
            },
        )
        check(
            "a cookie is set on obscura",
            ok and res.get("success") is True,
            json.dumps(res)[:200],
        )

        ok, res = await rpc.invoke("browser_tabs", {"profile": "default", "action": "list"})
        check(
            "the tab list shows the page before the switch",
            args.page_url in json.dumps(res),
            json.dumps(res)[:200],
        )

        ok, res = await rpc.invoke(
            "browser_session",
            {
                "profile": "default",
                "action": "switch_engine",
                "engine": "chromium",
                "migrate": True,
            },
        )
        # `ok` alone is not the claim: every arm of this tool returns
        # `success: false` inside an `ok` envelope for a refusal, so reading
        # only `ok` would call a refused switch a success and blame the wrong
        # step three claims later.
        check(
            "switch_engine succeeds",
            ok and res.get("success") is True,
            json.dumps(res)[:400],
        )
        check(
            "it reports the engine it landed on",
            res.get("switched_to") == "chromium",
            str(res.get("switched_to")),
        )
        check(
            "it reports at least one cookie carried",
            (res.get("cookies_moved") or 0) >= 1,
            str(res.get("cookies_moved")),
        )
        check(
            "it returns the new page tree in the same call",
            bool(res.get("snapshot_text")),
            str(res.get("snapshot_text"))[:120],
        )

        ok, res = await rpc.invoke("browser_cookies", {"profile": "default", "action": "list"})
        listed = json.dumps(res)
        check(
            "the cookie set on obscura is readable on chromium",
            COOKIE in listed and VALUE in listed,
            listed[:300],
        )

        ok, res = await rpc.invoke("browser_tabs", {"profile": "default", "action": "list"})
        check(
            "the tab is on the same URL it was on before the switch",
            args.page_url in json.dumps(res),
            json.dumps(res)[:200],
        )

        # F2, second half. A fresh `browser_snapshot`, NOT the `snapshot_text`
        # the switch returned in its own envelope: this has to be the same TOOL
        # on the same page with only the engine different, or the comparison is
        # between two faces rather than between two engines (B12 — a reading
        # taken beside the path is not a reading about the path).
        ok, body, chromium_tree = await snapshot_text(rpc)
        check(
            "browser_snapshot on chromium returned a tree",
            ok and bool(chromium_tree),
            json.dumps(body)[:200],
        )

        # Every verb above this line went through `get_backend`, which resolves
        # the engine from the PROFILE CONFIG. That they answered at all is the
        # only evidence that the switch moved the config as well as the
        # registry — without it each one would have answered `EngineMismatch`.
        chrome = main_processes(f"--user-data-dir={CHROMIUM_UDD}")
        check("a chromium is now running", bool(chrome), str(chrome))

        # The load-bearing claim, and the only one invisible from the RPC face.
        after = token_processes(OBSCURA_PAT)
        check(
            "no obscura with our storage dir survives the switch",
            not after,
            f"before={before} after={after}",
        )

        # The record, on the REAL registry layout. The unit test asserts this
        # against a fake whose sidecar naming was itself wrong on exactly this
        # dimension until this round, so the claim is worth making where the
        # directory is the product's own. Keyed on (profile, engine): before
        # that, the target's launch overwrote the source's record and the
        # source's shutdown then deleted the file that described the SURVIVOR.
        check(
            "the surviving chromium has an orphan-reap record of its own",
            os.path.exists(CHROMIUM_RECORD),
            f"{CHROMIUM_RECORD} in {sidecar_listing()}",
        )
        check(
            "and the dead obscura's record was cleaned up, not left to strand",
            not os.path.exists(OBSCURA_RECORD),
            f"{OBSCURA_RECORD} in {sidecar_listing()}",
        )

        ok, res = await rpc.invoke(
            "browser_session",
            {
                "profile": "default",
                "action": "switch_engine",
                "engine": "chromium",
                "migrate": True,
            },
        )
        check(
            "switching to the engine already running is refused, not reported as success",
            res.get("success") is False,
            json.dumps(res)[:200],
        )

        # ------------------------------------------------------------------
        # F2 — the comparison. See the block above `_quoted_at` for what the
        # key is, what it excludes and why each exclusion is a different
        # question rather than a tolerance.
        # ------------------------------------------------------------------
        obscura_keys, obscura_bad = addressable_keys(obscura_tree)
        chromium_keys, chromium_bad = addressable_keys(chromium_tree)

        # An unreadable ref line is "I could not look", never "no difference".
        check(
            "every ref line in obscura's tree parsed",
            not obscura_bad,
            str(obscura_bad[:3]),
        )
        check(
            "every ref line in chromium's tree parsed",
            not chromium_bad,
            str(chromium_bad[:3]),
        )
        # Non-vacuity, and it is not optional: two empty trees have an empty
        # symmetric difference, so without this the headline claim below is
        # green on a comparison that saw nothing (判据 §2). Anchored on an
        # element the fixture page adds for this stage rather than on a count,
        # so it stays meaningful if the page grows.
        check(
            "the obscura tree actually offers the fixture's controls",
            any(name == "Press me" for _, name in obscura_keys),
            describe(obscura_keys)[:400],
        )

        only_obscura = obscura_keys - chromium_keys
        only_chromium = chromium_keys - obscura_keys

        # The pin is dated to a BUILD, so the build is checked rather than
        # assumed. A version bump is the likeliest way this expires, and an
        # expectation that cannot notice its own expiry is prose.
        build = obscura_version(args.obscura_binary)
        pinned_build = check(
            "the obscura this run used is the one the F2 pin is dated to",
            build == PIN_OBSCURA_BUILD,
            f"running {build or 'unreadable'}, pinned to {PIN_OBSCURA_BUILD} "
            f"(measured {PIN_MEASURED_AT})",
        )

        # Two directions, two sentences. Together they are equality; apart they
        # say WHICH way the world moved, and the closing direction has to be as
        # loud as the growing one or the pin outlives the fact (判据 §5).
        no_growth = only_chromium <= PIN_CHROMIUM_ONLY and not only_obscura
        grew = check(
            "the known engine gap has not GROWN or changed shape",
            no_growth,
            f"within the pin: [{describe(only_chromium)}]"
            if no_growth
            else f"pinned chromium-only [{describe(PIN_CHROMIUM_ONLY)}]; observed "
            f"chromium-only [{describe(only_chromium)}], obscura-only "
            f"[{describe(only_obscura)}] — a key here that the pin does not name is "
            f"a NEW divergence, and the pin does not cover it",
        )
        # The detail is built from the OUTCOME. A green line reading "THE PIN HAS
        # EXPIRED" is the wrong label, and a wrong label costs more than a
        # missing one (判据 §17) — which is the same rule the `check` above this
        # one had to learn too.
        still_open = PIN_CHROMIUM_ONLY <= only_chromium
        closed = check(
            f"the F2 pin still describes the world (obscura {PIN_OBSCURA_BUILD}, "
            f"measured {PIN_MEASURED_AT})",
            still_open,
            f"as pinned: [{describe(PIN_CHROMIUM_ONLY)}]"
            if still_open
            else f"pinned [{describe(PIN_CHROMIUM_ONLY)}] but observed "
            f"[{describe(only_chromium)}]. THE PIN HAS EXPIRED, GO UPDATE IT — this "
            f"is not a breakage: obscura now offers something it did not. Delete the "
            f"matching entry from PIN_CHROMIUM_ONLY, and if it empties, delete the pin, "
            f"browser_snapshot's DESCRIPTION clause about JS-listener clickables, and "
            f"the narrowed interchangeability sentence in FL §3.12 with it",
        )
        same = grew and closed and pinned_build
        # Printed whatever the verdict: the comparison's INPUT is the thing a
        # reader needs in order to disagree with it, and a green run that shows
        # nothing is a number without its predicate (判据 §18).
        log(f"  obscura   ({sum(obscura_keys.values())} refs): {describe(obscura_keys)}")
        log(f"  chromium  ({sum(chromium_keys.values())} refs): {describe(chromium_keys)}")
        if not same:
            # A key like `generic ''` names the SHAPE of the disagreement and
            # not the element, and an operator cannot act on that. The trees are
            # ~20 lines each on this fixture; printing them both is what turns
            # "one extra generic" into "which node, at what depth, under what
            # parent". Only on a failure — a green run has nothing to locate.
            log("\n  --- obscura tree ---")
            for line in obscura_tree.splitlines():
                log(f"  | {line}")
            log("\n  --- chromium tree ---")
            for line in chromium_tree.splitlines():
                log(f"  | {line}")

    return led.verdict()


sys.exit(asyncio.run(main()))
