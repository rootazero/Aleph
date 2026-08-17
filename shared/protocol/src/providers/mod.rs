//! The `providers.*` contract: LLM provider CRUD, the preset catalogue, live
//! model discovery, and the matcher every picker filters through.
//!
//! Split three ways because they answer three different questions:
//!
//! * [`catalog`] — *what is true about a model* (capabilities, price,
//!   lifecycle, provenance). The wire projection of `alephcore`'s four curated
//!   reference tables, which re-exports these types so the table literals and
//!   the wire cannot describe different structs.
//! * [`wire`] — *what the RPC family sends and receives*. Shared by all four
//!   crates that speak it, because two of them are forbidden from depending on
//!   `alephcore` and each had shipped a permanently-broken hand copy.
//! * [`search`] — *which rows the user meant*. One matcher, so the Panel and
//!   the TUI agree about which row a bare Enter selects, and — through the
//!   [`search::Searchable`] trait — so the generation/embedding preset grids
//!   rank identically without keeping a second copy of the rule.
//!
//! [`generation`] sits alongside them for the sibling `generation_providers.*`
//! family, which had one client instead of three and the same defect for it:
//! a field the server sends that the client never declared.

pub mod catalog;
pub mod generation;
pub mod search;
pub mod wire;

pub use catalog::{
    DiscoveredModel, ModelCapabilities, ModelLifecycle, ModelSource, ModelStatus, RateBasis,
    RateCard, RosterModel,
};
pub use generation::{GenerationPresetRow, GenerationSettings};
pub use search::{
    filter_catalog, filter_rows, rank_entries, rank_models, rank_rows, EntryMatch, MatchRank,
    RowMatch, Searchable,
};
pub use wire::{
    deserialize_models, AuthKind, CatalogEntry, CatalogParams, CatalogResult, CatalogView,
    CreateParams, DeleteParams, DiscoveryFailureKind, GetParams, ModelsRefreshParams,
    ModelsRefreshResult, ModelsRefreshRow, OAuthStatus, ProviderConfigJson, ProviderGetResult,
    ProviderHealthResult, ProviderHealthRow, ProviderInfo, ProviderListResult, RefreshOutcome,
    SetDefaultParams, TestParams, TestResult, UpdateParams,
};
