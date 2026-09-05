//! Guard: no plain unwrapping read past an `.await` inside `spawn_local`.
//!
//! Two of them today — `RwSignal::get_untracked` and `StoredValue::get_value`
//! ([`PANICKING_READS`]). They are one hazard, not two: both unwrap, both panic
//! on a disposed owner, and both have a `try_` sibling that answers `None`.
//!
//! `RwSignal::get_untracked` unwraps — reading a **disposed** signal panics, and
//! a panic in the panel takes the whole page to the recovery overlay. Every
//! `spawn_local` in this crate is a window in which the component that owns the
//! signals can be unmounted: an RPC in flight while the user navigates away, a
//! `TimeoutFuture` still parked when a drawer closes, `getUserMedia` waiting on
//! a permission prompt the user never answers. The await resolves into a dead
//! scope and the continuation reads a corpse.
//!
//! Nothing about that is loud until it fires. The signal write on the *other*
//! side of the same `if` is a silent no-op (Leptos tolerates writes to disposed
//! signals), the type checker is happy, and the component that crashes is
//! whichever one happened to be rendering. So the rule is enforced textually,
//! here, rather than left to whoever writes the next `spawn_local`.
//!
//! **The rule admits no exceptions**, including reads of root-owned signals such
//! as `ChatState`, which provably cannot be disposed while the app lives. Two
//! reasons: an allowlist is a second source of truth about which owner holds
//! which signal, and it rots the first time something is re-scoped; and
//! `try_get_untracked` on a live signal is behaviourally identical, so the
//! uniform rule costs a `.flatten()` and buys immunity to that re-scoping.
//!
//! Sibling guard: [`crate::platform`]'s `context_ownership`, which covers the
//! *other* way this crate hands out disposed signals (a surviving context entry
//! pointing at a dead scope).

use std::path::{Path, PathBuf};

/// Shared with sibling source-level guards (e.g. `api::chat`'s single-producer
/// pin) so "where is this crate's source" has one answer, not one per guard.
pub(crate) fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `root`. Walking the tree (rather than `include_str!`)
/// is deliberate: a file added later must be covered without anyone remembering
/// this guard exists.
pub(crate) fn rust_sources(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else if path.extension().is_some_and(|e| e == "rs")
                // This file's RED fixtures are, by construction, the exact shape
                // the rule forbids. Scanning them would make the guard report
                // itself and never go green.
                && path.file_name().is_some_and(|n| n != "disposed_reads.rs")
            {
                out.push(path);
            }
        }
    }
    out
}

/// Brace delta of a line, ignoring whole-line comments.
///
/// Only *whole-line* comments are stripped. A trailing `// …` is left in because
/// stripping it would also cut `https://…` inside a string literal, and a
/// wrongly-shortened line closes a block early — which makes this scanner stop
/// looking, the one failure mode that is silent. Over-reading is loud: the
/// tree-wide test below fails immediately.
fn brace_delta(line: &str) -> i32 {
    let code = if line.trim_start().starts_with("//") {
        ""
    } else {
        line
    };
    i32::try_from(code.matches('{').count()).unwrap_or(0)
        - i32::try_from(code.matches('}').count()).unwrap_or(0)
}

/// Anything that opens a body which can suspend, as 0-based line bounds
/// `[start, end]`.
///
/// Only call sites that open a block on their own line (`spawn_local(async move
/// {`) are tracked. `spawn_local(some_future(..))` hands over a future built
/// elsewhere and has no body here to scan — treating it as a block would run the
/// brace counter to end-of-file and flag the entire remainder of the module.
///
/// `async fn` / `async move {` / `async {` are matched alongside
/// `spawn_local(`. The scanner covered only `spawn_local` bodies until
/// 2026-08-09, which left the hazard's other half invisible: the read that
/// panics is any read past an `.await`, and a bare `async fn` reached from a
/// `spawn_local` elsewhere is the same continuation with the same dead scope —
/// it just does not say `spawn_local` on the line above.
/// `DashboardState::await_gateway_ready` was written straight into that blind
/// spot and the guard stayed green.
///
/// Widening cost nothing to adopt: the sweep that motivated it found **zero**
/// pre-existing violations outside `spawn_local` bodies, so this is a uniform
/// rule rather than a rule plus an allowlist — the second source of truth this
/// module's doc refuses to keep.
const SUSPENDING_BLOCK_OPENERS: [&str; 4] =
    ["spawn_local(", "async fn ", "async move {", "async {"];

