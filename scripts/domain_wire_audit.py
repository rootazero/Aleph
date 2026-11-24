#!/usr/bin/env python3
"""Domain wire-seam guard - invariants the 2026-11-21 src/domain audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo root:

    python3 scripts/domain_wire_audit.py

Exit non-zero when any check fails.

Three kinds of check live here:

* CUT      - a severed wire that was removed must stay removed, or the cut is
             undone by a later edit and the dead surface grows back.
* LIVE     - a reconnected wire must stay connected: the producer, the
             consumer and the call that joins them.
* RESERVED - a deliberately dark wire (a documented duplicate that waits for a
             product ruling) must stay as it is, so a later audit does not
             "fix" it into a lie - and so a NEW copy cannot hide behind it.
"""
import re
import sys
from pathlib import Path

try:  # the repo's brace-matching test stripper is more precise than a cut
    from wiring_strip import strip_tests as _repo_strip_tests
except ImportError:  # pragma: no cover - standalone use
    _repo_strip_tests = None

ROOT = Path(__file__).resolve().parent.parent
BAD = []

# ---------------------------------------------------------------------------
# Vocabulary tables
# ---------------------------------------------------------------------------

# The domain vocabularies that live in src/domain/skill.rs. A hand-rolled
# `match` over these tokens is a second copy of the vocabulary: it can accept
# a different token set, a different case policy or a different failure
# policy than the domain parser, and the two copies then disagree.
VOCABULARIES = {
    "prompt-scope": ["system", "tool", "standalone", "disabled"],
    "os": ["darwin", "linux", "windows", "macos", "win"],
    "install-kind": ["brew", "apt", "scoop", "winget", "npm", "uv", "go", "download"],
}

# Files scanned for hand-rolled vocabulary matches: the skill subsystem plus
# the three known outside consumers of the same vocabulary.
SCAN_FILES = [
    "src/gateway/handlers/skills.rs",
    "src/extension/manifest/parsers.rs",
    "src/builtin_tools/skill_manage.rs",
]

# Hand-rolled copies that still exist and are pinned until a product ruling
# (audit report M1-03 / M1-04) unifies them with the domain parsers. Each
# entry pins the exact token set, so a NEW copy - or a changed copy - fails.
KNOWN_PENDING = {
    "src/skill/manifest.rs": {
        # M1-03: silent `PromptScope::Disabled` on an unknown scope.
        "prompt-scope": {"system", "tool", "standalone", "disabled"},
        # M1-04: unknown install kind -> warn and drop the whole spec.
        "install-kind": {"brew", "apt", "scoop", "winget", "npm", "uv", "go", "download"},
    },
    "src/gateway/handlers/skills.rs": {
        # M1-03: RPC `skills.set` answers INVALID_PARAMS on an unknown scope.
        "prompt-scope": {"system", "tool", "standalone", "disabled"},
    },
    "src/extension/manifest/parsers.rs": {
        # M1-03: the extension's own PromptScope, parsed case-sensitively.
        "prompt-scope": {"system", "tool", "standalone", "disabled"},
    },
    "src/builtin_tools/skill_manage.rs": {
        # M1-03 (fourth copy, found by the 2026-11-21 audit): the tool
        # answers `Invalid scope: {other}`.
        "prompt-scope": {"system", "tool", "standalone", "disabled"},
    },
}


def read(p):
    return (ROOT / p).read_text(encoding="utf-8")


def prod(p):
    """Production code only: every `#[cfg(test)]` item removed."""
    text = read(p)
    if _repo_strip_tests is not None:
        return _repo_strip_tests(text)
    return text.split("#[cfg(test)]")[0]


def bad(check, detail):
    BAD.append(f"{check}: {detail}")


def code_only(text):
    """Text with the line comments dropped (doc comments included)."""
    return "\n".join(
        line for line in text.splitlines() if not line.lstrip().startswith("//")
    )


def absent(check, path, needle, note):
    """A severed wire must stay severed."""
    text = read(path)
    if needle in text:
        bad(check, f"{path} still carries {needle!r} - {note}")


def absent_code(check, path, needle, note):
    """A severed wire must stay severed (comments are not code)."""
    text = code_only(read(path))
    if needle in text:
        bad(check, f"{path} still carries {needle!r} - {note}")


def present(check, path, needle, note):
    """A reconnected wire must stay connected."""
    text = read(path)
    if needle not in text:
        bad(check, f"{path} no longer carries {needle!r} - {note}")


def block(text, header):
    """The `{ ... }` block that follows `header` (brace matched)."""
    i = text.find(header)
    if i == -1:
        return None
    start = text.find("{", i)
    if start == -1:
        return None
    depth = 0
    for j in range(start, len(text)):
        if text[j] == "{":
            depth += 1
        elif text[j] == "}":
            depth -= 1
            if depth == 0:
                return text[start : j + 1]
    return None


# ---------------------------------------------------------------------------
# CUT: the severed wires stay severed
# ---------------------------------------------------------------------------


