//! Cross-cutting guards for `/btw`: the invariants no single call site owns.
//!
//! The module's other tests exercise behaviour — this file scans source,
//! because every failure it exists to catch is a *second answer* to a question
//! that already has one, and two answers that agree today are indistinguishable
//! from one answer at runtime. Only the text can tell them apart.
//!
//! # What is already pinned elsewhere, and is therefore not repeated here
//!
//! Each citation states what the cited guard *actually* pins, which in two
//! cases is narrower than its name suggests. A citation that overstates its
//! referent is worse than no citation: it is how a gap gets counted as covered.
//!
//! * `interfaces/tui/.../commands.rs::this_client_resolves_a_side_question_in_exactly_one_place`
//!   — that crate calls [`BtwTurn::resolve`](aleph_protocol::btw::BtwTurn::resolve)
//!   exactly once. Two limits: it counts *calls to the resolver*, so it cannot
//!   see a hand-rolled prefix test that never calls one (that half is
//!   [`only_the_shared_resolver_decides_what_a_side_question_is`] below); and it
//!   splits on the bare `#[cfg(test)]`, which in that crate cuts
//!   `tui/btw_overlay.rs` at line 97 of 853 and `tui/keys.rs` at 749 of 1510 —
//!   the two most `/btw`-relevant files in it — so a second resolver call below
//!   either cut keeps its count at one.
//! * `execution_engine/btw_wire_tests.rs::no_shipped_command_word_resolves_as_a_side_question`
//!   — no catalog command word resolves *as* a side question (behavioural).
//! * `execution_engine/slash_command.rs::stamp_btw_runs_before_admission_and_is_never_gated_on_slash_command_mode_key`
//!   — `execute()` stamps before `admit_run` and outside the slash gate.
//! * `execution_engine/slash_command.rs::every_run_start_handler_stamps_the_slash_mode_before_the_busy_lane`
//!   — phrased over whichever `pub async fn handle_*` spawn, **but only inside
//!   `src/bin/aleph-server/server_init.rs`**, which is a hardcoded path. A
//!   run-start handler added in another file inherits nothing from it. Where it
//!   does apply, `stamp_slash_mode` calls `stamp_btw` first, so a handler that
//!   satisfies it also stamps `btw`.
//! * `continuation_lifecycle.rs::every_epoch_bump_or_content_wipe_reaches_a_side_session_retirement`
//!   — retirement reaches every surface that rolls or wipes a conversation.
//! * `inbound_router/command_handler.rs` — `/btw` stays on the router-owned
//!   help lines, the only place a channel user can learn the verb exists.
//!
//! Taken together the positive half — "every surface that could carry a side
//! question resolves it through the one resolver" — is pinned for `execute()`
//! and for handlers in one file. Nothing here closes the rest of it.
//!
//! # Why this file has its own scanner instead of the shared one
//!
//! `gateway::source_census::production_prefix` is `pub(crate)` and is the
//! repo's nearest thing to a single source; six further private copies exist,
//! and [`production_source`] here is a seventh. That is a real cost
//! (「这个问题在仓里已经有几个答案」), so the reason has to be precise rather
//! than a preference. The shared one differs in three ways, each of which would
//! weaken a guard below:
//!
//! 1. it cuts at the **bare** `#[cfg(test)]`, so a `#[cfg(test)] use …` near the
//!    top of a file blinds the scan to everything under it — the failure the
//!    precedent's own doc calls worse than none;
//! 2. it **drops** comment lines rather than blanking them, so every line number
//!    it could report is counted in a file nobody has (see [`production_source`]);
//! 3. it does not step over a visibility qualifier, so `#[cfg(test)]
//!    pub(crate) mod tests` is read as production.
//!
//! Fixes 1 and 2 could be pushed down into the shared helper — every existing
//! caller wants them, and both widen or preserve what is read. Fix 3 cannot go
//! with them: it makes twelve files under `src/` cut **earlier**, i.e. read
//! *less* text, and one of those files feeding a census that currently passes on
//! the wider view could go green by blindness. Migrating therefore means
//! re-measuring every census that depends on the shared helper, which is a
//! change of its own and not one this task can honestly land. Recorded as a
//! follow-up rather than as a comment.

use std::path::{Path, PathBuf};

// ---------------------------------------------------------------------------
// Shared scanning primitives
// ---------------------------------------------------------------------------

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

