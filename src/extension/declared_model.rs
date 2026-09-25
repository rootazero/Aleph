//! A plugin component's declared `model:` — a command's or an agent's — as
//! the model it pins: ONE policy for both faces.
//!
//! The command face (`gateway::execution_engine::slash_command_body::
//! command_model_pin`) turns the answer into this turn's model override; the
//! agent face (`extension::plugin_agent_to_def`) into the sub-agent's
//! `model_hint`, which the spawner stamps on every request the child makes.
//! Before this module the agent face passed `model:` through as written, and
//! Claude Code's `model: inherit` — what most upstream agents declare — asked
//! the provider for a model called "inherit".

/// What `declared` pins. `Ok(None)` pins nothing, `Ok(Some(id))` pins `id` as
/// written, `Err(why)` names a model the provider would fail. `kind`
/// (`command` / `agent`) only words the log line.
///
/// - `inherit` and a blank value pin nothing.
/// - Claude Code's aliases `sonnet` / `opus` / `haiku` name a model family,
///   not an id any provider routes, and Aleph has no alias table: nothing is
///   pinned — the component runs on the model it would run on without one —
///   and that is logged.
/// - An id the model catalog records as retired is `Err`, with its successor
///   named: the provider would fail it. The same `lifecycle_for` table and
///   rule as `select_model`, the other face that pins a model.
/// - Any other id is pinned as written, as `select_model` accepts an id the
///   catalog does not know. One no provider serves then fails at the
///   provider, or a fallback walk serves on another model and says so.
///
/// What an `Err` costs is the caller's: a command's turn is refused, an
/// agent loads with no hint.
pub(crate) fn declared_model_pin(
    kind: &str,
    declared: Option<&str>,
) -> Result<Option<String>, String> {
    let Some(model) = declared.map(str::trim).filter(|m| !m.is_empty()) else {
        return Ok(None);
    };
    match model {
        "inherit" => return Ok(None),
        "sonnet" | "opus" | "haiku" => {
            tracing::warn!(
                kind,
                model,
                "{kind} declares a Claude Code model alias; Aleph routes no aliases, so it pins \
                 no model and runs on the one it would use without a `model:`"
            );
            return Ok(None);
        }
        _ => {}
    }
    let life = crate::providers::model_catalog::lifecycle_for(None, model);
    if life.is_deprecated() {
        let mut why = format!("its `model: {model}` has been retired by its vendor");
        if let Some(note) = life.note {
            why.push_str(&format!(" ({note})"));
        }
        if let Some(successor) = life.successor {
            why.push_str(&format!("; the plugin should declare `{successor}`"));
        }
        return Err(why);
    }
    Ok(Some(model.to_string()))
}
