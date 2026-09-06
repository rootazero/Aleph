//! What Aleph records about a browser process it launched, and how a later
//! boot reclaims one it left behind. Engine-agnostic on purpose.
//!
//! Lifted out of `chromium_launch.rs` (HEAD `0a3e8a48a`) with one behaviour
//! change: a record now says WHICH engine wrote it, and the sweep matches the
//! argv switch that engine actually uses. Everything else — the three-state
//! argv probe, the four reap outcomes, the `.corrupt` quarantine, the atomic
//! write — moved unchanged, and its reasoning moved with it: those doc
//! comments are the record of four review findings and are not decoration.

use std::path::{Path, PathBuf};
use std::process::Child;
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::Engine;
use crate::browser::error::BrowserError;
use crate::browser::profile::BrowserType;

/// Extension every sidecar record is written with.
const SIDECAR_EXT: &str = "json";

/// How often a kill re-asks [`try_reap`] whether the child has settled.
///
/// `pub(crate)` because the engine-specific launchers own the poll loop (a
/// `wait()` that blocks would park a tokio worker whenever the kill was
/// refused) while the interval itself must stay one number for both engines.
pub(crate) const KILL_POLL_INTERVAL: Duration = Duration::from_millis(20);

/// The one directory every sidecar lives in — **`"chromium"`, and frozen**.
///
/// The leaf reads oddly once obscura writes its first record here, and it is
/// kept anyway because the alternative is worse: moving the directory makes
/// every record a pre-upgrade Aleph left behind invisible to the sweep, which
/// is precisely the browser this module exists to reclaim. The name costs a
/// reader one confused minute; the rename costs an orphaned browser per
/// profile, permanently, on every machine that upgrades.
///
/// A constant rather than an inline literal so that a rename is a deliberate
/// act that reddens `the_sidecar_registry_leaf_is_frozen` by name. **If it is
/// ever renamed, the same commit must migrate the old leaf's `*.json`** —
/// there is no later commit that can, because after the rename nothing knows
/// the old records exist.
const SIDECAR_REGISTRY_LEAF: &str = "chromium";

/// A live CDP endpoint on loopback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CdpEndpoint {
    /// `http://127.0.0.1:<port>` — the form `playwright-cli attach --cdp` takes
    /// (both this and the ws form were accepted in the spike; the http one is
    /// shorter and does not embed a browser id that changes per launch).
    pub http_url: String,
    /// `ws://127.0.0.1:<port>/devtools/browser/<id>` — what a raw CDP client
    /// connects to.
    pub ws_url: String,
    /// The browser process we launched.
    pub pid: u32,
}

/// What Aleph records about a browser it launched.
///
/// Chrome does not remove `DevToolsActivePort` on exit, so that file cannot
/// answer "is this browser mine and still running" — this sidecar is the
/// record that can.
///
/// These live in ONE registry directory, not beside each browser's profile.
/// A profile may configure `user_data_dir` to anywhere on disk (the repo's own
/// QA does), so a per-udd record puts itself outside anything a boot sweep can
/// walk — and the sweep would then miss exactly the case the fixture
/// exercises. One directory means "which browsers are there to reclaim" has a
/// single derivation (判据 §12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EngineSidecar {
    /// Which engine this record is about.
    ///
    /// `default` rather than required, because every record written before
    /// this field existed was a Chromium — and a required field would make
    /// those unparseable, routing them to the `.corrupt` arm below and
    /// leaving a live browser behind each one unreapable forever.
    #[serde(default = "Engine::chromium_default")]
    pub engine: Engine,
    pub pid: u32,
    /// `None` until the endpoint is known. The record is written immediately
    /// after the process is spawned — before the endpoint is known — so a
    /// cancelled or crashed launch still leaves a reapable record (round-1
    /// review finding F2); [`reap_orphans`] never reads this field, it decides
    /// on `pid` + `data_dir` + `engine` alone. `None` means "I do not know
    /// yet" and must never be rendered as a usable endpoint.
    pub http_url: Option<String>,
    /// The per-profile directory that process was launched with. Recorded
    /// rather than implied by the file's location, because the file is not
    /// stored there — and this is the value the orphan sweep matches against
    /// argv, under [`Engine::data_dir_flag`].
    ///
    /// The wire key stays `user_data_dir`: renaming it makes every
    /// pre-upgrade record unparseable, which is the same permanent-orphan
    /// loss the `engine` default above exists to prevent.
    #[serde(rename = "user_data_dir")]
    pub data_dir: PathBuf,
    /// The build that launched it. Not used as a gate — recorded because an
    /// orphan from a different version is exactly the case a reader will want
    /// named when this goes wrong. Wire key frozen, same reason as above.
    #[serde(rename = "aleph_version")]
    pub build: String,
}

/// The one directory every sidecar lives in.
pub fn sidecar_registry_dir() -> Result<PathBuf, BrowserError> {
    crate::browser::playwright_launch::browser_state_dir(SIDECAR_REGISTRY_LEAF)
}

/// This profile's record. Sanitized through the same helper the launch config
/// and the derived data dir use, so a profile name can never escape the
/// registry.
pub fn sidecar_path(session_key: &str) -> Result<PathBuf, BrowserError> {
    Ok(sidecar_registry_dir()?.join(format!(
        "{}.{SIDECAR_EXT}",
        crate::browser::playwright_launch::sanitize_session_key(session_key)
    )))
}

/// What a process's argv turned out to be — three states, not two.
///
/// **Chosen over `Option<Vec<String>>` + a separate `present` closure.** Both
/// shapes carry the same information; this one makes the sweep's `match`
/// exhaustive, so a fourth outcome added later cannot be silently folded into
/// an existing arm, and it removes the ordering hazard of two closures that
/// must agree about one pid. `Option` alone cannot carry it at all: a reader
/// that answers `None` for both "no such process" and "I could not read its
/// command line" makes the sweep spend an unknown as a certainty, and the
/// action on the other side is SIGKILL plus an irreversible record deletion
/// (判据 §8).
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ArgvProbe {
    /// The pid is not in the process table — or it is a **zombie**, i.e. it has
    /// already exited and the kernel is holding the entry until its parent
    /// reaps it. Either way there is nothing left to kill, so the record goes.
    Absent,
    /// The pid is there and its command line could not be read. Routine on
    /// Windows. We have learned nothing.
    Unreadable,
    /// The process's argv, one element per word as the kernel reports it.
    Argv(Vec<String>),
}

