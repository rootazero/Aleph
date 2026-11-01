//! Run the existing source census without the monolithic Core unit harness.
pub use alephcore::sync_primitives;

#[path = "../src/gateway/lane.rs"]
pub(crate) mod lane;
#[path = "../src/gateway/method_admin.rs"]
mod method_admin;
mod gateway {
    pub(crate) use crate::lane;
}
#[path = "../src/utils/source_scan.rs"]
pub(crate) mod source_scan;
mod utils {
    pub(crate) use crate::source_scan;
}
#[path = "../src/gateway/method_census.rs"]
mod method_census;
