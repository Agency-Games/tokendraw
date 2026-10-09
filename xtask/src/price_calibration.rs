//! The price calibration: the weight energy per USD that hosts of
//! open-weight models sell, from the endpoint files of the `OpenRouter` API
//! and the inputs in `xtask/price-calibration.toml`.
//!
//! For every host and model, the reference call's weight energy at load
//! over its price at that host (a host's endpoints at their median) is one
//! cell, at the prices the files list: those in force when they were
//! downloaded. The five values are the 5th and 25th percentile of the cells at the
//! low hardware factor, their median at the central factor, and the 75th and
//! 95th percentile at the high factor.

use std::collections::BTreeMap;
use std::fmt::Write as _;

use anyhow::{Context as _, bail};
use jiff::civil::Date;
use serde::{Deserialize, Serialize};

const J_PER_WH: f64 = 3600.0;

/// `xtask/price-calibration.toml`.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Input {
    id: String,
    description: String,
    chat_input_tokens: u64,
    chat_output_tokens: u64,
    pub(crate) anchor: Anchor,
    pub(crate) hardware: Hardware,
    markup: Factor,
    #[serde(default)]
    sources: BTreeMap<String, Source>,
    pub(crate) model: Vec<OpenModel>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Anchor {
    pub(crate) active_b: f64,
    pub(crate) in_j: f64,
    pub(crate) out_j: f64,
    pub(crate) sources: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Hardware {
    pub(crate) low: f64,
    pub(crate) central: f64,
    pub(crate) high: f64,
    pub(crate) sources: Vec<String>,
    basis: String,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct OpenModel {
    pub(crate) id: String,
    openrouter_slugs: Vec<String>,
    pub(crate) active_b: f64,
    pub(crate) active_prefill_b: Option<f64>,
    pub(crate) sources: Vec<String>,
    basis: String,
}

/// A five-value factor in the schema of the data files.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Factor {
    pub(crate) envelope_low: f64,
    pub(crate) plausible_low: f64,
    pub(crate) central: f64,
    pub(crate) plausible_high: f64,
    pub(crate) envelope_high: f64,
    pub(crate) label: String,
    pub(crate) sources: Vec<String>,
    pub(crate) basis: String,
}

/// A source in the schema of the data files.
#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct Source {
    title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    url: Option<String>,
    publisher: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    published: Option<String>,
    accessed: toml::value::Datetime,
    #[serde(skip_serializing_if = "Option::is_none")]
    licence: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    sha256: Option<String>,
}

pub struct EndpointFile {
    pub name: String,
    pub bytes: Vec<u8>,
    pub sha256: String,
}

#[derive(Deserialize)]
struct Endpoints {
    data: EndpointsData,
}

#[derive(Deserialize)]
struct EndpointsData {
    id: String,
    endpoints: Vec<Endpoint>,
}

#[derive(Deserialize)]
struct Endpoint {
    provider_name: String,
    model_id: String,
    pricing: Pricing,
}

/// Prices in USD per token, as decimal strings: the price in force when the
/// file was downloaded.
#[derive(Deserialize)]
struct Pricing {
    prompt: Option<String>,
    completion: Option<String>,
}

impl Pricing {
    fn listed(&self) -> Option<(f64, f64)> {
        let parse = |value: Option<&String>| {
            value
                .and_then(|text| text.parse::<f64>().ok())
                .filter(|price| price.is_finite() && *price > 0.0)
        };
        Some((
            parse(self.prompt.as_ref())?,
            parse(self.completion.as_ref())?,
        ))
    }
}

pub struct Calibration {
    pub five: [f64; 5],
    pub cells: usize,
    /// Endpoints left out: variants such as batch, and endpoints without a
    /// positive input and output price.
    pub skipped: usize,
}

