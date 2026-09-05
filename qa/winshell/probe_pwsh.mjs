#!/usr/bin/env node
// Probe the HOST's PowerShell for the seven contracts Aleph's Windows shell
// now rests on. Not an integration test: no server is booted, no config is
// written, nothing in `~/.aleph` is touched. Every stage spawns the host's own
// `pwsh` and reports what it answered.
//
// Why it exists: this round replaced `bash` with PowerShell 7 as the Windows
// shell, and the design rests on MEASURED facts — the encoding a child gets,
// whether a native child's exit code survives, the command-line ceiling, what
// `-NoProfile` costs, what a stripped environment breaks. Those numbers were
// measured once, in a chat window. A number that cannot be re-derived is a
// number nobody can check, and this repo's discipline says it must carry the
// predicate it measured and the commit it measured at (判据 §18). This file is
// how the next person re-derives them.
//
// ## What each stage can be made to say NO
//
// A stage that cannot go red is not a probe (判据 §2), so every one of them has
// a falsification knob — `QA_WINSHELL_FALSIFY=<name>` breaks exactly one input
// and the stage must go red:
//
//   resolve   look for a program name that cannot exist
//   prologue  `encoding`'s with-prologue arms lose the prologue
//   epilogue  `exit`'s with-epilogue arm loses the epilogue
//   join      `comment`'s newline join becomes `;`
//   length    `length`'s search is capped below the ceiling
//   threshold `length`'s 5c is forced on, with a threshold above the ceiling
//   profile   `profile`'s spawns carry a flag pwsh rejects
//   env       `env`'s full-environment arm passes only PATH
//
// ## Two confounds this file is shaped around
//
// * **Argument encoding.** Every script here is pure ASCII; the non-ASCII
//   string in the `encoding` stage is built from code points
//   (`[char]0x4E2D + [char]0x6587`) rather than written literally. A literal
//   would put non-ASCII bytes in the argv, and then a wrong answer could be the
//   argv's fault rather than the child's output encoding — a confound that
//   already fooled one measurement this round.
// * **A second copy of the contract.** The prologue, epilogue, argv flags and
//   the separators joining them are DERIVED from `src/utils/shell.rs`
//   (`derive_ps_contract.mjs`), never spelled here. Where `pwsh` lives is
//   deliberately NOT derived — `resolve` walks PATH itself, because deriving it
//   would make the fixture agree with the code by construction.
//
// Usage:  node probe_pwsh.mjs <repo-root> <stage>
import fs from "node:fs";
import path from "node:path";
import { spawnSync } from "node:child_process";
import { deriveShellContract, derivePassEnv } from "./derive_ps_contract.mjs";

const STAGES = [
  "resolve",
  "encoding",
  "exit",
  "comment",
  "length",
  "profile",
  "env",
];

const REPO = process.argv[2];
const STAGE = process.argv[3] ?? "all";
const FALSIFY = process.env.QA_WINSHELL_FALSIFY ?? "";
// `profile` times cold spawns, so it is the only slow stage. Small on purpose.
const SPAWN_SAMPLES = Number(process.env.QA_WINSHELL_N ?? "5");

if (!REPO || (STAGE !== "all" && !STAGES.includes(STAGE))) {
  console.error(`usage: probe_pwsh.mjs <repo-root> <${STAGES.join("|")}|all>`);
  process.exit(2);
}

// ---------------------------------------------------------------------------
// Reporting. Counts are the deliverable: an OBSV line is an observation the
// stage reports but does NOT gate on, and it must never be mistaken for a pass.
// ---------------------------------------------------------------------------
let PASS = 0;
let FAIL = 0;
let SKIP = 0;

const ok = (name, observed) => {
  console.log(`  PASS  ${name}`);
  console.log(`        observed: ${observed}`);
  PASS += 1;
};
const bad = (name, observed, why) => {
  console.log(`  FAIL  ${name}`);
  console.log(`        observed: ${observed}`);
  if (why) console.log(`        expected: ${why}`);
  FAIL += 1;
};
const skip = (name, reason) => {
  console.log(`  SKIP  ${name}`);
  console.log(`        reason:   ${reason}`);
  SKIP += 1;
};
/** Reported, not gated. Never counts toward PASS. */
const obsv = (name, observed) => {
  console.log(`  OBSV  ${name}`);
  console.log(`        observed: ${observed}`);
};
const check = (name, cond, observed, why) =>
  cond ? ok(name, observed) : bad(name, observed, why);
const head = (t) => console.log(`\n-- ${t} ${"-".repeat(Math.max(0, 68 - t.length))}`);

// ---------------------------------------------------------------------------
// Platform gate. A stage that cannot run must never report a pass (判据 §2).
// ---------------------------------------------------------------------------
if (process.platform !== "win32") {
  console.log(`\n=== SKIP: qa/winshell on ${process.platform} ===`);
  console.log(
    "  Every contract here is about a WINDOWS host's PowerShell: the code page",
  );
  console.log(
    "  a child inherits, whether a native child's exit code survives, the",
  );
  console.log(
    "  32767-character CreateProcess command line, and what an env_clear()ed",
  );
  console.log(
    "  child is missing. `pwsh` exists on Linux and macOS, but it is not the",
  );
  console.log(
    "  shell Aleph selects there (`utils::shell::resolve` picks bash) and none",
  );
  console.log("  of the five Windows-shaped facts above are even askable.");
  console.log("  THIS FIXTURE ASSERTED NOTHING. It is not a pass.");
  console.log(
    `\n=== 0 passed, 0 failed, ${STAGES.length} skipped (not this platform) ===`,
  );
  process.exit(0);
}

// ---------------------------------------------------------------------------
// The contract, read out of the product's own source.
// ---------------------------------------------------------------------------
let CONTRACT;
let PASSENV;
try {
  CONTRACT = deriveShellContract(REPO);
  PASSENV = derivePassEnv(REPO);
} catch (e) {
  console.error(`\n!!! ${String(e.message ?? e)}`);
  process.exit(1);
}

