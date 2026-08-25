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
//! `a_writer_split_across_lines_is_still_a_writer` fails by name if it regresses.
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

use crate::utils::source_scan::{production_prefix, rust_sources_under, strip_comment_lines};

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
    /// `Outcome` shape or a changed call site. See this module's doc ("Why
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
    /// `route_handle::GLOBAL`, + 45 slots), and only the sum is invariant.
    #[test]
    fn every_installed_global_is_a_capability_slot() {
        let sites = capability_handles();
        let offenders: Vec<String> = sites
            .iter()
            .filter(|s| !s.is_slot)
            .filter(|s| {
                !(s.file.ends_with("src/providers/route_handle.rs") && s.name == "GLOBAL")
            })
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
            46,
            "capability handle total drifted: {raw} raw + {slots} slots = {}, not \
             46. Never assert either side alone: raw shrinks and slots grows as \
             migration proceeds, so only the SUM is stable. A drift here means \
             either a census recogniser regressed (see the module doc's \
             recogniser blind spots) or a handle genuinely left the corpus — \
             investigate before editing this number.",
            raw + slots
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
    /// The discriminating experiment, for whoever repeats it: split the
    /// signature **and** delete that accessor's `#[allow(dead_code)]`. If this
    /// function sees the accessor,
    /// `every_roster_accessor_still_carries_the_expiring_allow` fires and NAMES
    /// it; if it were blind, that guard would pass and
    /// `every_migrated_slot_has_a_roster_accessor` would name the static
    /// instead. One green proves nothing on its own.
    ///
    /// ⚠️ **`parse_static_decl` and the slot arm of `take_census` ARE
    /// line-based** — both read `static NAME: Container<` from a single line.
    /// That is safe, and the reason is worth writing down rather than
    /// rediscovering: rustfmt's over-width wrap for a `static` breaks *inside
    /// the generic list*, so line one keeps `static NAME: Container<` intact —
    /// verified for both a raw handle and a slot, census unchanged at 46. Any
    /// other wrap is not a fixed point of rustfmt, so `cargo fmt --check`
    /// rejects it. And if one ever does slip through, it fails LOUDLY: the
    /// handle leaves the census and
    /// `the_capability_handle_inventory_is_the_size_we_measured` reds on the
    /// total. Do not "fix" those two by making them span lines without first
    /// producing a wrap that actually breaks them.
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
             `the_capability_handle_inventory_is_the_size_we_measured` in the \
             same run — it will be red too, and its message is the accurate \
             one.\n\
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
}