fn awaiting_blocks(src: &str) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !SUSPENDING_BLOCK_OPENERS.iter().any(|o| line.contains(o)) || brace_delta(line) <= 0 {
            continue;
        }
        let mut depth = 0;
        for (j, inner) in lines.iter().enumerate().skip(i) {
            depth += brace_delta(inner);
            if depth <= 0 {
                out.push((i, j));
                break;
            }
        }
    }
    out
}

/// The unwrapping reads. Each has a `try_`-prefixed sibling that answers
/// `None` instead of panicking, which is what the rule asks for.
///
/// `.get_value()` joined `.get_untracked()` on 2026-09-04. The scanner matched
/// the signal accessor and nothing else, so `StoredValue::get_value` — which
/// unwraps and panics on a disposed owner exactly the same way — was
/// STRUCTURALLY invisible to a guard whose module doc says the rule admits no
/// exceptions (判据 §3: a guard's green covers only the shapes it recognises).
/// The final review of the terminal round found a fresh instance of it and the
/// guard had nothing to say.
const PANICKING_READS: [&str; 2] = [".get_untracked()", ".get_value()"];

/// `(line_number, text)` for every plain [`PANICKING_READS`] call that a
/// suspending block reaches *after* its first `.await`. Line numbers are
/// 1-based.
///
/// The `try_` form is the sanctioned one and is not reported — the prefix is
/// checked against the text immediately before the match, so it cannot be
/// confused with the bare call.
///
/// Findings are deduplicated by line: `spawn_local(async move {` matches two
/// openers at once, and one offending read must not be reported twice.
///
/// # What this shape cannot see (判据 §3 — name it, do not imply it is closed)
///
/// The scan is a LINE RANGE: a read is only examined if its own line sits
/// inside a block that suspends. A helper defined at component scope and
/// CALLED from a continuation is therefore invisible, however dead the owner is
/// by the time it runs — and that is the exact shape of the instance that
/// motivated widening this list (`views/terminal/mod.rs`'s `attach_to`, called
/// from two `spawn_local` continuations and from an event callback). Closing it
/// needs a call graph, which a textual scanner does not have. Until then, a
/// helper reachable from a continuation carries the rule in its own doc, the
/// way `publish_selection` and `canvas_ctx` in that file do.
fn late_untracked_reads(src: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out: Vec<(usize, String)> = Vec::new();
    for (start, end) in awaiting_blocks(src) {
        let mut seen_await = false;
        for (n, line) in lines.iter().enumerate().take(end + 1).skip(start) {
            if line.contains(".await") {
                seen_await = true;
            }
            if !seen_await {
                continue;
            }
            // `rfind`-free membership test: any panicking read not preceded by
            // `try_`.
            let offends = PANICKING_READS.iter().any(|token| {
                line.match_indices(token)
                    .any(|(idx, _)| !line[..idx].ends_with("try"))
            });
            if offends && !out.iter().any(|(seen, _)| *seen == n + 1) {
                out.push((n + 1, (*line).trim().to_string()));
            }
        }
    }
    out.sort_by_key(|(line, _)| *line);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Sanity: the scrape reaches real source and actually parses blocks.
    /// Without this the tree-wide assertion below passes vacuously the day the
    /// layout moves, the manifest dir changes, or `brace_delta` starts closing
    /// every block on its opening line.
    #[test]
    fn the_scan_reaches_the_source_tree() {
        let files = rust_sources(&src_dir());
        assert!(
            files.len() > 50,
            "found {} sources — the walk is broken, not the code",
            files.len()
        );
        let awaiting = files
            .iter()
            .filter_map(|p| std::fs::read_to_string(p).ok())
            .map(|src| {
                let lines: Vec<&str> = src.lines().collect();
                awaiting_blocks(&src)
                    .into_iter()
                    .filter(|(s, e)| lines[*s..=*e].iter().any(|l| l.contains(".await")))
                    .count()
            })
            .sum::<usize>();
        assert!(
            awaiting > 20,
            "the scanner found only {awaiting} awaiting blocks in the whole \
             crate — it is not parsing, so the rule below is not being enforced"
        );
    }

    /// The blind spot the openers were widened to cover: a plain `async fn`
    /// that reads a signal after awaiting. No `spawn_local` appears anywhere in
    /// it, so the pre-2026-08-09 scanner walked straight past this shape.
    #[test]
    fn the_check_rejects_a_bare_async_fn_continuation() {
        let before = r#"
            async fn await_gateway_ready(&self) -> Result<(), String> {
                loop {
                    TimeoutFuture::new(50).await;
                    if self.is_connected.get_untracked() {
                        return Ok(());
                    }
                }
            }
        "#;
        let found = late_untracked_reads(before);
        assert_eq!(
            found.len(),
            1,
            "expected exactly one finding, got {found:?}"
        );
        assert!(found[0].1.contains("is_connected"));
    }

    /// …and the sanctioned form in the same shape is silent, so the widening
    /// did not just make the guard shout at every `async fn`.
    #[test]
    fn a_bare_async_fn_using_the_sanctioned_form_is_accepted() {
        let after = r#"
            async fn await_gateway_ready(&self) -> Result<(), String> {
                loop {
                    TimeoutFuture::new(50).await;
                    if self.is_connected.try_get_untracked() == Some(true) {
                        return Ok(());
                    }
                }
            }
        "#;
        assert!(late_untracked_reads(after).is_empty());
    }

    /// RED proof for the token the scanner was blind to until 2026-09-04:
    /// `StoredValue::get_value`, which unwraps just like `get_untracked` and
    /// panics on a disposed owner just the same. Before the widening this
    /// fixture was GREEN — the guard reported nothing at all for it.
    #[test]
    fn the_check_rejects_a_stored_value_read_after_an_await() {
        let before = r#"
            spawn_local(async move {
                let resp = state.rpc_call("pty.list", params).await;
                if session_id.get_value().as_deref() == Some(&sid) {
                    render(resp);
                }
            });
        "#;
        let found = late_untracked_reads(before);
        assert_eq!(
            found.len(),
            1,
            "expected exactly one finding, got {found:?}"
        );
        assert!(found[0].1.contains("session_id"));
    }

    /// …and the sanctioned `try_get_value()` in the same position is silent,
    /// so the widening did not simply outlaw `StoredValue` after an await.
    #[test]
    fn the_check_accepts_try_get_value() {
        let after = r#"
            spawn_local(async move {
                let resp = state.rpc_call("pty.list", params).await;
                let Some(current) = session_id.try_get_value() else { return; };
                if current.as_deref() == Some(&sid) { render(resp); }
            });
        "#;
        assert!(late_untracked_reads(after).is_empty());
    }

    #[test]
    fn no_plain_untracked_read_survives_an_await() {
        let mut offenders = Vec::new();
        for path in rust_sources(&src_dir()) {
            let Ok(src) = std::fs::read_to_string(&path) else {
                continue;
            };
            for (line, text) in late_untracked_reads(&src) {
                offenders.push(format!("{}:{line}: {text}", path.display()));
            }
        }
        assert!(
            offenders.is_empty(),
            "plain `get_untracked()` after an `.await` — the owning component may \
             already be disposed there, and that read panics the whole panel. Use \
             `try_get_untracked()` and skip the work (`.flatten()` keeps a \
             comparison byte-identical).\n{}",
            offenders.join("\n")
        );
    }

    /// RED proof, shape 1: an RPC continuation. This is `sessions.delete`
    /// as it stood before this guard existed.
    #[test]
    fn the_check_rejects_an_rpc_continuation() {
        let before = r#"
            spawn_local(async move {
                match dash.rpc_call("sessions.delete", params).await {
                    Ok(_) => {
                        if chat.session_key.get_untracked().as_deref() == Some(&key) {
                            chat.clear_session();
                        }
                    }
                    Err(e) => log(e),
                }
            });
        "#;
        let found = late_untracked_reads(before);
        assert_eq!(
            found.len(),
            1,
            "expected exactly one finding, got {found:?}"
        );
        assert!(found[0].1.contains("session_key"));
    }

    /// RED proof, shape 2: a timer continuation, nested two blocks deep. This
    /// is the rename-on-blur handler, the worst of the set — blur is very often
    /// the last thing that happens before the row is unmounted.
    #[test]
    fn the_check_rejects_a_nested_timer_continuation() {
        let before = r#"
            on:blur=move |_| {
                spawn_local(async move {
                    TimeoutFuture::new(100).await;
                    if editing_key.get_untracked().as_deref() == Some(&key) {
                        let text = edit_text.get_untracked();
                        do_rename(key, text);
                    }
                });
            }
        "#;
        assert_eq!(late_untracked_reads(before).len(), 2);
    }

    /// …and does not fire on the sanctioned form.
    #[test]
    fn the_check_accepts_try_get_untracked() {
        let after = r#"
            spawn_local(async move {
                TimeoutFuture::new(100).await;
                let Some(cur) = editing_key.try_get_untracked() else { return; };
                if cur.as_deref() == Some(&key) { rename(key); }
            });
        "#;
        assert!(late_untracked_reads(after).is_empty());
    }

    /// A read taken *before* the await is fine: the scope is provably alive,
    /// because nothing has yielded yet. Flagging these would push callers into
    /// noise and teach them to ignore the guard.
    #[test]
    fn the_check_ignores_a_read_before_the_await() {
        let ok = r#"
            spawn_local(async move {
                let key = chat.session_key.get_untracked();
                dash.rpc_call("sessions.delete", key).await;
            });
        "#;
        assert!(late_untracked_reads(ok).is_empty());
    }

    /// Reads outside `spawn_local` are ordinary synchronous code and are not
    /// this guard's business.
    #[test]
    fn the_check_ignores_reads_in_a_body_that_cannot_suspend() {
        let ok = r#"
            let go_up = move |_| {
                let Some(parent) = listing.get_untracked().and_then(|l| l.parent) else { return; };
                current_path.set(Some(parent));
            };
        "#;
        assert!(late_untracked_reads(ok).is_empty());
    }

    /// `spawn_local(future_built_elsewhere(..))` opens no block here. Counting
    /// it as one ran the brace scanner to end-of-file and reported every later
    /// `get_untracked` in the module — 8 false positives in `directory_browser`
    /// alone while this scanner was being calibrated.
    #[test]
    fn the_check_ignores_a_spawn_local_with_no_body() {
        let ok = r#"
            spawn_local(hydrate_session_history(dash, key));

            let go_up = move |_| {
                dash.rpc_call("x", p).await;
                let n = listing.get_untracked();
            };
        "#;
        assert!(
            late_untracked_reads(ok).is_empty(),
            "a bodyless spawn_local swallowed the rest of the file"
        );
    }
}

