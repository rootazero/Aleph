#!/usr/bin/env python3
"""Config/context/diagnostics wire-seam guard - invariants the 2026-11-20 audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo root:

    python3 scripts/config_context_diagnostics_wire_audit.py

Exit non-zero when any check fails.

Two kinds of check live here:

* CUT      - a severed wire that was removed must stay removed, or the cut is
             undone by a later edit and the dead surface grows back.
* LIVE     - a reconnected wire must stay connected: the producer, the
             consumer and the call that joins them.
* RESERVED - a deliberately dark wire (documented forward vocabulary) must stay
             as it is, so a later audit does not "fix" it into a lie.
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


def absent(check, path, needle, note):
    """A severed wire must stay severed."""
    text = read(path)
    if needle in text:
        bad(check, f"{path} still carries {needle!r} - {note}")


def present(check, path, needle, note):
    """A reconnected wire must stay connected."""
    text = read(path)
    if needle not in text:
        bad(check, f"{path} no longer carries {needle!r} - {note}")


def check_config_cuts():
    """The routing-rule mutators had no caller: `Config` grew five methods
    that nothing could reach. The rule list is still editable through the
    config RPC path, so nothing needs them on the struct."""
    for name in ("add_rule_at_top", "remove_rule", "move_rule", "get_rule", "rule_count"):
        absent("config/cut", "src/config/methods.rs", f"fn {name}", "the routing-rule mutator is dead")

    absent("config/cut", "src/config/structs.rs", "pub fn new() -> Self",
           "Config::new() had no caller; Default is the only construction")
    absent("config/cut", "src/config/types/agents_def.rs", "fn model_str",
           "AgentModelRef::model_str had no production consumer")
    absent("config/cut", "src/gateway/handlers/config.rs", "pub async fn handle_get(",
           "the config.get handler was never registered - config.get serves handle_get_full_config")


def check_config_live():
    """`config.get` is served by one handler; the Panel asks for a section."""
    present("config/live", "src/bin/aleph-server/commands/start/builder/handlers/settings.rs",
            "handle_get_full_config", "config.get must stay registered")
    absent("config/live", "src/bin/aleph-server/commands/start/builder/handlers/settings.rs",
           "config_handlers::handle_get(", "the unregistered handler must not come back")
    acp = read("interfaces/webchat/src/api/acp.rs")
    call = acp.find('rpc_call("config.get"')
    if call < 0:
        bad("config/live", "the Panel must read the acp section through config.get")
    else:
        payload = acp[call: call + 200]
        if '"section"' not in payload:
            bad("config/live", "the Panel's config.get call must name the section it wants")
        if '"key"' in payload:
            bad("config/live", "the Panel's config.get call must not send `key` - the server never read it")
    present("config/live", "src/config/live_apply.rs", "LIVE_SUBSECTIONS",
            "the live-apply guard must keep watching the live subsections")
    present("config/live", "src/config/tests/guide_doc_drift.rs", "every_deployed_guide_section_and_key_exists_in_the_config_schema",
            "the guide drift guard must stay installed")
    present("config/live", "src/config/reload_impact.rs", "pub fn wire(",
            "every reload_impact producer must serialize the same shape")
    present("config/live", "src/config/reload_impact.rs", "pub fn strongest(",
            "the per-field producers must fold into one verdict")
    for producer in ("src/gateway/handlers/config.rs",
                     "src/gateway/handlers/execution_config.rs",
                     "src/gateway/handlers/search_config/update.rs",
                     "src/gateway/handlers/search_config/delete.rs",
                     "src/gateway/handlers/browser_config.rs",
                     "src/builtin_tools/self_config.rs"):
        text = prod(producer)
        if "reload_impact" not in text:
            bad("config/live", f"{producer} no longer reports a reload impact")
        elif ".wire(" not in text:
            bad("config/live", f"{producer} must use ReloadImpact::wire, not a hand-rolled shape")
    if re.search(r'"reload_impact"\s*:\s*"', prod("src/gateway/handlers/execution_config.rs")):
        bad("config/live", "a bare verdict string is a second reload_impact shape")
    absent("config/cut", "interfaces/webchat/src/api/settings.rs", "pub typing_speed",
           "the Panel must not ask for a field the server retired")
    absent("config/cut", "interfaces/webchat/src/platform/wide/views/settings/behavior.rs",
           "TypingSpeedSection", "the Panel's dead typing-speed knob must stay gone")
    absent("config/cut", "interfaces/webchat/src/state/typewriter.rs", "pub cps:",
           "the Panel clock must not carry a server-owned rate field")


def check_diagnostics_cuts():
    """`HealthCheck::title` was a second name for the finding's own title."""
    diag = read("src/diagnostics/check.rs")
    if "fn title" in diag:
        bad("diag/cut", "HealthCheck::title still has a definition - the renderer reads the finding title")
    for path in Path(ROOT / "src/diagnostics/checks").glob("*.rs"):
        text = read(str(path.relative_to(ROOT)))
        if re.search(r"impl\s+HealthCheck\s+for\b[^}]*?fn\s+title\b", text, re.S):
            bad("diag/cut", f"{path} still implements HealthCheck::title")
    absent("diag/cut", "src/diagnostics/mod.rs", "pub async fn run(",
           "DiagnosticEngine::run was a wrapper with no production caller")
    absent("diag/cut", "src/diagnostics/checks/discord_channel_health.rs", "impl Default for DiscordChannelHealthCheck",
           "the shadowing Default impl had no caller")
    if "from_records" in prod("src/diagnostics/checks/plugins_activated.rs"):
        bad("diag/cut", "PluginsActivatedCheck::from_records must stay test-only - the daemon reads the live registry")
    present("diag/live", "src/diagnostics/mod.rs", "every_check_module_is_attached_to_an_engine_face",
            "the check-registration parity guard must stay installed")
    present("diag/live", "src/diagnostics/checks/cache_health.rs", "malformed",
            "malformed alarm rows must fail closed, not read as 'no alarms'")
    present("diag/live", "src/diagnostics/finding.rs", "redact_secrets",
            "every free-text field of a finding must pass the redaction chokepoint")