/// Whether `argv` names `dir` as the browser's profile directory.
///
/// **Token equality over the argv vector — never a substring scan over a
/// joined command line.** Both halves matter, because the action this
/// predicate authorises is a kill:
///
/// * **Prefix collision, no bleed required.** The sweep walks profiles that sit
///   under one root, so the flags it builds are prefixes of one another:
///   `--user-data-dir=<root>/default` is a substring of a live browser's
///   `--user-data-dir=<root>/default-2`. `sanitize_session_key` produces
///   prefix-related names routinely (`work` / `work-archive`). A substring test
///   therefore kills the neighbouring profile's browser — the exact case the
///   argv check was added to prevent, failing on its most likely neighbour.
/// * **The macOS argv/env bleed, already measured and pinned in this repo.**
///   `crates/agent-detect/src/engine.rs:427-431` records it verbatim: a process
///   that rewrites its title (every Node CLI does) leaves `sysinfo::cmd()`
///   reading past the argv region into the environment, and `:938-957` pins a
///   real reading in which an exported variable whose value contained spaces
///   scattered the bare words `prefer`, `modern`, `like` into the command line.
///   That module's defence is to tokenize and skip `VAR=value` words rather
///   than scan a joined string; 判据 §16 says the twin's answer gets carried
///   over rather than rediscovered.
///
/// `flag` is the switch the RECORD's own engine uses
/// ([`Engine::data_dir_flag`]) — Chromium's `--user-data-dir`, obscura's
/// `--storage-dir`. Passing it in rather than deriving it here keeps one
/// answer to "which switch names this process's directory": the record.
/// Both spellings are matched: `<flag>=<path>` and the two-token
/// `<flag> <path>`. Missing the second would let a browser launched that way
/// become unreapable — and obscura's own argv (spec §6.2) uses exactly that
/// form.
///
/// **Nothing here splits, either.** An implementation that joined the argv and
/// split it on whitespace would lose every `user_data_dir` containing a space —
/// `~/Library/Application Support/…` is an ordinary place for an operator to
/// point a profile — and it would fail silently: the token never matches, so
/// that browser is never recognised as ours and its orphan is never reaped, on
/// every boot, forever. The kernel already split the argv; comparing whole
/// elements inherits that and adds nothing of its own.
#[must_use]
pub(crate) fn argv_names_dir(argv: &[String], flag: &str, dir: &Path) -> bool {
    let joined = format!("{flag}={}", dir.display());
    let value = dir.to_string_lossy();
    argv.iter().enumerate().any(|(i, word)| {
        word == &joined || (word == flag && argv.get(i + 1).is_some_and(|v| v.as_str() == value))
    })
}

/// Write (or overwrite) one profile's sidecar record.
///
/// Called twice per successful launch: once immediately after the process is
/// spawned (`http_url: None` — pid and `user_data_dir` are already known, the
/// endpoint is not yet), and again once the port file parses (`http_url:
/// Some(..)`). The reaper never reads `http_url` — it decides on `pid` +
/// `user_data_dir` alone — so the early record is fully reapable (round-1
/// review finding F2).
///
/// Atomic: writes to a `.tmp` sibling in the same directory, then renames
/// over the target. `tokio::fs::write` alone is not atomic, and a crash
/// mid-write would leave a truncated file that `reap_orphans` would then have
/// to treat as unparseable — exactly the torn-write case round-1 review
/// finding F5 named. `rename` within one directory is atomic on every
/// platform this runs on.
pub(crate) async fn write_sidecar_record(
    engine: Engine,
    session_key: &str,
    pid: u32,
    data_dir: &Path,
    http_url: Option<String>,
) {
    let path = match sidecar_path(session_key) {
        Ok(p) => p,
        Err(e) => {
            tracing::warn!(error = %e, "cannot resolve the engine sidecar path");
            return;
        }
    };
    if let Some(dir) = path.parent() {
        if let Err(e) = tokio::fs::create_dir_all(dir).await {
            tracing::warn!(error = %e, "cannot create the engine sidecar registry");
            return;
        }
    }
    let body = match serde_json::to_string(&EngineSidecar {
        engine,
        pid,
        http_url,
        data_dir: data_dir.to_path_buf(),
        build: env!("ALEPH_VERSION").to_string(),
    }) {
        Ok(b) => b,
        Err(e) => {
            tracing::warn!(error = %e, "cannot serialize the chromium sidecar");
            return;
        }
    };
    let tmp_path = {
        let mut s = path.clone().into_os_string();
        s.push(".tmp");
        PathBuf::from(s)
    };
    // Best-effort here, but NOT unobserved: a missing sidecar costs an
    // orphan across a crash. The QA `attach` stage asserts the file exists
    // and that its pid matches a live process (Task 9 step 6), because no
    // unit test can see this.
    if let Err(e) = tokio::fs::write(&tmp_path, body).await {
        tracing::warn!(
            error = %e,
            path = %tmp_path.display(),
            "cannot write the chromium sidecar temp file"
        );
        return;
    }
    if let Err(e) = tokio::fs::rename(&tmp_path, &path).await {
        tracing::warn!(
            error = %e,
            path = %path.display(),
            "cannot rename the chromium sidecar into place"
        );
    }
}

/// What one [`reap_orphans`] pass did. Not a bare `usize`: before this (Final
/// Review M6), a `.corrupt` sidecar was renamed aside and then never looked
/// at again by anything — the sweep's own extension filter skipped it on
/// every future boot, so a live orphaned Chromium behind one stayed
/// unreapable and utterly unmentioned, forever. The rename was always
/// correct (deleting on an unparseable record risks the same live-browser
/// loss `Unreadable` protects against); the silence was the defect. A bare
/// count cannot say "N reaped" and "M corrupt, still unresolved" at once, so
/// this carries both, the same way [`ArgvProbe`] carries three states an
/// `Option` could not.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub(crate) struct ReapOutcome {
    /// Orphaned chromium processes killed (or already gone) this sweep,
    /// their sidecar removed.
    pub reaped: usize,
    /// `.corrupt` sidecars seen this sweep whose session key has no fresh,
    /// parseable sidecar written since — genuinely unresolved. If a live
    /// orphaned browser sits behind one, it stays unreapable, and this
    /// number is the thing that says so instead of silence.
    pub corrupt_pending: usize,
    /// `.corrupt` sidecars removed this sweep because a fresh sidecar has
    /// since been written under the SAME session key — the same
    /// determinate-supersession rule a recycled pid or an absent process is
    /// dropped under (not a time-based guess): something else has already
    /// moved on for that key, so whatever this record once meant is stale.
    pub corrupt_superseded: usize,
}

