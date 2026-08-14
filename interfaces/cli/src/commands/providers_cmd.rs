//! AI provider management commands
//!
//! The request/response shapes are `aleph_protocol::providers::*` — the same
//! types the server builds its responses from. They are not re-declared here
//! on purpose; see that module for what happened the last time they were, and
//! why two of the commands below had never once worked.
//!
//! Everything here is I/O (R4): the roster a provider offers, which rows match
//! a query, and whether a discovery failure is worth retrying are all decided
//! server-side or by the shared matcher. This file picks columns.

use chrono::TimeZone;
use serde_json::Value;

use crate::output;
use aleph_client::{AlephClient, CliConfig, CliError, CliResult};
use aleph_protocol::providers::{
    filter_catalog, CatalogEntry, CatalogParams, CatalogResult, CatalogView, CreateParams,
    DeleteParams, DiscoveryFailureKind, GetParams, ModelCapabilities, ModelsRefreshParams,
    ModelsRefreshResult, ModelsRefreshRow, ProviderConfigJson, ProviderGetResult,
    ProviderHealthResult, ProviderHealthRow, ProviderInfo, ProviderListResult, RateCard,
    RefreshOutcome, RosterModel, Searchable, SetDefaultParams, TestParams, TestResult,
    UpdateParams,
};

/// `providers list` filters through the same matcher as every other picker.
///
/// A configured provider's name is both its id and its label — there is no
/// second display string, and inventing one would make the display-name tier
/// fire on rows that have none.
struct ProviderRow<'a>(&'a ProviderInfo);

impl Searchable for ProviderRow<'_> {
    fn search_id(&self) -> &str {
        &self.0.name
    }
    fn search_display_name(&self) -> &str {
        &self.0.name
    }
}

/// Columns of `aleph providers list`.
///
/// `Type` and `Default` printed a dash on every row from the day this command
/// was written: it read `type` and `default`, and the server has only ever
/// sent `provider_type` and `is_default`. Both now come off [`ProviderInfo`].
const LIST_HEADERS: &[&str] = &["Name", "Type", "Default"];

/// Columns of `aleph providers health`.
const HEALTH_HEADERS: &[&str] = &["Provider", "State", "Latency", "Detail"];

/// Columns of `aleph providers models`.
///
/// `Context` and `$/Mtok` are the two facts a headless operator picks a ladder
/// by, and they were on the wire for three rounds without reaching any face: the
/// catalogue attached them to the provider's *default* model, on a row whose job
/// is offering the others. They are per-model now, so they are columns.
const ROSTER_HEADERS: &[&str] = &["Provider", "Model", "Context", "$/Mtok", "Source", "Status"];

/// Columns of `aleph providers models --refresh`.
const REFRESH_HEADERS: &[&str] = &["Provider", "State", "Models", "Detail"];

/// Render one `providers list` row.
///
/// `-` under Type means the server sent no protocol for this provider, which
/// is a fact about the provider — unlike the previous `-`, which meant this
/// CLI was reading a field name the server does not have.
fn row_cells(provider: &ProviderInfo) -> Vec<String> {
    vec![
        provider.name.clone(),
        provider
            .provider_type
            .clone()
            .unwrap_or_else(|| "-".to_string()),
        yes_no(provider.is_default),
    ]
}

/// Render one provider as the labelled lines `aleph providers get` prints.
///
/// Three [`ProviderInfo`] fields are deliberately absent. `api_key` is never
/// sent — the key lives in the vault and a response is the one place it must
/// not appear, so `Key` reports `has_api_key` instead. `model` is the server's
/// `models[0]` projection for older clients; printing it beside `Models` would
/// put one fact on screen twice. `color` is Panel styling with nothing to say
/// in a terminal.
fn detail_pairs(provider: &ProviderInfo) -> Vec<(&'static str, String)> {
    let or_dash = |value: &Option<String>| value.clone().unwrap_or_else(|| "-".to_string());
    let number = |value: Option<u32>| value.map_or_else(|| "-".to_string(), |n| n.to_string());

    vec![
        ("Name", provider.name.clone()),
        ("Type", or_dash(&provider.provider_type)),
        ("Default", yes_no(provider.is_default)),
        ("Enabled", yes_no(provider.enabled)),
        ("Verified", yes_no(provider.verified)),
        ("Key", yes_no(provider.has_api_key)),
        ("Base URL", or_dash(&provider.base_url)),
        (
            "Models",
            if provider.models.is_empty() {
                "-".to_string()
            } else {
                provider.models.join(", ")
            },
        ),
        ("Timeout", format!("{}s", provider.timeout_seconds)),
        ("Max tokens", number(provider.max_tokens)),
        ("Context window", number(provider.context_window)),
        (
            "Temperature",
            provider
                .temperature
                .map_or_else(|| "-".to_string(), |t| t.to_string()),
        ),
    ]
}

fn yes_no(value: bool) -> String {
    if value { "yes" } else { "no" }.to_string()
}

