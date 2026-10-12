//! Parameter sets of open-weight models from their published
//! configurations: the attention layouts in `xtask/open-weight-sets.toml`
//! and the active parameters, weight energy anchor and hardware factors in
//! `xtask/price-calibration.toml`.
//!
//! The per-call formula is
//! `a_p * prefill_pairs + a_d * decode_pairs + b_in * n_new + b_out * n_out + c0 * L`.
//! Attention whose cost grows with every earlier token (full attention,
//! compression without a top-k cap, lightning indexers) goes into `a_p` and
//! `a_d`; attention capped at a window, a top-k or a fixed state costs the
//! same per token beyond the cap and goes into `b_in` and `b_out` at the
//! cap. Each scenario gives one value; the bounds and the low and high case
//! are the low and the high scenario, the base case the base scenario.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Context as _, bail};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

use crate::price_calibration::{self, Factor, Source};

/// `xtask/open-weight-sets.toml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    hardware: Hardware,
    #[serde(default)]
    sources: BTreeMap<String, Source>,
    model: Vec<Model>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Hardware {
    sources: Vec<String>,
    basis: String,
    low: Scenario,
    base: Scenario,
    high: Scenario,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Scenario {
    description: String,
    e_flop_pj: f64,
    e_byte_pj: f64,
    reload_nj_per_byte: f64,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Model {
    id: String,
    name: String,
    hf_repo: String,
    sources: Vec<String>,
    basis: String,
    group: Vec<Group>,
}

/// A group of attention layers. Which fields apply depends on `kind` and
/// `layout`; [`check`] refuses a group without the fields its kind and
/// layout need.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Group {
    kind: Kind,
    layers: u32,
    layout: Layout,
    heads: u32,
    kv_heads: Option<u32>,
    qk_dim: Option<u32>,
    v_dim: Option<u32>,
    kv_bytes: Option<f64>,
    kv_lora: Option<u32>,
    rope: Option<u32>,
    head_dim: Option<u32>,
    kv_entry_bytes: Option<f64>,
    state_bytes: Option<f64>,
    window: Option<u32>,
    topk: Option<u32>,
    ratio: Option<u32>,
    index_heads: Option<u32>,
    index_dim: Option<u32>,
    index_bits: Option<u32>,
    index_layers: Option<u32>,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Kind {
    Full,
    Sliding,
    Sparse,
    Compressed,
    Linear,
}

#[derive(Clone, Copy, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
enum Layout {
    Gqa,
    Mla,
    SharedKv,
    Linear,
}

/// One model's five parameters on one scenario, in the units of the data
/// files: uJ per pair, J per token, mJ per cached token.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Values {
    pub a_p: f64,
    pub a_d: f64,
    pub b_in: f64,
    pub b_out: f64,
    pub c0: f64,
}

/// The parts of a scenario's values that a basis names.
#[derive(Clone, Copy, Default)]
struct Parts {
    values: Values,
    /// Weight energy in `b_in` and `b_out`, J per token.
    weights_in: f64,
    weights_out: f64,
    /// Key-value bytes a token stores over all layers.
    stored_bytes: f64,
}

/// A parameter set in the schema of the data files.
#[derive(Serialize)]
struct Record {
    id: String,
    description: String,
    reviewed: toml::value::Datetime,
    a_p_uj_per_pair: Factor,
    a_d_uj_per_pair: Factor,
    b_in_j_per_token: Factor,
    b_out_j_per_token: Factor,
    c0_mj_per_cached_token: Factor,
    idle: Factor,
}

#[derive(Serialize)]
struct Output {
    parameter_set: Vec<Record>,
    sources: BTreeMap<String, Source>,
}

const PJ: f64 = 1e-12;
const NJ: f64 = 1e-9;

impl Group {
    fn need<T: Copy>(value: Option<T>, field: &str) -> anyhow::Result<T> {
        value.with_context(|| format!("needs {field}"))
    }

    /// FLOPs per attended pair in prefill and in decode, and bytes stored
    /// per key-value entry.
    fn layout(&self) -> anyhow::Result<(f64, f64, f64)> {
        let heads = f64::from(self.heads);
        Ok(match self.layout {
            Layout::Gqa => {
                let width = f64::from(Self::need(self.qk_dim, "qk_dim")?)
                    + f64::from(Self::need(self.v_dim, "v_dim")?);
                let flops = 2.0 * heads * width;
                let entry = f64::from(Self::need(self.kv_heads, "kv_heads")?)
                    * width
                    * Self::need(self.kv_bytes, "kv_bytes")?;
                (flops, flops, entry)
            }
            Layout::Mla => {
                let prefill = 2.0
                    * heads
                    * (f64::from(Self::need(self.qk_dim, "qk_dim")?)
                        + f64::from(Self::need(self.v_dim, "v_dim")?));
                let latent = f64::from(Self::need(self.kv_lora, "kv_lora")?);
                let rope = f64::from(Self::need(self.rope, "rope")?);
                let decode = 2.0 * heads * (2.0 * latent + rope);
                let entry = (latent + rope) * Self::need(self.kv_bytes, "kv_bytes")?;
                (prefill, decode, entry)
            }
            Layout::SharedKv => {
                let flops = 2.0 * heads * 2.0 * f64::from(Self::need(self.head_dim, "head_dim")?);
                (
                    flops,
                    flops,
                    Self::need(self.kv_entry_bytes, "kv_entry_bytes")?,
                )
            }
            Layout::Linear => bail!("layout linear belongs to kind linear"),
        })
    }

    /// FLOPs to expand one cached latent into keys and values, for the
    /// expanded form of multi-head latent attention.
    fn expand_flops(&self) -> anyhow::Result<f64> {
        let qk = Self::need(self.qk_dim, "qk_dim")?;
        let rope = Self::need(self.rope, "rope")?;
        let nope = qk.checked_sub(rope).context("rope is wider than qk_dim")?;
        Ok(2.0
            * f64::from(Self::need(self.kv_lora, "kv_lora")?)
            * f64::from(self.heads)
            * (f64::from(nope) + f64::from(Self::need(self.v_dim, "v_dim")?)))
    }
}

fn check(model: &Model) -> anyhow::Result<()> {
    if model.group.is_empty() {
        bail!("model {} has no attention layers", model.id);
    }
    for (index, group) in model.group.iter().enumerate() {
        check_group(group).with_context(|| format!("model {}, group {}", model.id, index + 1))?;
    }
    Ok(())
}

fn check_group(group: &Group) -> anyhow::Result<()> {
    let set = |name: &'static str, present: bool| (name, present);
    let fields = [
        set("kv_heads", group.kv_heads.is_some()),
        set("qk_dim", group.qk_dim.is_some()),
        set("v_dim", group.v_dim.is_some()),
        set("kv_bytes", group.kv_bytes.is_some()),
        set("kv_lora", group.kv_lora.is_some()),
        set("rope", group.rope.is_some()),
        set("head_dim", group.head_dim.is_some()),
        set("kv_entry_bytes", group.kv_entry_bytes.is_some()),
        set("state_bytes", group.state_bytes.is_some()),
        set("window", group.window.is_some()),
        set("topk", group.topk.is_some()),
        set("ratio", group.ratio.is_some()),
        set("index_heads", group.index_heads.is_some()),
        set("index_dim", group.index_dim.is_some()),
        set("index_bits", group.index_bits.is_some()),
        set("index_layers", group.index_layers.is_some()),
    ];
    let mut used: Vec<&str> = match group.layout {
        Layout::Gqa => vec!["kv_heads", "qk_dim", "v_dim", "kv_bytes"],
        Layout::Mla => vec!["kv_lora", "rope", "qk_dim", "v_dim", "kv_bytes"],
        Layout::SharedKv => vec!["head_dim", "kv_entry_bytes"],
        Layout::Linear => vec!["qk_dim", "v_dim", "state_bytes"],
    };
    let required = used.clone();
    let index = ["index_heads", "index_dim", "index_bits", "index_layers"];
    match group.kind {
        Kind::Linear if group.layout != Layout::Linear => {
            bail!("kind linear needs layout linear")
        }
        Kind::Linear => {}
        _ if group.layout == Layout::Linear => bail!("layout linear needs kind linear"),
        Kind::Full => {}
        Kind::Sliding => used.push("window"),
        Kind::Sparse => used.extend(["topk", "window"].into_iter().chain(index)),
        Kind::Compressed => used.extend(["ratio", "topk", "window"].into_iter().chain(index)),
    }
    for name in required {
        if !fields
            .iter()
            .any(|(field, present)| *field == name && *present)
        {
            bail!("needs {name}");
        }
    }
    for (name, present) in fields {
        if present && !used.contains(&name) {
            bail!("{name} does not apply to this kind and layout");
        }
    }
    match group.kind {
        Kind::Sliding if group.window.is_none() => bail!("needs window"),
        Kind::Sparse if group.topk.is_none() => bail!("needs topk"),
        Kind::Compressed if group.ratio.is_none_or(|ratio| ratio == 0) => {
            bail!("needs a ratio above 0")
        }
        _ => {}
    }
    let indexer = index.map(|name| {
        fields
            .iter()
            .any(|(field, present)| *field == name && *present)
    });
    if indexer.iter().any(|present| *present) && !indexer.iter().all(|present| *present) {
        bail!("an indexer needs index_heads, index_dim, index_bits and index_layers");
    }
    if group
        .index_layers
        .is_some_and(|layers| layers > group.layers)
    {
        bail!("index_layers is above layers");
    }
    Ok(())
}

/// The values of `model` with `weights` (its active parameters) on one
/// scenario: energy per FLOP and per byte, reload energy and the weight
/// factor.
fn values(
    model: &Model,
    weights: &price_calibration::OpenModel,
    anchor: &price_calibration::Anchor,
    scenario: &Scenario,
    weight_factor: f64,
) -> anyhow::Result<Parts> {
    let e_flop = scenario.e_flop_pj * PJ;
    let e_byte = scenario.e_byte_pj * PJ;
    let mut v = Values::default();
    let mut stored = 0.0;
    for group in &model.group {
        let layers = f64::from(group.layers);
        if group.kind == Kind::Linear {
            let state = f64::from(group.heads)
                * f64::from(Group::need(group.qk_dim, "qk_dim")?)
                * f64::from(Group::need(group.v_dim, "v_dim")?);
            let state_bytes = Group::need(group.state_bytes, "state_bytes")?;
            v.b_in += layers * 4.0 * state * e_flop;
            v.b_out += layers * (state * state_bytes * e_byte).max(4.0 * state * e_flop);
            continue;
        }
        let (mut prefill, decode, entry) = group.layout()?;
        let decode_cost = (entry * e_byte).max(decode * e_flop);
        if group.layout == Layout::Mla {
            if group.kind == Kind::Full {
                // The expanded form: each cached and new token's latent is
                // expanded once per call.
                let expand = layers * group.expand_flops()? * e_flop;
                v.c0 += expand;
                v.b_in += expand;
            } else {
                // The absorbed form, since the context expanded for a
                // capped selection costs more than the selection.
                prefill = decode;
            }
        }
        let prefill_cost = prefill * e_flop;
        let window = f64::from(group.window.unwrap_or(0));
        let ratio = f64::from(group.ratio.unwrap_or(1));
        // Keys per token beyond the cap, and the share of every earlier
        // token attended.
        let (capped, per_pair) = match group.kind {
            Kind::Full => (0.0, 1.0),
            Kind::Sliding => (window, 0.0),
            Kind::Sparse => (f64::from(Group::need(group.topk, "topk")?) + window, 0.0),
            Kind::Compressed => match group.topk {
                Some(topk) => (f64::from(topk) + window, 0.0),
                None => (window, 1.0 / ratio),
            },
            Kind::Linear => unreachable!("handled above"),
        };
        v.a_p += layers * per_pair * prefill_cost;
        v.a_d += layers * per_pair * decode_cost;
        v.b_in += layers * capped * prefill_cost;
        v.b_out += layers * capped * decode_cost;
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
            let share = if group.kind == Kind::Compressed {
                1.0 / ratio
            } else {
                1.0
            };
            v.a_p += index_layers * share * flops * precision * e_flop;
            v.a_d += index_layers * share * (bytes * e_byte).max(flops * precision * e_flop);
        }
        stored += layers * entry / ratio;
    }
    v.c0 += stored * scenario.reload_nj_per_byte * NJ;
    let factor = weight_factor / anchor.active_b;
    let weights_in = anchor.in_j * weights.active_prefill_b.unwrap_or(weights.active_b) * factor;
    let weights_out = anchor.out_j * weights.active_b * factor;
    v.b_in += weights_in;
    v.b_out += weights_out;
    // To the units of the data files.
    v.a_p *= 1e6;
    v.a_d *= 1e6;
    v.c0 *= 1e3;
    Ok(Parts {
        values: v,
        weights_in,
        weights_out,
        stored_bytes: stored,
    })
}