/// Kill Chromium processes left behind by a previous Aleph.
///
/// `registry` is [`sidecar_registry_dir`] — one directory holding one record
/// per profile, whatever each profile's `user_data_dir` happens to be. That is
/// why the sweep can be a single walk (判据 §12: the set has one derivation).
///
/// Four outcomes per PARSEABLE record, and they are deliberately NOT collapsed:
///
/// * [`ArgvProbe::Argv`] naming our directory → it is ours: kill it, drop the
///   record;
/// * [`ArgvProbe::Argv`] naming something else → the pid was recycled and now
///   belongs to somebody else's program: kill nothing, drop the record (this
///   answer is determinate);
/// * [`ArgvProbe::Absent`] → the process is gone: nothing to kill, the record
///   is stale, drop it;
/// * [`ArgvProbe::Unreadable`] → we have learned **nothing**. Kill nothing, and
///   **keep the record**. Deleting it here is irreversible: the browser stays
///   alive and the only thing that could ever find it again is gone (判据 §8
///   crossed with §15). Routine on Windows, where `sysinfo` often cannot read
///   another process's command line — i.e. the platform spec §3.6 already flags
///   as unexercised is exactly the one where the wrong answer would be permanent.
///
/// A fifth, for records that never parsed at all: see [`ReapOutcome`].
///
/// Both effects are injected so the decision is testable without a browser;
/// [`reap_orphans_now`] is the production wiring.
///
/// `kill` answers **`true`** when the process is gone or was killed, `false`
/// when it is still alive and refused to die. The sidecar is deleted and the
/// return count incremented ONLY on `true` — a refused kill must not be
/// spent as a reap, because the record is the only way that browser can ever
/// be found again (判据 §4: a guard must assert the effect landed, not that
/// the call happened; round-1 review finding F1).
pub(crate) fn reap_orphans(
    registry: &Path,
    argv_of: &dyn Fn(u32) -> ArgvProbe,
    kill: &dyn Fn(u32) -> bool,
) -> ReapOutcome {
    let Ok(entries) = std::fs::read_dir(registry) else {
        // The dir not existing is the normal first-boot state, not a failure.
        return ReapOutcome::default();
    };
    let mut outcome = ReapOutcome::default();
    for entry in entries.flatten() {
        let path = entry.path();
        let ext = path.extension().and_then(|e| e.to_str());
        if ext == Some("corrupt") {
            // Superseded once a fresh sidecar exists for the SAME key —
            // existence alone is the signal, the same way a recycled pid's
            // argv is a determinate answer without needing to inspect it
            // further. Not parseability: a second torn write under the same
            // key is a different, pre-existing hazard this sweep does not
            // also need to solve to close M6.
            let name = path.to_string_lossy();
            let Some(original) = name.strip_suffix(".corrupt") else {
                continue; // `ext == Some("corrupt")` already guarantees this
            };
            if Path::new(original).exists() {
                let _ = std::fs::remove_file(&path);
                outcome.corrupt_superseded += 1;
            } else {
                outcome.corrupt_pending += 1;
            }
            continue;
        }
        if ext != Some(SIDECAR_EXT) {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rec) = serde_json::from_str::<EngineSidecar>(&body) else {
            // A record we cannot parse might name a browser that is still
            // running: a torn write from a crash mid-write looks exactly like
            // this, and deleting it forecloses every future boot's sweep from
            // ever finding that process again (round-1 review finding F5).
            // Rename it aside instead — an accumulating `.corrupt` file is
            // cheap; an unfindable live browser is not. The `.corrupt` arm
            // above is what keeps looking at it on every later sweep.
            tracing::warn!(path = %path.display(), "unparseable chromium sidecar; renaming aside");
            let corrupt = {
                let mut s = path.clone().into_os_string();
                s.push(".corrupt");
                PathBuf::from(s)
            };
            let _ = std::fs::rename(&path, &corrupt);
            outcome.corrupt_pending += 1;
            continue;
        };
        match argv_of(rec.pid) {
            ArgvProbe::Argv(argv)
                if argv_names_dir(&argv, rec.engine.data_dir_flag(), &rec.data_dir) =>
            {
                tracing::info!(
                    pid = rec.pid,
                    engine = rec.engine.as_str(),
                    dir = %rec.data_dir.display(),
                    "reaping orphaned browser"
                );
                if kill(rec.pid) {
                    outcome.reaped += 1;
                    let _ = std::fs::remove_file(&path);
                } else {
                    // The kill was refused (still alive, EPERM or similar):
                    // deleting the record now would make this browser
                    // unfindable forever. Keep it so a later sweep can retry.
                    tracing::warn!(
                        pid = rec.pid,
                        "chromium kill was refused; keeping the sidecar so it can be retried"
                    );
                }
            }
            // A pid that resolved to somebody ELSE's argv is provably not ours:
            // determinate, so the record goes and the process is left alone.
            ArgvProbe::Argv(_) => {
                let _ = std::fs::remove_file(&path);
            }
            ArgvProbe::Absent => {
                let _ = std::fs::remove_file(&path);
            }
            // Present, argv unreadable: keep it. See the doc above.
            ArgvProbe::Unreadable => tracing::warn!(
                pid = rec.pid,
                "chromium sidecar kept: the process exists but its argv is unreadable"
            ),
        }
    }
    outcome
}

/// The real process-table reader: **the argv vector, not a joined line**.
///
/// `Process::cmd()` is `&[OsString]` — one element per word as the kernel
/// recorded it. Nothing here joins, and nothing here splits: a joined string
/// can only be matched with `str::contains`, and a split one loses any
/// `user_data_dir` containing a space (`~/Library/Application Support/…` is a
/// perfectly ordinary place for an operator to point a profile, and the token
/// would then never match, so that orphan would never be reaped — forever).
///
/// `UpdateKind::Always` is not optional: `UpdateKind` defaults to `Never`
/// (sysinfo 0.39.3 `src/common/system.rs:2319-2327`), so a refresh kind that
/// does not name it leaves `cmd()` empty and **every** probe would answer
/// `Unreadable`.
///
/// The three states come from two questions this call answers separately: the
/// process lookup returns `None` only when the pid is **not in the process
/// table**, and a `Some` whose `cmd()` is empty means the process is there and
/// its command line **could not be read** — routine on Windows. An
/// `Option<Vec<String>>` would have to pick one of those two to represent, and
/// collapsing them is the defect this enum exists to prevent.
///
/// Goes through `utils::process_alive::with_process_specifics` rather than
/// building a `System` here. That helper takes the `ProcessRefreshKind` as an
/// argument and refreshes **only this pid** (`ProcessesToUpdate::Some(&[pid])`,
/// `process_alive.rs:130-131`); its own doc claims sole ownership of the idiom
/// and says a second `System::new()` + refresh copy is 判据 §1's "same fact
/// written twice", drifting on exactly the axis that matters — which fields get
/// refreshed. A hand-rolled `System::new_with_specifics(...)` would be that
/// second copy and would walk every process on the machine per call.
///
/// Its `Option` is the `Absent` boundary: `None` means the pid is not in the
/// process table at all.
fn argv_probe(pid: u32) -> ArgvProbe {
    use sysinfo::{ProcessRefreshKind, UpdateKind};
    let argv = crate::utils::process_alive::with_process_specifics(
        pid,
        ProcessRefreshKind::nothing().with_cmd(UpdateKind::Always),
        |p| ProcessFacts {
            argv: p
                .cmd()
                .iter()
                .map(|s| s.to_string_lossy().into_owned())
                .collect(),
            is_zombie: p.status() == sysinfo::ProcessStatus::Zombie,
        },
    );
    match argv {
        None => ArgvProbe::Absent,
        // A zombie has already exited; the entry is a corpse the kernel keeps
        // until its parent reaps it, and its `cmd()` is typically empty. Left
        // as `Unreadable` it would keep its sidecar on every boot forever, for
        // a process that can never be killed again. `Absent` is the honest
        // answer: there is nothing here to stop.
        //
        // Free to ask for: `Process::status()` (sysinfo 0.39.3
        // `src/common/system.rs:1869`) sits behind no refresh flag — the
        // `impl_get_set!` list at `:2515-2533` covers memory / cwd / cmd / exe
        // / tasks / user and there is no `with_status` anywhere in the crate —
        // so it is already populated on the process this call refreshed.
        Some(v) if v.is_zombie => ArgvProbe::Absent,
        Some(v) if v.argv.is_empty() => ArgvProbe::Unreadable,
        Some(v) => ArgvProbe::Argv(v.argv),
    }
}

/// What one refresh yields about a process. A struct rather than a tuple so the
/// `match` above reads as the three answers it is deciding between.
struct ProcessFacts {
    argv: Vec<String>,
    is_zombie: bool,
}

