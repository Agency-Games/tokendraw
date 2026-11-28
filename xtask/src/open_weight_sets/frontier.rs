//! The frontier set's attention terms from the open-weight sets, so that a
//! closed model's attention follows the documented designs as sets are
//! added or corrected, instead of a hand estimate that can drift from them.

use std::fmt::Write as _;

use anyhow::{Context as _, bail};
use serde::{Deserialize, Serialize};

use std::collections::BTreeMap;

use super::{Group, Input, Model, Record, Set, Values, attention, check};
use crate::price_calibration::{self, Factor, Source};

/// The `[frontier]` table of `xtask/open-weight-sets.toml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(super) struct Rule {
    min_active_b: f64,
    basis: String,
    reference: Vec<Reference>,
}

/// A documented design that bounds the range but ships no set.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Reference {
    id: String,
    name: String,
    sources: Vec<String>,
    basis: String,
    group: Vec<Group>,
}

/// `data/energy/parameter-sets.toml`, which may also define sources, as
/// every data file may.
#[derive(Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
struct ParameterSets {
    parameter_set: Vec<Record>,
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    sources: BTreeMap<String, Source>,
}

/// `a_p`, `a_d` and `c0` of the frontier set.
fn envelope(
    input: &Input,
    weights: &price_calibration::Input,
    sets: &[Set],
) -> anyhow::Result<[Factor; 3]> {
    let rule = &input.frontier;
    let mut large = Vec::new();
    for (model, set) in input.model.iter().zip(sets) {
        let active = weights
            .model
            .iter()
            .find(|listed| listed.id == model.id)
            .with_context(|| format!("model {} has no active parameters", model.id))?;
        if active.active_b >= rule.min_active_b {
            large.push((model, set));
        }
    }
    if large.is_empty() {
        bail!(
            "no open-weight set has at least {} billion active parameters",
            rule.min_active_b
        );
    }
    let mut references = Vec::new();
    for reference in &rule.reference {
        check(&format!("reference {}", reference.id), &reference.group)?;
        let (values, _) = attention(&reference.group, &input.hardware.high)?;
        references.push(values.in_file_units());
    }
    let bands = bands(rule, &large);
    let large_sets: Vec<&Set> = large.iter().map(|(_, set)| *set).collect();
    let factor = |pick: fn(&Values) -> f64, what: &str| {
        let [lower_bound, low_case, base_case, high_case, upper_bound] =
            spread(&large_sets, sets, &references, pick);
        // The set the lower bound comes from, which need not be large.
        let lowest = input
            .model
            .iter()
            .zip(sets)
            .min_by(|(_, a), (_, b)| pick(&a.low).total_cmp(&pick(&b.low)))
            .map(|(model, _)| model);
        let mut sources: Vec<String> = Vec::new();
        for id in large
            .iter()
            .flat_map(|(model, _)| &model.sources)
            .chain(lowest.iter().flat_map(|model| &model.sources))
            .chain(
                rule.reference
                    .iter()
                    .flat_map(|reference| &reference.sources),
            )
            .chain(&input.hardware.sources)
        {
            if !sources.contains(id) {
                sources.push(id.clone());
            }
        }
        let from = lowest.map_or_else(String::new, |model| {
            format!(" The lower bound is {}'s.", model.name)
        });
        Factor {
            lower_bound,
            low_case,
            base_case,
            high_case,
            upper_bound,
            label: "D".to_owned(),
            sources,
            basis: format!("{what} {bands}{from}"),
        }
    };
    Ok([
        factor(
            |v| v.a_p,
            "Prefill attention per pair of a new token and an earlier token, as the open-weight sets derive it from their configurations.",
        ),
        factor(
            |v| v.a_d,
            "Decode attention per pair of an output token and an earlier token, as the open-weight sets derive it from their configurations.",
        ),
        factor(
            |v| v.c0,
            "Reloading a cached token's key-value entries once per call, as the open-weight sets derive it from their configurations.",
        ),
    ])
}

/// The five values of one term: the cases from the base scenario of the
/// `large` sets, the lower bound from every set's low scenario and the
/// upper bound from the large sets' high scenario and the `references`.
fn spread(
    large: &[&Set],
    sets: &[Set],
    references: &[Values],
    pick: fn(&Values) -> f64,
) -> [f64; 5] {
    let mut base: Vec<f64> = large.iter().map(|set| pick(&set.base)).collect();
    base.sort_by(f64::total_cmp);
    let middle = base.len() / 2;
    let median = if base.len().is_multiple_of(2) {
        f64::midpoint(base[middle - 1], base[middle])
    } else {
        base[middle]
    };
    let lower = sets
        .iter()
        .map(|set| pick(&set.low))
        .fold(f64::INFINITY, f64::min);
    let upper = large
        .iter()
        .map(|set| pick(&set.high))
        .chain(references.iter().map(pick))
        .fold(f64::NEG_INFINITY, f64::max);
    [lower, base[0], median, base[base.len() - 1], upper]
}

/// How the five values are chosen, with the models and references named.
fn bands(rule: &Rule, large: &[(&Model, &Set)]) -> String {
    let names = large
        .iter()
        .map(|(model, _)| model.name.as_str())
        .collect::<Vec<_>>()
        .join(", ");
    let reference_names = rule
        .reference
        .iter()
        .map(|reference| reference.name.as_str())
        .collect::<Vec<_>>()
        .join(" and ");
    let mut text = format!(
        "Low case, base case and high case: the lowest, the median and the highest base case of the open-weight sets with at least {} billion active parameters: {names}. Lower bound: the lowest lower bound of every open-weight set. Upper bound: the highest upper bound of those sets and of {reference_names} on the high scenario. {}",
        rule.min_active_b, rule.basis
    );
    for reference in &rule.reference {
        let _ = write!(text, " {}: {}", reference.name, reference.basis);
    }
    text
}

/// `data/energy/parameter-sets.toml`, whose text is `parameter_sets`, with
/// the frontier set's attention terms from `sets`. Every other value, the
/// review date included, is kept: the date records a review of the whole
/// set, which regenerating the sets is not.
pub fn render_parameter_sets(
    input: &Input,
    weights: &price_calibration::Input,
    sets: &[Set],
    parameter_sets: &str,
) -> anyhow::Result<String> {
    let mut file: ParameterSets = toml::from_str(parameter_sets)
        .context("`data/energy/parameter-sets.toml` does not fit its schema")?;
    let frontier = file
        .parameter_set
        .iter_mut()
        .find(|set| set.id == "frontier")
        .context("`data/energy/parameter-sets.toml` has no frontier set")?;
    let [a_p, a_d, c0] = envelope(input, weights, sets)?;
    frontier.a_p_uj_per_pair = a_p;
    frontier.a_d_uj_per_pair = a_d;
    frontier.c0_mj_per_cached_token = c0;
    let mut text = String::new();
    let _ = writeln!(
        text,
        "# Parameter sets of the per-call energy model. Each parameter has five\n\
         # values: lower bound, low case, base case, high case and upper bound.\n\
         # The bounds are what non-disclosure leaves open, the low and high case\n\
         # the spread across documented architectures and hardware.\n\
         #\n\
         # `cargo xtask open-weight-sets` writes the frontier set's a_p, a_d and\n\
         # c0 from the open-weight sets. Every other value is edited here, and\n\
         # the command keeps it.\n"
    );
    text.push_str(&toml::to_string(&file).context("the parameter sets as TOML")?);
    Ok(text)
}
