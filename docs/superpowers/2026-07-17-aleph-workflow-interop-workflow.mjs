export const meta = {
  name: 'aleph-workflow-interop-harden',
  description: 'Adversarially verify the Aleph workflow-interop .mjs-compat diff (effort opt + .mjs extension) as a cargo-check substitute, then synthesize a gap report',
  whenToUse: 'Re-run after changing src/workflow/interop/* to confirm export<->import round-trip symmetry, redline compliance, and Rust compile-correctness without invoking cargo.',
  phases: [
    { title: 'Verify', detail: 'four parallel adversarial reviewers, one per dimension' },
    { title: 'Report', detail: 'synthesize findings into a reference-compatible gapreport.json' },
  ],
}

const WT = '/Volumes/TBU/Workspace/Aleph/.claude/worktrees/workflow-interop-mjs-compat'
const FILES = 'src/workflow/interop/manifest.rs, src/workflow/interop/export.rs, src/workflow/interop/import.rs, src/workflow/store.rs, src/builtin_tools/workflow_tool.rs'

const FINDINGS_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['dimension', 'verdict', 'issues', 'notes'],
  properties: {
    dimension: { type: 'string' },
    verdict: { type: 'string', enum: ['PASS', 'FAIL'] },
    issues: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['severity', 'file', 'summary', 'fix'],
        properties: {
          severity: { type: 'string', enum: ['CRITICAL', 'HIGH', 'MEDIUM', 'LOW'] },
          file: { type: 'string' },
          line: { type: 'integer' },
          summary: { type: 'string' },
          fix: { type: 'string' },
        },
      },
    },
    notes: { type: 'string' },
  },
}

const DIMENSIONS = [
  {
    key: 'compile',
    prompt: [
      'You are a Rust compile-correctness reviewer. cargo is FORBIDDEN — reason from source only.',
      `Repo worktree: ${WT} (cd there; run \`git -C ${WT} diff\` and Read the changed files: ${FILES}).`,
      'Context: a new interchange-only field `effort: Option<String>` was added to `WorkflowManifestStep`, and the export write extension changed to `.mjs`.',
      'VERIFY, exhaustively:',
      '1. EVERY `WorkflowManifestStep { ... }` struct literal in the repo (interop/manifest.rs, export.rs, import.rs, store.rs) now lists an `effort:` field. A single missing one is a CRITICAL compile break (E0063 missing field). Enumerate each literal you find and confirm.',
      '2. `WorkflowStepDef` literals (def.rs, compile.rs, tests) must NOT have gained an `effort` field (that type never had one) — a stray `effort:` there is a CRITICAL error.',
      '3. `to_def()` builds `WorkflowStepDef` and must correctly OMIT effort (dropped) — confirm it does not reference `.effort` on the target.',
      '4. The new test `effort_opt_rendered_and_roundtrips_via_bare_scan` in export.rs: confirm every symbol it uses is in scope (`EMBED_PREFIX`, `parse_workflow_js`, `step`, `manifest`, `render_workflow_js`) and the types line up (`back.steps[0].effort.as_deref()` is `Option<&str>`).',
      '5. import.rs: `AgentOpts.effort` field exists, `assign_string_opt` has an `"effort"` arm, and both the agent-map (`effort: call.opts.effort`) and clarify-map (`effort: None`) set it.',
      '6. Trailing commas, field types, serde attrs all well-formed.',
      'Report FAIL with a CRITICAL issue for any missing/extra field; otherwise PASS. Be a skeptic — default to hunting for the one missed literal.',
    ].join('\n'),
  },
  {
    key: 'roundtrip',
    prompt: [
      'You are an export<->import round-trip symmetry reviewer for the Aleph workflow interop layer.',
      `Repo worktree: ${WT}. Read export.rs (render_agent_call, render_workflow_js), import.rs (read_agent_opts, assign_string_opt, scan_bare agent-map), manifest.rs (WorkflowManifestStep serde).`,
      'The invariant: an exported `.workflow.js`/`.mjs` must re-import to an identical manifest, on BOTH the embedded-`@aleph-workflow`-header path AND the header-stripped bare-scan path.',
      'VERIFY:',
      '1. export renders `effort: "<v>"` as a bare-string opt (via js_str) inside the agent() opts object.',
      '2. import bare-scan recovers it: read_agent_opts hits the `effort` key -> string branch -> assign_string_opt sets opts.effort -> agent-map copies to the step. Trace the path and confirm no gap.',
      '3. Embedded-header path: `effort` is a serde field on WorkflowManifestStep with camelCase rename + skip_serializing_if=Option::is_none, so it JSON-roundtrips and an effort-less step stays byte-identical (key omitted). Confirm.',
      '4. Ordering: export emits effort AFTER model, BEFORE schema. The reader is order-independent (key-driven), so this is safe — confirm the reader does not assume field order.',
      '5. Does the existing full-opts symmetry test (import.rs, the "sym" test) now also exercise effort? Confirm it sets effort and its assertion compares the whole manifest.',
      'FAIL if any asymmetry (export emits but import drops, or vice-versa, or a byte-drift for effort-less steps). Otherwise PASS.',
    ].join('\n'),
  },
  {
    key: 'redline',
    prompt: [
      'You are an Aleph architectural-redline reviewer. Redlines that matter here: R3 (no JS engine in core), R7 (LLM sovereignty — no deterministic reasoning), R10 (thin harness — no executable state creep), and backward-compat / non-destructive refactor rules.',
      `Repo worktree: ${WT}. Read the diff and manifest.rs/def.rs/compile.rs.`,
      'VERIFY:',
      '1. `effort` is INTERCHANGE-ONLY: it must appear ONLY in WorkflowManifestStep (interop) + export/import, and must NOT have leaked into `WorkflowDef`/`WorkflowStepDef` (def.rs), `compile.rs`, or any executor/dispatcher path. Grep to confirm effort is absent from def.rs/compile.rs. (This mirrors how isolation/agentType are interchange-only, unlike model which is executable.)',
      '2. The `.mjs` extension change: confirm it is NON-BREAKING — no code globs/reads `*.workflow.js` from a directory (import takes a raw `source` string, not a path). The only change is the on-disk output filename. Confirm store.rs write_text_at stays a generic ext-agnostic writer.',
      '3. No JS engine / JS-subset parser was added (R3): the reader is still the hand-written char scanner.',
      '4. No new deterministic routing/intent logic (R7/R10).',
      'FAIL if effort leaked into the executable core or the .mjs change breaks a reader. Otherwise PASS. Note whether promoting effort to executable (like model) is left as a documented follow-up.',
    ].join('\n'),
  },
  {
    key: 'entropy',
    prompt: [
      'You are a completeness + entropy-reduction reviewer (dead code, stale docs, missed sites, doc drift).',
      `Repo worktree: ${WT}. Read the diff and grep the tree.`,
      'VERIFY:',
      '1. No WorkflowManifestStep literal was missed (cross-check by grepping `WorkflowManifestStep {` and confirming each has effort).',
      '2. Doc drift: after switching export to `.mjs`, are there user-facing strings/comments still claiming the export writes `.workflow.js` that should now say `.mjs`? Distinguish (a) genuinely-stale (export output filename) from (b) intentionally-correct (the interop FORMAT is still called `.workflow.js` in prose; import still accepts `.workflow.js`). List only genuinely-stale ones.',
      '3. WORKFLOW_INTEROP.md (docs/reference/) — does it need an `effort` row in the agent-opts table and an extension note? (It will be updated separately; just flag what is needed.)',
      '4. Any dead code introduced (unused field, unreachable arm)? effort is consumed by export+import so it is live.',
      'Report issues as MEDIUM/LOW. PASS if nothing genuinely broken; list doc updates needed in notes.',
    ].join('\n'),
  },
]

