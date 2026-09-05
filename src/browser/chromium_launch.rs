//! Aleph launches Chromium; `playwright-cli` only attaches to what it finds.
//!
//! Why this module exists at all is a measurement, not a preference. The Chrome
//! spike (`docs/superpowers/specs/2026-09-05-browser-live-view-evidence/`)
//! established that a CLI-launched Chrome *does* open a debug port — and that
//! the port is useless as a contract: it is random per launch, a user-supplied
//! `--remote-debugging-port` loses to Playwright's own (Chrome takes the last
//! occurrence), no `DevToolsActivePort` file is written into Playwright's
//! profile dir, and `playwright-cli list` prints no endpoint. The only
//! discovery route left was scraping `ps`. Launching it ourselves replaces all
//! of that with a file Chrome writes on purpose.
//!
//! The second consequence is ownership: under `attach --cdp`, `playwright-cli
//! close` disconnects and leaves the browser running (measured: 9 Chrome
//! processes before and after, endpoint still serving, page still on its URL).
//! So the browser's life is ours to end, which is what [`ChromiumChild`] and
//! [`reap_orphans`] are for.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};

use crate::utils::no_window::NoWindow;

use super::error::BrowserError;

/// How long the `DevToolsActivePort` file may take to appear before the launch
/// is called failed.
///
/// A cold Chrome on a loaded machine is slow, and the spike never measured this
/// window (it read the file after the fact) — so the number is chosen to match
/// the repo's existing answer to "how long may bringing up a browser take":
/// `playwright_cli::SESSION_START_TIMEOUT_SECS` and `chrome_mcp`'s
/// `create_session` both say 60 s. Half of it is the budget for the *port*,
/// which appears well before the browser is usable.
pub(crate) const DEVTOOLS_PORT_DEADLINE: Duration = Duration::from_secs(30);

/// How often the port file is polled while waiting.
const PORT_POLL_INTERVAL: Duration = Duration::from_millis(50);

/// Chrome's own file, written into the user-data-dir. Name fixed by Chrome.
const DEVTOOLS_PORT_FILE: &str = "DevToolsActivePort";

/// Extension every sidecar record is written with.
const SIDECAR_EXT: &str = "json";

/// Everything the Chromium process needs at launch.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ChromiumLaunchSpec {
    pub binary: PathBuf,
    pub user_data_dir: PathBuf,
    pub headless: bool,
    pub proxy: Option<String>,
    pub extra_args: Vec<String>,
}

impl ChromiumLaunchSpec {
    /// The full argv, operator args first.
    ///
    /// Order is the contract. Chrome resolves a duplicated switch to its LAST
    /// occurrence — that is precisely how Playwright's own
    /// `--remote-debugging-port=58419` beat a caller-supplied `=0` in the spike.
    /// So `extra_args` lead and every switch this launch depends on follows
    /// them, where an operator's duplicate cannot displace it. The URL is last
    /// because it is positional.
    pub(crate) fn argv(&self) -> Vec<String> {
        let mut argv = self.extra_args.clone();
        argv.push("--no-first-run".to_string());
        argv.push("--no-default-browser-check".to_string());
        if self.headless {
            argv.push("--headless=new".to_string());
        }
        if let Some(proxy) = &self.proxy {
            argv.push(format!("--proxy-server={proxy}"));
        }
        argv.push(format!("--user-data-dir={}", self.user_data_dir.display()));
        argv.push("--remote-debugging-port=0".to_string());
        // `about:blank` keeps the launch out of the SSRF guard's way; the
        // caller navigates afterwards through the guarded path. Same reasoning
        // the deleted `open_argv` carried.
        argv.push("about:blank".to_string());
        argv
    }
}

/// A live CDP endpoint on loopback.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct CdpEndpoint {
    /// `http://127.0.0.1:<port>` — the form `playwright-cli attach --cdp` takes
    /// (both this and the ws form were accepted in the spike; the http one is
    /// shorter and does not embed a browser id that changes per launch).
    pub http_url: String,
    /// `ws://127.0.0.1:<port>/devtools/browser/<id>` — what a raw CDP client
    /// (the live view, Plan 2) connects to.
    pub ws_url: String,
    /// The Chromium process we launched.
    pub pid: u32,
}

