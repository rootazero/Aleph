// Derive the PowerShell invocation contract from the Rust source that owns it.
//
// This fixture probes whether the strings Aleph actually wraps around every
// script do what their comments claim. If those strings were hand-copied into
// the fixture, the fixture would be measuring a SECOND copy — and a second copy
// drifts silently the first time someone edits the first one (判据 §1). So the
// prologue, the epilogue, the argv flags and — load-bearing for the `comment`
// stage — the SEPARATORS the three are joined with, all come out of
// `src/utils/shell.rs` here. The Windows environment allowlist comes out of
// `src/builtin_tools/code_exec.rs` for the same reason.
//
// The one thing deliberately NOT derived is where `pwsh` lives: the `resolve`
// stage walks PATH itself. Deriving that would make the fixture agree with the
// code by construction, which is the failure mode this whole file exists to
// avoid pointing the wrong way.
//
// A shape this cannot parse is a HARD ERROR, never a silent default. A default
// prologue would be a second copy wearing a disguise, and every stage
// downstream would go green against a string the product does not use.
//
// Usage (also importable):  node derive_ps_contract.mjs <repo-root>
import fs from "node:fs";
import path from "node:path";
import { pathToFileURL } from "node:url";

/** Unescape one Rust string literal's body. */
const unescapeRust = (s) =>
  s.replace(/\\(u\{([0-9a-fA-F]+)\}|x([0-9a-fA-F]{2})|.)/g, (_m, all, u, x) => {
    if (u) return String.fromCodePoint(parseInt(u, 16));
    if (x) return String.fromCharCode(parseInt(x, 16));
    switch (all) {
      case "n":
        return "\n";
      case "r":
        return "\r";
      case "t":
        return "\t";
      case "0":
        return "\0";
      default:
        return all; // \\ \" \' and anything else stands for itself
    }
  });

/** Every `"…"` literal inside `src`, in order, unescaped. */
const literals = (src) =>
  [...src.matchAll(/"((?:[^"\\]|\\.)*)"/g)].map((m) => unescapeRust(m[1]));

/**
 * `const NAME: &str = …;` in any of the three spellings this module might use:
 * `concat!("a", "b")`, a plain `"…"`, or a raw `r#"…"#`.
 *
 * Three forms rather than one because the prologue is under active edit (a
 * `try { } catch { }` wrapper is on its way in), and a deriver that only knows
 * today's spelling turns this fixture red for a change that IMPROVES the thing
 * it tests. Still a hard error when none of them match — a default would be a
 * second copy of the contract wearing a disguise.
 */
function constStr(src, name) {
  const concat = src.match(
    new RegExp(`const ${name}: &str = concat!\\(([\\s\\S]*?)\\n\\);`),
  );
  if (concat) {
    const parts = literals(concat[1]);
    return parts.length ? parts.join("") : null;
  }
  // `r#"…"#` / `r##"…"##`: no escapes inside, so take the body verbatim.
  const raw = src.match(new RegExp(`const ${name}: &str = r(#+)"([\\s\\S]*?)"\\1;`));
  if (raw) return raw[2];
  const plain = src.match(
    new RegExp(`const ${name}: &str = "((?:[^"\\\\]|\\\\.)*)";`),
  );
  return plain ? unescapeRust(plain[1]) : null;
}

/**
 * `const NAME: usize = 32 * 1024;` -> 32768.
 *
 * Only integer arithmetic is accepted. A value that references another const
 * (or anything else non-numeric) returns `{ expr }` with no `value`, so the
 * caller reports "could not resolve" rather than inventing a number — a
 * threshold this fixture guessed at would be worse than no threshold check.
 */
function constUsize(src, name) {
  const m = src.match(new RegExp(`const ${name}: usize = ([^;]+);`));
  if (!m) return null;
  const expr = m[1].trim();
  if (!/^[0-9_+*\-()\s]+$/.test(expr)) return { expr, value: null };
  try {
    // Validated above as digits and arithmetic only — no identifiers, no calls.
    const value = Function(`"use strict";return (${expr.replace(/_/g, "")})`)();
    return Number.isSafeInteger(value) ? { expr, value } : { expr, value: null };
  } catch {
    return { expr, value: null };
  }
}

/** `const NAME: &[&str] = &[ … ];` -> the string list, comments stripped. */
function constStrSlice(src, name) {
  const m = src.match(
    new RegExp(`const ${name}: &\\[&str\\] = &\\[([\\s\\S]*?)\\n?\\];`),
  );
  if (!m) return null;
  // Strip `//` comment tails first: a comment can contain a quoted word
  // (`PowerShell's \`.EXE\``) and would otherwise contribute a phantom name.
  const body = m[1]
    .split("\n")
    .map((l) => l.replace(/\/\/.*$/, ""))
    .join("\n");
  const names = literals(body);
  return names.length ? names : null;
}

/**
 * The PowerShell arm of `ShellKind::invocation`: the flags before `-Command`,
 * and the two separators in `format!("{PS_PROLOGUE}<a>{script}<b>{PS_EPILOGUE}")`.
 *
 * `sep_after_script` is what the `comment` stage is about. The claim in
 * shell.rs is "joined with newlines, never `;`" — a claim about THIS character,
 * so the fixture has to read it rather than assume it.
 */