def check_entity_cut():
    """`Entity` had three impls and zero generic-bound consumers - the only
    call was a UFCS call an inherent `SkillManifest::id()` already served.
    A marker trait may only exist if some generic bound actually uses it."""
    absent("domain/cut", "src/domain/mod.rs", "trait Entity",
           "the Entity marker trait had no generic-bound consumer")
    absent("domain/cut", "src/domain/skill.rs", "impl Entity for",
           "SkillManifest no longer implements the removed Entity trait")
    absent("domain/cut", "src/domain/skill.rs", "use super::Entity",
           "the Entity import is dead with the trait")
    absent("domain/cut", "src/memory/context/fact.rs", "crate::domain::Entity",
           "MemoryFact no longer implements the removed Entity trait")
    absent("domain/cut", "src/a2a/domain/task.rs", "crate::domain::Entity",
           "A2ATask no longer implements the removed Entity trait")
    absent("domain/cut", "src/skill/manifest.rs", "crate::domain::Entity::",
           "the UFCS call is served by the inherent SkillManifest::id()")


def check_prompt_scope_cut():
    """`PromptScope::as_str` and its `Display` impl had no consumer: the
    value only ever travelled through serde and `match` comparisons."""
    absent("domain/cut", "src/domain/skill.rs", "impl fmt::Display for PromptScope",
           "no code formats a PromptScope value")
    scope_block = block(prod("src/domain/skill.rs"), "impl PromptScope {")
    if scope_block is not None and "as_str" in scope_block:
        bad("domain/cut", "src/domain/skill.rs PromptScope::as_str had no consumer")


def check_extension_parser_cut():
    """The extension's tolerant PromptScope parser had no caller: the live
    parse site is the strict match in extension/manifest/parsers.rs."""
    absent("domain/cut", "src/extension/types/hooks.rs", "from_str_or_default",
           "the tolerant PromptScope parser had no caller")
    absent("domain/cut", "src/extension/types/hooks.rs", "impl std::str::FromStr for PromptScope",
           "the extension PromptScope FromStr impl had no caller")


def check_id_newtype_cut():
    """`Borrow<str>` / `AsRef<str>` / `PartialEq<str>` / `PartialEq<&str>` on
    the id newtypes had no consumer: the registry looks up by `&SkillId`."""
    for ty in ("SkillId", "PluginId"):
        for head in ("impl std::borrow::Borrow<str> for",
                     "impl AsRef<str> for",
                     "impl PartialEq<str> for",
                     "impl PartialEq<&str> for"):
            absent("domain/cut", "src/domain/skill.rs", f"{head} {ty}",
                   f"{ty} has no consumer for this trait")


def check_aleph_spec_cut():
    """`From<&AlephSkillSpec> for SkillManifest` was a lossy bridge whose
    only caller was its own test: it dropped `requires.bins` and the
    execution/security metadata and hardcoded `SkillSource::Global`."""
    absent("domain/cut", "src/tools/markdown_skill/spec.rs",
           "impl From<&AlephSkillSpec> for",
           "the lossy AlephSkillSpec -> SkillManifest bridge had no production caller")


# ---------------------------------------------------------------------------
# LIVE: the reconnections stay connected
# ---------------------------------------------------------------------------


def check_prompt_scope_parser():
    """One vocabulary, one case policy. The serde path and the RPC path must
    share the domain `FromStr` instead of two parsers with two policies."""
    present("domain/live", "src/domain/skill.rs", "impl FromStr for PromptScope",
            "the domain PromptScope parser is the single vocabulary")
    present("domain/live", "src/domain/skill.rs", "impl<'de> Deserialize<'de> for PromptScope",
            "the serde path must reuse the domain parser")
    present("domain/live", "src/domain/skill.rs", "impl FromStr for Os",
            "the domain OS parser is the single OS vocabulary")
    present("domain/live", "src/domain/skill.rs", "impl FromStr for InstallKind",
            "the domain install-kind parser is the single install-kind vocabulary")

    # The enum must not derive Deserialize as well - two impls, one type.
    domain = read("src/domain/skill.rs")
    enum_at = domain.find("pub enum PromptScope")
    if enum_at == -1:
        bad("domain/live", "src/domain/skill.rs no longer defines PromptScope")
        return
    derive = domain[max(0, enum_at - 200) : enum_at]
    if "Deserialize" in derive.rsplit("#[derive", 1)[-1]:
        bad("domain/live", "PromptScope must not derive Deserialize - the manual impl is the serde path")


def check_os_parse():
    """An unparseable OS token must never be dropped silently: `Some([])` was
    read downstream as "reject every current OS"."""
    present("domain/live", "src/skill/manifest.rs", "parse_os_list",
            "the OS list parser must report unknown tokens")
    present("domain/live", "src/skill/manifest.rs", "token.parse::<Os>()",
            "OS tokens must go through the domain OS parser")
    absent_code("domain/live", "src/skill/manifest.rs",
                "filter_map(|s| s.parse::<Os>().ok())",
                "the lossy OS parse turns an unknown token into 'reject every OS'")