/// Flatten a catalogue into one row per offerable model.
///
/// A provider with an empty roster still gets a row. Flat-mapping it away
/// would drop the provider off a listing whose entire job is answering "what
/// can I use here", and an operator who has just pointed a preset at their own
/// `base_url` — which is what suppresses the curated rungs — is precisely the
/// person asking.
fn roster_rows(entries: &[CatalogEntry]) -> Vec<Vec<String>> {
    entries
        .iter()
        .flat_map(|entry| {
            if entry.roster.is_empty() {
                return vec![empty_roster_cells(entry)];
            }
            entry
                .roster
                .iter()
                .map(|model| {
                    vec![
                        entry.id.clone(),
                        model.id.clone(),
                        // Blank, never `0` and never `free`: an absent row means
                        // the curated tables have nothing on this family, which
                        // is the normal state for a scraped id. Both cells are
                        // formatted by the contract, so this table, the Panel's
                        // ladder and the TUI's picker print one number each
                        // rather than three spellings of it.
                        model
                            .capabilities
                            .as_ref()
                            .map(ModelCapabilities::context_window_short)
                            .unwrap_or_default(),
                        model
                            .cost
                            .as_ref()
                            .and_then(RateCard::io_per_mtok_short)
                            .unwrap_or_default(),
                        model.source.as_str().to_string(),
                        lifecycle_cell(model),
                    ]
                })
                .collect()
        })
        .collect()
}

/// The row for a provider that offers nothing yet.
///
/// `discoverable` decides which of the two things to suggest: pointing someone
/// at `--refresh` for one of the six presets with no `/models` endpoint is
/// offering a button that can only ever fail, which is the whole reason the
/// server sends that bit.
fn empty_roster_cells(entry: &CatalogEntry) -> Vec<String> {
    let hint = if entry.discoverable {
        "no models \u{2014} try --refresh"
    } else {
        "no models \u{2014} no /models endpoint; list them by hand"
    };
    vec![
        entry.id.clone(),
        "-".to_string(),
        "-".to_string(),
        "-".to_string(),
        "-".to_string(),
        hint.to_string(),
    ]
}

/// The Status cell for one roster row.
///
/// The lifecycle words are the contract's own `ModelStatus::as_str`, not a
/// second vocabulary invented here. A retired id names its successor because
/// that is the only part of the row an operator can act on: the id they are
/// looking at may already be 400ing.
fn lifecycle_cell(model: &RosterModel) -> String {
    let status = model.lifecycle.status.as_str().to_string();
    match (model.lifecycle.is_deprecated(), &model.lifecycle.successor) {
        (true, Some(successor)) => format!("{status} \u{2192} {successor}"),
        _ => status,
    }
}

/// Render the sweep, one row per provider.
///
/// The three states the server distinguishes are the three this prints: a
/// listing that is current (`live`), a listing that is the last known good
/// snapshot because the live fetch failed (`stale`), and no listing at all
/// (`failed`). Collapsing the middle one into either neighbour is what makes a
/// dated answer look authoritative.
fn refresh_rows<Tz: TimeZone>(rows: &[ModelsRefreshRow], tz: &Tz) -> Vec<Vec<String>>
where
    Tz::Offset: std::fmt::Display,
{
    rows.iter()
        .map(|row| {
            vec![
                row.provider.clone(),
                refresh_state(row).to_string(),
                row.models.len().to_string(),
                refresh_detail(row, tz),
            ]
        })
        .collect()
}

fn refresh_state(row: &ModelsRefreshRow) -> &'static str {
    match row.outcome() {
        RefreshOutcome::Live => "live",
        RefreshOutcome::Stale => "stale",
        // Not a failure: nothing broke, this endpoint publishes no listing at
        // all. Printing `failed` here is the red-row-about-a-healthy-provider
        // the health face was fixed for, arriving through the other door.
        RefreshOutcome::NotApplicable => "n/a",
        RefreshOutcome::Failed => "failed",
    }
}

/// The Detail cell: why the listing is what it is.
///
/// A stale row carries both halves — the failure that stopped the live fetch
/// *and* the age of what is being shown instead — because either one alone
/// reads as a different row than it is.
fn refresh_detail<Tz: TimeZone>(row: &ModelsRefreshRow, tz: &Tz) -> String
where
    Tz::Offset: std::fmt::Display,
{
    let why = row
        .kind
        .map(|kind| failure_detail(&row.provider, kind, row.error.as_deref()));
    let fetched = row.fetched_at.and_then(|at| fetched_stamp(at, tz));

    match (why, fetched) {
        (Some(why), Some(at)) => format!("{why}; snapshot from {at}"),
        (Some(why), None) => why,
        (None, Some(at)) => at,
        (None, None) => String::new(),
    }
}

/// Unix seconds as a local wall clock, or `None` when the value is not a real
/// instant. `fetched_at` is UTC on the wire, and a bare UTC clock in a
/// human-facing column reads as a wrong time rather than as another timezone.
///
/// The timezone is a parameter rather than a hardcoded `Local` so the
/// rendering can be asserted against a fixed offset: a test that converted its
/// expectation the way the code does would agree with any offset, including a
/// wrong one.
fn fetched_stamp<Tz: TimeZone>(fetched_at: u64, tz: &Tz) -> Option<String>
where
    Tz::Offset: std::fmt::Display,
{
    i64::try_from(fetched_at)
        .ok()
        .and_then(|secs| chrono::DateTime::from_timestamp(secs, 0))
        .map(|at| at.with_timezone(tz).format("%Y-%m-%d %H:%M").to_string())
}