/// Read or die. A guard that *skips* an unreadable path is a guard that goes
/// green when its subject moves, which is the failure mode these exist to
/// prevent, one level up.
fn read_file(path: &Path) -> String {
    std::fs::read_to_string(path).unwrap_or_else(|e| panic!("{}: {e}", path.display()))
}

/// Every crate source root in this workspace, derived from `Cargo.toml`'s
/// `members` list plus the root package's own `src`.
///
/// Derived rather than listed: a `/btw` affordance can be added to any crate
/// that ships a surface, and a new crate joining the workspace is exactly the
/// case a hand-written list of directory names cannot fail on.
fn workspace_source_roots() -> Vec<PathBuf> {
    let root = repo_root();
    let manifest = read_file(&root.join("Cargo.toml"));
    let list = manifest
        .split_once("members = [")
        .expect("the workspace manifest still declares `members = [`")
        .1
        .split_once(']')
        .expect("the members list is still closed")
        .0;

    let mut roots = vec![root.join("src")];
    for member in list.split('"').skip(1).step_by(2) {
        let src = root.join(member).join("src");
        if src.is_dir() {
            roots.push(src);
        }
    }
    assert!(
        roots.len() >= 10,
        "only {} crate source root(s) resolved from the workspace manifest — \
         the members list stopped parsing, and a scan of almost nothing reports \
         the same green as a scan of everything",
        roots.len()
    );
    roots
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let entries = std::fs::read_dir(dir).unwrap_or_else(|e| panic!("{}: {e}", dir.display()));
    for entry in entries {
        let path = entry.expect("a readable dir entry").path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|e| e == "rs") {
            out.push(path);
        }
    }
}

/// Files these rules cannot speak about, decided by what the file *is* rather
/// than by what it currently contains.
///
/// Test code only — this file included, which is why it is named
/// `guard_tests.rs`: it holds every literal these guards hunt for, and a
/// scanner that reads its own hunting ground is how a lexical guard certifies
/// the thing it was written to catch. Naming it out by hand would have been a
/// licence; matching the repo's existing structural spelling is a rule.
fn is_test_only(rel: &str) -> bool {
    rel.ends_with("/tests.rs")
        || rel.contains("/tests/")
        || rel.ends_with("_tests.rs")
        || rel.contains("/test_utils")
}

/// Byte offset of the first `#[cfg(test)]` whose next item is a `mod`, with an
/// optional visibility qualifier stepped over (`pub`, `pub(crate)`,
/// `pub(super)`), or `None`.
///
/// Not the first `#[cfg(test)]` of any kind: a `#[cfg(test)] use …` near the
/// top of a long file would cut it to nothing and leave the scanner blind to
/// the production code below — a green that means "I cannot see you".
fn test_module_offset(src: &str) -> Option<usize> {
    const ATTR: &str = "#[cfg(test)]";
    let mut cursor = 0;
    while let Some(hit) = src[cursor..].find(ATTR) {
        let at = cursor + hit;
        let mut rest = src[at + ATTR.len()..].trim_start();
        if let Some(after_pub) = rest.strip_prefix("pub") {
            rest = after_pub.trim_start();
            if rest.starts_with('(') {
                rest = rest
                    .split_once(')')
                    .map_or(rest, |(_, tail)| tail.trim_start());
            }
        }
        if rest.starts_with("mod ") {
            return Some(at);
        }
        cursor = at + ATTR.len();
    }
    None
}

/// The part of a file that ships: CR stripped (a CRLF checkout makes
/// `\n`-anchored splits match nothing), comment lines **blanked**, and then the
/// tests module dropped. The "the only hit is the comment explaining the rule"
/// trap is the standard way a scan like this reports green.
///
/// # Blanked rather than dropped
///
/// Every scanner this is modelled on deletes the line, so any line number they
/// report is counted in a file nobody has. `slash_command.rs:903` is the live
/// instance in this repo — it enumerates a comment-dropped text and prints
/// `{rel}:{n+1}`. Blanking costs one empty string per comment and keeps
/// `n + 1` meaning what the editor means.
///
/// # Blanked *before* the cut, which is the whole point of the ordering
///
/// A comment is not only a place a match can hide; it is a place the **cut**
/// can hide. One production line spelled
/// ``//! the test module below is `#[cfg(test)] mod tests;` `` truncates that
/// file's production view to whatever sits above it, and every guard in this
/// module then reports green on a file it can no longer see. Not hypothetical:
/// planting exactly that comment plus a hand-rolled `/btw` predicate in
/// `inbound_router/executor.rs` passed all four guards, and this module's own
/// doc contained such a line.
///
/// Cutting the blanked text closes it, and the ordering *is* the fix: a scanner
/// that blanks (or drops) comments after computing the offset still has the
/// hole. Nothing downstream can see this class of failure — the file is still
/// counted, just emptied — so the order has to be right rather than checked.
fn production_source(raw: &str) -> String {
    let blanked = raw
        .replace('\r', "")
        .lines()
        .map(|l| {
            if l.trim_start().starts_with("//") {
                ""
            } else {
                l
            }
        })
        .collect::<Vec<_>>()
        .join("\n");
    match test_module_offset(&blanked) {
        Some(at) => blanked[..at].to_string(),
        None => blanked,
    }
}

