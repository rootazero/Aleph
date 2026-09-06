//! Newtypes for the two CDP identifiers this crate routes on.
//!
//! Both are `#[serde(transparent)]`: a `sessionId` on the wire is a bare JSON string, and a struct
//! that serialised as `{"0": "…"}` would be a silently wrong frame that the peer answers with a
//! parse error rather than a refusal we can read.

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
