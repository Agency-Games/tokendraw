//! Everything the tool writes to stdout.

use std::io::{self, Write};
use std::path::{MAIN_SEPARATOR, Path, PathBuf};

use jiff::civil::Date;
use serde::Serialize;

use crate::commands::label::Change;
use crate::commands::projects::{Inventory, quoted};
use crate::commands::tokens::Report;
use crate::data::{Action, Data, Factor, Five, Stale};
use crate::ledger::{FinalUsage, Forgotten, LabelRecord, Target, Unrecorded};
use crate::projects::{Elsewhere, PROJECT, is_pattern};
use crate::quantity::{GramsCo2ePerKwh, Tokens};

mod projects;
pub(crate) mod redact;
mod report;

/// The version of the JSON of `tokendraw label list`; it changes only when
/// a field is removed or changed.
const LABELS_SCHEMA_VERSION: u32 = 1;

fn shown_path(path: &str) -> String {
    let Some(home) = std::env::home_dir() else {
        return path.to_owned();
    };
    let home = home.to_string_lossy();
    match path.strip_prefix(home.as_ref()) {
        Some("") => "~".to_owned(),
        Some(rest) if rest.starts_with(MAIN_SEPARATOR) => format!("~{rest}"),
        _ => path.to_owned(),
    }
}

/// Why a path given to `report --project` selects nothing, and the command
/// that reports the calls made there.
#[must_use]
pub fn unselected(name: &str, elsewhere: &Elsewhere) -> String {
    let name = shown_path(name);
    let Some((first, _)) = elsewhere.projects.first() else {
        return match elsewhere.unresolved {
            0 => format!(
                "no calls were made in {name} or attributed to it. Run `tokendraw projects` to list the projects."
            ),
            1 => format!(
                "{name} is no project: the call made there belongs to no project. Run `tokendraw projects` to list it with the labels that would assign it."
            ),
            calls => format!(
                "{name} is no project: the {} calls made there belong to no project. Run `tokendraw projects` to list them with the labels that would assign them.",
                count(calls)
            ),
        };
    };
    let mut projects: Vec<String> = elsewhere
        .projects
        .iter()
        .take(3)
        .map(|(project, _)| shown_path(project))
        .collect();
    match elsewhere.projects.len() {
        0..=3 => {}
        4 => projects.push("1 other project".to_owned()),
        many => projects.push(format!("{} other projects", count(many as u64 - 3))),
    }
    let projects = match projects.split_last() {
        Some((last, [])) => last.clone(),
        Some((last, rest)) => format!("{} and {last}", rest.join(", ")),
        None => String::new(),
    };
    let report = format!("tokendraw report --project {}", quoted(&shown_path(first)));
    match elsewhere.unresolved {
        0 => format!(
            "{name} is no project: the calls made there are attributed to {projects}. Run `{report}`."
        ),
        unresolved => {
            let assigned: u64 = elsewhere.projects.iter().map(|(_, calls)| calls).sum();
            format!(
                "{name} is no project: of the calls made there, {} attributed to {projects} and {} to no project. Run `{report}`, or `tokendraw projects` to assign the rest.",
                match assigned {
                    1 => "1 is".to_owned(),
                    calls => format!("{} are", count(calls)),
                },
                match unresolved {
                    1 => "1 belongs".to_owned(),
                    calls => format!("{} belong", count(calls)),
                },
            )
        }
    }
}

pub fn projects(inventory: &Inventory, json: bool) -> io::Result<()> {
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, inventory)?;
        writeln!(out)?;
    } else {
        projects::write(&mut out, inventory)?;
    }
    out.flush()
}

fn shown_value(key: &str, value: &str) -> String {
    if key == PROJECT {
        shown_path(value)
    } else {
        value.to_owned()
    }
}

