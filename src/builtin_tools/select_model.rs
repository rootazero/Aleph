//! `SelectModelTool` — LLM-facing model picker (R8 "everything is a tool").
//!
//! An explicit per-session model pin: the `select_model` tool lets the main-loop
//! LLM choose a model and optionally pin its provider. This is explicit model
//! binding selected by the user or LLM, with no automatic routing layer. The pick is
//! recorded in `session_model_handle` and resolved by `SessionModelProvider` at
//! each provider request boundary. The request already in flight remains
//! immutable while subsequent requests use the pick.
//!
//! A pick therefore takes effect at the next provider request for this session;
//! the request already in flight remains immutable. Crash recovery preserves the
//! interrupted run's frozen target, so a pick made after a crash applies to the
//! next new message rather than retroactively changing the resumed run.

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
    #[schemars(
        description = "Model id to switch to for the rest of this conversation. Pass a plain model id, or qualify it as provider/model."
    )]
    pub model: String,
    /// Optional configured provider id. Omit only for an unambiguous model id,
    /// or qualify the model as `provider/model`.
    #[serde(default)]
    #[schemars(description = "Optional provider id to pin; omit to use the model's configured provider.")]
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
    /// Ordering generation the pick was recorded under. `None` when nothing
    /// was written — refusals and no active turn.
    pub generation: Option<u64>,
    /// Whether the pick names a known, generally-available model. `false` for
    /// previews, unknown ids, and every refusal.
    pub stable: bool,
    /// Machine-readable config/runtime resolution refusal; absent on success.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub rejection: Option<session_model_handle::ModelSelectionError>,
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
        `provider` to pin a specific provider. The change applies at the next provider request boundary: \
        the request currently in flight finishes on its immutable target, and subsequent requests use \
        the new selection.";

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
                rejection: None,
                ok: false,
                model: args.model,
                provider: args.provider,
                generation: None,
                stable: false,
                message,
            });
        };

        let key = ctx.session_key.to_key_string();

        // Resolve one immutable config/runtime projection before any state write.
        let pref = match session_model_handle::resolve_selectable_model(
            args.provider.as_deref(),
            &args.model,
        ) {
            Ok(pref) => pref,
            Err(rejection) => {
                let message = rejection.to_string();
                notify_tool_result(Self::NAME, &message, false);
                return Ok(SelectModelOutput {
                    ok: false,
                    model: args.model,
                    provider: args.provider,
                    generation: None,
                    stable: false,
                    message,
                    rejection: Some(rejection),
                });
            }
        };
        if let Some(refusal) = refuse_unusable_model(&pref.model, pref.provider.as_deref()) {
            notify_tool_result(Self::NAME, &refusal, false);
            return Ok(SelectModelOutput {
                ok: false,
                model: pref.model,
                provider: pref.provider,
                generation: None,
                stable: false,
                message: refusal,
                rejection: None,
            });
        }
        let update =
            session_model_handle::update_session_model_durable(&key, Some(pref.clone())).await;
        if !update.accepted || !update.durable {
            let message = if update.generation == u64::MAX {
                "Model pin generation exhausted; model unchanged.".to_string()
            } else {
                "Model selection could not be durably persisted; model unchanged.".to_string()
            };
            notify_tool_result(Self::NAME, &message, false);
            return Ok(SelectModelOutput {
                ok: false,
                model: pref.model,
                provider: pref.provider,
                generation: None,
                stable: false,
                message,
                rejection: None,
            });
        }
        // Durable refusals leave the existing model pin intact.
        let args = SelectModelArgs {
            model: pref.model,
            provider: pref.provider,
        };

        // A normal live run reads the selection at its next provider request
        // boundary. Crash-resume replays the interrupted run's frozen
        // RunStarted envelope, so a selection made after the crash applies to
        // the next new message rather than retroactively changing that replay.
        let mut message = match &args.provider {
            Some(p) => format!(
                "Model switched to '{}' (provider '{}'); the current request finishes on its \
                 immutable target, and the next provider request uses the new selection.",
                args.model, p
            ),
            None => format!(
                "Model switched to '{}'; the current request finishes on its immutable target, \
                 and the next provider request uses the new selection.",
                args.model
            ),
        };
        if let Some(caveat) = caveat_for_model(&args.model, args.provider.as_deref()) {
            message.push(' ');
            message.push_str(&caveat);
        }
        let stable = is_stable_pick(&args.model, args.provider.as_deref());
        notify_tool_result(Self::NAME, &message, true);
        Ok(SelectModelOutput {
            rejection: None,
            ok: true,
            model: args.model,
            provider: args.provider,
            generation: Some(update.generation),
            stable,
            message,
        })
    }
}