// ---------------------------------------------------------------------------
// Spawning
// ---------------------------------------------------------------------------
const SPAWN_TIMEOUT_MS = 60_000;

/**
 * Run `program` and hand back everything a verdict could need — including the
 * spawn error, because "I could not ask" must never render as "the answer is
 * no" (判据 §8).
 */
function run(program, args, { input = null, env = undefined } = {}) {
  const t0 = process.hrtime.bigint();
  const r = spawnSync(program, args, {
    input: input === null ? undefined : input,
    env,
    windowsHide: true,
    timeout: SPAWN_TIMEOUT_MS,
    maxBuffer: 16 * 1024 * 1024,
  });
  const ms = Number(process.hrtime.bigint() - t0) / 1e6;
  return {
    code: r.status,
    signal: r.signal,
    out: r.stdout ?? Buffer.alloc(0),
    err: (r.stderr ?? Buffer.alloc(0)).toString("utf8"),
    error: r.error ? `${r.error.code ?? ""} ${r.error.message}`.trim() : null,
    ms,
  };
}

/** One line of a program's stderr, for a failure report. */
const firstErrLine = (s) =>
  (s || "").split(/\r?\n/).find((l) => l.trim().length) ?? "(no stderr)";

/** How a run ended, in one string. */
const outcome = (r) =>
  r.error
    ? `spawn error: ${r.error}`
    : `exit=${r.code}${r.signal ? ` signal=${r.signal}` : ""}`;

// ---------------------------------------------------------------------------
// Byte-level output parsing. The `encoding` stage is ABOUT bytes, so nothing
// here may decode stdout as a string before the verdict is taken.
// ---------------------------------------------------------------------------
const UTF8_BOM = Buffer.from([0xef, 0xbb, 0xbf]);

function stripBom(buf) {
  const had = buf.length >= 3 && buf.subarray(0, 3).equals(UTF8_BOM);
  return { had, buf: had ? buf.subarray(3) : buf };
}

/** The bytes after `KEY=` on the first line that starts with it, or null. */
function fieldBytes(buf, key) {
  const pfx = Buffer.from(`${key}=`, "ascii");
  let start = 0;
  const lines = [];
  for (let i = 0; i < buf.length; i += 1) {
    if (buf[i] === 0x0a) {
      lines.push(buf.subarray(start, i));
      start = i + 1;
    }
  }
  if (start < buf.length) lines.push(buf.subarray(start));
  for (let ln of lines) {
    if (ln.length && ln[ln.length - 1] === 0x0d) ln = ln.subarray(0, ln.length - 1);
    if (ln.length >= pfx.length && ln.subarray(0, pfx.length).equals(pfx)) {
      return ln.subarray(pfx.length);
    }
  }
  return null;
}

const hex = (buf) =>
  buf ? [...buf].map((b) => b.toString(16).padStart(2, "0")).join(" ") : "(none)";

// ---------------------------------------------------------------------------
// Invocation shapes. `full()` is EXACTLY what `ShellKind::invocation` builds;
// the other two exist so a stage can vary one thing at a time.
// ---------------------------------------------------------------------------
const wrap = (script, { prologue = true, epilogue = true, joinAfter = null } = {}) => {
  const pro = prologue ? CONTRACT.prologue + CONTRACT.sep_before_script : "";
  const sep = joinAfter ?? CONTRACT.sep_after_script;
  const epi = epilogue ? sep + CONTRACT.epilogue : "";
  return pro + script + epi;
};

/** argv for `-Command <literal>`. */
const argvCommand = (text) => [...CONTRACT.flags, text];
/** argv for `-Command -` (script arrives on stdin). */
const argvStdin = () => [...CONTRACT.flags, "-"];

// ===========================================================================
// 1. resolve — where pwsh is, and what version
// ===========================================================================
//
// Resolved by walking PATH with PATHEXT, the way `which::which` does, and NOT
// by reading a path out of the product. A hardcoded
// `C:\Program Files\PowerShell\7\pwsh.exe` would agree with the code by
// construction and could not report a host that has none.
function whichAll(name) {
  const exts = (process.env.PATHEXT ?? ".COM;.EXE;.BAT;.CMD")
    .split(";")
    .map((e) => e.trim())
    .filter(Boolean);
  const dirs = (process.env.PATH ?? "").split(path.delimiter).filter(Boolean);
  const hits = [];
  for (const d of dirs) {
    for (const ext of exts) {
      const p = path.join(d, name + ext);
      try {
        if (!fs.statSync(p).isFile()) continue;
        // The on-disk spelling, not the one PATHEXT handed us: PATHEXT is
        // upper-case, so a raw join reports `pwsh.EXE` for a file called
        // `pwsh.exe` — a path the reader would fail to find by eye.
        const real = fs.realpathSync.native(p);
        if (!hits.includes(real)) hits.push(real);
      } catch {
        /* not there */
      }
    }
  }
  return hits;
}

let PWSH = null; // the absolute path every later stage spawns

function stageResolve() {
  head("1. resolve — pwsh's absolute path and version");
  const want = FALSIFY === "resolve" ? "pwsh-no-such-program" : "pwsh";
  if (want !== "pwsh") console.log(`  (falsified: looking for \`${want}\`)`);

  const hits = whichAll(want);
  const ladder = [];
  for (const alt of ["powershell", "cmd"]) {
    const h = whichAll(alt);
    ladder.push(`${alt}=${h[0] ?? "(absent)"}`);
  }
  ladder.push(`COMSPEC=${process.env.COMSPEC ?? "(unset)"}`);

  if (!hits.length) {
    bad(
      "1a pwsh is on PATH",
      `no \`${want}\` in any of ${(process.env.PATH ?? "").split(path.delimiter).length} PATH entries`,
      `an absolute path; the ladder below it is what utils::shell would fall to — ${ladder.join(" ")}`,
    );
    return false;
  }
  PWSH = hits[0];
  ok(
    "1a pwsh is on PATH",
    hits.length > 1 ? `${PWSH}  (+${hits.length - 1} more: ${hits.slice(1).join(", ")})` : PWSH,
  );

  const r = run(PWSH, [
    "-NoProfile",
    "-NonInteractive",
    "-Command",
    "$PSVersionTable.PSVersion.ToString()",
  ]);
  const ver = r.out.toString("utf8").trim();
  check(
    "1b $PSVersionTable.PSVersion",
    !r.error && r.code === 0 && /^\d+\./.test(ver),
    ver ? `${ver}  (${outcome(r)})` : `${outcome(r)}  stderr: ${firstErrLine(r.err)}`,
    "a version string and exit 0",
  );
  // Reported, not gated: the ladder is what `utils::shell::resolve` would fall
  // to, and it is worth knowing even on a host where pwsh is present.
  obsv("1c the fallback ladder below pwsh", ladder.join("   "));
  return true;
}