/// Panic unless the scan is reading a whole tree rather than a fragment of one.
///
/// Two numbers, because they fail differently. The **file count** catches a walk
/// that stopped walking. The **byte count** catches a walk that still visits
/// every file but reads almost nothing out of them: a cut regressing, a
/// separator that stopped matching, `is_test_only` over-matching. A file-count
/// floor is structurally blind to that, because the files are all still counted,
/// just emptied. Neither number can see a *single* file being truncated; only
/// getting [`production_source`]'s ordering right does.
fn assert_the_scan_read_the_tree(sources: &[(String, String)]) {
    let bytes: usize = sources.iter().map(|(_, src)| src.len()).sum();
    assert!(
        sources.len() > 2_000 && bytes > 10_000_000,
        "the scan read {} shipping file(s) / {bytes} byte(s) of production \
         source, against a workspace of roughly 2 900 files and 18 MB. A scan \
         of almost nothing reports the same green as a scan of everything",
        sources.len()
    );
}

/// `(relative path, production source)` for every shipping `.rs` in the
/// workspace.
fn workspace_production_sources() -> Vec<(String, String)> {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in workspace_source_roots() {
        rust_files(&dir, &mut files);
    }
    files
        .into_iter()
        .filter_map(|path| {
            let rel = path
                .strip_prefix(&root)
                .unwrap_or(&path)
                .to_string_lossy()
                .replace('\\', "/");
            (!is_test_only(&rel)).then(|| (rel, production_source(&read_file(&path))))
        })
        .collect()
}

/// Every `"…"` string literal on `line`, with backslash escapes reduced to the
/// escaped character (enough to compare a command word, not a lexer).
fn string_literals(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] != '"' {
            i += 1;
            continue;
        }
        let mut buf = String::new();
        let mut j = i + 1;
        while j < chars.len() && chars[j] != '"' {
            if chars[j] == '\\' && j + 1 < chars.len() {
                buf.push(chars[j + 1]);
                j += 2;
                continue;
            }
            buf.push(chars[j]);
            j += 1;
        }
        out.push(buf);
        i = j + 1;
    }
    out
}

// ---------------------------------------------------------------------------
// Guard 2 — one resolver decides what a side question is
// ---------------------------------------------------------------------------

/// A literal's content read as a possible spelling of the command word.
///
/// Trimmed, `@botname` suffix cut, leading `/` stripped, lowercased — the four
/// transformations [`BtwTurn::resolve`](aleph_protocol::btw::BtwTurn::resolve)
/// itself performs. A literal that survives them as exactly `btw` is a spelling
/// of the verb, which is what a hand-rolled predicate compares against;
/// anything longer (`/btw promote`, `/btw {body}`, a help sentence) is text
/// being *emitted*, not a value being tested.
fn as_command_word(literal: &str) -> String {
    let s = literal.trim();
    let s = s.split('@').next().unwrap_or(s);
    let s = s.strip_prefix('/').unwrap_or(s);
    s.to_ascii_lowercase()
}

