#!/usr/bin/env python3
"""Build the three screens `qa/terminal/fake-claude` paints, FROM the shipped
manifest — never by hand-copying its strings into a script.

The fake agent's whole job is to make `crates/agent-detect/src/manifests/
claude.toml` fire. A fixture that hard-codes `esc to interrupt` next to a
manifest that owns that literal is the same fact written twice (判据 §1): the
day the manifest's wording moves, the fixture keeps painting the old chrome,
no rule matches, `state` stays `unknown` — and the stage reads as "detection is
broken" rather than "the fixture is stale".

So the literals are EXTRACTED from the rules by id:

    live_prompt_box    (idle,    region prompt_box_body)
    live_turn_working  (working, region bottom_non_empty_lines(12))
    live_blocked_form  (blocked, region after_last_horizontal_rule)

`contains` literals are taken as they are written; `line_regex` literals are
recovered by walking the pattern and keeping its maximal literal runs (the
`.*`/`\\s*`/character-class parts are skipped, so `^\\s*[⏸⏵].*esc to interrupt…`
yields the class's first glyph and the run `esc to interrupt`). Each built
line is then CHECKED against the manifest's own pattern with `re` — a rule
whose shape this script cannot satisfy any more is a loud failure here, at
generation time, instead of a mysterious `unknown` forty seconds into a stage.

What this script deliberately does NOT do is decide which rule *wins*. That
would mean reimplementing the region extractor and the priority walk in
Python — a second engine, which is the very shape §1 warns about, and one that
would be wrong in a different way from the real one. The winner is asserted at
RUNTIME instead, by `terminal{explain}`, which runs the shipped engine and
names the rule it matched. This script only guarantees the screens are the
ones those three rules describe.

Two outputs, written from one in-memory dict in one run so they cannot drift:

    chrome.json  — for the python driver (rule ids, states, screen text)
    chrome.env   — for the bash fake (the same screens as shell strings)

Usage:  derive_chrome.py <claude.toml> <out-dir>
"""
import json
import re
import sys
import tomllib

# Metacharacters that end a literal run. `-` and `,` are NOT here: they are
# literal outside a character class, and dropping them would silently shorten
# a run that a future rule depends on.
META = set(r"^$.|?*+()[]{}")

# A quantifier binds only the character before it, so `abc*` contributes the
# run `ab`, not `abc`. Recorded here rather than special-cased below because
# it is the one place this walker can be wrong in a way that still *looks*
# like a plausible literal.
QUANTIFIERS = set("*+?")


def rust_regex_to_python(pattern: str) -> str:
    """`\\x{2800}` is Rust-regex spelling; Python wants `\\uXXXX`/`\\UXXXXXXXX`."""

    def repl(m: re.Match) -> str:
        code = int(m.group(1), 16)
        return f"\\u{code:04x}" if code <= 0xFFFF else f"\\U{code:08x}"

    return re.sub(r"\\x\{([0-9A-Fa-f]+)\}", repl, pattern)


def _skip_escape(pattern: str, i: int) -> int:
    """Index just past the escape sequence starting at `pattern[i] == '\\'`."""
    if i + 1 >= len(pattern):
        return i + 1
    if pattern[i + 1] == "x" and i + 2 < len(pattern) and pattern[i + 2] == "{":
        end = pattern.find("}", i + 2)
        if end != -1:
            return end + 1
    return i + 2


def literal_runs(pattern: str) -> list[str]:
    """The maximal runs of literal text in a regex, longest first.

    Escapes, groups and character classes end a run; a trailing quantifier
    takes its own character with it.
    """
    runs: list[str] = []
    cur: list[str] = []
    i = 0

    def flush(drop_last: bool = False) -> None:
        nonlocal cur
        if drop_last and cur:
            cur.pop()
        if cur:
            runs.append("".join(cur))
        cur = []

    while i < len(pattern):
        ch = pattern[i]
        if ch == "\\":
            flush()
            i = _skip_escape(pattern, i)
            continue
        if ch == "[":
            flush()
            i = _skip_class(pattern, i)
            continue
        if ch in QUANTIFIERS:
            flush(drop_last=True)
            i += 1
            continue
        if ch in META:
            flush()
            i += 1
            continue
        cur.append(ch)
        i += 1
    flush()
    return sorted((r for r in runs if r.strip()), key=len, reverse=True)


