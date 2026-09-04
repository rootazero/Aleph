//! The membership rule for capability handles, and the guards that close it.
//!
//! # The rule (derived, never a hand-written list)
//!
//! A `static` of an install-once container (`OnceLock` / `OnceCell` /
//! `ArcSwap*`) is a **capability handle** iff its own file **installs** it.
//! There are two install forms, and both are properties of the code:
//!
//! 1. **Written** — something calls `set` / `store` / `swap` on it. The value
//!    comes from the caller.
//! 2. **First-caller-wins** — a `get_or_init` **or `get_or_try_init`** whose
//!    initialiser *depends on a parameter of the enclosing function*. The value
//!    also comes from the caller; only the delivery differs.
//!
//! ```ignore
//! GLOBAL.get_or_init(|| Arc::new(RouteHandle::from_config(cfg)))    // install (uses `cfg`)
//! WITNESSES.get_or_init(|| RwLock::new(BoundedWitnesses::default())) // lazy (no parameter)
//! ```
//!
//! A container only ever `get_or_init`-ed from data it can reach by itself is a
//! **lazy cache**: "not built yet" is the correct answer there, so it cannot
//! write an honest `MissingSemantics`. Excluded by derivation, never by a name
//! list.
//!
//! ## Why `get_or_try_init` is an initialiser, not a writer
//!
//! It sat in the writer set until form 2 existed, and that was the only place it
//! could go: a fallible initialiser is still an initialiser, but with one arm
//! there was nowhere to route it. Form 2 is what makes the writer membership
//! wrong — `get_or_try_init` is `get_or_init` with a fallible closure, so the
//! same question applies to it and had never been asked.
//!
//! Asking it moves exactly one static off the roster:
//! `extension/template.rs::FILE_REF_REGEX`, a compiled-regex cache in the
//! zero-parameter `fn file_ref_regex()`. Its own initialiser says why it does not
//! belong — *"The regex is a compile-time constant; a parse failure is a
//! programmer error"* — so its `Err` arm cannot occur and an "uninstalled" read
//! simply initialises itself. It has none of the round-7 failure semantics, and
//! migrating it onto a slot would mean either boot "installing" a regex or a
//! diagnostic reporting "never installed" forever: the over-see direction.
//!
//! ## Why form 2 is a rule and not an exemption
//!
//! `providers/route_handle.rs::GLOBAL` is boot-installed
//! (`orchestrator_init.rs:276`) and read through `try_global_route_handle() ->
//! Option`, so it is a genuine member — but it has no setter anywhere in its own
//! file. The specification's roster contained it anyway, because that roster was
//! produced by a **corpus-wide word-boundary** writer search, and it is not the
//! only static called `GLOBAL`: seven container statics in `src/` carry that
//! name, six of them are `.set(` in their own files (nine sites across six
//! files), and a corpus-wide search cannot tell the seventh from the other six.
//! It was in the roster by name collision — rename any one of those six and the
//! roster would have lost a real member with no signal at all. Form 2 selects it
//! for the reason it actually belongs, and survives the rename.
//!
//! The discriminating half is *use*, not *presence*. Ten sites across eight
//! statics in `src/` are `get_or_init`-ed inside a function that HAS parameters
//! and do not use them (`cached_repo_root(working_dir)`,
//! `cached_glob_regex(pattern)`, … — caches keyed by an argument, built from
//! nothing). All eight stay lazy. A predicate
//! that asked only "does the enclosing fn take arguments" would swallow them,
//! and `self_initialising_containers_are_excluded_by_derivation` fails by name
//! if it ever degrades that way.
//!
//! `self` counts as a parameter. It selects nothing today, so the choice is
//! free — and free choices go to the over-see side, because a rule that selects
//! a non-member gets argued about while a rule that misses one goes quiet.
//!
//! # Recogniser blind spots, and why each one is closed
//!
//! ⚠️ The type pattern MUST accept qualified paths (`std::sync::OnceLock`,
//! `once_cell::sync::OnceCell`, `arc_swap::ArcSwap`). A first pass that matched
//! only bare type names counted 29 boot handles where the true number is 40 —
//! and `spend::GLOBAL_LEDGER`, the anchor of the round-7 fix this generalises,
//! is written in the qualified form. A guard's green only covers the shapes its
//! recogniser knows.
//!
//! ⚠️ It must also accept a leading visibility modifier. `static ` alone misses
//! `pub static NAME: std::sync::OnceLock<…>`; measured 2026-08-24 that admits
//! exactly one further candidate in `src/`
//! (`extension/manifest/mod.rs::GLOBAL_MANIFEST_CACHE`), which the writer
//! predicate then excludes as a lazy cache — so accepting the form costs nothing
//! today and closes a blind spot that would otherwise open silently the first
//! time a handle is declared `pub`.
//!
//! ⚠️ A method call is matched across whitespace, so `X\n    .set(v)` counts as
//! a writer. A `contains("X.set(")` test does not, and that is not a style nit:
//! it is exactly how `metrics/mod.rs::METRICS_RUNTIME` — installed at boot from
//! `Config::load`, read as `.get().copied().unwrap_or_default()`, i.e. the
//! round-7 indistinguishable-default shape verbatim — was classified as a lazy
//! cache and left off the roster. `rustfmt` decides where that line break goes,
//! so a same-line matcher makes roster membership a function of line length.
//! `the_writer_recogniser_reads_across_line_breaks` fails by name if it regresses.
//!
//! ⚠️ The parameter-use test is **textual and non-transitive**. It asks whether
//! the initialiser mentions a parameter *by name*, so
//! `get_or_init(|| build(local))` — where `local` was itself derived from a
//! parameter — is declined. Direction: **under-see**. Measured 2026-08-24: the
//! shape exists (`skill/mod.rs::CACHED_MANIFEST` initialises from a local,
//! `global_skills`) but that local comes from `get_skills_dir()` and is
//! parameter-independent, so today the verdict is right anyway — zero instances,
//! by luck rather than by construction.
//!
//! # Known gap: interior-mutable installs
//!
//! **Unfixed. Direction: under-see** — handles in this class never get slots, so
//! they never get diagnostics, which is the silent-approval direction.
//!
//! Both forms above ask about the **container**. A handle whose container is
//! lazily built but whose *contents* are installed at boot through interior
//! mutability is invisible to both: `OnceLock<RwLock<Option<T>>>` reached by
//! `get_or_init(|| RwLock::new(None))` — no argument, so form 2 declines it —
//! and then filled by a `store_*` that takes a write guard, which is not a call
//! on the static, so form 1 declines it too. Such a handle has the round-7
//! failure semantics in full: an uninstalled read yields `None`, which reads as
//! a legal "not configured" and no caller can tell.
//!
//! **At least SEVENTEEN confirmed instances — and the number is the least
//! reliable thing in this section.** Read the counting rule below before using
//! it. Five members share one spelling; twelve do not, and the class is defined
//! by the *install*, never by the syntax:
//!
//! | static | shape | an uninstalled read means |
//! |---|---|---|
//! | `providers/moa/config_handle.rs::MOA_CONFIG` | `OnceLock<RwLock<Option<MoaToml>>>` | "no `[moa]` section configured" |
//! | `gateway/middleware/request_state.rs::STATE_REGISTRY` | `OnceLock<RwLock<Option<Arc<…>>>>` | `request.state` sees no registry |
//! | `gateway/middleware/latency.rs::GLOBAL_LATENCY` | `OnceLock<RwLock<Option<Arc<…>>>>` | `/metrics` renders no latency family |
//! | `gateway/event_emitter/origin_fanout.rs::CHANNEL_REGISTRY` | `OnceLock<RwLock<Option<Arc<…>>>>` | origin fan-out silently skipped |
//! | `gateway/event_emitter/team_fanout.rs::TEAM_EVENT_BUS` | `OnceLock<RwLock<Option<Arc<…>>>>` | team fan-out silently skipped |
//! | `security/audit.rs::GLOBAL_AUDIT` | `RwLock<Option<…>>` — **no `OnceLock`** | no security audit trail |
//! | `agents/background_persistence.rs::STORE_DIR` | `LazyLock<Mutex<Option<PathBuf>>>` | "persistence disabled … every entry point is a no-op" |
//! | `builtin_tools/process_journal.rs::STORE_DIR` | `LazyLock<Mutex<Option<PathBuf>>>` | the same sentence, verbatim |
//! | `builtin_tools/scratchpad_registry.rs::STORE_PATH` | `Lazy<Mutex<Option<PathBuf>>>` | "keeps the registry in-memory-only" |
//! | `builtin_tools/process_journal.rs::RESERVED_THROUGH` | `LazyLock<Mutex<u64>>` | "`0` = nothing reserved (also the value while persistence is off)" |
//! | `exec/masker.rs::OPERATOR_PATTERNS` | `LazyLock<RwLock<Arc<Vec<…>>>>` | zero mask patterns — redaction degrades to none |
//! | `utils/paths.rs::PLUGIN_SKILL_DIRS` | `RwLock<Vec<PathBuf>>` — **no `OnceLock`** | "no plugin skills" = `load_all` never ran |
//! | `agents/registry.rs::PLUGIN_SUBAGENTS` | `OnceLock<RwLock<Arc<[AgentDef]>>>` | "no plugin sub-agents" = extensions never loaded |
//! | `projects/roster.rs::ROSTER` | `OnceLock<RwLock<RosterSnapshot>>` | `is_member` → `false` for everyone (its own doc says so) |
//! | `scope/directory.rs::NAMES` | `OnceLock<RwLock<HashMap<…>>>` | every user renders as a bare id; `hydrate` is "called once at boot" |
//! | `gateway/interfaces/plugin.rs::PLUGINS` | `LazyLock<RwLock<HashMap<…>>>` | `get_factory` → `None` = "unknown channel type"; each channel registers "exactly once at startup" |
//! | `agents/subagent_tool/types.rs::MAX_CONCURRENT_SUBAGENTS` | `AtomicUsize` — **no lock at all** | `4`; an operator who set `[execution] max_concurrent_subagents = 1` silently gets the default |
//!
//! **Already answers, deliberately not counted:** `browser/manager.rs::LIVE_MANAGER`
//! (`Mutex<Option<Weak<…>>>`) is in the class by install, but `apply_policy_live`
//! returns a `bool` and its doc forbids reporting the change as live — so it
//! *can* be asked whether boot reached it. It is the one member of this shape
//! that already has the property this round adds. Listed so the next sweep does
//! not re-discover it as a defect.
//!
//! **Boundary cases, examined and left out** (they accumulate at runtime, so
//! "empty" is a true answer as well as an ambiguous one):
//! `gateway/handlers/markdown_skills.rs::SKILL_PATHS`,
//! `agents/background_persistence.rs::INDEX`,
//! `builtin_tools/process_journal.rs::{INDEX, UNDELIVERED}`. Named rather than
//! silently dropped: each is one argument away from being a member, and the
//! next person to count deserves to inherit the argument instead of re-deriving
//! it. They are the reason "at least".
//!
//! `logging/level_control.rs::CURRENT_LOG_LEVEL` (`AtomicU8`, stored by
//! `init_log_level` under a `Once`) is left out for the same reason by a
//! different route: its uninstalled read is `Info`, which is also the
//! *documented* behaviour when `RUST_LOG` is unset — so the default is a true
//! answer as well as an ambiguous one. Same treatment as `SKILL_PATHS`, recorded
//! so the next count inherits the argument rather than re-deriving it.
//!
//! # How to count this class (the number will be wrong again)
//!
//! ⚠️ **RE-DERIVE FROM THE DEFINITION. DO NOT TRUST THE FIGURE ABOVE.** The
//! class is *a handle whose contents are installed after boot through interior
//! mutability, whose uninstalled read is a legal-looking value*. It has been
//! counted seven times and revised upward six:
//! **1 → 4 → 6 → 9 → 14 → 16 → 17.** The revision before last wrote "assume nine
//! is too low as well" into this file and was then found low by five; the one
//! after it shipped the method below and was still found low by one.
//! **A warning is not a counting rule**, which is why what follows is a method
//! rather than another warning — and the seventh revision found its member
//! because the METHOD had the same defect the method itself diagnoses, one level
//! up. See step 2.
//!
//! 1. **Grep the rationale sentence, not the type.** Authors who choose this
//!    pattern explain themselves in near-identical words: *"installed once at
//!    boot"*, *"a process-global publish … rather than threading it through
//!    every boot seam"*, *"published after every extension load"*, *"called
//!    once during subsystem boot"*. Three members were found this way and by no
//!    type-shaped grep. The sentence is a better fingerprint than the shape,
//!    because it is what the author writes when they make this choice.
//! 2. **Then enumerate interior-mutable statics** — `Mutex` / `RwLock` at any
//!    nesting (under `OnceLock` / `LazyLock` / `Lazy` / bare), `ArcSwap`, **and
//!    atomics** — and ask of each: who writes through it, and when?
//!    Boot/subsystem-publish ⇒ member. Accumulates during normal work ⇒ not.
//!
//!    ⚠️ This step read "lock-bearing statics" for one revision, and that is how
//!    `MAX_CONCURRENT_SUBAGENTS` stayed out: an atomic is interior mutability
//!    **without a lock**, so it was outside the enumeration by construction.
//!    That is step 3's disease one level up — the scan was keyed on the
//!    *container* spelling instead of the *absent value* spelling, and it came
//!    out low for exactly the same reason. The `ArcSwap` half was checked when
//!    this was widened: outside `spend::GLOBAL_POLICY` (already migrated) there
//!    are none, so atomics were the live gap.
//!
//!    ⚠️ And enumerate over **declarations, not lines**:
//!    `gateway/interfaces/plugin.rs::PLUGINS` — a member already in the table
//!    above — spans three lines, so a single-line `grep` for
//!    `static … : …Mutex/RwLock…` drops it. Anyone running this step literally
//!    loses a member that was already counted.
//! 3. **Do not key on `Option`.** "Absent" has been spelled at least five ways
//!    here: `None`, an empty `Vec`/slice, an empty `HashMap`, a sentinel `0`,
//!    and a `Default::default()` snapshot. Every table above was built by a
//!    scan keyed on one of those spellings, which is precisely how it kept
//!    coming out low.
//!
//! Nothing asserts seventeen, and nothing can: the class is *below this rule's
//! resolution* by construction, so no guard sees the eighteenth arrive. That
//! is the reason this is prose with a method attached rather than a test.
//!
//! `STATE_REGISTRY` shows how little hiding this takes: `origin_fanout.rs` names
//! it in the doc comment on its own row above ("Mirrors the
//! `middleware::request_state` global-registry pattern"), and it still sat
//! outside the count for two revisions.
//!
//! `GLOBAL_AUDIT` and `PLUGIN_SKILL_DIRS` are why this section is no longer
//! titled after the `OnceLock<Lock<Option<T>>>` spelling. Neither is a container
//! static, so neither is even a *candidate* here — no widening of either install
//! form could reach them. `MAX_CONCURRENT_SUBAGENTS` is a third reason: an
//! atomic bears no lock at all. The class is **interior-mutable installs**: two
//! gateway fan-out paths, two gateway middleware handles, the channel-factory
//! table, the audit trail, three persistence roots, the id floor, the operator
//! mask patterns, the plugin skill dirs and sub-agents, the project roster, the
//! user directory, the sub-agent concurrency cap, and the `[moa]` config — none
//! of which can currently be asked whether boot reached them.
//!
//! `the_interior_mutable_install_class_is_below_this_rules_resolution` pins
//! `MOA_CONFIG`'s shape. All seventeen are excluded here, and all seventeen were
//! excluded from the specification's roster too — this class has been missing
//! all along, so nothing regressed; it was never seen.
//!
//! Closing it needs a "who writes through the guard" predicate — reachability
//! from a `write()`/`lock()` guard binding to the static — not a method-name
//! scan. That is separate work and it would move the number pinned below, so it
//! is recorded here rather than silently absorbed. A gap that lives only in a
//! round's ledger is a gap the next reader never sees.

use crate::utils::source_scan::{
    cfg_test_portion, code_text, production_prefix, rust_sources_under, strip_comment_lines,
};

/// One process-global container `static`, as the rule sees it.
pub(crate) struct HandleSite {
    pub file: String,
    pub name: String,
    pub container: String,
    pub is_slot: bool,
}

/// A container `static` the rule excluded, with the one fact needed to tell
/// "the predicate declined it" from "the predicate never got to run on it".
struct LazySite {
    file: String,
    name: String,
    /// Form 2 found at least one `get_or_init` / `get_or_try_init` call on this
    /// static. Distinguishes "the arm looked and said no" from "the arm never
    /// reached it" — which is the difference between a correct exclusion and a
    /// recogniser that silently stopped matching.
    saw_install_call: bool,
    /// This static is initialised inside a function that HAS parameters, and the
    /// initialiser uses none of them. Without this the discrimination guard
    /// could not tell a working predicate from one that stopped parsing function
    /// signatures — both report "excluded".
    saw_parameterised_get_or_init: bool,
}

/// Install-once container types. Compared against the FINAL path segment, so a
/// qualified path resolves to the same member as a bare name.
const CONTAINERS: &[&str] = &[
    "OnceLock",
    "OnceCell",
    "ArcSwapOption",
    "ArcSwapAny",
    "ArcSwap",
];

/// The ways a caller writes an install-once container from outside it.
///
/// `get_or_init` and `get_or_try_init` are both deliberately absent: they are
/// handled by the second install form, which asks where the initialiser's data
/// came from. A fallible initialiser is still an initialiser — putting
/// `get_or_try_init` here made "did this call succeed" the question instead,
/// which is not the one that decides membership.
const WRITERS: &[&str] = &["set", "store", "swap"];