/// Kill a pid through the process table, answering **`true` when the process
/// is gone or was killed** and `false` when it is still alive and refused.
///
/// Extracted from `reap_orphans_now`'s closure so the process-table kill has
/// ONE author. It is deliberately **not** what
/// [`super::chromium::ChromiumLauncher::kill`] uses: a launcher holds the
/// `Child` and can `wait()` it, which is a stronger answer than a
/// process-table scan and immune to pid recycling. Two paths, two questions.
///
/// `pub(crate)` rather than private only because Task 16's obscura sweep wiring
/// is the second caller; if that task does not consume it, drop it to `fn`.
pub(crate) fn kill_by_pid(pid: u32) -> bool {
    let killed = crate::utils::process_alive::with_process_specifics(
        pid,
        sysinfo::ProcessRefreshKind::nothing(),
        sysinfo::Process::kill,
    );
    match killed {
        // `None`: the pid is no longer in the process table at all — already
        // gone, which counts the same as a successful kill.
        Some(true) | None => true,
        Some(false) => {
            tracing::warn!(pid, "orphaned browser did not accept the kill");
            false
        }
    }
}

/// [`reap_orphans`] wired to the real process table.
///
/// Its only caller is `ProfileManager::sweep_orphaned_engines`, which is
/// `#[cfg(not(test))]` — sealed there because that boot hook has a unit-test
/// caller and its task is detached, so under test it would resolve the
/// developer's real `$ALEPH_HOME` and kill their own Aleph's Chromium. So in a
/// test-cfg build this function structurally cannot have a caller.
///
/// `cfg_attr(test, ...)` rather than a bare `#[allow(dead_code)]` on purpose:
/// the bare form would also swallow the case that matters — the PRODUCTION
/// caller disappearing — and that is precisely the wire
/// `manager.rs`'s `the_boot_hook_still_calls_the_orphan_sweep` exists to pin.
/// This form exempts only the cfg that cannot answer, and leaves
/// `cargo clippy -p alephcore --lib -- -D warnings` fully honest.
#[cfg_attr(test, allow(dead_code))]
pub(crate) fn reap_orphans_now() -> ReapOutcome {
    let registry = match sidecar_registry_dir() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "cannot sweep orphaned chromium processes");
            return ReapOutcome::default();
        }
    };
    reap_orphans(&registry, &argv_probe, &kill_by_pid)
}

/// One non-blocking attempt to reap a child we launched.
///
/// `true` means the process has exited **and been waited on**, which are two
/// facts and only the pair is safe to report. A child that exited without a
/// `wait()` is a zombie: it holds a process-table entry until this daemon
/// exits, `kill -0` still succeeds on it, and every liveness guard that asks
/// the OS therefore answers "alive" about a process that is already dead. So
/// the effect this predicate asserts is the reap, not the signal (判据 §4).
///
/// A successful `try_wait` IS the reap — `std::process::Child::try_wait`
/// consumes the exit status when it returns `Ok(Some(_))` — so the slot is
/// emptied at the same moment, and that `None` is what tells a later poll the
/// question is already settled.
///
/// `Err` from `try_wait` answers **false**: "I could not tell" is not "it
/// died", and the caller's next move on false is to keep polling, which costs
/// nothing and is the fail-closed direction (判据 §8).
///
/// Takes the `Arc` rather than a guard so the caller cannot hold a `std`
/// mutex across an `await` — the poll loop in `ChromiumLauncher::kill`
/// sleeps between calls.
pub(crate) fn try_reap(
    child: &std::sync::Arc<crate::sync_primitives::Mutex<Option<Child>>>,
) -> bool {
    let mut guard = child.lock().unwrap_or_else(|e| e.into_inner());
    let Some(handle) = guard.as_mut() else {
        // Already reaped by an earlier poll. Determinate, not an unknown.
        return true;
    };
    match handle.try_wait() {
        Ok(Some(_status)) => {
            *guard = None;
            true
        }
        // Still running, or we could not ask. Neither is "gone".
        Ok(None) | Err(_) => false,
    }
}

/// Everything a launch needs, in terms neither engine owns.
///
/// `profile` and `session_key` carry the same string today (`engine_handle`
/// passes the profile name for both, exactly as `ChromiumChild::spawn`'s doc
/// at `chromium_launch.rs:311-315` describes). They are kept apart because
/// they answer different questions: `session_key` is what NAMES the record in
/// the sidecar registry — the only thing that can find this process after a
/// crash — and `profile` is the configuration identity an error message
/// should quote. Collapsing them would make a future per-profile multi-session
/// scheme silently share one record.
///
/// `Clone` because Task 9's `EngineRegistry` keeps a request by reference while
/// [`EngineProcess::launch`] takes one by value. Cloning is safe by
/// construction: every field is owned data describing a launch, not a handle to
/// one — the handle lives in [`Launched`], which is deliberately NOT `Clone`.
///
/// ⚠️ `Debug` is derived (matching `ChromiumLaunchSpec`,
/// `chromium_launch.rs:50`), and `proxy` may carry inline credentials
/// (`socks5://user:pass@host`). **Never log a whole request** — `?req` in a
/// `tracing` call puts that password in the log file. The launch already strips
/// secret env from the child (`chromium_launch.rs:344-348`); a proxy URL is a
/// separate vector that stripping does not reach.
#[derive(Clone, Debug)]
pub struct LaunchRequest {
    pub profile: String,
    pub session_key: String,
    pub data_dir: PathBuf,
    pub headless: bool,
    pub proxy: Option<String>,
    /// Chromium only: **which member of the Chromium family** to resolve on
    /// disk — the profile's own `browser` field
    /// (`super::super::profile::BrowserProfile`). obscura ignores it, the same
    /// way Chromium ignores `stealth` below; the two engines each carry the
    /// other's inapplicable knobs rather than the request splitting in two.
    ///
    /// **Carried rather than defaulted at the point of use (R68).** Resolving
    /// `BrowserType::default()` inside the launcher would make this field
    /// unreachable on the Cdp path: a profile configured for Brave or Edge
    /// would silently get Chromium, with nothing anywhere reporting that the
    /// operator's setting had been ignored — a no-op that reports success
    /// (判据 §11), in a user-facing setting. The request is the only thing that
    /// knows which profile it belongs to, so it is the only thing that can
    /// answer this.
    pub browser: BrowserType,
    /// obscura only (`--allow-private-network`, spec §6.2): passed **only**
    /// when this profile's network policy already permits private ranges.
    /// Chromium has no such switch; Aleph's own SSRF guard is the gate there.
    pub allow_private_network: bool,
    /// obscura only. Chromium ignores it — and says so in a log line rather
    /// than dropping it silently, because a launch that quietly discards a
    /// requested mode is a no-op reporting success (判据 §11).
    pub stealth: bool,
    pub extra_args: Vec<String>,
}

/// What a successful launch produced.
///
/// **Deliberately not `Clone` and deliberately not serialisable.** The child
/// handle has exactly one owner, and the thing that gets written to disk is
/// [`EngineSidecar`], not this. Adding a serde derive later fails to compile on
/// the `child` field, which is the right kind of failure — a `#[serde(skip)]`
/// there would silently persist a launch record with no way to reap it.
pub struct Launched {
    pub pid: u32,
    pub endpoint: CdpEndpoint,
    /// The record this launch wrote. Returned rather than re-derived by the
    /// caller so "where is this process's record" has one answer.
    pub sidecar_path: PathBuf,
    /// The live handle to the process, so it can be **waited on** after it is
    /// killed.
    ///
    /// A pid alone cannot do that: `std::process::Child` is the only thing
    /// that can `wait()`, and a child killed without a wait stays a zombie for
    /// the daemon's whole life (see [`try_reap`]). It travels with the launch
    /// rather than in a side map keyed by pid, which would reintroduce the pid
    /// recycling window the sweep's argv check exists to close.
    ///
    /// `Option` because a successful reap empties it, and `Arc<Mutex<..>>`
    /// because Task 9's `EngineHandle` is shared and its shutdown may race a
    /// caller's own kill; the second one through finds `None` and answers
    /// `true` without signalling anything.
    pub child: std::sync::Arc<crate::sync_primitives::Mutex<Option<Child>>>,
}

