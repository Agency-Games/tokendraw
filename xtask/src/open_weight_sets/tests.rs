use super::*;

const LAYOUTS: &str = include_str!("../../open-weight-sets.toml");
const CALIBRATION: &str = include_str!("../../price-calibration.toml");
const PARAMETER_SETS: &str = include_str!("../../../data/energy/parameter-sets.toml");

fn shipped() -> anyhow::Result<(Input, price_calibration::Input)> {
    Ok((
        parse_input(LAYOUTS)?,
        price_calibration::parse_input(CALIBRATION)?,
    ))
}

#[derive(Clone, Copy)]
struct Call {
    cached: f64,
    new: f64,
    out: f64,
}

/// The integral of `min(c, cap)` for `c` from `a` to `b`: the keys attended
/// by the tokens at positions `a` to `b` when each attends to at most
/// `cap`.
fn integral_min(a: f64, b: f64, cap: f64) -> f64 {
    if b <= a {
        0.0
    } else if cap <= a {
        cap * (b - a)
    } else if cap >= b {
        (b * b - a * a) / 2.0
    } else {
        (cap * cap - a * a) / 2.0 + cap * (b - cap)
    }
}

/// Keys attended and entries scored by an indexer for the tokens at
/// positions `a` to `b` in one layer.
fn attended(group: &Group, a: f64, b: f64) -> (f64, f64) {
    let all = integral_min(a, b, f64::INFINITY);
    let local = group
        .window
        .map_or(0.0, |window| integral_min(a, b, f64::from(window)));
    match group.kind {
        Kind::Full => (all, 0.0),
        Kind::Sliding => (local, 0.0),
        Kind::Sparse => (
            integral_min(a, b, f64::from(group.topk.unwrap_or(0))) + local,
            all,
        ),
        Kind::Compressed => {
            let ratio = f64::from(group.ratio.unwrap_or(1));
            let cap = group
                .topk
                .map_or(f64::INFINITY, |topk| f64::from(topk) * ratio);
            let indexed = if group.index_heads.is_some() {
                all / ratio
            } else {
                0.0
            };
            (integral_min(a, b, cap) / ratio + local, indexed)
        }
        Kind::Linear => (0.0, 0.0),
    }
}

/// Node energy of a call in J, integrated over the call's token positions,
/// with multi-head latent attention in prefill in whichever form costs
/// less for the call.
fn exact(
    model: &Model,
    active: &price_calibration::OpenModel,
    anchor: &price_calibration::Anchor,
    scenario: &Scenario,
    factor: f64,
    call: Call,
) -> anyhow::Result<f64> {
    let e_flop = scenario.e_flop_pj * PJ;
    let e_byte = scenario.e_byte_pj * PJ;
    let Call { cached, new, out } = call;
    let mut joules = 0.0;
    let mut stored = 0.0;
    for group in &model.group {
        let layers = f64::from(group.layers);
        if group.kind == Kind::Linear {
            let state = f64::from(group.heads)
                * f64::from(group.qk_dim.unwrap_or(0))
                * f64::from(group.v_dim.unwrap_or(0));
            joules += layers * new * 4.0 * state * e_flop;
            joules += layers
                * out
                * (state * group.state_bytes.unwrap_or(0.0) * e_byte).max(4.0 * state * e_flop);
            continue;
        }
        let (mut prefill, decode, entry) = group.layout()?;
        let (keys, indexed) = attended(group, cached, cached + new);
        if group.layout == Layout::Mla && new > 0.0 && keys > 0.0 {
            prefill = (prefill + group.expand_flops()? * (cached + new) / keys).min(decode);
        }
        joules += layers * keys * prefill * e_flop;
        let (keys_d, indexed_d) = attended(group, cached + new, cached + new + out);
        joules += layers * keys_d * (entry * e_byte).max(decode * e_flop);
        if let (Some(heads), Some(dim), Some(bits), Some(index_layers)) = (
            group.index_heads,
            group.index_dim,
            group.index_bits,
            group.index_layers,
        ) {
            let precision = f64::from(bits) / 16.0;
            let flops = 2.0 * f64::from(heads) * f64::from(dim);
            let bytes = f64::from(dim) * f64::from(bits) / 8.0;
            let index_layers = f64::from(index_layers);
            joules += index_layers * indexed * flops * precision * e_flop;
            joules += index_layers * indexed_d * (bytes * e_byte).max(flops * precision * e_flop);
        }
        stored += layers * entry / f64::from(group.ratio.unwrap_or(1));
    }
    let weights = factor / anchor.active_b;
    joules += new * anchor.in_j * active.active_prefill_b.unwrap_or(active.active_b) * weights;
    joules += out * anchor.out_j * active.active_b * weights;
    joules += cached * stored * scenario.reload_nj_per_byte * NJ;
    Ok(joules)
}

/// Node energy of a call in J from the five parameters.
fn formula(v: &Values, call: Call) -> f64 {
    let Call { cached, new, out } = call;
    v.a_p * 1e-6 * new * (cached + new / 2.0)
        + v.a_d * 1e-6 * out * (cached + new + out / 2.0)
        + v.b_in * new
        + v.b_out * out
        + v.c0 * 1e-3 * cached
}