def check_prompt_budget():
    """The snapshot hands the budget to the prompt layer, so it must carry
    the CONFIGURED budget - not the default, overwritten afterwards."""
    present("domain/live", "src/skill/snapshot.rs", "prompt_budget: SkillPromptBudget",
            "SkillSnapshot::build must take the configured budget")
    snap = read("src/skill/snapshot.rs")
    build_block = block(snap, "pub fn build(")
    if build_block is None:
        bad("domain/live", "src/skill/snapshot.rs no longer defines SkillSnapshot::build")
    elif "SkillPromptBudget::default()" in build_block:
        bad("domain/live", "SkillSnapshot::build must not hardcode the default budget")
    absent("domain/live", "src/skill/mod.rs", "new_snapshot.prompt_budget =",
           "the caller must pass the budget in, not overwrite it afterwards")


def check_vocabulary_single_parser():
    """Single vocabulary parser: no hand-rolled `match` over the domain
    vocabulary tokens, except the copies pinned in KNOWN_PENDING."""
    files = list(SCAN_FILES)
    files += sorted(str(p.relative_to(ROOT)) for p in (ROOT / "src/skill").rglob("*.rs"))
    files += sorted(str(p.relative_to(ROOT)) for p in (ROOT / "src/domain").rglob("*.rs"))

    for rel in files:
        text = code_only(prod(rel))
        for vocab, tokens in VOCABULARIES.items():
            found = set()
            for token in tokens:
                pattern = re.compile(r'"' + re.escape(token) + r'"\s*=>')
                if pattern.search(text):
                    found.add(token)
            if not found:
                continue
            pinned = KNOWN_PENDING.get(rel, {}).get(vocab)
            if pinned is None:
                bad("domain/vocab", f"{rel} hand-rolls the {vocab} vocabulary ({sorted(found)}) - "
                                    "the domain parser in src/domain/skill.rs is the single vocabulary")
            elif found != pinned:
                bad("domain/vocab", f"{rel} changed the pinned {vocab} copy: {sorted(found)} "
                                    f"(pinned {sorted(pinned)}) - the pending ruling must be recorded "
                                    "before the shape changes")


def check_priority_order():
    """The source-priority order has one source of truth: the numbers in
    `SkillSource::priority`. The registry doc must not tell a different
    story."""
    domain = read("src/domain/skill.rs")
    impl_at = domain.find("impl SkillSource {")
    if impl_at == -1:
        bad("domain/order", "src/domain/skill.rs no longer defines `impl SkillSource`")
        return
    prio = block(domain[impl_at:], "pub const fn priority(&self) -> u8")
    if prio is None:
        bad("domain/order", "src/domain/skill.rs no longer defines SkillSource::priority")
        return
    levels = {}
    for m in re.finditer(r"Self::(\w+)(?:\(_\))?(?:\s*\|\s*Self::(\w+)(?:\(_\))?)?\s*=>\s*(\d+)",
                         prio):
        for name in (m.group(1), m.group(2)):
            if name:
                levels[name.lower()] = int(m.group(3))
    if not levels:
        bad("domain/order", "src/domain/skill.rs SkillSource::priority has no readable arms")
        return

    registry = read("src/skill/registry.rs")
    chains = re.findall(r"`([^`]* > [^`]*)`", registry)
    if not chains:
        bad("domain/order", "src/skill/registry.rs no longer documents the priority order")
        return
    chain = chains[0]
    groups = [[n.strip().lower() for n in part.split("=")] for part in chain.split(">")]

    documented = set()
    for group in groups:
        for name in group:
            documented.add(name)
            if name not in levels:
                bad("domain/order", f"src/skill/registry.rs documents {name!r}, which has no priority")
    for name in levels:
        if name not in documented:
            bad("domain/order", f"src/domain/skill.rs defines a priority for {name!r}, "
                                "which src/skill/registry.rs does not document")

    prev = None
    for group in groups:
        group_levels = {levels.get(name) for name in group}
        if None in group_levels:
            continue
        if len(group_levels) > 1:
            bad("domain/order", f"src/skill/registry.rs ties {group} but the domain priorities differ: "
                                f"{ {name: levels[name] for name in group} }")
        level = group_levels.pop()
        if prev is not None and level >= prev:
            bad("domain/order", f"src/skill/registry.rs documents {chain!r} highest-first, but "
                                f"{group} has priority {level} (previous group {prev})")
        prev = level


def main():
    check_entity_cut()
    check_prompt_scope_cut()
    check_extension_parser_cut()
    check_id_newtype_cut()
    check_aleph_spec_cut()
    check_prompt_scope_parser()
    check_os_parse()
    check_prompt_budget()
    check_vocabulary_single_parser()
    check_priority_order()
    if BAD:
        print(f"FAIL: {len(BAD)} wire invariant(s) broken")
        for item in BAD:
            print(f"  - {item}")
        return 1
    print("PASS: domain wire invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