pub fn labelled(change: &Change) -> io::Result<()> {
    let mut out = io::stdout().lock();
    let calls = match change.calls {
        1 => "1 call in the ledger".to_owned(),
        calls => format!("{} calls in the ledger", count(calls)),
    };
    let (target, calls) = match &change.target {
        Target::Directory(pattern) if is_pattern(pattern) => (
            format!("{} and every directory below a match", shown_path(pattern)),
            format!(
                "{calls} in {} matching {} that no checkout holds",
                count(change.directories.unwrap_or_default()),
                if change.directories == Some(1) {
                    "directory"
                } else {
                    "directories"
                }
            ),
        ),
        Target::Directory(dir) => (
            format!("{} and every directory below it", shown_path(dir)),
            format!(
                "{calls} {} made there",
                if change.calls == 1 { "was" } else { "were" }
            ),
        ),
        Target::Session { client, session_id } => (
            format!("session {session_id} of {client}"),
            format!(
                "{calls} {} to the session",
                if change.calls == 1 {
                    "belongs"
                } else {
                    "belong"
                }
            ),
        ),
    };
    if !change.set.is_empty() {
        let set: Vec<String> = change
            .set
            .iter()
            .map(|(key, value)| format!("{key}={}", shown_value(key, value)))
            .collect();
        writeln!(out, "labelled {target}: {}", set.join(", "))?;
    }
    if !change.removed.is_empty() {
        writeln!(out, "removed {} from {target}", change.removed.join(", "))?;
    }
    writeln!(out, "{calls}")?;
    out.flush()
}

#[derive(Serialize)]
struct LabelList<'a> {
    schema_version: u32,
    labels: Vec<LabelEntry<'a>>,
}

#[derive(Serialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
enum LabelEntry<'a> {
    Directory {
        directory: &'a str,
        key: &'a str,
        value: &'a str,
    },
    Session {
        client: &'a str,
        session_id: &'a str,
        key: &'a str,
        value: &'a str,
    },
}

pub fn labels(labels: &[LabelRecord], json: bool) -> io::Result<()> {
    let mut out = io::stdout().lock();
    if json {
        let list = LabelList {
            schema_version: LABELS_SCHEMA_VERSION,
            labels: labels
                .iter()
                .map(|label| match &label.target {
                    Target::Directory(directory) => LabelEntry::Directory {
                        directory,
                        key: &label.key,
                        value: &label.value,
                    },
                    Target::Session { client, session_id } => LabelEntry::Session {
                        client,
                        session_id,
                        key: &label.key,
                        value: &label.value,
                    },
                })
                .collect(),
        };
        serde_json::to_writer_pretty(&mut out, &list)?;
        writeln!(out)?;
        return out.flush();
    }
    if labels.is_empty() {
        writeln!(out, "no labels")?;
        return out.flush();
    }
    let mut directories = vec![report::cells(["Directory labels"])];
    let mut sessions = vec![report::cells(["Session labels"])];
    for label in labels {
        let value = format!("{}={}", label.key, shown_value(&label.key, &label.value));
        match &label.target {
            Target::Directory(dir) => {
                directories.push(vec![
                    format!("{}{}", report::INDENT, shown_path(dir)),
                    value,
                ]);
            }
            Target::Session { client, session_id } => sessions.push(vec![
                format!("{}{client}", report::INDENT),
                session_id.clone(),
                value,
            ]),
        }
    }
    let mut first = true;
    for (table, aligns) in [
        (directories, &[report::Align::Left, report::Align::Left][..]),
        (
            sessions,
            &[
                report::Align::Left,
                report::Align::Left,
                report::Align::Left,
            ][..],
        ),
    ] {
        if table.len() == 1 {
            continue;
        }
        if !first {
            writeln!(out)?;
        }
        first = false;
        report::write_table(&mut out, "", aligns, &table)?;
    }
    out.flush()
}

pub fn tokens(report: &Report, json: bool) -> io::Result<()> {
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, report)?;
        writeln!(out)?;
    } else {
        write_tokens(&mut out, report)?;
    }
    out.flush()
}

pub fn report(
    report: &crate::commands::report::Report,
    json: bool,
    verbose: bool,
) -> io::Result<()> {
    let mut out = io::stdout().lock();
    if json {
        serde_json::to_writer_pretty(&mut out, report)?;
        writeln!(out)?;
    } else {
        report::write(&mut out, report, verbose)?;
    }
    out.flush()
}

pub fn forgotten(client: &str, forgotten: Forgotten) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(
        out,
        "forgot {} calls and {} files of {client}; the next `tokendraw tokens` reads its logs again",
        count(forgotten.calls),
        count(forgotten.files),
    )?;
    out.flush()
}