pub fn calibrate(input: &Input, files: &[EndpointFile]) -> anyhow::Result<Calibration> {
    let mut models: BTreeMap<&str, &OpenModel> = BTreeMap::new();
    for model in &input.model {
        for slug in &model.openrouter_slugs {
            if models.insert(slug, model).is_some() {
                bail!("`xtask/price-calibration.toml` lists the slug {slug} twice");
            }
        }
    }
    let mut seen: BTreeMap<String, &str> = BTreeMap::new();
    // The reference call's price at each endpoint, by host and model; their
    // median makes the cell.
    let mut cells: BTreeMap<(String, &str), Vec<f64>> = BTreeMap::new();
    let mut skipped = 0;
    let (chat_in, chat_out) = chat(input);
    for file in files {
        let parsed: Endpoints = serde_json::from_slice(&file.bytes).with_context(|| {
            format!(
                "{} is not an OpenRouter endpoint file; download `https://openrouter.ai/api/v1/models/<slug>/endpoints`",
                file.name
            )
        })?;
        let slug = parsed.data.id;
        let Some(model) = models.get(slug.as_str()) else {
            bail!(
                "{} is for {slug}, which no model in `xtask/price-calibration.toml` lists",
                file.name
            );
        };
        if let Some(earlier) = seen.insert(slug.clone(), &file.name) {
            bail!(
                "{earlier} and {} are both for {slug}; give one file per slug",
                file.name
            );
        }
        for endpoint in parsed.data.endpoints {
            match endpoint.pricing.listed() {
                Some((prompt, completion)) if endpoint.model_id == slug => {
                    cells
                        .entry((endpoint.provider_name, model.id.as_str()))
                        .or_default()
                        .push(chat_in * prompt + chat_out * completion);
                }
                _ => skipped += 1,
            }
        }
    }
    for slug in models.keys() {
        if !seen.contains_key(*slug) {
            bail!(
                "no file for {slug}; download `https://openrouter.ai/api/v1/models/{slug}/endpoints`"
            );
        }
    }
    for model in &input.model {
        if !cells.keys().any(|(_, id)| *id == model.id) {
            bail!(
                "model {} has no endpoint with an input and an output price",
                model.id
            );
        }
    }
    let mut ratios: [Vec<f64>; 3] = [Vec::new(), Vec::new(), Vec::new()];
    for ((_, id), prices) in &cells {
        let model = input
            .model
            .iter()
            .find(|model| model.id == *id)
            .context("a cell of an unlisted model")?;
        let usd = median(prices);
        let factors = [
            input.hardware.low,
            input.hardware.central,
            input.hardware.high,
        ];
        for (ratios, factor) in ratios.iter_mut().zip(factors) {
            ratios.push(weight_wh(input, model, factor) / usd);
        }
    }
    let [low, central, high] = ratios;
    Ok(Calibration {
        five: [
            quantile(&low, 0.05),
            quantile(&low, 0.25),
            median(&central),
            quantile(&high, 0.75),
            quantile(&high, 0.95),
        ],
        cells: cells.len(),
        skipped,
    })
}

#[expect(
    clippy::cast_precision_loss,
    reason = "reference calls have a few thousand tokens"
)]
fn chat(input: &Input) -> (f64, f64) {
    (
        input.chat_input_tokens as f64,
        input.chat_output_tokens as f64,
    )
}

/// The reference call's weight energy at load in Wh on `model` at hardware
/// `factor`.
fn weight_wh(input: &Input, model: &OpenModel, factor: f64) -> f64 {
    let (chat_in, chat_out) = chat(input);
    let anchor = &input.anchor;
    let prefill = model.active_prefill_b.unwrap_or(model.active_b);
    (chat_in * anchor.in_j * prefill + chat_out * anchor.out_j * model.active_b) / anchor.active_b
        * factor
        / J_PER_WH
}

fn sorted(values: &[f64]) -> Vec<f64> {
    let mut values = values.to_vec();
    values.sort_by(f64::total_cmp);
    values
}

fn median(values: &[f64]) -> f64 {
    let values = sorted(values);
    let n = values.len();
    if n % 2 == 1 {
        values[n / 2]
    } else {
        f64::midpoint(values[n / 2 - 1], values[n / 2])
    }
}

/// The `q` quantile with linear interpolation between the closest ranks.
#[expect(
    clippy::cast_precision_loss,
    clippy::cast_possible_truncation,
    clippy::cast_sign_loss,
    reason = "a few hundred cells; the position is non-negative and below their count"
)]
fn quantile(values: &[f64], q: f64) -> f64 {
    let values = sorted(values);
    let position = q * (values.len() - 1) as f64;
    let low = position as usize;
    let high = (low + 1).min(values.len() - 1);
    values[low] + (values[high] - values[low]) * (position - low as f64)
}

