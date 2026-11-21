#!/usr/bin/env python3
"""Extension wire-seam guard - invariants the 2026-11-21/22 src/extension audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo root:

    python3 scripts/extension_wire_audit.py

Exit non-zero when any check fails.

Three kinds of check live here:

* CUT   - a severed wire that was removed must stay removed.
* LIVE  - a reconnected wire must stay connected: the producer, the consumer
           and the call that joins them.
* DARK  - a deliberately dark wire must stay as it is, so a later audit does
           not "fix" it into a lie.
"""
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


def absent(check, path, needle, note):
    if needle in read(path):
        bad(check, f"{path} still contains {needle!r} - {note}")


def absent_prod(check, path, needle, note):
    if needle in prod(path):
        bad(check, f"{path} still exposes {needle!r} in production code - {note}")


# ---------------------------------------------------------------------------
# CUT: the severed wires stay cut

def check_manifest_dispatch():
    """`parse_manifest_from_dir` (async) had no production caller.

    Production resolves a manifest through the process-wide cache
    (`parse_manifest_from_dir_cached_global`) or the sync entry point."""
    absent_prod(
        "extension/cut",
        "src/extension/manifest/mod.rs",
        "pub async fn parse_manifest_from_dir",
        "the async manifest dispatcher has no production caller",
    )


def check_manifest_display_metadata():
    """PluginManifest's display metadata (author/homepage/repository/license/
    keywords) was parsed by three dialects and read by nobody."""
    for needle in ("pub author:", "pub homepage:", "pub repository:", "pub license:",
                   "pub keywords:"):
        absent("extension/cut", "src/extension/manifest/types.rs", needle,
               "the manifest display metadata has no reader")
    absent("extension/cut", "src/extension/manifest/types.rs", "struct AuthorInfo",
           "the manifest display metadata has no reader")
    absent("extension/cut", "src/extension/manifest/toml_types.rs", "struct PluginAuthorToml",
           "the manifest display metadata has no reader")


def check_dead_registry_surface():
    """The registry accessors that nothing read."""
    for needle in ("pub fn stats(", "struct RegistryStats", "pub fn get_hooks_for_event",
                   "pub fn clear_diagnostics"):
        absent("extension/cut", "src/extension/registry/plugin_registry/mod.rs", needle,
               "the accessor has no production caller")
    # `get_agent` by bare name: production resolves agents through the registrar.
    body = read("src/extension/registry/plugin_registry/mod.rs")
    idx = body.find("pub fn get_agent(")
    if idx != -1 and "#[cfg(test)]" not in body[max(0, idx - 200):idx]:
        bad("extension/cut", "src/extension/registry/plugin_registry/mod.rs exposes "
                             "get_agent() in production code - production resolves agents "
                             "through the registrar")


def check_dead_context_accessors():
    """`register_context` / `unregister_context_bundle` / `list_context_descriptors`
    had no production caller; the bundle variants are the live ones."""
    body = read("src/extension/registry/plugin_registry/context.rs")
    for needle in ("pub fn register_context(", "pub fn unregister_context_bundle(",
                   "pub fn unregister_context_bundle_for_session(",
                   "pub fn list_context_descriptors("):
        idx = body.find(needle)
        if idx == -1:
            continue
        if "#[cfg(test)]" not in body[max(0, idx - 400):idx]:
            bad("extension/cut", "src/extension/registry/plugin_registry/context.rs exposes "
                                 f"{needle} in production code - the bundle variants are live")


def check_dead_agent_types():
    """`PermissionRule`, `AgentFrontmatter` and the AgentRegistration fields that
    no producer ever filled."""
    absent("extension/cut", "src/extension/types/agents.rs", "struct PermissionRule",
           "no producer fills it")
    absent("extension/cut", "src/extension/types/agents.rs", "struct AgentFrontmatter",
           "no producer fills it")
    for needle in ("pub hidden:", "pub color:", "pub temperature:", "pub top_p:",
                   "pub permission:", "pub options:"):
        absent("extension/cut", "src/extension/registry/types.rs", needle,
               "no producer fills the field")


# ---------------------------------------------------------------------------
# LIVE: the reconnected wires stay connected

