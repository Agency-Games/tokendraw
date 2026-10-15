use super::*;
use crate::data::{self, Error};
use crate::quantity::Tokens;

fn usage(client: &str, provider: &str, model: &str, sums: [f64; 6]) -> Usage {
    let [
        calls,
        cache_read,
        new_input,
        output,
        prefill_pairs,
        decode_pairs,
    ] = sums;
    #[expect(
        clippy::cast_possible_truncation,
        clippy::cast_sign_loss,
        reason = "whole counts written as numbers"
    )]
    let count = |value: f64| value as u64;
    Usage {
        client: client.to_owned(),
        provider: provider.to_owned(),
        model: model.to_owned(),
        role: "interactive".to_owned(),
        prompt_above: 0,
        cached_above: false,
        calls: count(calls),
        cache_read: Tokens(count(cache_read)),
        new_input: Tokens(count(new_input)),
        output: Tokens(count(output)),
        prefill_pairs,
        decode_pairs,
        estimated_output: Tokens(0),
        estimated_input: Tokens(0),
        without_fixed: WithoutFixed::default(),
    }
}

fn kwh(values: &Five<WattHours>) -> [f64; 5] {
    let values = values.map(|value| value.0 / 1000.0);
    [
        values.lower_bound,
        values.low_case,
        values.base_case,
        values.high_case,
        values.upper_bound,
    ]
}

fn kg(values: &Five<GramsCo2e>) -> [f64; 5] {
    kwh(&values.map(|value| WattHours(value.0)))
}

fn assert_near(actual: [f64; 5], expected: [f64; 5], tolerance: f64) {
    for (actual, expected) in actual.iter().zip(expected) {
        assert!(
            (actual - expected).abs() <= tolerance,
            "{actual:?} is not within {tolerance} of {expected:?}"
        );
    }
}

// A month of Claude Code and Codex use, 104,831 calls, worked by hand
// with the shipped frontier parameter set and the Anthropic and OpenAI
// provider records.
//
// The sums per client (calls, `L`, `n_new`, `n_out`, the prefill pairs
// and the decode pairs; the pair sums to five significant figures):
//
// - Claude Code: 34,222; 9,129,322,826; 169,380,824; 24,821,250;
//   3.8067 x 10^13; 7.3027 x 10^12.
// - Codex: 70,609; 9,014,650,880; 208,150,924; 31,606,763;
//   2.2493 x 10^13; 4.5822 x 10^12.
//
// One kWh is 3.6 x 10^6 J, so each unit of a parameter contributes, before
// `idle` and PUE, for Claude Code: `a_p` 3.8067 x 10^13 x 10^-6 / 3.6 x
// 10^6 = 10.574 kWh per uJ, `a_d` 2.0285 kWh per uJ, `b_in` 47.050 kWh per
// J, `b_out` 6.8948 kWh per J, `c0` 2.5359 kWh per mJ; for Codex: 6.2480,
// 1.2728, 57.820, 8.7797 and 2.5041.
//
// Claude Code's node subtotal is then, from lower bound to upper bound,
// 15.13 / 58.28 / 106.58 / 222.95 / 381.14 kWh (base case: 4 x 10.574 + 20 x
// 2.0285 + 0.4 x 47.050 + 0.6 x 6.8948 + 0.3 x 2.5359); times `idle` (1.0 /
// 1.0 / 1.12 / 1.35 / 1.9) 15.13 / 58.28 / 119.38 / 300.98 / 724.18; times
// Anthropic's PUE (1.09 / 1.09 / 1.115 / 1.14 / 1.56) the energy below.
// Codex's node subtotal is 12.82 / 42.62 / 79.60 / 164.80 / 288.60, times
// `idle` 12.82 / 42.62 / 89.15 / 222.48 / 548.35, times OpenAI's PUE (1.09
// / 1.17 / 1.17 / 1.17 / 1.56) the energy below.
//
// Facility energy is IT energy times (PUE - 1): for Claude Code 15.13 x
// 0.09 = 1.36, 58.28 x 0.09 = 5.24, 119.38 x 0.115 = 13.73, 300.98 x 0.14 =
// 42.14, 724.18 x 0.56 = 405.54.
//
// Emissions are energy times the provider's grid in kg CO2e per kWh:
// Anthropic 0.027756 / 0.267563 / 0.400652 / 0.642875 / 0.674293, so
// 16.50 x 0.027756 = 0.46 and so on; OpenAI 0.027756 / 0.311362 / 0.378013
// / 0.529942 / 0.674293.
//
// The expected values are rounded to 0.01 and the pair sums to five
// significant figures, so values agree to within 0.01.
#[test]
fn a_hand_worked_month_gives_its_energy_facility_and_emissions() -> Result<(), Error> {
    let data = data::load(None)?;
    let usage = [
        usage(
            "claude-code",
            "anthropic",
            "claude-opus-4-1",
            [
                34_222.0,
                9_129_322_826.0,
                169_380_824.0,
                24_821_250.0,
                3.8067e13,
                7.3027e12,
            ],
        ),
        usage(
            "codex",
            "openai",
            "gpt-5-codex",
            [
                70_609.0,
                9_014_650_880.0,
                208_150_924.0,
                31_606_763.0,
                2.2493e13,
                4.5822e12,
            ],
        ),
    ];
    let estimate = l1(&data, &usage);
    // Claude Code's calls are Anthropic's and Codex's OpenAI's.
    let [claude_code, codex] = &estimate.groups[..] else {
        panic!("two groups expected, got {:?}", estimate.groups);
    };
    assert_eq!(claude_code.provider, "anthropic");
    assert_eq!(codex.provider, "openai");
    assert_near(
        kwh(&claude_code.energy),
        [16.50, 63.52, 133.10, 343.12, 1_129.71],
        0.01,
    );
    assert_near(
        kwh(&codex.energy),
        [13.97, 49.87, 104.30, 260.30, 855.42],
        0.01,
    );
    assert_near(
        kwh(&claude_code.facility),
        [1.36, 5.24, 13.73, 42.14, 405.54],
        0.01,
    );
    assert_near(
        kwh(&codex.facility),
        [1.15, 7.25, 15.16, 37.82, 307.08],
        0.01,
    );
    assert_near(
        kg(&claude_code.emissions),
        [0.46, 17.00, 53.33, 220.58, 761.76],
        0.01,
    );
    assert_near(
        kg(&codex.emissions),
        [0.39, 15.53, 39.43, 137.95, 576.81],
        0.01,
    );
    // The sum of the clients, before the cache residency term R.
    assert_near(
        kwh(&estimate.energy),
        [30.47, 113.39, 237.41, 603.42, 1_985.14],
        0.01,
    );
    assert_near(
        kwh(&estimate.facility),
        [2.52, 12.49, 28.88, 79.96, 712.61],
        0.01,
    );
    assert_eq!(estimate.unmatched_models, []);
    assert_eq!(estimate.unmatched_providers, []);
    Ok(())
}

