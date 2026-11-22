#!/usr/bin/env python3
"""Gateway wire-seam guard - invariants the 2026-11-21/22 src/gateway audit fixed.

A guard is a DETECTOR, not a judge. Each check encodes a fact the type system
cannot express and that a later edit can silently break. Run from the repo root:

    python3 scripts/gateway_wire_audit.py

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


def missing(check, path, note):
    """A deleted file must stay deleted."""
    if (ROOT / path).exists():
        bad(check, f"{path} exists again - {note}")


def count(text, needle):
    return text.count(needle)


# ---------------------------------------------------------------------------
# CUT: the severed wires stay severed
# ---------------------------------------------------------------------------


def check_deleted_modules_stay_deleted():
    """The modules the audit deleted must not come back.

    `http_server` was a second static-file server that nothing started; the
    link subsystem (`LinkManager`, `LinkConfig`, `~/.aleph/links/*.yaml`) had
    no path from a config file to a running bridge and was deleted by an
    operator ruling; the adapter helpers below were unreachable or were
    write-only state."""
    for path, note in (
        ("src/gateway/http_server.rs", "the second static-file server had no starter"),
        ("src/gateway/link/mod.rs", "the link subsystem was deleted by an operator ruling"),
        ("src/gateway/link/manager.rs", "the link subsystem was deleted by an operator ruling"),
        ("src/gateway/link/types.rs", "the link subsystem was deleted by an operator ruling"),
        ("src/gateway/interfaces/telegram/audit.rs", "the telegram audit ring was write-only"),
        ("src/gateway/interfaces/telegram/sticker.rs",
         "the sticker description cache had no producer"),
        ("src/gateway/interfaces/discord/draft.rs", "the discord draft chunker had no caller"),
        ("src/gateway/interfaces/discord/group_policy.rs", "the discord group policy had no reader"),
        ("src/gateway/interfaces/discord/session_key.rs", "the discord session key had no reader"),
        ("src/gateway/interfaces/whatsapp/message.rs", "the whatsapp inbound mapper was dead"),
        ("src/gateway/interfaces/whatsapp/types.rs", "the whatsapp jid helpers were dead"),
        ("src/gateway/interfaces/wechat/media.rs", "the wechat media decryptor was dead"),
    ):
        missing("gateway/cut", path, note)


def check_module_declarations_stay_cut():
    """`src/gateway/mod.rs` must not declare the deleted modules."""
    for needle in ("pub mod http_server;", "pub mod link;"):
        absent("gateway/cut", "src/gateway/mod.rs", needle,
               "the module was deleted - the declaration would not compile")


def check_shadow_config_stays_cut():
    """The gateway's shadow `[sandbox]` / `[tools]` / `[[bindings]]` copy stays cut.

    `crate::gateway::config::GatewayConfig` deserialized the same operator
    TOML as the app-level `Config`, so `[sandbox]`, `[tools.chrome]` and the
    canonical `[[bindings]]` array-of-tables either parsed into a copy nobody
    read or made the file fail to load at all. The live types are
    `crate::sandbox::SandboxConfig` and `crate::config::types::tools::ToolsConfig`."""
    body = read("src/gateway/config.rs")
    for field in ("sandbox:", "tools:", "bindings:"):
        if field in body:
            bad("gateway/cut", f"src/gateway/config.rs still declares {field!r} - the shadow "
                               f"config surface is back and the operator TOML parses into a copy "
                               f"nobody reads")


def check_removed_config_knobs_stay_cut():
    """The knobs the audit cut must not come back as inert fields."""
    absent("gateway/cut", "src/gateway/interfaces/telegram/config.rs", "polling_interval_secs",
           "the knob had no reader - the telegram poller uses its own interval")
    absent("gateway/cut", "src/gateway/interfaces/discord/config.rs", "ThreadConfig",
           "the thread map was never read - the binding table was the dead copy")
    absent("gateway/cut", "src/gateway/interfaces/wechat/auth.rs", "fn qr_login",
           "the stub had no caller and no effect")


def check_removed_panel_rows_stay_cut():
    """The Panel row for a removed knob must go with it."""
    panel = "interfaces/webchat/src/platform/wide/views/settings/channels/definitions.rs"
    absent("gateway/cut", panel, "polling_interval_secs",
           "the config key is gone - the Panel toggle would write a key nobody reads")


def check_dead_handlers_stay_cut():
    """The unreachable handler must stay unreachable."""
    absent("gateway/cut", "src/gateway/handlers/memory/mod.rs", "fn handle_reembed(",
           "the route calls handle_owned_reembed -> start_reembed")


def check_test_only_helpers_stay_test_only():
    """Two helpers are only called by their own tests - they must say so."""
    for path, needle in (
        ("src/gateway/voice/format.rs", "fn build_format_prompt("),
        ("src/gateway/telecom/public_run.rs", "fn run_public_sms("),
    ):
        body = prod(path)
        if needle not in body:
            # the helper itself is behind `#[cfg(test)]`, so it is gone from
            # the production prefix: that is the intended end state.
            continue
        index = body.find(needle)
        window = body[max(0, index - 200) : index]
        if "#[cfg(test)]" not in window:
            bad("gateway/cut", f"{path}: {needle!r} is reachable from production code - it has no "
                               f"production caller")


def check_delivery_queue_stamp_doc():
    """The delivery queue's crash-window stamp is written by `begin_replay`.

    `mark_inflight` stamped by id alone and had no production caller, so the
    module doc that named it as the pre-send stamp was describing a wire that
    did not exist. The method itself stays (it is the store's raw statement
    helper, used by the store's own tests), but it must not be credited with
    the production stamp and it must say who really stamps."""
    doc = read("src/gateway/delivery_queue.rs")
    if "[`DeliveryStore::mark_inflight`]" in doc:
        bad("gateway/cut", "src/gateway/delivery_queue.rs still credits mark_inflight with the "
                           "pre-send stamp - production stamps inside begin_replay")
    if "begin_replay" not in doc:
        bad("gateway/live", "src/gateway/delivery_queue.rs no longer names begin_replay as the "
                            "production attempt stamp")
    body = prod("src/gateway/delivery_queue.rs")
    if "fn mark_inflight" in body and "begin_replay" not in body:
        bad("gateway/cut", "src/gateway/delivery_queue.rs exposes mark_inflight in production code "
                           "without pointing at the production stamp path (begin_replay)")


# ---------------------------------------------------------------------------
# LIVE: the reconnected wires stay connected
# ---------------------------------------------------------------------------


def check_pair_loop_wiring_live():
    """The per-channel bot-loop guard must stay wired end to end.

    The operator key `ChannelAccessConfig::bot_loop_protection` had no
    reader, so the guard ran on hard-coded defaults and the doc promised a
    fallback that did not exist. The chain is now: the config key ->
    `PairLoopGuardConfig::from_channel_config` -> the boot loop ->
    `InboundMessageRouter::pair_loop_config_for` -> the check."""
    present_prod("gateway/live", "src/gateway/pair_loop_guard.rs", "fn from_channel_config",
                 "the operator key must have a reader")
    present_prod("gateway/live", "src/gateway/inbound_router/mod.rs", "fn pair_loop_config_for",
                 "the router must resolve the guard config per channel")
    present("gateway/live", "src/gateway/inbound_router/mod.rs",
            "self.pair_loop_config_for(msg.channel_id.as_str())",
            "the check must look the config up by the message's channel")
    present("gateway/live", "src/bin/aleph-server/commands/start/builder/subsystems.rs",
            "register_pair_loop_override",
            "the boot loop must register the per-channel override")
    present("gateway/live", "src/gateway/channel_policy.rs", "bot_loop_protection",
            "the operator key must stay in the config surface")


def check_session_origin_live():
    """The file session backend must persist the session origin.

    `SessionStore::set_source_channel` had a no-op default and the file
    backend never overrode it, so every file-backed session silently lost
    the channel it came from."""
    present_prod("gateway/live", "src/gateway/session_store/file_backend/mod.rs",
                 "async fn set_source_channel",
                 "the file backend must persist the origin instead of dropping it")


def check_request_state_writers_live():
    """`AwaitingResponse` and `Cancelled` must keep their writers.

    Both states existed in the request-state machine and in the metrics
    snapshot, but nothing ever wrote them: the `awaiting` and `cancelled`
    counters were structurally always zero, and a dropped request kept
    counting as in-flight until the pruner removed it."""
    present("gateway/live", "src/gateway/middleware/metrics.rs",
            "state_registry.transition(request_id, RequestState::AwaitingResponse)",
            "the awaiting state must have a writer")
    present("gateway/live", "src/gateway/middleware/metrics.rs", "self.registry.cancel(",
            "a dropped request must be marked cancelled")


def check_backpressure_carries_the_request_id():
    """A backpressure rejection must answer the request's own id.

    `id: null` leaves the client's pending promise hanging until its own
    timeout, so the rejection was invisible to the caller that asked."""
    present("gateway/live", "src/gateway/middleware/metrics.rs", "rpc_id",
            "the backpressure rejection must carry the request id")


def check_steering_rescue_runs():
    """The steering rescue must run on both the normal and the fast path."""
    body = read("src/gateway/execution_engine/execute.rs")
    if count(body, "post_run_steering_rescue") < 3:
        bad("gateway/live", "src/gateway/execution_engine/execute.rs no longer calls the steering "
                             "rescue on every path (definition + both call sites)")


def check_agent_end_update_output_is_honest():
    """An `AgentEnd` `update_output:` must not be silently dropped.

    The hook result was written into the run's final text and then thrown
    away by the caller, so the hook looked like it worked. The seam now
    says plainly that the output was already delivered."""
    present("gateway/live", "src/gateway/execution_engine/run_loop/mod.rs",
            "AgentEnd hook returned `update_output:`",
            "the swallowed hook result must be reported")


def check_hook_stop_reaches_the_channel():
    """A graceful hook stop must emit the stop text.

    The hook's `Ok(stop_msg)` was returned to the caller, which dropped the
    string and never sent a completion - the user saw nothing at all."""
    for path in ("src/gateway/execution_engine/run_loop/mod.rs",
                 "src/gateway/execution_engine/run_loop/inner.rs"):
        present("gateway/live", path, "emit_response_chunk",
                "the graceful stop must emit the stop text to the channel")


# ---------------------------------------------------------------------------
# DARK: the deliberately dark wires stay as they are
# ---------------------------------------------------------------------------


def check_method_visibility_stays_dark():
    """`method_visibility.rs` is an audit trail, not a dispatch gate.

    Nothing in it intercepts a request; the table exists so that a removed
    enforcement call becomes a named test failure. It must keep saying so,
    and `treatment_of` (the registry accessor its pinned tests read) must
    stay."""
    doc = read("src/gateway/method_visibility.rs")
    if "NOT a dispatch" not in doc:
        bad("gateway/dark", "src/gateway/method_visibility.rs no longer says it is NOT a dispatch "
                            "gate - a later reader would treat it as enforcement")
    present("gateway/dark", "src/gateway/method_visibility.rs", "fn treatment_of",
            "the registry accessor the pinned tests read must stay")


def main():
    check_deleted_modules_stay_deleted()
    check_module_declarations_stay_cut()
    check_shadow_config_stays_cut()
    check_removed_config_knobs_stay_cut()
    check_removed_panel_rows_stay_cut()
    check_dead_handlers_stay_cut()
    check_test_only_helpers_stay_test_only()
    check_delivery_queue_stamp_doc()
    check_pair_loop_wiring_live()
    check_session_origin_live()
    check_request_state_writers_live()
    check_backpressure_carries_the_request_id()
    check_steering_rescue_runs()
    check_agent_end_update_output_is_honest()
    check_hook_stop_reaches_the_channel()
    check_method_visibility_stays_dark()
    if BAD:
        print(f"FAIL: {len(BAD)} wire invariant(s) broken")
        for item in BAD:
            print(f"  - {item}")
        return 1
    print("PASS: gateway wire invariants hold")
    return 0


if __name__ == "__main__":
    sys.exit(main())