const CALLS: [Call; 6] = [
    Call {
        cached: 0.0,
        new: 1000.0,
        out: 500.0,
    },
    Call {
        cached: 5000.0,
        new: 2000.0,
        out: 300.0,
    },
    Call {
        cached: 20_000.0,
        new: 500.0,
        out: 200.0,
    },
    Call {
        cached: 100_000.0,
        new: 3000.0,
        out: 800.0,
    },
    Call {
        cached: 150_000.0,
        new: 200.0,
        out: 100.0,
    },
    Call {
        cached: 300_000.0,
        new: 5000.0,
        out: 2000.0,
    },
];

/// Node energy in J of each of `CALLS` on the base scenario, computed by
/// an independent implementation of the per-position integration outside
/// this repository.
#[expect(
    clippy::unreadable_literal,
    reason = "the values as the implementation printed them"
)]
const INTEGRATED_BASE_CASE: [(&str, [f64; 6]); 10] = [
    (
        "deepseek-v4-pro",
        [
            367.1077914672861,
            520.0627564906854,
            173.57727551638487,
            940.602399761626,
            81.21718950618812,
            1975.1578350838488,
        ],
    ),
    (
        "deepseek-v4.1-flash",
        [
            89.81337518350253,
            104.84135720677189,
            40.425890646244326,
            207.30713445490164,
            20.03765752162595,
            477.44572646244325,
        ],
    ),
    (
        "glm-5.3",
        [
            315.4887223534703,
            460.07607497150275,
            158.94572791083243,
            886.236583923373,
            82.77471243741405,
            2063.9338231083243,
        ],
    ),
    (
        "kimi-k3",
        [
            821.9409173033514,
            1115.3175731615136,
            423.14240971416217,
            2947.4838411208652,
            399.3591351406703,
            10031.249013621622,
        ],
    ),
    (
        "gpt-oss-120b",
        [
            43.53649958747053,
            73.40618180289731,
            54.94784549396756,
            871.9033493054269,
            143.77116265250595,
            5722.400822939676,
        ],
    ),
    (
        "qwen3.5-397b-a17b",
        [
            151.44232529643244,
            205.61099645232434,
            99.46023992389189,
            1060.9514337487567,
            149.6404573792865,
            5668.926879238919,
        ],
    ),
    (
        "qwen3.8-2.4t-a95b",
        [
            780.709617395546,
            1062.7985873186594,
            448.8464474783135,
            4002.1290815142056,
            522.1546999271092,
            18823.059546783134,
        ],
    ),
    (
        "llama-3.3-70b-instruct",
        [
            568.0303641081081,
            902.9556650810811,
            569.6765369729729,
            8175.49905318919,
            1304.3388318616217,
            51671.74392972973,
        ],
    ),
    (
        "minimax-m3",
        [
            192.0134995350611,
            269.71285911662704,
            112.11827194672433,
            898.6433572241297,
            119.23493213350055,
            4018.923327467243,
        ],
    ),
    (
        "mimo-v2.6-flash",
        [
            117.18555907177168,
            169.39864782875676,
            77.75664984908109,
            818.3154514404324,
            118.94425416233513,
            4287.303458490811,
        ],
    ),
];

#[test]
fn the_integration_reproduces_an_independent_implementation() -> anyhow::Result<()> {
    let (input, weights) = shipped()?;
    let sets = compute(&input, &weights)?;
    for (id, expected) in INTEGRATED_BASE_CASE {
        let index = input
            .model
            .iter()
            .position(|model| model.id == id)
            .context("a fixture's model")?;
        let model = &input.model[index];
        let active = weights
            .model
            .iter()
            .find(|listed| listed.id == id)
            .context("active parameters")?;
        for (call, expected) in CALLS.into_iter().zip(expected) {
            let integrated = exact(
                model,
                active,
                &weights.anchor,
                &input.hardware.base,
                weights.hardware.base,
                call,
            )?;
            assert!(
                (integrated / expected - 1.0).abs() < 1e-9,
                "{id}: {integrated} against {expected}"
            );
            assert!(
                formula(&sets[index].base, call) / expected > 1.0 - 1e-9,
                "{id}"
            );
        }
    }
    Ok(())
}

