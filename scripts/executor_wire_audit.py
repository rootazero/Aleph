#!/usr/bin/env python3
"""Executor wire-seam guard - invariants the 2026-11-21 src/executor audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo root:

    python3 scripts/executor_wire_audit.py

Exit non-zero when any check fails.

Three kinds of check live here:

* CUT   - a severed wire that was removed must stay removed, or the cut is
           undone by a later edit and the dead surface grows back.
* LIVE  - a reconnected wire must stay connected: the producer, the consumer
           and the call that joins them.
* DARK  - a deliberately dark wire (a duplicate that waits for a product
           ruling) must stay as it is, so a later audit does not "fix" it into
           a lie - and so a NEW copy cannot hide behind it.
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
    if needle in read(path):
        bad(check, f"{path} still carries {needle!r} - {note}")


def absent_prod(check, path, needle, note):
    """A severed wire must stay severed (test code may keep a local helper)."""
    if needle in prod(path):
        bad(check, f"{path} still carries {needle!r} in production code - {note}")


def present(check, path, needle, note):
    """A reconnected wire must stay connected."""
    if needle not in read(path):
        bad(check, f"{path} no longer carries {needle!r} - {note}")


def present_prod(check, path, needle, note):
    """A reconnected wire must stay connected in production code."""
    if needle not in prod(path):
        bad(check, f"{path} no longer carries {needle!r} in production code - {note}")


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


def block_prod(path, header):
    """The `{ ... }` block that follows `header` in production code."""
    return block(prod(path), header)


def slice_table(text, header):
    """The `&[ ... ]` body that follows `header` (bracket matched).

    The type annotation `&[(&str, &str)]` also opens with `&[`, so the
    search starts at the `= &[` that assigns the table."""
    i = text.find(header)
    if i == -1:
        return None
    start = text.find("= &[", i)
    if start == -1:
        return None
    start += 3
    depth = 0
    for j in range(start, len(text)):
        if text[j] == "[":
            depth += 1
        elif text[j] == "]":
            depth -= 1
            if depth == 0:
                return text[start : j + 1]
    return None


def table_names(text, header):
    """The tool names in a `&[("name", ...), ...]` table, comments dropped.

    The tables use both one-line and multi-line tuple entries, and their
    comments quote names that are NOT entries - so the comment lines go
    before the names are read."""
    body = slice_table(text, header)
    if body is None:
        return None
    return re.findall(r'\(\s*"([a-z0-9_]+)"', code_only(body))


def name_list(text, header):
    """The tool names in a plain `&["name", ...]` list, comments dropped."""
    body = slice_table(text, header)
    if body is None:
        return None
    return re.findall(r'"([a-z0-9_]+)"', code_only(body))


# ---------------------------------------------------------------------------
# CUT: the severed wires stay severed
# ---------------------------------------------------------------------------


def check_config_knobs_cut():
    """`BuiltinToolConfig` carried three knobs nobody read.

    `embedder`, `orientation` and `profile_synthesizer` were never set by
    any construction site and never read by any consumer: the memory runtime
    owns all three, and the registry builds the note/profile tools from
    `memory_runtime`. They were three inert copies of facts the runtime
    already held."""
    body = block_prod("src/executor/builtin_registry/config.rs", "pub struct BuiltinToolConfig {")
    if body is None:
        bad("executor/cut", "src/executor/builtin_registry/config.rs no longer defines "
                            "BuiltinToolConfig")
        return
    for field in ("embedder:", "orientation:", "profile_synthesizer:"):
        if field in body:
            bad("executor/cut", f"BuiltinToolConfig still declares {field!r} - the knob has no "
                                f"producer and no reader")


def check_memory_provider_cut():
    """The registry's memory-context cell had no caller.

    `memory_context_provider` / `memory_context_provider_cell()` existed so
    the `remember` tool could resolve the per-agent store, but the tool holds
    the `CuratedMemoryStore` directly and the cell was never set."""
    for path, needle in (
        ("src/executor/builtin_registry/registry/struct_def.rs", "memory_context_provider"),
        ("src/executor/builtin_registry/registry/inherent.rs", "memory_context_provider_cell"),
    ):
        absent_prod("executor/cut", path, needle,
                    "the deferred memory-context cell had no caller and no writer")


def check_reflector_setters_cut():
    """The registry's reflector setters had no production caller.

    `set_memory_reflector` / `set_query_filer` existed for a server-builder
    step that never happened: production builds the `memory_reflect` tool
    per call from the Memory runtime, so the registry-held instance could
    only ever be an unwired placeholder."""
    absent_prod("executor/cut", "src/executor/builtin_registry/registry/inherent.rs",
                "fn set_memory_reflector",
                "the registry-held reflector had no production injector")
    absent_prod("executor/cut", "src/executor/builtin_registry/registry/inherent.rs",
                "fn set_query_filer",
                "the registry-held query filer had no production injector")
    absent_prod("executor/cut", "src/executor/builtin_registry/registry/struct_def.rs",
                "memory_reflect_tool:",
                "the registry no longer holds a memory_reflect instance")


# ---------------------------------------------------------------------------
# LIVE: the reconnected wires stay connected
# ---------------------------------------------------------------------------


def check_description_table_live():
    """Every registry-only tool has a description, and it is ONE table.

    The Panel's tools view looked descriptions up in the catalog only, so the
    21 tools the registry registers outside the catalog reached it with an
    empty description. The fix moved the lookup to one table that both sides
    read - a second table would restore the split."""
    names = name_list(prod("src/executor/mod.rs"),
                      "pub const REGISTRY_ONLY_TOOL_NAMES: &[&str] = &[")
    if names is None:
        bad("executor/live", "src/executor/mod.rs no longer declares REGISTRY_ONLY_TOOL_NAMES")
        return
    described = table_names(
        prod("src/executor/builtin_registry/definitions.rs"),
        "pub(crate) const REGISTRY_ONLY_DESCRIPTIONS: &[(&str, &str)] = &[",
    )
    if described is None:
        bad("executor/live", "src/executor/builtin_registry/definitions.rs no longer declares "
                             "REGISTRY_ONLY_DESCRIPTIONS")
        return
    missing = sorted(set(names) - set(described))
    if missing:
        bad("executor/live", f"tools registered outside the catalog have no description: {missing}")
    extra = sorted(set(described) - set(names))
    if extra:
        bad("executor/live", f"REGISTRY_ONLY_DESCRIPTIONS names tools the registry-only list does "
                             f"not: {extra}")
    for tool in ("team_task_control", "task_exit_journal"):
        if tool not in described:
            bad("executor/live", f"{tool} is registered but has no description - the Panel would "
                                  f"list it with an empty description")


def check_description_lookup_live():
    """One lookup, one owner.

    `builtin_tool_description` is the single place that answers "what does
    this tool say about itself"; the Panel's schema builder must use it."""
    present_prod("executor/live", "src/executor/mod.rs", "fn builtin_tool_description",
                 "the shared description lookup must stay")
    present("executor/live", "src/gateway/handlers/agents.rs", "builtin_tool_description",
            "the tools schema must read the shared description lookup, not the catalog alone")


