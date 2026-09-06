//! Newtypes for the two CDP identifiers this crate routes on.
//!
//! Both are `#[serde(transparent)]`. For a single-field tuple struct like these, serde's derive
//! already serialises the wire form as the bare inner value (`"S1"`, not `{"0": "S1"}`) via
//! `serialize_newtype_struct`/`deserialize_newtype_struct` — the attribute does not change that
//! for either of these types today, and `ids_are_transparent_over_serde_and_errors_name_what_failed`
//! in `tests/smoke.rs` only proves the shape serde already gives newtypes for free, not something
//! this attribute adds (verified: removing `#[serde(transparent)]` from `SessionId` still passes
//! that test). What the attribute actually buys, verified by compiling a two-field version of this
//! struct: `#[serde(transparent)]` requires "at most one transparent field", so it turns "someone
//! adds a second field to `SessionId`/`TargetId` later" into a compile error
//! (`#[serde(transparent)] requires struct to have at most one transparent field`) instead of a
//! silent wire-format change to `{"0": "…", "1": …}` that a peer would answer with an opaque parse
//! error rather than a refusal we could read. Kept for that reason, not for the wire shape itself.

use std::fmt;

use serde::{Deserialize, Serialize};

/// A flat CDP session, as returned by `Target.attachToTarget{flatten:true}`.
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct SessionId(pub String);

/// A CDP target (a tab, in every use this crate has).
#[derive(Clone, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TargetId(pub String);

macro_rules! id_impls {
    ($t:ty) => {
        impl $t {
            /// The bare identifier, for building JSON params.
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }
        impl fmt::Display for $t {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str(&self.0)
            }
        }
        impl From<String> for $t {
            fn from(s: String) -> Self {
                Self(s)
            }
        }
        impl From<&str> for $t {
            fn from(s: &str) -> Self {
                Self(s.to_string())
            }
        }
        impl AsRef<str> for $t {
            fn as_ref(&self) -> &str {
                &self.0
            }
        }
    };
}

id_impls!(SessionId);
id_impls!(TargetId);