pub struct Set {
    pub id: String,
    pub low: Values,
    pub base: Values,
    pub high: Values,
    parts: [Parts; 3],
}

pub fn compute(input: &Input, weights: &price_calibration::Input) -> anyhow::Result<Vec<Set>> {
    let mut seen = BTreeSet::new();
    for model in &input.model {
        if !seen.insert(model.id.as_str()) {
            bail!(
                "`xtask/open-weight-sets.toml` lists model {} twice",
                model.id
            );
        }
    }
    for listed in &weights.model {
        if !seen.contains(listed.id.as_str()) {
            bail!(
                "model {} of `xtask/price-calibration.toml` has no attention layout in `xtask/open-weight-sets.toml`",
                listed.id
            );
        }
    }
    let hardware = &weights.hardware;
    input
        .model
        .iter()
        .map(|model| {
            check(model)?;
            let active = weights
                .model
                .iter()
                .find(|listed| listed.id == model.id)
                .with_context(|| {
                    format!(
                        "model {} has no active parameters in `xtask/price-calibration.toml`",
                        model.id
                    )
                })?;
            let on = |scenario, factor| values(model, active, &weights.anchor, scenario, factor);
            let parts = [
                on(&input.hardware.low, hardware.low)?,
                on(&input.hardware.base, hardware.base)?,
                on(&input.hardware.high, hardware.high)?,
            ];
            Ok(Set {
                id: model.id.clone(),
                low: parts[0].values,
                base: parts[1].values,
                high: parts[2].values,
                parts,
            })
        })
        .collect()
}

