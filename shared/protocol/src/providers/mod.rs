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
//!   the TUI agree about which row a bare Enter selects.

pub mod catalog;
pub mod search;
pub mod wire;

pub use catalog::{
    DiscoveredModel, ModelCapabilities, ModelLifecycle, ModelSource, ModelStatus, RateBasis,
    RateCard, RosterModel,
};
pub use search::{filter_catalog, rank_entries, rank_models, EntryMatch, MatchRank};
pub use wire::{
    deserialize_models, AuthKind, CatalogEntry, CatalogParams, CatalogResult, CatalogView,
    CreateParams, DeleteParams, DiscoveryFailureKind, GetParams, ModelsRefreshParams,
    ModelsRefreshResult, ModelsRefreshRow, OAuthStatus, ProviderConfigJson, ProviderGetResult,
    ProviderHealthRow, ProviderInfo, ProviderListResult, SetDefaultParams, TestParams, TestResult,
    UpdateParams,
};