// One call with `L` = 100,000, `n_new` = 1,000 and `n_out` = 500 on the
// base case frontier parameters at Anthropic:
//
// - prefill pairs 1,000 x (100,000 + 1,000 / 2) = 100,500,000, times 4 uJ
//   = 402 J;
// - decode pairs 500 x (100,000 + 1,000 + 500 / 2) = 50,625,000, times
//   20 uJ = 1,012.5 J;
// - new tokens 1,000 x 0.4 J = 400 J; output 500 x 0.6 J = 300 J;
//   cached tokens 100,000 x 0.3 mJ = 30 J;
// - node energy 2,144.5 J; IT energy times 1.12 = 2,401.84 J; energy
//   times 1.115 = 2,678.0516 J = 0.74390322 Wh; facility 2,401.84 x 0.115
//   = 276.211_6 J = 0.07672544 Wh;
// - emissions 0.74390322 Wh x 400.65175 g/kWh, the mean of Anthropic's
//   four sites (642.875 + 312.815 + 379.354 + 267.563) / 4, = 0.29804631 g.
#[test]
fn one_call_worked_by_hand() -> Result<(), Error> {
    let data = data::load(None)?;
    let call = usage(
        "claude-code",
        "anthropic",
        "claude-sonnet-4-5",
        [1.0, 100_000.0, 1_000.0, 500.0, 100_500_000.0, 50_625_000.0],
    );
    let estimate = l1(&data, &[call]);
    let near = |actual: f64, expected: f64| (actual - expected).abs() < 1e-8;
    assert!(near(estimate.energy.base_case.0, 2_678.051_6 / 3600.0));
    assert!(near(estimate.facility.base_case.0, 276.211_6 / 3600.0));
    assert!(near(
        estimate.emissions.base_case.0,
        2_678.051_6 / 3600.0 / 1000.0 * 400.651_75
    ));
    assert!(near(estimate.groups[0].it.base_case.0, 2_401.84 / 3600.0));
    Ok(())
}

