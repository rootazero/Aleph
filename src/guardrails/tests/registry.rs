//! `GuardrailRegistry` semantics + `disable_all` kill-switch.

use crate::sync_primitives::Arc;

use async_trait::async_trait;
use serde_json::Value;

use crate::error::ErrorClass;
use crate::guardrails::decision::GuardrailDecision;
use crate::guardrails::registry::GuardrailRegistry;
use crate::guardrails::traits::{InputGuardrail, OutputGuardrail, ToolCallGuardrail};

struct AllowAll;
#[async_trait]
impl InputGuardrail for AllowAll {
    fn name(&self) -> &str {
        "allow_all"
    }
    async fn evaluate_input(&self, _text: &str) -> GuardrailDecision {
        GuardrailDecision::Allow
    }
}
#[async_trait]
impl OutputGuardrail for AllowAll {
    fn name(&self) -> &str {
        "allow_all"
    }
    async fn evaluate_output(&self, _text: &str) -> GuardrailDecision {
        GuardrailDecision::Allow
    }
}
#[async_trait]
impl ToolCallGuardrail for AllowAll {
    fn name(&self) -> &str {
        "allow_all"
    }
    async fn evaluate_tool_call(&self, _name: &str, _args: &Value) -> GuardrailDecision {
        GuardrailDecision::Allow
    }
}

struct AlwaysBlock;
#[async_trait]
impl InputGuardrail for AlwaysBlock {
    fn name(&self) -> &str {
        "always_block"
    }
    async fn evaluate_input(&self, _text: &str) -> GuardrailDecision {
        GuardrailDecision::Block {
            reason: "blocked".into(),
            class: ErrorClass::Fixable,
        }
    }
}
#[async_trait]
impl OutputGuardrail for AlwaysBlock {
    fn name(&self) -> &str {
        "always_block"
    }
    async fn evaluate_output(&self, _text: &str) -> GuardrailDecision {
        GuardrailDecision::Block {
            reason: "blocked".into(),
            class: ErrorClass::Fixable,
        }
    }
}
#[async_trait]
impl ToolCallGuardrail for AlwaysBlock {
    fn name(&self) -> &str {
        "always_block"
    }
    async fn evaluate_tool_call(&self, _name: &str, _args: &Value) -> GuardrailDecision {
        GuardrailDecision::Block {
            reason: "blocked".into(),
            class: ErrorClass::Fixable,
        }
    }
}

#[tokio::test]
async fn empty_registry_allows_everything() {
    let r = GuardrailRegistry::builder().build();
    assert!(r.evaluate_input("anything").await.is_allow());
    assert!(r.evaluate_output("anything").await.is_allow());
    assert!(r
        .evaluate_tool_call("t", &serde_json::json!({}))
        .await
        .is_allow());
}

#[tokio::test]
async fn block_guardrail_blocks_all_three_surfaces() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AlwaysBlock))
        .with_output(Arc::new(AlwaysBlock))
        .with_tool_call(Arc::new(AlwaysBlock))
        .build();
    assert!(r.evaluate_input("x").await.is_block());
    assert!(r.evaluate_output("x").await.is_block());
    assert!(r
        .evaluate_tool_call("t", &serde_json::json!({}))
        .await
        .is_block());
}

#[tokio::test]
async fn disable_all_short_circuits_to_allow() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AlwaysBlock))
        .with_output(Arc::new(AlwaysBlock))
        .with_tool_call(Arc::new(AlwaysBlock))
        .build();
    r.disable_all();
    assert!(!r.is_enabled());
    assert!(r.evaluate_input("x").await.is_allow());
    assert!(r.evaluate_output("x").await.is_allow());
    assert!(r
        .evaluate_tool_call("t", &serde_json::json!({}))
        .await
        .is_allow());

    r.enable_all();
    assert!(r.is_enabled());
    assert!(r.evaluate_input("x").await.is_block());
}

#[tokio::test]
async fn first_non_allow_wins_when_multiple_registered() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AllowAll))
        .with_input(Arc::new(AlwaysBlock))
        .build();
    let d = r.evaluate_input("x").await;
    assert!(d.is_block());
}

#[tokio::test]
async fn counts_match_registered_guardrails() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AllowAll))
        .with_input(Arc::new(AlwaysBlock))
        .with_output(Arc::new(AllowAll))
        .with_tool_call(Arc::new(AllowAll))
        .with_tool_call(Arc::new(AllowAll))
        .with_tool_call(Arc::new(AllowAll))
        .build();
    assert_eq!(r.input_count(), 2);
    assert_eq!(r.output_count(), 1);
    assert_eq!(r.tool_call_count(), 3);
}

