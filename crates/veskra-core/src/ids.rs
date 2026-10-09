//! Stable public identifiers. IDs are 128-bit and serialize as hyphenated UUID strings.
//!
//! IDs are immutable once assigned and are never reused within a document.
//! Deterministic construction (`from_u128`) exists for tests and replay fixtures.

use serde::{Deserialize, Serialize};
use std::fmt;
use uuid::Uuid;

macro_rules! id_type {
    ($(#[$m:meta])* $name:ident) => {
        $(#[$m])*
        #[derive(Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(Uuid);

        impl $name {
            /// New random (v4) identifier.
            pub fn new_random() -> Self {
                Self(Uuid::new_v4())
            }
            /// Deterministic identifier for fixtures and replay.
            pub const fn from_u128(v: u128) -> Self {
                Self(Uuid::from_u128(v))
            }
            pub const fn as_u128(&self) -> u128 {
                self.0.as_u128()
            }
            /// Parse the canonical hyphenated form.
            pub fn parse(s: &str) -> Result<Self, crate::VeskraError> {
                Uuid::parse_str(s).map(Self).map_err(|_| {
                    crate::VeskraError::new(
                        crate::ErrorCode::InvalidArgument,
                        concat!("malformed ", stringify!($name)),
                    )
                })
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                fmt::Display::fmt(&self.0.hyphenated(), f)
            }
        }

        impl fmt::Debug for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, concat!(stringify!($name), "({})"), self.0.hyphenated())
            }
        }
    };
}

id_type!(
    /// Identity of a document (stable across save/reopen).
    DocumentId
);
id_type!(
    /// Identity of a layer node (raster, vector-stroke or group).
    LayerId
);
id_type!(
    /// Identity of a committed stroke record.
    StrokeId
);
id_type!(
    /// Identity of an engine session (runtime only, never persisted).
    SessionId
);
id_type!(
    /// Identity of a background job such as an export (runtime only).
    JobId
);

/// Deterministic ID source: splitmix64-based, used by headless tests and replay so that
/// documents created from the same command script have identical IDs.
#[derive(Debug, Clone)]
pub struct DeterministicIds {
    state: u64,
}

impl DeterministicIds {
    pub fn new(seed: u64) -> Self {
        Self { state: seed }
    }

    fn next_u64(&mut self) -> u64 {
        self.state = self.state.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.state;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn next_u128(&mut self) -> u128 {
        ((self.next_u64() as u128) << 64) | self.next_u64() as u128
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn id_roundtrip_display_parse_serde() {
        let id = LayerId::from_u128(0x0123_4567_89ab_cdef_0011_2233_4455_6677);
        let s = id.to_string();
        assert_eq!(s, "01234567-89ab-cdef-0011-223344556677");
        assert_eq!(LayerId::parse(&s).unwrap(), id);
        let json = serde_json::to_string(&id).unwrap();
        assert_eq!(json, format!("\"{s}\""));
        let back: LayerId = serde_json::from_str(&json).unwrap();
        assert_eq!(back, id);
    }

    #[test]
    fn malformed_id_is_typed_error() {
        let e = StrokeId::parse("not-a-uuid").unwrap_err();
        assert_eq!(e.code, crate::ErrorCode::InvalidArgument);
    }

    #[test]
    fn deterministic_ids_repeat() {
        let mut a = DeterministicIds::new(7);
        let mut b = DeterministicIds::new(7);
        for _ in 0..10 {
            assert_eq!(a.next_u128(), b.next_u128());
        }
        assert_ne!(DeterministicIds::new(1).next_u128(), DeterministicIds::new(2).next_u128());
    }
}