def check_team_tool_registration_live():
    """The two team tools register under a LITERAL name.

    The source census can only see a `tools.insert("name", ...)` literal; a
    `td.name.clone()` insert is invisible to it, so the tool was advertised
    to nobody and dropped from every skill's allowed-tools list."""
    team = read("src/executor/builtin_registry/builder/constructor/coord_team_tools.rs")
    for tool in ("team_task_control", "task_exit_journal"):
        if f'tools.insert("{tool}".to_string()' not in team:
            bad("executor/live", f"{tool} is no longer registered under a literal name - the "
                                  f"source census cannot see it")


def check_diagnostics_group_live():
    """The diagnostics tool is in a tool category.

    `TOOL_CATEGORIES` is the Policies screen's only source, so a tool
    missing from it cannot be listed at all."""
    groups = prod("src/executor/builtin_registry/groups.rs")
    if '"capability_projection_diagnostics"' not in groups:
        bad("executor/live", "capability_projection_diagnostics is in no tool category - the "
                              "Policies screen cannot list it")


def check_skill_list_filter_live():
    """The skill_list schema declares its `filter` argument.

    The handler reads `filter`, but the hand-written schema did not name it,
    so the model could not pass it."""
    present("executor/live", "src/executor/builtin_registry/builder/core_tools.rs",
            'tool_schema::<crate::builtin_tools::skill_reader::ListSkillsArgs>("skill_list")',
            "the skill_list schema must be derived from the handler's own argument type")


def check_diagnostics_schema_live():
    """The diagnostics schema names the operation bodies.

    The handler requires `tool_name`, `plane` and `duration_ms`; the schema
    named none of them, so a call could not be expressed."""
    schema = read("src/builtin_tools/capability_projection_diagnostics.rs")
    for field in ('"tool_name"', '"plane"', '"duration_ms"'):
        if field not in schema:
            bad("executor/live", f"the capability diagnostics schema no longer declares {field}")


def check_memory_reflect_partition_live():
    """The memory_reflect arm still composes the caller's partition.

    The arm was reduced to the runtime branch, but the partition it composes
    is what makes the query filer's write land in the caller's own memory
    rather than the org partition every principal's reads union in."""
    impl = prod("src/executor/builtin_registry/registry/tool_registry_impl.rs")
    arm = block(impl, '"memory_reflect" => {')
    if arm is None:
        bad("executor/live", "src/executor/builtin_registry/registry/tool_registry_impl.rs no "
                             "longer has a memory_reflect dispatch arm")
        return
    if "caller_memory_partition" not in arm:
        bad("executor/live", "the memory_reflect arm no longer composes the caller's memory "
                              "partition - the filer would write to the org partition")


# ---------------------------------------------------------------------------
# DARK: the deliberately dark wires stay as they are
# ---------------------------------------------------------------------------


def check_create_tool_boxed_dark():
    """`create_tool_boxed` stays public until a product ruling.

    It is re-exported from the crate root, so removing it is a public API
    change, not a private cleanup. The audit classified it DECIDE."""
    present("executor/dark", "src/executor/mod.rs", "create_tool_boxed",
            "the public re-export must stay until the product ruling lands")


def main():
    check_config_knobs_cut()
    check_memory_provider_cut()
    check_reflector_setters_cut()
    check_description_table_live()
    check_description_lookup_live()
    check_team_tool_registration_live()
    check_diagnostics_group_live()
    check_skill_list_filter_live()
    check_diagnostics_schema_live()
    check_memory_reflect_partition_live()
    check_create_tool_boxed_dark()
    if BAD:
        print(f"FAIL: {len(BAD)} wire invariant(s) broken")
        for item in BAD:
            print(f"  - {item}")
        return 1
    print("PASS: executor wire invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