/// The install-once *initialiser* methods, routed to form 2.
///
/// Matched whole: `method_call_open_paren` rejects a trailing identifier byte,
/// so `get_or_init` cannot match the head of `get_or_try_init`.
const INITIALISERS: &[&str] = &["get_or_init", "get_or_try_init"];

/// Strip an optional leading visibility modifier, returning the rest.
///
/// Handles `pub`, `pub(crate)`, `pub(super)`, `pub(in path)`. Anything it does
/// not recognise is returned unchanged, so an unparsed line simply fails the
/// `static ` test below rather than being mangled into a false match.
fn strip_visibility(t: &str) -> &str {
    let Some(rest) = t.strip_prefix("pub") else {
        return t;
    };
    if rest.starts_with('(') {
        return match rest.split_once(')') {
            Some((_, tail)) => tail.trim_start(),
            None => t,
        };
    }
    if rest.starts_with(char::is_whitespace) {
        rest.trim_start()
    } else {
        t // `pub` was the prefix of some other identifier
    }
}

/// Parse `[vis] static NAME : <maybe::qualified::>Container <`.
fn parse_static_decl(line: &str) -> Option<(String, String)> {
    let t = strip_visibility(line.trim_start());
    let rest = t.strip_prefix("static ")?;
    let (name, after) = rest.split_once(':')?;
    let name = name.trim();
    if name.is_empty()
        || !name
            .chars()
            .all(|c| c.is_ascii_uppercase() || c == '_' || c.is_ascii_digit())
    {
        return None;
    }
    // Drop any qualifying path segments before the type name.
    let ty = after.trim().split('<').next()?.trim();
    let last = ty.rsplit("::").next()?.trim();
    let container = CONTAINERS.iter().find(|c| **c == last)?;
    Some((name.to_string(), (*container).to_string()))
}

/// Rust identifier byte: what must NOT sit either side of a whole-word match.
fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Byte offsets of every whole-word occurrence of `word` in `text`.
fn word_occurrences(text: &str, word: &str) -> Vec<usize> {
    let (bytes, w) = (text.as_bytes(), word.as_bytes());
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = text[from..].find(word) {
        let at = from + rel;
        let before_ok = at == 0 || !is_ident_byte(bytes[at - 1]);
        let after = at + w.len();
        let after_ok = after >= bytes.len() || !is_ident_byte(bytes[after]);
        if before_ok && after_ok {
            out.push(at);
        }
        from = at + 1;
    }
    out
}

/// Index of the first non-whitespace byte at or after `i`.
fn skip_ws(bytes: &[u8], mut i: usize) -> usize {
    while i < bytes.len() && bytes[i].is_ascii_whitespace() {
        i += 1;
    }
    i
}

/// Starting just past an occurrence of the receiver, match ` . method (` with
/// arbitrary whitespace anywhere between the tokens, and return the offset of
/// that `(`.
///
/// Whitespace-tolerant on purpose: `rustfmt` puts the break wherever the line
/// got long, so a same-line matcher makes roster membership depend on line
/// length. See the module note on `METRICS_RUNTIME`.
fn method_call_open_paren(text: &str, after_receiver: usize, method: &str) -> Option<usize> {
    let bytes = text.as_bytes();
    let i = skip_ws(bytes, after_receiver);
    if bytes.get(i)? != &b'.' {
        return None;
    }
    let i = skip_ws(bytes, i + 1);
    if !text[i..].starts_with(method) {
        return None;
    }
    let end = i + method.len();
    if bytes.get(end).is_some_and(|b| is_ident_byte(*b)) {
        return None; // `set_foo`, not `set`
    }
    let p = skip_ws(bytes, end);
    (bytes.get(p) == Some(&b'(')).then_some(p)
}

/// Given the offset of an opening delimiter, the offset of its match.
fn matching(text: &str, open: usize, o: u8, c: u8) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    for (i, b) in bytes.iter().enumerate().skip(open) {
        if *b == o {
            depth += 1;
        } else if *b == c {
            depth -= 1;
            if depth == 0 {
                return Some(i);
            }
        }
    }
    None
}

/// One function body, with the identifiers its signature binds.
struct FnSpan {
    params: Vec<String>,
    body: std::ops::Range<usize>,
}

/// Every function body in `text`, with its bound parameter identifiers.
///
/// Signature-only declarations (trait methods ending in `;`) are skipped: they
/// have no body, so nothing can be enclosed by them.
fn fn_spans(text: &str) -> Vec<FnSpan> {
    let bytes = text.as_bytes();
    let mut out = Vec::new();
    for at in word_occurrences(text, "fn") {
        // name
        let i = skip_ws(bytes, at + 2);
        let mut j = i;
        while j < bytes.len() && is_ident_byte(bytes[j]) {
            j += 1;
        }
        if j == i {
            continue; // `fn(` in a type position, e.g. `Box<dyn fn…>`
        }
        let mut k = skip_ws(bytes, j);
        // optional generic list
        if bytes.get(k) == Some(&b'<') {
            let Some(end) = matching_generics(text, k) else {
                continue;
            };
            k = skip_ws(bytes, end + 1);
        }
        if bytes.get(k) != Some(&b'(') {
            continue;
        }
        let Some(close) = matching(text, k, b'(', b')') else {
            continue;
        };
        let params = param_idents(&text[k + 1..close]);
        // body: the first `{` after the parameter list; a `;` first means no body
        let mut b = close + 1;
        while b < bytes.len() && bytes[b] != b'{' && bytes[b] != b';' {
            b += 1;
        }
        if bytes.get(b) != Some(&b'{') {
            continue;
        }
        let Some(end) = matching(text, b, b'{', b'}') else {
            continue;
        };
        out.push(FnSpan {
            params,
            body: b..end,
        });
    }
    out
}

/// Offset of the `>` closing the generic list opened at `open`.
///
/// `->` and `=>` are not closers, and a `(` group inside (`Fn(u32) -> u32`) is
/// skipped whole — both shapes appear in this repo's bounds.
fn matching_generics(text: &str, open: usize) -> Option<usize> {
    let bytes = text.as_bytes();
    let mut depth = 0usize;
    let mut i = open;
    while i < bytes.len() {
        match bytes[i] {
            b'<' => depth += 1,
            b'>' if !matches!(bytes.get(i.wrapping_sub(1)), Some(b'-' | b'=')) => {
                depth -= 1;
                if depth == 0 {
                    return Some(i);
                }
            }
            b'(' => i = matching(text, i, b'(', b')')?,
            b'{' | b';' => return None, // ran past the signature
            _ => {}
        }
        i += 1;
    }
    None
}

/// Identifiers a parameter list binds, including `self`.
///
/// Takes the identifiers left of each top-level `:` — that is the pattern half,
/// so destructured parameters (`Config { root, .. }: Config`) bind all of their
/// names rather than none.
fn param_idents(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    for part in split_top_level_commas(src) {
        let p = part.trim();
        if p.is_empty() {
            continue;
        }
        if p == "self" || p.ends_with(" self") || p.ends_with("&self") || p.ends_with("mut self") {
            out.push("self".to_string());
            continue;
        }
        let lhs = match top_level_colon(p) {
            Some(i) => &p[..i],
            None => p,
        };
        let mut cur = String::new();
        for ch in lhs.chars().chain(std::iter::once(' ')) {
            if ch.is_ascii_alphanumeric() || ch == '_' {
                cur.push(ch);
            } else if !cur.is_empty() {
                if !matches!(cur.as_str(), "mut" | "ref")
                    && cur.starts_with(|c: char| c.is_ascii_lowercase() || c == '_')
                {
                    out.push(std::mem::take(&mut cur));
                } else {
                    cur.clear();
                }
            }
        }
    }
    out
}

fn split_top_level_commas(src: &str) -> Vec<&str> {
    let (bytes, mut depth, mut start) = (src.as_bytes(), 0i32, 0usize);
    let mut out = Vec::new();
    for (i, b) in bytes.iter().enumerate() {
        match b {
            b'(' | b'<' | b'[' | b'{' => depth += 1,
            b')' | b'>' | b']' | b'}' => depth -= 1,
            b',' if depth == 0 => {
                out.push(&src[start..i]);
                start = i + 1;
            }
            _ => {}
        }
    }
    out.push(&src[start..]);
    out
}

fn top_level_colon(p: &str) -> Option<usize> {
    let (bytes, mut depth) = (p.as_bytes(), 0i32);
    let mut i = 0usize;
    while i < bytes.len() {
        match bytes[i] {
            b'(' | b'<' | b'[' | b'{' => depth += 1,
            b')' | b'>' | b']' | b'}' => depth -= 1,
            b':' if depth == 0 => {
                if bytes.get(i + 1) == Some(&b':') {
                    i += 1; // path separator
                } else {
                    return Some(i);
                }
            }
            _ => {}
        }
        i += 1;
    }
    None
}

/// Does any `WRITERS` method get called on `name` anywhere in `text`?
fn is_written(text: &str, name: &str) -> bool {
    word_occurrences(text, name).into_iter().any(|at| {
        WRITERS
            .iter()
            .any(|m| method_call_open_paren(text, at + name.len(), m).is_some())
    })
}

/// Verdict of the first-caller-wins arm, with the fact the guards need.
struct FirstCaller {
    /// Some initialiser used a parameter of its enclosing fn.
    installs: bool,
    /// An initialiser call on this static was found at all.
    saw_call: bool,
    /// Some initialiser sat in a fn WITH parameters and used none of them —
    /// i.e. the discriminating half actually ran and said no.
    declined_on_use: bool,
}

/// Is `name` installed by an initialiser that depends on a parameter of the
/// enclosing function?
fn first_caller_install(text: &str, name: &str, spans: &[FnSpan]) -> FirstCaller {
    let mut verdict = FirstCaller {
        installs: false,
        saw_call: false,
        declined_on_use: false,
    };
    for at in word_occurrences(text, name) {
        let Some(open) = INITIALISERS
            .iter()
            .find_map(|m| method_call_open_paren(text, at + name.len(), m))
        else {
            continue;
        };
        verdict.saw_call = true;
        let Some(close) = matching(text, open, b'(', b')') else {
            continue;
        };
        let init = &text[open + 1..close];
        // innermost enclosing body
        let Some(span) = spans
            .iter()
            .filter(|s| s.body.contains(&at))
            .max_by_key(|s| s.body.start)
        else {
            continue;
        };
        if span.params.is_empty() {
            continue;
        }
        if span
            .params
            .iter()
            .any(|p| !word_occurrences(init, p).is_empty())
        {
            verdict.installs = true;
        } else {
            verdict.declined_on_use = true;
        }
    }
    verdict
}

/// What the rule saw, in one walk.
///
/// One struct from one walk on purpose: two functions that each re-scanned
/// would be two answers to "what did the rule see", and the self-counting
/// assertions need every half of a single verdict.
struct Census {
    /// Selected by install form 1 (a writer in its own file).
    written: Vec<HandleSite>,
    /// Selected by install form 2 (parameter-dependent `get_or_init`).
    first_caller: Vec<HandleSite>,
    /// Already migrated onto the slot types.
    slots: Vec<HandleSite>,
    /// Excluded by derivation.
    lazy: Vec<LazySite>,
}

impl Census {
    /// Every selected handle, in the shape Tasks 7–10 consume.
    fn handles(self) -> Vec<HandleSite> {
        let mut out = self.written;
        out.extend(self.first_caller);
        out.extend(self.slots);
        out
    }
}

fn take_census() -> Census {
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
    let sources = rust_sources_under(&root);
    assert!(
        sources.len() > 100,
        "the source walk found only {} files under src/ — the census scanned \
         nothing, which is not the same as finding nothing wrong",
        sources.len()
    );

    let mut c = Census {
        written: Vec::new(),
        first_caller: Vec::new(),
        slots: Vec::new(),
        lazy: Vec::new(),
    };
    for (rel, text) in sources {
        let prod = strip_comment_lines(&production_prefix(&text));
        let mut spans: Option<Vec<FnSpan>> = None;
        for line in prod.lines() {
            let Some((name, container)) = parse_static_decl(line) else {
                continue;
            };
            let site = || HandleSite {
                file: rel.clone(),
                name: name.clone(),
                container: container.clone(),
                is_slot: false,
            };
            if is_written(&prod, &name) {
                c.written.push(site());
                continue;
            }
            let spans = spans.get_or_insert_with(|| fn_spans(&prod));
            let v = first_caller_install(&prod, &name, spans);
            if v.installs {
                c.first_caller.push(site());
            } else {
                c.lazy.push(LazySite {
                    file: rel.clone(),
                    name,
                    saw_install_call: v.saw_call,
                    saw_parameterised_get_or_init: v.declined_on_use,
                });
            }
        }
        // Slots are declared with the newtype, not a raw container. Migration
        // moves handles from the first two buckets to this one; the SUM is what
        // the guard pins, so a half-finished migration cannot go quiet.
        for line in prod.lines() {
            let t = strip_visibility(line.trim_start());
            let Some(rest) = t.strip_prefix("static ") else {
                continue;
            };
            if !rest.contains("CapabilitySlot<") {
                continue; // also covers MutableCapabilitySlot<, which contains it
            }
            let Some((name, after)) = rest.split_once(':') else {
                continue;
            };
            // Read the declared type rather than hard-coding one of the two.
            // A literal here was invisible while zero `MutableCapabilitySlot`
            // statics existed and became false the moment the first one landed
            // — in a column this file's own doc calls a cross-task interface.
            // `split_once`, not `split(..).next()`: the latter can never
            // return `None`, so its `else { continue }` arm was unreachable
            // code wearing the shape of a fallible parse.
            let declared = after.split_once('<').map_or(after, |(head, _)| head).trim();
            c.slots.push(HandleSite {
                file: rel.clone(),
                name: name.trim().to_string(),
                container: declared.rsplit("::").next().unwrap_or(declared).to_string(),
                is_slot: true,
            });
        }
    }
    c
}

