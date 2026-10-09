//! Layer L1, inference electricity.
//!
//! Node energy at load is
//! `a_p * prefill_pairs + a_d * decode_pairs + b_in * n_new + b_out * n_out + c0 * L`;
//! IT energy is node energy times `idle`, facility energy IT energy times
//! (PUE - 1), and emissions energy times the provider's grid intensity.
//! Every product pairs like bands: envelope low with envelope low, and so on.

use std::collections::{BTreeMap, BTreeSet};
use std::ops::Add;

use crate::data::{Data, Factor, Five, Label, ParameterSet, ResolvedProvider};
use crate::ledger::Usage;
use crate::quantity::{GramsCo2e, WattHours};

const JOULES_PER_WATT_HOUR: f64 = 3600.0;
const JOULES_PER_MICROJOULE: f64 = 1e-6;
const JOULES_PER_MILLIJOULE: f64 = 1e-3;
const WATT_HOURS_PER_KILOWATT_HOUR: f64 = 1000.0;

#[derive(Clone, Debug, PartialEq)]
pub struct Group {
    pub provider: String,
    /// The id of the provider record that applied, possibly the fallback.
    pub record: String,
    pub calls: u64,
    pub it: Five<WattHours>,
    pub facility: Five<WattHours>,
    pub energy: Five<WattHours>,
    pub emissions: Five<GramsCo2e>,
    /// The weakest label of the parameters and PUE used.
    pub energy_quality: Label,
    /// The weakest label of the provider record and its geographies.
    pub grid_quality: Label,
    pub emissions_quality: Label,
}

/// Parts of the central L1 energy, by what the calls did.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Breakdown {
    /// The terms that grow with the context: `a_p`, `a_d` and `c0`.
    pub context_terms: WattHours,
    /// The calls whose cache-read tokens exceed the threshold the usage was
    /// split by.
    pub cached_above: WattHours,
    pub by_client: BTreeMap<String, WattHours>,
    pub by_role: BTreeMap<String, WattHours>,
}

/// Excludes the cache residency term `R`, which the data does not hold.
#[derive(Clone, Debug, PartialEq)]
pub struct L1 {
    /// By provider name.
    pub groups: Vec<Group>,
    pub energy: Five<WattHours>,
    pub facility: Five<WattHours>,
    pub emissions: Five<GramsCo2e>,
    /// The weakest label of the parameters and PUE used; `None` without
    /// calls.
    pub energy_quality: Option<Label>,
    /// The weakest label of those, the provider records and the
    /// geographies of their sites and envelope sites; `None` without calls.
    pub emissions_quality: Option<Label>,
    pub parameter_sets: Vec<String>,
    /// Model names no pattern matched, with their calls.
    pub unmatched_models: Vec<(String, u64)>,
    /// Provider names no record named, with their calls.
    pub unmatched_providers: Vec<(String, u64)>,
    pub breakdown: Breakdown,
}

