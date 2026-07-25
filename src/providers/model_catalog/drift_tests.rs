//! Cross-table drift guards.
//!
//! Aleph's model reference data lives in four tables that nobody forces to
//! agree: `presets::PROFILES` (which model each provider ships as its
//! default), `capabilities::CAPABILITY_TABLE` (window / max-output),
//! `pricing::PRICE_TABLE` (rates) and `lifecycle::LIFECYCLE_TABLE` (retired
//! ids). They have drifted repeatedly, and every time the fix was a manual
//! "refresh round" that noticed the drift by reading all four side by side.
//!
//! These tests do that reading. They are the actual deliverable of this round:
//! the data will drift again, and next time the build says so.
//!
//! Each guard carries an explicit escape hatch rather than a blanket
//! `#[ignore]`, because the hatch is itself useful documentation — the
//! `UNCATALOGUED_FAMILIES` list below *is* the answer to "which providers
//! won't show a context window in the picker, and why".

use super::capabilities::capabilities_for;
use super::lifecycle::lifecycle_for;
use crate::pricing::rate_card;
use crate::providers::presets::{get_preset, ProviderPreset, PRESETS};

/// Providers whose model families Aleph carries no verified capability data
/// for. They fall back to `CONSERVATIVE_CONTEXT_WINDOW`, which is a real (if
/// pessimistic) answer — the alternative is inventing window sizes.
///
/// Entries are keyed by preset id. Removing one is an improvement; adding one
/// should come with a note on why the figure could not be confirmed.
const UNCATALOGUED_FAMILIES: &[(&str, &str)] = &[
    ("baichuan", "Baichuan4 window not published in English docs"),
    (
        "hunyuan",
        "hunyuan-pro window varies by Tencent Cloud region",
    ),
    ("spark", "iFlytek 4.0Ultra window not published"),
    ("inflection", "inflection_3_pi window not published"),
    (
        "nous",
        "Hermes-3 is self-hosted; window depends on the deployment",
    ),
    (
        "lmstudio",
        "`local-model` is a placeholder for whatever is loaded",
    ),
    ("t8star", "relay preset: model chosen per deployment"),
    ("anyscale", "relay preset: model chosen per deployment"),
    ("replicate", "model chosen per deployment"),
    ("lepton", "relay preset: model chosen per deployment"),
];

/// Model ids that only exist on one vendor-specific endpoint and have no
/// public capability or price data. They are kept in their preset's fallback
/// chain because the endpoint accepts them; they are exempt from the coverage
/// guards because there is nothing verifiable to record.
const ENDPOINT_LOCAL_ALIASES: &[(&str, &str)] = &[(
    "k2p5",
    "kimi-for-coding subscription endpoint alias; not on the public Kimi catalog",
)];

fn is_uncatalogued(preset_id: &str) -> bool {
    UNCATALOGUED_FAMILIES.iter().any(|(id, _)| *id == preset_id)
}

fn is_endpoint_local(model: &str) -> bool {
    ENDPOINT_LOCAL_ALIASES
        .iter()
        .any(|(id, _)| model.eq_ignore_ascii_case(id))
}

/// Every model id a preset can put on the wire without the operator naming it.
fn advertised_models(preset: &ProviderPreset) -> Vec<&'static str> {
    let mut out = vec![preset.default_model];
    out.extend(preset.fallback_models.iter().copied());
    if let Some(aux) = preset.default_aux_model {
        out.push(aux);
    }
    out.retain(|m| !m.is_empty());
    out.sort_unstable();
    out.dedup();
    out
}

/// A preset with no `default_model` is not a preset — `list_models` skips the
/// empty id, the catalog RPC renders a blank cell, and selecting the provider
/// sends `model: ""` to the vendor.
#[test]
fn every_preset_declares_a_default_model() {
    let empty: Vec<&str> = PRESETS
        .iter()
        .filter(|(_, p)| p.default_model.is_empty() && !p.requires_explicit_model)
        .map(|(name, _)| *name)
        .collect();
    assert!(
        empty.is_empty(),
        "presets with no default_model and no `requires_explicit_model()` \
         (unusable out of the box, and nothing says so): {empty:?}"
    );
}

/// The inverse: `requires_explicit_model()` is a statement about a relay with
/// no fixed roster. A preset that both declares a default *and* claims to need
/// one is contradicting itself, and the surfaces would show both.
#[test]
fn byo_model_presets_do_not_also_ship_a_default() {
    let contradictory: Vec<&str> = PRESETS
        .iter()
        .filter(|(_, p)| p.requires_explicit_model && !p.default_model.is_empty())
        .map(|(name, _)| *name)
        .collect();
    assert!(
        contradictory.is_empty(),
        "presets claiming requires_explicit_model while shipping a default: {contradictory:?}"
    );
}

/// The default must be a model the vendor still serves. This is the guard that
/// would have caught `deepseek-chat` on the day it was retired instead of
/// after someone noticed.
#[test]
fn no_preset_defaults_to_a_retired_model() {
    let mut stale = Vec::new();
    for (name, preset) in PRESETS.iter() {
        let life = lifecycle_for(preset.default_model);
        if life.is_deprecated() {
            stale.push(format!(
                "{name} defaults to {} (retired; successor {:?})",
                preset.default_model, life.successor
            ));
        }
    }
    assert!(
        stale.is_empty(),
        "presets defaulting to retired models: {stale:#?}"
    );
}