def check_plugin_agent_mode_is_rejected():
    """The plugin agent mode axis is not implemented, so a `mode:` key must fail
    loudly instead of being dropped by the deserializer."""
    body = read("src/extension/manifest/parsers.rs")
    if "agent frontmatter `mode:` is not supported for plugin agents" not in body:
        bad("extension/live", "src/extension/manifest/parsers.rs no longer rejects a plugin "
                              "agent `mode:` key - it would be silently dropped")
    if "mode: Option<crate::yaml::Value>" not in body:
        bad("extension/live", "src/extension/manifest/parsers.rs no longer keeps the "
                              "presence-only `mode` field on AgentFm")


def check_config_schema_has_one_owner():
    """The schema budget and the schema compile must have exactly one owner."""
    owner = read("src/extension/manifest/config_validation.rs")
    for needle in ("MAX_SCHEMA_DEPTH", "MAX_SCHEMA_PROPERTIES", "fn check_complexity"):
        if needle not in owner:
            bad("extension/live", "src/extension/manifest/config_validation.rs lost "
                                  f"{needle} - the budget must have one owner")
    if "jsonschema::validator_for" not in owner:
        bad("extension/live", "src/extension/manifest/config_validation.rs no longer compiles "
                              "the schema with jsonschema")
    catalog = read("src/hub/hub_catalog.rs")
    if "validate_config_schema_declaration" not in catalog:
        bad("extension/live", "src/hub/hub_catalog.rs no longer calls the shared config-schema "
                              "validator - the two owners would drift")
    for needle in ("MAX_SCHEMA_DEPTH", "MAX_SCHEMA_PROPERTIES", "fn check_complexity"):
        if needle in catalog:
            bad("extension/cut", f"src/hub/hub_catalog.rs defines its own {needle} - the "
                                 "budget must have one owner")


def check_validation_warnings_reach_the_operator():
    """`validate_plugin` warnings were dropped by both production callers."""
    marketplace = read("src/extension/marketplace/mod.rs")
    if "pub struct InstalledPlugin" not in marketplace or "warnings" not in marketplace:
        bad("extension/live", "src/extension/marketplace/mod.rs no longer returns the plugin "
                              "validation warnings - the operator would never see them")
    cli = read("src/bin/aleph-server/commands/plugins.rs")
    if "warnings" not in cli:
        bad("extension/live", "src/bin/aleph-server/commands/plugins.rs no longer reports the "
                              "plugin validation warnings")


def check_context_audit_is_read():
    """The context grant audit ring was write-only."""
    body = read("src/builtin_tools/plugin_manage.rs")
    if "context_audit(" not in body:
        bad("extension/live", "src/builtin_tools/plugin_manage.rs no longer surfaces the context "
                              "grant audit - the ring would be write-only again")


def check_cli_validates_the_config_schema():
    """`aleph plugin validate` compiled no schema, so a manifest with a broken
    config_schema passed the CLI while the server refused it."""
    body = read("interfaces/cli/src/commands/plugin_cmd.rs")
    if "config_schema" not in body:
        bad("extension/live", "interfaces/cli/src/commands/plugin_cmd.rs no longer looks at the "
                              "config_schema - the CLI would green-light what the server refuses")


# ---------------------------------------------------------------------------
# DARK: the deliberately dark wires stay dark

def check_manifest_cache_stays_documented():
    """The manifest cache is a documented behavior, not a bug to fix."""
    body = read("src/extension/manifest/mod.rs")
    if "Process-wide manifest parse cache" not in body:
        bad("extension/dark", "src/extension/manifest/mod.rs no longer documents the "
                              "process-wide manifest cache")


def main():
    check_manifest_dispatch()
    check_manifest_display_metadata()
    check_dead_registry_surface()
    check_dead_context_accessors()
    check_dead_agent_types()
    check_plugin_agent_mode_is_rejected()
    check_config_schema_has_one_owner()
    check_validation_warnings_reach_the_operator()
    check_context_audit_is_read()
    check_cli_validates_the_config_schema()
    check_manifest_cache_stays_documented()

    if BAD:
        print(f"extension wire audit: {len(BAD)} problem(s)")
        for line in BAD:
            print(f"  - {line}")
        return 1
    print("extension wire audit: PASS")
    return 0


if __name__ == "__main__":
    sys.exit(main())