/// Every `window_event_listener` either cleans itself up or says why it never
/// has to.
///
/// `leptos_dom::helpers::window_event_listener` registers **no** cleanup: the
/// returned handle has to be `.remove()`d by hand, and dropping it on the floor
/// leaves the closure attached to `window` forever. In a component that can
/// unmount, that is not a leak — it is a crash. The orphaned closure keeps
/// reading signals its owner has disposed, and the next matching key event
/// panics the whole app into the recovery overlay.
///
/// That happened: `components/artifacts/preview.rs` attached an Escape handler
/// with no handle. The right rail unmounts whenever the layout mode changes or
/// the viewport crosses the phone breakpoint, so — reproduced on a real machine
/// 2026-08-18 — loading the Panel wide, narrowing the window past 640px, and
/// pressing Escape *once* took the Panel down every time. `preset_picker.rs`
/// had already written the hazard down in a doc comment; prose does not fail a
/// build.
///
/// A call site satisfies this rule one of two ways:
///
/// * bind the handle (`let h = window_event_listener(..)`) and `h.remove()`
///   somewhere in the same file — in practice inside `on_cleanup`; or
/// * carry a `// window-listener-permanent:` note within the six lines above
///   it, stating why that listener outlives every owner (app-root installs).
///
/// The annotation is deliberately **local** rather than a central allowlist.
/// A list in this file would be a permission slip nothing shrinks; a comment at
/// the call site is read by whoever touches the code, and a new site cannot
/// inherit someone else's justification without writing its own.
#[cfg(test)]
mod window_listener_tests {
    use super::{rust_sources, src_dir};

