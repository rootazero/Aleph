//! Compact runtime-readiness banner shown at the top of the Browser page.
//!
//! Keeps the Browser config page focused on configuration while giving
//! visibility into whether the underlying runtime is installed.
//!
//! # Only an `Ok` may claim health
//!
//! `runtimes.list` is admin-gated (`method_admin.rs`), so for a member it comes
//! back refused — and this banner used to fold every `Err` into an untouched,
//! empty runtime list, whose "nothing is missing" reading painted a green
//! READY. That is the first row of [`admin_refusal`]'s own table: a refused
//! read consumed as a VALUE, and the expensive direction of it, because a
//! confident false claim about a runtime the user does not have costs more than
//! a blank.
//!
//! So the state machine has three states, not two: [`BannerState::Unknown`]
//! exists precisely so that "I could not find out" has somewhere to go that is
//! not "ready". Every failure mode lands there — refusal, disconnect, and a
//! response this build cannot parse are all things the banner does not know the
//! answer to, and only the refusal gets a permission explanation
//! ([`admin_refusal::settings_load_error`] passes the rest through with the call
//! site's own framing).
//!
//! # Which runtimes count depends on the DRIVER
//!
//! The list used to be a flat literal, `["fnm", "node", "playwright-cli"]` —
//! the playwright chain, which was the browser stack when it was written. It no
//! longer is: `BrowserDriver::default()` is `cdp` and `Engine::default()` is
//! obscura, an entirely different runtime. The literal was therefore wrong in
//! **both** directions at once, which is what made it worth fixing rather than
//! deciding:
//!
//! * on a correct default install (obscura present, playwright chain absent) it
//!   painted a warning naming three runtimes that install will never invoke —
//!   and a guard that cries wolf is read as "the browser is broken";
//! * on a broken one (playwright chain present, obscura absent) it painted
//!   READY over a `browser_open` that cannot launch anything.
//!
//! So the requirement is derived from the driver the operator has selected, and
//! the caller passes it in: [`required_runtimes`] is the table, and `None` from
//! it means "this build does not know what that driver needs" — which renders
//! nothing rather than a verdict (判据 §8).
//!
//! There is no write path here on purpose: this banner only reads. Installing a
//! runtime is the Runtimes page's verb, and the link below is how a user gets
//! to it.

use crate::api::runtimes::{RuntimeInfo, RuntimeStatus, RuntimesApi};
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

/// The `cdp` driver's chain: one engine binary, installed from Aleph's own
/// runtime ledger.
///
/// The name is `aleph_protocol`'s, not a literal: it has to be the string the
/// server puts in `runtimes.list`, and a private copy here would filter nothing
/// and report READY — the very defect this list is being fixed for, restored
/// silently (判据 §10). `alephcore::runtimes::specs::OBSCURA_RUNTIME` is
/// defined as the same constant, so there is one author.
const CDP_RUNTIMES: &[&str] = &[aleph_protocol::browser::OBSCURA_RUNTIME_WIRE];

/// The `managed` driver's chain: `playwright-cli`, which runs on node, which
/// fnm provides.
const MANAGED_RUNTIMES: &[&str] = &["fnm", "node", "playwright-cli"];

/// What the browser needs installed, given the driver this install is set to.
///
/// `None` is "this build does not know", and it is reachable: the wire
/// vocabulary is the server's (`aleph_protocol::browser::BROWSER_DRIVER_WIRE`),
/// so a driver added there arrives here as a value this `match` has no arm for.
/// Answering `&[]` for it would read as "nothing is missing", which is a
/// verdict this build has no basis for.
fn required_runtimes(driver: &str) -> Option<&'static [&'static str]> {
    match driver {
        "cdp" => Some(CDP_RUNTIMES),
        "managed" => Some(MANAGED_RUNTIMES),
        // The user's own Chrome, attached to rather than launched. Aleph
        // installs nothing for it, so there is nothing this banner can find
        // missing — an empty requirement, not an unknown one.
        "existing_session" => Some(&[]),
        _ => None,
    }
}

/// What the banner is entitled to say about the browser runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BannerState {
    /// The server answered and every runtime this driver needs on this OS is
    /// Ready.
    Ready,
    /// The server answered and these runtimes are not Ready.
    Missing(Vec<String>),
    /// No answer arrived. Carries the server's error verbatim — classified into
    /// user copy at render time, never here, so this stays testable without an
    /// i18n context.
    Unknown(String),
}

