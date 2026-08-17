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
//! There is no write path here on purpose: this banner only reads. Installing a
//! runtime is the Runtimes page's verb, and the link below is how a user gets
//! to it.

use crate::api::runtimes::{RuntimeInfo, RuntimeStatus, RuntimesApi};
use crate::components::admin_refusal;
use crate::context::DashboardState;
use crate::i18n::{t, t_string, use_i18n};
use leptos::prelude::*;
use leptos::task::spawn_local;

const BROWSER_RUNTIMES: &[&str] = &["fnm", "node", "playwright-cli"];

/// What the banner is entitled to say about the browser runtime.
#[derive(Debug, Clone, PartialEq, Eq)]
enum BannerState {
    /// The server answered and every browser runtime this OS supports is Ready.
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
fn banner_state(reply: Result<Vec<RuntimeInfo>, String>) -> BannerState {
    match reply {
        Err(err) => BannerState::Unknown(err),
        Ok(runtimes) => {
            let missing: Vec<String> = runtimes
                .iter()
                .filter(|r| {
                    BROWSER_RUNTIMES.contains(&r.name.as_str())
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

#[component]
#[must_use]
pub fn RuntimeSummaryBanner() -> impl IntoView {
    let state = expect_context::<DashboardState>();
    let i18n = use_i18n();
    // `None` = the call has not come back yet. Distinct from `Unknown`, which is
    // an answer ("there will not be one").
    let banner = RwSignal::new(None::<BannerState>);

    {
        spawn_local(async move {
            let reply = RuntimesApi::list(&state).await.map(|r| r.runtimes);
            banner.set(Some(banner_state(reply)));
        });
    }

    view! {
        {move || {
            // Still in flight: the banner says nothing rather than guessing.
            let state = banner.get()?;
            match state {
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
        let refused = banner_state(Err(ADMIN_REQUIRED_MESSAGE.to_string()));
        assert_eq!(
            refused,
            BannerState::Unknown(ADMIN_REQUIRED_MESSAGE.to_string())
        );
        assert_ne!(refused, banner_state(Ok(Vec::new())));
    }

    /// Not only the refusal: a transport failure and an unparseable response
    /// are equally not evidence of health. `RuntimesApi::list` folds both into
    /// `Err`, and every `Err` means the same thing here — no answer.
    #[test]
    fn every_failure_is_unknown_not_ready() {
        for err in ["Not connected", "invalid type: null, expected a sequence"] {
            assert_eq!(
                banner_state(Err(err.to_string())),
                BannerState::Unknown(err.to_string()),
                "`{err}` must not be read as a runtime verdict"
            );
        }
    }

    #[test]
    fn an_answered_list_still_reports_ready_and_missing() {
        assert_eq!(
            banner_state(Ok(vec![
                runtime("node", RuntimeStatus::Ready),
                runtime("playwright-cli", RuntimeStatus::Ready),
            ])),
            BannerState::Ready
        );
        assert_eq!(
            banner_state(Ok(vec![
                runtime("node", RuntimeStatus::Ready),
                runtime("playwright-cli", RuntimeStatus::Missing),
            ])),
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
            banner_state(Ok(vec![unsupported, runtime("uv", RuntimeStatus::Missing)])),
            BannerState::Ready
        );
    }
}