def _skip_class(pattern: str, i: int) -> int:
    """Index just past the character class starting at `pattern[i] == '['`."""
    j = i + 1
    while j < len(pattern) and pattern[j] != "]":
        j = _skip_escape(pattern, j) if pattern[j] == "\\" else j + 1
    return j + 1


def first_class_glyph(pattern: str) -> str:
    """The first alternative of the pattern's first character class.

    `[⏸⏵]` -> `⏸`. Used for the working rule's spinner glyph, which is a set
    of equivalent choices rather than a literal.
    """
    start = pattern.find("[")
    if start == -1:
        die(f"no character class in {pattern!r}")
    end = _skip_class(pattern, start)
    body = pattern[start + 1 : end - 1]
    if body.startswith("^"):
        die(f"negated character class is not a source of glyphs: {pattern!r}")
    if body.startswith("\\"):
        after = _skip_escape(body, 0)
        escaped = body[:after]
        expanded = re.sub(
            r"\\x\{([0-9A-Fa-f]+)\}", lambda m: chr(int(m.group(1), 16)), escaped
        )
        if len(expanded) == 1:
            return expanded
        die(f"cannot expand {escaped!r} to one glyph")
    if not body:
        die(f"empty character class in {pattern!r}")
    return body[0]


def die(msg: str) -> None:
    print(f"derive_chrome: {msg}", file=sys.stderr)
    raise SystemExit(1)


def rule_by_id(manifest: dict, rule_id: str) -> dict:
    for rule in manifest.get("rules", []):
        if rule.get("id") == rule_id:
            return rule
    die(
        f"rule {rule_id!r} is gone from the manifest. The fixture's screens are "
        f"derived from it; pick the rule that replaced it rather than pasting "
        f"its old text back into fake-claude."
    )
    raise AssertionError("unreachable")


def expect_region(rule: dict, want: str) -> None:
    got = rule.get("region")
    if got != want:
        die(
            f"rule {rule['id']!r} now reads region {got!r}, not {want!r}. The "
            f"screen this script builds is shaped for {want!r} (horizontal "
            f"rules, prompt box), so it would no longer be shown to the rule."
        )


def check_line(pattern: str, line: str, rule_id: str) -> None:
    """The built line must satisfy the manifest's own pattern."""
    compiled = re.compile(rust_regex_to_python(pattern))
    if not compiled.search(line):
        die(
            f"the line built for {rule_id!r} does not match its own pattern.\n"
            f"  pattern: {pattern}\n  line:    {line!r}"
        )


def check_not_gates(rule: dict, region_text: str) -> None:
    """None of the rule's `not` clauses may match the region we built."""
    lowered = region_text.lower()
    for gate in rule.get("not", []):
        needles = gate.get("contains", [])
        if needles and all(n.lower() in lowered for n in needles):
            die(f"rule {rule['id']!r}'s `not` clause {needles!r} matches the built screen")
        for pattern in gate.get("regex", []) + gate.get("line_regex", []):
            if re.compile(rust_regex_to_python(pattern)).search(region_text):
                die(f"rule {rule['id']!r}'s `not` pattern {pattern!r} matches the built screen")


# `is_horizontal_rule` (crates/agent-detect/src/manifest.rs) accepts a line of
# `─` when the run is at least 3 long. 40 is comfortably inside a 100-column
# QA terminal, so the rule never wraps into two lines — a wrapped rule is not
# a rule, and the region would silently become the whole screen.
HRULE = "─" * 40