// The call above has context terms of 402 + 1,012.5 + 30 = 1,444.5 J of
// node energy, times 1.12 and 1.115: 1,803.8916 J of its 2,678.0516 J.
#[test]
fn the_breakdown_splits_the_base_case_energy_by_terms_threshold_client_and_role()
-> Result<(), Error> {
    let data = data::load(None)?;
    let mut long = usage(
        "claude-code",
        "anthropic",
        "claude-sonnet-4-5",
        [1.0, 100_000.0, 1_000.0, 500.0, 100_500_000.0, 50_625_000.0],
    );
    long.cached_above = true;
    let mut short = usage(
        "codex",
        "openai",
        "gpt-5",
        [3.0, 30.0, 300.0, 60.0, 4_500.0, 2_400.0],
    );
    short.role = "subagent".to_owned();
    let short_alone = l1(&data, std::slice::from_ref(&short));
    let estimate = l1(&data, &[long, short]);
    let (total, long_energy) = (estimate.energy.base_case.0, 2_678.051_6 / 3600.0);
    let near = |actual: f64, expected: f64| (actual - expected).abs() < 1e-8;
    let parts = &estimate.breakdown;
    assert!(near(parts.cached_above.0, long_energy));
    assert!(near(
        parts.context_terms.0,
        1_803.891_6 / 3600.0 + short_alone.breakdown.context_terms.0
    ));
    assert!(short_alone.breakdown.context_terms.0 > 0.0);
    for split in [&parts.by_client, &parts.by_role] {
        assert_eq!(split.len(), 2);
        assert!(near(split.values().map(|part| part.0).sum(), total));
    }
    assert!(near(parts.by_client["claude-code"].0, long_energy));
    assert!(near(parts.by_role["subagent"].0, total - long_energy));
    Ok(())
}

// A call of 280,000 cache-read, 2,000 new and 500 output tokens whose
// thread started with 20,000 tokens: without them its prompt of 262,000
// tokens falls below the 272,000-token tier of its price.
#[test]
fn the_fixed_context_is_the_energy_less_that_of_the_call_without_it_at_its_own_tier()
-> Result<(), Error> {
    let data = data::load(None)?;
    let model = "gpt-6-astra";
    assert_ne!(
        data.parameter_set(model, 272_000).record.id,
        data.parameter_set(model, 0).record.id,
        "the tiers have their own parameter sets"
    );
    let pairs = |cached: f64, new: f64, out: f64| {
        [new * (cached + new / 2.0), out * (cached + new + out / 2.0)]
    };
    let [prefill, decode] = pairs(280_000.0, 2_000.0, 500.0);
    let mut call = usage(
        "codex",
        "openai",
        model,
        [1.0, 280_000.0, 2_000.0, 500.0, prefill, decode],
    );
    call.prompt_above = 272_000;
    let [prefill, decode] = pairs(260_000.0, 2_000.0, 500.0);
    let without = usage(
        "codex",
        "openai",
        model,
        [1.0, 260_000.0, 2_000.0, 500.0, prefill, decode],
    );
    call.without_fixed.prompt_above = 0;
    call.without_fixed.fixed = Tokens(20_000);
    call.without_fixed.cache_read = without.cache_read;
    call.without_fixed.new_input = without.new_input;
    call.without_fixed.prefill_pairs = without.prefill_pairs;
    call.without_fixed.decode_pairs = without.decode_pairs;
    let expected = l1(&data, std::slice::from_ref(&call)).energy.base_case.0
        - l1(&data, &[without]).energy.base_case.0;
    let estimate = l1(&data, &[call]);
    let fixed = estimate.breakdown.fixed_by_client["codex"];
    assert_eq!((fixed.calls, fixed.tokens), (1, Tokens(20_000)));
    assert!(expected > 0.0);
    assert!((fixed.energy.0 - expected).abs() < 1e-9);
    Ok(())
}

