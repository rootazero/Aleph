// Batch 1c — re-run of the six slices that were lost to a provider throttle.
// Same agent and method as batch1/batch2, with two discipline changes that
// the earlier failures taught us:
//   1. the report file is written INCREMENTALLY (a finding is appended the
//      moment it is confirmed) so a truncated child still leaves evidence;
//   2. the child must NOT call the compress tool — a child that spends its
//      budget folding context times out with no report on disk.
const ROOT = "/home/zou/data/workspace/Aleph";
const OUT = ROOT + "/.superpowers/sdd/2026-11-28-severed-wire-audit";
const MODEL = "minimax-cn/MiniMax-M3:medium";
const REPORT_FORMAT = `## A. Findings

| ID | Form | Severity | Title | Producer | Consumer |
|----|------|----------|-------|----------|----------|
| SW-<SLICE>-<N> | <1-6> | <high/medium/low> | <one line> | <path:line> | <path:line or "none"> |

**Severity tally**: high <n> · medium <n> · low <n> · (UNCERTAIN <n>)

### SW-<SLICE>-<N> — <one-line title>
- **Form**: <n> (<name of the form>)
- **Producer**: \`<path:line>\` — <what produces it>
- **Consumer**: \`<path:line>\` or **none found** — <what consumes it, or why nothing does>
- **Live caller search**: <the exact rg commands you ran and what they returned>
- **Symptom**: <what a user would observe, or "none user-visible">
- **Hypothesis**: CONNECT / CUT / DECIDE
- **Confidence**: high / medium / UNCERTAIN
- **Severity**: <n>

## B. Clean / verified symbols (no severed wire)
## C. New observations (not findings)
## D. Coverage statement (files read, lenses applied, seams not covered and why)`;

const common = (slice, focus, outFile) => `You are a READ-ONLY severed-wire auditor for a slice of the Aleph repo (Rust). Repo root: ${ROOT} (your cwd is already the repo root).
HARD RULES
- READ-ONLY: never edit a file, never git add/commit/checkout/stash, never run cargo (check/test/build/clippy). Read and search only.
- Do NOT call the compress tool, and do NOT spend your budget re-reading files you already read. Batch your greps.
- Budget discipline: read the slice files listed below, grep the whole repo for the symbols you care about, and WRITE THE REPORT INCREMENTALLY — append each confirmed finding to the output file as soon as you confirm it, then keep going. A child that is cut off must still leave a usable report on disk.
- The graph at graphify-out/graph.json (160946 nodes) was built from an OLDER commit than HEAD: use \`graphify query "<question>"\` for orientation only and CONFIRM every answer with rg — rg is truth.

YOUR SLICE (read all of it):
${slice}

YOUR JOB: find SEVERED WIRES — a producer's output that never reaches its consumer (missing registration, missing dispatch, missing subscription, missing call, missing config read), so the feature looks broken, or looks present but is dark even though everything compiles. Use the 6 forms (1 no consumer, 2 stub far-end, 3 inert config, 4 client ghost, 5 name/path drift, 6 never-compiled far-end) and the 7 lenses (registration parity, call-vs-handler parity, classifier-vs-handler parity, event emit-vs-subscribe parity, config-reader parity, path/route parity, stub sweep).

SPECIFIC FOCUS FOR THIS SLICE:
${focus}

MANDATORY METHOD — for EVERY candidate you report you MUST have grepped for a LIVE production caller/handler on the far end and found none (or found a stub). State the exact greps you ran and their result. "Missing handler" is very often a dead client wrapper with zero callers.
- \`rg -n --glob '*.rs' '<pattern>' .\` — search the WHOLE repo, not just your slice. A reader outside your slice counts.
- Distinguish production from \`#[cfg(test)]\` / \`mod tests\` / \`#[test] fn\`. A symbol exercised only by tests is severed for production purposes unless the test asserts the production path. Say which it is.
- No speculation: if unsure, mark the candidate UNCERTAIN and say what evidence would settle it.

OUTPUT: write the report (Markdown) to EXACTLY this path: ${OUT}/${outFile}, using this shape:
${REPORT_FORMAT.replace(/<SLICE>/g, outFile.replace(/\.md$/, "").toUpperCase())}
Then return a summary of at most 40 lines: the candidate count split by form; the top 10 candidates as one line each (\`<id> | <form> | <producer path:line> | <consumer path:line or "none"> | <one-line reason>\`); and the seams you did NOT cover and why.`;