#[derive(Serialize)]
struct Output {
    price_calibration: Vec<Record>,
    sources: BTreeMap<String, Source>,
}

#[derive(Serialize)]
struct Record {
    id: String,
    description: String,
    reviewed: toml::value::Datetime,
    chat_input_tokens: u64,
    chat_output_tokens: u64,
    anchor_in_j: f64,
    anchor_out_j: f64,
    wh_per_usd: Factor,
    markup: Factor,
}

/// The source id of the endpoint file for `slug`, without dots, which TOML
/// would read as nested keys in a table header.
fn source_id(slug: &str) -> String {
    format!("openrouter-{}", slug.replace(['/', '.'], "-"))
}

pub fn render(
    input: &Input,
    calibration: &Calibration,
    files: &[EndpointFile],
    accessed: Date,
) -> anyhow::Result<String> {
    let date: toml::value::Datetime = accessed
        .to_string()
        .parse()
        .context("the accessed date as TOML")?;
    let mut sources = input.sources.clone();
    let mut endpoint_sources = Vec::new();
    for file in files {
        let parsed: Endpoints = serde_json::from_slice(&file.bytes)?;
        let slug = parsed.data.id;
        let id = source_id(&slug);
        sources.insert(
            id.clone(),
            Source {
                title: format!("OpenRouter endpoints of {slug}: each host's prices"),
                url: Some(format!(
                    "https://openrouter.ai/api/v1/models/{slug}/endpoints"
                )),
                publisher: "OpenRouter".to_owned(),
                published: None,
                accessed: date,
                licence: None,
                sha256: Some(file.sha256.clone()),
            },
        );
        endpoint_sources.push(id);
    }
    endpoint_sources.sort();
    let mut cited = endpoint_sources;
    for id in input
        .model
        .iter()
        .flat_map(|model| &model.sources)
        .chain(&input.anchor.sources)
        .chain(&input.hardware.sources)
    {
        if !cited.contains(id) {
            cited.push(id.clone());
        }
    }
    let (chat_in, chat_out) = (input.chat_input_tokens, input.chat_output_tokens);
    let anchor = &input.anchor;
    let [
        envelope_low,
        plausible_low,
        central,
        plausible_high,
        envelope_high,
    ] = calibration.five;
    let wh_per_usd = Factor {
        envelope_low,
        plausible_low,
        central,
        plausible_high,
        envelope_high,
        label: "D".to_owned(),
        sources: cited,
        basis: format!(
            "Over {} host and model cells of OpenRouter's endpoints for {} open-weight models on {accessed}, the weight energy at load of a call of {chat_in} new input and {chat_out} output tokens, ({chat_in} x {} J x prefill active parameters + {chat_out} x {} J x active parameters) / {} billion times the hardware factor, divided by the call's price at the host (several endpoints of a host at their median). Envelope: 5th and 95th percentile at the low and the high hardware factor ({} and {}). Plausible: 25th and 75th percentile at the same factors. Central: the median at {}. {}",
            calibration.cells,
            input.model.len(),
            anchor.in_j,
            anchor.out_j,
            anchor.active_b,
            input.hardware.low,
            input.hardware.high,
            input.hardware.central,
            input.hardware.basis,
        ),
    };
    let output = Output {
        price_calibration: vec![Record {
            id: input.id.clone(),
            description: input.description.clone(),
            reviewed: date,
            chat_input_tokens: chat_in,
            chat_output_tokens: chat_out,
            anchor_in_j: anchor.in_j,
            anchor_out_j: anchor.out_j,
            wh_per_usd,
            markup: input.markup.clone(),
        }],
        sources,
    };
    let mut text = header(input);
    text.push_str(&toml::to_string(&output).context("the calibration as TOML")?);
    Ok(text)
}

