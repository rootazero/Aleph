//! The `browser_config` surface's wire vocabulary.
//!
//! One item, and it is here rather than in either side because both sides
//! depend on this crate and neither depends on the other. The Panel renders a
//! radio per driver; `alephcore::browser::profile::BrowserDriver` decides what
//! a driver means. With the list spelled in both places, a driver added on the
//! server reaches the Panel as a value it can display **no control for** — and
//! that is not hypothetical: after the default flipped to `cdp`, the Panel's
//! two-option group rendered with nothing selected and had no way to write the
//! new default back, so it was a one-way door off it (判据 §10 — a wire
//! vocabulary held twice cancels out; 判据 §17 — the setting it cannot display
//! is the one that matters).
//!
//! Neither side may treat this as the authority on MEANING. The enum is the
//! authority; this is the enumeration, pinned to it by
//! `BrowserDriver::ALL`-derived guards on the server side and by a
//! completeness guard on the Panel side.

/// Every `default_driver` value `browser_config.get` can report and
/// `browser_config.update` accepts, in the order the server declares them.
///
/// These are serde spellings on a persisted config surface: they are frozen,
/// and adding one is a server-side decision that this array records rather
/// than makes.
pub const BROWSER_DRIVER_WIRE: [&str; 3] = ["managed", "existing_session", "cdp"];

/// The ledger name of the browser engine the `cdp` driver runs.
///
/// Here for the same reason as the array above: the Panel's Browser page shows
/// a runtime-readiness banner, and to say anything true it has to filter
/// `runtimes.list` by the name the SERVER gives that runtime. Held privately on
/// the Panel side, a stale or misspelled copy does not fail loudly — it matches
/// nothing, finds nothing missing, and paints READY over an install with no
/// engine (判据 §10: a wire key held twice cancels out, and this one cancels
/// toward the confident wrong answer).
///
/// `alephcore::runtimes::specs::OBSCURA_RUNTIME` is defined AS this constant,
/// so the ledger entry and the Panel's filter have one author rather than two
/// that agree today.
pub const OBSCURA_RUNTIME_WIRE: &str = "obscura";