/// Launch and kill, per engine.
///
/// `kill` takes the **[`Launched`]**, not a bare pid, because the handle
/// inside it is the only thing that can `wait()` the process afterwards — and
/// a child that is killed and never waited on stays a zombie until the daemon
/// exits.
///
/// This does not leave the crash-recovery case out. A pid recovered from the
/// sidecar registry after a restart is killed by [`reap_orphans`] →
/// [`kill_by_pid`], which never goes through this trait; and such a process
/// was orphaned by a *dead* parent, so init/launchd reaps it and there is no
/// zombie for anyone to wait on. Two paths, each complete, neither able to
/// answer the other's question.
#[async_trait::async_trait]
pub trait EngineProcess: Send + Sync {
    fn engine(&self) -> Engine;
    async fn launch(&self, req: LaunchRequest) -> Result<Launched, BrowserError>;
    /// Kill the process and **confirm it was reaped** within `grace`;
    /// `Ok(false)` means it is still there.
    ///
    /// `grace` bounds the REAP, and there is no SIGTERM phase before it.
    /// `std::process::Child::kill()` is SIGKILL and std offers nothing softer;
    /// more to the point, `manager.rs:677-687` states the rule for this exact
    /// path — *"SIGKILL plus a bounded reap per child, and never a graceful
    /// handshake. No SIGTERM-then-wait, no CDP `Browser.close`"* — because one
    /// of its two callers is the wedged-daemon watchdog whose
    /// `std::process::exit(0)` waits for nobody, and a browser that wedged the
    /// shutdown is the last process to negotiate with.
    async fn kill(&self, launched: &Launched, grace: Duration) -> Result<bool, BrowserError>;
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fixture: a registry directory holding one sidecar per profile.
    fn registry_with(entries: &[(&str, u32, &str)]) -> tempfile::TempDir {
        registry_of(Engine::Chromium, entries)
    }

    /// The same fixture, for a chosen engine — the sweep now reads the
    /// engine off each record, so the tests that are ABOUT that must be able
    /// to write one.
    fn registry_of(engine: Engine, entries: &[(&str, u32, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (profile, pid, data_dir) in entries {
            std::fs::write(
                dir.path().join(format!("{profile}.json")),
                serde_json::to_string(&EngineSidecar {
                    engine,
                    pid: *pid,
                    http_url: Some("http://127.0.0.1:1".into()),
                    data_dir: PathBuf::from(data_dir),
                    build: env!("ALEPH_VERSION").to_string(),
                })
                .expect("serialize"),
            )
            .expect("write");
        }
        dir
    }

    #[test]
    fn the_sidecar_round_trips_and_records_the_dir_it_is_not_stored_in() {
        let json = serde_json::to_string(&EngineSidecar {
            engine: Engine::Chromium,
            pid: 4242,
            http_url: Some("http://127.0.0.1:58363".into()),
            data_dir: PathBuf::from("/tmp/explicit-udd"),
            build: env!("ALEPH_VERSION").to_string(),
        })
        .expect("serialize");
        let back: EngineSidecar = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.pid, 4242);
        assert_eq!(back.http_url, Some("http://127.0.0.1:58363".to_string()));
        // The whole point of the registry: the record lives in ONE directory
        // and names the data dir, instead of living IN it where a profile
        // that configures its own path puts it outside anything a sweep
        // walks.
        assert_eq!(back.data_dir, PathBuf::from("/tmp/explicit-udd"));
        assert_eq!(back.build, env!("ALEPH_VERSION"));
        assert_eq!(back.engine, Engine::Chromium);
    }

    /// F2's whole point: the record written right after spawn, before the
    /// port file has parsed, has no endpoint yet. `None` must round-trip as
    /// `None` — never coerced into an empty string a later reader might treat
    /// as a connectable (but empty) URL.
    #[test]
    fn a_sidecar_with_no_endpoint_yet_round_trips_as_none() {
        let json = serde_json::to_string(&EngineSidecar {
            engine: Engine::Chromium,
            pid: 4242,
            http_url: None,
            data_dir: PathBuf::from("/tmp/explicit-udd"),
            build: env!("ALEPH_VERSION").to_string(),
        })
        .expect("serialize");
        let back: EngineSidecar = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.http_url, None);
    }

    /// The containment property the registry inherits from `config_path_for`:
    /// one component under the state dir, whatever the profile is called.
    ///
    /// Needs its own `$ALEPH_HOME` (like every test below that resolves the
    /// registry path) — otherwise it races `kill_only`'s sidecar test for
    /// the ambient env var: `dir` and `p` are two separate resolutions of
    /// the same env var, and without holding the lock across both, another
    /// thread's guard can flip `$ALEPH_HOME` in between them, making `dir`
    /// and `p`'s parent name two different homes instead of the same one.
    #[test]
    fn a_sidecar_path_is_one_component_under_the_registry() {
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        let dir = sidecar_registry_dir().expect("home resolves");
        for hostile in [
            "default",
            "../../etc/passwd",
            "/etc/passwd",
            "..",
            "",
            "a/b",
        ] {
            let p = sidecar_path(hostile).expect("home resolves");
            assert_eq!(p.parent(), Some(dir.as_path()), "escaped with {hostile:?}");
            assert_eq!(
                p.components().count(),
                dir.components().count() + 1,
                "not a single component for {hostile:?}"
            );
        }
    }

    /// Convenience: the argv vector a real process table yields.
    fn argv(words: &[&str]) -> Vec<String> {
        words.iter().map(|w| (*w).to_string()).collect()
    }