    #[test]
    fn every_window_listener_is_removed_or_declared_permanent() {
        let files = rust_sources(&src_dir());
        assert!(
            files.len() > 50,
            "found {} sources — the walk is broken, not the code",
            files.len(),
        );

        let mut offenders = Vec::new();
        let mut checked = 0usize;
        for path in &files {
            let Ok(raw) = std::fs::read_to_string(path) else {
                continue;
            };
            // CRLF checkouts: anchor nothing to a bare `\n`. `production_lines`
            // normalises `\r` itself; this copy exists because the annotation
            // look-back below indexes the RAW file and its line numbers have to
            // line up with the ones `production_lines` reports.
            let src = raw.replace('\r', "");
            let lines: Vec<&str> = src.split('\n').collect();

            // `i18n_census::production_lines` is this crate's one answer to
            // "where does production code end". It walks `#[cfg(test)]` ITEMS
            // and drops whole-line comments; this scan used to cut at the first
            // `#[cfg(test)]` marker instead, which could only ever UNDER-scan —
            // a gated `use`, helper `fn` or `mod` anywhere above the trailing
            // test module truncated the file there, and every
            // `window_event_listener(` below it went unseen and was reported as
            // a clean pass. The census's own doc records that same cut hiding
            // 2 266 lines when it was found on the other guard.
            let production = crate::i18n_census::production_lines(&src);
            let prod_text: String = production
                .iter()
                .map(|(_, line)| line.as_str())
                .collect::<Vec<_>>()
                .join("\n");

            for (lineno, line) in &production {
                if !line.contains("window_event_listener(") {
                    continue;
                }
                checked += 1;

                // The look-back reads RAW lines on purpose: the annotation is a
                // `//` comment, and `production_lines` has already removed it.
                let i = lineno - 1;
                let permanent = lines[i.saturating_sub(6)..i]
                    .iter()
                    .any(|l| l.contains("window-listener-permanent:"));
                if permanent {
                    continue;
                }

                // `let <name> = window_event_listener(` … and `<name>.remove()`
                // later in the same file.
                let bound = line
                    .split_once("let ")
                    .and_then(|(_, rest)| rest.split_once('='))
                    .map(|(name, _)| name.trim().to_string())
                    .filter(|n| {
                        !n.is_empty() && n.chars().all(|c| c.is_alphanumeric() || c == '_')
                    });

                let removed = bound
                    .as_ref()
                    .is_some_and(|n| prod_text.contains(&format!("{n}.remove()")));

                if !removed {
                    let rel = path.strip_prefix(src_dir()).unwrap_or(path);
                    offenders.push(format!("{}:{}", rel.display(), lineno));
                }
            }
        }

        assert!(
            checked >= 8,
            "only {checked} window_event_listener call sites seen — the scan stopped early",
        );
        assert!(
            offenders.is_empty(),
            "window_event_listener with no cleanup and no `// window-listener-permanent:` note \
             — an orphaned closure reads disposed signals and panics the app:\n  {}",
            offenders.join("\n  "),
        );
    }
}
