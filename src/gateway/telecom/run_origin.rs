//! The telecom origin of the run currently executing, as a task-local.
//!
//! `RunRequest.telecom_public` is the admitted fact; this module makes it
//! readable by the tools that *create* autonomous work (`goal set`,
//! `loop start`) and by `session_send` (which copies it onto the delegated
//! child's typed `RunRequest.telecom_public`) without ever taking it from a
//! model-writable argument. The creator's origin is then stored with the goal
//! / loop and is what every later autonomous run (hook continuation, loop tick,
//! wake) restores — never the origin of whichever unrelated turn happened to
//! complete last.
//!
//! Rules
//! - [`seed_run_origin`] has exactly TWO production callers, each seeding from
//!   a request's own typed `telecom_public`: the gateway-side
//!   `execution_engine::run_loop::with_request_scope`, and the harness-side
//!   `orchestrator::dispatch` spawn (a `tokio::spawn` drops the task-local, so
//!   the harness task re-seeds it from `FlowRequest::telecom_public`). A census
//!   test pins this.
//! - [`CarriedAttribution`](crate::scope::CarriedAttribution) is the only user
//!   of [`capture_carry`] / [`reestablish_carry`], so every spawn that already
//!   carries scope / author carries the origin too.
//! - A bare `tokio::spawn` loses the task-local (same class as scope and room
//!   author). A reader then sees "unseeded", which is reported as `None`
//!   (ordinary). That is a known gap for spawns that do not use the carrier;
//!   see the "Not done" list in `task-4-sessions-provenance-report.md` and the
//!   harness-spawn fix in `task-4-harness-origin-report.md`.

use super::TelecomPublicContext;

#[derive(Clone, Debug, PartialEq, Eq)]
enum RunOrigin {
    Ordinary,
    Public(TelecomPublicContext),
}

tokio::task_local! {
    static RUN_ORIGIN: RunOrigin;
}

/// Run `fut` with this run's origin visible to [`current_run_origin`].
/// `None` is seeded explicitly as "ordinary run" (distinct from unseeded).
pub(crate) async fn seed_run_origin<F, T>(ctx: Option<TelecomPublicContext>, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    let origin = match ctx {
        Some(c) => RunOrigin::Public(c),
        None => RunOrigin::Ordinary,
    };
    // Boxed: this wraps the whole `run_agent_loop` future, and nesting one more
    // task-local combinator inline overflows the debug-build test-thread stack
    // (same hazard as `CarriedAttribution::reestablish`).
    RUN_ORIGIN.scope(origin, Box::pin(fut)).await
}

/// The public context of the run this code executes under, if any. Ordinary
/// and unseeded both read `None`.
#[must_use]
pub(crate) fn current_run_origin() -> Option<TelecomPublicContext> {
    RUN_ORIGIN
        .try_with(|o| match o {
            RunOrigin::Public(c) => Some(c.clone()),
            RunOrigin::Ordinary => None,
        })
        .ok()
        .flatten()
}

/// Opaque spawn carrier; produced only by [`capture_carry`].
#[derive(Clone, Default)]
pub(crate) struct CarriedRunOrigin(Option<RunOrigin>);

/// Snapshot for a spawn. Call before `tokio::spawn`.
#[must_use]
pub(crate) fn capture_carry() -> CarriedRunOrigin {
    CarriedRunOrigin(RUN_ORIGIN.try_with(Clone::clone).ok())
}

/// Re-establish a snapshot inside the spawned task. An unseeded snapshot stays
/// unseeded (never upgraded to "ordinary").
pub(crate) async fn reestablish_carry<F, T>(carry: CarriedRunOrigin, fut: F) -> T
where
    F: std::future::Future<Output = T>,
{
    match carry.0 {
        Some(o) => RUN_ORIGIN.scope(o, fut).await,
        None => fut.await,
    }
}

