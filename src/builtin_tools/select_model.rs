//! `SelectModelTool` — LLM-facing model picker (R8 "everything is a tool").
//!
//! The "A layer" of AI dynamic routing: lets the main-loop LLM choose the model
//! for the rest of the conversation in one inference, rather than a separate
//! routing model deciding for it (which would violate R7/R9). The pick is
//! recorded in `session_model_handle`
//! and applied at the next run's construction (`harness_bridge`), where it
//! wraps the chosen provider in a `ModelOverrideProvider` to stamp the model.
//!
//! Model binding is per-run, not per-iteration: `llm` is bound ONCE at run
//! construction and the whole Think→Act loop executes inside that binding. A
//! pick therefore takes effect at the next **run** — the next user message —
//! not at the next loop iteration. The tool says exactly that, because a model
//! that reads "next turn" as "iteration 4 onward" will keep working under an
//! assumption (a bigger context window, vision support) that will not hold for
//! the remaining 37 iterations of the current run.

use async_trait::async_trait;
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

use super::{notify_tool_result, notify_tool_start};
use crate::error::Result;
use crate::providers::session_model_handle;
use crate::tools::turn_context::current_turn_context;
use crate::tools::AlephTool;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SelectModelArgs {
    /// Model id to use for the rest of this conversation, e.g.
    /// `"claude-sonnet-5"`, `"gpt-5.6"`, `"deepseek-v4-flash"`.
    ///
    /// Keep these examples on current ids: they are the only model ids many
    /// callers ever see, and this doc previously offered `deepseek-chat` —
    /// retired 2026-07-24 — as a suggestion.
    #[schemars(description = "Model id to switch to for the rest of this conversation.")]
    pub model: String,
    /// Optional provider id to pin (e.g. `"openai"`, `"anthropic"`). Omit to
    /// let the system route by model name / fall back to the default provider.
    #[serde(default)]
    #[schemars(description = "Optional provider id to pin; omit to auto-route by model name.")]
    pub provider: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SelectModelOutput {
    /// True when the preference was recorded.
    pub ok: bool,
    /// The model now selected.
    pub model: String,
    /// The provider pinned, if any.
    pub provider: Option<String>,
    /// Human-readable confirmation.
    pub message: String,
}

#[derive(Clone, Default)]
pub struct SelectModelTool;

#[async_trait]
impl AlephTool for SelectModelTool {
    const NAME: &'static str = "select_model";
    const DESCRIPTION: &'static str = "Switch the LLM model for the rest of this conversation. \
        Use when a task needs a different model than the current one — e.g. a larger context \
        window for a big document, a vision-capable model for images, a reasoning model for hard \
        problems, or a cheaper model for simple chat. Pass `model` (required) and optionally \
        `provider` to pin a specific provider. The change applies to the next MESSAGE, not the \
        next step of the current one: the whole current run — every remaining tool call and \
        model reply — finishes on the current model, so do not switch mid-task and then act as \
        if the new model's limits already apply.";

    type Args = SelectModelArgs;
    type Output = SelectModelOutput;

    async fn call(&self, args: Self::Args) -> Result<Self::Output> {
        notify_tool_start(Self::NAME, &args.model);

        // The session to scope the preference to comes from the task-local
        // turn context set by the tool dispatcher. Outside a turn (should not
        // happen for a real tool call) there is nothing to scope to.
        let Some(ctx) = current_turn_context() else {
            let message =
                "No active session: select_model must run inside a conversation turn.".to_string();
            notify_tool_result(Self::NAME, &message, false);
            return Ok(SelectModelOutput {
                ok: false,
                model: args.model,
                provider: args.provider,
                message,
            });
        };

        let key = ctx.session_key.to_key_string();

        // Normal model picks are recorded for the next run.
        // Validate before recording.
        if let Some(refusal) = refuse_unusable_model(&args.model, args.provider.as_deref()) {
            notify_tool_result(Self::NAME, &refusal, false);
            return Ok(SelectModelOutput {
                ok: false,
                model: args.model,
                provider: args.provider,
                message: refusal,
            });
        }
        if let Some(refusal) = refuse_unpinnable_provider(args.provider.as_deref()) {
            notify_tool_result(Self::NAME, &refusal, false);
            return Ok(SelectModelOutput {
                ok: false,
                model: args.model,
                provider: args.provider,
                message: refusal,
            });
        }

        session_model_handle::set_session_model(&key, args.provider.clone(), args.model.clone());

        // "takes effect from your next message — this run finishes on the
        // current model" is a promise, and crash recovery is the one path that
        // could have broken it: a resume replays the (provider, model) frozen
        // on the interrupted run's `RunStarted` marker, so a `select_model`
        // call made *after* the crash does NOT retroactively re-serve the
        // recovered run. It governs the next message, as promised. Guarded by
        // `resume_coordinator::tests::a_live_snapshot_model_is_replayed_as_a_qualified_override`.
        let mut message = match &args.provider {
            Some(p) => format!(
                "Model switched to '{}' (provider '{}'); takes effect from your next message \
                 — this run finishes on the current model.",
                args.model, p
            ),
            None => format!(
                "Model switched to '{}'; takes effect from your next message — this run \
                 finishes on the current model.",
                args.model
            ),
        };
        if let Some(caveat) = caveat_for_model(&args.model, args.provider.as_deref()) {
            message.push(' ');
            message.push_str(&caveat);
        }
        notify_tool_result(Self::NAME, &message, true);
        Ok(SelectModelOutput {
            ok: true,
            model: args.model,
            provider: args.provider,
            message,
        })
    }
}

/// Hard refusals: picks that are known to fail, or that name nothing at all.
///
/// Deliberately narrow. An id absent from the reference tables is *not*
/// refused — the tables are curated and always behind, custom relays alias
/// their own names, and refusing the unknown would make Aleph unable to reach
/// any model released since the binary was built. Only two cases are certain
/// enough to block: an empty selection, and an id the vendor has retired.
fn refuse_unusable_model(model: &str, provider: Option<&str>) -> Option<String> {
    if model.trim().is_empty() {
        return Some("No model id given; model unchanged.".to_string());
    }
    // `provider` matters: half the retirements Aleph records are one host's word
    // about its own catalog (Groq dropped both Llama tiers; Together still
    // serves them). Dropping it here would have meant either refusing ids that
    // work, or never refusing the ones that do not.
    let life = crate::providers::model_catalog::lifecycle_for(provider, model);
    if !life.is_deprecated() {
        return None;
    }
    let mut msg = format!("'{model}' has been retired by its vendor and will fail");
    if let Some(note) = life.note {
        msg.push_str(&format!(" ({note})"));
    }
    msg.push_str("; model unchanged.");
    if let Some(successor) = life.successor {
        msg.push_str(&format!(" Use '{successor}' instead."));
    }
    Some(msg)
}

/// Refuse a `provider` pin that the run builder cannot resolve.
///
/// Unlike the model id — where the curated tables are always behind the vendors,
/// so an unknown id is a caveat rather than a refusal — the provider key set is
/// *closed*: it is exactly the `[providers]` table this daemon booted with, and
/// [`pinnable_providers`](session_model_handle::pinnable_providers) publishes
/// the very map the next run resolves against. An unknown key is therefore not
/// "possibly new", it is certainly wrong, and accepting it means silently
/// serving the answer from a different vendor than the one just confirmed.
///
/// Matching is exact, mirroring the runtime's `HashMap` lookup — a case
/// mismatch (`"Anthropic"`) is refused with the real spelling in the list
/// rather than accepted and then silently substituted.
fn refuse_unpinnable_provider(provider: Option<&str>) -> Option<String> {
    let provider = provider?;
    // No published set (tests, pre-boot) = unvalidated, not "nothing valid".
    let known = session_model_handle::pinnable_providers()?;
    if known.contains(provider) {
        return None;
    }
    let mut msg = format!("Provider '{provider}' is not configured; model unchanged.");
    if known.is_empty() {
        msg.push_str(" No providers are registered for pinning.");
    } else {
        msg.push_str(&format!(
            " Configured providers: {}. Omit `provider` to keep the default routing.",
            known.iter().cloned().collect::<Vec<_>>().join(", ")
        ));
    }
    Some(msg)
}

/// Soft caveats appended to a successful switch.
///
/// The pick still happens — this is the "we did it, but you should know"
/// channel, so the model can course-correct on the same turn instead of
/// discovering the problem from a downstream failure.
fn caveat_for_model(model: &str, provider: Option<&str>) -> Option<String> {
    let record = crate::providers::model_catalog::ModelRecord::resolve(
        provider.unwrap_or_default(),
        model,
        None,
        crate::providers::model_catalog::ModelSource::Configured,
        None,
    );
    if matches!(
        record.lifecycle.status,
        crate::providers::model_catalog::ModelStatus::Preview
    ) {
        return Some(format!(
            "Note: '{model}' is a vendor preview id — it can change or disappear without notice."
        ));
    }
    if record.is_unknown() {
        return Some(format!(
            "Note: '{model}' is not in Aleph's model catalog, so its context window and cost are \
             unknown (the request will still be sent as-is). Call `list_models` — optionally with \
             `refresh: true` — if you meant a catalogued id."
        ));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routing::session_key::SessionKey;
    use crate::tools::turn_context::{TurnContext, TURN_CONTEXT};
    use crate::tools::AlephTool;

    #[tokio::test]
    async fn writes_session_model_under_turn_context() {
        let sk = SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "select-model-test".to_string(),
        };
        let key = sk.to_key_string();
        session_model_handle::clear_session_model(&key);

        let ctx = TurnContext {
            session_key: sk.clone(),
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
            telecom_authority: None,
        };
        let out = TURN_CONTEXT
            .scope(ctx, async {
                SelectModelTool
                    .call(SelectModelArgs {
                        model: "gpt-5".to_string(),
                        provider: Some("openai".to_string()),
                    })
                    .await
            })
            .await
            .unwrap();

        assert!(out.ok);
        let pref = session_model_handle::get_session_model(&key).unwrap();
        assert_eq!(pref.model, "gpt-5");
        assert_eq!(pref.provider.as_deref(), Some("openai"));
        session_model_handle::clear_session_model(&key);
    }

    /// A retired id must be refused at the tool call, not one turn later at
    /// the provider — and the refusal must name the successor.
    #[tokio::test]
    async fn retired_model_is_refused_with_its_successor() {
        let sk = SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "select-retired".to_string(),
        };
        let key = sk.to_key_string();
        session_model_handle::clear_session_model(&key);

        let ctx = TurnContext {
            session_key: sk,
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
            telecom_authority: None,
        };
        let out = TURN_CONTEXT
            .scope(ctx, async {
                SelectModelTool
                    .call(SelectModelArgs {
                        model: "deepseek-chat".to_string(),
                        provider: Some("deepseek".to_string()),
                    })
                    .await
            })
            .await
            .unwrap();

        assert!(!out.ok, "{}", out.message);
        assert!(out.message.contains("deepseek-v4-flash"), "{}", out.message);
        // Nothing recorded: the session keeps whatever it had.
        assert!(session_model_handle::get_session_model(&key).is_none());
    }

    /// An id Aleph has never heard of must still be accepted — the catalog is
    /// curated and always trails the vendors — but with the caveat attached so
    /// the model knows the window and cost figures are unavailable.
    #[tokio::test]
    async fn unknown_model_is_accepted_with_a_caveat() {
        let sk = SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "select-unknown".to_string(),
        };
        let key = sk.to_key_string();
        session_model_handle::clear_session_model(&key);

        let ctx = TurnContext {
            session_key: sk,
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
            telecom_authority: None,
        };
        let out = TURN_CONTEXT
            .scope(ctx, async {
                SelectModelTool
                    .call(SelectModelArgs {
                        model: "internal-relay-model-v9".to_string(),
                        provider: None,
                    })
                    .await
            })
            .await
            .unwrap();

        assert!(out.ok, "{}", out.message);
        assert!(
            out.message.contains("not in Aleph's model catalog"),
            "{}",
            out.message
        );
        assert_eq!(
            session_model_handle::get_session_model(&key).unwrap().model,
            "internal-relay-model-v9"
        );
        session_model_handle::clear_session_model(&key);
    }

    #[test]
    fn refusals_cover_empty_and_retired_only() {
        assert!(refuse_unusable_model("   ", None).is_some());
        assert!(refuse_unusable_model("deepseek-reasoner", None).is_some());
        // Current ids and unknown ids both pass the hard gate.
        assert!(refuse_unusable_model("claude-sonnet-5", None).is_none());
        assert!(refuse_unusable_model("some-brand-new-model", None).is_none());
    }

    /// The `provider` argument is not decoration: half of Aleph's retirement
    /// records are one host's word about its own catalog. Dropping it (as this
    /// call site did) meant a pin that is certain to 404 sailed through.
    #[test]
    fn host_scoped_retirement_is_refused_only_for_that_host() {
        let refusal = refuse_unusable_model("llama-3.3-70b-versatile", Some("groq"))
            .expect("Groq retired this id");
        assert!(refusal.contains("gpt-oss-120b"), "{refusal}");
        // Same id, a host that still serves it.
        assert!(refuse_unusable_model("llama-3.3-70b-versatile", Some("together")).is_none());
        // …and with no provider named we cannot attribute the retirement.
        assert!(refuse_unusable_model("llama-3.3-70b-versatile", None).is_none());
    }

    #[test]
    fn preview_ids_get_a_caveat_but_not_a_refusal() {
        assert!(refuse_unusable_model("gemini-3.1-pro-preview", Some("gemini")).is_none());
        let caveat = caveat_for_model("gemini-3.1-pro-preview", Some("gemini")).unwrap();
        assert!(caveat.contains("preview"), "{caveat}");
        // A fully catalogued stable model gets no noise at all.
        assert!(caveat_for_model("claude-sonnet-5", Some("anthropic")).is_none());
    }

    #[tokio::test]
    async fn no_turn_context_is_graceful() {
        // Outside a turn scope there is no session to bind to — degrade, not panic.
        let out = SelectModelTool
            .call(SelectModelArgs {
                model: "claude-opus-4".to_string(),
                provider: None,
            })
            .await
            .unwrap();
        assert!(!out.ok);
    }
}
