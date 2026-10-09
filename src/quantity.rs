//! Physical quantities as types, so units cannot be mixed.

use std::ops::Add;

use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(transparent)]
pub struct Tokens(pub u64);

/// The classes do not overlap except `reasoning`, which is part of `output`.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct TokenClasses {
    pub uncached_input: Tokens,
    pub cache_write: Tokens,
    pub cache_read: Tokens,
    pub output: Tokens,
    /// `None` when no log records it.
    pub reasoning: Option<Tokens>,
}

#[derive(Clone, Copy, Debug, PartialEq, PartialOrd, Deserialize, Serialize)]
#[serde(transparent)]
pub struct GramsCo2ePerKwh(pub f64);

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct WattHours(pub f64);

#[derive(Clone, Copy, Debug, Default, PartialEq, PartialOrd, Serialize)]
#[serde(transparent)]
pub struct GramsCo2e(pub f64);

impl Add for WattHours {
    type Output = WattHours;
    fn add(self, other: WattHours) -> WattHours {
        WattHours(self.0 + other.0)
    }
}

impl Add for GramsCo2e {
    type Output = GramsCo2e;
    fn add(self, other: GramsCo2e) -> GramsCo2e {
        GramsCo2e(self.0 + other.0)
    }
}