/// The authoritative inventory Tasks 7–10 migrate and Task 11 closes.
///
/// Regenerable by construction: the rule is deterministic over `src/`, so a lost
/// copy of the printed inventory is one test run, not a blocked task.
pub(crate) fn capability_handles() -> Vec<HandleSite> {
    take_census().handles()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Guard A — the class is closed. A new bare install-once static is a new
    /// handle nobody can observe.
    ///
    /// `providers/route_handle.rs::GLOBAL` is the one deliberate exception: the
    /// census's single first-caller-wins member, ruled to stay a raw
    /// `OnceLock` rather than migrate onto [`super::CapabilitySlot`] — fitting
    /// it to [`super::CapabilitySlot::install`] would need either a different
    /// `Outcome` shape or a changed call site, and the second one would change
    /// WHICH caller's config wins the initialisation. The ruling and its full
    /// reasoning live at the static itself (`providers::route_handle::GLOBAL`,
    /// adjudicated 2026-08-25) so that a migrator who opens the code before the
    /// docs still finds it; this comment is a pointer, not a second account.
    /// See this module's doc ("Why
    /// form 2 is a rule and not an exemption") and
    /// `route_handle_global_is_selected_by_the_first_caller_wins_arm_alone`
    /// below for why it genuinely belongs to this census while staying off
    /// the offender list. Named rather than silently swallowed by a weaker
    /// predicate — the same rule the rest of this module follows for every
    /// other named exclusion.
    ///
    /// The offender scan alone cannot see the census itself shrinking (fewer
    /// sites means fewer offenders, not more), so the total is pinned too —
    /// the SUM, never a bare count of either side, because that is the form
    /// that survives further migration: `raw` and `slots` move against each
    /// other as handles migrate (today: 1 raw, all of it
    /// `route_handle::GLOBAL`, + 46 slots), and only the sum is invariant.
    ///
    /// Neither of those two assertions can see the exemption ITSELF growing:
    /// widen the filter below to also swallow a second raw handle and
    /// `offenders` empties by construction, the sum is untouched, and both
    /// assertions stay green. An exemption that can grow silently is a
    /// licence, not a named exception, so a third assertion pins its SIZE —
    /// exactly one match, by name — independent of the other two.
    #[test]
    fn every_installed_global_is_a_capability_slot() {
        let sites = capability_handles();
        let is_the_route_handle_global_exemption = |s: &&HandleSite| {
            s.file.ends_with("src/providers/route_handle.rs") && s.name == "GLOBAL"
        };
        let offenders: Vec<String> = sites
            .iter()
            .filter(|s| !s.is_slot)
            .filter(|s| !is_the_route_handle_global_exemption(s))
            .map(|s| format!("{}::{} ({})", s.file, s.name, s.container))
            .collect();
        assert!(
            offenders.is_empty(),
            "these are written at runtime but are not CapabilitySlots, so nothing \
             can tell 'never installed' from 'installed with this value':\n  {}",
            offenders.join("\n  ")
        );

        let (raw, slots) = (
            sites.iter().filter(|s| !s.is_slot).count(),
            sites.iter().filter(|s| s.is_slot).count(),
        );
        assert_eq!(
            raw + slots,
            48,
            "capability handle total drifted: {raw} raw + {slots} slots = {}, not \
             48. Never assert either side alone: raw shrinks and slots grows as \
             migration proceeds, so only the SUM is stable. A drift here means \
             either a census recogniser regressed (see the module doc's \
             recogniser blind spots) or a handle genuinely left the corpus — \
             investigate before editing this number. Last moved 2026-09-04: \
             47 -> 48 when `heartbeat/service` was added, so `users.update`'s \
             deactivation freeze had a fourth subsystem to reach.",
            raw + slots
        );

        let exempted: Vec<String> = sites
            .iter()
            .filter(|s| !s.is_slot)
            .filter(|s| is_the_route_handle_global_exemption(s))
            .map(|s| format!("{}::{}", s.file, s.name))
            .collect();
        assert_eq!(
            exempted.len(),
            1,
            "the route_handle::GLOBAL exemption above matched {} raw handle(s), \
             not exactly one: {exempted:?}. This assertion FORBIDS widening \
             that filter: it exists for the one member ruled exempt \
             (providers/route_handle.rs::GLOBAL, see this test's doc and \
             `route_handle_global_is_selected_by_the_first_caller_wins_arm_alone`), \
             and a broader predicate would make `offenders` empty by \
             construction while a second, genuinely unmigrated handle rode \
             along unnoticed. If a new raw handle needs the same treatment, \
             that is a decision for a human to write here explicitly — name \
             it, do not generalise the predicate to catch it.",
            exempted.len()
        );
    }

    /// Guard B — the roster is complete. A slot missing from `ALL_SLOTS` is
    /// invisible to the `core/capability-wiring` diagnostic, which is the same
    /// silence as before this round.
    ///
    /// Compares COUNTS, not names: a slot's `id()` (e.g. `"spend/policy"`) is
    /// deliberately not its static's name (`GLOBAL_POLICY`), so there is no
    /// shared key to compare `declared` and `rostered` element-by-element. The
    /// count equality is the real check, self-referential against the live
    /// census rather than a hand-carried figure; the `>= 40` floor beneath it
    /// exists only so the equality cannot pass vacuously at `0 == 0` if both
    /// sides went blind at once.
    ///
    /// ⚠️ `declared` is keyed on `{file}::{name}`, not on the bare static
    /// name: six of today's 45 slots are named `GLOBAL` (this module's own doc
    /// walks through why that name collides so often), so a `BTreeSet` keyed
    /// on `name` alone collapses those six into one and undercounts by five —
    /// measured, not guessed against: first-written this test compared bare
    /// names and asserted `40 == 45` against a correctly-sized roster.
    #[test]
    fn every_declared_slot_is_in_the_roster() {
        let declared: std::collections::BTreeSet<String> = capability_handles()
            .into_iter()
            .filter(|s| s.is_slot)
            .map(|s| format!("{}::{}", s.file, s.name))
            .collect();
        let rostered: std::collections::BTreeSet<String> = crate::capability::ALL_SLOTS
            .iter()
            .map(|s| s.id().to_string())
            .collect();
        assert_eq!(
            declared.len(),
            rostered.len(),
            "declared slots: {declared:?}\nroster ids: {rostered:?}\n\
             every CapabilitySlot::new() must be reachable from ALL_SLOTS"
        );
        assert!(
            rostered.len() >= 40,
            "roster has {} entries; 45 were measured on 2026-08-25 (the census \
             decomposes as 1 raw -- route_handle::GLOBAL, staying raw by ruling, \
             see every_installed_global_is_a_capability_slot above -- + 45 \
             slots). A shrinking roster and a broken scan look identical in a \
             green report.",
            rostered.len()
        );
    }

    /// Edge checks drawn from `task-6-boundary-cases.md`.
    ///
    /// ⚠️ This is NOT the membership definition — that is the rule above, and a
    /// guard that lists its own members goes blind the moment the set grows.
    /// This pins the two handles the whole round is ANCHORED on, because a count
    /// assertion alone cannot see them leave: a rule that dropped
    /// `spend::GLOBAL_*` while picking up two unrelated statics still counts 47.
    #[test]
    fn the_round_seven_anchors_are_selected_by_the_rule() {
        let sites = capability_handles();
        let found = |file: &str, name: &str| {
            sites
                .iter()
                .any(|s| s.file.ends_with(file) && s.name == name)
        };
        assert!(
            found("src/spend/mod.rs", "GLOBAL_LEDGER"),
            "GLOBAL_LEDGER is the §5.22 round-7 anchor and is written in the \
             QUALIFIED form (`std::sync::OnceLock`); a recogniser that matches \
             only bare type names loses it while still reporting green"
        );
        assert!(
            found("src/spend/mod.rs", "GLOBAL_POLICY"),
            "GLOBAL_POLICY is the round-7 anchor and the sole member \
             MutableCapabilitySlot exists for"
        );
    }

    /// The self-initialising exclusions, verified rather than assumed — and, for
    /// the two that sit inside a parameterised function, verified to have been
    /// declined on parameter **use** rather than on parameter **absence**.
    ///
    /// That second half is what stops the first-caller-wins arm from degrading
    /// into "the enclosing fn takes arguments". Ten such sites across eight
    /// statics in `src/` would be swallowed by that weaker predicate;
    /// `cached_repo_root(working_dir)` and
    /// `guess_source(path)` are two of them, and both build their cache from
    /// nothing while merely being *keyed* by the argument.
    #[test]
    fn self_initialising_containers_are_excluded_by_derivation() {
        let c = take_census();
        let selected = |file: &str, name: &str| {
            c.written
                .iter()
                .chain(c.first_caller.iter())
                .chain(c.slots.iter())
                .any(|s| s.file.ends_with(file) && s.name == name)
        };
        // (file, name, sits in a parameterised fn and must be declined on USE)
        for (file, name, declined_on_use) in [
            ("src/providers/route_witness.rs", "WITNESSES", false),
            ("src/skill/mod.rs", "CACHED_MANIFEST", true),
            ("src/thinker/runtime_context.rs", "REPO_ROOT_CACHE", true),
        ] {
            assert!(
                !selected(file, name),
                "{name} is installed by neither form and must not be selected"
            );
            let site = c
                .lazy
                .iter()
                .find(|s| s.file.ends_with(file) && s.name == name)
                .unwrap_or_else(|| {
                    panic!(
                        "{name} was neither selected nor seen as a candidate — the \
                         recogniser stopped matching its declaration form, which \
                         looks exactly like a correct exclusion"
                    )
                });
            assert_eq!(
                site.saw_parameterised_get_or_init, declined_on_use,
                "{name}: expected declined-on-parameter-use = {declined_on_use}. \
                 A `false` where `true` is expected means the arm never parsed \
                 the enclosing signature, so it is excluding this static for the \
                 wrong reason and would exclude a real handle the same way."
            );
        }
    }

    /// `providers/route_handle.rs::GLOBAL` is selected by the first-caller-wins
    /// arm and by nothing else.
    ///
    /// This is the member the specification's roster held by accident: a
    /// corpus-wide writer search could not distinguish it from the six OTHER
    /// container statics named `GLOBAL` that are `.set(` in their own files. It is
    /// boot-installed at `orchestrator_init.rs:276` and read through
    /// `try_global_route_handle() -> Option`, so it genuinely belongs — and it
    /// now belongs for a reason a rename cannot break.
    ///
    /// The assertion is two-sided on purpose. "Is a handle" would still pass if
    /// someone widened the writer search until it swallowed this static by name
    /// again, which is precisely the derivation this test exists to forbid.
    #[test]
    fn route_handle_global_is_selected_by_the_first_caller_wins_arm_alone() {
        let c = take_census();
        let is = |v: &[HandleSite]| {
            v.iter()
                .any(|s| s.file.ends_with("src/providers/route_handle.rs") && s.name == "GLOBAL")
        };
        assert!(
            is(&c.first_caller),
            "route_handle::GLOBAL is no longer selected by the first-caller-wins \
             arm. Either its install shape changed (it was \
             `GLOBAL.get_or_init(|| Arc::new(RouteHandle::from_config(cfg)))`, \
             installed from the enclosing fn's `cfg` parameter), or that arm \
             stopped working — and a handle silently leaving the roster is the \
             failure this whole round exists to remove."
        );
        assert!(
            !is(&c.written),
            "route_handle::GLOBAL is now matched by the WRITER search. If someone \
             gave it a real setter, that is fine — delete this test and the note \
             in the module doc. If instead the writer search was widened until it \
             matched across files, revert that: it would be selecting this static \
             by its very short name, which is how the specification's roster got \
             it, and a rename would silently drop a real member."
        );
    }

    /// A writer split across lines is still a writer.
    ///
    /// Roster membership must not be a function of line length.
    ///
    /// ⚠️ **This guard used to be anchored on a live member and could not stay
    /// there.** `metrics/mod.rs::METRICS_RUNTIME` was the anchor: installed from
    /// `Config::load` via `init_metrics_runtime(policy)`, read as
    /// `.get().copied().unwrap_or_default()`, and missing from the
    /// specification's hand-written roster for one reason only — rustfmt broke
    /// the line, so the source read `if METRICS_RUNTIME\n        .set(…)` and a
    /// `contains("METRICS_RUNTIME.set(")` test said no.
    ///
    /// Batch D migrated it, and `written` is now **0 by design**: emptying that
    /// bucket is this round's completion signal. Any assertion of the form
    /// `c.written.iter().any(…)` is therefore structurally doomed — not by a
    /// regression, but by the round succeeding. Re-anchoring on some other
    /// member would only defer that by one batch.
    ///
    /// So the property is pinned against a fabricated corpus instead, which is
    /// strictly stronger: it survives an empty `written` bucket, it names the
    /// exact wrap it defends, and it cannot go quiet because the last live
    /// example moved. The negative half is what makes it a discrimination test
    /// rather than a "does anything match" test.
    #[test]
    fn the_writer_recogniser_reads_across_line_breaks() {
        // Byte-for-byte the shape rustfmt produced for `init_metrics_runtime`.
        let split = "if METRICS_RUNTIME\n    .set(MetricsRuntime { a: 1 })\n    .is_err()\n{}";
        assert!(
            is_written(split, "METRICS_RUNTIME"),
            "the writer arm no longer sees a `.set(` that sits on the line AFTER \
             its receiver. This is how a real capability handle drops off the \
             roster with no other signal: it reads as a lazy cache, the total \
             moves by one, and the only explanation on offer is \"someone deleted \
             a static\"."
        );
        // Same bytes, one line: the recogniser must not have become "matches
        // anything containing the name".
        assert!(
            is_written("METRICS_RUNTIME.set(v);", "METRICS_RUNTIME"),
            "the contiguous form must match too"
        );
        assert!(
            !is_written(
                "let x = METRICS_RUNTIME\n    .get()\n    .copied();",
                "METRICS_RUNTIME"
            ),
            "a reader is not a writer — if this fails the recogniser has \
             degraded to \"the name appears near a method call\", which would \
             select every lazy cache in src/"
        );
    }

    /// The known gap, pinned as a fact rather than left in prose: `MOA_CONFIG`
    /// is `OnceLock<RwLock<Option<T>>>`, lazily built with no argument and then
    /// filled THROUGH the lock. It has the round-7 failure semantics and neither
    /// install form can see it — the whole `OnceLock<Lock<Option<T>>>` class is
    /// below this rule's resolution. See the module doc's "Known gap" section.
    ///
    /// This test does not assert the gap is acceptable. It asserts the gap is
    /// still THIS gap, so that the day `MOA_CONFIG` grows a real writer, the
    /// number moves for a reason someone reads about instead of guesses at.
    #[test]
    fn the_interior_mutable_install_class_is_below_this_rules_resolution() {
        let src = std::fs::read_to_string(
            std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
                .join("src/providers/moa/config_handle.rs"),
        )
        .expect("config_handle.rs");
        let prod = strip_comment_lines(&production_prefix(&src));
        assert!(
            prod.contains("static MOA_CONFIG:"),
            "MOA_CONFIG is gone or renamed"
        );
        assert!(
            !is_written(&prod, "MOA_CONFIG"),
            "MOA_CONFIG now has a writer: it is selectable by install form 1, the \
             count moves by one, and the module doc's known-gap section should \
             record that its confirmed instance is gone."
        );
        let c = take_census();
        assert!(
            c.lazy
                .iter()
                .any(|s| s.file.ends_with("src/providers/moa/config_handle.rs")
                    && s.name == "MOA_CONFIG"),
            "MOA_CONFIG is no longer a candidate at all — the recogniser stopped \
             seeing its declaration, which looks exactly like a correct exclusion"
        );
    }

    /// Test-only statics are removed by the extractor, not by a name filter.
    ///
    /// Both of these sit inside a `#[cfg(test)]` **function** — the shape the
    /// deleted `split("#[cfg(test)]")` idiom got right only by cutting the whole
    /// tail of the file. They must be absent from BOTH halves of the verdict:
    /// absent from the handles is not evidence (the install predicates would
    /// also have excluded them), absent from the candidates is.
    #[test]
    fn test_only_statics_are_not_candidates_at_all() {
        let c = take_census();
        for (file, name) in [
            ("src/providers/moa/config_handle.rs", "LOCK"),
            ("src/session/store.rs", "TEST_STORE"),
        ] {
            let in_handles = c
                .written
                .iter()
                .chain(c.first_caller.iter())
                .chain(c.slots.iter())
                .any(|s| s.file.ends_with(file) && s.name == name);
            let in_lazy = c
                .lazy
                .iter()
                .any(|s| s.file.ends_with(file) && s.name == name);
            assert!(
                !in_handles && !in_lazy,
                "{name} lives in a #[cfg(test)] fn in {file} and must not reach \
                 the census at all — seeing it means production_prefix failed to \
                 remove the enclosing item"
            );
        }
    }

    /// `capability_handles()` must label each site the way Tasks 7–11 read it.
    ///
    /// `is_slot` is the projection's only lossy field: the buckets inside this
    /// module know exactly which arm selected a site, and the exported
    /// `HandleSite` collapses that to "already migrated / not yet". Task 11
    /// closes this round by asserting the roster is fully migrated, and it asks
    /// that question through this bool — so a site labelled `is_slot: true`
    /// while still being a raw container would report the migration finished
    /// while it had not. Nothing outside this module can catch that, because
    /// nothing outside this module can see the buckets.
    #[test]
    fn the_exported_projection_labels_slots_and_raw_containers_correctly() {
        let c = take_census();
        let (raw, slots) = (c.written.len() + c.first_caller.len(), c.slots.len());
        let sites = capability_handles();
        assert_eq!(
            sites.len(),
            raw + slots,
            "the projection dropped or duplicated sites"
        );
        assert_eq!(
            sites.iter().filter(|s| s.is_slot).count(),
            slots,
            "is_slot does not agree with the slot bucket. Task 11 asks \"is the \
             migration finished\" through this bool alone."
        );
        assert!(
            sites
                .iter()
                .filter(|s| !s.is_slot)
                .all(|s| !s.container.ends_with("CapabilitySlot")),
            "a site labelled raw carries a slot container type — the two \
             halves of the same fact have drifted. `ends_with`, not `==`: \
             `MutableCapabilitySlot` is the second spelling, and an equality \
             check would have been blind to exactly the one that exists."
        );
    }

    /// A fallible initialiser is an initialiser, not a writer.
    ///
    /// `extension/template.rs::FILE_REF_REGEX` is the only `get_or_try_init`
    /// site in `src/`. It is a compiled-regex cache in the **zero-parameter**
    /// `fn file_ref_regex()`, and its own initialiser comment says the regex is
    /// a compile-time constant whose parse failure would be a programmer error —
    /// so its `Err` arm cannot occur and an "uninstalled" read initialises
    /// itself. None of the round-7 failure semantics, and it was on the
    /// specification's roster purely because `get_or_try_init` sat in `WRITERS`.
    ///
    /// Three-sided on purpose. "Not selected" alone would also pass if form 2
    /// stopped recognising `get_or_try_init` altogether — the arm would simply
    /// never reach this static, and a silent miss is spelled exactly like a
    /// correct exclusion. `saw_install_call` is what tells them apart.
    ///
    /// The day someone writes `X.get_or_try_init(|| from(cfg))`, form 2 selects
    /// it on its own merits and the count moves by derivation.
    #[test]
    fn a_fallible_initialiser_is_not_a_writer() {
        let c = take_census();
        let here = |s: &&HandleSite| {
            s.file.ends_with("src/extension/template.rs") && s.name == "FILE_REF_REGEX"
        };
        assert!(
            !c.written.iter().any(|s| here(&s)),
            "FILE_REF_REGEX is selected by the WRITER arm — `get_or_try_init` is \
             back in WRITERS. A fallible initialiser is still an initialiser; \
             putting it there asks \"did the call succeed\" instead of \"where did \
             the initialiser's data come from\", and that is not the question \
             that decides membership."
        );
        assert!(
            !c.first_caller.iter().any(|s| here(&s)),
            "FILE_REF_REGEX is selected by the first-caller-wins arm, so \
             `fn file_ref_regex()` has grown a parameter that its initialiser \
             uses. If that is a real config-driven install, delete this test and \
             let the count move."
        );
        let site = c
            .lazy
            .iter()
            .find(|s| s.file.ends_with("src/extension/template.rs") && s.name == "FILE_REF_REGEX")
            .expect("FILE_REF_REGEX is not even a candidate — the declaration recogniser stopped matching it");
        assert!(
            site.saw_install_call,
            "form 2 never found an initialiser call on FILE_REF_REGEX. It is the \
             only `get_or_try_init` site in src/, so this means the arm stopped \
             matching that method — and every future `get_or_try_init` install \
             would be missed in silence, which looks identical to a corpus that \
             has none."
        );
        assert!(
            !site.saw_parameterised_get_or_init,
            "FILE_REF_REGEX was declined for lack of parameter USE, but \
             `fn file_ref_regex()` takes no parameters at all — it must be \
             declined for their ABSENCE. A mismatch here means the signature \
             parser is attributing this site to the wrong enclosing function."
        );
    }

    /// The signature parser must actually parse signatures.
    ///
    /// Every guard above that says "excluded" leans on `fn_spans`; if it silently
    /// stopped resolving function bodies, every first-caller-wins verdict would
    /// become "no" and the only visible symptom would be a count that a sincere
    /// fixer edits. This pins the shapes the parser is claimed to handle,
    /// including the two it was written for — a generic list containing `->`,
    /// and a destructured parameter.
    #[test]
    fn the_signature_parser_resolves_the_shapes_it_claims() {
        let src = r#"
fn plain(a: u32, b: &str) { let _ = (a, b); }
fn generic<T: Fn(u32) -> u32>(f: T) -> u32 { f(1) }
fn destructured(Cfg { root, .. }: Cfg) { let _ = root; }
trait T { fn no_body(&self, x: u32) -> u32; }
impl S { fn method(&mut self, cfg: &Cfg) { let _ = cfg; } }
"#;
        let spans = fn_spans(src);
        let params: Vec<Vec<String>> = spans.iter().map(|s| s.params.clone()).collect();
        assert_eq!(
            params,
            vec![
                vec!["a".to_string(), "b".to_string()],
                vec!["f".to_string()],
                vec!["root".to_string()],
                vec!["self".to_string(), "cfg".to_string()],
            ],
            "signature parsing changed. `no_body` must be absent (declaration, no \
             body to enclose anything); `generic` must survive the `->` inside \
             its bound; `destructured` must bind `root`, not nothing."
        );
    }

    // ========================================================================
    // The accessor contract (Task 7 writes it, Tasks 8-10 copy it, Task 11
    // consumes it)
    // ========================================================================

    /// One production `fn … -> &'static dyn SlotStatus` — the roster's entry
    /// point for one migrated handle.
    struct AccessorSite {
        file: String,
        name: String,
        /// The function body, used to tie an accessor to the static it returns.
        /// The BODY, not the name: `&GLOBAL_LEDGER` is a fact the compiler
        /// checks, while a name is a convention that drifts.
        body: String,
        allows_dead_code: bool,
    }

    /// If `text[..at]` (everything left of a `SlotStatus` token) ends with
    /// `&'static dyn [some::path::]`, everything left of that `&`.
    ///
    /// The shared half of both recognisers below: one asks what sits to the
    /// left of the reference (`->` = an accessor, `<` / `[` / `,` = a
    /// collection), the other does not care.
    ///
    /// Token-wise rather than substring-wise so it does not matter where
    /// rustfmt breaks the signature — the same lesson `method_call_open_paren`
    /// carries above: a matcher that only works on one line makes its verdict a
    /// function of line length.
    fn slot_status_ref_at(text: &str, at: usize) -> Option<&str> {
        let mut head = text[..at]
            .trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == ':');
        for token in ["dyn", "'static", "&"] {
            head = head.trim_end().strip_suffix(token)?;
        }
        Some(head)
    }

    /// The offset of the `->` in `-> &'static dyn [path::]SlotStatus`.
    fn slot_status_return_at(text: &str, at: usize) -> Option<usize> {
        let head = slot_status_ref_at(text, at)?
            .trim_end()
            .strip_suffix("->")?;
        Some(head.len())
    }

    /// Every ROSTER declaration in `text` — an item whose type puts
    /// `&'static dyn SlotStatus` in a **collection** position.
    ///
    /// This answers "does the roster exist yet?" without naming it, which is
    /// the whole point: `ALL_SLOTS` appears in no brief and nowhere in `src/`,
    /// so a guard keyed on that identifier would stay green forever if Task 11
    /// picked any other name — and it could not be broken today, which by this
    /// repo's rule means it would not yet be a guard. A roster has to put these
    /// references in a `Vec` / slice / array to be a roster at all, and THAT is
    /// checkable now (`the_roster_recogniser_knows_a_roster_from_an_accessor`).
    ///
    /// Two boundaries, both deliberate and both toward under-seeing:
    ///
    /// - **Item position only.** The enclosing fragment must be a `static`
    ///   declaration or carry a `->`. A `let` binding is NOT a roster — and
    ///   that exclusion is load-bearing rather than tidy:
    ///   `spend::tests::the_slot_accessors_expose_both_handles_to_the_roster`
    ///   contains `let slots: [&'static dyn SlotStatus; 2]`, and
    ///   `production_prefix` strips nothing from a test-only *file* (the
    ///   `#[cfg(test)]` lives on `mod tests;` in the PARENT). Without this
    ///   check that local would be read as a roster and the guard below would
    ///   fire on day one.
    /// - A roster assembled into a local and never named at item level is
    ///   missed. Task 11's roster must be reachable from other modules, so it
    ///   will be a `static` or a `fn`; the miss is theoretical and silent, so
    ///   it is recorded here rather than assumed away.
    fn roster_collections(text: &str) -> Vec<String> {
        let mut out = Vec::new();
        for at in word_occurrences(text, "SlotStatus") {
            let Some(head) = slot_status_ref_at(text, at) else {
                continue;
            };
            let head = head.trim_end();
            // `<` (Vec<…>), `[` (slice or array), `,` (tuple / later element).
            if !head.ends_with(['<', '[', ',']) {
                continue;
            }
            // The enclosing fragment: back to the previous statement or block
            // boundary. `'static` is removed before testing for the `static`
            // keyword, because `&'static ` contains it as a substring.
            let start = head.rfind([';', '{', '}']).map_or(0, |i| i + 1);
            let fragment = head[start..].replace("'static", "");
            if fragment.contains("->") || fragment.contains("static ") {
                out.push(fragment.split_whitespace().collect::<Vec<_>>().join(" "));
            }
        }
        out
    }

    /// Every roster declaration across `src/`.
    fn rosters_in_src() -> Vec<String> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        rust_sources_under(&root)
            .into_iter()
            .flat_map(|(rel, text)| {
                roster_collections(&strip_comment_lines(&production_prefix(&text)))
                    .into_iter()
                    .map(move |d| format!("{rel}: {d}"))
            })
            .collect()
    }

    /// Every roster accessor in `src/`, recognised by RETURN TYPE, never by name.
    ///
    /// ⚠️ The `_slot()` suffix is not the anchor and must not become one: it is
    /// already taken for something else by
    /// `gateway/event_emitter/origin_fanout.rs::registry_slot()` and
    /// `team_fanout.rs::team_event_bus_slot()`, which return
    /// `&'static RwLock<Option<…>>` — and those two are themselves members of
    /// the interior-mutable-install gap this module documents, so the collision
    /// gets worse, not better, if that gap is ever closed.
    /// `-> &'static dyn SlotStatus` is what Task 11's roster actually consumes,
    /// so it is what this matches.
    ///
    /// Comment-stripped first, and that is load-bearing twice over:
    /// `#[allow(dead_code)]` appears in `spend/mod.rs` prose explaining why the
    /// real attribute is there, and any doc comment quoting this return type
    /// would mint a phantom accessor.
    ///
    /// # Line breaks: this recogniser tolerates them; two others do not
    ///
    /// Measured 2026-08-25 rather than assumed, because "the guard is green" and
    /// "the guard cannot see this" print identically and the difference was
    /// argued the wrong way once already.
    ///
    /// **This function is newline-tolerant.** `slot_status_ref_at` peels the
    /// return type with `trim_end().strip_suffix(..)`, and `str::trim_end`
    /// removes `\n` like any other whitespace — the same property `skip_ws`
    /// gives `method_call_open_paren` after the `METRICS_RUNTIME` fix. Verified
    /// against three split forms, including the one **rustfmt actually
    /// produces** for an over-width signature, which breaks after the opening
    /// paren rather than before the arrow:
    ///
    /// ```ignore
    /// pub(crate) fn a_name_long_enough_to_push_this_signature_past_100_cols(
    /// ) -> &'static dyn SlotStatus {
    /// ```
    ///
    /// The discriminating experiment, for whoever repeats it: split an
    /// EXISTING accessor's signature — reformat it, do not delete it — and
    /// confirm `every_migrated_slot_has_a_roster_accessor` stays green. If
    /// this function is blind to the split form, that guard fires and names
    /// the static — the same message it prints for a genuinely missing
    /// accessor, since a blind parser cannot tell the two apart. That is why
    /// the accessor must stay present: with it gone, a red would be
    /// ambiguous between "blind" and "missing", and would prove nothing
    /// about this function. With it present, the accessor's existence is not
    /// in question, so a red can only mean this function failed to find it —
    /// blindness is what remains once "missing" is ruled out by construction.
    ///
    /// This used to be a two-sided cross-check: splitting the signature
    /// **and** deleting that accessor's `#[allow(dead_code)]` made
    /// `every_roster_accessor_still_carries_the_expiring_allow` fire and NAME
    /// it if this function saw the accessor, so which of the two guards fired
    /// told you which side broke, without trusting either green alone —
    /// closed with "one green proves nothing on its own." That guard was
    /// deleted along with the last permit once `ALL_SLOTS` gave every
    /// accessor a real caller (see
    /// `the_expiring_allow_is_gone_once_the_roster_exists`): no permit left
    /// to delete, no second guard left to fire on the "sees it" side. What
    /// is lost is the general disambiguation — a red on
    /// `every_migrated_slot_has_a_roster_accessor` alone still cannot tell
    /// "this function is blind" apart from "the accessor does not exist",
    /// because it never could; the two-guard version never fixed that either,
    /// it just gave the OTHER guard something to say instead. What survives
    /// is this one experiment, where "does not exist" is excluded by not
    /// deleting anything, and a green is proof this function saw the split
    /// form, which is why repeating it below still matters.
    ///
    /// Re-verified 2026-08-25 against the real wrap, not a hand-typed one:
    /// renamed an EXISTING accessor to the over-width name in the `ignore`
    /// block below (kept its body, only the name and its one `ALL_SLOTS`
    /// caller changed), ran `cargo fmt`, confirmed it produced exactly that
    /// wrap, and confirmed `every_migrated_slot_has_a_roster_accessor` stayed
    /// green — this function is not blind to it. Reverted the rename and the
    /// caller.
    ///
    /// ⚠️ **`parse_static_decl` and the slot arm of `take_census` ARE
    /// line-based** — both read `static NAME: Container<` from a single line.
    /// That is safe, and the reason is worth writing down rather than
    /// rediscovering: rustfmt's over-width wrap for a `static` breaks *inside
    /// the generic list*, so line one keeps `static NAME: Container<` intact —
    /// verified for both a raw handle and a slot, census unchanged at 46. Any
    /// other wrap is not a fixed point of rustfmt, so `cargo fmt --check`
    /// rejects it. And if one ever does slip through, it fails LOUDLY: the
    /// handle leaves the census and `every_installed_global_is_a_capability_slot`'s
    /// `raw + slots == 46` assertion reds on the total. Do not "fix" those two
    /// by making them span lines without first producing a wrap that actually
    /// breaks them.
    ///
    /// The `#[allow(dead_code)]` scan below is line-based too, and survives that
    /// same real wrap (the attribute still sits directly above the line carrying
    /// `fn`). A hypothetically split attribute would make it *over*-report,
    /// which is the safe direction.
    fn roster_accessors() -> Vec<AccessorSite> {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src");
        let mut out = Vec::new();
        for (rel, text) in rust_sources_under(&root) {
            let prod = strip_comment_lines(&production_prefix(&text));
            for at in word_occurrences(&prod, "SlotStatus") {
                let Some(arrow) = slot_status_return_at(&prod, at) else {
                    continue;
                };
                let Some(fn_kw) = word_occurrences(&prod[..arrow], "fn").last().copied() else {
                    continue;
                };
                let name_start = skip_ws(prod.as_bytes(), fn_kw + 2);
                let name: String = prod[name_start..]
                    .chars()
                    .take_while(|c| c.is_ascii_alphanumeric() || *c == '_')
                    .collect();
                let Some(open) = prod[at..].find('{').map(|i| at + i) else {
                    continue;
                };
                let Some(close) = matching(&prod, open, b'{', b'}') else {
                    continue;
                };
                // Attribute lines sitting directly above the fn's own line.
                let line_start = prod[..fn_kw].rfind('\n').map_or(0, |i| i + 1);
                let mut allows_dead_code = false;
                let mut before = &prod[..line_start];
                loop {
                    let trimmed = before.trim_end_matches('\n');
                    let start = trimmed.rfind('\n').map_or(0, |i| i + 1);
                    let line = trimmed[start..].trim();
                    if !line.starts_with("#[") {
                        break;
                    }
                    if line.contains("allow(dead_code)") {
                        allows_dead_code = true;
                    }
                    before = &trimmed[..start];
                }
                out.push(AccessorSite {
                    file: rel.clone(),
                    name,
                    body: prod[open + 1..close].to_string(),
                    allows_dead_code,
                });
            }
        }
        out
    }

    /// Every migrated slot has a roster accessor.
    ///
    /// This is the half of the accessor contract that matters, and it went
    /// unnamed until review: a slot with no accessor is silently absent from
    /// Task 11's roster — a capability handle that cannot say whether boot
    /// installed it, i.e. **this round's own defect, reintroduced by its fix**.
    /// It is also the likelier mistake by an order of magnitude. A batch
    /// migrates ~15 handles; the accessor is two lines per handle; omitting two
    /// lines breaks no build, fails no test, and reads as finished work.
    #[test]
    fn every_migrated_slot_has_a_roster_accessor() {
        let c = take_census();
        let accessors = roster_accessors();
        // Vacuity: "no slots to check" and "the recogniser stopped matching"
        // are the same green without this.
        assert!(
            !c.slots.is_empty(),
            "no migrated slots at all — either every handle was reverted, or \
             the slot arm of take_census stopped matching"
        );
        assert!(
            !accessors.is_empty(),
            "{} slots exist but the accessor scan found none, so this guard is \
             about to report every slot as unwired. Check the recogniser \
             (`-> &'static dyn SlotStatus`) before believing the failures below.",
            c.slots.len()
        );
        for slot in &c.slots {
            let wired = accessors
                .iter()
                .any(|a| a.file == slot.file && !word_occurrences(&a.body, &slot.name).is_empty());
            assert!(
                wired,
                "{} in {} is a migrated slot with NO roster accessor. Nothing \
                 else can see this: it compiles, every test passes, and the \
                 handle is simply missing from Task 11's roster — unable to say \
                 whether boot installed it, which is the defect this round \
                 exists to remove. Add, next to the static:\n\n    \
                 #[allow(dead_code)]\n    pub(crate) fn {}_slot() -> &'static \
                 dyn SlotStatus {{ &{} }}\n\n(the name is convention; the \
                 return type is what this guard and the roster both read)\n\n\
                 ⚠️ If the accessor DOES exist but lives in another file, this \
                 message is misleading: the search is scoped to the static's \
                 own file, deliberately, because that is where every migration \
                 so far puts it. Centralising accessors is a legitimate choice \
                 — it just needs this scope widened in the same commit, not an \
                 accessor added twice.",
                slot.name,
                slot.file,
                slot.name.to_lowercase(),
                slot.name
            );
        }
        // F3: this 1:1 invariant used to live in the expiry guard below — the
        // one Task 11 is told to DELETE. "No stray accessor" would therefore
        // have vanished at exactly the moment the roster started consuming
        // accessors, i.e. when it first mattered. It lives here instead,
        // because this guard outlives the roster.
        //
        // ⚠️ ORDER IS LOAD-BEARING: this runs AFTER the per-slot loop above, and
        // it used to run before it. A missing accessor — which this guard's own
        // doc calls the likeliest mistake in the round — trips BOTH assertions,
        // so whichever runs first is the only message anyone reads. Running the
        // count first produced "7 roster accessors for 8 slots. Either a slot
        // has two, or one accessor returns … for something that is not a
        // migrated handle": both explanations false for that defect, and both
        // sending the reader after a stray accessor that does not exist, while
        // the message written for the case — the one above, which names the
        // static and pastes in the fix — was unreachable. A genuinely stray
        // accessor printed the SAME sentence with 9 instead of 7, so two
        // opposite defects differed by one digit.
        //
        // The loop cannot absorb the stray direction (no slot is unwired by an
        // extra accessor), so this assertion still has to exist — it just has to
        // run second. Falsified in both directions.
        assert_eq!(
            accessors.len(),
            c.slots.len(),
            "{} roster accessors for {} slots, and every slot has one. Two \
             causes, and the counts tell them apart:\n\
             \n\
             - MORE accessors than slots: a STRAY accessor — one returns \
             `&'static dyn SlotStatus` for something that is not a migrated \
             handle, or a slot has two. Task 11 would carry it into the roster.\n\
             - FEWER accessors than slots is impossible here (the loop above \
             runs first and names any unwired slot), so if you are reading this \
             with slots > accessors, a slot DECLARATION stopped being \
             recognised: `parse_static_decl` and the slot arm of `take_census` \
             read `static NAME: Container<` from ONE line. Check \
             `every_installed_global_is_a_capability_slot` in the same run — \
             its `raw + slots == 46` assertion will be red too, and its \
             message is the accurate one.\n\
             \n\
             (If you expected 'missing accessor', that is the loop above.)",
            accessors.len(),
            c.slots.len()
        );
    }

    /// The permits are gone once the roster exists.
    ///
    /// This used to be one half of a matched pair. The sibling,
    /// `every_roster_accessor_still_carries_the_expiring_allow`, asserted the
    /// permits were still PRESENT — it could catch a partial removal but not
    /// total inaction, and total inaction was the hazard the exemption was
    /// written against: ~46 permits shipped, each one able to mask a
    /// genuinely unwired handle. It fired the moment `ALL_SLOTS` landed, by
    /// design, and was deleted in the same change that removed the last
    /// `#[allow(dead_code)]`: keeping a guard that is now permanently red is
    /// not "documentation of intent", it is a red CI the next contributor has
    /// to explain away. This guard is what remains — proof the removal was
    /// total, and the one that keeps working after the sibling is gone.
    ///
    /// Proven by mutation, not argued: a roster added with every permit left
    /// in place left the whole suite green before this test existed.
    ///
    /// The trigger is the roster's TYPE SHAPE, never its name — see
    /// `roster_collections` for why `if ALL_SLOTS exists` would have been
    /// unfalsifiable and rename-fragile.
    ///
    /// ⚠️ Not implementable as "does production code call `X_slot()`": the
    /// accessors' only callers today are in `src/spend/tests.rs`, a test-only
    /// *file* whose `#[cfg(test)]` sits on `mod tests;` in the parent, so
    /// `production_prefix` strips nothing from it and a caller scan would read
    /// those as production callers and red on day one. Nor as
    /// `#[expect(dead_code)]`: tests already reach the accessors, so
    /// `dead_code` never fires in the test build and the expectation would be
    /// unfulfilled today.
    #[test]
    fn the_expiring_allow_is_gone_once_the_roster_exists() {
        let accessors = roster_accessors();
        assert!(
            !accessors.is_empty(),
            "the accessor scan found none — with nothing to check this guard \
             reports the same green it reports when every permit is correctly \
             gone. Check the recogniser before trusting it."
        );
        let rosters = rosters_in_src();
        let permits: Vec<String> = accessors
            .iter()
            .filter(|a| a.allows_dead_code)
            .map(|a| format!("{} ({})", a.name, a.file))
            .collect();
        assert!(
            rosters.is_empty() || permits.is_empty(),
            "a roster now exists:\n  {}\n\nbut these accessors still carry \
             `#[allow(dead_code)]`:\n  {}\n\nThe permit's stated reason — \"no \
             production caller yet\" — has expired. Remove it from EVERY \
             accessor with a permit. A permit left on an accessor whose \
             consumer exists silences a real `dead_code`, which is how a \
             handle that nothing wires reaches the roster looking wired.",
            rosters.join("\n  "),
            permits.join("\n  ")
        );
    }

    /// The roster recogniser, exercised on synthetic declarations.
    ///
    /// Necessary because `src/` contains no roster yet: without this, a
    /// recogniser that silently stopped matching would look exactly like "the
    /// roster has not landed", and the guard above would stay green forever
    /// with every permit shipped. This is the same vacuity trap the two scans
    /// above guard with `!is_empty()`, in the one place where the corpus cannot
    /// supply a positive case.
    #[test]
    fn the_roster_recogniser_knows_a_roster_from_an_accessor() {
        for src in [
            "static ALL_SLOTS: &[&'static dyn SlotStatus] = &[];",
            "pub fn all_slots() -> Vec<&'static dyn SlotStatus> { vec![] }",
            "fn r() -> [&'static dyn SlotStatus; 2] { todo!() }",
            "fn r() -> Vec<&'static dyn crate::capability::SlotStatus> { todo!() }",
            "pub static SLOT_ROSTER: &[&'static dyn SlotStatus] = &[a(), b()];",
        ] {
            assert!(
                !roster_collections(src).is_empty(),
                "a roster this shape is not recognised, so the expiry guard \
                 would never fire for it: {src}"
            );
        }
        for src in [
            // An accessor is not a roster.
            "pub(crate) fn global_ledger_slot() -> &'static dyn SlotStatus { &X }",
            // ⚠️ This exact line is in `src/spend/tests.rs`, which
            // `production_prefix` does not strip (its `#[cfg(test)]` is on
            // `mod tests;` in the parent). Reading it as a roster would red the
            // expiry guard on day one.
            "let slots: [&'static dyn SlotStatus; 2] = [global_ledger_slot(), global_policy_slot()];",
            "let erased: &dyn SlotStatus = &GLOBAL_POLICY;",
        ] {
            assert!(
                roster_collections(src).is_empty(),
                "recognised as a roster when it is not one — the expiry guard \
                 would fire before any roster exists: {src}"
            );
        }
    }

    // =====================================================================
    // Conditional installs (Task 14)
    // =====================================================================

    /// Production lines of `text`, comment-free, each paired with its ORIGINAL
    /// 1-based line number.
    ///
    /// `production_prefix` first, then `strip_comment_lines` — the order this
    /// repo's `source_scan` doc requires, because dropping comment lines first
    /// can discard production code that shares a line with a delimiter.
    ///
    /// Both of those functions DROP lines rather than blanking them, so an
    /// index into the result is not a line number. Reporting one as if it were
    /// is not cosmetic: the first draft of this guard named
    /// `src/bin/aleph-server/main.rs:16` for a site at line 79, and a
    /// coordinate that points at unrelated code is worse than none — the reader
    /// concludes the guard is broken and stops reading it. Recovered by a
    /// two-pointer walk: the kept lines are a subsequence of the original ones,
    /// content unchanged, so each match resyncs the cursor.
    fn prod_lines(text: &str) -> (Vec<usize>, Vec<String>) {
        let original: Vec<String> = text.replace('\r', "").lines().map(str::to_string).collect();
        let kept: Vec<String> = strip_comment_lines(&production_prefix(text))
            .lines()
            .map(str::to_string)
            .collect();
        let mut nums = Vec::with_capacity(kept.len());
        let mut cursor = 0usize;
        for line in &kept {
            while cursor < original.len() && &original[cursor] != line {
                cursor += 1;
            }
            nums.push(cursor + 1);
            cursor += 1;
        }
        (nums, kept)
    }

    fn indent_of(line: &str) -> usize {
        line.len() - line.trim_start().len()
    }

    /// `(body, index_of_closing_line)` for the block opening at `start`.
    ///
    /// The block ends at the first line that starts with `}` at an indent <=
    /// the opener's — the block's own syntactic terminus. NOT a line budget:
    /// a fixed-size window reads into whatever follows, and this repo has
    /// shipped a guard that passed because its 400-character window read the
    /// neighbouring declaration.
    fn block_at(lines: &[String], start: usize) -> (String, usize) {
        let indent = indent_of(&lines[start]);
        let mut body = vec![lines[start].clone()];
        for (offset, line) in lines[start + 1..].iter().enumerate() {
            if !line.trim().is_empty()
                && indent_of(line) <= indent
                && line.trim_start().starts_with('}')
            {
                return (body.join("\n"), start + 1 + offset);
            }
            body.push(line.clone());
        }
        (body.join("\n"), lines.len().saturating_sub(1))
    }

    /// Is `line` a call to the free/associated function `name`?
    ///
    /// Rejects an identifier byte before the name (so `handle_plugins_install(`
    /// is not a call to `install`) and rejects a `.` (so `x.install(` — a method
    /// on a value — is not a call to the module-level wrapper).
    fn calls(line: &str, name: &str) -> bool {
        let bytes = line.as_bytes();
        let mut from = 0usize;
        while let Some(rel) = line[from..].find(name) {
            let at = from + rel;
            let after = at + name.len();
            let ok_before = at == 0 || (!is_ident_byte(bytes[at - 1]) && bytes[at - 1] != b'.');
            if ok_before && line[after..].starts_with('(') {
                return true;
            }
            from = after;
        }
        false
    }

    fn is_fn_definition(line: &str) -> bool {
        let t = strip_visibility(line.trim_start()).trim_start();
        let t = t.strip_prefix("async ").unwrap_or(t);
        let t = t.strip_prefix("const ").unwrap_or(t);
        t.starts_with("fn ")
    }

    /// The slot statics `body` calls `method` on, as `"{file}::{STATIC}"` keys.
    ///
    /// Whitespace-tolerant through `method_call_open_paren`, so `X\n  .install(v)`
    /// counts — the same reason this module's writer recogniser is, and the same
    /// failure if it were not (`rustfmt` decides where the break goes).
    fn slots_touched(
        body: &str,
        statics: &[String],
        method: &str,
        file: &str,
    ) -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for name in statics {
            let hit = word_occurrences(body, name)
                .into_iter()
                .any(|at| method_call_open_paren(body, at + name.len(), method).is_some());
            if hit {
                out.insert(format!("{file}::{name}"));
            }
        }
        out
    }

    /// Names of the slot statics declared in one file's production half.
    fn slot_statics(lines: &[String]) -> Vec<String> {
        lines
            .iter()
            .filter_map(|l| {
                let t = strip_visibility(l.trim_start());
                let rest = t.strip_prefix("static ")?;
                if !rest.contains("CapabilitySlot<") {
                    return None;
                }
                let (name, _) = rest.split_once(':')?;
                Some(name.trim().to_string())
            })
            .collect()
    }

    /// Both halves of the capability wrapper vocabulary, derived crate-wide.
    ///
    /// A wrapper is a `pub`-ish fn whose body calls `.install(` (or `.decline(`)
    /// on a slot static declared in its own file. Both maps carry the SLOT keys
    /// the body touches, not just the name — which is what lets the gate rule
    /// below ask "was *this* handle declined" instead of "was the word `decline`
    /// present somewhere".
    ///
    /// ⚠️ The file-declares-a-slot condition removes NOTHING today — measured
    /// 2026-08-25: 44 functions qualify on the `.install(` clause alone and all
    /// 44 survive the slot clause. `service::platform::install`,
    /// `ResolverScope::install`, `runtimes::bootstrap::install` and
    /// `security::audit::install_global` — the four an earlier draft of this doc
    /// claimed it excluded — are already excluded by the first clause, because
    /// none of them contains `.install(` in its body. The condition is kept for
    /// the case it *would* catch: a future `fn install_x()` that calls
    /// `SOMETHING.install(v)` on a non-slot container in a file with no slot.
    /// The real over-see is the one named below, and it is name-keying.
    ///
    /// ⚠️ Keyed on the bare NAME, so five wrappers called `init_global` in five
    /// modules are one entry. Direction: over-see for the call-site scan (three
    /// of those four functions above DO have their call sites examined, through
    /// the bare names `install` / `install_global` that other files contribute),
    /// and over-see for the hazard rule. Both fail loudly — a demanded `decline`
    /// that should not be there, or a forbidden one that is — rather than going
    /// quiet.
    ///
    /// Measured 2026-08-25, three entries own more than one slot and so land in
    /// [`GateInstalls`]'s `ambiguous` bucket — but only two of the three are
    /// name collisions, and the difference matters when reading a failure:
    ///
    /// * `init_global` — 5 slots, 5 modules. A collision.
    /// * `set_global` — 2 slots, 2 modules (`codex_token_refresher`,
    ///   `gateway::security::shared_token`). A collision.
    /// * `install` — 2 slots, **one function**: `identity::ledger::install`
    ///   installs `LEDGER` and `WRITER` together. Not a collision at all; it
    ///   shares the bucket because the bucket's test is "more than one slot",
    ///   which is the right test for the gate rule and the wrong word for this
    ///   paragraph.
    ///
    /// ⚠️ [`every_decline_wrapper_has_a_production_caller`] inherits the same
    /// name-keying in the OPPOSITE direction — **under**-see. `decline_global` is
    /// defined in two files (`tasks/cron/mod.rs`,
    /// `gateway/codex_token_refresher.rs`) and `decline_file` records only one,
    /// so a caller of either satisfies both and one of the two could lose its
    /// only caller unnoticed. Not over-see, which fails loudly; this one goes
    /// quiet.
    struct CapWrappers {
        installs: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
        declines: std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
        /// decline-wrapper name -> the file that defines it.
        decline_file: std::collections::BTreeMap<String, String>,
    }

    fn capability_wrappers() -> CapWrappers {
        let mut w = CapWrappers {
            installs: std::collections::BTreeMap::new(),
            declines: std::collections::BTreeMap::new(),
            decline_file: std::collections::BTreeMap::new(),
        };
        for (rel, text) in rust_sources_under(&manifest_src()) {
            if !text.contains("CapabilitySlot") {
                continue;
            }
            let (_, lines) = prod_lines(&text);
            let statics = slot_statics(&lines);
            if statics.is_empty() {
                continue;
            }
            for i in 0..lines.len() {
                if !is_fn_definition(&lines[i]) {
                    continue;
                }
                let t = strip_visibility(lines[i].trim_start()).trim_start();
                let t = t.strip_prefix("async ").unwrap_or(t);
                let Some(rest) = t.strip_prefix("fn ") else {
                    continue;
                };
                let Some(name) = rest.split('(').next() else {
                    continue;
                };
                let name = name.split('<').next().unwrap_or(name).trim();
                if name.is_empty() {
                    continue;
                }
                let (body, _) = block_at(&lines, i);
                let installs = slots_touched(&body, &statics, "install", &rel);
                if !installs.is_empty() {
                    w.installs
                        .entry(name.to_string())
                        .or_default()
                        .extend(installs);
                }
                let declines = slots_touched(&body, &statics, "decline", &rel);
                if !declines.is_empty() {
                    w.declines
                        .entry(name.to_string())
                        .or_default()
                        .extend(declines);
                    w.decline_file.insert(name.to_string(), rel.clone());
                }
            }
        }
        w
    }

    /// The cargo target a source file belongs to.
    ///
    /// `src/bin/<name>/**` is its own binary crate; everything else under `src/`
    /// is the library.
    fn target_of(rel: &str) -> String {
        rel.strip_prefix("src/bin/")
            .and_then(|r| r.split('/').next())
            .map_or_else(|| "lib".to_string(), |bin| format!("bin:{bin}"))
    }

    /// `(target, fn name)` -> the slots that function's body installs (or
    /// declines) through the wrapper vocabulary, and how many definitions of
    /// that name merged into the entry.
    type HopMap =
        std::collections::BTreeMap<(String, String), (std::collections::BTreeSet<String>, usize)>;

    /// One hop of call-graph reach, per binary target.
    struct Hop {
        installs: HopMap,
        declines: HopMap,
    }

    fn one_hop(w: &CapWrappers, test_only: &std::collections::BTreeSet<String>) -> Hop {
        let mut hop = Hop {
            installs: HopMap::new(),
            declines: HopMap::new(),
        };
        for (rel, text) in rust_sources_under(&manifest_src()) {
            if rel.starts_with("src/capability/") || test_only.contains(&rel) {
                continue;
            }
            let target = target_of(&rel);
            if !target.starts_with("bin:") {
                continue; // see `hoppable`: the library gets no hop
            }
            let (_, lines) = prod_lines(&text);
            let statics = slot_statics(&lines);
            for i in 0..lines.len() {
                if !is_fn_definition(&lines[i]) {
                    continue;
                }
                let t = strip_visibility(lines[i].trim_start()).trim_start();
                let t = t.strip_prefix("async ").unwrap_or(t);
                let t = t.strip_prefix("const ").unwrap_or(t);
                let Some(rest) = t.strip_prefix("fn ") else {
                    continue;
                };
                let Some(name) = rest.split('(').next() else {
                    continue;
                };
                let name = name.split('<').next().unwrap_or(name).trim().to_string();
                if name.is_empty() {
                    continue;
                }
                let (body, _) = block_at(&lines, i);
                let installs = slots_in(&body, &w.installs, &statics, &rel, "install");
                // ⚠️ SUBTRACT THE OVERLAP. A function's hop sets are whole-body
                // unions — nothing here knows which internal branch produced
                // which slot — so a function that installs a handle in one gate
                // and declines it in that gate's `else` would otherwise be
                // registered as a *decliner* of a handle it installs, and could
                // then satisfy any gate arm in this binary that merely names it.
                //
                // That is not a hypothetical shape, and it is not a badly
                // written function: it is exactly what "own your gate, decline
                // in your own `else`" produces. Measured 2026-08-25, five of the
                // eight install-side entries were self-decliners
                // (`register_agent_handlers` 7/7, `start_server` 25/8,
                // `initialize_channels`, `initialize_extension_manager`, `main`).
                // Forced: replacing `agent_init`'s seven simulated-mode declines
                // with one call to `register_agent_handlers` — the INSTALLER —
                // took the gate rule from naming all seven to naming one.
                //
                // Costs the shipped case nothing: `decline_orchestrator_slots`
                // installs none of the four handles it declines. Fails loud — a
                // self-declining wiring function stops satisfying gates it
                // should never have satisfied.
                //
                // ⚠️ `block_at` ends at the fn's own syntactic terminus, so a
                // nested fn or closure counts as part of the enclosing body —
                // which is why `start_server`'s install set is 25 slots wide.
                // Harmless today (nothing calls `start_server` from inside a
                // gate arm) and it is the same whole-body union one scale up,
                // so the subtraction above covers it too.
                let declines: std::collections::BTreeSet<String> =
                    slots_in(&body, &w.declines, &statics, &rel, "decline")
                        .difference(&installs)
                        .cloned()
                        .collect();
                for (map, slots) in [(&mut hop.installs, installs), (&mut hop.declines, declines)] {
                    if slots.is_empty() {
                        continue;
                    }
                    let e = map
                        .entry((target.clone(), name.clone()))
                        .or_insert_with(|| (std::collections::BTreeSet::new(), 0));
                    e.0.extend(slots);
                    e.1 += 1;
                }
            }
        }
        hop
    }

    /// The `(name, slots)` pairs a gate in `file` may hop through.
    ///
    /// **Two restrictions. The first is forced by measurement; the second is
    /// reasoned, and saying which is which matters because an earlier draft of
    /// this doc credited the first one's evidence to the second.**
    ///
    /// 1. **Binary targets only — measured.** A gate in the library gets no hop.
    ///    Hopping symmetrically inside `lib` takes the gate count from 15 to 77
    ///    and produces **61 offenders, every one of them false**, because
    ///    `ExecutionEngine::new` installs `gateway/concurrency-limiter` and, once
    ///    a hop is keyed on a bare fn name, `new(` appears in almost every
    ///    `if`/`else` body in the crate — ~918 `fn new(` definitions under
    ///    `src/`, exactly one of them slot-bearing. What makes a binary
    ///    different is not its path: `src/bin/<name>/` is its own crate, so a
    ///    function defined there can only be called from that binary, and a gate
    ///    there is the last word on what this process wires. A library function
    ///    is a component that does not know its callers — the same argument this
    ///    round used when it refused to decline `src/executor/`'s handles from
    ///    `agent_init`.
    /// 2. **Unique names only — reasoned, not measured, and it counts
    ///    SLOT-BEARING definitions.** `one_hop` only creates an entry when a
    ///    body yields a non-empty slot set, so `defs` counts the definitions of
    ///    that name which install or decline something, not the definitions of
    ///    that name. A second `fn decline_orchestrator_slots` with an empty body
    ///    does **not** suppress the hop (forced 2026-08-25). That is the better
    ///    of the two behaviours — refusing to hop because of an unrelated
    ///    same-named function would be a false offender — but it is not what an
    ///    earlier draft of this sentence said.
    ///
    ///    Measured 2026-08-25: all **8** install-side entries in
    ///    `bin:aleph-server` have `defs == 1`, so this filter has never excluded
    ///    anything. It is belt-and-braces against a future collision, and it
    ///    contributed **nothing** to the 61 above: running the symmetric hop with
    ///    the filter and without it gives 77 gates / 61 offenders either way,
    ///    because `new` is slot-bearing exactly once. Restriction 1 is the whole
    ///    of that argument.
    ///
    /// Direction of both: under-see.
    fn hoppable<'a>(
        hop: &'a HopMap,
        file: &str,
    ) -> Vec<(&'a String, &'a std::collections::BTreeSet<String>)> {
        let target = target_of(file);
        if !target.starts_with("bin:") {
            return Vec::new();
        }
        hop.iter()
            .filter(|((t, _), (_, defs))| t == &target && *defs == 1)
            .map(|((_, name), (slots, _))| (name, slots))
            .collect()
    }

    /// Names of the `pub fn`s that install a capability — the vocabulary the
    /// conditional-site scan matches call sites against.
    fn install_wrapper_names() -> std::collections::BTreeSet<String> {
        capability_wrappers().installs.into_keys().collect()
    }

    fn manifest_src() -> std::path::PathBuf {
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
    }

    /// Files that only exist in a `cfg(test)` build, derived from the parents
    /// that declare them.
    ///
    /// `production_prefix` works one file at a time, so a file whose ENTIRE
    /// contents are test code — because its parent wrote `#[cfg(test)] mod
    /// tests;` — is fully "production" to any scanner that only asks the file
    /// itself. `src/spend/tests.rs` is the case this module's own roster
    /// recogniser already had to reason about; this makes the exclusion a rule
    /// instead of a per-guard workaround.
    fn test_only_module_files() -> std::collections::BTreeSet<String> {
        let mut out = std::collections::BTreeSet::new();
        for (rel, text) in rust_sources_under(&manifest_src()) {
            for line in cfg_test_portion(&text).lines() {
                let t = strip_visibility(line.trim_start()).trim_start();
                let Some(rest) = t.strip_prefix("mod ") else {
                    continue;
                };
                let Some(name) = rest.strip_suffix(';') else {
                    continue; // `mod tests {` is inline, not a separate file
                };
                let name = name.trim();
                if name.is_empty() || !name.bytes().all(is_ident_byte) {
                    continue;
                }
                let dir = rel
                    .strip_suffix("/mod.rs")
                    .or_else(|| rel.strip_suffix(".rs"))
                    .unwrap_or(&rel);
                out.insert(format!("{dir}/{name}.rs"));
                out.insert(format!("{dir}/{name}/mod.rs"));
            }
        }
        out
    }

    /// The text of every arm after the first in the `if`-chain whose first block
    /// closes at `first_close`. `None` when the chain has no `else` at all.
    ///
    /// Shared by [`governing_alternative`] and the gate rule, so "what counts as
    /// the else side of a conditional" has one answer rather than two that drift.
    fn else_chain(lines: &[String], first_close: usize) -> Option<String> {
        if !lines
            .get(first_close)
            .is_some_and(|l| l.trim_start().starts_with("} else"))
        {
            return None;
        }
        let mut out = Vec::new();
        let mut cur = first_close;
        loop {
            let (body, c) = block_at(lines, cur);
            out.push(body);
            if lines
                .get(c)
                .is_some_and(|l| l.trim_start().starts_with("} else"))
            {
                cur = c;
            } else {
                break;
            }
        }
        Some(out.join("\n"))
    }

    /// The sibling arms of the construct that governs the install at `site`.
    ///
    /// Three-way on purpose, and the middle case is the whole point:
    ///
    /// * `None` — nothing conditional encloses this install. Skip it.
    /// * `Some("")` — a conditional encloses it and there is NO other arm.
    ///   **This is the defect the task exists to close** (`if cond { install() }`
    ///   with no `else`), so it must reach the caller as an examined site with
    ///   no decline, never as a skip. The first draft of this function folded it
    ///   into `None`; the guard then went green while being structurally unable
    ///   to find a missing `else`, and the two real remaining sites
    ///   (`src/config/load.rs`) were invisible to it.
    /// * `Some(text)` — the sibling arms, to be searched for a `decline`.
    ///
    /// ## Shapes this does not read, and why they are `None` rather than loud
    ///
    /// The opener is the first strictly-shallower line. Three shapes reach it
    /// that this scan cannot judge: an install inside an `else` / `} else if`
    /// arm, a braceless match arm (`Ok(x) => install(x),`), and a bare
    /// `match x {` directly above the call. A draft returned `Some("")` for all
    /// three so they would surface as offenders a human must look at. Measured
    /// against the real tree, that produced exactly two hits and BOTH were
    /// false:
    ///
    /// * `src/config/load.rs` — `init_metrics_runtime` sits in `Config::load`'s
    ///   no-config-file arm. Its sibling arm installs the same handle too, one
    ///   call deeper (`load_from_file` → `load_from_file_reporting_dead_keys`),
    ///   so nothing is absent and no textual rule can see that. It is also one
    ///   of the two decline-then-install hazards, so a `decline` added to
    ///   satisfy a red here would be a defect, not a fix.
    /// * `src/bin/aleph-server/commands/service/mod.rs` — `platform::install()`
    ///   is the OS service installer, sharing a bare name with
    ///   `identity::ledger::install`. The over-see this module's wrapper-set doc
    ///   warns about, landing.
    ///
    /// A red that is wrong twice out of twice teaches the reader to stop reading
    /// the guard, so these three shapes are `None`. Direction: **under-see** —
    /// an install in an `else` arm whose sibling does NOT install would be
    /// skipped. Zero such sites in `src/` as of 2026-08-25.
    fn governing_alternative(lines: &[String], site: usize) -> Option<String> {
        let my = indent_of(&lines[site]);
        let mut opener = None;
        for j in (0..site).rev() {
            if lines[j].trim().is_empty() {
                continue;
            }
            if indent_of(&lines[j]) >= my {
                continue;
            }
            opener = Some(j);
            break; // first shallower line decides; do not keep walking out
        }
        let g = opener?;
        let t = lines[g].trim_start();

        if t.starts_with("if ") || t.starts_with("if let ") {
            let (_, first_close) = block_at(lines, g);
            // `None` here is "conditional, and no alternative exists to hold a
            // `decline`" — the defect, not a skip.
            return Some(else_chain(lines, first_close).unwrap_or_default());
        }

        // A `match` arm: the alternative is every OTHER arm of the same match.
        if t.ends_with("=> {") {
            let arm_indent = indent_of(&lines[g]);
            let mut head = None;
            for j in (0..g).rev() {
                if lines[j].trim().is_empty() {
                    continue;
                }
                if indent_of(&lines[j]) >= arm_indent {
                    continue;
                }
                head = Some(j);
                break;
            }
            let m = head?;
            if !lines[m].contains("match ") {
                return None;
            }
            let (_, m_close) = block_at(lines, m);
            let (_, arm_close) = block_at(lines, g);
            let mut out = Vec::new();
            for (offset, line) in lines[m + 1..m_close.min(lines.len())].iter().enumerate() {
                let idx = m + 1 + offset;
                if idx >= g && idx <= arm_close {
                    continue; // the install's own arm is not its own alternative
                }
                out.push(line.clone());
            }
            return Some(out.join("\n"));
        }

        // Governed by something this scan does not parse — see the doc above
        // for the two measured instances and why loud was the wrong answer.
        None
    }

    struct CondSite {
        wrapper: String,
        at: String,
        declines: bool,
    }

    /// The call site's fully qualified path for `wrapper`, e.g.
    /// `alephcore::tasks::cron::init_global`, falling back to the bare name
    /// when the call is written unqualified.
    ///
    /// Rule 2 below asks "can THIS handle be declined and then installed in one
    /// process", and the bare wrapper name cannot answer it: `init_global` is
    /// five different functions in five modules, so the moment a second one of
    /// them got a conditional boot site the rule reported a hazard between two
    /// slots that never touch each other. The qualifier written at the call
    /// site is the cheapest thing that separates them, and it keeps the two
    /// genuinely-paired sites together (`config/load.rs`'s two
    /// `init_defaults_override` calls are both written
    /// `crate::config::defaults_override::init_defaults_override`).
    fn qualified_call(line: &str, wrapper: &str) -> String {
        let bytes = line.as_bytes();
        let mut from = 0usize;
        while let Some(rel) = line[from..].find(wrapper) {
            let at = from + rel;
            let after = at + wrapper.len();
            let ok_before = at == 0 || (!is_ident_byte(bytes[at - 1]) && bytes[at - 1] != b'.');
            if ok_before && line[after..].starts_with('(') {
                let mut start = at;
                while start > 0 && (is_ident_byte(bytes[start - 1]) || bytes[start - 1] == b':') {
                    start -= 1;
                }
                return line[start..after].to_string();
            }
            from = after;
        }
        wrapper.to_string()
    }

    /// Every conditional capability install either says why it was skipped, or
    /// is one of the pairs that structurally cannot.
    ///
    /// Two rules, and the second is the reason this is not a one-sided check:
    ///
    /// 1. A wrapper with exactly ONE conditional call site MUST have a
    ///    `decline` in the governing alternative. Absent it, "deliberately not
    ///    configured" and "wiring gap" are the same reading, which is the
    ///    silence this whole round exists to remove.
    /// 2. A wrapper with TWO OR MORE conditional call sites must have NO
    ///    `decline` anywhere among them. Two conditional sites means
    ///    decline-then-install is reachable in one process, and
    ///    `capability::Outcome` cannot describe that sequence — first writer
    ///    wins, so the stamp would keep saying `Declined` about an installed
    ///    handle. Pinned by
    ///    `capability::tests::decline_then_install_is_the_one_pair_this_type_cannot_describe`.
    ///    Today's only member is `init_defaults_override`
    ///    (`src/config/load.rs`, the two mutually-exclusive paths of
    ///    `Config::load`, which runs many times per process).
    ///
    /// Rule 2 keeps rule 1 from rotting in the dangerous direction: adding a
    /// second conditional site next to an existing `decline` makes that decline
    /// hazardous, and this goes red at the named line instead of leaving a
    /// sentence in front of an operator describing a state the process has left.
    ///
    /// ## What this cannot see
    ///
    /// * **Multi-CALL, single-site.** Rule 2 approximates reachability by
    ///   counting sites. One conditional site inside a function that runs many
    ///   times per process reaches the same hazard and is not detected.
    ///   Direction: under-see. Re-checked 2026-09-04: the 20 sites resolve to 19
    ///   distinct wrappers, 18 of them single-site (the 19th is
    ///   `init_defaults_override`, the two-site exempt pair), and none of those
    ///   18 is multi-call — so the rule holds — but the reason is
    ///   per-wrapper, not "boot runs once". An earlier draft justified it with
    ///   "(every conditional install is on a once-per-process boot path)",
    ///   which this guard's own exempt pair falsifies: `config/load.rs:208` and
    ///   `:342` are conditional installs in `Config::load`, and the entire
    ///   ruling at those two sites rests on `Config::load` running many times
    ///   per process. They are safe because rule 2 already forbids stamping
    ///   them, not because they run once.
    /// * **One wrapper called under two different spellings.** Sites are
    ///   grouped by the qualified path written at the call site
    ///   ([`qualified_call`]), so a wrapper reached once as
    ///   `crate::x::init_global(..)` and once as a `use`-imported bare
    ///   `init_global(..)` lands in two groups and rule 2 does not see the
    ///   pair. Direction: under-see, and it is the price of no longer
    ///   conflating five different `init_global`s into one handle — which was
    ///   over-see loud enough to fail on an unrelated slot being added
    ///   (measured 2026-09-04, when `tasks::heartbeat::init_global` joined
    ///   `tasks::cron::init_global`). Zero instances in `src/` today, and that
    ///   is asserted rather than asserted-in-prose: the body below refuses any
    ///   site whose wrapper is called unqualified, which is the only spelling
    ///   that can produce the pair. ⚠️ The first draft of this paragraph said
    ///   "zero instances" as prose and it was FALSE — `start/mod.rs`'s Codex
    ///   refresher gate called a `use`-imported bare `set_global`, a name two
    ///   modules own. The assertion found it on its first run; the prose would
    ///   have gone on being cited.
    /// * **Which slot was declined.** The check is `contains("decline")` over
    ///   the alternative, so an arm that declines a DIFFERENT handle satisfies
    ///   it. Deriving the expected `decline_*` name from the install wrapper's
    ///   name was rejected: `ensure_dream_daemon_with_orientation` /
    ///   `decline_dream_daemon` already breaks the convention, so the rule
    ///   would need an exception list on day one.
    ///   [`every_slot_installed_inside_a_gate_is_declined_in_its_else`] asks the
    ///   per-slot question this one cannot, by resolving wrappers to the slot
    ///   statics they touch rather than to their names — the two are
    ///   complementary and neither subsumes the other.
    /// * **Conditionals above the nearest one.** The opener is the first
    ///   strictly-shallower line, matching this scan's stated rule; an install
    ///   nested three blocks inside a gate is judged against the innermost. The
    ///   gate rule named above covers this direction, because it starts from the
    ///   gate rather than from the install.
    /// * **A multi-line `if` condition whose `{` sits on its own line.** The
    ///   opener then resolves to a bare `{` and the install reads as
    ///   unconditional. Zero instances in `src/` on 2026-08-25 — the
    ///   orchestrator gate's `if let (Some(default_provider), Some(session_service))`
    ///   is single-line — but it is one `rustfmt` reflow away, so it is named
    ///   here rather than discovered later. The gate rule sees these (its opener
    ///   test is the `if` line itself, not the line above the body).
    #[test]
    fn no_conditional_capability_install_is_silent() {
        let wrappers = install_wrapper_names();
        assert!(
            wrappers.len() >= 39,
            "derived only {} install wrappers; >=39 expected (the live count on \
             2026-08-25). A derivation that stopped matching makes this guard \
             pass by finding nothing.",
            wrappers.len()
        );
        let test_only = test_only_module_files();

        let mut sites: Vec<CondSite> = Vec::new();
        let mut suppressed = 0usize;
        for (rel, text) in rust_sources_under(&manifest_src()) {
            if rel.starts_with("src/capability/") {
                continue;
            }
            let is_fixture = test_only.contains(&rel);
            let present: Vec<&String> = wrappers.iter().filter(|w| text.contains(*w)).collect();
            if present.is_empty() {
                continue;
            }
            let (nums, lines) = prod_lines(&text);
            for i in 0..lines.len() {
                if is_fn_definition(&lines[i]) {
                    continue;
                }
                let Some(w) = present.iter().find(|w| calls(&lines[i], w)) else {
                    continue;
                };
                let Some(alt) = governing_alternative(&lines, i) else {
                    continue; // unconditional install: nothing to explain
                };
                if is_fixture {
                    suppressed += 1;
                    continue;
                }
                sites.push(CondSite {
                    // The QUALIFIED path, not the bare name — see
                    // `qualified_call`. Grouping rule 2 on the bare name makes
                    // two unrelated slots read as one handle.
                    wrapper: qualified_call(&lines[i], w),
                    at: format!("{rel}:{}", nums[i]),
                    declines: alt.contains("decline"),
                });
            }
        }

        assert!(
            sites.len() >= 20,
            "examined only {} conditional installs; 20 were measured on \
             2026-09-04 (19 on 2026-08-25, plus the heartbeat service's boot \
             gate), and this floor sits flush against that measurement on \
             purpose. An earlier draft left it at 15 — four below — and the \
             slack was not caution, it was already-issued permission: it turned \
             a mutation the guard CAN see (reverting the if-with-no-`else` fix, \
             which costs exactly two sites) into a documented 'boundary'. \
             Zero-or-few is how this guard reports 'all clear' about sites it \
             never read.",
            sites.len()
        );

        // The test-only exclusion, asserted on the quantity it protects rather
        // than on the one an earlier draft counted. That draft asserted
        // `!test_only_module_files().is_empty()` against a live 230 — a number
        // that moves every time anyone anywhere adds a `#[cfg(test)] mod x;`, so
        // no floor could sit flush against it, and one that tried would be
        // measuring test-module churn rather than this exclusion. What the
        // exclusion is FOR is keeping fixture call sites out of the verdict, and
        // that is a property, not a count: it either removes some or it does not.
        //
        // ⚠️ Ordered AFTER the site floor deliberately. `suppressed` is counted
        // through the same opener reader as `sites`, so a break in that reader
        // zeroes both — and when it does, "you examined too few sites" is the
        // accurate diagnosis while "the exclusion stopped running" sends the
        // reader to the wrong file. Forced 2026-08-25: reverting the
        // if-with-no-`else` fix zeroes `suppressed` (all four fixture sites are
        // bare `if`s) and this assertion fired first, naming the wrong cause.
        assert!(
            suppressed > 0,
            "no call site was removed by the test-only-module exclusion. Three \
             causes, and the message cannot tell them apart: that exclusion \
             stopped running (this guard is now reading test fixtures as boot \
             code); the opener reader above stopped matching; or the four fixture \
             call sites were legitimately deleted, in which case re-measure and \
             lower this to the new count. Measured 2026-08-25: 4 removed, all in \
             plugins/handlers/tests.rs."
        );

        // The grouping key below is only as good as the spelling at each call
        // site: two spellings of one wrapper split into two groups and rule 2
        // stops seeing the pair (named under "What this cannot see"). That
        // blind spot is empty today, and this is what keeps it empty — a
        // sentence saying "every site is written fully qualified" is prose,
        // and prose does not go red.
        let unqualified: Vec<&str> = sites
            .iter()
            .filter(|s| !s.wrapper.contains("::"))
            .map(|s| s.at.as_str())
            .collect();
        assert!(
            unqualified.is_empty(),
            "these conditional install sites call their wrapper unqualified, so \
             the qualified-path grouping below cannot tell them from a \
             same-named wrapper in another module (`init_global` is five \
             functions). Write the call as `crate::…::wrapper(..)`, or widen \
             the grouping key:\n  {}",
            unqualified.join("\n  ")
        );

        let mut per_wrapper: std::collections::BTreeMap<&str, Vec<&CondSite>> =
            std::collections::BTreeMap::new();
        for s in &sites {
            per_wrapper.entry(&s.wrapper).or_default().push(s);
        }

        let mut silent: Vec<String> = Vec::new();
        let mut hazardous: Vec<String> = Vec::new();
        let mut exempt: Vec<&str> = Vec::new();
        for (w, group) in &per_wrapper {
            if group.len() >= 2 {
                exempt.push(w);
                for s in group {
                    if s.declines {
                        hazardous.push(format!("{} ({w})", s.at));
                    }
                }
            } else if !group[0].declines {
                silent.push(format!("{} ({w})", group[0].at));
            }
        }

        assert!(
            silent.is_empty(),
            "these conditional capability installs never say why they were \
             skipped — add an `else` (or a sibling match arm) that calls the \
             slot's `decline(because)`:\n  {}",
            silent.join("\n  ")
        );
        assert!(
            hazardous.is_empty(),
            "the wrappers {exempt:?} each have two or more conditional install \
             sites, so decline-then-install is reachable within one process and \
             the stamp would outlive the state it describes — drop the decline \
             at these sites and record the reason in a comment instead:\n  {}",
            hazardous.join("\n  ")
        );
    }

    /// The slots a span of code installs (or declines), whether through a
    /// wrapper call or a direct `STATIC.install(v)` on a slot the span's own
    /// file declares.
    ///
    /// Both spellings are needed: boot calls wrappers, while a setter that
    /// declines itself (`init_cron_trigger`, `set_global_result_budget_ceiling`)
    /// touches its static directly.
    fn slots_in(
        span: &str,
        wrappers: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
        local_statics: &[String],
        file: &str,
        method: &str,
    ) -> std::collections::BTreeSet<String> {
        let mut out = slots_touched(span, local_statics, method, file);
        for (name, slots) in wrappers {
            if span_calls(span, name) {
                out.extend(slots.iter().cloned());
            }
        }
        out
    }

    /// Does any production line of `span` call the free function `name`?
    fn span_calls(span: &str, name: &str) -> bool {
        span.lines()
            .any(|line| !is_fn_definition(line) && calls(line, name))
    }

    /// What a span installs, split by whether the scan can name the slot.
    ///
    /// `strict` is every slot reached through a wrapper name that belongs to
    /// exactly one slot, plus every direct `STATIC.install(v)` — both resolve to
    /// a single handle, so the gate rule can demand that handle by name.
    ///
    /// `ambiguous` is one entry per wrapper NAME that several modules share
    /// (`init_global` is five slots; `set_global` is two). The scan is
    /// name-keyed, so at such a call site it cannot tell which module was meant
    /// — and a guard that cannot tell must not assert which. Demanding all five
    /// produced three false offenders when this rule was first run against the
    /// real tree (the cron gate and both codex gates), every one of them a
    /// collision rather than a missing decline. Direction: under-see on
    /// ambiguous names only.
    struct GateInstalls {
        strict: std::collections::BTreeSet<String>,
        ambiguous: Vec<(String, std::collections::BTreeSet<String>)>,
    }

    fn gate_installs(
        span: &str,
        wrappers: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
        local_statics: &[String],
        file: &str,
        hop: &HopMap,
    ) -> GateInstalls {
        let mut g = GateInstalls {
            strict: slots_touched(span, local_statics, "install", file),
            ambiguous: Vec::new(),
        };
        for (name, slots) in wrappers {
            if !span_calls(span, name) {
                continue;
            }
            if slots.len() == 1 {
                g.strict.extend(slots.iter().cloned());
            } else {
                g.ambiguous.push((name.clone(), slots.clone()));
            }
        }
        for (name, slots) in hoppable(hop, file) {
            if span_calls(span, name) {
                g.strict.extend(slots.iter().cloned());
            }
        }
        g
    }

    /// The slots an else-chain declines, including one hop into a same-binary
    /// aggregator.
    ///
    /// Symmetric with [`gate_installs`], and load-bearing rather than tidy:
    /// `decline_orchestrator_slots` declares no slot and calls no `.decline(` —
    /// it calls four leaf wrappers. Adding the hop on the install side alone
    /// turns the unmutated tree red at the orchestrator gate.
    fn gate_declines(
        span: &str,
        wrappers: &std::collections::BTreeMap<String, std::collections::BTreeSet<String>>,
        local_statics: &[String],
        file: &str,
        hop: &HopMap,
    ) -> std::collections::BTreeSet<String> {
        let mut out = slots_in(span, wrappers, local_statics, file, "decline");
        for (name, slots) in hoppable(hop, file) {
            if span_calls(span, name) {
                out.extend(slots.iter().cloned());
            }
        }
        out
    }

    /// Every handle installed inside a gated block is declined in that block's
    /// `else`, slot by slot.
    ///
    /// This is the half [`no_conditional_capability_install_is_silent`] cannot
    /// reach. That guard is *site*-centric: it finds an install whose own
    /// nearest enclosing line is an `if`, and asks whether the word `decline`
    /// appears in the alternative. Two consequences, both measured on
    /// 2026-08-25 by mutating the real tree:
    ///
    /// * An install nested several blocks below the gate is not its own
    ///   conditional site, so it is never examined — the seven handles
    ///   `agent_init` installs inside `if let Some(provider_registry)` were held
    ///   up by ONE of them (`ensure_dream_daemon_with_orientation`, whose opener
    ///   does resolve to that gate).
    /// * `contains("decline")` is satisfied by any one decline, so deleting SIX
    ///   of those seven declines was **green**. Forced, not reasoned.
    ///
    /// Asking it per slot fixes both: the gate's `else` must decline every
    /// handle the gate's body installs.
    ///
    /// ⚠️ The scan of the then-arm **excludes the opener line**, and that is
    /// load-bearing rather than tidy. `if EXTENSION_MANAGER.install(manager) { Ok }
    /// else { Err(rejected) }` and `Config::set_effective_path`'s twin both
    /// install *in the condition*; their `else` means "a handle is already
    /// installed", which is the opposite of a decline. Including the opener made
    /// both of them offenders.
    ///
    /// ## What this cannot see
    ///
    /// * **Transitive installs beyond one hop, and any hop out of a binary.**
    ///   [`hoppable`] resolves a gate body's call into a function defined in the
    ///   same binary target, once. So `start/mod.rs`'s orchestrator gate IS
    ///   covered (its body calls `initialize_orchestrator`, boot-local, which
    ///   installs four handles) — but the seven residue handles installed from
    ///   `src/executor/` are not, because `BuiltinToolRegistry::with_config` is a
    ///   library function whose callers it does not know. That is the same
    ///   boundary this round drew when it refused to decline those seven from
    ///   `agent_init`: a gate is the last word only on what its own binary
    ///   wires. Direction: under-see, and deliberate.
    ///
    ///   ⚠️ One of the seven is looser than "no decline exists":
    ///   `loop-graph/cron-trigger` HAS an inline decline, in `init_cron_trigger`'s
    ///   own `else` (`loop_graph/service.rs`), and that `if`/`else` is one of the
    ///   gates below. It reads "never reached" on a provider-less boot because
    ///   its *caller* never runs, not because the slot lacks a decline.
    ///
    /// * **A decline that is itself conditional.** `span_calls` is lexical over
    ///   the else-chain's text, so a decline nested under a further `if` inside
    ///   the `else` still reads as present: both `if args.daemon { decline_… }`
    ///   and `if !args.daemon { decline_… }` are green. Pre-existing — it is a
    ///   property of the whole gate rule and applies to all sixteen gates, not
    ///   something the hop introduced — but it IS a removal shape for the class
    ///   the hop was built to close, so "the class is closed" carries this
    ///   qualifier. Requiring the call at the alternative's own indent costs
    ///   more than it buys.
    ///
    ///   ⚠️ Not hypothetical at the orchestrator gate specifically: the comment
    ///   immediately above that `else` says, verbatim, that the diagnostic there
    ///   must NOT be gated on `!args.daemon` because daemon is the production
    ///   path. The file already knows someone will be tempted to write exactly
    ///   that wrapper right there, and prose is the only thing in the way.
    ///
    /// * **Anything outside `alephcore`.** Every guard in this module walks
    ///   `CARGO_MANIFEST_DIR/src`, so `interfaces/tui`, `interfaces/cli` and
    ///   `interfaces/webchat` are outside all of them. Pre-existing and not this
    ///   round's, but named here because a reader who has just been told the
    ///   rule covers "binary targets" will otherwise assume it covers the other
    ///   binaries. It covers `src/bin/*` only.
    ///
    /// * **`match` gates.** This opens `if` / `if let` only. A `match` whose one
    ///   arm installs two handles and whose other declines one satisfies
    ///   [`no_conditional_capability_install_is_silent`]'s `contains("decline")`
    ///   and is invisible here. Today's three match gates (the cron `Err` arm,
    ///   the extension-manager `Err` arm, the tool-result-store `Err` arm) are
    ///   covered by that site guard — but by a property of today's call counts
    ///   (`init_global` has exactly one conditional site, so it misses the
    ///   two-site exempt bucket), not by either rule. A second conditional site
    ///   for any of those wrappers would move it into the exempt bucket and
    ///   leave the match arm unguarded by anything.
    #[test]
    fn every_slot_installed_inside_a_gate_is_declined_in_its_else() {
        let w = capability_wrappers();
        let test_only = test_only_module_files();
        let hop = one_hop(&w, &test_only);
        let mut gates = 0usize;
        let mut offenders: Vec<String> = Vec::new();

        for (rel, text) in rust_sources_under(&manifest_src()) {
            if rel.starts_with("src/capability/") || test_only.contains(&rel) {
                continue;
            }
            let (nums, lines) = prod_lines(&text);
            let statics = slot_statics(&lines);
            for i in 0..lines.len() {
                let t = lines[i].trim_start();
                if !(t.starts_with("if ") || t.starts_with("if let ")) {
                    continue;
                }
                let (_, close) = block_at(&lines, i);
                let Some(alt) = else_chain(&lines, close) else {
                    continue; // no `else`: the site-centric guard owns this shape
                };
                // `(i + 1).min(close)`: `block_at`'s fallback for an unterminated
                // `if` on a file's final production line returns `lines.len()-1`,
                // which can be < i + 1 and would panic the slice.
                let body = lines[(i + 1).min(close)..close.min(lines.len())].join("\n");
                let installed = gate_installs(&body, &w.installs, &statics, &rel, &hop.installs);
                if installed.strict.is_empty() && installed.ambiguous.is_empty() {
                    continue;
                }
                gates += 1;
                let declined = gate_declines(&alt, &w.declines, &statics, &rel, &hop.declines);
                let mut missing: Vec<String> =
                    installed.strict.difference(&declined).cloned().collect();
                missing.extend(
                    installed
                        .ambiguous
                        .iter()
                        .filter(|(_, slots)| slots.is_disjoint(&declined))
                        .map(|(name, slots)| format!("{name} (any of its {} slots)", slots.len())),
                );
                if !missing.is_empty() {
                    offenders.push(format!(
                        "{rel}:{} — else never declines {missing:?}",
                        nums[i]
                    ));
                }
            }
        }

        assert_eq!(
            gates, 17,
            "examined {gates} gated blocks that install a capability; 17 were \
             measured on 2026-09-04 (16 lexical + the orchestrator gate, which is \
             reached only through the one-hop rule). A count that moved without a capability \
             being added or removed means the block reader stopped matching — \
             which is how this guard would report 'all clear' about blocks it \
             never opened. Last moved 2026-09-04: 16 -> 17, the `[heartbeat] \
             enabled` gate at `start/mod.rs` that now installs \
             `heartbeat/service`."
        );
        assert!(
            offenders.is_empty(),
            "these gates install a capability and their `else` never declines \
             it, so on the configuration where the handle is absent the operator \
             is told nothing:\n  {}",
            offenders.join("\n  ")
        );
    }

    /// Every `decline_*` wrapper is called from production code outside its own
    /// file.
    ///
    /// The rule the two site-centric guards structurally cannot state. Eleven of
    /// this round's fifteen declines fire from places that are **unconditional
    /// at their own call site** — `decline_orchestrator_slots`'s four, because
    /// `initialize_orchestrator` installs them unconditionally while its CALLER
    /// is gated; and `agent_init`'s seven, which sit in an `else` no install
    /// site resolves to. Emptying `decline_orchestrator_slots`' body was
    /// **green** against the site-centric guard (forced 2026-08-25). It is red
    /// here, because those four wrappers then have no caller at all.
    ///
    /// This is also this repo's own R10 rule arriving at the same place from the
    /// other side: a wrapper with zero consumers is an abstraction to withdraw,
    /// not a capability that is quietly unstamped.
    ///
    /// ⚠️ "Outside its own file" is deliberate. `ensure_dream_daemon_with_orientation`
    /// (a decliner as well as an installer) is called by `ensure_dream_daemon`
    /// in the same file — and that wrapper has zero callers of its own, so an
    /// in-file caller would have satisfied this rule with a dead one. The cost
    /// is a false orphan for any decliner whose only legitimate caller is
    /// in-file; there is none today.
    ///
    /// What it does NOT check: that the call sits in the right arm — and the
    /// gap that opens is not hypothetical. `decline_orchestrator_slots` is a
    /// cross-file aggregator: it lives in `orchestrator_init.rs` and its body
    /// calls the four leaf wrappers, so **its body is their production caller
    /// whether or not anything ever calls the aggregator.** Deleting the gate's
    /// `else`-arm call to it — five lines, no `dead_code` warning, because the
    /// `Err` arm still calls it — left all three guards green when the
    /// re-review forced it on 2026-08-25, silently un-declining the round's one
    /// `FailsOpen` handle. That is why [`hoppable`] exists: the gate rule now
    /// resolves that call and demands the four slots by name. The two rules are
    /// complementary and neither subsumes the other.
    #[test]
    fn every_decline_wrapper_has_a_production_caller() {
        let w = capability_wrappers();
        assert!(
            w.declines.len() >= 24,
            "derived only {} decline wrappers; >=24 expected — the live count on \
             2026-08-25, so this floor sits flush against its own measurement. \
             A derivation that stopped matching makes this guard pass by finding \
             nothing.",
            w.declines.len()
        );
        let test_only = test_only_module_files();

        let mut called: std::collections::BTreeSet<String> = std::collections::BTreeSet::new();
        for (rel, text) in rust_sources_under(&manifest_src()) {
            if rel.starts_with("src/capability/") || test_only.contains(&rel) {
                continue;
            }
            let present: Vec<&String> = w
                .declines
                .keys()
                .filter(|n| text.contains(n.as_str()) && w.decline_file.get(*n) != Some(&rel))
                .collect();
            if present.is_empty() {
                continue;
            }
            let (_, lines) = prod_lines(&text);
            for line in &lines {
                if is_fn_definition(line) {
                    continue;
                }
                for name in &present {
                    if calls(line, name) {
                        called.insert((*name).clone());
                    }
                }
            }
        }

        let orphans: Vec<&String> = w.declines.keys().filter(|n| !called.contains(*n)).collect();
        assert!(
            orphans.is_empty(),
            "these `decline_*` wrappers have no production caller outside their \
             own file, so the handles they speak for read as a bare 'never \
             reached' — either wire them where the install is skipped, or delete \
             them (R10):\n  {orphans:?}"
        );
    }

    /// Whitespace removed around `.` and `::`, so a call split across lines
    /// reads the same as one written inline.
    ///
    /// `GLOBAL_POLICY\n        .update(policy)` is the shape this exists for:
    /// the round's own review had to hand-write a newline-tolerant scan to
    /// answer the same question about the same static.
    fn tighten(src: &str) -> String {
        let mut out = String::with_capacity(src.len());
        let mut gap = false;
        for c in src.chars() {
            if c.is_whitespace() {
                gap = true;
                continue;
            }
            if gap {
                if c != '.' && !out.ends_with('.') && !out.ends_with(':') {
                    out.push(' ');
                }
                gap = false;
            }
            out.push(c);
        }
        out
    }

    /// One slot declaration, as the variant census reads it.
    struct SlotDecl {
        /// The file that declares it.
        file: String,
        /// The `"<id>"` first argument to `::new`.
        id: String,
        /// The `MissingSemantics::<V>` second argument.
        variant: String,
    }

    /// Every `static _: [Mutable]CapabilitySlot<..> = ..::new("<id>",
    /// MissingSemantics::<V>..)` under `src/`, outside `src/capability/`.
    ///
    /// The window is the `static` item's own syntactic end — the first line
    /// whose trimmed text ends in `;` — not a line count. A fixed window is
    /// how a scan in this repo once read the NEXT declaration's text and
    /// stayed green after the line it was checking had been deleted.
    fn slot_declarations() -> Vec<SlotDecl> {
        let mut out = Vec::new();
        for (rel, text) in rust_sources_under(&manifest_src()) {
            if rel.starts_with("src/capability/") {
                continue;
            }
            let prod = production_prefix(&text);
            let lines: Vec<&str> = prod.lines().collect();
            for (i, line) in lines.iter().enumerate() {
                let t = strip_visibility(line.trim_start());
                let Some(rest) = t.strip_prefix("static ") else {
                    continue;
                };
                if !rest.contains("CapabilitySlot<") {
                    continue; // also covers MutableCapabilitySlot<
                }
                let mut item = String::new();
                for l in &lines[i..] {
                    item.push_str(l);
                    item.push('\n');
                    if l.trim_end().ends_with(';') {
                        break;
                    }
                }
                let id = item
                    .split_once('"')
                    .and_then(|(_, after)| after.split_once('"'))
                    .map(|(id, _)| id.to_string());
                let variant = item.split_once("MissingSemantics::").map(|(_, after)| {
                    after
                        .chars()
                        .take_while(|c| c.is_alphanumeric() || *c == '_')
                        .collect::<String>()
                });
                if let (Some(id), Some(variant)) = (id, variant) {
                    out.push(SlotDecl {
                        file: rel.clone(),
                        id,
                        variant,
                    });
                }
            }
        }
        out
    }

    /// Every slot names its own `MissingSemantics` variant inside its own
    /// module's test code.
    ///
    /// # Why this and not a guard on the distribution
    ///
    /// The obvious guard after a reclassification is a ratchet on
    /// `FailsClosed 19 / IndistinguishableDefault 13 / ConsumerDecides 9 /
    /// FailsOpen 4`. Three reasons it would be the wrong subject, and all
    /// three are this repo's own recorded lessons:
    ///
    /// - There is nothing to derive those four numbers *against*. Any
    ///   comparison target is a literal in the guard, so it is a third copy of
    ///   the same figure — 「派生不要列举」 inverted.
    /// - It cannot tell a reclassification from a NEW slot, and adding a slot
    ///   is a normal operation. Every new slot would trip it and the fix would
    ///   be "bump four numbers", which trains the next author to rubber-stamp
    ///   exactly the figure the guard exists to protect.
    /// - The numbers are a proxy. The hazard is "a slot's variant changed
    ///   without anyone re-reading its consumers", and the per-slot assertion
    ///   is what actually catches that: when `loop-graph/store` moved to
    ///   `FailsOpen`, its own module's test destructured the old variant and
    ///   panicked, so the change could not land without a human editing the
    ///   assertion. Only the aggregate was unguarded.
    ///
    /// So this completes the habit rather than inventing one: 33 of the 45
    /// slots already pinned their variant in their own module; this required
    /// the remaining 12, and requires it of every slot added later. It reads
    /// declarations, never a list; a new slot trips it only by arriving
    /// unpinned, which is the defect and not a false alarm.
    ///
    /// # Scope of "its own module"
    ///
    /// The declaring file's `cfg_test_portion`, plus any **test-only file**
    /// (`test_only_module_files`, the existing derivation) under the declaring
    /// file's module directory — `src/spend/mod.rs`'s pins live in
    /// `src/spend/tests.rs`, and that is the right place for them.
    ///
    /// # What it cannot see
    ///
    /// - A pin that names the id and the variant in the same file without
    ///   asserting them *together*. Text cannot see the `assert!`.
    /// - A variant moved WITH its assertion updated but without the consumers
    ///   re-read. Nothing text-level can see that; the assertion's doc is what
    ///   asks for it.
    /// - It does not keep FEATURE_LOCATOR's four numbers accurate. It makes
    ///   them re-derivable instead: the tally is in the failure messages
    ///   below.
    ///
    /// The tally lives here rather than in
    /// [`every_declared_slot_is_in_the_roster`], which the re-review suggested,
    /// for the reason this whole round is about: that guard does not parse the
    /// `MissingSemantics` argument, so putting the breakdown there would mint a
    /// SECOND parser for the same fact. One parser, and the guard that already
    /// owns it prints the tally.
    #[test]
    fn every_slot_pins_its_own_missing_semantics() {
        let decls = slot_declarations();
        let test_only = test_only_module_files();
        let sources = rust_sources_under(&manifest_src());

        let mut tally: std::collections::BTreeMap<&str, usize> = std::collections::BTreeMap::new();
        for d in &decls {
            *tally.entry(d.variant.as_str()).or_default() += 1;
        }
        let tally_line = || {
            let body = tally
                .iter()
                .map(|(v, n)| format!("{v} {n}"))
                .collect::<Vec<_>>()
                .join(" / ");
            format!("{body} = {}", decls.len())
        };

        // Self-count, against the roster guard's own 45. A parser that stopped
        // matching would pass this test by finding nothing to check.
        assert!(
            decls.len() >= 40,
            "parsed only {} slot declaration(s) — a variant census that read no \
             declarations is green and blind. Tally: {}",
            decls.len(),
            tally_line()
        );

        let module_dir = |file: &str| -> String {
            file.strip_suffix("/mod.rs")
                .or_else(|| file.strip_suffix(".rs"))
                .unwrap_or(file)
                .to_string()
        };

        let mut unpinned: Vec<String> = Vec::new();
        for d in &decls {
            let own = sources
                .iter()
                .find(|(rel, _)| *rel == d.file)
                .map(|(_, t)| cfg_test_portion(t))
                .unwrap_or_default();
            let dir = module_dir(&d.file);
            let mut scope = own;
            for (rel, text) in &sources {
                if test_only.contains(rel) && rel.starts_with(&format!("{dir}/")) {
                    scope.push('\n');
                    scope.push_str(text);
                }
            }
            let needle = format!("MissingSemantics::{}", d.variant);
            if !(scope.contains(&needle) && scope.contains(&format!("\"{}\"", d.id))) {
                unpinned.push(format!("{} ({}) in {}", d.id, d.variant, d.file));
            }
        }

        assert!(
            unpinned.is_empty(),
            "these slots do not name their own MissingSemantics variant in their own \
             module's tests, so a reclassification there lands with nothing red — and \
             the variant is the operator-facing severity, `FailsOpen` being the one \
             that exits `aleph doctor` non-zero:\n  {}\n\nCurrent tally: {}",
            unpinned.join("\n  "),
            tally_line()
        );
    }

    /// Does this `impl` line name exactly `ty` as its target?
    ///
    /// **This is what makes [`TYPES`]'s order irrelevant, and that is the
    /// point.** `"impl<T: 'static> MutableCapabilitySlot<T>"` CONTAINS the
    /// substring `"CapabilitySlot<"`, so the `contains` test this replaces
    /// required the array to be written longest-name-first — a requirement
    /// nothing expressed and only a reader could know. Written the other way
    /// round, both inherent impls collapsed onto `CapabilitySlot`: the guard
    /// named `CapabilitySlot::{update,load}`, which are innocent, AND stopped
    /// recording `MutableCapabilitySlot::*` at all, masking the real orphan.
    /// Loud, and misleading in the loud direction — a reader who "resolves"
    /// the two false accusations disarms the guard against the one instance it
    /// was written for.
    ///
    /// A comment would have described the hazard; the left boundary removes
    /// it. The right boundary is the `<` itself.
    fn names_type(line: &str, ty: &str) -> bool {
        line.match_indices(&format!("{ty}<")).any(|(at, _)| {
            line[..at]
                .chars()
                .next_back()
                .is_none_or(|c| !(c.is_alphanumeric() || c == '_'))
        })
    }

    /// Every `pub` method on the two slot types has a production call site.
    ///
    /// # The blind spot this closes
    ///
    /// [`every_decline_wrapper_has_a_production_caller`] is the round's guard
    /// for exactly this class, and it could not see
    /// `MutableCapabilitySlot::decline` — zero callers anywhere, production or
    /// test. Two correct rules left a gap between them: that guard's subject is
    /// the derived set of `decline_*` **wrapper functions** in slot modules,
    /// and its scan skips `src/capability/` outright so the census cannot audit
    /// itself. A `decline` **method on the slot type** is outside both.
    ///
    /// # Why this is receiver-typed and not name-based
    ///
    /// The obvious cheap rule — "does `.decline(` appear anywhere" — is green
    /// on precisely the instance that motivated it: `CapabilitySlot::decline`
    /// has 29 call sites, and a name-only scan cannot tell them from the
    /// `MutableCapabilitySlot` method of the same name with none. So call sites
    /// are resolved against the statics **declared with that type**, read out
    /// of the tree by the same `static NAME: …Slot<` shape the roster guards
    /// use, plus the `Type::method(` path form.
    ///
    /// # What it can and cannot see
    ///
    /// - *Sees*: `pub fn` / `pub const fn` in the INHERENT impls of
    ///   `CapabilitySlot` / `MutableCapabilitySlot`, and their calls anywhere
    ///   in the production half of `src/**` — including inside
    ///   `src/capability/` itself, because `SlotStatus`'s forwarding body is a
    ///   genuine dispatch path and not self-audit.
    /// - *Blind to* trait-impl methods (`SlotStatus::{id,missing,outcome}`).
    ///   They are not `pub fn`, and their consumer is `&dyn SlotStatus` in the
    ///   diagnostic — a different question, already answered by
    ///   `every_declared_slot_is_in_the_roster`.
    /// - *Blind to* a call through a `&'static` handle passed as a parameter
    ///   (`fn f(h: &'static MutableCapabilitySlot<T>) { h.update(..) }`). There
    ///   is no such indirection today — `spend` calls every method directly on
    ///   the static, and its doc records that the injectable
    ///   `update_policy_into(handle, ..)` seam was REMOVED. If one returns,
    ///   this guard over-reports rather than under-reports, which is the
    ///   direction a reader can act on.
    /// - *Blind to* a method that is called only from the `#[cfg(test)]` half.
    ///   That is deliberate: R10 asks for a **production** consumer, which is
    ///   also what [`every_decline_wrapper_has_a_production_caller`] asks.
    #[test]
    fn every_public_slot_method_has_a_production_caller() {
        const TYPES: [&str; 2] = ["MutableCapabilitySlot", "CapabilitySlot"];

        // Asserted, not documented: this array may be written in either order.
        // The version of this guard that used `contains` was silently
        // order-sensitive, and reordering it made the guard accuse two
        // innocent methods while hiding the real orphan.
        let mut reversed = TYPES;
        reversed.reverse();
        for (line, want) in [
            (
                "impl<T: 'static> MutableCapabilitySlot<T> {",
                "MutableCapabilitySlot",
            ),
            ("impl<T: 'static> CapabilitySlot<T> {", "CapabilitySlot"),
            (
                "impl<T: Send + Sync> SlotStatus for MutableCapabilitySlot<T> {",
                "MutableCapabilitySlot",
            ),
        ] {
            for order in [TYPES, reversed] {
                assert_eq!(
                    order.iter().find(|ty| names_type(line, ty)).copied(),
                    Some(want),
                    "`{line}` must resolve to {want} whatever order TYPES is written in"
                );
            }
        }
        // And the boundary really is a boundary, in both directions.
        assert!(!names_type(
            "impl<T> MutableCapabilitySlot<T> {",
            "CapabilitySlot"
        ));
        assert!(names_type("impl<T> CapabilitySlot<T> {", "CapabilitySlot"));

        let sources = rust_sources_under(&manifest_src());

        // Declarations, read out of the inherent impls.
        let mod_rs = sources
            .iter()
            .find(|(rel, _)| rel == "src/capability/mod.rs")
            .map(|(_, t)| code_text(&production_prefix(t)))
            .expect("src/capability/mod.rs must be readable");
        let mut declared: Vec<(&'static str, String)> = Vec::new();
        let mut current: Option<&'static str> = None;
        for line in mod_rs.lines() {
            if line.starts_with('}') {
                current = None;
            }
            let t = line.trim_start();
            if t.starts_with("impl") {
                // `impl<..> Trait for Type` is a trait impl: its methods are
                // reached through the trait object, not by name here.
                current = if t.contains(" for ") {
                    None
                } else {
                    TYPES.iter().find(|ty| names_type(t, ty)).copied()
                };
                continue;
            }
            let Some(ty) = current else { continue };
            let Some(rest) = t
                .strip_prefix("pub const fn ")
                .or_else(|| t.strip_prefix("pub fn "))
            else {
                continue;
            };
            let name = rest.split(['(', '<']).next().unwrap_or("").trim();
            if !name.is_empty() {
                declared.push((ty, name.to_string()));
            }
        }

        // Receivers: the statics actually declared with each type.
        let mut receivers: std::collections::BTreeMap<&'static str, Vec<String>> =
            std::collections::BTreeMap::new();
        for (_, text) in &sources {
            for line in production_prefix(text).lines() {
                let t = strip_visibility(line.trim_start());
                let Some(rest) = t.strip_prefix("static ") else {
                    continue;
                };
                let Some((name, after)) = rest.split_once(':') else {
                    continue;
                };
                let head = after.split_once('<').map_or(after, |(h, _)| h).trim();
                let head = head.rsplit("::").next().unwrap_or(head);
                if let Some(ty) = TYPES.iter().find(|ty| **ty == head) {
                    receivers
                        .entry(ty)
                        .or_default()
                        .push(name.trim().to_string());
                }
            }
        }

        let corpus: String = sources
            .iter()
            .map(|(_, t)| tighten(&code_text(&production_prefix(t))))
            .collect::<Vec<_>>()
            .join("\n");

        // Self-counts. MEASURED by raising these floors and reading the
        // failure message, not estimated: 10 public methods (5 per type),
        // 45 `CapabilitySlot` statics, 1 `MutableCapabilitySlot`.
        assert!(
            declared.len() >= 8,
            "parsed only {} public slot method(s) out of src/capability/mod.rs — a \
             guard that found no declarations passes by examining nothing",
            declared.len()
        );
        for ty in TYPES {
            assert!(
                receivers.get(ty).is_some_and(|r| !r.is_empty()),
                "no `static _: {ty}<..>` found; call sites for its methods cannot be \
                 resolved, so this guard would pass every one of them blindly"
            );
        }

        let mut orphans: Vec<String> = Vec::new();
        for (ty, method) in &declared {
            let by_path = corpus.contains(&format!("{ty}::{method}("));
            let by_receiver = receivers.get(ty).is_some_and(|rs| {
                rs.iter()
                    .any(|r| corpus.contains(&format!("{r}.{method}(")))
            });
            if !by_path && !by_receiver {
                orphans.push(format!("{ty}::{method}"));
            }
        }
        assert!(
            orphans.is_empty(),
            "these public slot methods have no production call site — R10 says \
             withdraw, not leave a hook for later. Delete them, or land them in \
             the same change as the caller that needs them:\n  {orphans:?}"
        );
    }

    /// The block/alternative readers do what the guard above claims, on input
    /// small enough to check by eye.
    ///
    /// Written because the guard's green cannot distinguish "every site
    /// declines" from "`governing_alternative` returns `None` everywhere" —
    /// the second reads as "no conditional installs found", and the
    /// `sites.len() >= 15` floor is the only other thing standing between that
    /// and a silent pass.
    #[test]
    fn the_alternative_reader_finds_each_shape_it_claims() {
        let lines = |s: &str| -> Vec<String> { s.lines().map(str::to_string).collect() };

        let if_else = lines(
            "fn boot() {\n    if let Some(x) = maybe {\n        install_it(x);\n    } else {\n        decline_it(\"why\");\n    }\n}",
        );
        let alt = governing_alternative(&if_else, 2).expect("if/else must be read");
        assert!(alt.contains("decline_it"), "got: {alt}");

        let if_only = lines("fn boot() {\n    if cond {\n        install_it(x);\n    }\n}");
        assert!(
            governing_alternative(&if_only, 2).as_deref() == Some(""),
            "an `if` with no `else` is GOVERNED with no alternative — folding it \
             into `None` is what made the first draft of this guard unable to \
             find a missing `else` at all"
        );

        let match_arms = lines(
            "fn boot() {\n    match build() {\n        Ok(v) => {\n            install_it(v);\n        }\n        Err(e) => {\n            decline_it(\"why\");\n        }\n    }\n}",
        );
        let alt = governing_alternative(&match_arms, 3).expect("match arm must be read");
        assert!(alt.contains("decline_it"), "got: {alt}");
        assert!(
            !alt.contains("install_it"),
            "an arm is not its own alternative; got: {alt}"
        );

        let unconditional = lines("fn boot() {\n    install_it(x);\n}");
        assert!(
            governing_alternative(&unconditional, 1).is_none(),
            "an unconditional install must not be examined"
        );

        // The name matcher's two rejections, which are what keep four unrelated
        // `install` functions out of the wrapper set's call-site scan.
        assert!(calls("    foo::install_it(x);", "install_it"));
        assert!(!calls("    handle_plugins_install_it(x);", "install_it"));
        assert!(!calls("    thing.install_it(x);", "install_it"));
    }
}