#[test]
fn the_sets_bound_the_integrated_energy_from_above_and_meet_it_at_long_contexts()
-> anyhow::Result<()> {
    let (input, weights) = shipped()?;
    let sets = compute(&input, &weights)?;
    let scenarios = [
        (&input.hardware.low, weights.hardware.low),
        (&input.hardware.base, weights.hardware.base),
        (&input.hardware.high, weights.hardware.high),
    ];
    for (set, model) in sets.iter().zip(&input.model) {
        let active = weights
            .model
            .iter()
            .find(|listed| listed.id == model.id)
            .context("active parameters")?;
        for ((scenario, factor), values) in scenarios.iter().zip([&set.low, &set.base, &set.high]) {
            for call in CALLS {
                let integrated = exact(model, active, &weights.anchor, scenario, *factor, call)?;
                let ratio = formula(values, call) / integrated;
                assert!(
                    ratio > 1.0 - 1e-9,
                    "{} below the integrated energy: {ratio}",
                    model.id
                );
                // Caps count in full below them; the expanded form of latent
                // attention costs more than the absorbed one for few new tokens.
                assert!(ratio < 1.2, "{} at {ratio} for {}", model.id, call.cached);
                if call.cached >= 20_000.0 && call.new >= 500.0 {
                    assert!(ratio < 1.001, "{} at {ratio} for {}", model.id, call.cached);
                }
            }
        }
    }
    Ok(())
}

#[test]
fn full_attention_is_the_pair_formula_exactly() -> anyhow::Result<()> {
    let (input, weights) = shipped()?;
    let sets = compute(&input, &weights)?;
    let llama = input
        .model
        .iter()
        .position(|model| model.id == "llama-3.3-70b-instruct")
        .context("llama")?;
    // 80 layers x 2 x 64 heads x 256 FLOPs per pair at 1.05 pJ.
    let a_p = 80.0 * 2.0 * 64.0 * 256.0 * 1.05e-12 * 1e6;
    assert!((sets[llama].base.a_p - a_p).abs() < 1e-12);
    // 8 key-value heads x 256 x 2 bytes = 4,096 bytes per layer at 225 pJ.
    let a_d = 80.0 * 4096.0 * 225e-12 * 1e6;
    assert!((sets[llama].base.a_d - a_d).abs() < 1e-12);
    // 0.23 J x 70.554 / 37 x 0.65.
    assert!((sets[llama].base.b_in - 0.23 * 70.554 / 37.0 * 0.65).abs() < 1e-12);
    Ok(())
}

#[test]
fn the_output_holds_a_set_per_model_with_frontier_idle_and_is_reproducible() -> anyhow::Result<()> {
    let (input, weights) = shipped()?;
    let sets = compute(&input, &weights)?;
    let date = "2026-10-08".parse()?;
    let text = render(&input, &weights, &sets, PARAMETER_SETS, date)?;
    assert_eq!(text, render(&input, &weights, &sets, PARAMETER_SETS, date)?);
    let value: toml::Table = toml::from_str(&text)?;
    let records = value["parameter_set"].as_array().context("records")?;
    assert_eq!(records.len(), input.model.len());
    let kimi = records
        .iter()
        .filter_map(toml::Value::as_table)
        .find(|record| record["id"].as_str() == Some("kimi-k3"))
        .context("kimi-k3")?;
    assert_eq!(kimi["idle"]["base_case"].as_float(), Some(1.12));
    let a_p = kimi["a_p_uj_per_pair"].as_table().context("a_p")?;
    assert_eq!(a_p["lower_bound"].as_float(), a_p["low_case"].as_float());
    assert_eq!(a_p["label"].as_str(), Some("D"));
    let sources = value["sources"].as_table().context("sources")?;
    assert!(sources.contains_key("config-kimi-k3"));
    Ok(())
}

#[test]
fn a_calibration_model_without_a_layout_is_refused() -> anyhow::Result<()> {
    let (mut input, weights) = shipped()?;
    input.model.retain(|model| model.id != "glm-5.3");
    let error = compute(&input, &weights).err().context("accepted")?;
    assert!(
        format!("{error:#}")
            .contains("model glm-5.3 of `xtask/price-calibration.toml` has no attention layout")
    );
    Ok(())
}

#[test]
fn a_group_without_the_fields_of_its_kind_is_refused() -> anyhow::Result<()> {
    let (_, weights) = shipped()?;
    let text = LAYOUTS.replacen("topk = 2048\n", "", 1);
    let input = parse_input(&text)?;
    let error = compute(&input, &weights).err().context("accepted")?;
    let message = format!("{error:#}");
    assert!(
        message.contains("model glm-5.3, group 1: needs topk"),
        "{message}"
    );
    let text = LAYOUTS.replacen("state_bytes = 4\n", "state_bytes = 4\nwindow = 4\n", 1);
    let input = parse_input(&text)?;
    let error = compute(&input, &weights).err().context("accepted")?;
    assert!(format!("{error:#}").contains("window does not apply"));
    Ok(())
}

#[test]
fn the_shipped_file_is_what_the_command_writes() -> anyhow::Result<()> {
    let file = include_str!("../../../data/energy/open-weight-sets.toml");
    let reviewed = file
        .lines()
        .find_map(|line| line.strip_prefix("reviewed = "))
        .context("a review date")?;
    let (input, weights) = shipped()?;
    let sets = compute(&input, &weights)?;
    let text = render(&input, &weights, &sets, PARAMETER_SETS, reviewed.parse()?)?;
    assert!(
        text == file,
        "`data/energy/open-weight-sets.toml` is out of date; run `cargo xtask open-weight-sets --reviewed <date>`"
    );
    Ok(())
}