// ===========================================================================
// 2. encoding — is the child UTF-8 without the prologue?
// ===========================================================================
//
// The claim in shell.rs is that the encoding is a property of the INVOCATION
// FORM, not of the host — 65001 via `-Command`, the host ANSI page via stdin —
// which is why the prologue states it rather than assuming it. That is two
// claims, and this stage measures both plus the contract they justify.
const ENC_SCRIPT = [
  '"CP=" + [Console]::OutputEncoding.CodePage',
  // Built from code points on purpose: a literal 中文 here would put non-ASCII
  // bytes in the argv, and a wrong answer could then be the argv's fault.
  '"S=" + [char]0x4E2D + [char]0x6587',
].join("\n");
const WANT_UTF8 = Buffer.from("中文", "utf8"); // e4 b8 ad e6 96 87

function encodingArm(label, viaStdin, withPrologue) {
  const text = wrap(ENC_SCRIPT, {
    prologue: withPrologue && FALSIFY !== "prologue",
    epilogue: withPrologue,
  });
  const r = viaStdin
    ? run(PWSH, argvStdin(), { input: text })
    : run(PWSH, argvCommand(text));
  const { had: bom, buf } = stripBom(r.out);
  const cp = fieldBytes(buf, "CP");
  const s = fieldBytes(buf, "S");
  return {
    label,
    r,
    bom,
    cp: cp ? cp.toString("ascii").trim() : null,
    bytes: s,
    isUtf8: s ? s.equals(WANT_UTF8) : false,
  };
}

function stageEncoding() {
  head("2. encoding — the child's code page, with and without the prologue");
  if (FALSIFY === "prologue")
    console.log("  (falsified: the with-prologue arms carry NO prologue)");

  // What this host's console actually uses, so `936` below is not a mystery
  // number. Labelled with the command that produced it (判据 §18). Read as
  // latin1: under CP936 the localised prefix is not valid UTF-8, and the digits
  // are all this needs.
  const chcp = run(process.env.COMSPEC ?? "cmd.exe", ["/c", "chcp"]);
  const chcpText = chcp.out.toString("latin1").trim();
  const consoleCp = (chcpText.match(/(\d{3,5})\s*$/) ?? [])[1] ?? null;
  obsv(
    "2z host console code page",
    `\`cmd /c chcp\` -> ${chcpText || outcome(chcp)}${consoleCp ? `   (parsed: ${consoleCp})` : "   (unparsed)"}`,
  );

  const arms = [
    encodingArm("-Command literal, with prologue", false, true),
    encodingArm("stdin `-Command -`, with prologue", true, true),
    encodingArm("-Command literal, NO prologue", false, false),
    encodingArm("stdin `-Command -`, NO prologue", true, false),
  ];

  // "No CP= line" has two very different causes and they must not read alike:
  // a child that errored, and a child that ran, printed NOTHING and exited 0.
  // The second is the expensive one (判据 §11, a no-op reporting success), and
  // it is reachable here — see the stdin hazard noted below.
  const describe = (a) =>
    a.cp === null
      ? `${outcome(a.r)}  no CP= line; stdout=${a.r.out.length} bytes` +
        (a.r.out.length
          ? ` [${hex(a.r.out.subarray(0, 24))}]`
          : " — the child produced NOTHING") +
        `  stderr: ${firstErrLine(a.r.err)}`
      : `codepage=${a.cp}  bytes=[${hex(a.bytes)}]  ${a.isUtf8 ? "= UTF-8 中文" : "NOT UTF-8"}${a.bom ? "  (stdout began with a UTF-8 BOM)" : ""}`;
  for (const a of arms) obsv(`2· ${a.label}`, describe(a));

  const [cmdPro, stdinPro, cmdBare, stdinBare] = arms;

  // CONTRACT — the thing the product depends on. Both with-prologue arms must
  // land on 65001 AND emit UTF-8 bytes. Code page alone is not enough: a host
  // reporting 65001 while writing CP936 bytes would pass a code-page-only check.
  check(
    "2a with prologue, `-Command` literal is UTF-8",
    cmdPro.cp === "65001" && cmdPro.isUtf8,
    describe(cmdPro),
    "codepage=65001 and bytes e4 b8 ad e6 96 87",
  );
  // ⚠️ MEASURED 2026-09-05: `pwsh -Command -` parses stdin as a sequence of
  // COMPLETE statements, so a script whose first line opens a multi-line block
  // (`try {` … `} catch {}`) is discarded WHOLE — no output, no stderr, exit 0.
  // A one-line `try { … } catch {}` is fine, and the same multi-line block via
  // `-Command <literal>` is fine. This check is the thing standing between that
  // and a silent no-op reporting success, which is why it asserts on the
  // observed code page and bytes rather than on the prologue's text: a prologue
  // that changes shape must be re-measured, not re-matched.
  check(
    "2b with prologue, stdin is UTF-8",
    stdinPro.cp === "65001" && stdinPro.isUtf8,
    describe(stdinPro),
    "codepage=65001 and bytes e4 b8 ad e6 96 87",
  );

  // PREMISE — why the prologue's two encoding lines exist at all. If every
  // no-prologue arm were already UTF-8 here, those lines would be unjustified
  // on this host, and saying so is the honest report (判据 §18: the conclusion's
  // scope is the method, not the wish).
  const bareUtf8 = [cmdBare, stdinBare].filter((a) => a.cp === "65001" && a.isUtf8);
  check(
    "2c at least one invocation form is NOT UTF-8 without the prologue",
    bareUtf8.length < 2,
    `-Command -> codepage=${cmdBare.cp} ${cmdBare.isUtf8 ? "UTF-8" : "not UTF-8"}; ` +
      `stdin -> codepage=${stdinBare.cp} ${stdinBare.isUtf8 ? "UTF-8" : "not UTF-8"}`,
    "at least one form to disagree — otherwise the prologue's two encoding lines are unjustified HERE",
  );

  // WHICH form disagrees is a separate question from WHETHER one does, and the
  // two answers are not interchangeable. shell.rs's comment says the code page
  // is "a property of the invocation form, not of the host" — 65001 via
  // `-Command`, 936 via stdin. That is a claim this stage can check, and it is
  // reported rather than gated: a wrong REASON in a comment is a documentation
  // defect, not a contract failure, and gating it would turn a host that
  // matches the comment red for being correct.
  //
  // Measured by hand on this host 2026-09-05, four ways (stdout to a file, to a
  // pipe, from Node with CREATE_NO_WINDOW, and from Node without): the
  // no-prologue answer follows the CONSOLE's code page in every one of them,
  // and moves to 65001 for both forms when the parent console is `chcp 65001`.
  // That experiment is not automated here because it mutates the operator's
  // console for the whole terminal — re-run it by hand if this line ever
  // disagrees with itself.
  const bareAgree = cmdBare.cp === stdinBare.cp;
  const followsConsole =
    consoleCp !== null && cmdBare.cp === consoleCp && stdinBare.cp === consoleCp;
  obsv(
    "2d WHY the no-prologue answer is what it is (ungated)",
    bareAgree
      ? `both forms answered ${cmdBare.cp}${followsConsole ? `, which is this console's code page` : ""} — ` +
        `so HERE the code page is a property of the CONSOLE, not of the invocation form. ` +
        `shell.rs's PS_PROLOGUE comment says the opposite (65001 via -Command, 936 via stdin); ` +
        `its CONCLUSION holds and then some, its stated reason does not reproduce.`
      : `-Command answered ${cmdBare.cp} and stdin answered ${stdinBare.cp} — the forms differ, ` +
        `matching shell.rs's PS_PROLOGUE comment.`,
  );
}

