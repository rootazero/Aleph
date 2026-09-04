//! Source-level census: a provider's HTTP failure path must come out of the
//! `base::send` funnel, never out of a `reqwest` value the provider mapped
//! itself.
//!
//! # Why this cannot be a runtime test
//!
//! Same reason as `capability_census`: seven of the nine providers hardcode
//! their endpoint, so there is nowhere to point an HTTP mock that would
//! observe a hand-rolled status mapping. The source is the only place where
//! "did this provider map a status code itself" can be asked of all nine.
//!
//! # What is forbidden and why
//!
//! `base::send` owns three cross-cutting rules — credential redaction,
//! status-to-error-kind mapping, the bounded body excerpt. A provider that
//! dispatches or maps errors on its own forgets one half of that (`google.rs`
//! did, for years, which is why the funnel exists). The markers below are the
//! vocabulary of doing it yourself: `.send()` (dispatching without the
//! funnel), `.status()` / `StatusCode` / `error_for_status` (mapping a status
//! privately), `Client::builder` / `Client::new` (a client
//! `base::build_client` did not build).
//!
//! A legitimate exception — a case the funnel genuinely cannot carry — goes
//! in [`ALLOWANCES`] with a reason: a visible, reviewed edit rather than a
//! silent weakening of the guard. Empty today.

use super::capability_census::{production_view, provider_sources};

/// Markers of a provider dispatching a request or mapping an HTTP status
/// outside `base::send`. Comments and string/char literal payloads are
/// already gone from the scanned text (`production_view`), so these match
/// code only — a doc comment saying "we do not call `.status()`" cannot trip
/// the guard, and neither can this file's own prose.
const FORBIDDEN: &[&str] = &[
    ".send()",
    ".status()",
    "StatusCode",
    "error_for_status",
    "Client::builder",
    "Client::new",
];

/// Reviewed exceptions: `(provider, marker, reason)`. Each entry covers ONE
/// marker in ONE provider file and says why the funnel cannot carry that
/// case. Empty is the desired state — the table exists so that a future
/// exception is a deliberate, greppable decision rather than an uncommented
/// weakening. `every_allowance_is_load_bearing` keeps entries from rotting
/// after the code they covered is removed.
const ALLOWANCES: &[(&str, &str, &str)] = &[];

/// The negative half of the guard: no provider file carries the vocabulary
/// of doing its own dispatch or status mapping.
#[test]
fn no_provider_dispatches_or_maps_http_errors_outside_the_funnel() {
    for (name, src) in provider_sources() {
        let prod = production_view(src);
        for marker in FORBIDDEN {
            if !prod.contains(marker) {
                continue;
            }
            let covered = ALLOWANCES
                .iter()
                .any(|(provider, allowed, _)| *provider == name && *allowed == *marker);
            assert!(
                covered,
                "provider `{name}` contains `{marker}` — dispatching a request or mapping an \
                 HTTP status outside `base::send` bypasses the funnel's redaction and \
                 error-kind mapping. Route through `base::send`, or record why it cannot \
                 carry this case in `ALLOWANCES` with a reason."
            );
        }
    }
}

/// The positive half, and the guard's anti-vacuity assertion: a provider
/// file that did no HTTP at all would pass the negative half silently, so
/// every provider must actually call the funnel. The provider list itself is
/// pinned against the directory listing by `capability_census`'s
/// `the_census_sees_every_provider_file`, which is why this file reuses its
/// `provider_sources()` rather than keeping a second list.
#[test]
fn every_provider_actually_uses_the_funnel() {
    let providers = provider_sources();
    assert_eq!(
        providers.len(),
        9,
        "the census expects the nine providers capability_census enumerates; \
         if that list changed, change it there — not here"
    );
    for (name, src) in providers {
        let prod = production_view(src);
        assert!(
            prod.contains("send("),
            "provider `{name}` never calls `send(` — either it makes no HTTP calls at all \
             (and the negative half of this census is vacuously green for it), or it \
             dispatches through a spelling the forbidden markers do not cover. Both need a \
             look before this guard can be said to cover `{name}`."
        );
    }
}

/// An allowance that covers nothing is a weakening nobody can see anymore.
#[test]
fn every_allowance_is_load_bearing() {
    for (provider, marker, reason) in ALLOWANCES {
        let providers = provider_sources();
        let src = providers.get(provider).unwrap_or_else(|| {
            panic!("ALLOWANCES names `{provider}`, which is not a provider file")
        });
        assert!(
            production_view(src).contains(marker),
            "ALLOWANCES covers `{marker}` in `{provider}`, but the marker is gone — \
             delete the stale entry"
        );
        assert!(
            !reason.trim().is_empty(),
            "an allowance without a reason is just a hole: ({provider}, {marker})"
        );
    }
}