/// The idle factor of the frontier set in `parameter_sets`, the text of
/// `data/energy/parameter-sets.toml`.
fn frontier_idle(parameter_sets: &str) -> anyhow::Result<Factor> {
    #[derive(Deserialize)]
    struct File {
        parameter_set: Vec<toml::Table>,
    }
    let file: File = toml::from_str(parameter_sets)
        .context("`data/energy/parameter-sets.toml` is not valid TOML")?;
    let frontier = file
        .parameter_set
        .into_iter()
        .find(|set| set.get("id").and_then(toml::Value::as_str) == Some("frontier"))
        .context("`data/energy/parameter-sets.toml` has no frontier set")?;
    frontier
        .get("idle")
        .cloned()
        .context("the frontier set has no idle factor")?
        .try_into()
        .context("the frontier set's idle factor does not fit its schema")
}

fn factor(set: &Set, pick: fn(&Values) -> f64, sources: Vec<String>, basis: String) -> Factor {
    let (low, base_case, high) = (pick(&set.low), pick(&set.base), pick(&set.high));
    Factor {
        lower_bound: low,
        low_case: low,
        base_case,
        high_case: high,
        upper_bound: high,
        label: "D".to_owned(),
        sources,
        basis,
    }
}

const BANDS: &str =
    "Bounds, low case and high case: the low and the high scenario; base case: the base scenario.";