// ===========================================================================
// 3. exit — a native child's code, with and without the epilogue
// ===========================================================================
function stageExit() {
  head("3. exit — does `cmd /c exit 3` survive?");
  if (FALSIFY === "epilogue")
    console.log("  (falsified: the with-epilogue arm carries NO epilogue)");

  const bare = run(PWSH, argvCommand("cmd /c exit 3"));
  obsv(
    "3· bare `-Command 'cmd /c exit 3'` (no prologue, no epilogue)",
    `${outcome(bare)}   stderr: ${firstErrLine(bare.err)}`,
  );
  // PREMISE: if a bare invocation already answered 3, the epilogue would be
  // dead weight.
  check(
    "3a without the epilogue the child's code does NOT survive",
    bare.code !== 3,
    `exit=${bare.code} (the child asked for 3)`,
    "anything but 3 — otherwise the epilogue is unnecessary on this host",
  );

  const full = run(
    PWSH,
    argvCommand(wrap("cmd /c exit 3", { epilogue: FALSIFY !== "epilogue" })),
  );
  check(
    "3b with the epilogue the child's code survives",
    full.code === 3,
    `exit=${full.code}${full.error ? ` (${full.error})` : ""}   stderr: ${firstErrLine(full.err)}`,
    "exit=3",
  );

  // The闸's other direction (判据 §14): an epilogue that invents failures would
  // pass 3b and break every successful command.
  const zero = run(PWSH, argvCommand(wrap("cmd /c exit 0")));
  check(
    "3c the epilogue does not invent a failure",
    zero.code === 0,
    `exit=${zero.code}   stderr: ${firstErrLine(zero.err)}`,
    "exit=0",
  );
}

// ===========================================================================
// 4. comment — the epilogue must not be swallowed by a trailing `#`
// ===========================================================================
//
// The rule shell.rs states is "joined with newlines, never `;`". Both halves
// are measured: the separator comes out of the source (so a change to the join
// changes what this stage runs), and the `;` arm is run for real so the green
// is a comparison rather than an assertion about a string.
function stageComment() {
  head("4. comment — a trailing `# comment` must not eat the epilogue");
  if (FALSIFY === "join")
    console.log("  (falsified: the product's join is replaced by `;`)");

  obsv(
    "4z the join this build actually uses",
    `prologue${JSON.stringify(CONTRACT.sep_before_script)}script` +
      `${JSON.stringify(CONTRACT.sep_after_script)}epilogue   (from ${CONTRACT.source})`,
  );

  // A SUCCEEDING script. This is the pair that can tell the two joins apart:
  // `$__aleph_ok=$?` is the epilogue's first line, so a `;` join comments out
  // exactly the statement that carries "the script succeeded", and the
  // remaining lines then fall through to `exit 1` — a green script reported as
  // a failure, in silence.
  const okScript = "Write-Output ok\n# a comment";
  const nl = run(
    PWSH,
    argvCommand(wrap(okScript, { joinAfter: FALSIFY === "join" ? ";" : null })),
  );
  const semi = run(PWSH, argvCommand(wrap(okScript, { joinAfter: ";" })));

  check(
    "4a newline-joined: a succeeding script exits 0",
    nl.code === 0,
    `exit=${nl.code}  stdout=${JSON.stringify(nl.out.toString("utf8").trim())}  stderr: ${firstErrLine(nl.err)}`,
    "exit=0",
  );
  check(
    "4b `;`-joined: the same script does NOT exit 0",
    semi.code !== 0,
    `exit=${semi.code}  stdout=${JSON.stringify(semi.out.toString("utf8").trim())}`,
    "anything but 0 — otherwise `;` is harmless here and the rule is unjustified",
  );

  // The native-failure shape, reported and deliberately NOT gated. Only the
  // epilogue's FIRST line is swallowed by a `;` join, and `$LASTEXITCODE` is
  // read on the second — so this pair answers the same on both joins and could
  // not tell them apart. A check here would be a 恒真的谓词 (判据 §2); saying
  // so is cheaper than shipping one.
  const nat = "cmd /c exit 3\n# a comment";
  const natNl = run(PWSH, argvCommand(wrap(nat)));
  const natSemi = run(PWSH, argvCommand(wrap(nat, { joinAfter: ";" })));
  obsv(
    "4· native-failure script, both joins (ungated — it cannot separate them)",
    `newline exit=${natNl.code}   \`;\` exit=${natSemi.code}` +
      `   ${natNl.code === natSemi.code ? "(identical, as predicted)" : "(they DIFFER — worth a look)"}`,
  );
}