def build(manifest_path: str) -> dict:
    with open(manifest_path, "rb") as fh:
        manifest = tomllib.load(fh)

    agent_id = manifest.get("id")
    if agent_id != "claude":
        die(f"expected the claude manifest, got id={agent_id!r}")

    # --- idle: a prompt box whose body carries the prompt glyph -------------
    idle_rule = rule_by_id(manifest, "live_prompt_box")
    expect_region(idle_rule, "prompt_box_body")
    idle_patterns = idle_rule.get("line_regex", [])
    if not idle_patterns:
        die("live_prompt_box no longer carries a line_regex to derive the prompt glyph from")
    idle_runs = literal_runs(idle_patterns[0])
    if not idle_runs:
        die(f"no literal to build a prompt line from in {idle_patterns[0]!r}")
    idle_body = f" {idle_runs[0]} "
    check_line(idle_patterns[0], idle_body, idle_rule["id"])
    check_not_gates(idle_rule, idle_body)
    # `prompt_box_top_border_index` walks up from the bottom and wants the
    # SECOND rule it meets, so the box needs both borders.
    idle_screen = "\n".join([HRULE, idle_body, HRULE])

    # --- working: the live-turn spinner line --------------------------------
    working_rule = rule_by_id(manifest, "live_turn_working")
    expect_region(working_rule, "bottom_non_empty_lines(12)")
    branches = [b for b in working_rule.get("any", []) if b.get("line_regex")]
    if not branches:
        die("live_turn_working no longer has an `any` branch with a line_regex")
    working_pattern = branches[0]["line_regex"][0]
    working_runs = literal_runs(working_pattern)
    if not working_runs:
        die(f"no literal to build a working line from in {working_pattern!r}")
    glyph = first_class_glyph(working_pattern)
    working_line = f"{glyph} qa fixture {working_runs[0]}"
    check_line(working_pattern, working_line, working_rule["id"])
    check_not_gates(working_rule, working_line)
    working_screen = working_line

    # --- blocked: a confirmation form below a horizontal rule ---------------
    blocked_rule = rule_by_id(manifest, "live_blocked_form")
    expect_region(blocked_rule, "after_last_horizontal_rule")
    blocked_needles = list(blocked_rule.get("contains", []))
    if not blocked_needles:
        die("live_blocked_form no longer carries `contains` literals")
    any_branch = next(
        (b for b in blocked_rule.get("any", []) if b.get("contains") and not b.get("any")),
        None,
    )
    if any_branch is None:
        die("live_blocked_form has no simple `any` branch to satisfy")
    blocked_body = " · ".join(any_branch["contains"] + blocked_needles)
    check_not_gates(blocked_rule, blocked_body)
    lowered = blocked_body.lower()
    missing = [n for n in blocked_needles + any_branch["contains"] if n.lower() not in lowered]
    if missing:
        die(f"built blocked line is missing its own literals: {missing!r}")
    blocked_screen = "\n".join([HRULE, blocked_body])

    for name, screen in (
        ("idle", idle_screen),
        ("working", working_screen),
        ("blocked", blocked_screen),
    ):
        if "'" in screen:
            die(f"the {name} screen contains a single quote; chrome.env quotes with '...'")

    return {
        "manifest": manifest_path,
        "agent_id": agent_id,
        "manifest_version": manifest.get("version"),
        "screens": {
            "idle": {
                "rule": idle_rule["id"],
                "state": idle_rule["state"],
                "region": idle_rule["region"],
                "priority": idle_rule["priority"],
                "text": idle_screen,
            },
            "working": {
                "rule": working_rule["id"],
                "state": working_rule["state"],
                "region": working_rule["region"],
                "priority": working_rule["priority"],
                "text": working_screen,
            },
            "blocked": {
                "rule": blocked_rule["id"],
                "state": blocked_rule["state"],
                "region": blocked_rule["region"],
                "priority": blocked_rule["priority"],
                "text": blocked_screen,
            },
        },
    }


def main() -> None:
    if len(sys.argv) != 3:
        die("usage: derive_chrome.py <claude.toml> <out-dir>")
    manifest_path, out_dir = sys.argv[1], sys.argv[2]
    built = build(manifest_path)

    with open(f"{out_dir}/chrome.json", "w") as fh:
        json.dump(built, fh, indent=2, ensure_ascii=False)
        fh.write("\n")

    # Rendered from the SAME dict in the same run: two files, one derivation.
    with open(f"{out_dir}/chrome.env", "w") as fh:
        fh.write("# generated by qa/terminal/derive_chrome.py -- do not edit\n")
        fh.write(f"# from {manifest_path} (version {built['manifest_version']})\n")
        for name, screen in built["screens"].items():
            fh.write(f"# {screen['rule']} / {screen['state']} / {screen['region']}\n")
            fh.write(f"QA_CLAUDE_{name.upper()}='{screen['text']}'\n")

    for name, screen in built["screens"].items():
        print(f"  {name:8} <- {screen['rule']} (priority {screen['priority']})")
    print(f"  manifest version {built['manifest_version']}")


if __name__ == "__main__":
    main()
