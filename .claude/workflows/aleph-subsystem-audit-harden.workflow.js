export const meta = {
  name: 'aleph-subsystem-audit-harden',
  description: 'Audit an existing Aleph Rust subsystem against reference patterns + redlines, close the highest-value test gap, then compile-verify',
  whenToUse: 'Re-run on any Aleph subsystem: pass args.subsystem + args.paths + args.hardenTarget. Defaults to the workflow subsystem.',
  phases: [
    { title: 'Audit', detail: '4 parallel lenses: pattern conformance / redline compliance / test gap / wiring' },
    { title: 'Harden', detail: 'add the missing unit tests for the one untested file (surgical)' },
    { title: 'Verify', detail: 'scoped cargo check + cargo test, no cargo fmt' },
    { title: 'Synthesize', detail: 'merge findings + verification into one report' },
  ],
}

// ---- Parameterisation (reusable) -----------------------------------------
const cfg = (args && typeof args === 'object') ? args : {}
const subsystem = cfg.subsystem || 'workflow'
const paths = cfg.paths || [
  'src/workflow/mod.rs',
  'src/workflow/def.rs',
  'src/workflow/store.rs',
  'src/workflow/compile.rs',
  'src/builtin_tools/workflow_tool.rs',
]
// File the harden phase is allowed to touch (the one with zero tests).
const hardenTarget = cfg.hardenTarget || 'src/builtin_tools/workflow_tool.rs'

// Ground truth established by the orchestrator's inline scout — agents verify &
// deepen this rather than rediscovering it. NOTE: no raw backticks anywhere in
// these template strings (a raw backtick would terminate the literal).
const GROUND_TRUTH = [
  'The "' + subsystem + '" subsystem ALREADY EXISTS and is fully wired (commit cd8ebc14c).',
  'Files: ' + paths.join(', ') + '.',
  'Shape: a thin executor — a declarative WorkflowDef template (name + steps, each',
  'step = {id, agent, prompt, depends_on}) is validated + topologically sorted (Kahn)',
  'and compiled by workflow::compile::materialize() into the existing coord_tasks DAG,',
  'then executed by the existing TeamDispatcher. The engine adds NO scheduler and NO',
  'reasoning (R7/R10 safe). It is exposed to the LLM as the R8 tool "workflow" with an',
  'action discriminator: save / list / describe / delete / run.',
  'Wiring chain (verified): builtin_tools/workflow_tool.rs defines WorkflowTool ->',
  'executor/builtin_registry/builder/constructor.rs:876 constructs it when a',
  'CoordTaskStore is present -> registry.rs:286 stores it, :1110 dispatches ->',
  'definitions.rs:601 + groups.rs:164 expose it to the agent loop.',
  'Test state: def.rs/store.rs/compile.rs are well tested (~26 tests). The file',
  hardenTarget + ' has ZERO tests — that is the single confirmed gap.',
].join('\n')

const REPO_RULES = [
  'Aleph repo gotchas (MANDATORY):',
  '- NEVER run cargo fmt — it dirties ~185 unrelated files.',
  '- Scope every build: cargo check -p alephcore / cargo test -p alephcore --lib <filter>. Never bare cargo build.',
  '- Compiles are slow; do not loop on full builds.',
  '- Code comments in English.',
  '- Surgical changes only (CLAUDE.md): touch ONLY what the task needs; do not refactor or reformat existing code.',
].join('\n')

const AUDIT_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['lens', 'summary', 'conformance', 'gaps', 'strengths'],
  properties: {
    lens: { type: 'string' },
    summary: { type: 'string' },
    conformance: { type: 'string', enum: ['full', 'partial', 'missing', 'n/a'] },
    gaps: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['title', 'severity', 'detail', 'recommendation'],
        properties: {
          title: { type: 'string' },
          severity: { type: 'string', enum: ['critical', 'high', 'medium', 'low'] },
          detail: { type: 'string' },
          recommendation: { type: 'string' },
        },
      },
    },
    strengths: { type: 'array', items: { type: 'string' } },
  },
}