// ===========================================================================
// 5. length — the `-Command` ceiling
// ===========================================================================
//
// The subject is the HOST's ceiling: the largest script `pwsh -Command` will
// carry here, binary-searched rather than asserted.
//
// ⚠️ Deliberately says nothing about which arm the product takes. Whether
// PowerShell stays on `-Command` at every size or gains a stdin route above a
// threshold is a live decision, and a fixture that described one of those two
// worlds would be wrong the day the other one landed — while still passing,
// because nothing here would notice. The ceiling is what stays true either way,
// and it is the number a threshold has to stay UNDER, which is what `5c` checks
// once the number is in hand.
//
// The 32767-character CreateProcess command line is the mechanism; the script
// is only PART of that line, so the two numbers are reported separately.
function stageLength() {
  head("5. length — the largest script `-Command` will carry");
  const CAP = FALSIFY === "length" ? 4096 : 1 << 20;
  if (FALSIFY === "length") console.log(`  (falsified: search capped at ${CAP})`);

  // Padding is a single-quoted run of `a`, so nothing here needs escaping and
  // the script's length is exactly what we asked for.
  const script = (n) => {
    const head = "$__pad='";
    const tail = "'\n'LEN-OK'";
    const pad = Math.max(0, n - head.length - tail.length);
    return head + "a".repeat(pad) + tail;
  };
  const tryLen = (n) => {
    const text = wrap(script(n));
    const argv = argvCommand(text);
    const r = run(PWSH, argv);
    // Approximate, and deliberately labelled as such below: the real line adds
    // the quotes Node puts around the program path and the `-Command` argument.
    // It is within a handful of characters, which is all this needs to place the
    // boundary against Windows' 32767.
    const cmdline = [PWSH, ...argv].join(" ").length;
    return {
      n,
      cmdline,
      okay: !r.error && r.code === 0 && r.out.toString("utf8").includes("LEN-OK"),
      how: r.error ? `spawn error: ${r.error}` : `exit=${r.code} stderr: ${firstErrLine(r.err)}`,
    };
  };

  let lo = tryLen(1024);
  if (!lo.okay) {
    bad("5a a small script spawns", `1024 chars -> ${lo.how}`, "a 1 KiB script to run");
    return;
  }
  // Grow until it breaks, so the bracket is measured rather than assumed.
  let hi = null;
  let probe = 2048;
  while (probe <= CAP) {
    const r = tryLen(probe);
    if (!r.okay) {
      hi = r;
      break;
    }
    lo = r;
    probe *= 2;
  }
  if (!hi) {
    bad(
      "5a the `-Command` ceiling is reachable",
      `every size up to ${lo.n} chars ran (command line ${lo.cmdline} chars); search cap was ${CAP}`,
      "a size that fails — without one this stage measured no ceiling at all",
    );
    return;
  }
  // Binary search the boundary to within 64 chars: below that the number stops
  // meaning anything (the argv's own quoting shifts the command line).
  while (hi.n - lo.n > 64) {
    const mid = Math.floor((lo.n + hi.n) / 2);
    const r = tryLen(mid);
    if (r.okay) lo = r;
    else hi = r;
  }
  ok(
    "5a the `-Command` ceiling is reachable and bracketed",
    `largest script that ran: ${lo.n} chars (command line ~${lo.cmdline} chars)`,
  );
  ok(
    "5b above it the failure is reported, not silent",
    `${hi.n} chars (command line ~${hi.cmdline}) -> ${hi.how}`,
  );
  obsv(
    "5· against the Windows limit",
    `CreateProcess caps a command line at 32767 chars; the boundary measured ` +
      `~${lo.cmdline}-${hi.cmdline}, i.e. ~${32767 - lo.cmdline} chars of headroom at the ` +
      `last size that ran. The prologue+epilogue+argv cost ${lo.cmdline - lo.n} of that. ` +
      `Command-line lengths here are approximate (Node's own quoting is not counted); ` +
      `the SCRIPT sizes are exact.`,
  );

  // 5c — the threshold has to stay UNDER the ceiling.
  //
  // A size branch only helps if it fires BEFORE the spawn fails. Set at or above
  // the ceiling, the branch is a 恒真的谓词 in the direction that matters (判据
  // §2): every script that would have taken it has already failed to spawn, so
  // the route is never exercised and nothing goes red.
  //
  // The number is READ from `shell.rs`, never restated here — restating it would
  // make this a second copy of the threshold, which is the defect this whole
  // fixture is built to avoid (判据 §1). Which number it is depends on the tree:
  // the pwsh arm's own, when that arm has a size branch; otherwise the module's
  // only threshold, reported as such. Both are worth gating — the second is the
  // one such an arm would most plausibly reuse, and it is the one `invocation`'s
  // own doc comment relates to this ceiling.
  const t =
    FALSIFY === "threshold"
      ? { ...CONTRACT.threshold, value: lo.n + 1000, expr: "(falsified)", is_pwsh_arms_own: true }
      : CONTRACT.threshold;

  if (t.value === null) {
    // "I could not read it" is not "it is fine" (判据 §8).
    skip(
      "5c the pwsh stdin threshold stays under the measured ceiling",
      `\`${t.ident}\` is ${t.expr === null ? "not present in shell.rs" : `\`${t.expr}\`, which is not plain integer arithmetic`} — ` +
        `this fixture will not guess a number for it. Nothing was asserted about the threshold.`,
    );
  } else if (t.is_pwsh_arms_own) {
    check(
      "5c the pwsh stdin threshold stays under the measured ceiling",
      t.value < lo.n,
      `${t.ident} = ${t.value} vs a measured ceiling of ${lo.n} chars`,
      `a threshold below ${lo.n} — at or above it, a script that should take the ` +
        `stdin route fails to spawn first and the route is never reached`,
    );
  } else {
    // NOT gated, and the asymmetry is deliberate. The pwsh arm has no size
    // branch in this tree, so there is no threshold being misapplied and a red
    // here would be a guard crying wolf — and a guard that misfires is more
    // expensive than one that stays quiet, because it gets cited as evidence
    // (判据 §3). What IS worth saying is the number such an arm would have to
    // beat, so that adding one is not a coin flip. The check above turns itself
    // on the moment the arm gains a branch: `is_pwsh_arms_own` is read from the
    // arm, not configured here.
    const wouldFit = t.value < lo.n;
    obsv(
      "5c no pwsh stdin arm in this tree — the number one would have to beat",
      `measured ceiling ${lo.n} chars; the module's only threshold is ` +
        `${t.ident} = ${t.value} (${t.expr}). ` +
        (wouldFit
          ? `A pwsh stdin arm reusing it would fire before the ceiling. This line becomes a GATED check the moment such an arm exists.`
          : `⚠️ A pwsh stdin arm reusing it would be UNREACHABLE — every script big enough to take the route fails to spawn ${t.value - lo.n} chars earlier. A new arm needs its own, lower constant.`),
    );
  }
}