fn header(input: &Input) -> String {
    let mut text = String::new();
    let _ = writeln!(
        text,
        "# Generated by `cargo xtask price-calibration` from OpenRouter's endpoint\n\
         # files and xtask/price-calibration.toml. Do not edit by hand; rerun the\n\
         # command with newer files. No OpenRouter data is copied here, only the\n\
         # five values computed from it.\n#\n\
         # Models and active parameters, billions (prefill / decode):"
    );
    for model in &input.model {
        let prefill = model
            .active_prefill_b
            .map_or_else(String::new, |prefill| format!("{prefill} / "));
        crate::ember::entry(
            &mut text,
            &format!("{}: {prefill}{}; {}", model.id, model.active_b, model.basis),
        );
    }
    text.push('\n');
    text
}

pub fn parse_input(text: &str) -> anyhow::Result<Input> {
    toml::from_str(text).context("`xtask/price-calibration.toml` does not fit its schema")
}

#[cfg(test)]
mod tests {
    use super::*;

    const INPUT: &str = r#"
id = "hosts"
description = "d"
chat_input_tokens = 1000
chat_output_tokens = 500

[anchor]
active_b = 37
in_j = 0.23
out_j = 0.38
sources = ["fleet"]

[hardware]
low = 0.5
central = 0.65
high = 1.0
sources = ["bench"]
basis = "b"

[markup]
envelope_low = 1
plausible_low = 1
central = 1.825
plausible_high = 3.33
envelope_high = 3.33
label = "D"
sources = ["margin"]
basis = "b"

[sources.margin]
title = "t"
publisher = "p"
accessed = 2026-10-07

[[model]]
id = "a"
openrouter_slugs = ["lab/a", "lab/a-0813"]
active_b = 37
sources = ["card-a"]
basis = "b"

[[model]]
id = "b"
openrouter_slugs = ["lab/b"]
active_b = 74
active_prefill_b = 37
sources = ["card-b"]
basis = "b"
"#;

    fn file(slug: &str, endpoints: &[(&str, &str, &str, &str)]) -> EndpointFile {
        let endpoints: Vec<String> = endpoints
            .iter()
            .map(|(host, model_id, prompt, completion)| {
                format!(
                    r#"{{"provider_name": "{host}", "model_id": "{model_id}", "pricing": {{"prompt": "{prompt}", "completion": "{completion}"}}}}"#
                )
            })
            .collect();
        let text = format!(
            r#"{{"data": {{"id": "{slug}", "endpoints": [{}]}}}}"#,
            endpoints.join(", ")
        );
        EndpointFile {
            name: format!("{}.json", slug.replace('/', "_")),
            sha256: "0".repeat(64),
            bytes: text.into_bytes(),
        }
    }

    fn files() -> Vec<EndpointFile> {
        vec![
            file(
                "lab/a",
                &[
                    // One host with two endpoints, at their median: $0.002 a call.
                    ("H1", "lab/a", "0.000001", "0.000002"),
                    ("H1", "lab/a", "0.000001", "0.000004"),
                    // A batch variant and a free endpoint are left out.
                    ("H1", "lab/a:batch", "0.0000001", "0.0000001"),
                    ("H2", "lab/a", "0", "0"),
                    // $0.004 a call.
                    ("H2", "lab/a", "0.000002", "0.000004"),
                ],
            ),
            file(
                "lab/a-0813",
                &[("H3", "lab/a-0813", "0.000001", "0.000002")],
            ),
            file("lab/b", &[("H1", "lab/b", "0.000002", "0.000008")]),
        ]
    }

    fn input() -> anyhow::Result<Input> {
        parse_input(INPUT)
    }

    fn refusal(files: &[EndpointFile]) -> anyhow::Result<String> {
        match calibrate(&input()?, files) {
            Ok(_) => bail!("the files were accepted"),
            Err(error) => Ok(format!("{error:#}")),
        }
    }