/// Parse Chrome's two-line `DevToolsActivePort`: the port, then the browser
/// websocket path.
///
/// Returns `None` for every shape that is not both lines — which is the normal
/// state while Chrome is still writing the file. That `None` means "not yet",
/// and the poll loop is the only thing allowed to spend it; nothing may read it
/// as "failed" (判据 §8).
pub(crate) fn parse_devtools_active_port(text: &str) -> Option<(u16, String)> {
    let mut lines = text.lines();
    let port: u16 = lines.next()?.trim().parse().ok()?;
    if port == 0 {
        return None;
    }
    let path = lines.next()?.trim();
    if !path.starts_with('/') {
        return None;
    }
    Some((port, path.to_string()))
}

/// [`parse_devtools_active_port`] plus the pid, as one endpoint.
pub(crate) fn endpoint_from_port_file(text: &str, pid: u32) -> Option<CdpEndpoint> {
    let (port, path) = parse_devtools_active_port(text)?;
    Some(CdpEndpoint {
        http_url: format!("http://127.0.0.1:{port}"),
        ws_url: format!("ws://127.0.0.1:{port}{path}"),
        pid,
    })
}

/// What Aleph records about a browser it launched.
///
/// These live in ONE registry directory, not beside each browser's profile.
/// A profile may configure `user_data_dir` to anywhere on disk (the repo's own
/// QA does), so a per-udd record puts itself outside anything a boot sweep can
/// walk — and the sweep would then miss exactly the case the fixture
/// exercises. One directory means "which browsers are there to reclaim" has a
/// single derivation (判据 §12).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ChromiumSidecar {
    pub pid: u32,
    pub http_url: String,
    /// The profile directory that process was launched with. Recorded rather
    /// than implied by the file's location, because the file is not stored
    /// there — and this is the value the orphan sweep matches against argv.
    pub user_data_dir: PathBuf,
    /// The build that launched it. Not used as a gate — recorded because an
    /// orphan from a different version is exactly the case a reader will want
    /// named when this goes wrong.
    pub aleph_version: String,
}

/// The one directory every sidecar lives in.
pub(crate) fn sidecar_registry_dir() -> Result<PathBuf, BrowserError> {
    super::playwright_launch::browser_state_dir("chromium")
}