/// Concurrency hammer: spawn N tasks each calling `evaluate_tool_call` in a
/// loop while a sibling task flips `disable_all` / `enable_all`. The contract
/// being tested is the sequential consistency of the AtomicBool kill-switch:
/// every individual call must observe a coherent on/off state and produce
/// either an `Allow` (registry off OR no guardrails fired) or a `Block` (an
/// `AlwaysBlock` guardrail fired while registry was on). No call should
/// panic, deadlock, or return an inconsistent decision.
///
/// This stands in for a `loom` model test. The repository's loom feature is
/// wired into `sync_primitives`, but loom models only single-thread
/// pseudo-random scheduling; a tokio hammer with high task count gives more
/// practical coverage for a single-AtomicBool flag at minimal cost.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_evaluate_vs_disable_all_is_consistent() {
    use crate::sync_primitives::{AtomicUsize, Ordering};
    let registry = Arc::new(
        GuardrailRegistry::builder()
            .with_tool_call(Arc::new(AlwaysBlock))
            .build(),
    );

    const READERS: usize = 16;
    const ITERS: usize = 200;
    let blocks = Arc::new(AtomicUsize::new(0));
    let allows = Arc::new(AtomicUsize::new(0));

    let toggler = {
        let registry = registry.clone();
        tokio::spawn(async move {
            for i in 0..(READERS * ITERS) {
                if i % 2 == 0 {
                    registry.disable_all();
                } else {
                    registry.enable_all();
                }
                // Yield so readers actually get scheduled between toggles —
                // without this the toggler can run to completion in one slot
                // and the test degenerates into sequential execution.
                if i % 8 == 0 {
                    tokio::task::yield_now().await;
                }
            }
        })
    };

    let mut readers = Vec::with_capacity(READERS);
    for _ in 0..READERS {
        let registry = registry.clone();
        let blocks = blocks.clone();
        let allows = allows.clone();
        readers.push(tokio::spawn(async move {
            for _ in 0..ITERS {
                let d = registry
                    .evaluate_tool_call("any", &serde_json::json!({"x": 1}))
                    .await;
                assert!(
                    d.is_block() || d.is_allow(),
                    "unexpected decision under concurrency: {d:?}"
                );
                match d {
                    GuardrailDecision::Block { .. } => {
                        blocks.fetch_add(1, Ordering::Relaxed);
                    }
                    GuardrailDecision::Allow => {
                        allows.fetch_add(1, Ordering::Relaxed);
                    }
                    _ => {}
                }
            }
        }));
    }

    for r in readers {
        let result = r.await;
        assert!(result.is_ok(), "reader task failed: {result:?}");
    }
    let result = toggler.await;
    assert!(result.is_ok(), "toggler task failed: {result:?}");

    let total = blocks.load(Ordering::Relaxed) + allows.load(Ordering::Relaxed);
    assert_eq!(total, READERS * ITERS, "every call must produce a decision");
    // The contract being tested is sequential consistency of the
    // AtomicBool — every call returned a coherent decision and no task
    // panicked or deadlocked. We do NOT assert that both arms fired
    // because the scheduler may run the toggler to completion before any
    // readers schedule (especially on a busy CI runner).
}

// -- screen_session_input ----------------------------------------------------

use crate::guardrails::registry::SessionInputScreen;
use crate::session::events::{now_ms, MessageContent, SessionEvent, SessionEventRecord, TurnId};

struct SanitizeSecret;
#[async_trait]
impl InputGuardrail for SanitizeSecret {
    fn name(&self) -> &str {
        "sanitize_secret"
    }
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        if text.contains("SECRET") {
            GuardrailDecision::Sanitize(crate::guardrails::decision::Replacement {
                text: text.replace("SECRET", "[X]"),
                source: "test".into(),
            })
        } else {
            GuardrailDecision::Allow
        }
    }
}

fn rec(seq: u64, text: &str, synthetic: bool) -> SessionEventRecord {
    SessionEventRecord {
        seq,
        event: SessionEvent::UserMessage {
            turn_id: TurnId::new_v4(),
            content: MessageContent {
                text: text.to_string(),
                blocks: Vec::new(),
                thinking: None,
                thinking_signature: None,
            },
            at: now_ms(),
            synthetic,
            author_user_id: None,
        },
        created_at_ms: now_ms(),
    }
}

/// The compaction summary a split child session is rebuilt from.
fn summary_rec(seq: u64, text: &str) -> SessionEventRecord {
    SessionEventRecord {
        seq,
        event: SessionEvent::SystemMessage {
            turn_id: TurnId::new_v4(),
            content: text.to_string(),
            at: now_ms(),
        },
        created_at_ms: now_ms(),
    }
}