/// Only [`BtwTurn::resolve`](aleph_protocol::btw::BtwTurn::resolve) decides
/// whether an input is a side question.
///
/// The regression this exists to catch is a surface that answers the question
/// itself — `input.starts_with("/btw")` in the router, in a channel adapter, in
/// a client. Such code agrees with the resolver on the day it is written and
/// drifts the first time the server learns a spelling (`/BTW`, `/btw@bot`, the
/// empty body that is deliberately *not* a side question). Runtime cannot tell
/// the two apart — both say yes to `/btw x`.
///
/// # Why it hunts the *word* rather than a list of comparison spellings
///
/// A rule keyed on `starts_with` / `==` / `strip_prefix` / a match arm is a
/// list, and the spelling it does not know about is the one that ships. What
/// every hand-rolled predicate must contain, in any spelling, is the verb
/// itself as a literal. Emitting code embeds the verb in a longer string;
/// comparing code cannot. So the scan is for the *shape of the literal*, and it
/// is blind to no comparison operator.
///
/// # The two sanctioned occurrences, derived rather than listed
///
/// * the file that *defines* the resolver (the one carrying both `impl BtwTurn`
///   and `pub fn resolve`), where the comparison is the single answer, and
/// * a `const … : &str = "btw"` **definition**, recognised by the shape of the
///   line itself — that is `BTW_METADATA_KEY`, the metadata stamp's own single
///   source, which is a name for a map key and not a test of an input.
///
/// # Known limits
///
/// Three, all of them predicates that never write the word:
///
/// * one comparing against a constant imported from elsewhere. That constant's
///   own definition is visible — it is a `const … = "btw"` line and there may be
///   exactly one — but an *aliasing* constant (`const S: &str = BTW_METADATA_KEY;`)
///   carries no literal at all and defeats this guard and guard 6 together;
/// * a regex (`Regex::new(r"^/btw\b")` normalises to `^/btw\b`, not `btw`) —
///   low probability, since R7/P8 bans regex for reading user intent, but it is
///   a comparison this scan does not see;
/// * anything below a file's first `#[cfg(test)] mod`, by construction.
#[test]
fn only_the_shared_resolver_decides_what_a_side_question_is() {
    let sources = workspace_production_sources();
    assert_the_scan_read_the_tree(&sources);

    let resolver_files: Vec<&String> = sources
        .iter()
        .filter(|(_, src)| src.contains("impl BtwTurn") && src.contains("pub fn resolve"))
        .map(|(rel, _)| rel)
        .collect();
    assert_eq!(
        resolver_files.len(),
        1,
        "the shared resolver must have exactly one definition; found: {resolver_files:?}"
    );
    let resolver = resolver_files[0].clone();

    let mut in_resolver = Vec::new();
    let mut key_definitions = Vec::new();
    let mut hand_rolled = Vec::new();

    for (rel, src) in &sources {
        for (n, line) in src.lines().enumerate() {
            if !line.to_ascii_lowercase().contains("btw") {
                continue;
            }
            if !string_literals(line)
                .iter()
                .any(|lit| as_command_word(lit) == "btw")
            {
                continue;
            }
            let site = format!("{rel}:{}: {}", n + 1, line.trim());
            if *rel == resolver {
                in_resolver.push(site);
            } else if line.contains("const ") && line.contains(": &str =") {
                key_definitions.push(site);
            } else {
                hand_rolled.push(site);
            }
        }
    }

    assert!(
        hand_rolled.is_empty(),
        "these compare a value against the `/btw` verb without going through \
         the one resolver — a second predicate agrees with it today and drifts \
         the first time a spelling changes, and nothing at runtime can tell the \
         two apart:\n  {}",
        hand_rolled.join("\n  ")
    );
    assert_eq!(
        in_resolver.len(),
        1,
        "the resolver itself must compare the verb exactly once; found: {in_resolver:#?}"
    );
    assert_eq!(
        key_definitions.len(),
        1,
        "the metadata stamp's key must have exactly one definition — a second \
         spelling of it is how the writer and the readers come to disagree \
         about which map key marks a side turn; found: {key_definitions:#?}"
    );
}

// ---------------------------------------------------------------------------
// Guard 3's companion — the side key is derived in one place
// ---------------------------------------------------------------------------