#[must_use]
pub fn l1(data: &Data, usage: &[Usage]) -> L1 {
    let zero = Five::same(WattHours(0.0));
    let mut groups: BTreeMap<String, Group> = BTreeMap::new();
    let mut unmatched_models: BTreeMap<String, u64> = BTreeMap::new();
    let mut unmatched_providers: BTreeMap<String, u64> = BTreeMap::new();
    let mut total = L1 {
        groups: Vec::new(),
        energy: zero,
        facility: zero,
        emissions: Five::same(GramsCo2e(0.0)),
        energy_quality: None,
        emissions_quality: None,
        parameter_sets: Vec::new(),
        unmatched_models: Vec::new(),
        unmatched_providers: Vec::new(),
        breakdown: Breakdown::default(),
    };
    let mut sets = BTreeSet::new();
    for row in usage {
        let set = data.parameter_set(&row.model, row.prompt_above);
        if !set.matched {
            *unmatched_models.entry(row.model.clone()).or_default() += row.calls;
        }
        let provider = data.provider(&row.provider);
        if !provider.matched {
            *unmatched_providers.entry(row.provider.clone()).or_default() += row.calls;
        }
        let (set, provider) = (set.record, provider.record);
        sets.insert(set.id.clone());
        let energy_label = parameter_label(set).max(provider.provider.pue.label);
        let grid_quality = grid_label(data, provider);
        total.energy_quality = total.energy_quality.max(Some(energy_label));
        total.emissions_quality = total
            .emissions_quality
            .max(Some(energy_label.max(grid_quality)));
        let pue = provider.provider.pue.five();
        let (node, context) = node(set, row);
        let it = node.zip(&set.idle.five(), |node, idle| node * idle);
        let facility = it.zip(&pue, |it, pue| WattHours(it * (pue - 1.0)));
        let energy = it.zip(&pue, |it, pue| it * pue);
        let emissions = energy.zip(&provider.grid, |energy, grid| {
            GramsCo2e(energy / WATT_HOURS_PER_KILOWATT_HOUR * grid.0)
        });
        let (it, energy) = (it.map(WattHours), energy.map(WattHours));
        let group = groups.entry(row.provider.clone()).or_insert_with(|| Group {
            provider: row.provider.clone(),
            record: provider.provider.id.clone(),
            calls: 0,
            it: zero,
            facility: zero,
            energy: zero,
            emissions: Five::same(GramsCo2e(0.0)),
            energy_quality: energy_label,
            grid_quality,
            emissions_quality: energy_label.max(grid_quality),
        });
        group.energy_quality = group.energy_quality.max(energy_label);
        group.emissions_quality = group.emissions_quality.max(energy_label.max(grid_quality));
        group.calls += row.calls;
        group.it = add(&group.it, &it);
        group.facility = add(&group.facility, &facility);
        group.energy = add(&group.energy, &energy);
        group.emissions = add(&group.emissions, &emissions);
        let breakdown = &mut total.breakdown;
        let central = WattHours(energy.central.0);
        breakdown.context_terms =
            breakdown.context_terms + WattHours(context * set.idle.central * pue.central);
        if row.cached_above {
            breakdown.cached_above = breakdown.cached_above + central;
        }
        for (part, key) in [
            (&mut breakdown.by_client, &row.client),
            (&mut breakdown.by_role, &row.role),
        ] {
            let sum = part.entry(key.clone()).or_insert(WattHours(0.0));
            *sum = *sum + central;
        }
        total.facility = add(&total.facility, &facility);
        total.energy = add(&total.energy, &energy);
        total.emissions = add(&total.emissions, &emissions);
    }
    total.groups = groups.into_values().collect();
    total.parameter_sets = sets.into_iter().collect();
    total.unmatched_models = unmatched_models.into_iter().collect();
    total.unmatched_providers = unmatched_providers.into_iter().collect();
    total
}

/// In Wh, with the central value of the context terms.
fn node(set: &ParameterSet, usage: &Usage) -> (Five<f64>, f64) {
    #[expect(
        clippy::cast_precision_loss,
        reason = "token counts stay far below 2^53"
    )]
    let (cache_read, new_input, output) = (
        usage.cache_read.0 as f64,
        usage.new_input.0 as f64,
        usage.output.0 as f64,
    );
    let term =
        |factor: &Factor, unit: f64, count: f64| factor.five().map(|value| value * unit * count);
    let terms = [
        term(
            &set.a_p_uj_per_pair,
            JOULES_PER_MICROJOULE,
            usage.prefill_pairs,
        ),
        term(
            &set.a_d_uj_per_pair,
            JOULES_PER_MICROJOULE,
            usage.decode_pairs,
        ),
        term(&set.b_in_j_per_token, 1.0, new_input),
        term(&set.b_out_j_per_token, 1.0, output),
        term(
            &set.c0_mj_per_cached_token,
            JOULES_PER_MILLIJOULE,
            cache_read,
        ),
    ];
    let total = terms
        .iter()
        .fold(Five::same(0.0), |total, term| total.zip(term, |a, b| a + b))
        .map(|joules| joules / JOULES_PER_WATT_HOUR);
    let [a_p, a_d, _, _, c0] = &terms;
    let context = (a_p.central + a_d.central + c0.central) / JOULES_PER_WATT_HOUR;
    (total, context)
}

pub fn parameter_label(set: &ParameterSet) -> Label {
    [
        &set.a_p_uj_per_pair,
        &set.a_d_uj_per_pair,
        &set.b_in_j_per_token,
        &set.b_out_j_per_token,
        &set.c0_mj_per_cached_token,
        &set.idle,
    ]
    .iter()
    .map(|factor| factor.label)
    .max()
    .unwrap_or(Label::M)
}

fn grid_label(data: &Data, provider: &ResolvedProvider) -> Label {
    provider
        .site_ids
        .iter()
        .chain(&provider.envelope_ids)
        .filter_map(|id| data.geographies.get(id))
        .map(|geography| geography.label)
        .fold(provider.provider.label, Label::max)
}

fn add<T: Copy + Add<Output = T>>(total: &Five<T>, more: &Five<T>) -> Five<T> {
    total.zip(more, |a, b| a + b)
}

#[cfg(test)]
mod tests;