struct Common<'a> {
    hardware: &'a Hardware,
    weights: &'a price_calibration::Input,
    scenarios: String,
    idle: Factor,
    reviewed: toml::value::Datetime,
}

impl Common<'_> {
    #[expect(
        clippy::too_many_lines,
        reason = "the five factors of one record with their bases"
    )]
    fn record(&self, model: &Model, set: &Set) -> anyhow::Result<Record> {
        let hw = self.hardware;
        let anchor = &self.weights.anchor;
        let factors = &self.weights.hardware;
        let active = self
            .weights
            .model
            .iter()
            .find(|listed| listed.id == model.id)
            .context("a set without active parameters")?;
        let attention: Vec<String> = model.sources.iter().chain(&hw.sources).cloned().collect();
        let mut both = attention.clone();
        for id in active
            .sources
            .iter()
            .chain(&anchor.sources)
            .chain(&factors.sources)
        {
            if !both.contains(id) {
                both.push(id.clone());
            }
        }
        let base = set.parts[1];
        let expand = if model
            .group
            .iter()
            .any(|group| group.layout == Layout::Mla && group.kind == Kind::Full)
        {
            (
                ", and expanding each new token's latent once",
                ", and expanding each cached latent into keys and values once per call",
            )
        } else {
            ("", "")
        };
        let scale = format!(
            "and by the hardware factor {} / {} / {}",
            factors.low, factors.base, factors.high
        );
        let scenarios = &self.scenarios;
        Ok(Record {
            id: set.id.clone(),
            description: format!(
                "{} ({}) from its published configuration: {}",
                model.name, model.hf_repo, model.basis
            ),
            reviewed: self.reviewed,
            a_p_uj_per_pair: factor(
                set,
                |v| v.a_p,
                attention.clone(),
                format!(
                    "Prefill attention whose cost grows with every earlier token: full attention, compressed attention without a top-k cap (over its ratio) and lightning indexers, FLOPs per pair of a new token and an earlier token times energy per FLOP. Capped attention is in b_in. {BANDS} {scenarios}"
                ),
            ),
            a_d_uj_per_pair: factor(
                set,
                |v| v.a_d,
                attention.clone(),
                format!(
                    "Decode attention whose cost grows with every earlier token, as for a_p: per pair of an output token and an earlier token, the larger of the key-value bytes read times energy per byte and the FLOPs times energy per FLOP. Capped attention is in b_out. {BANDS}"
                ),
            ),
            b_in_j_per_token: factor(
                set,
                |v| v.b_in,
                both.clone(),
                format!(
                    "Weights: {} J per new token at {} billion active parameters on H800 (DeepSeek's fleet), scaled to {} billion active in prefill {scale}, {:.4} J base case. Attention capped at a window, a top-k selection or a fixed state, at its cap{}, {:.4} J base case. {BANDS}",
                    anchor.in_j,
                    anchor.active_b,
                    active.active_prefill_b.unwrap_or(active.active_b),
                    base.weights_in,
                    expand.0,
                    base.values.b_in - base.weights_in,
                ),
            ),
            b_out_j_per_token: factor(
                set,
                |v| v.b_out,
                both,
                format!(
                    "Weights: {} J per output token at {} billion active parameters on H800, scaled to {} billion active {scale}, {:.4} J base case. Attention capped at a window, a top-k selection or a fixed state, at its cap, {:.4} J base case. {BANDS}",
                    anchor.out_j,
                    anchor.active_b,
                    active.active_b,
                    base.weights_out,
                    base.values.b_out - base.weights_out,
                ),
            ),
            c0_mj_per_cached_token: factor(
                set,
                |v| v.c0,
                attention,
                format!(
                    "Reloading the {:.0} bytes of key-value entries a cached token holds over all layers (compressed entries over their ratio) from host memory once per call at {} / {} / {} nJ per byte{}. {BANDS}",
                    base.stored_bytes,
                    hw.low.reload_nj_per_byte,
                    hw.base.reload_nj_per_byte,
                    hw.high.reload_nj_per_byte,
                    expand.1,
                ),
            ),
            idle: self.idle.clone(),
        })
    }
}