/// The side session's id has one prefix, one definition of it, and one place
/// that builds an id out of it.
///
/// Write and read must be the same function: two call sites each hashing "the
/// same way" are byte-identical at epoch 0 and diverge only on a machine that
/// has run `/new` — the shape that never reproduces locally. The prefix is the
/// half a second author is most likely to re-spell, because it looks like a
/// constant worth inlining.
///
/// A thin client must never build the key at all, by design: the derivation
/// hashes the main key *including its epoch*, which no client holds, so a
/// client-side copy would address a session the server has never heard of — and
/// would get it wrong for the first time only after someone ran `/new`. A raw
/// `btw-` literal outside the constant's own definition is what that regression
/// would look like, in core or in a client, so the scan covers every crate.
#[test]
fn the_side_key_prefix_is_defined_once_and_built_once() {
    let sources = workspace_production_sources();
    assert_the_scan_read_the_tree(&sources);

    let mut mentions = Vec::new();
    let mut definitions = Vec::new();
    let mut constructions = Vec::new();
    let mut raw_literals = Vec::new();

    for (rel, src) in &sources {
        for (n, line) in src.lines().enumerate() {
            let site = format!("{rel}:{}: {}", n + 1, line.trim());
            let is_definition = line.contains("SIDE_KEY_PREFIX")
                && line.contains("const ")
                && line.contains(": &str =");
            if line.contains("SIDE_KEY_PREFIX") {
                mentions.push(site.clone());
                if is_definition {
                    definitions.push(site.clone());
                } else if line.contains("format!") {
                    constructions.push(site.clone());
                }
            }
            // The constant's own definition is the one place the prefix is
            // spelled out; every other spelling is a second opinion about what
            // a side key looks like.
            if !is_definition && string_literals(line).iter().any(|lit| lit.contains("btw-")) {
                raw_literals.push(site);
            }
        }
    }

    assert_eq!(
        definitions.len(),
        1,
        "the side-key prefix must have exactly one definition; found: {definitions:#?}"
    );
    assert_eq!(
        constructions.len(),
        1,
        "exactly one place may build a side-session id out of the prefix — a \
         second one is a second derivation of the key, and the two agree until \
         the first `/new`; found: {constructions:#?}"
    );
    assert!(
        raw_literals.is_empty(),
        "these spell the side-key prefix as a raw literal instead of using the \
         constant. In a client this is worse than duplication: the derivation \
         hashes the main key including its epoch, which no client holds, so a \
         client-built key addresses a session the server has never heard \
         of:\n  {}",
        raw_literals.join("\n  ")
    );
    assert_eq!(
        mentions.len(),
        3,
        "the prefix is mentioned {} time(s) in shipping code; it was written to \
         have exactly three — its definition, `is_side_key`'s read, and \
         `side_key_for`'s construction. A fourth is a new opinion about what a \
         side key looks like and needs a reason:\n  {}",
        mentions.len(),
        mentions.join("\n  ")
    );
}

// ---------------------------------------------------------------------------
// Guard 5 — promote is the only crossing back into the main conversation
// ---------------------------------------------------------------------------

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