    #[test]
    fn cells_are_hosts_and_models_with_a_host_at_its_median() -> anyhow::Result<()> {
        let calibration = calibrate(&input()?, &files())?;
        // Cells: (H1, a) $0.0025 (median of 0.002 and 0.003), (H2, a) $0.004,
        // (H3, a) $0.002, (H1, b) $0.006.
        assert_eq!(calibration.cells, 4);
        assert_eq!(calibration.skipped, 2);
        // Model a at 37 billion: (1000 x 0.23 + 500 x 0.38) / 3600 Wh = 0.11667 Wh
        // at factor 1. Model b: (1000 x 0.23 x 1 + 500 x 0.38 x 2) / 3600 Wh.
        let a = 420.0 / 3600.0;
        let b = 610.0 / 3600.0;
        let mut central = [a / 0.0025, a / 0.004, a / 0.002, b / 0.006].map(|v| v * 0.65);
        central.sort_by(f64::total_cmp);
        let expected_central = f64::midpoint(central[1], central[2]);
        assert!((calibration.five[2] - expected_central).abs() < 1e-9);
        let mut high = [a / 0.0025, a / 0.004, a / 0.002, b / 0.006];
        high.sort_by(f64::total_cmp);
        // The 95th percentile of four values: position 2.85.
        let expected = high[2] + (high[3] - high[2]) * 0.85;
        assert!((calibration.five[4] - expected).abs() < 1e-9);
        assert!(calibration.five.windows(2).all(|pair| pair[0] <= pair[1]));
        Ok(())
    }

    #[test]
    fn a_slug_without_a_file_is_refused() -> anyhow::Result<()> {
        let mut files = files();
        files.remove(1);
        assert!(refusal(&files)?.contains("no file for lab/a-0813"));
        Ok(())
    }

    #[test]
    fn two_files_for_one_slug_are_refused() -> anyhow::Result<()> {
        let mut files = files();
        files.push(file("lab/b", &[("H9", "lab/b", "0.000001", "0.000001")]));
        assert!(refusal(&files)?.contains("are both for lab/b"));
        Ok(())
    }

    #[test]
    fn a_file_for_an_unlisted_slug_is_refused() -> anyhow::Result<()> {
        let mut files = files();
        files.push(file("lab/a:batch", &[]));
        assert!(refusal(&files)?.contains("lab/a:batch, which no model"));
        Ok(())
    }

    #[test]
    fn a_model_without_a_priced_endpoint_is_refused() -> anyhow::Result<()> {
        let mut files = files();
        files[2] = file("lab/b", &[("H1", "lab/b:batch", "0.000001", "0.000001")]);
        assert!(refusal(&files)?.contains("model b has no endpoint"));
        Ok(())
    }

    #[test]
    fn a_file_that_is_not_an_endpoint_file_is_refused() -> anyhow::Result<()> {
        let mut files = files();
        files[0].bytes = b"{}".to_vec();
        assert!(refusal(&files)?.contains("is not an OpenRouter endpoint file"));
        Ok(())
    }

    #[test]
    fn the_output_is_a_calibration_record_with_its_sources() -> anyhow::Result<()> {
        let input = input()?;
        let files = files();
        let calibration = calibrate(&input, &files)?;
        let text = render(&input, &calibration, &files, "2026-10-08".parse()?)?;
        let value: toml::Table = toml::from_str(&text)?;
        let record = value["price_calibration"]
            .as_array()
            .and_then(|records| records.first())
            .and_then(toml::Value::as_table)
            .context("one record")?;
        assert_eq!(record["id"].as_str(), Some("hosts"));
        assert_eq!(record["markup"]["central"].as_float(), Some(1.825));
        assert_eq!(
            record["wh_per_usd"]["central"].as_float(),
            Some(calibration.five[2])
        );
        let sources = value["sources"].as_table().context("sources")?;
        let endpoint = sources["openrouter-lab-a-0813"]
            .as_table()
            .context("an endpoint source")?;
        assert_eq!(
            endpoint["url"].as_str(),
            Some("https://openrouter.ai/api/v1/models/lab/a-0813/endpoints")
        );
        assert_eq!(endpoint["sha256"].as_str(), Some("0".repeat(64).as_str()));
        assert!(sources.contains_key("margin"));
        let cited: Vec<&str> = record["wh_per_usd"]["sources"]
            .as_array()
            .context("cited")?
            .iter()
            .filter_map(toml::Value::as_str)
            .collect();
        assert_eq!(
            cited,
            [
                "openrouter-lab-a",
                "openrouter-lab-a-0813",
                "openrouter-lab-b",
                "card-a",
                "card-b",
                "fleet",
                "bench"
            ]
        );
        Ok(())
    }
}
