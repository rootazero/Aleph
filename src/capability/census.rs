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
//! **At least nine confirmed instances — and read the counting rule below
//! before you use that number.** Five share one spelling; four do not, and the
//! class is defined by the *install*, never by the syntax:
//!
//! | static | shape | an uninstalled read means |
//! |---|---|---|
//! | `providers/moa/config_handle.rs::MOA_CONFIG` | `OnceLock<RwLock<Option<MoaToml>>>` | "no `[moa]` section configured" |
//! | `gateway/middleware/request_state.rs::STATE_REGISTRY` | `OnceLock<RwLock<Option<Arc<…>>>>` | `request.state` sees no registry |
//! | `gateway/middleware/latency.rs::GLOBAL_LATENCY` | `OnceLock<RwLock<Option<Arc<…>>>>` | `/metrics` renders no latency family |
//! | `gateway/event_emitter/origin_fanout.rs::CHANNEL_REGISTRY` | `OnceLock<RwLock<Option<Arc<…>>>>` | origin fan-out silently skipped |
//! | `gateway/event_emitter/team_fanout.rs::TEAM_EVENT_BUS` | `OnceLock<RwLock<Option<Arc<…>>>>` | team fan-out silently skipped |
//! | `security/audit.rs::GLOBAL_AUDIT` | `RwLock<Option<SecurityAuditLog>>` — **no `OnceLock`** | no security audit trail |
//! | `agents/background_persistence.rs::STORE_DIR` | `LazyLock<Mutex<Option<PathBuf>>>` | "persistence disabled … every entry point is a no-op" |
//! | `builtin_tools/process_journal.rs::STORE_DIR` | `LazyLock<Mutex<Option<PathBuf>>>` | same sentence, verbatim |
//! | `builtin_tools/scratchpad_registry.rs::STORE_PATH` | `Lazy<Mutex<Option<PathBuf>>>` | "keeps the registry in-memory-only" |
//!
//! The last three each state the round-7 ambiguity **in their own doc comment**
//! and are boot-installed by an `init_*` through a lock guard. They are *more*
//! invisible than the first six: `LazyLock` / `Lazy` are not in `CONTAINERS`,
//! so unlike the five `OnceLock` ones they are not even in the population of
//! candidates — the carve-out this section already makes for `GLOBAL_AUDIT`.
//!
//! ⚠️ COUNT THIS YOURSELF, **AND COUNT BY THE CLASS — NOT BY THE SPELLING.**
//! The class is *a handle whose contents are installed at boot through interior
//! mutability*; `OnceLock<Lock<Option<T>>>` is the shape five members happen to
//! share, and grepping for it is a proxy that under-counts by construction. The
//! previous revision of this section said "count from the spelling" and was
//! itself the fourth undercount in a row: one instance, then four, then six,
//! now nine — **three of those four revisions were written by someone who had
//! just been told the previous figure was too low.** So assume nine is too low
//! as well. It is a floor, not a count.
//!
//! Two reasons to believe that specifically. First, nothing here asserts nine:
//! the class is *below this rule's resolution* by construction, so no guard can
//! see a tenth arrive. Second, even the `Option` in the shapes above is a
//! proxy for "absent" — `process_journal.rs::RESERVED_THROUGH`
//! (`LazyLock<Mutex<u64>>`) says *"`0` = nothing reserved (also the value while
//! persistence is off)"*, the identical ambiguity carried by a sentinel integer
//! instead. Any scan keyed on `Option` misses every member that spells "absent"
//! some other way, and this table was built by such a scan.
//!
//! `STATE_REGISTRY` shows how little hiding this takes: `origin_fanout.rs` names
//! it in the doc comment on its own row above ("Mirrors the
//! `middleware::request_state` global-registry pattern"), and it still sat
//! outside the count for two revisions.
//!
//! `GLOBAL_AUDIT` is why this section is no longer titled after the
//! `OnceLock<Lock<Option<T>>>` spelling. It is not a container static, so it is
//! not even a *candidate* here — no widening of either install form could ever
//! reach it. The class is **interior-mutable installs**: two gateway fan-out
//! paths, two gateway middleware handles, the audit trail, three persistence
//! roots, and the `[moa]` config — none of which can currently be asked whether
//! boot reached them.
//!
//! `the_interior_mutable_install_class_is_below_this_rules_resolution` pins
//! `MOA_CONFIG`'s shape. All nine are excluded here, and all nine were excluded
//! from the specification's roster too — this class has been missing all along,
//! so nothing regressed; it was never seen.
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
    container: String,
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
                    container,
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
            let Some(declared) = after.trim().split('<').next() else {
                continue;
            };
            c.slots.push(HandleSite {
                file: rel.clone(),
                name: name.trim().to_string(),
                container: declared
                    .trim()
                    .rsplit("::")
                    .next()
                    .unwrap_or(declared)
                    .to_string(),
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

    /// Every container `static` in `src/`, selected or not — the population the
    /// count assertion partitions.
    fn population() -> usize {
        let c = take_census();
        c.written.len() + c.first_caller.len() + c.slots.len() + c.lazy.len()
    }

    /// The inventory this round migrates. Asserted, not printed: a census that
    /// silently shrinks and a census that stopped matching look identical.
    #[test]
    fn the_capability_handle_inventory_is_the_size_we_measured() {
        let c = take_census();
        let (written, first, slots, lazy) = (
            c.written.len(),
            c.first_caller.len(),
            c.slots.len(),
            c.lazy.len(),
        );

        // ONE write, not one per line. libtest prints its own progress lines to
        // the same stderr from another thread, and it spliced one of them into
        // the MIDDLE of a per-line `eprintln!` on the first run here -- which
        // silently dropped a handle from the grep-extracted inventory and cost
        // an hour chasing a census defect that did not exist. The inventory is a
        // cross-task interface; it must not be reassembled from interleaved
        // output.
        let mut report = format!(
            "--- capability handles: {} raw, {slots} slots ---\n",
            written + first
        );
        for s in c.written.iter() {
            report.push_str(&format!(
                "  RAW  {:14} {:32} {}\n",
                s.container, s.name, s.file
            ));
        }
        for s in c.first_caller.iter() {
            report.push_str(&format!(
                "  RAW  {:14} {:32} {}\n",
                s.container, s.name, s.file
            ));
        }
        for s in c.slots.iter() {
            report.push_str(&format!(
                "  SLOT {:14} {:32} {}\n",
                s.container, s.name, s.file
            ));
        }
        for s in &c.lazy {
            report.push_str(&format!(
                "  LAZY {:14} {:32} {}\n",
                s.container, s.name, s.file
            ));
        }
        report.push_str(&format!(
            "--- candidates {} = written {written} + first-caller-wins {first} + slots {slots} \
             + lazy caches {lazy} ---",
            written + first + slots + lazy
        ));
        eprintln!("{report}");

        // Self-count: the rule must DISCRIMINATE. A predicate stuck at "always
        // installed" would select everything and a predicate stuck at "never
        // installed" would select nothing; the count below catches both, but
        // only this one names which way the recogniser broke.
        assert!(
            lazy > 0,
            "the rule excluded no lazy caches at all — the install predicates \
             are answering the same thing for every static"
        );
        assert!(
            first > 0,
            "the first-caller-wins arm selected nothing. It is the arm that \
             cannot be reached by a setter search, so a silent regression here \
             looks exactly like a corpus with no such handles"
        );

        assert_eq!(
            written + first + slots,
            46,
            "the rule selected {} handles, not 46. 46 was measured on 2026-08-24 \
             over {} container statics, decomposed as written {written} + \
             first-caller-wins {first} + slots {slots}, with {lazy} lazy caches \
             excluded by derivation.\n\
             \n\
             ⚠️⚠️ THIS 46 IS NOT THE SPECIFICATION'S 46. The numbers agree; the \
             ROSTERS DO NOT. Read this before concluding the spec was right and \
             this census changed nothing — that conclusion is wrong, and the \
             coincidence is the third one around this number in one task.\n\
             \n\
             The decomposition is the tell: AS FIRST DERIVED on 2026-08-24, \
             BEFORE ANY MIGRATION, the spec's 46 was 46 WRITTEN handles and this \
             one was 45 written + 1 first-caller-wins. Migration moves members \
             from `written` into `slots`, so compare the SUM printed above, not \
             those two figures. Three members differ:\n\
             \n\
               OUT  extension/template.rs::FILE_REF_REGEX — a compiled-regex cache \
             in a ZERO-parameter fn whose own comment says the regex is a \
             compile-time constant and a parse failure is a programmer error. It \
             was on the spec's roster only because `get_or_try_init` sat in the \
             writer set, which is where a fallible initialiser had to go before \
             install form 2 existed.\n\
               IN   metrics/mod.rs::METRICS_RUNTIME — a real handle no setter \
             search saw, because rustfmt put its `.set(` on the next line.\n\
               SAME BUT FOR A DIFFERENT REASON  providers/route_handle.rs::GLOBAL \
             — on the spec's roster by a NAME COLLISION. Seven container statics \
             in src/ are called GLOBAL; six are `.set(` in their own files (nine \
             sites across six files), and a corpus-wide word-boundary search \
             cannot tell the seventh from them. It is now selected by derivation, \
             so a rename can no longer drop it silently.\n\
             \n\
             Arithmetic: the spec's 46, −1 FILE_REF_REGEX, +1 METRICS_RUNTIME = \
             46. GLOBAL does not move the count — it was already counted, wrongly. \
             Investigate before editing this number; the module doc carries the \
             full derivation.",
            written + first + slots,
            population()
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
    /// `metrics/mod.rs::METRICS_RUNTIME` is installed once from `Config::load`
    /// via `init_metrics_runtime(policy)` and read as
    /// `.get().copied().unwrap_or_default()` — an uninstalled read yields the
    /// compiled defaults and no caller can tell, which is the round-7
    /// indistinguishable-default shape verbatim. It was missing from the
    /// specification's roster for one reason only: `rustfmt` broke the line, so
    /// the source reads `if METRICS_RUNTIME\n        .set(…)` and a
    /// `contains("METRICS_RUNTIME.set(")` test says no.
    ///
    /// Roster membership must not be a function of line length.
    #[test]
    fn a_writer_split_across_lines_is_still_a_writer() {
        let c = take_census();
        assert!(
            c.written
                .iter()
                .any(|s| s.file.ends_with("src/metrics/mod.rs") && s.name == "METRICS_RUNTIME"),
            "METRICS_RUNTIME is not selected by the writer arm. Its `.set(` sits \
             on the line AFTER the receiver, so this fails the moment the writer \
             search goes back to matching `NAME.method(` as one contiguous \
             string — and it would take a real capability handle off the roster \
             with no other signal."
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
    /// `-> &'static dyn [some::path::]`, the offset of that `->`.
    ///
    /// Token-wise rather than substring-wise so it does not matter where
    /// rustfmt breaks the signature — the same lesson `method_call_open_paren`
    /// carries above: a matcher that only works on one line makes its verdict a
    /// function of line length.
    fn slot_status_return_at(text: &str, at: usize) -> Option<usize> {
        let mut head = text[..at]
            .trim_end_matches(|c: char| c.is_ascii_alphanumeric() || c == '_' || c == ':');
        for token in ["dyn", "'static", "&", "->"] {
            head = head.trim_end().strip_suffix(token)?;
        }
        Some(head.len())
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
                 return type is what this guard and the roster both read)",
                slot.name,
                slot.file,
                slot.name.to_lowercase(),
                slot.name
            );
        }
    }

    /// The `#[allow(dead_code)]` on every roster accessor is an exemption with
    /// an expiry, so this is the force that makes it shrink.
    ///
    /// ⚠️ **WHEN THE ROSTER LANDS THIS MUST BE 0 AND THIS ASSERTION MUST BE
    /// DELETED.** Deleting the attributes is part of wiring the roster, not a
    /// follow-up: an `#[allow(dead_code)]` left on an accessor whose consumer
    /// exists would later mask a genuinely unwired handle — the same defect
    /// `every_migrated_slot_has_a_roster_accessor` guards, arriving through the
    /// permit instead of through the omission.
    ///
    /// Deliberately NOT phrased as "if `ALL_SLOTS` exists": that name appears
    /// nowhere in `src/` and in no brief, so a roster called `SLOT_ROSTER` would
    /// leave such a guard green forever with ~46 permits shipped. This one is
    /// non-vacuous today and announces its own expiry — the day the attributes
    /// come off it goes red at a named line.
    #[test]
    fn every_roster_accessor_still_carries_the_expiring_allow() {
        let c = take_census();
        let accessors = roster_accessors();
        assert_eq!(
            accessors.len(),
            c.slots.len(),
            "{} roster accessors for {} slots. Either a slot has two, or one \
             accessor returns `&'static dyn SlotStatus` for something that is \
             not a migrated handle — and Task 11 builds the roster from exactly \
             these, so the roster would be wrong in the same direction.",
            accessors.len(),
            c.slots.len()
        );
        let bare: Vec<String> = accessors
            .iter()
            .filter(|a| !a.allows_dead_code)
            .map(|a| format!("{} ({})", a.name, a.file))
            .collect();
        assert!(
            bare.is_empty(),
            "these roster accessors have no `#[allow(dead_code)]`: {bare:?}\n\n\
             If the roster does NOT exist yet, this is a build error waiting to \
             happen: CI runs `cargo clippy -p alephcore -- -D warnings`, so an \
             unconsumed `pub(crate) fn` is an error, not a warning.\n\n\
             If the roster DOES now exist, this is the expiry firing as \
             designed: remove the attribute from EVERY accessor and delete this \
             test in the same commit. A half-removed set is the worst state — \
             the remaining permits would mask genuinely unwired handles."
        );
    }
}