    /// The match is **token equality over the argv vector**, never a substring
    /// scan over a joined line, and both halves of that sentence are load-bearing
    /// because the action this predicate authorises is SIGKILL.
    ///
    /// The junk in these vectors is not invented. `crates/agent-detect/src/engine.rs:938-957`
    /// pins a VERBATIM reading from this machine in which an exported variable
    /// whose value contains spaces scattered the bare words `prefer`, `modern`
    /// and `like` into `sysinfo::cmd()` — macOS lets a process that rewrites
    /// its title (every Node CLI does) leak past the argv region into the
    /// environment (`:427-431`). That module's defence is tokenize-and-skip
    /// assignments; this one is the same shape, and 判据 §16 says the twin's
    /// answer gets carried over rather than rediscovered.
    #[test]
    fn the_udd_match_is_token_equality_over_argv() {
        let dir = Path::new("/tmp/udd/default");

        // (1) The real flag, with a macOS env bleed sitting beside it.
        assert!(argv_names_dir(
            &argv(&[
                "/x/chrome",
                "--user-data-dir=/tmp/udd/default",
                "--headless=new",
                "about:blank",
                "ZSH_AI_PROMPT_EXTEND=Always",
                "prefer",
                "modern",
                "CLI",
                "tools",
                "like",
                "ripgrep,",
                "fd,",
                "and",
                "bat.",
            ]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));

        // Chrome accepts the two-token form too, so a browser someone launched
        // that way is still ours.
        assert!(argv_names_dir(
            &argv(&["/x/chrome", "--user-data-dir", "/tmp/udd/default"]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));

        // (2) THE SIBLING PREFIX. `reap_orphans` walks profiles under one root,
        // so the flags it builds are prefixes of one another — and
        // `sanitize_session_key` produces prefix-related names routinely
        // (`work` / `work-archive`). A substring test kills the neighbour's
        // live browser, which is precisely the case the argv check exists to
        // prevent, failing on its most likely neighbour.
        assert!(!argv_names_dir(
            &argv(&["/x/chrome", "--user-data-dir=/tmp/udd/default-2"]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));
        // …and the two-token form of the same trap.
        assert!(!argv_names_dir(
            &argv(&["/x/chrome", "--user-data-dir", "/tmp/udd/default-2"]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));

        // (3) The whole flag string appearing INSIDE a bled-in env value.
        assert!(!argv_names_dir(
            &argv(&[
                "/usr/bin/vim",
                "notes.txt",
                "LAST_CMD=chrome --user-data-dir=/tmp/udd/default",
            ]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));

        // The path as some other flag's value; a recycled pid; nothing at all.
        assert!(!argv_names_dir(
            &argv(&["/x/chrome", "--crash-dumps-dir=/tmp/udd/default"]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));
        assert!(!argv_names_dir(
            &argv(&["/usr/bin/vim", "/tmp/udd/default/notes.txt"]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));
        assert!(!argv_names_dir(&[], Engine::Chromium.data_dir_flag(), dir));
    }

    /// (a) A profile directory **containing a space**, which is where an
    /// operator on macOS naturally points one (`~/Library/Application Support/…`).
    ///
    /// This is the case a `split_whitespace()` implementation gets wrong, and
    /// it fails in the silent direction: the token never matches, so the
    /// browser is never recognised as ours and the orphan is never reaped —
    /// forever, on every boot. Matching argv ELEMENTS has no such failure,
    /// because the kernel already did the splitting and it did it correctly.
    #[test]
    fn a_user_data_dir_containing_a_space_still_matches() {
        let dir = Path::new("/tmp/App Support/udd/default");
        assert!(argv_names_dir(
            &argv(&[
                "/x/chrome",
                "--user-data-dir=/tmp/App Support/udd/default",
                "--headless=new",
            ]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));
        assert!(argv_names_dir(
            &argv(&[
                "/x/chrome",
                "--user-data-dir",
                "/tmp/App Support/udd/default"
            ]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));
        // The sibling-prefix trap survives the space.
        assert!(!argv_names_dir(
            &argv(&[
                "/x/chrome",
                "--user-data-dir=/tmp/App Support/udd/default-2"
            ]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));
    }

    /// (b) A single element that LOOKS like a joined command line. It must not
    /// match — matching it would mean the implementation is scanning inside an
    /// element rather than comparing elements, i.e. the substring behaviour has
    /// come back wearing a different shape.
    #[test]
    fn an_element_that_merely_contains_the_flag_does_not_match() {
        let dir = Path::new("/tmp/udd/default");
        assert!(!argv_names_dir(
            &argv(&["--user-data-dir=/tmp/udd/default --headless=new"]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));
        assert!(!argv_names_dir(
            &argv(&["/x/chrome --user-data-dir=/tmp/udd/default"]),
            Engine::Chromium.data_dir_flag(),
            dir
        ));
    }

    /// The whole point of reading argv before killing: a pid recorded hours ago
    /// may belong to somebody else's process now. "The sidecar named this pid"
    /// is not evidence; "the process still carries OUR user-data-dir" is.
    ///
    /// Four sidecars, four outcomes, one sweep. The SIBLING PREFIX
    /// (`recycled` vs `recycled-2`) is the one a substring test gets wrong:
    /// `reap_orphans` walks profiles under one root, so the flags it builds are
    /// prefixes of one another by construction, and `sanitize_session_key`
    /// produces prefix-related names routinely (`work` / `work-archive`).
    #[test]
    fn reap_orphans_kills_only_the_process_that_carries_our_own_flag() {
        let reg = registry_with(&[
            ("default", 111, "/tmp/udd/default"),
            ("recycled", 222, "/tmp/udd/recycled"),
            ("gone", 333, "/tmp/udd/gone"),
            ("opaque", 444, "/tmp/udd/opaque"),
        ]);
        let killed = std::cell::RefCell::new(Vec::new());
        let n = reap_orphans(
            reg.path(),
            &|pid| match pid {
                // Ours, with a macOS env bleed sitting in the argv.
                111 => ArgvProbe::Argv(argv(&[
                    "/x/chrome",
                    "--user-data-dir=/tmp/udd/default",
                    "--headless=new",
                    "ZSH_AI_PROMPT_EXTEND=Always",
                    "prefer",
                    "modern",
                    "CLI",
                    "tools",
                    "like",
                    "ripgrep",
                ])),
                // A recycled pid: alive, and it is the NEIGHBOURING profile's
                // browser. A substring test would kill it.
                222 => ArgvProbe::Argv(argv(&["/x/chrome", "--user-data-dir=/tmp/udd/recycled-2"])),
                333 => ArgvProbe::Absent,
                444 => ArgvProbe::Unreadable,
                _ => ArgvProbe::Absent,
            },
            &|pid| {
                killed.borrow_mut().push(pid);
                true
            },
        );
        assert_eq!(n.reaped, 1, "exactly the matching pid is reaped");
        assert_eq!(*killed.borrow(), vec![111]);

        // Killed -> record gone.
        assert!(!reg.path().join("default.json").exists());
        // Provably somebody else's -> record gone, process untouched.
        assert!(!reg.path().join("recycled.json").exists());
        // Absent from the process table -> nothing to kill, record stale, gone.
        assert!(!reg.path().join("gone.json").exists());
        // Argv unreadable -> we learned NOTHING. Keep the record.
        assert!(
            reg.path().join("opaque.json").exists(),
            "the record must survive an unreadable argv: it is the only way \
             this browser can ever be reaped"
        );
    }

    /// A kill that is refused (the process is still alive; `sysinfo` returned
    /// `false`, or an OS refusal like EPERM) must not be spent as a reap: the
    /// browser is still running, so deleting the sidecar here would make it
    /// unfindable forever — the same permanent-orphan outcome the
    /// `Unreadable` arm exists to prevent, arriving through the kill path
    /// instead (判据 §4 — the guard must assert the effect landed, not that
    /// the call happened; round-1 review finding F1).
    #[test]
    fn a_refused_kill_is_not_counted_and_the_record_survives() {
        let reg = registry_with(&[("default", 111, "/tmp/udd/default")]);
        let n = reap_orphans(
            reg.path(),
            &|_| ArgvProbe::Argv(argv(&["/x/chrome", "--user-data-dir=/tmp/udd/default"])),
            &|_pid| false,
        );
        assert_eq!(n.reaped, 0, "a refused kill must not be counted as reaped");
        assert!(
            reg.path().join("default.json").exists(),
            "the record must survive a refused kill: it is the only way \
             this browser can ever be found again"
        );
    }

    /// An absent pid takes its record with it, whatever caused the absence.
    /// `argv_probe` maps a **zombie** process (already exited, not yet reaped
    /// by its parent) onto this same `Absent` state by construction — there is
    /// nothing left to kill either way, so the record is stale and must go.
    /// Answering `Unreadable` instead would keep the sidecar on every boot
    /// forever for a process that can never be reaped again, which is the
    /// `Unreadable` arm's protection turned into a leak.
    ///
    /// ⚠️ This test exercises `reap_orphans`' handling of `Absent`, not
    /// `argv_probe`'s classification of a zombie — the mapping from
    /// `ProcessStatus::Zombie` to `Absent` lives in the production reader and
    /// has no unit coverage, because manufacturing a zombie in-process is not
    /// worth what it costs. Recorded rather than implied.
    #[test]
    fn an_absent_process_takes_its_record_with_it() {
        let reg = registry_with(&[("default", 555, "/tmp/udd/default")]);
        let killed = std::cell::RefCell::new(Vec::new());
        let n = reap_orphans(reg.path(), &|_| ArgvProbe::Absent, &|pid| {
            killed.borrow_mut().push(pid);
            true
        });
        assert_eq!(
            n.reaped, 0,
            "a process that already exited must not be 'reaped'"
        );
        assert!(killed.borrow().is_empty());
        assert!(!reg.path().join("default.json").exists());
    }

    /// The case the first two drafts of this function got backwards, kept as
    /// its own test because it is the expensive one.
    ///
    /// `Unreadable` is routine on Windows, where `sysinfo` often cannot read
    /// another process's command line — i.e. the platform spec §3.6 already
    /// flags as unexercised is exactly the one where the wrong answer would be
    /// permanent. Deleting the record there is irreversible: the browser stays
    /// alive and the only thing that could ever find it again is gone
    /// (判据 §8 crossed with §15 — a one-shot latch missed once is missed
    /// forever). It is also why the probe has THREE states and not `Option`:
    /// an `Option` cannot tell "no such process" from "I could not look", and
    /// collapsing those two IS the defect.
    #[test]
    fn an_unreadable_argv_kills_nothing_and_keeps_everything() {
        let reg = registry_with(&[("default", 444, "/tmp/udd/default")]);
        let killed = std::cell::RefCell::new(Vec::new());
        let n = reap_orphans(reg.path(), &|_| ArgvProbe::Unreadable, &|pid| {
            killed.borrow_mut().push(pid);
            true
        });
        assert_eq!(n.reaped, 0);
        assert!(killed.borrow().is_empty());
        assert!(reg.path().join("default.json").exists());
    }

    /// M6: an unparseable sidecar is quarantined to `.corrupt` AND counted
    /// `corrupt_pending` in the SAME sweep that creates it — before this fix,
    /// the count did not exist at all and the file was never looked at again
    /// by anything.
    #[test]
    fn an_unparseable_sidecar_is_quarantined_and_counted_pending_immediately() {
        let reg = registry_with(&[]);
        std::fs::write(reg.path().join("default.json"), b"not valid json").expect("write");
        let n = reap_orphans(reg.path(), &|_| ArgvProbe::Absent, &|_| true);
        assert_eq!(n.reaped, 0);
        assert_eq!(
            n.corrupt_pending, 1,
            "the fresh quarantine must be counted, not silent"
        );
        assert_eq!(n.corrupt_superseded, 0);
        assert!(
            !reg.path().join("default.json").exists(),
            "the unparseable record must still be renamed aside"
        );
        assert!(
            reg.path().join("default.json.corrupt").exists(),
            "renamed to .corrupt, not deleted — a live browser might be behind it"
        );
    }

    /// M6: a `.corrupt` sidecar with no fresh sibling is kept AND counted —
    /// the sweep must not go back to silence just because the file has been
    /// seen once already. Nothing has resolved this key since the previous
    /// sweep quarantined it, so it stays exactly where it was.
    #[test]
    fn a_corrupt_sidecar_with_no_fresh_sibling_stays_pending_across_sweeps() {
        let reg = registry_with(&[]);
        std::fs::write(
            reg.path().join("default.json.corrupt"),
            b"leftover from a torn write",
        )
        .expect("write");
        let n = reap_orphans(reg.path(), &|_| ArgvProbe::Absent, &|_| true);
        assert_eq!(n.reaped, 0);
        assert_eq!(
            n.corrupt_pending, 1,
            "an unresolved corrupt sidecar must stay counted"
        );
        assert_eq!(n.corrupt_superseded, 0);
        assert!(
            reg.path().join("default.json.corrupt").exists(),
            "nothing resolved this key since it was quarantined — must not be deleted blind"
        );
    }

    /// M6: once a fresh, parseable sidecar exists for the SAME session key,
    /// the stale `.corrupt` sibling is superseded — the same determinate
    /// rule a recycled pid or an absent process is dropped under, not a
    /// time-based guess. Existence of the fresh record is the whole signal:
    /// this fixture's fresh sidecar names a pid `reap_orphans` will itself
    /// keep (`ArgvProbe::Unreadable`), proving the two decisions are
    /// independent — superseding the `.corrupt` file must not depend on
    /// what happens to its sibling in the SAME sweep.
    #[test]
    fn a_corrupt_sidecar_is_superseded_once_a_fresh_sidecar_exists_for_the_same_key() {
        let reg = registry_with(&[("default", 444, "/tmp/udd/default")]);
        std::fs::write(
            reg.path().join("default.json.corrupt"),
            b"leftover from a torn write",
        )
        .expect("write");
        let n = reap_orphans(reg.path(), &|_| ArgvProbe::Unreadable, &|_| true);
        assert_eq!(
            n.corrupt_superseded, 1,
            "a fresh sibling must supersede the stale corrupt file"
        );
        assert_eq!(n.corrupt_pending, 0);
        assert!(
            !reg.path().join("default.json.corrupt").exists(),
            "superseded means removed, not kept alongside the fresh record"
        );
        assert!(
            reg.path().join("default.json").exists(),
            "the fresh sidecar's own fate (Unreadable -> keep) is unrelated to its \
             stale sibling being superseded"
        );
    }

    /// F4: `argv_probe` is the only classifier that runs in production —
    /// every other test drives `reap_orphans` through an injected closure.
    /// If `UpdateKind::Always` were ever dropped from the refresh kind,
    /// `cmd()` would go empty and every probe would silently answer
    /// `Unreadable` forever (判据 §2, §7) — the sweep would never reap
    /// anything again, with a log line that reads like an ordinary Windows
    /// quirk. Pin both ends of the real reader directly: our own live pid
    /// must come back readable, and a pid that cannot exist must come back
    /// `Absent`.
    #[test]
    fn argv_probe_reads_this_process_and_answers_absent_for_a_pid_that_cannot_exist() {
        assert!(matches!(argv_probe(std::process::id()), ArgvProbe::Argv(v) if !v.is_empty()));
        assert_eq!(argv_probe(u32::MAX), ArgvProbe::Absent);
    }

    /// A sidecar written by an Aleph that predates engines has no `engine`
    /// key at all. It must read as Chromium — the only engine that could
    /// have written it. Anything else (a parse failure, or obscura) sends a
    /// live orphaned Chromium either into the `.corrupt` quarantine or into
    /// a sweep that matches the wrong argv switch, and in both cases that
    /// browser is never reaped again.
    ///
    /// The JSON here is a byte-for-byte record of the shape
    /// `write_sidecar_record` produced at HEAD `0a3e8a48a`
    /// (`chromium_launch.rs:559-564`), not a re-serialisation of the new
    /// struct — a re-serialisation would only prove serde agrees with
    /// itself (判据 §10).
    #[test]
    fn a_sidecar_written_before_engines_existed_reads_as_chromium() {
        let legacy = r#"{"pid":4242,"http_url":"http://127.0.0.1:58363","user_data_dir":"/tmp/explicit-udd","aleph_version":"26.9.5"}"#;
        let rec: EngineSidecar = serde_json::from_str(legacy).expect("legacy record must parse");
        assert_eq!(rec.engine, Engine::Chromium);
        assert_eq!(rec.pid, 4242);
        assert_eq!(rec.data_dir, std::path::PathBuf::from("/tmp/explicit-udd"));
        assert_eq!(rec.build, "26.9.5");
        assert_eq!(rec.http_url.as_deref(), Some("http://127.0.0.1:58363"));
    }

    /// The wire keys did not move. A record this build writes must still be
    /// readable by the Aleph the operator may downgrade to — and, more
    /// importantly, by the SAME sweep after a partial rollout, where a new
    /// binary and an old one share one `$ALEPH_HOME`.
    #[test]
    fn a_new_sidecar_keeps_the_legacy_wire_keys() {
        let json = serde_json::to_string(&EngineSidecar {
            engine: Engine::Obscura,
            pid: 7,
            http_url: None,
            data_dir: std::path::PathBuf::from("/tmp/o"),
            build: "26.9.6".into(),
        })
        .expect("serialize");
        let v: serde_json::Value = serde_json::from_str(&json).expect("valid json");
        assert!(v.get("user_data_dir").is_some(), "wire key renamed: {json}");
        assert!(v.get("aleph_version").is_some(), "wire key renamed: {json}");
        assert!(
            v.get("data_dir").is_none(),
            "the Rust name leaked onto the wire: {json}"
        );
        assert_eq!(
            v.get("engine").and_then(serde_json::Value::as_str),
            Some("obscura")
        );
    }

    /// The per-engine switch, at the predicate that authorises the kill.
    /// A Chromium process must not satisfy an obscura record's flag and the
    /// reverse — this is the whole behaviour addition of this task.
    #[test]
    fn the_argv_match_uses_the_records_own_engine_switch() {
        let dir = std::path::Path::new("/tmp/state/default");
        let chromium_argv: Vec<String> = ["/x/chrome", "--user-data-dir=/tmp/state/default"]
            .iter()
            .map(|s| (*s).to_string())
            .collect();
        let obscura_argv: Vec<String> =
            ["/x/obscura", "serve", "--storage-dir", "/tmp/state/default"]
                .iter()
                .map(|s| (*s).to_string())
                .collect();

        assert!(argv_names_dir(
            &chromium_argv,
            Engine::Chromium.data_dir_flag(),
            dir
        ));
        assert!(argv_names_dir(
            &obscura_argv,
            Engine::Obscura.data_dir_flag(),
            dir
        ));
        // Crossed: the same directory, the other engine's switch. Both must
        // be false, or one engine's record can kill the other's process.
        assert!(!argv_names_dir(
            &chromium_argv,
            Engine::Obscura.data_dir_flag(),
            dir
        ));
        assert!(!argv_names_dir(
            &obscura_argv,
            Engine::Chromium.data_dir_flag(),
            dir
        ));
    }

    /// `kill` must assert the EFFECT — the child exited AND was reaped — not
    /// that a signal was sent (判据 §4). `try_reap` is that decision, and it
    /// is a plain function over a real `Child` so all three outcomes are
    /// deterministic without a browser and without signalling anything the
    /// test does not own.
    ///
    /// **Reaping is the whole point of holding the handle.** A child that is
    /// killed and never waited on stays a zombie in this process's table until
    /// the daemon exits — invisible to `kill -0`, which succeeds on a zombie,
    /// and therefore invisible to any guard that asks the OS "is this pid
    /// alive". The `None` left behind by a successful reap is what says the
    /// slot is settled.
    #[cfg(unix)]
    #[test]
    fn try_reap_answers_only_when_the_child_has_actually_been_reaped() {
        use crate::sync_primitives::Mutex;
        use std::sync::Arc;

        // (1) A live child is NOT gone, and the handle must survive the
        //     question so a later poll can still reap it.
        let child = std::process::Command::new("sleep")
            .arg("30")
            .spawn()
            .expect("spawn the stand-in browser");
        let pid = child.id();
        let slot = Arc::new(Mutex::new(Some(child)));
        assert!(!try_reap(&slot), "a running child must not read as gone");
        assert!(
            slot.lock().unwrap_or_else(|e| e.into_inner()).is_some(),
            "the handle must survive a negative answer — dropping it here \
             leaves a zombie nothing can ever wait on"
        );

        // (2) After a real kill it is gone, and the slot is emptied, which is
        //     how a second call knows the reap already happened.
        slot.lock()
            .unwrap_or_else(|e| e.into_inner())
            .as_mut()
            .expect("still held")
            .kill()
            .expect("kill the stand-in");
        let mut reaped = false;
        for _ in 0..200 {
            if try_reap(&slot) {
                reaped = true;
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(reaped, "a killed child must reap within two seconds");
        assert!(
            slot.lock().unwrap_or_else(|e| e.into_inner()).is_none(),
            "a successful reap must empty the slot"
        );
        // `kill -0` only fails once the child is REAPED, not merely dead — so
        // this assertion is what separates "we waited on it" from "we sent a
        // signal and walked away".
        let still_there = std::process::Command::new("kill")
            .args(["-0", &pid.to_string()])
            .status()
            .expect("kill -0");
        assert!(!still_there.success(), "the child was never reaped");

        // (3) An already-emptied slot is gone, and asking twice is safe: the
        //     kill path calls this on every poll.
        assert!(try_reap(&slot));
        assert!(try_reap(&Arc::new(Mutex::new(None))));
    }

    /// The sweep reads the switch off the RECORD, end to end — not just at
    /// the predicate. Two records under one root, two engines, and each
    /// engine's process carries the OTHER's directory flag. Nothing may die.
    ///
    /// Without the per-engine flag this test kills the chromium process for
    /// the obscura record (both would be matched with `--user-data-dir`),
    /// which is the cross-engine kill the whole change exists to prevent.
    #[test]
    fn a_record_never_authorises_a_kill_against_the_other_engines_process() {
        let reg = registry_of(Engine::Obscura, &[("default", 111, "/tmp/state/default")]);
        let killed = std::cell::RefCell::new(Vec::new());
        let n = reap_orphans(
            reg.path(),
            // A live CHROMIUM under the very same directory.
            &|_| ArgvProbe::Argv(argv(&["/x/chrome", "--user-data-dir=/tmp/state/default"])),
            &|pid| {
                killed.borrow_mut().push(pid);
                true
            },
        );
        assert_eq!(n.reaped, 0, "an obscura record must not reap a chromium");
        assert!(killed.borrow().is_empty());
        // Provably somebody else's argv is a DETERMINATE answer, so the
        // record goes — same arm a recycled pid takes.
        assert!(!reg.path().join("default.json").exists());
    }

    /// R10: the leaf is frozen. Moving it hides every record a previous Aleph
    /// left behind, and those records are the only way its browsers can ever
    /// be reaped. If it is ever renamed, the same commit must migrate the old
    /// leaf's `*.json` — after the rename, nothing knows they exist.
    #[test]
    fn the_sidecar_registry_leaf_is_frozen() {
        let home = tempfile::tempdir().expect("tempdir");
        let _guard = crate::utils::paths::AlephHomeEnvGuard::acquire_and_set(home.path());
        let dir = sidecar_registry_dir().expect("home resolves");
        assert_eq!(
            dir.file_name().and_then(|s| s.to_str()),
            Some("chromium"),
            "the sidecar registry leaf moved; every record a pre-upgrade Aleph \
             left behind is now invisible to the sweep"
        );
    }
}