/// A human sentence per discovery failure.
///
/// Matched exhaustively on purpose: a seventh kind added to the contract has
/// to be a compile error here, not a row that quietly prints the same sentence
/// as everything else. The whole point of the enum is that "this vendor
/// publishes no catalogue" and "the request timed out" are different answers
/// to *is this worth retrying?*, so each one says which it is.
fn failure_detail(provider: &str, kind: DiscoveryFailureKind, error: Option<&str>) -> String {
    let sentence = match kind {
        // The one failure the operator can act on, and the action is a command
        // they can run: provider keys are resolved from the vault under
        // `ai:<name>`, which is exactly what `aleph secret set` writes to.
        DiscoveryFailureKind::MissingCredential => {
            return format!("no API key \u{2014} run `aleph secret set ai:{provider}`")
        }
        // Never retryable, and there is no button to offer: this provider ships
        // no readable listing, so its models are whatever was configured.
        DiscoveryFailureKind::Unsupported => {
            return "no /models endpoint \u{2014} models must be listed by hand".to_string()
        }
        DiscoveryFailureKind::Transport => "network failure (retryable)",
        DiscoveryFailureKind::Status => "endpoint refused the request",
        DiscoveryFailureKind::Shape => "unreadable listing",
        DiscoveryFailureKind::Timeout => "timed out (retryable)",
    };

    match error {
        Some(detail) => format!("{sentence}: {detail}"),
        None => sentence.to_string(),
    }
}

/// Build the `providers.create` params.
///
/// The ladder is required and may not be empty — `ProviderConfigJson.models`
/// has no serde default and its deserializer rejects an empty list, so an
/// omitted `--model` is an `INVALID_PARAMS` waiting to happen. Refusing here
/// is the same rule `workspace update` follows: clap is where the omission is
/// visible as an omission, and a round trip has nothing to add.
fn create_params(
    name: &str,
    provider_type: &str,
    api_key: &str,
    base_url: Option<&str>,
    models: &[String],
) -> CliResult<CreateParams> {
    if models.is_empty() {
        return Err(CliError::Other(format!(
            "provider '{name}' needs at least one model: pass --model <ID> \
             (repeat it to declare failover rungs)"
        )));
    }

    let mut config = ProviderConfigJson::new(models.to_vec());
    config.protocol = Some(provider_type.to_string());
    config.api_key = Some(api_key.to_string());
    config.base_url = base_url.map(str::to_string);

    Ok(CreateParams {
        name: name.to_string(),
        config,
    })
}

/// Build the `providers.test` params from the provider the server has stored.
///
/// `providers.test` probes a *configuration*, not a name: `TestParams.config`
/// is required. The CLI has only a name, so it reads the stored row back and
/// asks the server to probe that. No key is sent — `name` is present, so the
/// handler resolves it from the vault, and a response never carries one back
/// to send anyway.
fn test_params(provider: &ProviderInfo) -> CliResult<TestParams> {
    if provider.models.is_empty() {
        return Err(CliError::Other(format!(
            "provider '{}' has no model configured; there is nothing to probe",
            provider.name
        )));
    }

    let mut config = ProviderConfigJson::new(provider.models.clone());
    config.enabled = provider.enabled;
    config.protocol = provider.provider_type.clone();
    config.base_url = provider.base_url.clone();
    config.timeout_seconds = Some(provider.timeout_seconds);
    config.max_tokens = provider.max_tokens;
    config.context_window = provider.context_window;
    config.temperature = provider.temperature;

    Ok(TestParams {
        name: Some(provider.name.clone()),
        config,
    })
}