/// This profile's record. Sanitized through the same helper the launch config
/// and the derived udd use, so a profile name can never escape the registry.
pub(crate) fn sidecar_path(session_key: &str) -> Result<PathBuf, BrowserError> {
    Ok(sidecar_registry_dir()?.join(format!(
        "{}.{SIDECAR_EXT}",
        super::playwright_launch::sanitize_session_key(session_key)
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
/// Both spellings Chrome accepts are matched: `--user-data-dir=<path>` and the
/// two-token `--user-data-dir <path>`. Missing the second would let a browser
/// launched that way become unreapable.
///
/// **Nothing here splits, either.** An implementation that joined the argv and
/// split it on whitespace would lose every `user_data_dir` containing a space —
/// `~/Library/Application Support/…` is an ordinary place for an operator to
/// point a profile — and it would fail silently: the token never matches, so
/// that browser is never recognised as ours and its orphan is never reaped, on
/// every boot, forever. The kernel already split the argv; comparing whole
/// elements inherits that and adds nothing of its own.
#[must_use]
pub(crate) fn argv_names_dir(argv: &[String], dir: &Path) -> bool {
    let joined = format!("--user-data-dir={}", dir.display());
    let value = dir.to_string_lossy();
    argv.iter().enumerate().any(|(i, word)| {
        word == &joined
            || (word == "--user-data-dir" && argv.get(i + 1).is_some_and(|v| v.as_str() == value))
    })
}

/// One Chromium process owned by this Aleph.
pub(crate) struct ChromiumChild {
    child: Child,
    endpoint: CdpEndpoint,
    user_data_dir: PathBuf,
    session_key: String,
}

impl ChromiumChild {
    /// Launch Chromium and wait for it to publish its debug port.
    ///
    /// `session_key` is the profile name, and it is taken here rather than
    /// derived, because it is what names this browser's record in the sidecar
    /// registry — the only thing that can find the process again after a crash.
    pub(crate) async fn spawn(
        spec: &ChromiumLaunchSpec,
        session_key: &str,
        deadline: Duration,
    ) -> Result<Self, BrowserError> {
        tokio::fs::create_dir_all(&spec.user_data_dir)
            .await
            .map_err(|e| BrowserError::LaunchFailed {
                stage: "spawn",
                detail: format!(
                    "cannot create the chromium user-data-dir {}: {e}",
                    spec.user_data_dir.display()
                ),
            })?;
        // A leftover file from the PREVIOUS launch would be read as this one's
        // endpoint — a port that is either closed or, worse, somebody else's.
        let port_file = spec.user_data_dir.join(DEVTOOLS_PORT_FILE);
        let _ = tokio::fs::remove_file(&port_file).await;

        let mut cmd = Command::new(&spec.binary);
        cmd.args(spec.argv())
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Same discipline as the CLI child (`playwright_cli::spawn`): the
        // browser never needs the parent's credentials, and over-stripping is
        // safe.
        for (name, _) in std::env::vars() {
            if crate::security::secret_env::is_secret_env(&name) {
                cmd.env_remove(&name);
            }
        }
        let mut child = cmd
            .no_window()
            .spawn()
            .map_err(|e| BrowserError::LaunchFailed {
                stage: "spawn",
                detail: format!("{}: {e}", spec.binary.display()),
            })?;
        let pid = child.id();

        let started = Instant::now();
        loop {
            if let Ok(text) = tokio::fs::read_to_string(&port_file).await {
                if let Some(endpoint) = endpoint_from_port_file(&text, pid) {
                    let me = Self {
                        child,
                        endpoint,
                        user_data_dir: spec.user_data_dir.clone(),
                        session_key: session_key.to_string(),
                    };
                    me.write_sidecar().await;
                    tracing::info!(pid, endpoint = %me.endpoint.http_url, "chromium launched");
                    return Ok(me);
                }
            }
            // Chrome died before publishing: a different fact from "the file is
            // late", and the operator's fix is different too (a missing shared
            // library, a bad `--user-data-dir`, a crashed sandbox).
            if let Ok(Some(status)) = child.try_wait() {
                return Err(BrowserError::LaunchFailed {
                    stage: "chromium-exit",
                    detail: format!(
                        "{} exited with {status} before writing {DEVTOOLS_PORT_FILE}",
                        spec.binary.display()
                    ),
                });
            }
            if started.elapsed() >= deadline {
                let _ = child.kill();
                let _ = child.wait();
                return Err(BrowserError::LaunchFailed {
                    stage: "devtools-port",
                    detail: format!(
                        "no {DEVTOOLS_PORT_FILE} under {} after {}s",
                        spec.user_data_dir.display(),
                        deadline.as_secs()
                    ),
                });
            }
            tokio::time::sleep(PORT_POLL_INTERVAL).await;
        }
    }

    pub(crate) const fn endpoint(&self) -> &CdpEndpoint {
        &self.endpoint
    }

    /// Whether the browser is still running.
    ///
    /// `Err` from `try_wait` is answered **`true`**, deliberately. "I could not
    /// tell" is not "it is dead", and killing on an unknown would orphan a live
    /// browser. The attach that follows settles it for free: a dead endpoint
    /// answers `ECONNREFUSED` and the driver's retry path forgets the child.
    pub(crate) fn alive(&mut self) -> bool {
        match self.child.try_wait() {
            Ok(None) | Err(_) => true,
            Ok(Some(_)) => false,
        }
    }

    /// Kill the browser and clear its registry record.
    ///
    /// `wait()` runs **only after a successful `kill()`**. It is a blocking
    /// call and every production caller is inside async code: after a kill the
    /// reap is immediate, but `kill()` can fail (EPERM, or the child was
    /// already reaped) and then `wait()` would park a tokio worker until the
    /// process happened to exit on its own.
    pub(crate) fn shutdown(mut self) {
        let pid = self.endpoint.pid;
        match self.child.kill() {
            Ok(()) => {
                let _ = self.child.wait();
                tracing::info!(pid, "chromium shut down");
            }
            // Say which of the two happened rather than logging "shut down"
            // over a process that may still be running: an untrue log line is
            // the thing a reader would spend as evidence.
            Err(e) => tracing::warn!(pid, error = %e, "could not kill chromium; leaving it"),
        }
        match sidecar_path(&self.session_key) {
            Ok(path) => {
                let _ = std::fs::remove_file(path);
            }
            Err(e) => tracing::warn!(error = %e, "cannot resolve the chromium sidecar to remove"),
        }
    }

    async fn write_sidecar(&self) {
        let path = match sidecar_path(&self.session_key) {
            Ok(p) => p,
            Err(e) => {
                tracing::warn!(error = %e, "cannot resolve the chromium sidecar path");
                return;
            }
        };
        if let Some(dir) = path.parent() {
            if let Err(e) = tokio::fs::create_dir_all(dir).await {
                tracing::warn!(error = %e, "cannot create the chromium sidecar registry");
                return;
            }
        }
        let body = match serde_json::to_string(&ChromiumSidecar {
            pid: self.endpoint.pid,
            http_url: self.endpoint.http_url.clone(),
            user_data_dir: self.user_data_dir.clone(),
            aleph_version: env!("ALEPH_VERSION").to_string(),
        }) {
            Ok(b) => b,
            Err(e) => {
                tracing::warn!(error = %e, "cannot serialize the chromium sidecar");
                return;
            }
        };
        if let Err(e) = tokio::fs::write(&path, body).await {
            // Best-effort here, but NOT unobserved: a missing sidecar costs an
            // orphan across a crash. The QA `attach` stage asserts the file
            // exists and that its pid matches a live process (Task 9 step 6),
            // because no unit test can see this.
            tracing::warn!(error = %e, path = %path.display(), "cannot write the chromium sidecar");
        }
    }
}

/// Kill Chromium processes left behind by a previous Aleph.
///
/// `registry` is [`sidecar_registry_dir`] — one directory holding one record
/// per profile, whatever each profile's `user_data_dir` happens to be. That is
/// why the sweep can be a single walk (判据 §12: the set has one derivation).
///
/// Four outcomes per record, and they are deliberately NOT collapsed:
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
/// Both effects are injected so the decision is testable without a browser;
/// [`reap_orphans_now`] is the production wiring.
pub(crate) fn reap_orphans(
    registry: &Path,
    argv_of: &dyn Fn(u32) -> ArgvProbe,
    kill: &dyn Fn(u32),
) -> usize {
    let Ok(entries) = std::fs::read_dir(registry) else {
        // The dir not existing is the normal first-boot state, not a failure.
        return 0;
    };
    let mut reaped = 0;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().is_none_or(|e| e != SIDECAR_EXT) {
            continue;
        }
        let Ok(body) = std::fs::read_to_string(&path) else {
            continue;
        };
        let Ok(rec) = serde_json::from_str::<ChromiumSidecar>(&body) else {
            // A record we cannot parse names no pid, so it can never be acted
            // on; dropping it is the only way it stops being read every boot.
            tracing::warn!(path = %path.display(), "unparseable chromium sidecar; dropping it");
            let _ = std::fs::remove_file(&path);
            continue;
        };
        match argv_of(rec.pid) {
            ArgvProbe::Argv(argv) if argv_names_dir(&argv, &rec.user_data_dir) => {
                tracing::info!(
                    pid = rec.pid,
                    dir = %rec.user_data_dir.display(),
                    "reaping orphaned chromium"
                );
                kill(rec.pid);
                reaped += 1;
                let _ = std::fs::remove_file(&path);
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
    reaped
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

/// [`reap_orphans`] wired to the real process table.
pub(crate) fn reap_orphans_now() -> usize {
    let registry = match sidecar_registry_dir() {
        Ok(d) => d,
        Err(e) => {
            tracing::warn!(error = %e, "cannot sweep orphaned chromium processes");
            return 0;
        }
    };
    reap_orphans(&registry, &argv_probe, &|pid| {
        let killed = crate::utils::process_alive::with_process_specifics(
            pid,
            sysinfo::ProcessRefreshKind::nothing(),
            sysinfo::Process::kill,
        );
        if killed != Some(true) {
            tracing::warn!(pid, "orphaned chromium did not accept the kill");
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn spec() -> ChromiumLaunchSpec {
        ChromiumLaunchSpec {
            binary: PathBuf::from("/Applications/Google Chrome.app/Contents/MacOS/Google Chrome"),
            user_data_dir: PathBuf::from("/tmp/udd"),
            headless: true,
            proxy: Some("socks5://127.0.0.1:1080".into()),
            extra_args: vec!["--disable-gpu".into()],
        }
    }

    /// Golden vector. The ORDER is the contract, not decoration: Chrome takes
    /// the LAST occurrence of a duplicated switch, which is exactly how
    /// Playwright's own `--remote-debugging-port` beat a user-supplied `=0` in
    /// the spike. So the operator's `extra_args` go FIRST and every switch this
    /// launch depends on goes after them, where a duplicate cannot displace it.
    #[test]
    fn argv_puts_the_contract_switches_after_the_operator_args() {
        assert_eq!(
            spec().argv(),
            vec![
                "--disable-gpu",
                "--no-first-run",
                "--no-default-browser-check",
                "--headless=new",
                "--proxy-server=socks5://127.0.0.1:1080",
                "--user-data-dir=/tmp/udd",
                "--remote-debugging-port=0",
                "about:blank",
            ]
        );
    }

    #[test]
    fn a_headed_launch_omits_the_headless_switch_and_a_proxyless_one_the_proxy() {
        let argv = ChromiumLaunchSpec {
            headless: false,
            proxy: None,
            extra_args: Vec::new(),
            ..spec()
        }
        .argv();
        assert!(!argv.iter().any(|a| a.starts_with("--headless")));
        assert!(!argv.iter().any(|a| a.starts_with("--proxy-server")));
        assert_eq!(argv.last().map(String::as_str), Some("about:blank"));
        assert!(argv.contains(&"--remote-debugging-port=0".to_string()));
    }

    /// The real two-line file, verbatim from the Chrome spike
    /// (`docs/superpowers/specs/2026-09-05-browser-live-view-evidence/chrome-spike-findings.md`
    /// STEP 1): a port on line 1, the browser path on line 2.
    #[test]
    fn the_real_port_file_parses_into_a_port_and_a_browser_path() {
        let text = "58363\n/devtools/browser/ac5f508a-1111-2222-3333-444455556666\n";
        assert_eq!(
            parse_devtools_active_port(text),
            Some((
                58363,
                "/devtools/browser/ac5f508a-1111-2222-3333-444455556666".to_string()
            ))
        );
        let ep = endpoint_from_port_file(text, 4242).expect("endpoint");
        assert_eq!(ep.http_url, "http://127.0.0.1:58363");
        assert_eq!(
            ep.ws_url,
            "ws://127.0.0.1:58363/devtools/browser/ac5f508a-1111-2222-3333-444455556666"
        );
        assert_eq!(ep.pid, 4242);
    }

    /// A half-written file is the NORMAL state during the poll — Chrome creates
    /// it and fills it in. Every partial shape must read as "not yet", never as
    /// an endpoint: `Option::None` here is the "I do not know yet" answer the
    /// poll loop is allowed to spend, and a `Some` built from half a file would
    /// hand `attach --cdp` a URL that cannot connect.
    #[test]
    fn every_partial_or_malformed_port_file_reads_as_not_yet() {
        for bad in [
            "",
            "\n",
            "58363",                     // port written, path not yet
            "58363\n",                   // ditto, with the newline
            "58363\ndevtools/browser/x", // path must be absolute
            "notaport\n/devtools/browser/x",
            "0\n/devtools/browser/x", // port 0 is never a listening port
            "99999999\n/devtools/browser/x",
        ] {
            assert_eq!(parse_devtools_active_port(bad), None, "accepted {bad:?}");
            assert!(
                endpoint_from_port_file(bad, 1).is_none(),
                "accepted {bad:?}"
            );
        }
    }

    #[test]
    fn the_sidecar_round_trips_and_records_the_dir_it_is_not_stored_in() {
        let json = serde_json::to_string(&ChromiumSidecar {
            pid: 4242,
            http_url: "http://127.0.0.1:58363".into(),
            user_data_dir: PathBuf::from("/tmp/explicit-udd"),
            aleph_version: env!("ALEPH_VERSION").to_string(),
        })
        .expect("serialize");
        let back: ChromiumSidecar = serde_json::from_str(&json).expect("deserialize");
        assert_eq!(back.pid, 4242);
        assert_eq!(back.http_url, "http://127.0.0.1:58363");
        // The whole point of the registry: the record lives in ONE directory
        // and names the udd, instead of living IN the udd where a profile that
        // configures its own path puts it outside anything a sweep walks.
        assert_eq!(back.user_data_dir, PathBuf::from("/tmp/explicit-udd"));
        assert_eq!(back.aleph_version, env!("ALEPH_VERSION"));
    }

    /// The containment property the registry inherits from `config_path_for`:
    /// one component under the state dir, whatever the profile is called.
    #[test]
    fn a_sidecar_path_is_one_component_under_the_registry() {
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
            dir
        ));

        // Chrome accepts the two-token form too, so a browser someone launched
        // that way is still ours.
        assert!(argv_names_dir(
            &argv(&["/x/chrome", "--user-data-dir", "/tmp/udd/default"]),
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
            dir
        ));
        // …and the two-token form of the same trap.
        assert!(!argv_names_dir(
            &argv(&["/x/chrome", "--user-data-dir", "/tmp/udd/default-2"]),
            dir
        ));

        // (3) The whole flag string appearing INSIDE a bled-in env value.
        assert!(!argv_names_dir(
            &argv(&[
                "/usr/bin/vim",
                "notes.txt",
                "LAST_CMD=chrome --user-data-dir=/tmp/udd/default",
            ]),
            dir
        ));

        // The path as some other flag's value; a recycled pid; nothing at all.
        assert!(!argv_names_dir(
            &argv(&["/x/chrome", "--crash-dumps-dir=/tmp/udd/default"]),
            dir
        ));
        assert!(!argv_names_dir(
            &argv(&["/usr/bin/vim", "/tmp/udd/default/notes.txt"]),
            dir
        ));
        assert!(!argv_names_dir(&[], dir));
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
            dir
        ));
        assert!(argv_names_dir(
            &argv(&[
                "/x/chrome",
                "--user-data-dir",
                "/tmp/App Support/udd/default"
            ]),
            dir
        ));
        // The sibling-prefix trap survives the space.
        assert!(!argv_names_dir(
            &argv(&[
                "/x/chrome",
                "--user-data-dir=/tmp/App Support/udd/default-2"
            ]),
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
            dir
        ));
        assert!(!argv_names_dir(
            &argv(&["/x/chrome --user-data-dir=/tmp/udd/default"]),
            dir
        ));
    }

    /// Fixture: a registry directory holding one sidecar per profile.
    fn registry_with(entries: &[(&str, u32, &str)]) -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        for (profile, pid, udd) in entries {
            std::fs::write(
                dir.path().join(format!("{profile}.json")),
                serde_json::to_string(&ChromiumSidecar {
                    pid: *pid,
                    http_url: "http://127.0.0.1:1".into(),
                    user_data_dir: PathBuf::from(udd),
                    aleph_version: env!("ALEPH_VERSION").to_string(),
                })
                .expect("serialize"),
            )
            .expect("write");
        }
        dir
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
            &|pid| killed.borrow_mut().push(pid),
        );
        assert_eq!(n, 1, "exactly the matching pid is reaped");
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
            killed.borrow_mut().push(pid)
        });
        assert_eq!(n, 0, "a process that already exited must not be 'reaped'");
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
            killed.borrow_mut().push(pid)
        });
        assert_eq!(n, 0);
        assert!(killed.borrow().is_empty());
        assert!(reg.path().join("default.json").exists());
    }
}