#[test]
fn calls_without_fixed_context_count_toward_the_finding() -> Result<(), Error> {
    let data = data::load(None)?;
    let mut carrying = usage(
        "claude-code",
        "anthropic",
        "claude-sonnet-4-5",
        [1.0, 100.0, 0.0, 10.0, 0.0, 1_500.0],
    );
    carrying.without_fixed.fixed = Tokens(100);
    carrying.without_fixed.decode_pairs = 50.0;
    let mut without = carrying.clone();
    without.role = "subagent".to_owned();
    without.without_fixed = WithoutFixed::default();
    let estimate = l1(&data, &[carrying, without]);
    let fixed = estimate.breakdown.fixed_by_client["claude-code"];
    assert_eq!((fixed.calls, fixed.tokens), (2, Tokens(100)));
    assert!(fixed.energy.0 > 0.0);
    Ok(())
}

#[test]
fn an_unmatched_model_and_provider_use_the_fallbacks_and_are_counted() -> Result<(), Error> {
    let data = data::load(None)?;
    let rows = [
        usage(
            "tool",
            "example-cloud",
            "example-medium",
            [3.0, 0.0, 30.0, 6.0, 450.0, 180.0],
        ),
        usage(
            "tool",
            "anthropic",
            "example-medium",
            [2.0, 0.0, 20.0, 4.0, 200.0, 80.0],
        ),
    ];
    let estimate = l1(&data, &rows);
    assert_eq!(
        estimate.unmatched_models,
        [("example-medium".to_owned(), 5)]
    );
    assert_eq!(
        estimate.unmatched_providers,
        [("example-cloud".to_owned(), 3)]
    );
    let providers: Vec<(&str, &str)> = estimate
        .groups
        .iter()
        .map(|group| (group.provider.as_str(), group.record.as_str()))
        .collect();
    assert_eq!(
        providers,
        [("anthropic", "anthropic"), ("example-cloud", "unknown")]
    );
    Ok(())
}

#[test]
fn no_calls_give_zero_and_no_factor_quality() -> Result<(), Error> {
    let data = data::load(None)?;
    let estimate = l1(&data, &[]);
    assert_eq!(estimate.energy, Five::same(WattHours(0.0)));
    assert_eq!(
        (estimate.energy_quality, estimate.emissions_quality),
        (None, None)
    );
    Ok(())
}

fn labelled_m() -> Result<Data, Error> {
    let mut data = data::load(None)?;
    for set in data.parameter_sets.values_mut() {
        for factor in [
            &mut set.a_p_uj_per_pair,
            &mut set.a_d_uj_per_pair,
            &mut set.b_in_j_per_token,
            &mut set.b_out_j_per_token,
            &mut set.c0_mj_per_cached_token,
            &mut set.idle,
        ] {
            factor.label = Label::M;
        }
    }
    for provider in data.providers.values_mut() {
        provider.provider.label = Label::M;
        provider.provider.pue.label = Label::M;
    }
    for geography in data.geographies.values_mut() {
        geography.label = Label::M;
    }
    Ok(data)
}

fn qualities(data: &Data) -> (Option<Label>, Option<Label>) {
    let call = usage(
        "codex",
        "openai",
        "gpt-5",
        [1.0, 10.0, 10.0, 10.0, 150.0, 250.0],
    );
    let estimate = l1(data, &[call]);
    (estimate.energy_quality, estimate.emissions_quality)
}

#[test]
fn factor_quality_is_the_weakest_label_of_the_records_used() -> Result<(), Error> {
    let data = labelled_m()?;
    assert_eq!(qualities(&data), (Some(Label::M), Some(Label::M)));

    let mut provider = data.clone();
    if let Some(openai) = provider.providers.get_mut("openai") {
        openai.provider.label = Label::I;
    }
    assert_eq!(qualities(&provider), (Some(Label::M), Some(Label::I)));

    let mut site = data.clone();
    if let Some(virginia) = site.geographies.get_mut("ember:US-VA") {
        virginia.label = Label::C;
    }
    assert_eq!(qualities(&site), (Some(Label::M), Some(Label::C)));

    let mut pue = data.clone();
    if let Some(openai) = pue.providers.get_mut("openai") {
        openai.provider.pue.label = Label::D;
    }
    assert_eq!(qualities(&pue), (Some(Label::D), Some(Label::D)));

    let mut idle = data;
    if let Some(frontier) = idle.parameter_sets.get_mut("frontier") {
        frontier.idle.label = Label::I;
    }
    assert_eq!(qualities(&idle), (Some(Label::I), Some(Label::I)));
    Ok(())
}

#[test]
fn the_shipped_data_gives_tool_default_quality() -> Result<(), Error> {
    let data = data::load(None)?;
    assert_eq!(qualities(&data), (Some(Label::D), Some(Label::D)));
    Ok(())
}