fn texts(events: &[SessionEventRecord]) -> Vec<String> {
    events
        .iter()
        .map(|r| match &r.event {
            SessionEvent::UserMessage { content, .. } => content.text.clone(),
            SessionEvent::SystemMessage { content, .. } => content.clone(),
            _ => String::new(),
        })
        .collect()
}

#[tokio::test]
async fn screen_rewrites_every_replayed_user_message_not_just_the_tail() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(SanitizeSecret))
        .build();
    // tail_start = 2: only the last message belongs to this turn, yet the
    // prompt replays all three.
    let events = vec![
        rec(1, "the password is SECRET", false),
        rec(2, "and the key is SECRET too", false),
        rec(3, "what did I say?", false),
    ];
    let SessionInputScreen::Pass(out) = r.screen_session_input(events, 2).await else {
        panic!("expected Pass");
    };
    assert!(
        texts(&out).iter().all(|t| !t.contains("SECRET")),
        "history must be sanitized too, got {:?}",
        texts(&out),
    );
}

#[tokio::test]
async fn screen_covers_the_compaction_summary_a_split_child_is_rebuilt_from() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(SanitizeSecret))
        .build();
    // A split child's head: the summary restates the user messages that were
    // compacted away, so the secret survives in it even after the originals are
    // gone. Screening only UserMessage would put it straight back on the wire.
    let events = vec![
        summary_rec(1, "[Context Summary] the user said the password is SECRET"),
        rec(2, "carry on", false),
    ];
    let SessionInputScreen::Pass(out) = r.screen_session_input(events, 1).await else {
        panic!("expected Pass");
    };
    assert!(
        !texts(&out)[0].contains("SECRET"),
        "the summary must be sanitized, got {:?}",
        texts(&out)[0],
    );
}

#[tokio::test]
async fn a_block_on_the_summary_redacts_it_instead_of_ending_the_turn() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AlwaysBlock))
        .build();
    // The summary is never "the message this turn is answering", so it can only
    // ever be redacted. If it could block, every turn of a split child session
    // would end immediately and the session would be unusable.
    let events = vec![summary_rec(1, "[Context Summary] whatever")];
    let SessionInputScreen::Pass(out) = r.screen_session_input(events, 1).await else {
        panic!("a block on the summary must not end the turn");
    };
    // See the seq-suffix note in `screen_blocks_only_this_turns_message_...`
    // — every redacted message in the post-`C1` registry carries a
    // `[redacted:seq=N]` suffix for audit-trail correlation.
    let expected_redacted = format!(
        "{} [redacted:seq={}]",
        crate::thinker::nudges::REDACTED_USER_MESSAGE,
        1
    );
    assert_eq!(texts(&out), vec![expected_redacted],);
}

#[tokio::test]
async fn screen_blocks_only_this_turns_message_and_redacts_older_ones() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AlwaysBlock))
        .build();
    // The turn's own message (index 1) blocks.
    let events = vec![rec(1, "old", false), rec(2, "new", false)];
    assert!(matches!(
        r.screen_session_input(events, 1).await,
        SessionInputScreen::Blocked(_)
    ));

    // A block on a replayed message cannot end the turn — it is redacted, and
    // the turn (whose own message index 1 is synthetic, hence never screened)
    // still runs. Re-blocking on every rebuild would brick the session.
    let events = vec![rec(1, "old", false), rec(2, "harness nudge", true)];
    let SessionInputScreen::Pass(out) = r.screen_session_input(events, 1).await else {
        panic!("a block on a replayed message must not end the turn");
    };
    // The redacted message now carries a `[redacted:seq=N]` suffix so the
    // audit trail can correlate the redaction with the event seq that
    // held the original text. This is the post-`C1` format; the older
    // test (which asserted on the bare constant) predates the suffix.
    let expected_redacted = format!(
        "{} [redacted:seq={}]",
        crate::thinker::nudges::REDACTED_USER_MESSAGE,
        1
    );
    assert_eq!(
        texts(&out),
        vec![expected_redacted, "harness nudge".to_string(),],
        "the replayed message is redacted; the synthetic one is left alone",
    );
}

#[tokio::test]
async fn redact_session_input_never_blocks() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(AlwaysBlock))
        .build();
    // The grace turn's face: everything is redacted, nothing ends the turn.
    // Each redacted message starts with the canonical redaction constant
    // and gains a `[redacted:seq=N]` audit-trail suffix (see the
    // seq-suffix note in `screen_blocks_only_this_turns_message_...`).
    let out = r
        .redact_session_input(vec![rec(1, "old", false), rec(2, "new", false)])
        .await;
    let prefix = crate::thinker::nudges::REDACTED_USER_MESSAGE;
    assert!(
        texts(&out).iter().all(|t| t.starts_with(prefix)),
        "every redacted message must carry the canonical prefix; got {:?}",
        texts(&out)
    );
}

