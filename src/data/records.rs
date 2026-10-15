use jiff::civil::Date;
use serde::{Deserialize, Deserializer, Serialize};

use crate::quantity::GramsCo2ePerKwh;

/// Best first, so the weakest of several labels is their maximum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Deserialize, Serialize)]
pub enum Label {
    /// Disclosed by the provider for this model.
    M,
    /// Published by a company, not verified.
    C,
    /// Independent research.
    I,
    /// The tool's default, with a range.
    D,
    /// Spend-based, a cross-check only.
    S,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Weights {
    Open,
    Closed,
    Unknown,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Five<T> {
    pub lower_bound: T,
    pub low_case: T,
    pub base_case: T,
    pub high_case: T,
    pub upper_bound: T,
}

impl<T: Copy> Five<T> {
    pub fn same(value: T) -> Five<T> {
        Five {
            lower_bound: value,
            low_case: value,
            base_case: value,
            high_case: value,
            upper_bound: value,
        }
    }

    pub fn map<U>(&self, f: impl Fn(T) -> U) -> Five<U> {
        Five {
            lower_bound: f(self.lower_bound),
            low_case: f(self.low_case),
            base_case: f(self.base_case),
            high_case: f(self.high_case),
            upper_bound: f(self.upper_bound),
        }
    }

    pub fn zip<U: Copy, V>(&self, other: &Five<U>, f: impl Fn(T, U) -> V) -> Five<V> {
        Five {
            lower_bound: f(self.lower_bound, other.lower_bound),
            low_case: f(self.low_case, other.low_case),
            base_case: f(self.base_case, other.base_case),
            high_case: f(self.high_case, other.high_case),
            upper_bound: f(self.upper_bound, other.upper_bound),
        }
    }

    pub(super) fn values(&self) -> [T; 5] {
        [
            self.lower_bound,
            self.low_case,
            self.base_case,
            self.high_case,
            self.upper_bound,
        ]
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Factor {
    pub lower_bound: f64,
    pub low_case: f64,
    pub base_case: f64,
    pub high_case: f64,
    pub upper_bound: f64,
    pub label: Label,
    pub sources: Vec<String>,
    pub basis: String,
}

impl Factor {
    #[must_use]
    pub fn five(&self) -> Five<f64> {
        Five {
            lower_bound: self.lower_bound,
            low_case: self.low_case,
            base_case: self.base_case,
            high_case: self.high_case,
            upper_bound: self.upper_bound,
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Source {
    pub title: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub url: Option<String>,
    pub publisher: String,
    /// As precise as the source states it: a year, a month or a day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub published: Option<String>,
    #[serde(deserialize_with = "toml_date")]
    pub accessed: Date,
    /// The SPDX identifier of the licence of data taken from the source.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub licence: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sha256: Option<String>,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ParameterSet {
    pub id: String,
    pub description: String,
    #[serde(deserialize_with = "toml_date")]
    pub reviewed: Date,
    pub a_p_uj_per_pair: Factor,
    pub a_d_uj_per_pair: Factor,
    pub b_in_j_per_token: Factor,
    pub b_out_j_per_token: Factor,
    pub c0_mj_per_cached_token: Factor,
    pub idle: Factor,
}

impl ParameterSet {
    pub(super) fn factors(&self) -> [(&'static str, &Factor); 6] {
        [
            ("a_p_uj_per_pair", &self.a_p_uj_per_pair),
            ("a_d_uj_per_pair", &self.a_d_uj_per_pair),
            ("b_in_j_per_token", &self.b_in_j_per_token),
            ("b_out_j_per_token", &self.b_out_j_per_token),
            ("c0_mj_per_cached_token", &self.c0_mj_per_cached_token),
            ("idle", &self.idle),
        ]
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Model {
    pub id: String,
    /// An exact model name, or a prefix followed by `*`.
    pub pattern: String,
    pub parameter_set: String,
    /// The list price whose weight terms replace the set's `b_in` and
    /// `b_out`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price: Option<String>,
    pub weights: Weights,
    pub label: Label,
    pub sources: Vec<String>,
    pub basis: String,
    #[serde(deserialize_with = "toml_date")]
    pub reviewed: Date,
}

/// A closed model's list price, from which its weight terms are derived.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Price {
    pub id: String,
    pub calibration: String,
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
    /// Long-context prices, lowest threshold first.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tier: Vec<PriceTier>,
    pub label: Label,
    pub sources: Vec<String>,
    pub basis: String,
    #[serde(deserialize_with = "toml_date")]
    pub reviewed: Date,
}

/// The prices of a call whose prompt, uncached input, cache writes and
/// cache reads together, exceeds `above_prompt_tokens`.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PriceTier {
    pub above_prompt_tokens: u64,
    pub input_usd_per_mtok: f64,
    pub output_usd_per_mtok: f64,
}

/// How a list price converts to weight energy: the weight energy per USD
/// that hosts of open-weight models sell for a reference call, divided by
/// how far a closed lab's price sits above theirs.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct PriceCalibration {
    pub id: String,
    pub description: String,
    #[serde(deserialize_with = "toml_date")]
    pub reviewed: Date,
    /// The reference call: new input and output tokens, no cache.
    pub chat_input_tokens: u64,
    pub chat_output_tokens: u64,
    /// Weight energy per new input and per output token at the anchor,
    /// whose ratio splits a price's energy into `b_in` and `b_out`.
    pub anchor_in_j: f64,
    pub anchor_out_j: f64,
    /// Node weight energy at load per USD of the reference call's price.
    pub wh_per_usd: Factor,
    /// A closed lab's price over the hosts' price for the same serving
    /// cost, at least 1.
    pub markup: Factor,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Sites {
    List(Vec<String>),
    /// Every site of a provider the shipped data names.
    Named,
}

#[derive(Clone, Debug, PartialEq, Default)]
pub enum BoundsSites {
    #[default]
    None,
    List(Vec<String>),
    /// Every geography that a provider the shipped data names lists in
    /// its sites or bounds sites.
    Listed,
    /// Every geography in the shipped grid data.
    All,
}

impl Serialize for Sites {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            Sites::List(sites) => sites.serialize(serializer),
            Sites::Named => serializer.serialize_str("named"),
        }
    }
}

impl Serialize for BoundsSites {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        match self {
            BoundsSites::None => [0_u8; 0].serialize(serializer),
            BoundsSites::List(sites) => sites.serialize(serializer),
            BoundsSites::Listed => serializer.serialize_str("listed"),
            BoundsSites::All => serializer.serialize_str("all"),
        }
    }
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Provider {
    /// The provider's name as the logs record it.
    pub id: String,
    #[serde(deserialize_with = "sites")]
    pub sites: Sites,
    #[serde(default, deserialize_with = "bounds_sites")]
    pub bounds_sites: BoundsSites,
    pub label: Label,
    pub sources: Vec<String>,
    pub basis: String,
    #[serde(deserialize_with = "toml_date")]
    pub reviewed: Date,
    pub pue: Factor,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Geography {
    pub id: String,
    pub name: String,
    pub year: i16,
    pub g_co2e_per_kwh: GramsCo2ePerKwh,
    pub basis: String,
    pub label: Label,
    pub sources: Vec<String>,
    #[serde(deserialize_with = "toml_date")]
    pub reviewed: Date,
}

/// The parameters of a finding of the report; each finding has its own.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Finding {
    pub id: String,
    /// `long-context`: the cache-read tokens a call must exceed.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cached_tokens: Option<u64>,
    pub label: Label,
    pub sources: Vec<String>,
    pub basis: String,
    #[serde(deserialize_with = "toml_date")]
    pub reviewed: Date,
}

/// What a user of one client does about one finding. The pair of `finding`
/// and `client` identifies the record, so an override can add the actions
/// of a user adapter's client.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct ClientAction {
    pub finding: String,
    pub client: String,
    /// Each names the client's command or setting as the client names it.
    pub steps: Vec<String>,
    pub label: Label,
    pub sources: Vec<String>,
    pub basis: String,
    #[serde(deserialize_with = "toml_date")]
    pub reviewed: Date,
}

impl ClientAction {
    #[must_use]
    pub fn id(&self) -> String {
        action_id(&self.finding, &self.client)
    }
}

#[must_use]
pub fn action_id(finding: &str, client: &str) -> String {
    format!("{finding}/{client}")
}

/// Where a client without an adapter keeps its logs, so a report can say
/// that they were found but not read.
#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Location {
    pub id: String,
    /// Checked in order; the first that exists on this OS is shown.
    pub paths: Vec<LocationPath>,
    pub label: Label,
    pub sources: Vec<String>,
    pub basis: String,
    #[serde(deserialize_with = "toml_date")]
    pub reviewed: Date,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocationPath {
    pub os: Vec<Os>,
    /// `/`-separated; starts with `~` for the home directory, or with
    /// `$NAME` or `%NAME%` for an environment variable.
    pub path: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Deserialize, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Os {
    Linux,
    Macos,
    Windows,
}

impl Os {
    /// The OS this binary runs on; other Unix systems count as Linux,
    /// whose paths they share.
    #[must_use]
    pub fn current() -> Os {
        if cfg!(windows) {
            Os::Windows
        } else if cfg!(target_os = "macos") {
            Os::Macos
        } else {
            Os::Linux
        }
    }
}

fn toml_date<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Date, D::Error> {
    use serde::de::Error as _;
    let datetime = toml::value::Datetime::deserialize(deserializer)?;
    match (datetime.date, datetime.time, datetime.offset) {
        (Some(date), None, None) => Date::new(
            date.year.cast_signed(),
            date.month.cast_signed(),
            date.day.cast_signed(),
        )
        .map_err(D::Error::custom),
        _ => Err(D::Error::custom(format!(
            "expected a date such as 2026-10-06, found {datetime}"
        ))),
    }
}

#[derive(Deserialize)]
#[serde(untagged)]
enum ListOrWord {
    List(Vec<String>),
    Word(String),
}

fn sites<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Sites, D::Error> {
    use serde::de::Error as _;
    match ListOrWord::deserialize(deserializer)? {
        ListOrWord::List(sites) => Ok(Sites::List(sites)),
        ListOrWord::Word(word) if word == "named" => Ok(Sites::Named),
        ListOrWord::Word(word) => Err(D::Error::custom(format!(
            "sites is {word:?}; expected a list of geography ids or \"named\""
        ))),
    }
}

fn bounds_sites<'de, D: Deserializer<'de>>(deserializer: D) -> Result<BoundsSites, D::Error> {
    use serde::de::Error as _;
    match ListOrWord::deserialize(deserializer)? {
        ListOrWord::List(sites) => Ok(BoundsSites::List(sites)),
        ListOrWord::Word(word) if word == "listed" => Ok(BoundsSites::Listed),
        ListOrWord::Word(word) if word == "all" => Ok(BoundsSites::All),
        ListOrWord::Word(word) => Err(D::Error::custom(format!(
            "bounds_sites is {word:?}; expected a list of geography ids, \"listed\" or \"all\""
        ))),
    }
}