const HARDEN_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['changed', 'file', 'testsAdded', 'approach', 'notes'],
  properties: {
    changed: { type: 'boolean' },
    file: { type: 'string' },
    testsAdded: { type: 'array', items: { type: 'string' } },
    approach: { type: 'string' },
    notes: { type: 'string' },
  },
}

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['checkPassed', 'testsPassed', 'testCount', 'failures', 'evidence'],
  properties: {
    checkPassed: { type: 'boolean' },
    testsPassed: { type: 'boolean' },
    testCount: { type: 'integer' },
    failures: { type: 'array', items: { type: 'string' } },
    evidence: { type: 'string' },
  },
}

// ---- Phase 1: Audit (4 parallel lenses) ----------------------------------
phase('Audit')

const lenses = [
  {
    key: 'patterns',
    prompt: [
      'You are auditing the Aleph "' + subsystem + '" subsystem for CONFORMANCE TO REFERENCE WORKFLOW PATTERNS.',
      GROUND_TRUTH,
      'Read the source files: ' + paths.join(', ') + '.',
      'Reference: Anthropic "Building Effective Agents" defines 5 workflow patterns:',
      '(1) prompt chaining, (2) routing, (3) parallelization (sectioning/voting),',
      '(4) orchestrator-workers, (5) evaluator-optimizer.',
      'For each pattern, judge whether Alephs DAG-of-steps model (WorkflowDef.steps with',
      'depends_on edges compiled to coord_tasks) can express it. A linear chain =',
      'prompt chaining; sibling steps with no edge = parallelization; a fan-in step =',
      'orchestrator-workers gather. Identify which patterns are NATURALLY expressible,',
      'which would need a new primitive, and whether any missing primitive is worth',
      'adding vs a YAGNI/R3 deferral. Report concrete gaps with severity and file:line.',
      'Do NOT modify any files.',
    ].join('\n'),
  },
  {
    key: 'redlines',
    prompt: [
      'You are auditing the Aleph "' + subsystem + '" subsystem for ARCHITECTURAL REDLINE COMPLIANCE.',
      GROUND_TRUTH,
      'Read: ' + paths.join(', ') + ' plus the redline definitions in CLAUDE.md (R3 core',
      'minimalism, R7 LLM sovereignty, R8 everything-is-a-tool, R10 thin harness / dumb',
      'loop / the 5 nots).',
      'Verify the engine is a TRUE thin executor: confirm compile.rs/def.rs/store.rs',
      'contain zero reasoning, no intent classification, no completion judging, no tool',
      'filtering — only deterministic data transforms (validate, topo-sort, file IO, DAG',
      'materialisation). Confirm the LLM authors the template and the system only',
      'schedules. Flag ANY place where deterministic code is doing something the LLM',
      'should own, or where the executor leaks into src/harness/. Report gaps with',
      'file:line + severity. Do NOT modify any files.',
    ].join('\n'),
  },
  {
    key: 'tests',
    prompt: [
      'You are doing a TEST-COVERAGE GAP ANALYSIS of the Aleph "' + subsystem + '" subsystem.',
      GROUND_TRUTH,
      'Read every file in ' + paths.join(', ') + ' and enumerate exactly what each',
      '#[cfg(test)] module covers. Then pinpoint untested behaviour. Pay special attention',
      'to ' + hardenTarget + ' (confirmed 0 tests): list each public behaviour that SHOULD',
      'be tested (every WorkflowArgs action: save, list, describe, delete, run, plus error',
      'paths and the WorkflowToolOutput shape). For the run action note that WorkflowTool',
      'holds an Arc<dyn CoordTaskStore> and an optional dispatch Notify; an in-memory',
      'SqliteCoordTaskStore is usable in tests (see compile.rs tests for the setup pattern).',
      'For save/list/describe/delete note they hit workflow::store which resolves paths from',
      'the ALEPH_HOME env var (process-global — call out the test-isolation hazard and how',
      'compile.rs/store.rs already test the _at variants). Output a concrete, prioritised',
      'list of tests to add. Do NOT modify any files.',
    ].join('\n'),
  },
  {
    key: 'wiring',
    prompt: [
      'You are auditing the END-TO-END WIRING of the Aleph "' + subsystem + '" subsystem.',
      GROUND_TRUTH,
      'Trace and VERIFY the full path that makes the LLM-facing workflow tool actually',
      'callable and its run action actually reach execution:',
      'builtin_tools/workflow_tool.rs (WorkflowTool / AlephTool impl) ->',
      'executor/builtin_registry/builder/constructor.rs (construction, ~line 876) ->',
      'executor/builtin_registry/registry.rs (field ~286, dispatch ~1110) ->',
      'executor/builtin_registry/definitions.rs (~601) + groups.rs (~164) ->',
      'workflow::materialize -> coord_tasks -> teams::dispatcher::TeamDispatcher.',
      'Confirm: is the tool gated on CoordTaskStore presence, and is that store present in',
      'the normal server build? Does run require a pre-created team (its members must cover',
      'every step.agent)? Is there any dangling/dead wire, missing registration, or silent',
      'no-op? Report concrete findings with file:line + severity. Do NOT modify any files.',
    ].join('\n'),
  },
]