/// Preserve vendor lifecycle refusals after resolving the configured provider.
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

/// Whether a successful pick names a known, generally-available model.
///
/// Mirrors [`caveat_for_model`]'s catalogue lookup: `stable` is `Active` *and*
/// known — an unknown id resolves with a default (`Active`) lifecycle but
/// `is_unknown() == true`, and calling it "stable" would be a confident false
/// answer (P7). Previews are not stable; deprecated ids never reach here (they
/// are refused earlier).
fn is_stable_pick(model: &str, provider: Option<&str>) -> bool {
    let record = crate::providers::model_catalog::ModelRecord::resolve(
        provider.unwrap_or_default(),
        model,
        None,
        crate::providers::model_catalog::ModelSource::Configured,
        None,
    );
    !record.is_unknown()
        && matches!(
            record.lifecycle.status,
            crate::providers::model_catalog::ModelStatus::Active
        )
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
             unknown (the id is explicitly present in the selectable roster). Call `list_models` — optionally with \
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

    // Explicit task-scoped publication: tests never relax production's
    // missing-publication refusal and do not race on a process-global roster.
    fn selectable_fixture() -> session_model_handle::SelectableModelsSnapshot {
        use crate::config::{Config, ProviderConfig};
        use std::collections::BTreeSet;
        let mut config = Config::default();
        config.providers.clear();
        for (name, model) in [
            ("openai", "gpt-5"),
            ("anthropic", "claude-sonnet-5"),
            ("deepseek", "deepseek-chat"),
            ("relay", "internal-relay-model-v9"),
            ("relay-peer", "shared-model"),
            ("offline", "offline-model"),
            ("noauth", "noauth-model"),
            ("disabled", "disabled-model"),
            ("not-wired", "unwired-model"),
        ] {
            let mut pc = ProviderConfig::test_config(model);
            if name == "relay" {
                pc.models.push("shared-model".into());
                pc.models.push("org/model".into());
                pc.model_defs.push(
                    serde_json::from_value(serde_json::json!({"id": "defined-only-model"}))
                        .unwrap(),
                );
            }
            if name == "noauth" {
                pc.api_key = None;
            }
            if name == "disabled" {
                pc.enabled = false;
            }
            config.providers.insert(name.into(), pc);
        }
        let routable: BTreeSet<_> = config
            .providers
            .keys()
            .filter(|name| name.as_str() != "not-wired")
            .cloned()
            .collect();
        let available = config
            .providers
            .keys()
            .filter(|name| name.as_str() != "offline")
            .cloned()
            .collect();
        session_model_handle::SelectableModelsSnapshot::from_config(
            &config,
            routable,
            &available,
            |name| name != "noauth",
        )
    }

    async fn call_select_model(args: SelectModelArgs) -> Result<SelectModelOutput> {
        session_model_handle::with_test_selectable_models(
            Some(selectable_fixture()),
            SelectModelTool.call(args),
        )
        .await
    }

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
                call_select_model(SelectModelArgs {
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
                call_select_model(SelectModelArgs {
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

    /// An explicitly configured custom id need not exist in compiled tables.
    /// Its metadata remains unknown, but roster membership is authoritative.
    #[tokio::test]
    async fn configured_custom_model_is_accepted_with_unknown_metadata() {
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
                call_select_model(SelectModelArgs {
                    model: "internal-relay-model-v9".to_string(),
                    provider: None,
                })
                .await
            })
            .await
            .unwrap();

        assert!(out.ok, "{}", out.message);
        assert!(!out.stable, "unknown model must not be reported stable");
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
    fn lifecycle_guard_covers_empty_and_retired() {
        assert!(refuse_unusable_model("   ", None).is_some());
        assert!(refuse_unusable_model("deepseek-reasoner", None).is_some());
        // This helper checks lifecycle only; roster resolution is a separate gate.
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
        let out = call_select_model(SelectModelArgs {
            model: "claude-opus-4".to_string(),
            provider: None,
        })
        .await
        .unwrap();
        assert!(!out.ok);
    }

    /// A successful pick reports its ordering generation and stability flag;
    /// a second pick advances the generation monotonically.
    #[tokio::test]
    async fn successful_pick_reports_generation_and_stability() {
        let sk = SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "select-generation".to_string(),
        };
        let key = sk.to_key_string();
        session_model_handle::clear_session_model(&key);

        let before = session_model_handle::get_session_model_snapshot(&key).generation;
        let ctx = || TurnContext {
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

        let first = TURN_CONTEXT
            .scope(ctx(), async {
                call_select_model(SelectModelArgs {
                    model: "claude-sonnet-5".to_string(),
                    provider: Some("anthropic".to_string()),
                })
                .await
            })
            .await
            .unwrap();
        assert!(first.ok, "{}", first.message);
        assert!(
            first.stable,
            "stable model reported not-stable: {}",
            first.message
        );
        assert_eq!(first.generation, Some(before + 1));

        let second = TURN_CONTEXT
            .scope(ctx(), async {
                call_select_model(SelectModelArgs {
                    model: "gpt-5".to_string(),
                    provider: None,
                })
                .await
            })
            .await
            .unwrap();
        assert!(second.ok, "{}", second.message);
        assert_eq!(second.generation, Some(before + 2));
        session_model_handle::clear_session_model(&key);
    }

    /// A refused pick (retired id) must not overwrite the pin the user already
    /// had — the old selection survives untouched and no generation is reported.
    #[tokio::test]
    async fn refused_pick_preserves_the_previous_pin() {
        let sk = SessionKey::Ephemeral {
            agent_id: "main".to_string(),
            ephemeral_id: "select-preserve".to_string(),
        };
        let key = sk.to_key_string();
        session_model_handle::set_session_model(
            &key,
            Some("openai".to_string()),
            "gpt-5".to_string(),
        );

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
                call_select_model(SelectModelArgs {
                    model: "deepseek-chat".to_string(),
                    provider: Some("deepseek".to_string()),
                })
                .await
            })
            .await
            .unwrap();

        assert!(!out.ok, "{}", out.message);
        assert_eq!(out.generation, None);
        assert!(!out.stable);
        let pref = session_model_handle::get_session_model(&key).unwrap();
        assert_eq!(pref.model, "gpt-5");
        assert_eq!(pref.provider.as_deref(), Some("openai"));
        session_model_handle::clear_session_model(&key);
    }

    fn test_context(id: &str) -> TurnContext {
        TurnContext {
            session_key: SessionKey::Ephemeral {
                agent_id: "main".into(),
                ephemeral_id: id.into(),
            },
            run_id: String::new(),
            channel_id: String::new(),
            conversation_id: String::new(),
            caller_role: None,
            channel_tool_permissions: None,
            unattended: false,
            plan_gate: None,
            side_question: false,
            telecom_authority: None,
        }
    }

    #[tokio::test]
    async fn resolution_refusals_preserve_pin_generation() {
        for (id, model, provider, expected) in [
            ("unknown", "not-configured-v999", None, "unknown"),
            (
                "unknown-provider",
                "gpt-5",
                Some("missing-provider"),
                "unknown",
            ),
            ("ambiguous", "shared-model", None, "ambiguous"),
            (
                "unauthenticated",
                "noauth-model",
                Some("noauth"),
                "unauthenticated",
            ),
            (
                "unavailable",
                "offline-model",
                Some("offline"),
                "unsupported",
            ),
            (
                "unsupported",
                "not-configured-v999",
                Some("openai"),
                "unsupported",
            ),
            (
                "disabled",
                "disabled-model",
                Some("disabled"),
                "unsupported",
            ),
            ("unwired", "unwired-model", Some("not-wired"), "unsupported"),
        ] {
            let ctx = test_context(&format!("select-refusal-{id}"));
            let key = ctx.session_key.to_key_string();
            session_model_handle::set_session_model(&key, Some("openai".into()), "gpt-5".into());
            let before = session_model_handle::get_session_model_snapshot(&key);
            let out = TURN_CONTEXT
                .scope(
                    ctx,
                    call_select_model(SelectModelArgs {
                        model: model.into(),
                        provider: provider.map(str::to_owned),
                    }),
                )
                .await
                .unwrap();
            assert!(!out.ok, "{id}: {}", out.message);
            assert_eq!(out.generation, None, "{id}");
            assert!(!out.stable, "{id}");
            let value = serde_json::to_value(&out).unwrap();
            assert_eq!(value["rejection"]["kind"], expected, "{id}: {value}");
            assert_eq!(
                session_model_handle::get_session_model_snapshot(&key),
                before,
                "{id}"
            );
            session_model_handle::clear_session_model(&key);
        }
    }

    #[tokio::test]
    async fn missing_publication_is_a_structured_refusal_and_preserves_pin() {
        let ctx = test_context("select-no-publication");
        let key = ctx.session_key.to_key_string();
        session_model_handle::set_session_model(&key, Some("openai".into()), "gpt-5".into());
        let before = session_model_handle::get_session_model_snapshot(&key);
        let out = TURN_CONTEXT
            .scope(
                ctx,
                session_model_handle::with_test_selectable_models(
                    None,
                    SelectModelTool.call(SelectModelArgs {
                        model: "gpt-5".into(),
                        provider: Some("openai".into()),
                    }),
                ),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert_eq!(
            out.rejection,
            Some(session_model_handle::ModelSelectionError::Unavailable)
        );
        assert_eq!(
            session_model_handle::get_session_model_snapshot(&key),
            before
        );
        session_model_handle::clear_session_model(&key);
    }

    #[tokio::test]
    async fn qualified_custom_model_def_and_slash_id_resolve_canonically() {
        for (id, model, provider, canonical) in [
            (
                "qualified",
                "relay/defined-only-model",
                None,
                "defined-only-model",
            ),
            (
                "explicit",
                "defined-only-model",
                Some("relay"),
                "defined-only-model",
            ),
            ("slash-id", "org/model", Some("relay"), "org/model"),
            ("bare-slash-id", "org/model", None, "org/model"),
        ] {
            let ctx = test_context(&format!("select-custom-{id}"));
            let key = ctx.session_key.to_key_string();
            let out = TURN_CONTEXT
                .scope(
                    ctx,
                    call_select_model(SelectModelArgs {
                        model: model.into(),
                        provider: provider.map(str::to_owned),
                    }),
                )
                .await
                .unwrap();
            assert!(out.ok, "{id}: {}", out.message);
            assert_eq!(out.model, canonical);
            assert_eq!(out.provider.as_deref(), Some("relay"));
            assert_eq!(out.generation, Some(1));
            assert!(out.rejection.is_none());
            let stored = session_model_handle::get_session_model(&key).unwrap();
            assert_eq!(stored.model, canonical);
            assert_eq!(stored.provider.as_deref(), Some("relay"));
            session_model_handle::clear_session_model(&key);
        }
    }

    #[tokio::test]
    async fn generation_exhaustion_preserves_pin() {
        let ctx = test_context("select-generation-exhausted");
        let key = ctx.session_key.to_key_string();
        let old = session_model_handle::SessionModelPref {
            provider: Some("openai".into()),
            model: "gpt-5".into(),
        };
        assert!(session_model_handle::hydrate_session_model_at_generation(
            &key,
            old.clone(),
            u64::MAX
        ));
        let out = TURN_CONTEXT
            .scope(
                ctx,
                call_select_model(SelectModelArgs {
                    model: "claude-sonnet-5".into(),
                    provider: Some("anthropic".into()),
                }),
            )
            .await
            .unwrap();
        assert!(!out.ok);
        assert!(out.message.contains("generation exhausted"));
        assert_eq!(out.generation, None);
        assert_eq!(
            session_model_handle::get_session_model_snapshot(&key),
            session_model_handle::SessionModelSnapshot {
                pref: Some(old),
                generation: u64::MAX
            }
        );
    }

}