/// Derive the banner's state from the `runtimes.list` reply.
///
/// The whole point is the `Err` arm: an empty list and a refused call are not
/// the same fact, and only the former is evidence of anything.
///
/// `required` is a PARAMETER rather than read off a module constant, so both
/// chains can be driven from a test. A branch whose input is a `const` can only
/// ever be exercised in whatever configuration the constant happens to name.
fn banner_state(reply: Result<Vec<RuntimeInfo>, String>, required: &[&str]) -> BannerState {
    match reply {
        Err(err) => BannerState::Unknown(err),
        Ok(runtimes) => {
            let missing: Vec<String> = runtimes
                .iter()
                .filter(|r| {
                    required.contains(&r.name.as_str())
                        && r.status != RuntimeStatus::Ready
                        && r.supported_on_current_os
                })
                .map(|r| r.name.clone())
                .collect();
            if missing.is_empty() {
                BannerState::Ready
            } else {
                BannerState::Missing(missing)
            }
        }
    }
}

/// `driver` is the `default_driver` this install is configured with, or `None`
/// while the Browser page has not loaded it (or could not). The banner says
/// nothing until it knows — "which runtimes does this install need" has no
/// answer without it, and the placeholder the page starts with is not one.
#[component]
#[must_use]
pub fn RuntimeSummaryBanner(driver: Signal<Option<String>>) -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    // `None` = the call has not come back yet. Distinct from `Unknown`, which is
    // an answer ("there will not be one"). The raw reply is held rather than a
    // derived state, because the state also depends on `driver`, which can
    // arrive after this does.
    let reply = RwSignal::new(None::<Result<Vec<RuntimeInfo>, String>>);

    {
        spawn_local(async move {
            reply.set(Some(RuntimesApi::list(&state).await.map(|r| r.runtimes)));
        });
    }

    view! {
        {move || {
            // Still in flight, or the page does not yet know its driver: the
            // banner says nothing rather than guessing.
            let answered = reply.get()?;
            let required = required_runtimes(&driver.get()?)?;
            match banner_state(answered, required) {
                BannerState::Ready => {
                    Some(view! {
                        <div class="p-3 bg-success-subtle border border-success/20 rounded-lg text-success text-sm flex items-center gap-2">
                            <span>"✓"</span>
                            <span>{t!(i18n, browser_banner.ready)}</span>
                        </div>
                    }.into_any())
                }
                BannerState::Missing(missing) => {
                    let names = missing.join(", ");
                    Some(view! {
                        <div class="p-3 bg-warning-subtle border border-warning/20 rounded-lg text-warning text-sm flex items-center justify-between gap-2">
                            <span>{format!("{}{names}", t_string!(i18n, browser_banner.missing_prefix))}</span>
                            <a href="/dashboard/runtimes"
                               class="text-sm font-medium underline hover:no-underline">
                                {t!(i18n, browser_banner.configure)}
                            </a>
                        </div>
                    }.into_any())
                }
                BannerState::Unknown(err) => {
                    // Informational, never green and never a missing-runtime
                    // claim: the honest content of this state is the server's
                    // reason, localized for the one reason the Panel can name.
                    let explained = admin_refusal::settings_load_error(
                        i18n,
                        &err,
                        |e| format!("Failed to load runtime status: {e}"),
                    );
                    Some(view! {
                        <div class="p-3 bg-info-subtle border border-info/20 rounded-lg text-info text-sm flex items-center justify-between gap-2">
                            <span>{explained}</span>
                            <a href="/dashboard/runtimes"
                               class="text-sm font-medium underline hover:no-underline">
                                {t!(i18n, browser_banner.configure)}
                            </a>
                        </div>
                    }.into_any())
                }
            }
        }}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use aleph_protocol::jsonrpc::ADMIN_REQUIRED_MESSAGE;

    fn runtime(name: &str, status: RuntimeStatus) -> RuntimeInfo {
        RuntimeInfo {
            name: name.to_string(),
            status,
            bin_path: None,
            version: None,
            llm_hint: None,
            deps: Vec::new(),
            supported_on_current_os: true,
        }
    }

    /// The defect this file was rewritten for. A refused read and an empty
    /// answer used to reach the same expression — `missing.is_empty()` over a
    /// list that had never been filled — and the empty answer's verdict is
    /// `Ready`. They must not be the same state.
    #[test]
    fn a_refused_read_is_not_a_ready_runtime() {
        let refused = banner_state(Err(ADMIN_REQUIRED_MESSAGE.to_string()), MANAGED_RUNTIMES);
        assert_eq!(
            refused,
            BannerState::Unknown(ADMIN_REQUIRED_MESSAGE.to_string())
        );
        assert_ne!(refused, banner_state(Ok(Vec::new()), MANAGED_RUNTIMES));
    }

    /// Not only the refusal: a transport failure and an unparseable response
    /// are equally not evidence of health. `RuntimesApi::list` folds both into
    /// `Err`, and every `Err` means the same thing here — no answer.
    #[test]
    fn every_failure_is_unknown_not_ready() {
        for err in ["Not connected", "invalid type: null, expected a sequence"] {
            assert_eq!(
                banner_state(Err(err.to_string()), MANAGED_RUNTIMES),
                BannerState::Unknown(err.to_string()),
                "`{err}` must not be read as a runtime verdict"
            );
        }
    }

    #[test]
    fn an_answered_list_still_reports_ready_and_missing() {
        assert_eq!(
            banner_state(
                Ok(vec![
                    runtime("node", RuntimeStatus::Ready),
                    runtime("playwright-cli", RuntimeStatus::Ready),
                ]),
                MANAGED_RUNTIMES
            ),
            BannerState::Ready
        );
        assert_eq!(
            banner_state(
                Ok(vec![
                    runtime("node", RuntimeStatus::Ready),
                    runtime("playwright-cli", RuntimeStatus::Missing),
                ]),
                MANAGED_RUNTIMES
            ),
            BannerState::Missing(vec!["playwright-cli".to_string()])
        );
    }

    /// A runtime that cannot exist on this OS is not missing, and a runtime
    /// outside the browser chain is not this banner's business.
    #[test]
    fn unsupported_and_unrelated_runtimes_are_ignored() {
        let mut unsupported = runtime("playwright-cli", RuntimeStatus::Missing);
        unsupported.supported_on_current_os = false;
        assert_eq!(
            banner_state(
                Ok(vec![unsupported, runtime("uv", RuntimeStatus::Missing)]),
                MANAGED_RUNTIMES
            ),
            BannerState::Ready
        );
    }

    /// The defect the dual-engine round found, in both of its directions. The
    /// same machine reads the opposite way under the two drivers, which is
    /// exactly why one flat list could not be right.
    #[test]
    fn the_default_driver_reads_obscura_and_not_the_playwright_chain() {
        // A correct default install: the engine is there, the playwright chain
        // is not. The flat list called this "Missing: fnm, node,
        // playwright-cli" on a browser that works.
        let correct_default = vec![
            runtime(
                aleph_protocol::browser::OBSCURA_RUNTIME_WIRE,
                RuntimeStatus::Ready,
            ),
            runtime("fnm", RuntimeStatus::Missing),
            runtime("node", RuntimeStatus::Missing),
            runtime("playwright-cli", RuntimeStatus::Missing),
        ];
        assert_eq!(
            banner_state(
                Ok(correct_default.clone()),
                required_runtimes("cdp").unwrap()
            ),
            BannerState::Ready
        );

        // …and the same list under `managed`, where those three ARE the chain.
        assert_eq!(
            banner_state(Ok(correct_default), required_runtimes("managed").unwrap()),
            BannerState::Missing(vec![
                "fnm".to_string(),
                "node".to_string(),
                "playwright-cli".to_string()
            ])
        );

        // The other direction: the playwright chain installed, no engine. The
        // flat list painted this READY over a `browser_open` that cannot launch
        // anything on the default driver.
        let broken_default = vec![
            runtime(
                aleph_protocol::browser::OBSCURA_RUNTIME_WIRE,
                RuntimeStatus::Missing,
            ),
            runtime("fnm", RuntimeStatus::Ready),
            runtime("node", RuntimeStatus::Ready),
            runtime("playwright-cli", RuntimeStatus::Ready),
        ];
        assert_eq!(
            banner_state(
                Ok(broken_default.clone()),
                required_runtimes("cdp").unwrap()
            ),
            BannerState::Missing(vec![
                aleph_protocol::browser::OBSCURA_RUNTIME_WIRE.to_string()
            ])
        );
        assert_eq!(
            banner_state(Ok(broken_default), required_runtimes("managed").unwrap()),
            BannerState::Ready
        );
    }

    /// Every driver the server can report must have a requirement this build
    /// can name. A driver added upstream arrives here as a string with no arm,
    /// and `None` is what keeps that from rendering as a verdict — but it is
    /// also a banner that has gone silent, so the addition has to be noticed
    /// here rather than in the field (判据 §5: a list only covers the world as
    /// it was on legislation day).
    #[test]
    fn every_wire_driver_has_a_known_requirement() {
        for wire in aleph_protocol::browser::BROWSER_DRIVER_WIRE {
            assert!(
                required_runtimes(wire).is_some(),
                "driver {wire:?} is in the server's wire vocabulary but this \
                 banner does not know what it needs installed — it would render \
                 nothing at all on an install configured that way"
            );
        }
        // …and the fallback is genuinely reachable, so the assertion above is
        // not a 恒真 predicate (判据 §2).
        assert!(required_runtimes("webdriver-bidi").is_none());
    }
}