const audit = await parallel(
  lenses.map((l) => () =>
    agent(l.prompt, { label: 'audit:' + l.key, phase: 'Audit', schema: AUDIT_SCHEMA })
  )
)
const findings = audit.filter(Boolean)
log('Audit complete: ' + findings.length + '/4 lenses returned; ' +
  findings.reduce((n, f) => n + f.gaps.length, 0) + ' gaps total')

const auditDigest = findings
  .map((f) =>
    '### Lens: ' + f.lens + ' (conformance: ' + f.conformance + ')\n' +
    f.summary + '\nGaps:\n' +
    (f.gaps.map((g) => '- [' + g.severity + '] ' + g.title + ': ' + g.detail + ' -> ' + g.recommendation).join('\n') || '- (none)') +
    '\nStrengths:\n' +
    (f.strengths.map((s) => '- ' + s).join('\n') || '- (none)'))
  .join('\n\n')

// ---- Phase 2: Harden (single writer — the only code mutation) ------------
phase('Harden')

const harden = await agent(
  [
    'You are CLOSING THE TEST GAP for the Aleph "' + subsystem + '" subsystem. This is the',
    'ONLY phase allowed to modify code, and you may modify ONLY this file:',
    '  ' + hardenTarget,
    REPO_RULES,
    GROUND_TRUTH,
    '',
    'Audit findings from the test-gap lens and others:',
    auditDigest,
    '',
    'TASK: Add a well-structured #[cfg(test)] mod tests block to ' + hardenTarget + ' that',
    'covers the WorkflowTool AlephTool behaviour. Requirements:',
    '- Cover the run action end-to-end using an in-memory SqliteCoordTaskStore (mirror the',
    '  setup_store() pattern in src/workflow/compile.rs tests: Connection::open_in_memory +',
    '  SqliteCoordTaskStore::new + .migrate().await). Assert WorkflowToolOutput.task_ids are',
    '  populated. Save a template to a TempDir-backed ALEPH_HOME first so run can load it,',
    '  or use workflow::store::save_at if the tool path requires the env. If you set',
    '  ALEPH_HOME, guard against cross-test races (env is process-global): prefer a single',
    '  combined test or a serialisation guard, and restore/scope the var. Read the actual',
    '  store API before writing — do not invent method names.',
    '- Cover the output shape of save/list/describe/delete actions (action string + message',
    '  + which Option fields are populated). Reuse a TempDir ALEPH_HOME for the file-backed',
    '  actions; assert list reflects a saved template and delete is idempotent.',
    '- Cover at least one error path (describe/run of a non-existent template returns Err).',
    '- Keep tests deterministic and hermetic. English comments. Match the existing test',
    '  style in src/workflow/*.rs. Do NOT touch any other file. Do NOT run cargo fmt.',
    '- Before finishing, sanity-check imports resolve (AlephTool::call via async_trait,',
    '  tokio::test for async, tempfile::TempDir, the coord task store path).',
    'Report what you added.',
  ].join('\n'),
  { label: 'harden:' + hardenTarget.split('/').pop(), phase: 'Harden', schema: HARDEN_SCHEMA }
)
log('Harden: changed=' + (harden && harden.changed) + ' testsAdded=' + (harden && harden.testsAdded ? harden.testsAdded.length : 0))