/// List configured AI providers, optionally narrowed to a query.
///
/// The filter is client-side and deliberately so: `providers.list` has no
/// `query` parameter and must not grow one — the rows are already in hand, and
/// a server-side filter would be a second answer to "which row did you mean"
/// that the Panel and the TUI do not share.
pub async fn list(
    server_url: &str,
    config: &CliConfig,
    query: Option<&str>,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("providers.list", None::<()>).await?;

    // `--json` is a raw passthrough and must not be gated on this CLI being
    // able to project the payload; only the table needs it, and there a shape
    // it cannot read is an error worth showing rather than a table of dashes.
    let rows: Vec<Vec<String>> = if json {
        Vec::new()
    } else {
        let providers = serde_json::from_value::<ProviderListResult>(result.clone())?.providers;
        let wrapped: Vec<ProviderRow<'_>> = providers.iter().map(ProviderRow).collect();
        aleph_protocol::providers::rank_rows(&wrapped, query.unwrap_or_default())
            .into_iter()
            .map(|m| row_cells(&providers[m.index]))
            .collect()
    };

    output::print_table(LIST_HEADERS, &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Get details of a specific AI provider
pub async fn get(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = GetParams {
        name: name.to_string(),
    };
    let result: Value = client.call("providers.get", Some(params)).await?;

    let pairs = if json {
        Vec::new()
    } else {
        detail_pairs(&serde_json::from_value::<ProviderGetResult>(result.clone())?.provider)
    };

    output::print_detail(&pairs, json, &result);

    client.close().await?;
    Ok(())
}

/// Build the `providers.update` body that changes only the ladder.
///
/// `providers.update` replaces the stored config wholesale, so every field not
/// being changed has to be re-sent. `api_key` stays absent on purpose: absent
/// means "keep the stored one", and a response never carries a secret back to
/// re-send anyway.
fn update_params(current: &ProviderInfo, models: Vec<String>) -> UpdateParams {
    let mut config = ProviderConfigJson::new(models);
    config.enabled = current.enabled;
    config.protocol = current.provider_type.clone();
    config.base_url = current.base_url.clone();
    config.color = Some(current.color.clone());
    config.timeout_seconds = Some(current.timeout_seconds);
    config.max_tokens = current.max_tokens;
    config.context_window = current.context_window;
    config.temperature = current.temperature;

    UpdateParams {
        name: current.name.clone(),
        config,
    }
}

/// Rewrite a stored provider's model ladder.
///
/// # Why this exists, and why it takes the whole ladder
///
/// `aleph providers models <name> --refresh` reports what a vendor serves now,
/// and until this verb existed there was no way to adopt any of it: `add`
/// demands an API key and refuses to touch an existing row, and nothing else
/// wrote `models`. The discover half of "link, discover, choose" shipped
/// without the choose half, which is the same "no client" defect that kept
/// `providers.modelsRefresh` itself unreachable for a whole round.
///
/// `providers.update` replaces the stored config wholesale, so a partial body
/// is a silent way to blank fields the caller never mentioned. Everything not
/// being changed is therefore read back from the server and re-sent — the same
/// passthrough discipline the Panel and the phone screen use, and for the same
/// reason: this endpoint has no merge semantics to rely on.
///
/// The ladder is ordered and the order is the point: `models[0]` is what a turn
/// naming no model gets, the rest are the failover rungs.
pub async fn set_models(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    models: &[String],
    json: bool,
) -> CliResult<()> {
    // An empty ladder is an `INVALID_PARAMS` the server is guaranteed to
    // answer, so it never leaves the terminal.
    if models.is_empty() {
        return Err(CliError::Other(format!(
            "provider '{name}' needs at least one model: pass --model <ID> \
             (repeat it, first is the default and the rest are failover rungs)"
        )));
    }

    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let stored: Value = client
        .call(
            "providers.get",
            Some(GetParams {
                name: name.to_string(),
            }),
        )
        .await?;
    let current = serde_json::from_value::<ProviderGetResult>(stored)?.provider;

    let result: Value = client
        .call(
            "providers.update",
            Some(update_params(&current, models.to_vec())),
        )
        .await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Provider '{name}' now offers: {}", models.join(", "));
        println!("  default: {}", models[0]);
        if models.len() > 1 {
            println!("  failover: {}", models[1..].join(" → "));
        }
    }

    client.close().await?;
    Ok(())
}

/// Add a new AI provider
#[allow(clippy::too_many_arguments)]
pub async fn add(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    provider_type: &str,
    api_key: &str,
    base_url: Option<&str>,
    models: &[String],
    json: bool,
) -> CliResult<()> {
    // Before connecting: a request the server is guaranteed to refuse is a
    // mistake at the command line, and there is nothing a round trip can add.
    let params = create_params(name, provider_type, api_key, base_url, models)?;

    let (client, _events) = AlephClient::connect(server_url, config).await?;
    let result: Value = client.call("providers.create", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Provider '{name}' added successfully.");
    }

    client.close().await?;
    Ok(())
}

/// Test provider connectivity
pub async fn test(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let stored: Value = client
        .call(
            "providers.get",
            Some(GetParams {
                name: name.to_string(),
            }),
        )
        .await?;
    let params = test_params(&serde_json::from_value::<ProviderGetResult>(stored)?.provider)?;

    let result: Value = client.call("providers.test", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        let outcome: TestResult = serde_json::from_value(result.clone())?;
        if outcome.success {
            match outcome.latency_ms {
                Some(ms) => println!("Provider '{name}' is reachable ({ms} ms)."),
                None => println!("Provider '{name}' is reachable."),
            }
        } else {
            let error = outcome.error.as_deref().unwrap_or("unknown error");
            println!("Provider '{name}' test failed: {error}");
        }
    }

    client.close().await?;
    Ok(())
}

/// Probe every configured provider in one sweep.
///
/// `providers.healthcheck` had no client at all until this one: the server half
/// was complete, registered and tested, and nothing anywhere called it. The
/// deployment that needs it most is the one with no Panel — a headless server
/// and a terminal — which is the same argument that put `modelsRefresh` on the
/// TUI.
pub async fn health(server_url: &str, config: &CliConfig, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let result: Value = client.call("providers.healthcheck", None::<()>).await?;

    let rows: Vec<Vec<String>> = if json {
        Vec::new()
    } else {
        serde_json::from_value::<ProviderHealthResult>(result.clone())?
            .providers
            .iter()
            .map(health_cells)
            .collect()
    };

    output::print_table(HEALTH_HEADERS, &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Render one `providers health` row.
///
/// A skipped row is not a failure, and the two reasons to skip read very
/// differently to an operator: one is their own switch, the other says the
/// endpoint cannot answer this question at all. `enabled` separates them —
/// the server deliberately ships no third field for it.
fn health_cells(row: &ProviderHealthRow) -> Vec<String> {
    let state = if row.skipped {
        if row.enabled {
            "skipped (endpoint opts out)"
        } else {
            "skipped (disabled)"
        }
    } else if row.ok {
        "reachable"
    } else {
        "unreachable"
    };
    vec![
        row.name.clone(),
        state.to_string(),
        row.latency_ms
            .map_or_else(|| "-".to_string(), |ms| format!("{ms} ms")),
        row.error.clone().unwrap_or_else(|| "-".to_string()),
    ]
}

/// Show the models a provider offers, or ask the vendor what it serves now.
///
/// Two different questions, hence the flag rather than two verbs. Without
/// `--refresh` this reads `providers.catalog` and prints each provider's
/// roster — the server's merge of preset rungs, the operator's configured
/// ladder and whatever a previous refresh cached, rendered verbatim (R4).
/// With `--refresh` it sweeps the vendors' `/models` endpoints and reports how
/// each one answered; the ids it discovers land in that same cache, so the
/// plain listing above is where you read them afterwards.
///
/// The view is `all`. A picker wants the narrow default, but an operator
/// asking what a provider offers is usually asking about one they have not
/// finished setting up.
pub async fn models(
    server_url: &str,
    config: &CliConfig,
    name: Option<&str>,
    refresh: bool,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let (headers, result) = if refresh {
        // Narrowing is the server's `provider` param, not a client-side
        // filter: a sweep must not dial vendors the caller did not ask about.
        let params = ModelsRefreshParams {
            provider: name.map(str::to_string),
        };
        let result: Value = client.call("providers.modelsRefresh", Some(params)).await?;
        (REFRESH_HEADERS, result)
    } else {
        let params = CatalogParams::for_view(CatalogView::All);
        let result: Value = client.call("providers.catalog", Some(params)).await?;
        (ROSTER_HEADERS, result)
    };

    let rows: Vec<Vec<String>> = if json {
        Vec::new()
    } else if refresh {
        let sweep: ModelsRefreshResult = serde_json::from_value(result.clone())?;
        refresh_rows(&sweep.providers, &chrono::Local)
    } else {
        // The shared ranker, so `aleph providers models kimi` lands on the same
        // row a bare Enter opens in the Panel and the TUI.
        let items = serde_json::from_value::<CatalogResult>(result.clone())?.items;
        roster_rows(&filter_catalog(&items, name.unwrap_or_default()))
    };

    output::print_table(headers, &rows, json, &result);

    client.close().await?;
    Ok(())
}

/// Set a provider as the default
pub async fn set_default(
    server_url: &str,
    config: &CliConfig,
    name: &str,
    json: bool,
) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = SetDefaultParams {
        name: name.to_string(),
    };
    let result: Value = client.call("providers.setDefault", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Provider '{name}' set as default.");
    }

    client.close().await?;
    Ok(())
}

/// Remove a provider
pub async fn remove(server_url: &str, config: &CliConfig, name: &str, json: bool) -> CliResult<()> {
    let (client, _events) = AlephClient::connect(server_url, config).await?;

    let params = DeleteParams {
        name: name.to_string(),
    };
    let result: Value = client.call("providers.delete", Some(params)).await?;

    if json {
        output::print_json(&result);
    } else {
        println!("Provider '{name}' removed.");
    }

    client.close().await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A stored row in the shape `providers.get` actually returns.
    fn stored_row() -> ProviderInfo {
        serde_json::from_value(serde_json::json!({
            "name": "my-relay",
            "enabled": true,
            "models": ["gpt-5"],
            "model": "gpt-5",
            "provider_type": "openai",
            "has_api_key": true,
            "base_url": "https://relay.example/v1",
            "color": "#123456",
            "timeout_seconds": 120,
            "max_tokens": 4096,
            "context_window": 128_000,
            "temperature": 0.5,
            "is_default": false,
            "verified": true,
        }))
        .expect("a real providers.get row must parse")
    }

    /// The whole point of `set-models`: adopting a discovered id must not
    /// silently blank everything else. `providers.update` replaces the stored
    /// config wholesale, so a body carrying only `models` is a way to wipe the
    /// base URL, the timeout and the tuning of a working provider.
    #[test]
    fn changing_the_ladder_carries_every_other_field_through() {
        let params = update_params(&stored_row(), vec!["gpt-5.6".into(), "gpt-5".into()]);

        assert_eq!(params.name, "my-relay");
        assert_eq!(params.config.models, vec!["gpt-5.6", "gpt-5"]);
        assert_eq!(params.config.protocol.as_deref(), Some("openai"));
        assert_eq!(
            params.config.base_url.as_deref(),
            Some("https://relay.example/v1")
        );
        assert_eq!(params.config.timeout_seconds, Some(120));
        assert_eq!(params.config.max_tokens, Some(4096));
        assert_eq!(params.config.context_window, Some(128_000));
        assert_eq!(params.config.temperature, Some(0.5));
        assert!(params.config.enabled);
        // Absent, not empty: an absent key means "keep the stored one", and a
        // response never carries a secret back to re-send.
        assert!(params.config.api_key.is_none());
    }

    /// The wire body, asserted from what actually serialises rather than from
    /// a literal written beside it — the flat `{name, type, api_key, base_url}`
    /// body this family used to send looked entirely plausible and was refused
    /// by every server that ever received it.
    #[test]
    fn the_update_body_is_the_envelope_the_handler_declares() {
        let params = update_params(&stored_row(), vec!["gpt-5.6".into()]);
        let wire = serde_json::to_value(&params).expect("params must serialise");

        assert_eq!(wire["name"], "my-relay");
        assert!(
            wire.get("config").and_then(|c| c.get("models")).is_some(),
            "the ladder must ride inside `config`, not at the top level: {wire}"
        );

        let decoded: UpdateParams = serde_json::from_value(wire)
            .expect("the handler must accept what the contract encodes");
        assert_eq!(decoded.config.models, vec!["gpt-5.6"]);
    }

    /// The `list` regression itself, driven by a body in the shape
    /// `provider_info_row` builds — not by a `ProviderInfo` constructed beside
    /// the assertion. `type` / `default` are absent from it because the server
    /// has never sent them; a renderer reading those names produces a dash
    /// here, which is what shipped.
    #[test]
    fn a_row_renders_every_column_from_a_real_list_body() {
        let body: ProviderListResult = serde_json::from_value(serde_json::json!({
            "providers": [{
                "name": "openai",
                "enabled": true,
                "models": ["gpt-5", "gpt-5-mini"],
                "model": "gpt-5",
                "provider_type": "openai",
                "has_api_key": true,
                "color": "#10a37f",
                "timeout_seconds": 300,
                "is_default": true,
                "verified": true,
            }]
        }))
        .expect("a real providers.list body must parse");

        assert_eq!(
            row_cells(&body.providers[0]),
            vec!["openai", "openai", "yes"]
        );
        assert_eq!(LIST_HEADERS.len(), row_cells(&body.providers[0]).len());
    }

    /// `providers.healthcheck` had no client, so nothing had ever read its
    /// body. Driven by the shape the handler builds, including the two skip
    /// rows it can produce — a disabled provider and a preset whose endpoint
    /// publishes no `/models`. They differ only by `enabled`, which is exactly
    /// what the server relies on instead of a third field, so a renderer that
    /// ignores it collapses "you turned this off" into "this cannot be checked".
    #[test]
    fn every_health_column_renders_from_a_real_healthcheck_body() {
        let body: ProviderHealthResult = serde_json::from_value(serde_json::json!({
            "providers": [
                { "name": "anthropic", "enabled": true, "ok": true, "skipped": false,
                  "latency_ms": 143 },
                { "name": "chatgpt", "enabled": true, "ok": false, "skipped": true },
                { "name": "groq", "enabled": false, "ok": false, "skipped": true },
                { "name": "openai", "enabled": true, "ok": false, "skipped": false,
                  "error": "401 Unauthorized" },
            ]
        }))
        .expect("a real providers.healthcheck body must parse");

        let rendered: Vec<Vec<String>> = body.providers.iter().map(health_cells).collect();
        assert_eq!(
            rendered,
            vec![
                vec!["anthropic", "reachable", "143 ms", "-"],
                vec!["chatgpt", "skipped (endpoint opts out)", "-", "-"],
                vec!["groq", "skipped (disabled)", "-", "-"],
                vec!["openai", "unreachable", "-", "401 Unauthorized"],
            ]
        );
        assert_eq!(HEALTH_HEADERS.len(), rendered[0].len());
    }

    /// `providers.get` wraps its row in a `provider` key. The command read the
    /// fields off the envelope itself, so every line of the detail view was a
    /// dash regardless of the two misspelled names.
    #[test]
    fn a_detail_renders_from_a_real_get_body() {
        let body: ProviderGetResult = serde_json::from_value(serde_json::json!({
            "provider": {
                "name": "ollama",
                "enabled": true,
                "models": ["qwen3:8b"],
                "model": "qwen3:8b",
                "provider_type": "openai",
                "has_api_key": false,
                "base_url": "http://127.0.0.1:11434/v1",
                "color": "#808080",
                "timeout_seconds": 600,
                "is_default": false,
                "verified": false,
            }
        }))
        .expect("a real providers.get body must parse");

        assert_eq!(
            detail_pairs(&body.provider),
            vec![
                ("Name", "ollama".to_string()),
                ("Type", "openai".to_string()),
                ("Default", "no".to_string()),
                ("Enabled", "yes".to_string()),
                ("Verified", "no".to_string()),
                ("Key", "no".to_string()),
                ("Base URL", "http://127.0.0.1:11434/v1".to_string()),
                ("Models", "qwen3:8b".to_string()),
                ("Timeout", "600s".to_string()),
                ("Max tokens", "-".to_string()),
                ("Context window", "-".to_string()),
                ("Temperature", "-".to_string()),
            ]
        );
    }

    /// What goes on the wire must be `{name, config: {...}}`. This asserts the
    /// serialized request, not a literal built next to it: the flat
    /// `{name, type, api_key, base_url}` body this replaces looked entirely
    /// plausible and was refused by every server that ever received it.
    #[test]
    fn the_create_request_nests_the_config_the_server_deserializes() {
        let wire = serde_json::to_value(
            create_params(
                "relay",
                "openai",
                "sk-test",
                Some("https://relay.example/v1"),
                &["gpt-5".to_string()],
            )
            .expect("a ladder with one rung is valid"),
        )
        .unwrap();

        assert_eq!(
            wire,
            serde_json::json!({
                "name": "relay",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["gpt-5"],
                    "api_key": "sk-test",
                    "base_url": "https://relay.example/v1",
                }
            })
        );

        // And the server can read it back: `models` has no serde default, so
        // this is the half a hand-written literal kept getting wrong.
        let parsed: CreateParams = serde_json::from_value(wire).expect("the server must accept it");
        assert_eq!(parsed.config.models, vec!["gpt-5"]);
    }

    /// An empty ladder is an `INVALID_PARAMS` the server is guaranteed to
    /// answer, so it never leaves the terminal.
    #[test]
    fn a_provider_with_no_model_is_refused_before_the_round_trip() {
        let err = create_params("relay", "openai", "sk-test", None, &[])
            .expect_err("an empty ladder must not reach the server");
        let message = err.to_string();
        assert!(message.contains("relay"), "unexpected: {message}");
        assert!(message.contains("--model"), "unexpected: {message}");
    }

    /// `providers.test` probes a configuration, not a name — the old flat
    /// `{name}` body was missing the whole of `config`. The key is deliberately
    /// absent: `name` is present, so the handler resolves it from the vault.
    #[test]
    fn the_test_request_carries_the_stored_config_and_no_key() {
        let body: ProviderGetResult = serde_json::from_value(serde_json::json!({
            "provider": {
                "name": "openai",
                "enabled": true,
                "models": ["gpt-5", "gpt-5-mini"],
                "model": "gpt-5",
                "provider_type": "openai",
                "has_api_key": true,
                "base_url": "https://api.openai.com/v1",
                "color": "#10a37f",
                "timeout_seconds": 300,
                "is_default": true,
                "verified": true,
            }
        }))
        .expect("a real providers.get body must parse");

        let wire = serde_json::to_value(test_params(&body.provider).expect("a ladder is present"))
            .unwrap();

        assert_eq!(
            wire,
            serde_json::json!({
                "name": "openai",
                "config": {
                    "protocol": "openai",
                    "enabled": true,
                    "models": ["gpt-5", "gpt-5-mini"],
                    "base_url": "https://api.openai.com/v1",
                    "timeout_seconds": 300,
                }
            })
        );

        let parsed: TestParams = serde_json::from_value(wire).expect("the server must accept it");
        assert_eq!(parsed.config.models.len(), 2);
    }

    /// A provider with no ladder cannot be probed, and `TestParams` cannot
    /// express one — so say which it is rather than sending a body that comes
    /// back as a parse error about a field the user never typed.
    #[test]
    fn a_provider_with_no_model_cannot_be_probed() {
        let body: ProviderGetResult = serde_json::from_value(serde_json::json!({
            "provider": { "name": "blank", "has_api_key": false }
        }))
        .expect("a sparse providers.get body must parse");

        let err = test_params(&body.provider).expect_err("nothing to probe");
        assert!(err.to_string().contains("blank"), "unexpected: {err}");
    }

    /// Provenance and retirement come off the roster rows, which is the whole
    /// reason `roster` stopped being a `Vec<String>`: a retired id has to name
    /// its successor, and a discovered id has to admit it was scraped.
    #[test]
    fn a_roster_row_shows_where_the_id_came_from_and_whether_it_is_retired() {
        let body: CatalogResult = serde_json::from_value(serde_json::json!({
            "items": [{
                "id": "openai",
                "display_name": "OpenAI",
                "default_model": "gpt-5",
                "base_url": "https://api.openai.com/v1",
                "protocol": "openai",
                "color": "#10a37f",
                "has_api_key": true,
                "verified": true,
                "enabled": true,
                "is_default": true,
                "auth_kind": "api_key",
                "endpoint": "cloud",
                "discoverable": true,
                "roster": [
                    {
                        "id": "gpt-5",
                        "source": "preset_default",
                        "capabilities": {
                            "context_window": 400000,
                            "max_output_tokens": 128000,
                            "supports_vision": true,
                            "supports_tools": true,
                            "supports_reasoning": true
                        },
                        "cost": {
                            "input_per_mtok": 1.25,
                            "output_per_mtok": 10.0,
                            "basis": "direct"
                        }
                    },
                    {
                        "id": "gpt-4o",
                        "source": "preset_fallback",
                        "lifecycle": { "status": "deprecated", "successor": "gpt-5" }
                    },
                    { "id": "scraped-1", "source": "discovered" }
                ]
            }]
        }))
        .expect("a real providers.catalog body must parse");

        assert_eq!(
            roster_rows(&body.items),
            vec![
                // The two decisive columns, formatted by the contract.
                vec![
                    "openai",
                    "gpt-5",
                    "391K",
                    "$1.25/$10",
                    "preset_default",
                    "active"
                ],
                // A curated id the reference tables happen not to price: blank,
                // not `$0` — an unpriced model is not a free one.
                vec![
                    "openai",
                    "gpt-4o",
                    "",
                    "",
                    "preset_fallback",
                    "deprecated → gpt-5"
                ],
                // A scraped id carries no curated row at all, which is exactly
                // when two empty cells are the honest answer.
                vec!["openai", "scraped-1", "", "", "discovered", "active"],
            ]
        );
        assert_eq!(ROSTER_HEADERS.len(), roster_rows(&body.items)[0].len());
    }

    /// A provider that offers nothing is still a row: flat-mapping it away
    /// would silently drop it from the one listing that answers "what can I
    /// use here". Which hint it gets is the server's `discoverable` bit —
    /// `--refresh` against a provider with no `/models` endpoint can only ever
    /// fail.
    #[test]
    fn a_provider_with_an_empty_roster_still_appears() {
        let body: CatalogResult = serde_json::from_value(serde_json::json!({
            "items": [
                {
                    "id": "relay",
                    "display_name": "relay",
                    "default_model": "",
                    "base_url": "https://relay.example/v1",
                    "protocol": "openai",
                    "color": "#808080",
                    "has_api_key": true,
                    "verified": false,
                    "enabled": true,
                    "is_default": false,
                    "endpoint": "cloud",
                    "discoverable": true,
                    "roster": []
                },
                {
                    "id": "bedrock",
                    "display_name": "Bedrock",
                    "default_model": "",
                    "base_url": "",
                    "protocol": "openai",
                    "color": "#808080",
                    "has_api_key": false,
                    "verified": false,
                    "enabled": false,
                    "is_default": false,
                    "endpoint": "cloud",
                    "discoverable": false,
                    "roster": []
                }
            ]
        }))
        .expect("a real providers.catalog body must parse");

        let rows = roster_rows(&body.items);
        // The hint is the last cell, addressed as such: pinning its index made
        // this assertion fail the day the table grew a column, which says
        // nothing about the behaviour it is here to protect.
        let hint = |row: &Vec<String>| row.last().expect("a row is never empty").clone();
        assert_eq!(rows.len(), 2, "neither provider may vanish");
        assert_eq!(rows[0][0], "relay");
        assert!(
            hint(&rows[0]).contains("--refresh"),
            "unexpected: {:?}",
            rows[0]
        );
        assert_eq!(rows[1][0], "bedrock");
        assert!(
            !hint(&rows[1]).contains("--refresh"),
            "a provider with no /models endpoint must not be sent to refresh: {:?}",
            rows[1]
        );
    }

    /// The sweep's three states, from a body in the shape `handle_models_refresh`
    /// builds. `stale` is the one that has to survive: it carries a listing
    /// *and* a failure, and collapsing it into either neighbour is what makes a
    /// dated answer look authoritative.
    #[test]
    fn a_sweep_row_distinguishes_live_from_stale_from_failed() {
        let sweep: ModelsRefreshResult = serde_json::from_value(serde_json::json!({
            "providers": [
                {
                    "provider": "openai",
                    "ok": true,
                    "fetched_at": 1_760_000_400,
                    "models": [{ "id": "gpt-5" }, { "id": "gpt-5-mini" }]
                },
                {
                    "provider": "groq",
                    "ok": true,
                    "stale": true,
                    "fetched_at": 1_760_000_400,
                    "models": [{ "id": "llama-3.3-70b" }],
                    "kind": "timeout",
                    "error": "deadline exceeded"
                },
                {
                    "provider": "anthropic",
                    "ok": false,
                    "kind": "missing_credential",
                    "error": "no API key"
                },
                {
                    "provider": "amazon-bedrock",
                    "ok": false,
                    "kind": "unsupported",
                    "error": "provider 'amazon-bedrock' publishes no model listing endpoint"
                }
            ]
        }))
        .expect("a real providers.modelsRefresh body must parse");

        let rows = refresh_rows(&sweep.providers, &chrono::Utc);
        assert_eq!(
            rows,
            vec![
                vec![
                    "openai".to_string(),
                    "live".to_string(),
                    "2".to_string(),
                    "2025-10-09 09:00".to_string(),
                ],
                vec![
                    "groq".to_string(),
                    "stale".to_string(),
                    "1".to_string(),
                    "timed out (retryable): deadline exceeded; \
                     snapshot from 2025-10-09 09:00"
                        .to_string(),
                ],
                vec![
                    "anthropic".to_string(),
                    "failed".to_string(),
                    "0".to_string(),
                    "no API key — run `aleph secret set ai:anthropic`".to_string(),
                ],
                // Nothing failed here. This endpoint has no listing to fetch,
                // and a red `failed` next to a provider that is answering
                // requests perfectly is the exact report the health face was
                // fixed for — the sweep just reached it through another door.
                vec![
                    "amazon-bedrock".to_string(),
                    "n/a".to_string(),
                    "0".to_string(),
                    "no /models endpoint — models must be listed by hand".to_string(),
                ],
            ]
        );
        assert_eq!(REFRESH_HEADERS.len(), rows[0].len());
    }

    /// The two kinds nothing can retry get an instruction instead of an error
    /// string. The vault key is `ai:<name>`, which is what `aleph secret set`
    /// writes — the point of the line is that it can be run as printed.
    #[test]
    fn the_unactionable_failures_say_what_to_do_instead_of_what_broke() {
        assert_eq!(
            failure_detail(
                "anthropic",
                DiscoveryFailureKind::MissingCredential,
                Some("vault miss")
            ),
            "no API key — run `aleph secret set ai:anthropic`"
        );
        assert_eq!(
            failure_detail("bedrock", DiscoveryFailureKind::Unsupported, None),
            "no /models endpoint — models must be listed by hand"
        );
    }

    /// `fetched_at` is UTC on the wire; printed as-is it is simply the wrong
    /// time for everyone not on UTC, which reads as a stale row rather than as
    /// another timezone.
    #[test]
    fn the_snapshot_stamp_is_the_fetch_instant_in_the_readers_zone() {
        let east8 = chrono::FixedOffset::east_opt(8 * 3600).expect("valid offset");
        assert_eq!(
            fetched_stamp(1_760_000_400, &east8).as_deref(),
            Some("2025-10-09 17:00")
        );
        assert_eq!(
            fetched_stamp(1_760_000_400, &chrono::Utc).as_deref(),
            Some("2025-10-09 09:00")
        );
    }
}