function psArm(src) {
  // `=> (` today, `=> {` once the arm grows a size branch. Matching only the
  // first would make this fixture break on the day the second lands — loudly,
  // but for a fixture reason rather than a product one.
  const open = src.match(/Self::Pwsh \| Self::WindowsPowerShell => [({]/);
  if (!open) return null;
  const start = open.index;
  const endM = src.slice(start).match(/Self::Cmd => [({]/);
  const arm = src.slice(start, endM ? start + endM.index : src.length);
  // Deduped: with a size branch both routes spell the same three flags, and the
  // `-Command` route's list is what stage 5 measures the ceiling of.
  const flags = [
    ...new Set([...arm.matchAll(/"(-[A-Za-z]+)"\.to_string\(\)/g)].map((m) => m[1])),
  ];
  const fmt = arm.match(
    /format!\("\{PS_PROLOGUE\}((?:[^"\\]|\\.)*)\{script\}((?:[^"\\]|\\.)*)\{PS_EPILOGUE\}"\)/,
  );
  if (!flags.length || !fmt) return null;
  // The size branch, if this arm has one. Today it does not — PowerShell stays
  // on `-Command` at every size — but that is a decision under active review,
  // so this reads the arm rather than assuming either answer. `null` means "no
  // branch here", which is a different report from "a branch I could not read".
  const branch = arm.match(/script\.len\(\)\s*>\s*([A-Z_][A-Z0-9_]*)/);
  return {
    flags,
    sep_before_script: unescapeRust(fmt[1]),
    sep_after_script: unescapeRust(fmt[2]),
    stdin_threshold_ident: branch ? branch[1] : null,
  };
}

/**
 * Everything the probe needs, read out of the repo at `repo`.
 * Throws with the file and the symbol that could not be parsed.
 */
export function deriveShellContract(repo) {
  const shellRs = path.join(repo, "src", "utils", "shell.rs");
  const src = fs.readFileSync(shellRs, "utf8");
  const prologue = constStr(src, "PS_PROLOGUE");
  const epilogue = constStr(src, "PS_EPILOGUE");
  const arm = psArm(src);
  const missing = [
    prologue ? null : "PS_PROLOGUE",
    epilogue ? null : "PS_EPILOGUE",
    arm ? null : "ShellKind::invocation's Pwsh arm",
  ].filter(Boolean);
  if (missing.length) {
    throw new Error(
      `cannot parse ${missing.join(", ")} out of ${shellRs} — ` +
        `the source's shape changed. This is a BROKEN FIXTURE, not a ` +
        `failing contract: fix the parser rather than reading any verdict ` +
        `below it.`,
    );
  }
  // The threshold at which a script would leave `-Command`. Resolved from the
  // ident the arm itself compares against; when the arm has no branch, the
  // module's only threshold is reported instead, FLAGGED as such — those two
  // are different facts and stage 5 says which one it got.
  const ident = arm.stdin_threshold_ident ?? "STDIN_PIPE_THRESHOLD";
  const resolved = constUsize(src, ident);
  const threshold = {
    ident,
    // False when the pwsh arm has no size branch and this is the bash one.
    is_pwsh_arms_own: arm.stdin_threshold_ident !== null,
    expr: resolved?.expr ?? null,
    value: resolved?.value ?? null,
  };
  return { source: shellRs, prologue, epilogue, threshold, ...arm };
}

/**
 * The environment names the sandbox rebuilds for a Windows child. The drivers
 * `env_clear()` first, so this list IS the child's environment.
 */
export function derivePassEnv(repo) {
  const codeExecRs = path.join(repo, "src", "builtin_tools", "code_exec.rs");
  const src = fs.readFileSync(codeExecRs, "utf8");
  const posix = constStrSlice(src, "POSIX_PASS_ENV");
  const windows = constStrSlice(src, "WINDOWS_PASS_ENV");
  if (!posix || !windows) {
    throw new Error(
      `cannot parse ${!posix ? "POSIX_PASS_ENV" : "WINDOWS_PASS_ENV"} out of ` +
        `${codeExecRs} — the source's shape changed. BROKEN FIXTURE.`,
    );
  }
  return { source: codeExecRs, posix, windows };
}

// CLI: print what was derived, so a parser break is diagnosable on its own.
// `pathToFileURL`, not a hand-built `file://…`: on Windows the two spellings
// differ by one slash (`file:///D:/…`) and the guard would never fire.
if (
  process.argv[1] &&
  import.meta.url === pathToFileURL(process.argv[1]).href
) {
  const repo = process.argv[2];
  if (!repo) {
    console.error("usage: derive_ps_contract.mjs <repo-root>");
    process.exit(2);
  }
  const show = (s) => JSON.stringify(s);
  try {
    const c = deriveShellContract(repo);
    const e = derivePassEnv(repo);
    console.log(`source        ${c.source}`);
    console.log(`flags         ${c.flags.join(" ")}`);
    console.log(`sep(pro,scr)  ${show(c.sep_before_script)}`);
    console.log(`sep(scr,epi)  ${show(c.sep_after_script)}`);
    console.log(
      `threshold     ${c.threshold.ident} = ${c.threshold.expr ?? "(absent)"}` +
        ` -> ${c.threshold.value ?? "UNRESOLVED"}` +
        `  (${c.threshold.is_pwsh_arms_own ? "the pwsh arm's own" : "the pwsh arm has NO size branch; this is the bash one"})`,
    );
    console.log(`prologue      ${show(c.prologue)}`);
    console.log(`epilogue      ${show(c.epilogue)}`);
    console.log(`source        ${e.source}`);
    console.log(`posix env     ${e.posix.join(" ")}`);
    console.log(`windows env   ${e.windows.join(" ")}`);
  } catch (err) {
    console.error(String(err.message ?? err));
    process.exit(1);
  }
}