phase('Verify')
const findings = await parallel(
  DIMENSIONS.map((d) => () =>
    agent(d.prompt, { label: `verify:${d.key}`, phase: 'Verify', schema: FINDINGS_SCHEMA, effort: 'high' })
  )
)
const real = findings.filter(Boolean)

phase('Report')
const GAPREPORT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['subsystem', 'goal', 'verdict', 'formatCompatMap', 'confirmedIssues', 'redlineCompliance', 'followUps'],
  properties: {
    subsystem: { type: 'string' },
    goal: { type: 'string' },
    verdict: { type: 'string', enum: ['SHIP', 'FIX_THEN_SHIP', 'BLOCK'] },
    formatCompatMap: {
      type: 'array',
      description: 'Claude Code dynamic-workflow .mjs construct -> Aleph interop mapping, post-change',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['construct', 'status', 'note'],
        properties: {
          construct: { type: 'string' },
          status: { type: 'string', enum: ['SUPPORTED', 'ADDED', 'INTENTIONALLY_DROPPED'] },
          note: { type: 'string' },
        },
      },
    },
    confirmedIssues: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['severity', 'file', 'summary', 'fix'],
        properties: {
          severity: { type: 'string' },
          file: { type: 'string' },
          line: { type: 'integer' },
          summary: { type: 'string' },
          fix: { type: 'string' },
        },
      },
    },
    redlineCompliance: { type: 'string' },
    followUps: { type: 'array', items: { type: 'string' } },
  },
}

const synth = await agent(
  [
    'You are the lead synthesizer. Below are four adversarial reviewers\' structured findings on a diff that (G1) added an interchange-only `effort` agent-opt to the Aleph workflow-interop layer and (G2) switched the export write extension to `.mjs`, for Claude Code dynamic-workflow compatibility.',
    'Reviewer findings (JSON):',
    JSON.stringify(real, null, 2),
    '',
    'Produce a gap report. Rules:',
    '- verdict = BLOCK if any CRITICAL issue survives; FIX_THEN_SHIP if only HIGH; SHIP if clean.',
    '- confirmedIssues: include ONLY issues you judge real after reading the findings; drop speculative ones. Deduplicate across reviewers.',
    '- formatCompatMap: enumerate the dynamic-workflow .mjs constructs (meta+phases, phase(), sequential agent(), parallel(), agent-opts label/model/phase/schema/isolation/agentType/effort/review/timeoutSecs/maxRetries, clarify(), pipeline(), log()/budget()/nested workflow()/loops+conditionals, .mjs file extension) and mark each SUPPORTED (already), ADDED (this diff), or INTENTIONALLY_DROPPED (R7/R10 by design).',
    '- redlineCompliance: one sentence on R3/R7/R10 + backward-compat.',
    '- followUps: concrete, e.g. promote effort to executable per-member override; update WORKFLOW_INTEROP.md + FEATURE_LOCATOR.md.',
    'subsystem="workflow-interop", goal="Claude Code dynamic-workflow .mjs format compatibility".',
  ].join('\n'),
  { label: 'synthesize:gapreport', phase: 'Report', schema: GAPREPORT_SCHEMA, effort: 'high' }
)

return synth