const SLICES = [
  {
    key: "s1a-gen-registry-types",
    label: "audit generation registry/types",
    outFile: "s1a-gen-registry-types.md",
    slice: `- src/generation/registry.rs
- src/generation/probe.rs
- src/generation/voice_catalog.rs
- src/generation/types/mod.rs
- src/generation/types/output.rs
- src/generation/types/params.rs
- src/generation/types/progress.rs
- src/generation/types/request.rs
- src/generation/types/generation_type.rs`,
    focus: `- Who registers a generation provider, and who READS the registry? Check every registry method for a production caller (get / names / len / is_empty / providers_for_type / names_for_type / get_voices_for_provider / first_for_type / iter / contains / remove / clear).
- The probe path (probe.rs): is the probe reachable from a real RPC/handler? Does the probe URL actually resolve for every provider type the factory supports (including aliases)? Is the "verified" flag it writes read back?
- voice_catalog.rs: is the catalog wired to the voice RPC the Panel calls, or is it a parallel list that drifts?
- types/*.rs: for each public field of GenerationParams / GenerationOutput / GenerationProgress / GenerationRequest / GenerationMetadata — is there a production producer AND a production consumer? Name the fields that are written but never read, or read but never written.`,
  },
  {
    key: "s1b-gen-trait-errors",
    label: "audit generation trait/errors",
    outFile: "s1b-gen-trait-errors.md",
    slice: `- src/generation/mod.rs
- src/generation/error.rs`,
    focus: `- The GenerationProvider trait: for EVERY trait method (generate / name / supported_types / supports / color / default_model / check_progress / cancel / edit_image / supports_image_editing / list_voices) name a production caller of the CONCRETE provider, not just of the trait.
- Which trait methods does the registry actually use, and which does only a test use?
- GenerationError: for every variant, find the production producer and the production consumer of that error. Which variants are produced but never surfaced (a swallowed error), or surfaced as a generic message (the classification is lost)?
- Is the error -> tool-error mapping (From<GenerationError> for ToolError) losing information that the caller needs (retryable / user-actionable)?`,
  },
  {
    key: "s5-gc-api",
    label: "audit group_chat api surface",
    outFile: "s5-gc-api.md",
    slice: `- src/group_chat/mod.rs
- src/group_chat/channel.rs
- src/group_chat/coordinator.rs
- src/group_chat/protocol.rs
- src/group_chat/persona.rs`,
    focus: `- Persona: every field of PersonaConfig / Persona (provider, model, thinking_level, name, description, system_prompt) — is it read by a production caller, or inert?
- Channel: every channel method and every channel message variant — who sends and who receives it? A variant nobody handles is a severed wire.
- Coordinator: what does it actually coordinate, and who listens to its output?
- Protocol: every message type and every parse/encode function — is there a peer that parses what this encodes and encodes what this parses (a round-trip that cannot close)?`,
  },
  {
    key: "s6-gc-exec",
    label: "audit group_chat execution",
    outFile: "s6-gc-exec.md",
    slice: `- src/group_chat/executor.rs
- src/group_chat/session.rs
- src/group_chat/orchestrator.rs`,
    focus: `- Every builder on GroupChatExecutor / GroupChatSession / GroupChatOrchestrator (with_provider_registry / with_coordinator_visible / with_history_window / with_database / with_max_rounds / max_no_commit_attempts …): name the production caller.
- Every method on the three types: name the production caller, or prove it is unreachable.
- Persistence: if a session or turn is written, who reads it back? Trace the whole chain (write -> storage -> read -> consumer).
- The round loop: is every branch reachable from a production entry point, and does every value the caller passes in actually reach the code that consumes it?`,
  },
  {
    key: "s7-cross-generation",
    label: "audit generation far end",
    outFile: "s7-cross-generation.md",
    slice: `- src/gateway/handlers/generation_providers/** (the whole directory)
- src/gateway/voice/local_provider.rs
- src/gateway/voice/inbound/provider.rs
- src/gateway/voice/outbound.rs
- src/builtin_tools/generation/** (the whole directory)
- src/gateway/inbound_router/** (the generation and voice paths)`,
    focus: `- The far end of the generation module: every RPC and every tool that calls into src/generation. For each one, is the value the caller passes actually consumed by the provider, or dropped on the floor?
- Config knobs: every generation/voice config field that a user can set in TOML or the Panel — name the code that reads it. A field that is parsed, validated and stored but never read is an inert config.
- The Panel/gateway wire types for generation: does every field the Panel sends reach the provider, and does every provider capability the gateway advertises reach the Panel?
- Registration parity: every RPC method name the gateway declares vs every handler registered at boot.`,
  },
  {
    key: "s8-cross-groupchat",
    label: "audit group_chat far end",
    outFile: "s8-cross-groupchat.md",
    slice: `- src/gateway/inbound_router/group_chat_handler.rs
- src/gateway/handlers/group_chat.rs
- src/gateway/handlers/mod.rs (the group_chat registration parts)
- src/bin/aleph-server/commands/start/** (the group_chat wiring parts)`,
    focus: `- Every group_chat RPC: the client-side call site, the registered handler, and the data that flows from the client into the store and back out. Where does a value the client sent stop?
- The boot wiring in src/bin/aleph-server/commands/start: every value the group_chat executor/orchestrator is constructed with — is it the live value or a placeholder? A placeholder wired into the production path is a severed wire.
- The gateway's group_chat placeholder handlers and their override at boot: does the override really win on every path (both the RPC and the tool paths)?`,
  },
];

const wave = (group) => {
  const launched = group.map((t) =>
    runs
      .run(t.key, {
        label: t.label,
        agent: "sev-auditor",
        cwd: ROOT,
        model: MODEL,
        options: { timeoutMs: 2700000 },
        task: common(t.slice, t.focus, t.outFile),
      })
      .then((r) => ({ key: t.key, ok: r.ok, summary: String(r.output || "").slice(0, 600) }))
      .catch((e) => ({ key: t.key, ok: false, summary: "child failed: " + String(e) }))
  );
  return Promise.all(launched);
};

const out = [];
out.push(...(await wave(SLICES.slice(0, 2))));
out.push(...(await wave(SLICES.slice(2, 4))));
out.push(...(await wave(SLICES.slice(4, 6))));
return out;
