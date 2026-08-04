//! Guard: no plain `get_untracked()` past an `.await` inside `spawn_local`.
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

fn src_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("src")
}

/// Every `.rs` file under `root`. Walking the tree (rather than `include_str!`)
/// is deliberate: a file added later must be covered without anyone remembering
/// this guard exists.
fn rust_sources(root: &Path) -> Vec<PathBuf> {
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

/// A `spawn_local` block, as 0-based line bounds `[start, end]`.
///
/// Only call sites that open a block on their own line (`spawn_local(async move
/// {`) are tracked. `spawn_local(some_future(..))` hands over a future built
/// elsewhere and has no body here to scan — treating it as a block would run the
/// brace counter to end-of-file and flag the entire remainder of the module.
fn spawn_local_blocks(src: &str) -> Vec<(usize, usize)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        if !line.contains("spawn_local(") || brace_delta(line) <= 0 {
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

/// `(line_number, text)` for every plain `get_untracked()` that a `spawn_local`
/// block reaches *after* its first `.await`. Line numbers are 1-based.
///
/// `try_get_untracked()` is the sanctioned form and is not reported — the
/// `try_` prefix is part of the matched token, so it cannot be confused with the
/// bare call.
fn late_untracked_reads(src: &str) -> Vec<(usize, String)> {
    let lines: Vec<&str> = src.lines().collect();
    let mut out = Vec::new();
    for (start, end) in spawn_local_blocks(src) {
        let mut seen_await = false;
        for (n, line) in lines.iter().enumerate().take(end + 1).skip(start) {
            if line.contains(".await") {
                seen_await = true;
            }
            if !seen_await {
                continue;
            }
            // `rfind`-free membership test: any `.get_untracked()` not preceded
            // by `try_`.
            for (idx, _) in line.match_indices(".get_untracked()") {
                if !line[..idx].ends_with("try") {
                    out.push((n + 1, (*line).trim().to_string()));
                    break;
                }
            }
        }
    }
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
                spawn_local_blocks(&src)
                    .into_iter()
                    .filter(|(s, e)| lines[*s..=*e].iter().any(|l| l.contains(".await")))
                    .count()
            })
            .sum::<usize>();
        assert!(
            awaiting > 20,
            "the scanner found only {awaiting} awaiting spawn_local blocks in the whole \
             crate — it is not parsing, so the rule below is not being enforced"
        );
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
    fn the_check_ignores_reads_outside_spawn_local() {
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
