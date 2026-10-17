//! Core mentions alephcore::providers in docs are inert.
// use alephcore::*;
use super::local::*;
use crate::local::Thing;
pub use aleph_protocol::Presentation;
use aleph_slot_api as api;
use api::{hooks::HookEvent, search::SearchResult as ResultRow};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, sync::Arc};
extern crate serde_json as json;
fn allowed() {
    let _ = std::mem::size_of::<api::hooks::HookEvent>();
    let _ = "use alephcore::*;";
    json::json!({});
}
mod local {
    pub struct Thing;
}