#[derive(Serialize)]
struct DataReport<'a> {
    #[serde(flatten)]
    data: &'a Data,
    stale: Vec<Stale>,
}

pub fn data(data: &Data, today: Date, json: bool) -> io::Result<()> {
    let mut out = io::stdout().lock();
    let report = DataReport {
        data,
        stale: data.stale(today),
    };
    if json {
        serde_json::to_writer_pretty(&mut out, &report)?;
        writeln!(out)?;
    } else {
        write_data(&mut out, &report)?;
    }
    out.flush()
}

pub fn exported(dir: &Path, written: &[PathBuf]) -> io::Result<()> {
    let mut out = io::stdout().lock();
    writeln!(
        out,
        "wrote {} shipped data files to {}",
        written.len(),
        dir.display()
    )?;
    for file in written {
        writeln!(out, "  {}", file.display())?;
    }
    out.flush()
}

fn write_data(out: &mut impl Write, report: &DataReport<'_>) -> io::Result<()> {
    write_version(out, report)?;
    write_records(out, report.data)
}

fn write_version(out: &mut impl Write, report: &DataReport<'_>) -> io::Result<()> {
    let data = report.data;
    let version = &data.version;
    writeln!(
        out,
        "data {}, shipped sha256 {}",
        version.id, version.shipped_sha256
    )?;
    writeln!(out, "shipped files, sha256")?;
    let width = version
        .files
        .iter()
        .map(|file| file.path.len())
        .max()
        .unwrap_or(0);
    for file in &version.files {
        writeln!(out, "  {:<width$}  {}", file.path, file.sha256)?;
    }
    if version.overrides.is_empty() {
        writeln!(out, "overrides: none")?;
    } else {
        writeln!(out, "overrides, sha256")?;
        for file in &version.overrides {
            writeln!(out, "  {}  {}", file.file.path, file.file.sha256)?;
            for record in &file.records {
                let action = match record.action {
                    Action::Replaced => "replaces",
                    Action::Added => "adds",
                };
                writeln!(out, "    {action} {} {}", record.kind, record.id)?;
            }
        }
    }
    if report.stale.is_empty() {
        writeln!(
            out,
            "stale: none (every record reviewed within {} days, every price within {})",
            data.review_interval_days, data.price_review_interval_days
        )?;
    } else {
        writeln!(
            out,
            "stale: reviewed more than {} days ago, a price more than {}",
            data.review_interval_days, data.price_review_interval_days
        )?;
        for stale in &report.stale {
            writeln!(
                out,
                "  {} {}, reviewed {}",
                stale.kind, stale.id, stale.reviewed
            )?;
        }
    }
    Ok(())
}