// ---- Phase 3: Verify ------------------------------------------------------
phase('Verify')

const verify = await agent(
  [
    'You are VERIFYING the Aleph "' + subsystem + '" subsystem after the harden phase added',
    'tests to ' + hardenTarget + '.',
    REPO_RULES,
    'Run, in order, and capture real output:',
    '1. cargo check -p alephcore   (must compile clean — report the first real error if not)',
    '2. cargo test -p alephcore --lib workflow   (runs workflow:: + workflow_tool:: tests)',
    'If the test filter workflow misses the new tests (they live in',
    'builtin_tools::workflow_tool), also try: cargo test -p alephcore --lib workflow_tool.',
    'Do NOT run cargo fmt. Do NOT modify code — you only verify. Report whether check',
    'passed, whether tests passed, the test count, any failures verbatim, and paste the',
    'decisive evidence line (the test result summary).',
  ].join('\n'),
  { label: 'verify:cargo', phase: 'Verify', schema: VERIFY_SCHEMA }
)
log('Verify: check=' + (verify && verify.checkPassed) + ' tests=' + (verify && verify.testsPassed) + ' count=' + (verify && verify.testCount))

// ---- Phase 4: Synthesize --------------------------------------------------
phase('Synthesize')

const report = await agent(
  [
    'Synthesize a final engineering report on the Aleph "' + subsystem + '" subsystem audit+harden run.',
    'Audience: the repo owner, who initially thought this feature needed building but it',
    'already existed. Be honest and concise.',
    '',
    'AUDIT FINDINGS (4 lenses):',
    auditDigest,
    '',
    'HARDEN RESULT:',
    'changed=' + (harden && harden.changed) + '; file=' + (harden && harden.file) + '; approach=' + (harden && harden.approach),
    'testsAdded=' + JSON.stringify(harden && harden.testsAdded) + '; notes=' + (harden && harden.notes),
    '',
    'VERIFY RESULT:',
    'checkPassed=' + (verify && verify.checkPassed) + '; testsPassed=' + (verify && verify.testsPassed) + ';',
    'testCount=' + (verify && verify.testCount) + '; failures=' + JSON.stringify(verify && verify.failures) + ';',
    'evidence=' + (verify && verify.evidence),
    '',
    'Produce a markdown report with these sections:',
    '1. Verdict — current state of the workflow feature (1 paragraph).',
    '2. Reference-pattern conformance — which of Anthropics 5 workflow patterns the DAG',
    '   model expresses, which are missing, and whether each missing one is worth adding',
    '   or should be deferred (R3/YAGNI).',
    '3. Redline compliance — R7/R8/R10 status.',
    '4. What this run changed — the tests added + verification outcome.',
    '5. Recommended follow-ups — prioritised, each tagged ADD or DEFER with a one-line reason.',
    'Return ONLY the markdown report.',
  ].join('\n'),
  { label: 'synthesize', phase: 'Synthesize' }
)

return {
  subsystem,
  auditLensCount: findings.length,
  totalGaps: findings.reduce((n, f) => n + f.gaps.length, 0),
  hardened: harden,
  verified: verify,
  report,
}
