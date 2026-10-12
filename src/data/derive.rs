use std::collections::{BTreeMap, BTreeSet};

use super::check::check;
use super::load::{Merged, Origin, Records, Version, invalid, records};
use super::records::{
    BoundsSites, Factor, Five, Geography, ParameterSet, Price, PriceCalibration, PriceTier,
    Provider, Sites,
};
use super::{Data, Error, ResolvedProvider};
use crate::quantity::GramsCo2ePerKwh;

/// The geographies `named`, `listed` and `all` stand for, taken from the
/// shipped data alone, so that a user's own provider or geography changes
/// only the records that name it.
pub(super) struct ShippedSites {
    named: Vec<String>,
    listed: Vec<String>,
    all: Vec<String>,
}

impl ShippedSites {
    pub(super) fn of(shipped: &Merged) -> Self {
        let named: BTreeSet<String> = shipped
            .providers
            .values()
            .filter_map(|(provider, _)| match &provider.sites {
                Sites::List(sites) => Some(sites.iter().cloned()),
                Sites::Named => None,
            })
            .flatten()
            .collect();
        let listed = shipped
            .providers
            .values()
            .filter_map(|(provider, _)| match &provider.bounds_sites {
                BoundsSites::List(sites) => Some(sites.iter().cloned()),
                _ => None,
            })
            .flatten()
            .chain(named.iter().cloned())
            .collect::<BTreeSet<_>>()
            .into_iter()
            .collect();
        Self {
            named: named.into_iter().collect(),
            listed,
            all: shipped.geographies.keys().cloned().collect(),
        }
    }
}

pub(super) fn resolve(
    merged: Merged,
    origins: &[Origin],
    shipped_sites: &ShippedSites,
    version: Version,
    [review_interval_days, price_review_interval_days]: [i32; 2],
) -> Result<Data, Error> {
    check(&merged, origins)?;
    let priced_sets = priced_sets(&merged, origins)?;
    let Merged {
        sources,
        parameter_sets,
        price_calibrations,
        prices,
        models,
        providers,
        geographies,
        locations,
        findings,
    } = merged;
    let mut resolved = BTreeMap::new();
    for (id, (provider, origin)) in providers {
        let site_ids = match &provider.sites {
            Sites::List(sites) => sites.clone(),
            Sites::Named => shipped_sites.named.clone(),
        };
        let bounds_ids = match &provider.bounds_sites {
            BoundsSites::None => Vec::new(),
            BoundsSites::List(sites) => sites.clone(),
            BoundsSites::Listed => shipped_sites.listed.clone(),
            BoundsSites::All => shipped_sites.all.clone(),
        };
        let grid = grid(
            &provider,
            &site_ids,
            &bounds_ids,
            &origins[origin],
            &geographies,
        )?;
        resolved.insert(
            id,
            ResolvedProvider {
                provider,
                site_ids,
                bounds_ids,
                grid,
            },
        );
    }
    Ok(Data {
        version,
        review_interval_days,
        price_review_interval_days,
        sources: records(sources),
        parameter_sets: records(parameter_sets),
        price_calibrations: records(price_calibrations),
        prices: records(prices),
        priced_sets,
        models: records(models),
        providers: resolved,
        geographies: records(geographies),
        locations: records(locations),
        findings: records(findings),
    })
}

/// Written ids hold neither `@` nor `>`, so no derived id equals another
/// or a written one.
#[must_use]
pub fn priced_set_id(set: &str, price: &str, tier: Option<&PriceTier>) -> String {
    match tier {
        None => format!("{set}@{price}"),
        Some(tier) => format!("{set}@{price}>{}", tier.above_prompt_tokens),
    }
}

fn priced_sets(
    merged: &Merged,
    origins: &[Origin],
) -> Result<BTreeMap<String, ParameterSet>, Error> {
    let mut sets = BTreeMap::new();
    for (model, origin) in merged.models.values() {
        let Some(price_id) = &model.price else {
            continue;
        };
        let (base, _) = &merged.parameter_sets[&model.parameter_set];
        let (price, _) = &merged.prices[price_id];
        let (calibration, _) = &merged.price_calibrations[&price.calibration];
        let tiers = std::iter::once(None).chain(price.tier.iter().map(Some));
        for tier in tiers {
            let id = priced_set_id(&base.id, &price.id, tier);
            if merged.parameter_sets.contains_key(&id) {
                return Err(invalid(
                    &origins[*origin],
                    format!(
                        "model {}: the derived parameter set {id} has the id of a written one",
                        model.id
                    ),
                ));
            }
            sets.entry(id.clone())
                .or_insert_with(|| priced_set(id, base, price, calibration, tier));
        }
    }
    Ok(sets)
}