fn write_records(out: &mut impl Write, data: &Data) -> io::Result<()> {
    writeln!(out)?;
    writeln!(
        out,
        "values: envelope low / plausible low / central / plausible high / envelope high, label"
    )?;
    for set in data.parameter_sets.values() {
        writeln!(out)?;
        writeln!(out, "parameter set {}, reviewed {}", set.id, set.reviewed)?;
        writeln!(out, "  {}", set.description)?;
        let rows = [
            ("a_p, uJ per pair", &set.a_p_uj_per_pair),
            ("a_d, uJ per pair", &set.a_d_uj_per_pair),
            ("b_in, J per token", &set.b_in_j_per_token),
            ("b_out, J per token", &set.b_out_j_per_token),
            ("c0, mJ per token", &set.c0_mj_per_cached_token),
            ("idle", &set.idle),
        ];
        for (name, factor) in rows {
            writeln!(out, "  {name:<20}{}", factor_values(factor))?;
        }
    }
    write_prices(out, data)?;
    writeln!(out)?;
    writeln!(out, "models: pattern, parameter set, price, weights, label")?;
    let models: Vec<[String; 5]> = data
        .models
        .values()
        .map(|model| {
            [
                model.pattern.clone(),
                model.parameter_set.clone(),
                model.price.clone().unwrap_or_else(|| "-".to_owned()),
                format!("{:?}", model.weights).to_lowercase(),
                format!("{:?}", model.label),
            ]
        })
        .collect();
    let widths: [usize; 4] = std::array::from_fn(|column| {
        models
            .iter()
            .map(|row| row[column].len() + 2)
            .max()
            .unwrap_or(0)
    });
    for [pattern, set, price, weights, label] in &models {
        writeln!(
            out,
            "  {pattern:<w0$}{set:<w1$}{price:<w2$}{weights:<w3$}{label}",
            w0 = widths[0],
            w1 = widths[1],
            w2 = widths[2],
            w3 = widths[3],
        )?;
    }
    writeln!(out)?;
    writeln!(out, "providers: grid g CO2e per kWh, PUE")?;
    let width = data
        .providers
        .values()
        .map(|provider| provider.provider.id.len() + 2)
        .max()
        .unwrap_or(0);
    for provider in data.providers.values() {
        writeln!(
            out,
            "  {:<width$}grid {}  {:?}",
            provider.provider.id,
            grid(&provider.grid),
            provider.provider.label
        )?;
        writeln!(
            out,
            "  {:<width$}PUE  {}",
            "",
            factor_values(&provider.provider.pue)
        )?;
        writeln!(
            out,
            "  {:<width$}sites {}",
            "",
            provider.site_ids.join(", ")
        )?;
    }
    write_locations(out, data)?;
    write_findings(out, data)?;
    writeln!(out)?;
    writeln!(
        out,
        "geographies: {}; `tokendraw data --json` lists them with every source",
        count(data.geographies.len() as u64)
    )?;
    Ok(())
}

fn write_findings(out: &mut impl Write, data: &Data) -> io::Result<()> {
    writeln!(out)?;
    writeln!(out, "findings: parameters, label")?;
    for finding in data.findings.values() {
        let mut parameters = Vec::new();
        if let Some(tokens) = finding.cached_tokens {
            parameters.push(format!("cached_tokens {}", count(tokens)));
        }
        writeln!(
            out,
            "  {}   {}   {:?}",
            finding.id,
            parameters.join(", "),
            finding.label
        )?;
    }
    Ok(())
}

fn write_locations(out: &mut impl Write, data: &Data) -> io::Result<()> {
    writeln!(out)?;
    writeln!(
        out,
        "locations of clients without an adapter, first existing path per OS: label, os, path"
    )?;
    let width = data
        .locations
        .values()
        .map(|location| location.id.len() + 2)
        .max()
        .unwrap_or(0);
    for location in data.locations.values() {
        for (index, path) in location.paths.iter().enumerate() {
            let os: Vec<String> = path
                .os
                .iter()
                .map(|os| format!("{os:?}").to_lowercase())
                .collect();
            let id = if index == 0 { location.id.as_str() } else { "" };
            let label = if index == 0 {
                format!("{:?}", location.label)
            } else {
                String::new()
            };
            writeln!(
                out,
                "  {id:<width$}{label:<3}{:<23}{}",
                os.join(", "),
                path.path
            )?;
        }
    }
    Ok(())
}

fn write_prices(out: &mut impl Write, data: &Data) -> io::Result<()> {
    use std::fmt::Write as _;
    for calibration in data.price_calibrations.values() {
        writeln!(out)?;
        writeln!(
            out,
            "price calibration {}, reviewed {}",
            calibration.id, calibration.reviewed
        )?;
        writeln!(out, "  {}", calibration.description)?;
        writeln!(
            out,
            "  {:<20}{}",
            "Wh per USD",
            factor_values(&calibration.wh_per_usd)
        )?;
        writeln!(
            out,
            "  {:<20}{}",
            "markup",
            factor_values(&calibration.markup)
        )?;
    }
    if !data.prices.is_empty() {
        writeln!(out)?;
        writeln!(
            out,
            "prices: input / output USD per million tokens, label; above a prompt of N tokens"
        )?;
        for price in data.prices.values() {
            let mut line = format!(
                "  {:<28}{} / {}  {:?}",
                price.id, price.input_usd_per_mtok, price.output_usd_per_mtok, price.label
            );
            for tier in &price.tier {
                let _ = write!(
                    line,
                    "; above {}: {} / {}",
                    count(tier.above_prompt_tokens),
                    tier.input_usd_per_mtok,
                    tier.output_usd_per_mtok
                );
            }
            writeln!(out, "{line}")?;
        }
    }
    if !data.priced_sets.is_empty() {
        writeln!(out)?;
        writeln!(
            out,
            "parameter sets from prices: b_in / b_out central, J per token; the other values as their base set"
        )?;
        for set in data.priced_sets.values() {
            writeln!(
                out,
                "  {:<44}{} / {}  {:?}",
                set.id,
                report::significant(set.b_in_j_per_token.central, 3),
                report::significant(set.b_out_j_per_token.central, 3),
                set.b_in_j_per_token.label
            )?;
        }
    }
    Ok(())
}