def check_context_cuts():
    """The preflight registry and the fabric result carried fields nobody read."""
    absent("context/cut", "src/context/budget/preflight.rs", "pub fn len(",
           "NativePreflightRegistry::len had no caller")
    absent("context/cut", "src/context/budget/preflight.rs", "pub fn is_empty(",
           "NativePreflightRegistry::is_empty had no caller")
    absent("context/cut", "src/context/fabric/runtime.rs", "native_preflight_registry",
           "the builder hook had no caller")
    if "pub fn search(" in prod("src/context/retrieval/content_index.rs"):
        bad("context/cut", "the public search wrapper had only test callers")
    if "pub fn open_in_memory(" in prod("src/context/retrieval/content_index.rs"):
        bad("context/cut", "the public in-memory constructor had only test callers")
    absent("context/cut", "src/context/budget/mod.rs", "pub const fn last_pressure(",
           "the pressure accessor had only test callers")


def check_context_live():
    """One path precedence, one owner: `path` then `file_path`."""
    present("context/live", "src/context/file_ops.rs", "pub(crate) fn raw_path(",
            "the raw path precedence must stay single-sourced")
    if '.get("file_path")' in prod("src/context/budget/cheap_passes/file_op_supersede.rs"):
        bad("context/live", "the cheap pass must not re-implement the path precedence")
    present("context/live", "src/context/budget/cheap_passes/image_stripping.rs",
            "historical_user_image_stripping",
            "the two image stages must be distinguishable in the preflight log")
    present("context/live", "src/context/budget/cheap_passes/image_stripping.rs",
            "historical_image_stripping",
            "the tool-result image stage must keep its name")
    # The plugin-facing result is a WIRE contract: the extension adapter
    # deserializes the guest's JSON into it and the host re-serializes it, so
    # a declared strategy/quality survives the round trip. Cutting the fields
    # would change the canonical JSON every plugin sees.
    present("context/live", "src/context/fabric.rs", "pub strategy: String",
            "the plugin's declared strategy is part of the canonical result contract")
    present("context/live", "src/context/fabric.rs", "pub quality: ContextQuality",
            "the plugin's declared quality is part of the canonical result contract")


def check_reserved():
    """Deliberately dark wires: documented forward vocabulary, not accidents.

    Cutting these would turn a reservation into a lie, so they are pinned.
    """
    reserved = [
        ("src/gateway/hot_reload.rs", "pub fn should_hot_reload(",
         "ReloadMode::should_hot_reload is documented as a future per-section policy"),
        ("src/config/load.rs", "fn merge_builtin_rules(",
         "the builtin-rule merge is a deliberate AI-first no-op hook"),
        ("src/config/ui_hints/mod.rs", "pub struct ConfigUiHints",
         "the DTO stays so the config.schema.ui_hints wire shape stays stable (2026-08-16)"),
        ("src/context/compact/folds.rs", "AutoLlm",
         "the fold strategies are forward vocabulary for a strategy this build never records"),
        ("src/context/compact/folds.rs", "PressureEscalation",
         "the fold triggers are forward vocabulary for producers this build does not have"),
        ("src/context/fabric.rs", "ContextPersistencePolicy",
         "the persistence policy is documentary metadata today"),
        ("src/context/fabric.rs", "CrossSession",
         "the wider namespace vocabulary is bounded by the only transport"),
        ("docs/reference/CONTEXT_PLUGINS.md", "session_shutdown",
         "the reserved host entry point is documented, not accidental"),
        ("src/config/reload_impact.rs", "INERT_SECTIONS",
         "the inert verdict is kept on purpose: the sections it lists are parsed but inert"),
    ]
    for path, needle, note in reserved:
        present("reserved", path, needle, note)


def main():
    check_config_cuts()
    check_config_live()
    check_diagnostics_cuts()
    check_context_cuts()
    check_context_live()
    check_reserved()
    if BAD:
        print(f"FAIL: {len(BAD)} wire invariant(s) broken")
        for item in BAD:
            print(f"  - {item}")
        return 1
    print("PASS: config/context/diagnostics wire invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