// ---------------------------------------------------------------------------
// Twin parity: a warn-free rewrite must survive on all three surfaces
// ---------------------------------------------------------------------------

/// The shape every real redaction guardrail has: it rewrites and reports
/// nothing. A PII scrubber does not "warn" — warning about a secret is one way
/// to leak it into the audit log. So this is the *common* case, not an edge one.
struct SilentScrubber;
#[async_trait]
impl InputGuardrail for SilentScrubber {
    fn name(&self) -> &str {
        "silent_scrubber"
    }
    async fn evaluate_input(&self, text: &str) -> GuardrailDecision {
        scrub(text)
    }
}
#[async_trait]
impl OutputGuardrail for SilentScrubber {
    fn name(&self) -> &str {
        "silent_scrubber"
    }
    async fn evaluate_output(&self, text: &str) -> GuardrailDecision {
        scrub(text)
    }
}
#[async_trait]
impl ToolCallGuardrail for SilentScrubber {
    fn name(&self) -> &str {
        "silent_scrubber"
    }
    async fn evaluate_tool_call(&self, _tool: &str, args: &Value) -> GuardrailDecision {
        scrub(&args.to_string())
    }
}
fn scrub(text: &str) -> GuardrailDecision {
    if text.contains("SECRET") {
        GuardrailDecision::Sanitize(crate::guardrails::decision::Replacement {
            text: text.replace("SECRET", "[REDACTED]"),
            source: "silent_scrubber".into(),
        })
    } else {
        GuardrailDecision::Allow
    }
}

struct WarnOnly;
#[async_trait]
impl InputGuardrail for WarnOnly {
    fn name(&self) -> &str {
        "warn_only"
    }
    async fn evaluate_input(&self, _text: &str) -> GuardrailDecision {
        GuardrailDecision::Warn {
            reason: "looks odd".into(),
        }
    }
}

/// The three `evaluate_*` chains must answer a warn-free rewrite identically.
///
/// They did not: `evaluate_tool_call` decided on "did the payload change",
/// while `evaluate_input` / `evaluate_output` decided on "did anything warn"
/// and therefore returned `Allow` — discarding the rewrite and putting the
/// original secret on the wire, with no error and no log. Two of three twins
/// wrong, and the correct predicate sat twenty lines below them in the same
/// file. This pins the *parity* rather than any one twin's answer: the failure
/// mode is divergence, so the assertion has to be about the set.
#[tokio::test]
async fn all_three_chains_surface_a_warn_free_rewrite() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(SilentScrubber))
        .with_output(Arc::new(SilentScrubber))
        .with_tool_call(Arc::new(SilentScrubber))
        .build();

    let surfaces: Vec<(&str, GuardrailDecision)> = vec![
        ("input", r.evaluate_input("the password is SECRET").await),
        ("output", r.evaluate_output("the password is SECRET").await),
        (
            "tool_call",
            r.evaluate_tool_call("t", &serde_json::json!({ "token": "SECRET" }))
                .await,
        ),
    ];

    for (surface, decision) in &surfaces {
        let GuardrailDecision::Sanitize(rep) = decision else {
            panic!("{surface}: a warn-free rewrite must surface as Sanitize, got {decision:?}");
        };
        assert!(
            !rep.text.contains("SECRET"),
            "{surface}: the rewrite must be the one the caller swaps in, got {:?}",
            rep.text,
        );
        assert!(
            rep.text.contains("[REDACTED]"),
            "{surface}: the scrubbed text must survive, got {:?}",
            rep.text,
        );
        assert!(
            !rep.source.contains("warn:"),
            "{surface}: no guardrail warned, so the audit source must not claim one did, got {:?}",
            rep.source,
        );
    }
}

/// A chain that only warns still surfaces as a no-op `Sanitize` carrying the
/// reasons — the pre-existing contract the fix above must not have traded away.
/// Without this, "stop dropping the rewrite" is one over-correction away from
/// "`Allow` unless rewritten", which silently discards every warn reason
/// instead — the same class of loss, pointed the other way.
#[tokio::test]
async fn a_warn_only_chain_still_carries_its_reasons_unchanged() {
    let r = GuardrailRegistry::builder()
        .with_input(Arc::new(WarnOnly))
        .build();
    let d = r.evaluate_input("plain text").await;
    let GuardrailDecision::Sanitize(rep) = d else {
        panic!("a warn must still surface, got {d:?}");
    };
    assert_eq!(rep.text, "plain text", "a warn must not rewrite anything");
    assert!(
        rep.source.contains("warn:"),
        "the reasons must reach the caller, got {:?}",
        rep.source
    );
    assert!(
        !rep.source.contains("sanitized"),
        "nothing was rewritten, so the source must not say it was, got {:?}",
        rep.source
    );
}