// ===========================================================================
// 6. profile — what `-NoProfile` costs (or saves)
// ===========================================================================
function stageProfile() {
  head(`6. profile — cold spawn time, ${SPAWN_SAMPLES} samples each`);
  const bogus = FALSIFY === "profile" ? ["-NoSuchSwitchHere"] : [];
  if (bogus.length) console.log(`  (falsified: spawning with ${bogus[0]})`);

  const sample = (noProfile) => {
    const args = [
      ...bogus,
      ...(noProfile ? ["-NoProfile"] : []),
      "-NonInteractive",
      "-Command",
      "'spawned'",
    ];
    const ms = [];
    let broke = null;
    let retries = 0;
    const attempt = () => {
      const r = run(PWSH, args);
      const good =
        !r.error && r.code === 0 && r.out.toString("utf8").includes("spawned");
      return { r, good };
    };
    for (let i = 0; i < SPAWN_SAMPLES; i += 1) {
      let a = attempt();
      if (!a.good) {
        // ONE retry, counted and reported. A spawn on this host occasionally
        // dies for reasons that are not pwsh's (a scanner holding the image,
        // say), and a 27-second fixture that cries wolf on one of those stops
        // being read — but a retry that is not COUNTED is a failure hidden
        // rather than survived, so the count rides along in the verdict string.
        retries += 1;
        const first = `${outcome(a.r)}  stderr: ${firstErrLine(a.r.err)}`;
        a = attempt();
        if (!a.good) {
          broke =
            `sample ${i + 1}/${SPAWN_SAMPLES} failed TWICE — ` +
            `first: ${first}; retry: ${outcome(a.r)}  stderr: ${firstErrLine(a.r.err)}`;
          break;
        }
      }
      ms.push(a.r.ms);
    }
    return { ms, broke, retries };
  };

  const stat = (ms) => ({
    mean: ms.reduce((a, b) => a + b, 0) / ms.length,
    min: Math.min(...ms),
    max: Math.max(...ms),
  });
  const show = (s) =>
    `mean ${s.mean.toFixed(0)} ms  (min ${s.min.toFixed(0)}, max ${s.max.toFixed(0)})`;

  const withNP = sample(true);
  const withProfile = sample(false);

  if (withNP.broke || withProfile.broke) {
    bad(
      "6a every timed spawn succeeded",
      withNP.broke ? `-NoProfile arm: ${withNP.broke}` : `profile arm: ${withProfile.broke}`,
      `${SPAWN_SAMPLES} clean spawns per arm — a timing measured over failed spawns is not a timing`,
    );
    return;
  }
  const a = stat(withNP.ms);
  const b = stat(withProfile.ms);
  const retried = (n) => (n ? `   ⚠️ ${n} spawn(s) needed one retry` : "");
  ok(
    `6a with \`-NoProfile\` (${SPAWN_SAMPLES} spawns)`,
    show(a) + retried(withNP.retries),
  );
  ok(
    `6b without \`-NoProfile\` (${SPAWN_SAMPLES} spawns)`,
    show(b) + retried(withProfile.retries),
  );
  // Gated, and the gate is the design decision: `-NoProfile` is in the argv to
  // save time. If it cost time here, that argv would be unjustified.
  check(
    "6c `-NoProfile` is not slower",
    a.mean <= b.mean,
    `${a.mean.toFixed(0)} ms with vs ${b.mean.toFixed(0)} ms without ` +
      `(${(b.mean / a.mean).toFixed(1)}x)`,
    "the -NoProfile arm to be the faster one",
  );
  // What the profile arm was actually paying for. A host with no profile files
  // at all would make 6c a coin flip, and that is worth seeing next to it.
  const profiles = run(PWSH, [
    "-NonInteractive",
    "-Command",
    "($PROFILE.AllUsersAllHosts,$PROFILE.AllUsersCurrentHost,$PROFILE.CurrentUserAllHosts,$PROFILE.CurrentUserCurrentHost) | ForEach-Object { if (Test-Path $_) { $_ } }",
  ])
    .out.toString("utf8")
    .split(/\r?\n/)
    .map((l) => l.trim())
    .filter(Boolean);
  obsv(
    "6· the profile files that exist on this host",
    profiles.length ? profiles.join("   ") : "(none — 6c's gap is not profile loading)",
  );
}