/// A preset's curated fallback chain exists to be tried when the default
/// fails; a retired id in it just burns a retry.
#[test]
fn no_preset_lists_a_retired_fallback() {
    let mut stale = Vec::new();
    for (name, preset) in PRESETS.iter() {
        for model in preset.fallback_models {
            if lifecycle_for(model).is_deprecated() {
                stale.push(format!("{name} → {model}"));
            }
        }
    }
    assert!(
        stale.is_empty(),
        "retired models in fallback chains: {stale:#?}"
    );
}

/// The first fallback is the default by convention across every preset that
/// declares a chain — it is what makes "retry the same model once" the first
/// recovery step. A refresh that bumps `default_model` and forgets the chain
/// silently demotes the new flagship to second choice.
#[test]
fn fallback_chain_leads_with_the_default_model() {
    let mut broken = Vec::new();
    for (name, preset) in PRESETS.iter() {
        let Some(first) = preset.fallback_models.first() else {
            continue;
        };
        if !first.eq_ignore_ascii_case(preset.default_model) {
            broken.push(format!(
                "{name}: default={} but fallback[0]={first}",
                preset.default_model
            ));
        }
    }
    assert!(
        broken.is_empty(),
        "fallback chains out of step with their default: {broken:#?}"
    );
}

/// Every model a preset advertises should resolve to a capability row.
/// Without one the context budget falls back to `CONSERVATIVE_CONTEXT_WINDOW`
/// (128K) — which silently over-compresses a 1M-window model, the exact
/// failure the 2026-07-17 window corrections were about.
#[test]
fn advertised_models_have_capability_rows() {
    let mut missing = Vec::new();
    for (name, preset) in PRESETS.iter() {
        if is_uncatalogued(name) {
            continue;
        }
        for model in advertised_models(preset) {
            if !is_endpoint_local(model) && capabilities_for(model).is_none() {
                missing.push(format!("{name} → {model}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "advertised models with no capability row (they fall back to a \
         conservative 128K window — add a row, or add the preset to \
         UNCATALOGUED_FAMILIES with a reason): {missing:#?}"
    );
}

/// If a model's vendor is one Aleph prices at all, the model must have a rate.
///
/// Scoped deliberately: demanding a rate for *every* preset would demand rates
/// for vendors Aleph has no price table for, which is a different (and
/// legitimate) gap. What this catches is the drift that actually happens — a
/// new default model added to an already-priced vendor without its price row,
/// leaving the flagship at `CostStatus::Unknown` while its predecessor was
/// priced.
#[test]
fn advertised_models_of_priced_vendors_have_rates() {
    let mut missing = Vec::new();
    for (name, preset) in PRESETS.iter() {
        for model in advertised_models(preset) {
            // "Priced vendor" is defined by the outcome, not by peeking into
            // the private table: some *other* model of this provider prices,
            // so this one should too.
            let vendor_is_priced = advertised_models(preset)
                .iter()
                .any(|m| rate_card(name, m).is_some());
            if vendor_is_priced && !is_endpoint_local(model) && rate_card(name, model).is_none() {
                missing.push(format!("{name} → {model}"));
            }
        }
    }
    assert!(
        missing.is_empty(),
        "models of an otherwise-priced provider with no rate row: {missing:#?}"
    );
}

/// The aux model is what summarisation / classification runs on. Pointing it at
/// something more expensive than the primary defeats its entire purpose, and is
/// an easy slip when a cheap tier is renamed.
#[test]
fn aux_model_is_not_pricier_than_the_default() {
    let mut inverted = Vec::new();
    for (name, preset) in PRESETS.iter() {
        let Some(aux) = preset.default_aux_model else {
            continue;
        };
        let (Some(aux_rate), Some(default_rate)) =
            (rate_card(name, aux), rate_card(name, preset.default_model))
        else {
            continue;
        };
        let blended = |c: &crate::pricing::RateCard| {
            c.input_per_mtok.unwrap_or(0.0) + c.output_per_mtok.unwrap_or(0.0)
        };
        if blended(&aux_rate) > blended(&default_rate) {
            inverted.push(format!(
                "{name}: aux={aux} (${:.2}) costs more than default={} (${:.2})",
                blended(&aux_rate),
                preset.default_model,
                blended(&default_rate)
            ));
        }
    }
    assert!(
        inverted.is_empty(),
        "aux models pricier than their primary: {inverted:#?}"
    );
}

/// Aliases must resolve to the same profile they were declared on — the lazy
/// alias expansion in `presets::registry` is the only thing keeping
/// `get_preset("kimi") == get_preset("moonshot")` true.
#[test]
fn declared_aliases_resolve_to_their_profile() {
    for (name, preset) in PRESETS.iter() {
        for alias in preset.aliases {
            let resolved = get_preset(alias)
                .unwrap_or_else(|| panic!("alias {alias:?} of {name} resolves to nothing"));
            assert_eq!(
                resolved.default_model, preset.default_model,
                "alias {alias:?} does not resolve to the {name} profile"
            );
        }
    }
}

/// Every exemption must still name something real — otherwise it outlives the
/// thing it exempted and silently widens the hole it was cut for.
#[test]
fn exemptions_still_name_something_real() {
    for (id, reason) in UNCATALOGUED_FAMILIES {
        assert!(
            get_preset(id).is_some(),
            "UNCATALOGUED_FAMILIES names {id:?} ({reason}) but no such preset exists"
        );
    }
    for (model, reason) in ENDPOINT_LOCAL_ALIASES {
        let still_advertised = PRESETS.iter().any(|(_, p)| {
            advertised_models(p)
                .iter()
                .any(|m| m.eq_ignore_ascii_case(model))
        });
        assert!(
            still_advertised,
            "ENDPOINT_LOCAL_ALIASES exempts {model:?} ({reason}) but no preset advertises it"
        );
    }
}