/// The low bands divide by the high markup, so every band stays in order.
fn priced_set(
    id: String,
    base: &ParameterSet,
    price: &Price,
    calibration: &PriceCalibration,
    tier: Option<&PriceTier>,
) -> ParameterSet {
    #[expect(
        clippy::cast_precision_loss,
        reason = "reference calls have a few thousand tokens"
    )]
    let (chat_in, chat_out) = (
        calibration.chat_input_tokens as f64,
        calibration.chat_output_tokens as f64,
    );
    let chat_usd =
        (chat_in * price.input_usd_per_mtok + chat_out * price.output_usd_per_mtok) / 1e6;
    let markup = calibration.markup.five();
    let reversed = Five {
        lower_bound: markup.upper_bound,
        low_case: markup.high_case,
        base_case: markup.base_case,
        high_case: markup.low_case,
        upper_bound: markup.lower_bound,
    };
    let joules = calibration
        .wh_per_usd
        .five()
        .zip(&reversed, |wh_per_usd, markup| {
            chat_usd * wh_per_usd / markup * 3600.0
        });
    let anchor = chat_in * calibration.anchor_in_j + chat_out * calibration.anchor_out_j;
    let (in_scale, out_scale, tier_text) = match tier {
        None => (1.0, 1.0, String::new()),
        Some(tier) => (
            tier.input_usd_per_mtok / price.input_usd_per_mtok,
            tier.output_usd_per_mtok / price.output_usd_per_mtok,
            format!(
                " Above {} prompt tokens the tier's prices, {} and {}, scale the base values.",
                tier.above_prompt_tokens, tier.input_usd_per_mtok, tier.output_usd_per_mtok
            ),
        ),
    };
    let mut sources = Vec::new();
    for source in price
        .sources
        .iter()
        .chain(&calibration.wh_per_usd.sources)
        .chain(&calibration.markup.sources)
    {
        if !sources.contains(source) {
            sources.push(source.clone());
        }
    }
    let label = price
        .label
        .max(calibration.wh_per_usd.label)
        .max(calibration.markup.label);
    let basis = priced_basis(price, calibration) + &tier_text;
    let factor = |anchor_j: f64, scale: f64| {
        let values = joules.map(|joules| joules * anchor_j / anchor * scale);
        Factor {
            lower_bound: values.lower_bound,
            low_case: values.low_case,
            base_case: values.base_case,
            high_case: values.high_case,
            upper_bound: values.upper_bound,
            label,
            sources: sources.clone(),
            basis: basis.clone(),
        }
    };
    let description = match tier {
        None => format!(
            "{}, with weight terms from price {}",
            base.description, price.id
        ),
        Some(tier) => format!(
            "{}, with weight terms from price {} above {} prompt tokens",
            base.description, price.id, tier.above_prompt_tokens
        ),
    };
    ParameterSet {
        id,
        description,
        reviewed: base.reviewed.min(price.reviewed).min(calibration.reviewed),
        a_p_uj_per_pair: base.a_p_uj_per_pair.clone(),
        a_d_uj_per_pair: base.a_d_uj_per_pair.clone(),
        b_in_j_per_token: factor(calibration.anchor_in_j, in_scale),
        b_out_j_per_token: factor(calibration.anchor_out_j, out_scale),
        c0_mj_per_cached_token: base.c0_mj_per_cached_token.clone(),
        idle: base.idle.clone(),
    }
}

fn priced_basis(price: &Price, calibration: &PriceCalibration) -> String {
    format!(
        "The weight energy that price {} buys: {} and {} USD per million input and output tokens for the reference call of {} new input and {} output tokens, times calibration {}'s Wh per USD, divided by its markup (the high markup for the low values), split in the ratio {} : {}.",
        price.id,
        price.input_usd_per_mtok,
        price.output_usd_per_mtok,
        calibration.chat_input_tokens,
        calibration.chat_output_tokens,
        calibration.id,
        calibration.anchor_in_j,
        calibration.anchor_out_j,
    )
}

fn grid(
    provider: &Provider,
    site_ids: &[String],
    bounds_ids: &[String],
    origin: &Origin,
    geographies: &Records<Geography>,
) -> Result<Five<GramsCo2ePerKwh>, Error> {
    let what = format!("provider {}", provider.id);
    if site_ids.is_empty() {
        return Err(invalid(origin, format!("{what} has no sites")));
    }
    let intensity = |site: &String| {
        geographies
            .get(site)
            .map(|(geography, _)| geography.g_co2e_per_kwh.0)
            .ok_or_else(|| {
                invalid(
                    origin,
                    format!("{what} names geography {site}, which no data file defines"),
                )
            })
    };
    let sites = site_ids
        .iter()
        .map(intensity)
        .collect::<Result<Vec<_>, _>>()?;
    let bounds = bounds_ids
        .iter()
        .map(intensity)
        .collect::<Result<Vec<_>, _>>()?;
    let lowest = |values: &[f64]| values.iter().copied().fold(f64::INFINITY, f64::min);
    let highest = |values: &[f64]| values.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    #[expect(
        clippy::cast_precision_loss,
        reason = "a provider has a handful of sites"
    )]
    let base_case = sites.iter().sum::<f64>() / sites.len() as f64;
    Ok(Five {
        lower_bound: GramsCo2ePerKwh(lowest(&sites).min(lowest(&bounds))),
        low_case: GramsCo2ePerKwh(lowest(&sites)),
        base_case: GramsCo2ePerKwh(base_case),
        high_case: GramsCo2ePerKwh(highest(&sites)),
        upper_bound: GramsCo2ePerKwh(highest(&sites).max(highest(&bounds))),
    })
}