// ===========================================================================
// 7. env — a child with only PATH
// ===========================================================================
//
// The sandbox drivers `env_clear()` and then set an explicit list, so that list
// IS the child's environment — anything absent from it is absent in the child.
// Reproducing that shape is not as simple as `spawnSync({env})`; see the
// instrument note above `LAUNCHER_PS` below.
const ENV_SCRIPT = [
  '"PATHEXT=" + $env:PATHEXT',
  '"TEMP=" + $env:TEMP',
  // Module-free on purpose: without PSModulePath a stripped child may not be
  // able to load Microsoft.PowerShell.Management, so `Get-ChildItem env:` could
  // fail for a reason that is not the one under test.
  "$k=@(); foreach ($e in [System.Environment]::GetEnvironmentVariables().GetEnumerator()) { $k += $e.Key }",
  '"KEYS=" + ($k -join ",")',
].join("\n");

// ⚠️ The instrument, and why it is not `spawnSync({env})`.
//
// Rust's `Command::env_clear().envs(list)` hands CreateProcessW exactly `list`.
// Node's does NOT: libuv's `make_program_env` copies eleven names out of the
// PARENT when the supplied block lacks them — HOMEDRIVE, HOMEPATH, LOGONSERVER,
// PATH, SYSTEMDRIVE, SYSTEMROOT, TEMP, USERDOMAIN, USERNAME, USERPROFILE,
// WINDIR. MEASURED here 2026-09-05: a `spawnSync(pwsh, {env:{PATH}})` child saw
// 13 variables, and `TEMP` was one of them — so a naive Node probe reports the
// stripped child as HEALTHIER than the product's own child would be, which is
// the direction that turns a real defect green.
//
// So both arms go through a LAUNCHER: a normal pwsh that builds a
// `ProcessStartInfo`, calls `.Environment.Clear()`, sets exactly the names we
// asked for and starts the real child. That is the same .NET call Rust's
// `env_clear` compiles down to. The values travel in the launcher's own
// environment rather than its argv, so nothing here needs quoting.
const LAUNCHER_PS = [
  "$psi = New-Object System.Diagnostics.ProcessStartInfo",
  "$psi.FileName = $env:QA_CHILD_EXE",
  'foreach ($a in ($env:QA_CHILD_FLAGS -split "`n")) { if ($a) { [void]$psi.ArgumentList.Add($a) } }',
  "[void]$psi.ArgumentList.Add($env:QA_CHILD_SCRIPT)",
  "$psi.UseShellExecute = $false",
  "$psi.RedirectStandardOutput = $true",
  "$psi.RedirectStandardError = $true",
  "$psi.Environment.Clear()",
  // One `NAME=VALUE` per line. No value we pass contains a newline (PATH and
  // friends cannot), and the script — which does — travels separately above.
  'foreach ($kv in ($env:QA_CHILD_ENV -split "`n")) {',
  "  if ($kv) { $i = $kv.IndexOf('='); $psi.Environment[$kv.Substring(0,$i)] = $kv.Substring($i+1) }",
  "}",
  "$p = [System.Diagnostics.Process]::Start($psi)",
  // Sequential ReadToEnd can deadlock on a child that fills the other pipe.
  // Safe here: the child prints three short lines. Not a pattern to copy.
  "$o = $p.StandardOutput.ReadToEnd()",
  "$e = $p.StandardError.ReadToEnd()",
  "$p.WaitForExit()",
  "[Console]::Out.Write($o)",
  "[Console]::Error.Write($e)",
  "exit $p.ExitCode",
].join("\n");

/** Run ENV_SCRIPT in a child whose environment is EXACTLY `childEnv`. */
function envArm(childEnv) {
  const r = run(PWSH, argvCommand(LAUNCHER_PS), {
    env: {
      ...process.env,
      QA_CHILD_EXE: PWSH,
      QA_CHILD_FLAGS: CONTRACT.flags.join("\n"),
      QA_CHILD_SCRIPT: wrap(ENV_SCRIPT),
      QA_CHILD_ENV: Object.entries(childEnv)
        .map(([k, v]) => `${k}=${v}`)
        .join("\n"),
    },
  });
  const txt = r.out.toString("utf8");
  const field = (k) => {
    const m = txt.split(/\r?\n/).find((l) => l.startsWith(`${k}=`));
    return m === undefined ? null : m.slice(k.length + 1).trim();
  };
  const keys = (field("KEYS") ?? "").split(",").filter(Boolean).sort();
  return { r, pathext: field("PATHEXT"), temp: field("TEMP"), keys, txt };
}

/** The same question asked through Node's own spawn, to show the difference. */
function envArmViaNode(childEnv) {
  const r = run(PWSH, argvCommand(wrap(ENV_SCRIPT)), { env: childEnv });
  const txt = r.out.toString("utf8");
  const line = (k) => {
    const m = txt.split(/\r?\n/).find((l) => l.startsWith(`${k}=`));
    return m === undefined ? null : m.slice(k.length + 1).trim();
  };
  return {
    r,
    pathext: line("PATHEXT"),
    temp: line("TEMP"),
    keys: (line("KEYS") ?? "").split(",").filter(Boolean).sort(),
  };
}

