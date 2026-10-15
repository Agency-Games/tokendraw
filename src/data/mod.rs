//! An override record replaces the shipped record with the same id whole,
//! never field by field, or adds a record with a new id.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use jiff::civil::Date;
use serde::Serialize;
use thiserror::Error;

use crate::adapters::Role;
use crate::quantity::GramsCo2ePerKwh;

mod check;
mod derive;
mod load;
mod records;

use derive::priced_set_id;
pub use load::{Action, Override, Version, load};
pub use records::*;

/// The shipped data files: their path below `data/` and their contents.
pub const SHIPPED: [(&str, &str); 10] = [
    ("data.toml", include_str!("../../data/data.toml")),
    (
        "energy/parameter-sets.toml",
        include_str!("../../data/energy/parameter-sets.toml"),
    ),
    (
        "energy/open-weight-sets.toml",
        include_str!("../../data/energy/open-weight-sets.toml"),
    ),
    (
        "energy/price-calibration.toml",
        include_str!("../../data/energy/price-calibration.toml"),
    ),
    (
        "energy/prices.toml",
        include_str!("../../data/energy/prices.toml"),
    ),
    (
        "energy/models.toml",
        include_str!("../../data/energy/models.toml"),
    ),
    ("providers.toml", include_str!("../../data/providers.toml")),
    (
        "grid/ember.toml",
        include_str!("../../data/grid/ember.toml"),
    ),
    ("discovery.toml", include_str!("../../data/discovery.toml")),
    ("findings.toml", include_str!("../../data/findings.toml")),
];

pub const FALLBACK_PARAMETER_SET: &str = "frontier";

pub const FALLBACK_PROVIDER: &str = "unknown";

/// The finding whose `cached_tokens` the report's breakdown uses.
pub const LONG_CONTEXT: &str = "long-context";

pub const FIXED_CONTEXT: &str = "fixed-context";

/// The finding whose `roles` and whose actions' `model` the estimate uses.
pub const MODEL_CHOICE: &str = "model-choice";

/// The findings the tool knows; a record with another id is an error.
pub const FINDINGS: [&str; 3] = [LONG_CONTEXT, FIXED_CONTEXT, MODEL_CHOICE];

#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot list the data override files in {}: {error}; check that the directory is readable", dir.display())]
    List { dir: PathBuf, error: io::Error },
    #[error("cannot read the data override file {}: {error}; check that the file is readable", file.display())]
    Read { file: PathBuf, error: io::Error },
    #[error("data file {file}: {problem}; {recovery}")]
    Invalid {
        file: String,
        problem: String,
        recovery: &'static str,
    },
    #[error("cannot write {}: {error}; choose an empty directory", file.display())]
    Export { file: PathBuf, error: io::Error },
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Kind {
    Source,
    ParameterSet,
    PriceCalibration,
    Price,
    Model,
    Provider,
    Geography,
    Location,
    Finding,
    Action,
}

impl fmt::Display for Kind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Kind::Source => "source",
            Kind::ParameterSet => "parameter set",
            Kind::PriceCalibration => "price calibration",
            Kind::Price => "price",
            Kind::Model => "model",
            Kind::Provider => "provider",
            Kind::Geography => "geography",
            Kind::Location => "location",
            Kind::Finding => "finding",
            Kind::Action => "action",
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct ResolvedProvider {
    #[serde(flatten)]
    pub provider: Provider,
    /// The geographies of `sites` after `named` is expanded.
    pub site_ids: Vec<String>,
    /// The geographies of `bounds_sites` after `listed` or `all` is
    /// expanded.
    pub bounds_ids: Vec<String>,
    pub grid: Five<GramsCo2ePerKwh>,
}

#[derive(Clone, Debug, Serialize)]
pub struct Stale {
    pub kind: Kind,
    pub id: String,
    pub reviewed: Date,
}

#[derive(Clone, Debug, Serialize)]
pub struct Data {
    pub version: Version,
    pub review_interval_days: i32,
    pub price_review_interval_days: i32,
    pub sources: BTreeMap<String, Source>,
    pub parameter_sets: BTreeMap<String, ParameterSet>,
    pub price_calibrations: BTreeMap<String, PriceCalibration>,
    pub prices: BTreeMap<String, Price>,
    /// The sets of priced models, derived from a written set, a price and
    /// its calibration, by their ids from [`priced_set_id`].
    pub priced_sets: BTreeMap<String, ParameterSet>,
    pub models: BTreeMap<String, Model>,
    pub providers: BTreeMap<String, ResolvedProvider>,
    pub geographies: BTreeMap<String, Geography>,
    pub locations: BTreeMap<String, Location>,
    pub findings: BTreeMap<String, Finding>,
    /// By [`action_id`].
    pub actions: BTreeMap<String, ClientAction>,
}

/// `matched` is false when `record` is the fallback.
pub struct Match<'a, T> {
    pub record: &'a T,
    pub matched: bool,
}

impl Data {
    /// True when the record's text is the user's, not the shipped data's.
    #[must_use]
    pub fn is_overridden(&self, kind: Kind, id: &str) -> bool {
        self.version.overrides.iter().any(|file| {
            file.records
                .iter()
                .any(|record| record.kind == kind && record.id == id)
        })
    }

    /// An exact pattern wins over any prefix, a longer prefix over a
    /// shorter one.
    pub fn model(&self, name: &str) -> Option<&Model> {
        self.models
            .values()
            .filter_map(|model| match model.pattern.strip_suffix('*') {
                None if model.pattern == name => Some((usize::MAX, model)),
                Some(prefix) if name.starts_with(prefix) => Some((prefix.len(), model)),
                _ => None,
            })
            .max_by_key(|(length, _)| *length)
            .map(|(_, model)| model)
    }