fn factor_values(factor: &Factor) -> String {
    format!("{}  {:?}", five(&factor.five()), factor.label)
}

/// Five values as the data states them.
fn five(values: &Five<f64>) -> String {
    format!(
        "{} / {} / {} / {} / {}",
        values.envelope_low,
        values.plausible_low,
        values.central,
        values.plausible_high,
        values.envelope_high
    )
}

/// Five computed grid intensities, to one decimal place like Ember's.
fn grid(values: &Five<GramsCo2ePerKwh>) -> String {
    format!(
        "{:.1} / {:.1} / {:.1} / {:.1} / {:.1}",
        values.envelope_low.0,
        values.plausible_low.0,
        values.central.0,
        values.plausible_high.0,
        values.envelope_high.0
    )
}

fn write_tokens(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let range = match (report.since, report.until) {
        (None, None) => "all calls".to_owned(),
        (Some(since), None) => format!("calls since {since}"),
        (None, Some(until)) => format!("calls before {until}"),
        (Some(since), Some(until)) => format!("calls from {since} to before {until}"),
    };
    for (index, client) in report.clients.iter().enumerate() {
        if index > 0 {
            writeln!(out)?;
        }
        writeln!(out, "{}, {range}", client.client)?;
        let tokens = &client.tokens;
        let reasoning = tokens.reasoning.map_or_else(
            || "not recorded".to_owned(),
            |reasoning| {
                format!(
                    "{} in the {} calls that record it",
                    count(reasoning.0),
                    count(client.calls_with_reasoning)
                )
            },
        );
        let mut rows = vec![
            ("calls", count(client.calls)),
            ("sessions", count(client.sessions)),
            ("uncached input", tokens_count(tokens.uncached_input)),
            ("cache write", tokens_count(tokens.cache_write)),
            ("cache read", tokens_count(tokens.cache_read)),
            ("output", tokens_count(tokens.output)),
            ("of which reasoning", reasoning),
        ];
        rows.extend(final_usage_rows(&client.final_usage));
        rows.extend(unrecorded_rows(&client.unrecorded));
        rows.extend([
            (
                "earliest call",
                client
                    .earliest_call
                    .map_or_else(|| "none".to_owned(), |time| time.to_string()),
            ),
            (
                "latest call",
                client
                    .latest_call
                    .map_or_else(|| "none".to_owned(), |time| time.to_string()),
            ),
        ]);
        write_rows(out, &rows)?;
        let sources = &client.sources;
        writeln!(out, "{} logs read so far", client.client)?;
        write_rows(
            out,
            &[
                ("files", count(sources.files)),
                ("unreadable lines", count(sources.unreadable_lines)),
                ("files without usage", count(sources.files_without_usage)),
            ],
        )?;
    }
    Ok(())
}

fn final_usage_rows(usage: &FinalUsage) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if usage.missing > 0 {
        rows.push((
            "without final usage",
            format!(
                "{}, {} output tokens logged",
                report::calls(usage.missing),
                count(usage.missing_logged_output.0)
            ),
        ));
        let estimated = usage.missing - usage.missing_without_estimate;
        if estimated > 0 {
            rows.push((
                "estimated missing",
                format!(
                    "{} output tokens in {}, an estimate",
                    count(usage.estimated_missing_output.0),
                    report::calls(estimated)
                ),
            ));
        }
        if usage.missing_without_estimate > 0 {
            rows.push((
                "without estimate",
                format!(
                    "{} with too few comparable calls",
                    report::calls(usage.missing_without_estimate)
                ),
            ));
        }
    }
    if usage.unknown > 0 {
        rows.push((
            "final usage unknown",
            format!(
                "{} whose logs were deleted before it was recorded",
                report::calls(usage.unknown)
            ),
        ));
    }
    rows
}