function stageEnv() {
  head("7. env — what an env_clear()ed child is missing");
  if (FALSIFY === "env")
    console.log("  (falsified: the full-environment arm passes only PATH)");

  const onlyPath = { PATH: process.env.PATH ?? "" };
  const minimal = envArm(onlyPath);
  obsv(
    "7· child with only PATH (ProcessStartInfo.Environment.Clear — the product's shape)",
    `${outcome(minimal.r)}  PATHEXT=${JSON.stringify(minimal.pathext)}  ` +
      `TEMP=${JSON.stringify(minimal.temp)}  saw ${minimal.keys.length} variables: ${minimal.keys.join(",") || "(none)"}`,
  );

  // The same question through Node's own spawn, reported so the difference is
  // on the record rather than in someone's memory. libuv puts eleven names back;
  // Rust's `env_clear` does not. A probe that used only this would report the
  // stripped child as healthier than production's.
  const viaNode = envArmViaNode(onlyPath);
  const injected = viaNode.keys.filter((k) => !minimal.keys.includes(k));
  obsv(
    "7· the same child through Node's spawnSync({env}) — a DIFFERENT instrument",
    `saw ${viaNode.keys.length} variables (vs ${minimal.keys.length}); ` +
      `TEMP=${JSON.stringify(viaNode.temp)}; libuv put back: ${injected.join(",") || "(nothing)"}`,
  );
  if (minimal.pathext === null) {
    bad(
      "7a a PATH-only child answered at all",
      `${outcome(minimal.r)}  stdout=${JSON.stringify(minimal.txt.slice(0, 200))}  stderr: ${firstErrLine(minimal.r.err)}`,
      "a PATHEXT= line — even a crippled child should print one",
    );
  } else {
    const hasExe = /(^|;)\s*\.EXE\s*(;|$)/i.test(minimal.pathext);
    // PREMISE: this is why the Windows list exists. If a PATH-only child were
    // already healthy, `WINDOWS_PASS_ENV` would be unjustified on this host.
    check(
      "7a a PATH-only child is crippled",
      !hasExe || !minimal.temp,
      `PATHEXT=${JSON.stringify(minimal.pathext)} (.EXE ${hasExe ? "present" : "ABSENT"}), TEMP=${JSON.stringify(minimal.temp)}`,
      "`.EXE` missing from PATHEXT, or an empty TEMP — otherwise WINDOWS_PASS_ENV buys nothing here",
    );
  }

  // The full list, DERIVED from code_exec.rs. Names absent from this process's
  // own environment are simply not passed — the same thing the product does.
  const names =
    FALSIFY === "env" ? ["PATH"] : [...PASSENV.posix, ...PASSENV.windows];
  const full = {};
  const unset = [];
  for (const n of names) {
    if (process.env[n] === undefined) unset.push(n);
    else full[n] = process.env[n];
  }
  const rich = envArm(full);
  obsv(
    "7· child with the derived list",
    `${names.length} names from ${path.basename(PASSENV.source)}` +
      `${unset.length ? ` (${unset.length} unset on this host: ${unset.join(",")})` : ""}` +
      `  ->  saw ${rich.keys.length} variables`,
  );
  const richHasExe = /(^|;)\s*\.EXE\s*(;|$)/i.test(rich.pathext ?? "");
  check(
    "7b PATHEXT reaches the child intact",
    richHasExe,
    `PATHEXT=${JSON.stringify(rich.pathext)}`,
    "a PATHEXT containing .EXE",
  );
  check(
    "7c TEMP reaches the child",
    Boolean(rich.temp),
    `TEMP=${JSON.stringify(rich.temp)}`,
    "a non-empty TEMP",
  );
}

// ===========================================================================
// Drive
// ===========================================================================
const RUNNERS = {
  resolve: stageResolve,
  encoding: stageEncoding,
  exit: stageExit,
  comment: stageComment,
  length: stageLength,
  profile: stageProfile,
  env: stageEnv,
};

// Assertion FLOOR — how many gated checks each stage makes when it runs to the
// end, MEASURED on the clean 2026-09-05 run. A fixture is also code that can
// stop working without saying so: a stage whose checks all vanished prints
// `0 passed, 0 failed` and exits 0, which reads exactly like a pass. Adding a
// check raises its floor in the same commit.
//
// OBSV lines are deliberately not counted: they are observations, and counting
// them would let an observation stand in for an assertion.
// `length` is the one stage whose count depends on the tree rather than on this
// file: `5c` is a gated check only when the pwsh arm HAS a size branch, and an
// observation otherwise. Derived from the same field the check reads, so the
// floor cannot disagree with the stage it is guarding.
const FLOORS = {
  resolve: 2,
  encoding: 3,
  exit: 3,
  comment: 2,
  length: 2 + (CONTRACT.threshold.is_pwsh_arms_own ? 1 : 0),
  profile: 3,
  env: 3,
};

console.log(`\n=== qa/winshell — PowerShell contract probe (${STAGE}) ===`);
console.log(`  host      ${process.platform} ${process.arch}, node ${process.version}`);
console.log(`  contract  ${CONTRACT.source}`);
console.log(`  argv      ${CONTRACT.flags.join(" ")} <prologue+script+epilogue>`);
if (FALSIFY) console.log(`  FALSIFY   ${FALSIFY}`);

const wanted = STAGE === "all" ? STAGES : [STAGE];

// Every later stage needs the path `resolve` found; without it they cannot ask,
// and "could not ask" is a SKIP, never a pass (判据 §8).
const resolved = stageResolve();
if (!resolved) {
  for (const s of wanted.filter((s) => s !== "resolve")) {
    skip(s, "pwsh did not resolve — stage 1 above says where it looked");
  }
} else {
  for (const s of wanted.filter((s) => s !== "resolve")) RUNNERS[s]();
}

console.log(`\n=== ${PASS} passed, ${FAIL} failed, ${SKIP} skipped ===`);

// The floor only means anything when the stages ran unmodified: falsification
// makes stages return early on purpose, and SKIPs mean they never ran at all.
let belowFloor = false;
if (!FALSIFY && SKIP === 0) {
  const want =
    FLOORS.resolve +
    wanted.filter((s) => s !== "resolve").reduce((a, s) => a + FLOORS[s], 0);
  if (PASS + FAIL < want) {
    belowFloor = true;
    console.log(
      `\n!!! ASSERTION FLOOR: ${PASS + FAIL} gated checks ran, ${want} expected for ` +
        `\`${STAGE}\`. Some check vanished — a stage that asserts nothing prints a ` +
        `green summary too. Whatever passed above is not evidence until this is ` +
        `explained (FLOORS in this file holds the per-stage numbers).`,
    );
  }
}
process.exit(FAIL === 0 && !belowFloor ? 0 : 1);