    /// `prompt_above` is a threshold of [`Data::prompt_thresholds`] or 0.
    pub fn parameter_set(&self, name: &str, prompt_above: u64) -> Match<'_, ParameterSet> {
        let model = self.model(name);
        let record = match model {
            None => &self.parameter_sets[FALLBACK_PARAMETER_SET],
            Some(model) => self.model_set(model, prompt_above),
        };
        Match {
            record,
            matched: model.is_some(),
        }
    }

    /// The set of the model record `id`, as [`Data::parameter_set`] gives it
    /// for a call; `None` when no record has that id.
    #[must_use]
    pub fn set_of_model(&self, id: &str, prompt_above: u64) -> Option<&ParameterSet> {
        self.models
            .get(id)
            .map(|model| self.model_set(model, prompt_above))
    }

    fn model_set(&self, model: &Model, prompt_above: u64) -> &ParameterSet {
        match &model.price {
            None => &self.parameter_sets[&model.parameter_set],
            Some(price) => {
                let tier = self.prices[price]
                    .tier
                    .iter()
                    .rfind(|tier| tier.above_prompt_tokens <= prompt_above);
                &self.priced_sets[&priced_set_id(&model.parameter_set, price, tier)]
            }
        }
    }

    /// The model record the calls of `client` are compared with for
    /// `model-choice`, and the roles the finding counts; `None` when the
    /// data names no such action.
    #[must_use]
    pub fn model_choice(&self, client: &str) -> Option<(&str, &[Role])> {
        let model = self
            .actions
            .get(&action_id(MODEL_CHOICE, client))?
            .model
            .as_deref()?;
        let roles = self.findings.get(MODEL_CHOICE)?.roles.as_deref()?;
        Some((model, roles))
    }

    #[must_use]
    pub fn set(&self, id: &str) -> Option<&ParameterSet> {
        self.parameter_sets
            .get(id)
            .or_else(|| self.priced_sets.get(id))
    }

    /// The cache-read tokens above which a call is long-context; loading
    /// ensures the record and the value exist.
    #[must_use]
    pub fn long_context_tokens(&self) -> u64 {
        self.findings
            .get(LONG_CONTEXT)
            .and_then(|finding| finding.cached_tokens)
            .unwrap_or_default()
    }

    /// The steps a user of `client` can take about `finding`; `None` when
    /// the data names none, so the finding leaves that client's calls out.
    #[must_use]
    pub fn steps(&self, finding: &str, client: &str) -> Option<&[String]> {
        self.actions
            .get(&action_id(finding, client))
            .map(|action| action.steps.as_slice())
    }

    #[must_use]
    pub fn prompt_thresholds(&self) -> Vec<u64> {
        self.prices
            .values()
            .flat_map(|price| price.tier.iter().map(|tier| tier.above_prompt_tokens))
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect()
    }

    pub fn provider(&self, name: &str) -> Match<'_, ResolvedProvider> {
        match self.providers.get(name) {
            Some(record) => Match {
                record,
                matched: true,
            },
            None => Match {
                record: &self.providers[FALLBACK_PROVIDER],
                matched: false,
            },
        }
    }

    #[must_use]
    pub fn stale(&self, today: Date) -> Vec<Stale> {
        let records = self
            .parameter_sets
            .values()
            .map(|record| (Kind::ParameterSet, &record.id, record.reviewed))
            .chain(
                self.price_calibrations
                    .values()
                    .map(|record| (Kind::PriceCalibration, &record.id, record.reviewed)),
            )
            .chain(
                self.prices
                    .values()
                    .map(|record| (Kind::Price, &record.id, record.reviewed)),
            )
            .chain(
                self.models
                    .values()
                    .map(|record| (Kind::Model, &record.id, record.reviewed)),
            )
            .chain(self.providers.values().map(|record| {
                (
                    Kind::Provider,
                    &record.provider.id,
                    record.provider.reviewed,
                )
            }))
            .chain(
                self.geographies
                    .values()
                    .map(|record| (Kind::Geography, &record.id, record.reviewed)),
            )
            .chain(
                self.locations
                    .values()
                    .map(|record| (Kind::Location, &record.id, record.reviewed)),
            )
            .chain(
                self.findings
                    .values()
                    .map(|record| (Kind::Finding, &record.id, record.reviewed)),
            )
            .chain(
                self.actions
                    .iter()
                    .map(|(id, record)| (Kind::Action, id, record.reviewed)),
            );
        records
            .filter(|(kind, _, reviewed)| {
                let interval = if *kind == Kind::Price {
                    self.price_review_interval_days
                } else {
                    self.review_interval_days
                };
                reviewed
                    .until(today)
                    .is_ok_and(|span| span.get_days() > interval)
            })
            .map(|(kind, id, reviewed)| Stale {
                kind,
                id: id.clone(),
                reviewed,
            })
            .collect()
    }
}

/// Never overwrites a file.
pub fn export(dir: &Path) -> Result<Vec<PathBuf>, Error> {
    let mut written = Vec::new();
    for (path, text) in SHIPPED {
        let file = dir.join(path);
        if let Some(parent) = file.parent() {
            fs::create_dir_all(parent).map_err(|error| Error::Export {
                file: parent.to_path_buf(),
                error,
            })?;
        }
        let result = fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&file)
            .and_then(|mut handle| io::Write::write_all(&mut handle, text.as_bytes()));
        result.map_err(|error| Error::Export {
            file: file.clone(),
            error,
        })?;
        written.push(file);
    }
    Ok(written)
}

#[cfg(test)]
mod tests;