pub fn render(
    input: &Input,
    weights: &price_calibration::Input,
    sets: &[Set],
    parameter_sets: &str,
    reviewed: Date,
) -> anyhow::Result<String> {
    let hw = &input.hardware;
    let common = Common {
        hardware: hw,
        weights,
        scenarios: format!(
            "Scenarios: low {} ({} pJ per FLOP, {} pJ per byte), base {} ({} pJ, {} pJ), high {} ({} pJ, {} pJ). {}",
            hw.low.description,
            hw.low.e_flop_pj,
            hw.low.e_byte_pj,
            hw.base.description,
            hw.base.e_flop_pj,
            hw.base.e_byte_pj,
            hw.high.description,
            hw.high.e_flop_pj,
            hw.high.e_byte_pj,
            hw.basis,
        ),
        idle: frontier_idle(parameter_sets)?,
        reviewed: reviewed
            .to_string()
            .parse()
            .context("the review date as TOML")?,
    };
    let output = Output {
        parameter_set: input
            .model
            .iter()
            .zip(sets)
            .map(|(model, set)| common.record(model, set))
            .collect::<anyhow::Result<_>>()?,
        sources: input.sources.clone(),
    };
    let mut text = String::new();
    let _ = writeln!(
        text,
        "# Generated by `cargo xtask open-weight-sets` from xtask/open-weight-sets.toml,\n\
         # xtask/price-calibration.toml and the frontier set's idle factor in\n\
         # data/energy/parameter-sets.toml. Do not edit by hand; change the inputs\n\
         # and rerun the command.\n"
    );
    text.push_str(&toml::to_string(&output).context("the parameter sets as TOML")?);
    Ok(text)
}

pub fn parse_input(text: &str) -> anyhow::Result<Input> {
    toml::from_str(text).context("`xtask/open-weight-sets.toml` does not fit its schema")
}

#[cfg(test)]
mod tests;