fn is_ident_char(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// The identifier naming the call whose argument list encloses `at`, together
/// with the offset of that call's opening parenthesis.
fn enclosing_call(src: &str, at: usize) -> Option<(String, usize)> {
    let bytes = src.as_bytes();
    let mut depth = 0i32;
    let mut i = at;
    let open = loop {
        if i == 0 {
            return None;
        }
        i -= 1;
        match bytes[i] {
            b')' => depth += 1,
            b'(' if depth == 0 => break i,
            b'(' => depth -= 1,
            _ => {}
        }
    };
    let mut start = open;
    while start > 0 && is_ident_byte(bytes[start - 1]) {
        start -= 1;
    }
    (start < open).then(|| (src[start..open].to_string(), open))
}

/// Byte offset just past the `)` that matches the `(` at `open`.
fn matching_paren(src: &str, open: usize) -> Option<usize> {
    let bytes = src.as_bytes();
    let mut depth = 0i32;
    for (i, b) in bytes.iter().enumerate().skip(open) {
        match b {
            b'(' => depth += 1,
            b')' => {
                depth -= 1;
                if depth == 0 {
                    return Some(i + 1);
                }
            }
            _ => {}
        }
    }
    None
}

/// Every site where the main session is passed as an argument, as
/// `(callee, spelling, offset of the callee's opening paren)`.
///
/// Whole-text rather than per-line: the one call that matters here spreads its
/// argument list over four lines, and a line-based scan would miss exactly it.
///
/// # It has to see the value through its decorations
///
/// The first version required the byte before `main` to be `(` or `,` and the
/// byte after to be `)` or `,` — i.e. it recognised only a bare, undecorated
/// argument. `emit_event(&*main, …)` planted in `seed/mod.rs` — an unlabelled
/// write into the main conversation, exactly the defect the guard exists to
/// catch — therefore left every guard green, and so did `&main`,
/// `main.clone()` and `main.as_ref()`. That is not the disclosed *alias* limit:
/// `&*main` **is** spelled `main`. A block recogniser that does not recognise
/// its own value wearing a sigil is the shape whose green means "I cannot see
/// you", and `emit_event` taking a `SessionId` by value would make `&main` the
/// natural spelling — an ordinary refactor away.
///
/// So an optional `&` / `*` sigil run before the identifier and an optional
/// chain of `.method(…)` calls after it are accepted, and **the spelling is
/// carried into the census key**: a respelling then shows up as a visible diff
/// (`emit_event(main)` → `emit_event(&*main)`) rather than as a disappearance.
fn main_argument_sites(src: &str) -> Vec<(String, String, usize)> {
    let bytes = src.as_bytes();
    let mut out = Vec::new();
    let mut cursor = 0;
    while let Some(hit) = src[cursor..].find("main") {
        let at = cursor + hit;
        let end = at + "main".len();
        cursor = end;
        if at > 0 && is_ident_byte(bytes[at - 1]) {
            continue;
        }
        if end < bytes.len() && is_ident_byte(bytes[end]) {
            continue;
        }

        // Walk left over `&`, `&*`, `*` (whitespace-tolerant).
        let mut lo = at;
        loop {
            let head = src[..lo].trim_end();
            if head.ends_with('&') || head.ends_with('*') {
                lo = head.len() - 1;
            } else {
                break;
            }
        }
        if !{
            let before = src[..lo].trim_end();
            before.ends_with('(') || before.ends_with(',')
        } {
            continue;
        }

        // Walk right over a `.method(…)` chain (`.clone()`, `.as_ref()`, …).
        let mut hi = end;
        loop {
            let tail = src[hi..].trim_start();
            let Some(after_dot) = tail.strip_prefix('.') else {
                break;
            };
            let after_dot = after_dot.trim_start();
            let name = after_dot.len() - after_dot.trim_start_matches(is_ident_char).len();
            if name == 0 {
                break;
            }
            let rest = after_dot[name..].trim_start();
            if !rest.starts_with('(') {
                break;
            }
            let Some(close) = matching_paren(src, src.len() - rest.len()) else {
                break;
            };
            hi = close;
        }
        if !{
            let after = src[hi..].trim_start();
            after.starts_with(')') || after.starts_with(',')
        } {
            continue;
        }

        let spelling: String = src[lo..hi].chars().filter(|c| !c.is_whitespace()).collect();
        if let Some((callee, open)) = enclosing_call(src, at) {
            out.push((callee, spelling, open));
        }
    }
    out
}

/// The text between `open`'s parenthesis and its match.
fn call_body(src: &str, open: usize) -> &str {
    match matching_paren(src, open) {
        Some(close) => &src[open + 1..close - 1],
        None => &src[open + 1..],
    }
}

/// The 1-based line `offset` falls on, for a message a reader can navigate to.
fn line_of(src: &str, offset: usize) -> usize {
    src[..offset].lines().count().max(1)
}

/// `promote` is the only traffic from the side thread back into the main
/// conversation, and it carries exactly one event.
///
/// The other direction is silent and constant — [`super::seed`] copies the main
/// conversation into the side thread on every question. This direction happens
/// once, because the user asked out loud, and what it carries is labelled as
/// not the user's own words all the way into the prompt. A second write from
/// this module into the main session would put unlabelled text into a
/// conversation nobody asked to change, and the surface it shows up on is the
/// transcript, where it is indistinguishable from something the user typed.
///
/// # Two assertions, because one of them cannot be about `emit_event`
///
/// The blanket rule "nothing in `btw/` writes a session" is wrong: promote
/// *is* the sanctioned crossing and it lives here. And a rule about
/// `emit_event` alone would be a list of one verb — a write through the store
/// (`patch_session`), through a fork helper, through anything added later,
/// would walk past it.
///
/// So the first assertion is a census of **every call in this module that
/// receives the main session**, keyed by file and callee. It knows nothing
/// about which calls write; it makes any new one visible, and a new callee
/// taking `main` is precisely the change that needs a human to say whether it
/// writes. The second assertion is about the one crossing that exists: exactly
/// one `emit_event` takes `main`, it is in `promote.rs`, it emits
/// `SessionEvent::synthetic_user`, and the carrier it emits was built by
/// `promoted_side_answer` before the call.
///
/// # Known limit
///
/// `main` is a parameter *name*. Renaming it in `seed`/`promote` reddens this
/// guard for no safety reason; that is the cost of a census, and the census
/// assertion — which runs first, so that it is the one a rename hits — says what
/// to do. The scan is blind to a write that reaches the main session through a
/// value that is not *spelled* `main` — a local alias, or a field read. Sigils
/// and method-call decorations are **not** in that class and are recognised; see
/// [`main_argument_sites`], which is where the first version of this guard was
/// silently defeated.
#[test]
fn promote_is_the_only_crossing_back_into_the_main_conversation() {
    let root = repo_root().join("src/gateway/btw");
    let mut files = Vec::new();
    rust_files(&root, &mut files);
    files.sort();

    let mut scanned = 0usize;
    let mut census: Vec<String> = Vec::new();
    let mut emits: Vec<String> = Vec::new();

    for path in &files {
        let rel = path
            .strip_prefix(repo_root())
            .unwrap_or(path)
            .to_string_lossy()
            .replace('\\', "/");
        if is_test_only(&rel) {
            continue;
        }
        scanned += 1;
        let src = production_source(&read_file(path));
        // Anchored on the CALL, not the name: a `use crate::thinker::nudges::
        // promoted_side_answer;` would otherwise put the first occurrence on the
        // `use` line and make the ordering assertion below unconditionally true
        // — an assertion that stops asserting without ever reddening.
        let carrier_built_at = src.find("promoted_side_answer(");
        for (callee, spelling, open) in main_argument_sites(&src) {
            let at = line_of(&src, open);
            census.push(format!("{rel}: {callee}({spelling})"));
            if callee != "emit_event" {
                continue;
            }
            let body = call_body(&src, open).to_string();
            let one_line = body.split_whitespace().collect::<Vec<_>>().join(" ");
            assert!(
                rel.ends_with("/promote.rs"),
                "{rel}:{at} appends to the MAIN session. Promote — one event, \
                 asked for out loud, labelled as not the user's words — is the \
                 only sanctioned crossing:\n  emit_event({one_line})"
            );
            assert!(
                body.contains("SessionEvent::synthetic_user"),
                "{rel}:{at}: the crossing must carry \
                 `SessionEvent::synthetic_user`. A plain user event would be \
                 re-wrapped by the prompt builder as words the user typed — the \
                 exact failure this carrier exists to prevent:\n  \
                 emit_event({one_line})"
            );
            assert!(
                carrier_built_at.is_some_and(|built| built < open),
                "{rel}:{at}: the promoted carrier must be built by a call to \
                 `nudges::promoted_side_answer` before it is appended — that \
                 function is what makes the text classifiable as a promoted side \
                 answer rather than as user speech"
            );
            emits.push(format!("{rel}:{at}: emit_event({one_line})"));
        }
    }

    assert!(
        scanned >= 4,
        "scanned only {scanned} shipping file(s) under src/gateway/btw — the \
         walk or the cfg(test) split stopped matching"
    );

    // The census runs BEFORE the count below, deliberately. The likeliest
    // maintenance event is a rename of the `main` parameter, which empties both
    // — and only this assertion's message says what to do about it. Ordered the
    // other way a rename reds with "exactly one event may cross … found: []",
    // which reads as "the crossing vanished".
    census.sort();
    let expected: Vec<String> = [
        // Pure derivations of the side key from the main one. The `digest`
        // entry is the one the first version of this census could not see:
        // `main.to_key_string()` wears a method call, and the old matcher
        // required a bare identifier. It was already in the tree.
        "src/gateway/btw/mod.rs: digest(main.to_key_string().as_bytes())",
        "src/gateway/btw/mod.rs: is_side_key(main)",
        "src/gateway/btw/mod.rs: side_key_for(main)",
        // The crossing.
        "src/gateway/btw/promote.rs: emit_event(main)",
        // Reads of the main log, and a marker written onto the SIDE session
        // that merely names the main one in its payload.
        "src/gateway/btw/seed/mod.rs: get_events(main)",
        "src/gateway/btw/seed/mod.rs: mark_forked(main)",
        "src/gateway/btw/seed/mod.rs: snapshot(main)",
    ]
    .into_iter()
    .map(String::from)
    .collect();
    assert_eq!(
        census, expected,
        "the set of calls in this module that receive the MAIN session changed. \
         Every one of them is a chance to write where only promote may write, so \
         each needs a human to say which it is: if the new call reads (or writes \
         the side session and merely names the main one), add it here with that \
         reason; if it writes the main session, it is the defect this guard \
         exists to catch. If both sides look empty the `main` parameter was \
         renamed — retarget the entries rather than deleting them."
    );

    assert_eq!(
        emits.len(),
        1,
        "exactly one event may cross into the main conversation; found: {emits:#?}"
    );
}

// ---------------------------------------------------------------------------
// Guard 6 — btw is not a sixth session knob
// ---------------------------------------------------------------------------

/// `btw` must not be filed with the five session knobs.
///
/// The five share one mechanism: precedence request > session > global, and a
/// request-carried value **written back onto the session** so the choice
/// outlives its turn. `btw` is the opposite — it must affect exactly one call.
/// Filing it with them would let a single side question drop the main
/// conversation to `Plan` permanently, and the symptom would arrive later, in
/// another conversation, with nothing on screen connecting it to the question
/// that caused it.
///
/// `super`'s module doc states this as a sentence naming three places. A
/// sentence in a doc comment is what the knob machinery already learned not to
/// rely on — `think_level` was left off two of these census points for as long
/// as it existed, and a knob with no reader looks exactly like a knob nobody
/// set. So the sentence gets a test.
///
/// # The one mention that is allowed, and why it is not a knob
///
/// `turn_permissions.rs` reads the stamp with `contains_key` to mint the
/// read-only ceiling. That is a *query* of the request, once, for this turn:
/// nothing is stored, nothing outlives the call. Any other spelling in a
/// `turn_*.rs` reddens — including a read spelled some other way, which is a
/// false positive and therefore visible, the direction a scanner should fail
/// in.
#[test]
fn btw_is_not_filed_with_the_five_session_knobs() {
    let root = repo_root();

    let snapshot = root.join("src/gateway/session_snapshot.rs");
    let snapshot_src = production_source(&read_file(&snapshot));
    assert!(
        snapshot_src.contains("SESSION_KEY"),
        "session_snapshot.rs no longer decodes any `*_SESSION_KEY` — this guard \
         is reading the wrong text and its green means nothing"
    );

    let modify = root.join("src/gateway/handlers/session/db_handlers/modify.rs");
    let modify_src = production_source(&read_file(&modify));
    assert!(
        modify_src.contains("knob_validators"),
        "sessions.patch's `knob_validators()` table moved out of modify.rs — \
         this guard is reading the wrong text"
    );

    // The per-turn knob resolvers, listed by the directory rather than by name,
    // so a sixth twin is scanned without anyone remembering to add it.
    let engine = root.join("src/gateway/execution_engine");
    let mut turn_files: Vec<PathBuf> = std::fs::read_dir(&engine)
        .unwrap_or_else(|e| panic!("{}: {e}", engine.display()))
        .map(|e| e.expect("a readable dir entry").path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("turn_") && n.ends_with(".rs"))
        })
        .collect();
    turn_files.sort();
    assert!(
        turn_files.len() >= 5,
        "found only {} turn_*.rs resolver(s); the knob family has five twins, so \
         the scan is not seeing them",
        turn_files.len()
    );

    let mut filed = Vec::new();
    let mut ceiling_reads = 0usize;

    for (rel, src) in [
        ("src/gateway/session_snapshot.rs".to_string(), snapshot_src),
        (
            "src/gateway/handlers/session/db_handlers/modify.rs".to_string(),
            modify_src,
        ),
    ]
    .into_iter()
    .chain(turn_files.iter().map(|p| {
        let rel = p
            .strip_prefix(&root)
            .unwrap_or(p)
            .to_string_lossy()
            .replace('\\', "/");
        (rel, production_source(&read_file(p)))
    })) {
        let is_turn_resolver = rel.contains("/turn_");
        for (n, line) in src.lines().enumerate() {
            if !line.to_ascii_lowercase().contains("btw") {
                continue;
            }
            if is_turn_resolver && line.contains("contains_key") {
                ceiling_reads += 1;
                continue;
            }
            filed.push(format!("{rel}:{}: {}", n + 1, line.trim()));
        }
    }

    assert!(
        filed.is_empty(),
        "`btw` reached the session-knob machinery. It is not a knob: the five \
         knobs persist a request-carried value onto the session so it outlives \
         the turn, and a persisted `btw` would drop the main conversation to \
         `Plan` for good, in a later conversation, with nothing on screen \
         connecting the two:\n  {}",
        filed.join("\n  ")
    );
    assert_eq!(
        ceiling_reads, 1,
        "the read-only ceiling reads the stamp exactly once, in \
         `turn_permissions.rs`; found {ceiling_reads} such read(s) — either the \
         ceiling is gone (a side question would run at the conversation's real \
         tier) or a second place now decides it"
    );
}