fn unrecorded_rows(unrecorded: &Unrecorded) -> Vec<(&'static str, String)> {
    let mut rows = Vec::new();
    if unrecorded.calls == 0 {
        return rows;
    }
    rows.push(("no usage record", report::compactions(unrecorded.calls)));
    let with_input = unrecorded.calls - unrecorded.without_input_estimate;
    if with_input > 0 {
        rows.push((
            "estimated input",
            format!(
                "{} uncached and {} cache read for {}, an estimate",
                count(unrecorded.estimated_uncached_input.0),
                count(unrecorded.estimated_cache_read.0),
                report::compactions(with_input)
            ),
        ));
    }
    let with_output = unrecorded.calls - unrecorded.without_output_estimate;
    if with_output > 0 {
        rows.push((
            "estimated output",
            format!(
                "{} tokens for {}, an estimate",
                count(unrecorded.estimated_output.0),
                report::compactions(with_output)
            ),
        ));
    }
    if unrecorded.without_input_estimate > 0 {
        rows.push((
            "no input estimate",
            format!(
                "{} without an earlier call or enough comparable compactions",
                report::compactions(unrecorded.without_input_estimate)
            ),
        ));
    }
    if unrecorded.without_output_estimate > 0 {
        rows.push((
            "no output estimate",
            format!(
                "{} without enough comparable compactions",
                report::compactions(unrecorded.without_output_estimate)
            ),
        ));
    }
    rows
}

fn write_rows(out: &mut impl Write, rows: &[(&str, String)]) -> io::Result<()> {
    for (label, value) in rows {
        writeln!(out, "  {label:<20}{value}")?;
    }
    Ok(())
}

fn tokens_count(tokens: Tokens) -> String {
    count(tokens.0)
}

/// Digits in groups of three, so large counts can be read and compared.
fn count(value: u64) -> String {
    let digits = value.to_string();
    let mut grouped = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, digit) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            grouped.push(',');
        }
        grouped.push(digit);
    }
    grouped
}

#[cfg(test)]
mod tests {
    use super::{count, unselected};
    use crate::projects::Elsewhere;

    #[test]
    fn a_refusal_names_at_most_three_projects_and_counts_the_rest() {
        let elsewhere = |projects: &[(&str, u64)], unresolved| Elsewhere {
            projects: projects
                .iter()
                .map(|(project, calls)| ((*project).to_owned(), *calls))
                .collect(),
            unresolved,
        };
        assert_eq!(
            unselected("/w", &elsewhere(&[], 1)),
            "/w is no project: the call made there belongs to no project. Run `tokendraw projects` to list it with the labels that would assign it."
        );
        assert_eq!(
            unselected("/w", &elsewhere(&[], 1200)),
            "/w is no project: the 1,200 calls made there belong to no project. Run `tokendraw projects` to list them with the labels that would assign them."
        );
        assert_eq!(
            unselected("/w", &elsewhere(&[("/a", 1), ("/my b", 1)], 0)),
            "/w is no project: the calls made there are attributed to /a and /my b. Run `tokendraw report --project /a`."
        );
        let five = [("/a", 5), ("/b", 4), ("/c", 3), ("/d", 2), ("/e", 1)];
        assert_eq!(
            unselected("/w", &elsewhere(&five, 0)),
            "/w is no project: the calls made there are attributed to /a, /b, /c and 2 other projects. Run `tokendraw report --project /a`."
        );
        assert_eq!(
            unselected("/w", &elsewhere(&five[..4], 2)),
            "/w is no project: of the calls made there, 14 are attributed to /a, /b, /c and 1 other project and 2 belong to no project. Run `tokendraw report --project /a`, or `tokendraw projects` to assign the rest."
        );
    }

    #[test]
    fn counts_are_grouped_in_thousands() {
        assert_eq!(count(0), "0");
        assert_eq!(count(999), "999");
        assert_eq!(count(1000), "1,000");
        assert_eq!(count(9_127_598_034), "9,127,598,034");
    }
}