/// Test fixture: a public SMS context for `binding`.
#[cfg(test)]
pub(crate) fn sample_public_context(binding: &str) -> TelecomPublicContext {
    TelecomPublicContext::from_admitted_sms(&super::SmsInboundMeta {
        region: "r".into(),
        binding_ref: binding.into(),
        provider_account_id: "a".into(),
        provider_message_id: "m".into(),
        envelope_event_id: "e".into(),
        connection_generation: 3,
        service_address: "+15550001111".into(),
        peer_address: "+15552223333".into(),
        encoding: aleph_protocol::telecom_relay::SmsEncoding::Gsm7,
        segment_count: 1,
        created_at: chrono::Utc::now(),
        expires_at: chrono::Utc::now(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn unseeded_ordinary_and_public_read_as_expected() {
        assert_eq!(current_run_origin(), None);
        assert_eq!(
            seed_run_origin(None, async { current_run_origin() }).await,
            None
        );
        let ctx = sample_public_context("b-1");
        assert_eq!(
            seed_run_origin(Some(ctx.clone()), async { current_run_origin() }).await,
            Some(ctx)
        );
    }

    #[tokio::test]
    async fn a_bare_spawn_loses_it_and_the_carrier_restores_it() {
        let ctx = sample_public_context("b-1");
        let (lost, carried) = seed_run_origin(Some(ctx.clone()), async {
            let lost = tokio::spawn(async { current_run_origin() }).await.unwrap();
            let carry = capture_carry();
            let carried = tokio::spawn(reestablish_carry(carry, async { current_run_origin() }))
                .await
                .unwrap();
            (lost, carried)
        })
        .await;
        assert_eq!(lost, None);
        assert_eq!(carried, Some(ctx));
    }

    #[tokio::test]
    async fn carrying_an_unseeded_run_does_not_invent_an_origin() {
        let carry = capture_carry();
        let seen = tokio::spawn(reestablish_carry(carry, async {
            RUN_ORIGIN.try_with(|_| ()).is_ok()
        }))
        .await
        .unwrap();
        assert!(!seen, "unseeded must stay unseeded across a carrier");
    }

    #[tokio::test]
    async fn captured_attribution_carries_the_origin_across_a_spawn() {
        let ctx = sample_public_context("b-2");
        let got = seed_run_origin(Some(ctx.clone()), async {
            let carrier = crate::scope::CarriedAttribution::capture();
            tokio::spawn(carrier.reestablish(async { current_run_origin() }))
                .await
                .unwrap()
        })
        .await;
        assert_eq!(got, Some(ctx));
    }

    /// Production files (text before the first `#[cfg(test)]`, comment lines
    /// dropped) under `src/` that contain `needle`, excluding `skip_suffix`.
    pub(crate) fn production_files_containing(needle: &str, skip_suffix: &str) -> Vec<String> {
        fn walk(dir: &std::path::Path, needle: &str, skip: &str, hits: &mut Vec<String>) {
            for e in std::fs::read_dir(dir).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, needle, skip, hits);
                } else if p.extension().is_some_and(|x| x == "rs")
                    && !p.display().to_string().ends_with(skip)
                {
                    let s = std::fs::read_to_string(&p).unwrap();
                    // Declared-test-module files (`tests.rs`, census modules)
                    // yield "" here; the rest drop `#[cfg(test)]` items.
                    let prod = crate::utils::source_scan::production_text(&p, &s);
                    if prod
                        .lines()
                        .any(|l| !l.trim_start().starts_with("//") && l.contains(needle))
                    {
                        hits.push(p.display().to_string());
                    }
                }
            }
        }
        let mut hits = Vec::new();
        walk(
            &std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("src"),
            needle,
            skip_suffix,
            &mut hits,
        );
        hits.sort();
        hits
    }

    #[test]
    fn seed_run_origin_has_exactly_two_production_callers() {
        let hits = production_files_containing("seed_run_origin(", "telecom/run_origin.rs");
        assert_eq!(hits.len(), 2, "{hits:?}");
        assert!(hits[0].ends_with("run_loop/mod.rs"), "{hits:?}");
        assert!(hits[1].ends_with("orchestrator/dispatch.rs"), "{hits:?}");
    }

    #[test]
    fn capture_carry_and_reestablish_carry_are_used_only_by_the_carrier() {
        for needle in ["capture_carry(", "reestablish_carry("] {
            let hits = production_files_containing(needle, "telecom/run_origin.rs");
            assert_eq!(hits.len(), 1, "{needle}: {hits:?}");
            assert!(hits[0].ends_with("scope/carried.rs"), "{hits:?}");
        }
    }

    #[test]
    fn origin_writers_have_exactly_one_production_caller_each() {
        let goal = production_files_containing(".put_with_origin(", "goal/store.rs");
        assert_eq!(goal.len(), 1, "{goal:?}");
        assert!(goal[0].ends_with("builtin_tools/goal.rs"), "{goal:?}");
        let lp = production_files_containing(".with_telecom_origin(", "looping/types.rs");
        assert_eq!(lp.len(), 1, "{lp:?}");
        assert!(lp[0].ends_with("builtin_tools/loop_manage.rs"), "{lp:?}");
    }

    #[test]
    fn the_run_hook_reads_creator_origin_not_the_completing_requests() {
        let src = include_str!("../execution_engine/execute.rs");
        let prod = crate::utils::source_scan::production_prefix(src);
        let hook_reads: Vec<&str> = prod
            .lines()
            .filter(|l| !l.trim_start().starts_with("//"))
            .filter(|l| l.contains("request.telecom_public"))
            .collect();
        assert!(hook_reads.is_empty(), "{hook_reads:?}");
    }
}
