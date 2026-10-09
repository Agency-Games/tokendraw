use std::fmt::Write as _;
use std::io::{self, Write};

use jiff::Timestamp;
use jiff::civil::Time;
use jiff::tz::TimeZone;

use super::{count, shown_path};
use crate::adapters::coverage::Status;
use crate::commands::report::{self, Estimate, Group, Part, Period, Report, ReportTokens};
use crate::data::{Action, FALLBACK_PARAMETER_SET, Five, Label};
use crate::quantity::{GramsCo2ePerKwh, Tokens};

/// The width prose is wrapped to, inside 80 columns.
const WIDTH: usize = 76;

pub(super) const INDENT: &str = "  ";

const GAP: &str = "   ";

/// Smallest first, each with its size in the base unit: Wh for energy, g
/// for mass.
const ENERGY_UNITS: [(f64, &str); 4] = [(1.0, "Wh"), (1e3, "kWh"), (1e6, "MWh"), (1e9, "GWh")];
const MASS_UNITS: [(f64, &str); 3] = [(1.0, "g"), (1e3, "kg"), (1e6, "t")];

const GRAMS_PER_KILOGRAM: f64 = 1e3;

/// The columns of a band table: a name, the central value, the plausible
/// band and the envelope.
const BAND_ALIGNS: [Align; 4] = [Align::Left, Align::Right, Align::Left, Align::Left];

const SOURCE_NAMES: usize = 28;

pub fn write(out: &mut impl Write, report: &Report, verbose: bool) -> io::Result<()> {
    match (&report.project, &report.directory) {
        (Some(project), _) => writeln!(
            out,
            "tokendraw report, project {}, {}",
            shown_path(project),
            period(&report.period)
        )?,
        (None, Some(directory)) => writeln!(
            out,
            "tokendraw report, unresolved calls in {}, {}",
            shown_path(directory),
            period(&report.period)
        )?,
        (None, None) => writeln!(out, "tokendraw report, {}", period(&report.period))?,
    }
    writeln!(out)?;
    write_usage(out, report)?;
    write_final_usage(out, report)?;
    write_unrecorded(out, report)?;
    writeln!(out)?;
    write_coverage(out, report)?;
    writeln!(out)?;
    let mut tables = Vec::new();
    if verbose && !report.l1_groups.is_empty() {
        tables.extend([pue(report), electricity(report), grid(report)]);
    }
    tables.push(emissions(report));
    // One set of column widths, so the tables read as one calculation.
    let widths = widths(tables.iter().flatten());
    for (index, table) in tables.iter().enumerate() {
        if index > 0 {
            writeln!(out)?;
        }
        write_rows(out, &BAND_ALIGNS, &widths, table)?;
    }
    write_projects(out, report)?;
    write_breakdown(out, report)?;
    if verbose {
        writeln!(out)?;
        write_sources(out, report)
    } else {
        write_notes(out, report)
    }
}

fn write_projects(out: &mut impl Write, report: &Report) -> io::Result<()> {
    if report.directory.is_some() {
        writeln!(out)?;
        return write_wrapped(
            out,
            "These calls belong to no project. Run `tokendraw projects` to list them with the labels that would assign them.",
        );
    }
    let (Some(projects), Some(unresolved)) = (&report.projects, &report.unresolved) else {
        return Ok(());
    };
    if projects.is_empty() && unresolved.calls == 0 {
        return Ok(());
    }
    let total = find(report, "L1", "emissions");
    let (scale, unit) = unit(total.map(|total| total.values.central), &MASS_UNITS);
    let mut rows = vec![cells([
        &format!("Projects, {unit} CO2e"),
        "calls",
        "central",
        "plausible",
        "envelope",
    ])];
    let mut row = |name: String, calls: u64, five: &Five<f64>| {
        let mut row = vec![format!("{INDENT}{name}"), count(calls)];
        row.extend(band_cells(five, scale));
        rows.push(row);
    };
    for project in projects {
        row(
            shown_path(&project.project),
            project.calls,
            &project.emissions_g.map(|value| value.0),
        );
    }
    if unresolved.calls > 0 {
        row(
            "unresolved".to_owned(),
            unresolved.calls,
            &unresolved.emissions_g.map(|value| value.0),
        );
    }
    writeln!(out)?;
    let aligns = [
        Align::Left,
        Align::Right,
        Align::Right,
        Align::Left,
        Align::Left,
    ];
    write_named_table(out, &aligns, &rows)?;
    if unresolved.calls == 0 {
        return Ok(());
    }
    let mut what = Vec::new();
    let in_directories = unresolved.calls - unresolved.calls_without_directory;
    if in_directories > 0 {
        let directories = match unresolved.directories {
            1 => "1 directory".to_owned(),
            many => format!("{} directories", count(many)),
        };
        what.push(format!("{} in {directories}", calls(in_directories)));
    }
    if unresolved.calls_without_directory > 0 {
        what.push(format!(
            "{} without a working directory",
            calls(unresolved.calls_without_directory)
        ));
    }
    writeln!(out)?;
    write_wrapped(
        out,
        &format!(
            "{} {} to no project. Run `tokendraw projects` to list them with the labels that would assign them.",
            what.join(" and "),
            if unresolved.calls == 1 {
                "belongs"
            } else {
                "belong"
            }
        ),
    )
}

/// The roles in the order they are shown; others follow by id.
const ROLES: [&str; 3] = ["interactive", "subagent", "headless"];

/// A split shows only with two or more parts with energy; one part at 100%
/// says nothing. The signal of each share is in the JSON only, since the
/// names already say it.
fn write_breakdown(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let Some(parts) = &report.breakdown else {
        return Ok(());
    };
    let mut rows = vec![vec!["Where the energy goes (central)".to_owned()]];
    for part in parts {
        let (name, share) = match part {
            Part::Share {
                name: "context_terms",
                share_of_central,
                ..
            } => (
                "rereading the context".to_owned(),
                percent(*share_of_central),
            ),
            Part::Share {
                share_of_central,
                threshold_cached_tokens,
                ..
            } => (
                format!(
                    "calls above {} cached tokens",
                    count(threshold_cached_tokens.unwrap_or_default())
                ),
                percent(*share_of_central),
            ),
            Part::Split {
                name,
                shares_of_central,
                ..
            } => {
                let mut shown: Vec<(&String, f64)> = shares_of_central
                    .iter()
                    .filter(|(_, share)| **share > 0.0)
                    .map(|(id, share)| (id, *share))
                    .collect();
                if shown.len() < 2 {
                    continue;
                }
                if *name == "role" {
                    shown.sort_by_key(|(id, _)| {
                        ROLES
                            .iter()
                            .position(|role| role == id)
                            .unwrap_or(ROLES.len())
                    });
                }
                let ids: Vec<&str> = shown.iter().map(|(id, _)| id.as_str()).collect();
                let shares: Vec<String> = shown.iter().map(|(_, share)| percent(*share)).collect();
                (ids.join(" / "), shares.join(" / "))
            }
        };
        rows.push(vec![format!("{INDENT}{name}"), share]);
    }
    writeln!(out)?;
    write_named_table(out, &[Align::Left, Align::Left], &rows)
}

/// Whole percent; a share above 0 that rounds to 0 is `<1%`.
fn percent(share: f64) -> String {
    let rounded = (share * 100.0).round();
    if rounded == 0.0 && share > 0.0 {
        "<1%".to_owned()
    } else {
        format!("{rounded:.0}%")
    }
}

/// The location is the last column and never wrapped, so it stays whole.
fn write_coverage(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let order = [
        (Status::Read, "read"),
        (Status::PartlyRead, "read in part"),
        (Status::NotFound, "not found"),
        (Status::FoundNotRead, "found, not read"),
        (Status::NoAdapter, "no adapter"),
    ];
    let mut rows = vec![cells(["Coverage", "status", "location"])];
    for (status, name) in order {
        for source in report
            .sources
            .iter()
            .filter(|source| source.coverage.status == status)
        {
            let mut details = Vec::new();
            match status {
                Status::NoAdapter => details.push("calls read earlier stay counted".to_owned()),
                _ => details.extend(source.coverage.reason.clone()),
            }
            if let Some(lines) = source.unreadable_lines.filter(|lines| *lines > 0) {
                let noun = if lines == 1 { "line" } else { "lines" };
                details.push(format!("{} unreadable {noun}", count(lines)));
            }
            let location = match (&source.coverage.location, status) {
                (Some(location), _) => location.clone(),
                (None, Status::NoAdapter) => String::new(),
                (None, _) => "command".to_owned(),
            };
            let location = match (location.is_empty(), details.is_empty()) {
                (_, true) => location,
                (true, false) => details.join(", "),
                (false, false) => format!("{location} ({})", details.join(", ")),
            };
            rows.push(vec![
                format!("{INDENT}{}", source.client),
                name.to_owned(),
                location,
            ]);
        }
    }
    write_named_table(out, &[Align::Left; 3], &rows)
}

fn write_final_usage(out: &mut impl Write, report: &Report) -> io::Result<()> {
    for source in &report.sources {
        let Some(usage) = &source.final_usage else {
            continue;
        };
        let mut text = String::new();
        let _ = write!(text, "{}:", source.client);
        if usage.missing > 0 {
            let estimated = usage.missing - usage.missing_without_estimate;
            let _ = write!(
                text,
                " {} without a line with the final usage.",
                calls(usage.missing)
            );
            if estimated > 0 {
                let _ = write!(
                    text,
                    " Energy and emissions add an estimated {} output tokens for {}, which the usage table leaves out.",
                    scaled_tokens(usage.estimated_missing_output),
                    calls(estimated)
                );
            }
            if usage.missing_without_estimate > 0 {
                let _ = write!(
                    text,
                    " Too few comparable calls for an estimate: {}, counted as logged.",
                    calls(usage.missing_without_estimate)
                );
            }
        }
        if usage.unknown > 0 {
            let _ = write!(
                text,
                " Read before tokendraw recorded final usage, with logs gone: {}, counted as logged.",
                calls(usage.unknown)
            );
        }
        if usage.missing == 0 && usage.unknown == 0 {
            continue;
        }
        writeln!(out)?;
        write_wrapped(out, &text)?;
    }
    Ok(())
}

fn write_unrecorded(out: &mut impl Write, report: &Report) -> io::Result<()> {
    for source in &report.sources {
        let Some(unrecorded) = &source.unrecorded else {
            continue;
        };
        if unrecorded.calls == 0 {
            continue;
        }
        let mut text = format!(
            "{}: {} without a usage record.",
            source.client,
            compactions(unrecorded.calls)
        );
        let with_input = unrecorded.calls - unrecorded.without_input_estimate;
        let with_output = unrecorded.calls - unrecorded.without_output_estimate;
        let input = scaled_tokens(Tokens(
            unrecorded.estimated_uncached_input.0 + unrecorded.estimated_cache_read.0,
        ));
        let output = scaled_tokens(unrecorded.estimated_output);
        let mut added = Vec::new();
        if with_input > 0 && with_input == with_output {
            added.push(format!(
                "{input} input and {output} output tokens for {}",
                compactions(with_input)
            ));
        } else {
            if with_input > 0 {
                added.push(format!(
                    "{input} input tokens for {}",
                    compactions(with_input)
                ));
            }
            if with_output > 0 {
                added.push(format!(
                    "{output} output tokens for {}",
                    compactions(with_output)
                ));
            }
        }
        if !added.is_empty() {
            let _ = write!(
                text,
                " Energy and emissions add an estimated {}, which the usage table leaves out.",
                added.join(" and ")
            );
        }
        if unrecorded.without_input_estimate > 0 {
            let _ = write!(
                text,
                " Without an earlier call or enough comparable compactions for an input estimate: {}.",
                compactions(unrecorded.without_input_estimate)
            );
        }
        if unrecorded.without_output_estimate > 0 {
            let _ = write!(
                text,
                " Too few comparable compactions for an output estimate: {}.",
                compactions(unrecorded.without_output_estimate)
            );
        }
        writeln!(out)?;
        write_wrapped(out, &text)?;
    }
    Ok(())
}

fn write_notes(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let unmatched = &report.unmatched;
    let names: Vec<String> = unmatched
        .models
        .iter()
        .map(|model| format!("model {}", model.name))
        .chain(
            unmatched
                .providers
                .iter()
                .map(|provider| format!("provider {}", provider.name)),
        )
        .collect();
    let mut notes = Vec::new();
    if !names.is_empty() {
        notes.push(format!(
            "No data record for {}; a data override can add one, and `--verbose` shows what was used instead.",
            names.join(", ")
        ));
    }
    let overrides: Vec<&str> = report
        .data_version
        .overrides
        .iter()
        .map(|file| file.file.path.as_str())
        .collect();
    if !overrides.is_empty() {
        notes.push(format!(
            "Data overrides in effect: {}.",
            overrides.join(", ")
        ));
    }
    if notes.is_empty() {
        return Ok(());
    }
    writeln!(out)?;
    for note in notes {
        write_wrapped(out, &note)?;
    }
    Ok(())
}

/// Writes `text` in lines of at most [`WIDTH`] columns. A command in
/// backticks is kept on one line, and a word or command longer than a line
/// stays whole.
pub(super) fn write_wrapped(out: &mut impl Write, text: &str) -> io::Result<()> {
    for line in wrapped(text, "") {
        writeln!(out, "{line}")?;
    }
    Ok(())
}

pub(super) fn wrapped(text: &str, indent: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut line = indent.to_owned();
    for word in unbroken(text) {
        let empty = line.len() == indent.len();
        if !empty && line.len() + 1 + word.len() > WIDTH {
            lines.push(std::mem::replace(&mut line, indent.to_owned()));
        } else if !empty {
            line.push(' ');
        }
        line.push_str(&word);
    }
    if line.len() > indent.len() {
        lines.push(line);
    }
    lines
}

fn unbroken(text: &str) -> Vec<String> {
    let mut words: Vec<String> = Vec::new();
    let mut open = false;
    for word in text.split(' ') {
        match words.last_mut() {
            Some(last) if open => {
                last.push(' ');
                last.push_str(word);
            }
            _ => words.push(word.to_owned()),
        }
        if word.matches('`').count() % 2 == 1 {
            open = !open;
        }
    }
    words
}

pub(super) fn period(period: &Period) -> String {
    let day = |time: Timestamp| {
        let zoned = time.to_zoned(TimeZone::UTC);
        (zoned.time() == Time::midnight()).then(|| zoned.date())
    };
    let shown =
        |time: Timestamp| day(time).map_or_else(|| time.to_string(), |day| format!("{day} UTC"));
    match (period.start, period.end) {
        (None, None) => "all calls".to_owned(),
        (Some(start), None) => format!("from {}", shown(start)),
        (None, Some(end)) => format!("before {}", shown(end)),
        (Some(start), Some(end)) => {
            match (day(start), day(end).and_then(|day| day.yesterday().ok())) {
                (Some(first), Some(last)) if first == last => format!("{first} UTC"),
                (Some(first), Some(last)) if first < last => format!("{first} to {last} UTC"),
                _ => format!("from {start} to before {end}"),
            }
        }
    }
}

#[derive(Clone, Copy)]
pub(super) enum Align {
    Left,
    Right,
}

pub(super) fn write_table(
    out: &mut impl Write,
    indent: &str,
    aligns: &[Align],
    rows: &[Vec<String>],
) -> io::Result<()> {
    let widths = widths(rows.iter());
    let rows: Vec<Vec<String>> = rows
        .iter()
        .map(|row| {
            let mut row = row.clone();
            if let Some(first) = row.first_mut() {
                first.insert_str(0, indent);
            }
            row
        })
        .collect();
    let mut widths = widths;
    if let Some(first) = widths.first_mut() {
        *first += indent.len();
    }
    write_rows(out, aligns, &widths, &rows)
}

/// The longest name a table row keeps beside its figures; a longer one
/// goes on a line of its own above them, so one long name does not push
/// every row's figures to the right.
const NAME_WIDTH: usize = 40;

/// A row of one cell is a line of its own and sets no width.
pub(super) fn write_named_table(
    out: &mut impl Write,
    aligns: &[Align],
    rows: &[Vec<String>],
) -> io::Result<()> {
    let mut lines = Vec::new();
    for row in rows {
        match row.split_first() {
            Some((name, figures)) if !figures.is_empty() && name.chars().count() > NAME_WIDTH => {
                lines.push(vec![name.clone()]);
                lines.push(
                    std::iter::once(String::new())
                        .chain(figures.iter().cloned())
                        .collect(),
                );
            }
            _ => lines.push(row.clone()),
        }
    }
    let widths = widths(lines.iter().filter(|row| row.len() > 1));
    write_rows(out, aligns, &widths, &lines)
}

pub(super) fn widths<'a>(rows: impl Iterator<Item = &'a Vec<String>>) -> Vec<usize> {
    let mut widths = Vec::new();
    for row in rows {
        for (index, cell) in row.iter().enumerate() {
            if widths.len() <= index {
                widths.push(0);
            }
            widths[index] = widths[index].max(cell.chars().count());
        }
    }
    widths
}

pub(super) fn write_rows(
    out: &mut impl Write,
    aligns: &[Align],
    widths: &[usize],
    rows: &[Vec<String>],
) -> io::Result<()> {
    for row in rows {
        let mut line = String::new();
        for (index, ((cell, width), align)) in row.iter().zip(widths).zip(aligns).enumerate() {
            if index > 0 {
                line.push_str(GAP);
            }
            let _ = match align {
                Align::Left => write!(line, "{cell:<width$}"),
                Align::Right => write!(line, "{cell:>width$}"),
            };
        }
        writeln!(out, "{}", line.trim_end())?;
    }
    Ok(())
}

pub(super) fn cells<const N: usize>(cells: [&str; N]) -> Vec<String> {
    cells.iter().map(|cell| (*cell).to_owned()).collect()
}

fn write_usage(out: &mut impl Write, report: &Report) -> io::Result<()> {
    let row = |name: String, calls: u64, sessions: u64, tokens: &ReportTokens| {
        vec![
            name,
            count(calls),
            count(sessions),
            scaled_tokens(tokens.cache_read),
            scaled_tokens(tokens.new_input),
            scaled_tokens(tokens.output),
        ]
    };
    let mut rows = vec![cells([
        "Usage",
        "calls",
        "sessions",
        "cache read",
        "new input",
        "output",
    ])];
    rows.extend(report.providers.iter().map(|provider| {
        row(
            format!("{INDENT}{}", provider.provider),
            provider.calls,
            provider.sessions,
            &provider.tokens,
        )
    }));
    let total = &report.observed_totals;
    rows.push(Vec::new());
    rows.push(row(
        format!("{INDENT}total"),
        total.calls,
        total.sessions,
        &total.tokens,
    ));
    let aligns = [
        Align::Left,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
        Align::Right,
    ];
    write_table(out, "", &aligns, &rows)
}

fn find<'a>(report: &'a Report, layer: &str, quantity: &str) -> Option<&'a Estimate> {
    report
        .estimates
        .iter()
        .find(|estimate| estimate.layer == layer && estimate.quantity == quantity)
}

fn group_row(group: &Group, five: &Five<f64>, scale: f64) -> Vec<String> {
    let mut row = band_cells(five, scale);
    row.insert(0, provider_name(group));
    row
}

fn total_row(estimate: Option<&Estimate>, name: &str, scale: f64) -> Option<Vec<String>> {
    let mut row = band_cells(&estimate?.values, scale);
    row.insert(0, format!("{INDENT}{name}"));
    Some(row)
}

fn band_heading(name: &str) -> Vec<String> {
    cells([name, "central", "plausible", "envelope"])
}

fn provider_name(group: &Group) -> String {
    if group.provider == group.record {
        format!("{INDENT}{}", group.provider)
    } else {
        format!("{INDENT}{} ({})", group.provider, group.record)
    }
}

fn pue(report: &Report) -> Vec<Vec<String>> {
    let shown = |low: f64, high: f64| range(&low.to_string(), &high.to_string());
    let mut rows = vec![band_heading("Power usage effectiveness (PUE)")];
    for group in &report.l1_groups {
        let pue = &group.pue;
        rows.push(vec![
            provider_name(group),
            pue.central.to_string(),
            shown(pue.plausible_low, pue.plausible_high),
            shown(pue.envelope_low, pue.envelope_high),
        ]);
    }
    rows
}

fn electricity(report: &Report) -> Vec<Vec<String>> {
    let total = find(report, "L1", "energy");
    let (scale, unit) = unit(total.map(|total| total.values.central), &ENERGY_UNITS);
    let mut rows = vec![band_heading(&format!("Electricity, {unit}"))];
    rows.extend(
        report
            .l1_groups
            .iter()
            .map(|group| group_row(group, &group.energy_wh.map(|value| value.0), scale)),
    );
    rows.push(Vec::new());
    rows.extend(total_row(total, "total", scale));
    rows.extend(total_row(
        find(report, "L1", "energy_facility"),
        "of which data centre overhead",
        scale,
    ));
    rows
}

fn grid(report: &Report) -> Vec<Vec<String>> {
    let kg = |value: GramsCo2ePerKwh, digits| significant(value.0 / GRAMS_PER_KILOGRAM, digits);
    let bound = |value| kg(value, 2);
    let mut rows = vec![band_heading("Grid intensity, kg CO2e per kWh")];
    for group in &report.l1_groups {
        let grid = &group.grid_g_per_kwh;
        rows.push(vec![
            provider_name(group),
            kg(grid.central, 3),
            range(&bound(grid.plausible_low), &bound(grid.plausible_high)),
            range(&bound(grid.envelope_low), &bound(grid.envelope_high)),
        ]);
    }
    rows
}

fn emissions(report: &Report) -> Vec<Vec<String>> {
    let total = find(report, "L1", "emissions");
    let (scale, unit) = unit(total.map(|total| total.values.central), &MASS_UNITS);
    let mut rows = vec![band_heading(&format!("Emissions, {unit} CO2e"))];
    rows.extend(
        report
            .l1_groups
            .iter()
            .map(|group| group_row(group, &group.emissions_g.map(|value| value.0), scale)),
    );
    rows.push(Vec::new());
    rows.extend(total_row(total, "total", scale));
    rows
}

fn quality(label: Label) -> &'static str {
    match label {
        Label::M => "disclosed by the provider",
        Label::C => "published by a company",
        Label::I => "independent research",
        Label::D => "tool default",
        Label::S => "spend-based",
    }
}

/// Names grouped by their label, best first, as `a, b: label; c: label`.
fn by_quality(records: impl Iterator<Item = (String, Label)>) -> String {
    let mut groups: Vec<(Label, Vec<String>)> = Vec::new();
    for (name, label) in records {
        match groups.iter_mut().find(|(other, _)| *other == label) {
            Some((_, names)) => {
                if !names.contains(&name) {
                    names.push(name);
                }
            }
            None => groups.push((label, vec![name])),
        }
    }
    groups.sort_by_key(|(label, _)| *label);
    for (_, names) in &mut groups {
        names.sort();
    }
    groups
        .iter()
        .map(|(label, names)| format!("{}: {}", names.join(", "), quality(*label)))
        .collect::<Vec<_>>()
        .join("; ")
}

fn write_sources(out: &mut impl Write, report: &Report) -> io::Result<()> {
    writeln!(out, "Data sources")?;
    write_source(out, "data version", &data_version(report))?;
    if report.l1_groups.is_empty() {
        return Ok(());
    }
    let sets = by_quality(
        report
            .parameter_sets
            .iter()
            .map(|set| (set.id.clone(), set.quality)),
    );
    let noun = if report.parameter_sets.len() == 1 {
        "parameter set"
    } else {
        "parameter sets"
    };
    write_source(out, "energy per call", &format!("{noun} {sets}"))?;
    let pue = by_quality(report.l1_groups.iter().map(|group| {
        (
            provider_name(group).trim_start().to_owned(),
            group.pue_label,
        )
    }));
    write_source(out, "power usage effectiveness", &pue)?;
    let sites = by_quality(report.l1_groups.iter().map(|group| {
        (
            provider_name(group).trim_start().to_owned(),
            group.sites_quality,
        )
    }));
    write_source(out, "provider sites", &sites)?;
    if let Some(grid) = grid_sources(report) {
        write_source(out, "grid intensity", &grid)?;
    }
    if let Some(models) = unmatched_models(report) {
        write_source(out, "models without data", &models)?;
    }
    Ok(())
}

fn write_source(out: &mut impl Write, name: &str, text: &str) -> io::Result<()> {
    let mut lines = Vec::new();
    let mut line = String::new();
    for word in text.split(' ') {
        if !line.is_empty() && INDENT.len() + SOURCE_NAMES + line.len() + 1 + word.len() > WIDTH {
            lines.push(std::mem::take(&mut line));
        }
        if !line.is_empty() {
            line.push(' ');
        }
        line.push_str(word);
    }
    lines.push(line);
    for (index, line) in lines.iter().enumerate() {
        let name = if index == 0 { name } else { "" };
        writeln!(out, "{INDENT}{name:<SOURCE_NAMES$}{line}")?;
    }
    Ok(())
}

fn grid_sources(report: &Report) -> Option<String> {
    let grid = &report.grid;
    if grid.sources.is_empty() {
        return None;
    }
    let mut line = grid.sources.join("; ");
    if let Some(basis) = &grid.basis {
        let _ = write!(line, "; {basis}");
    }
    if let (Some(earliest), Some(latest)) = (grid.earliest_year, grid.latest_year) {
        let _ = write!(line, "; {}", report::years(earliest, latest));
    }
    let qualities: Vec<&str> = grid.qualities.iter().map(|label| quality(*label)).collect();
    if !qualities.is_empty() {
        let _ = write!(line, ": {}", qualities.join(", "));
    }
    Some(line)
}

fn data_version(report: &Report) -> String {
    let version = &report.data_version;
    let shipped = &version.shipped_sha256[..12];
    if version.overrides.is_empty() {
        return format!("{}, shipped sha256 {shipped}, no overrides", version.id);
    }
    let files: Vec<String> = version
        .overrides
        .iter()
        .map(|file| {
            let records: Vec<String> = file
                .records
                .iter()
                .map(|record| {
                    let action = match record.action {
                        Action::Replaced => "replaces",
                        Action::Added => "adds",
                    };
                    format!("{action} {} {}", record.kind, record.id)
                })
                .collect();
            format!("{} ({})", file.file.path, records.join(", "))
        })
        .collect();
    format!(
        "{}, shipped sha256 {shipped}, overrides: {}",
        version.id,
        files.join("; ")
    )
}

fn unmatched_models(report: &Report) -> Option<String> {
    let models: Vec<String> = report
        .unmatched
        .models
        .iter()
        .map(|model| format!("{} ({})", model.name, calls(model.calls)))
        .collect();
    (!models.is_empty()).then(|| {
        format!(
            "{}, so the {FALLBACK_PARAMETER_SET} parameter set",
            models.join(", ")
        )
    })
}

fn band_cells(five: &Five<f64>, scale: f64) -> Vec<String> {
    let shown = |value: f64, digits| significant(value / scale, digits);
    vec![
        shown(five.central, 3),
        range(
            &shown(five.plausible_low, 2),
            &shown(five.plausible_high, 2),
        ),
        range(&shown(five.envelope_low, 2), &shown(five.envelope_high, 2)),
    ]
}

fn range(low: &str, high: &str) -> String {
    if low == high {
        low.to_owned()
    } else {
        format!("{low} to {high}")
    }
}

pub(super) fn compactions(count: u64) -> String {
    if count == 1 {
        "1 compaction".to_owned()
    } else {
        format!("{} compactions", super::count(count))
    }
}

pub(super) fn calls(count: u64) -> String {
    if count == 1 {
        "1 call".to_owned()
    } else {
        format!("{} calls", super::count(count))
    }
}

fn unit(value: Option<f64>, units: &[(f64, &'static str)]) -> (f64, &'static str) {
    let value = value.unwrap_or(0.0).abs();
    units
        .iter()
        .rev()
        .find(|(size, _)| value >= *size)
        .or(units.first())
        .copied()
        .unwrap_or((1.0, ""))
}

pub(super) fn significant(value: f64, digits: i32) -> String {
    if value == 0.0 || !value.is_finite() {
        return "0".to_owned();
    }
    let exponent = |value: f64| {
        #[expect(
            clippy::cast_possible_truncation,
            reason = "the decimal exponent of a finite f64 fits in an i32"
        )]
        let exponent = value.abs().log10().floor() as i32;
        exponent
    };
    let step = 10_f64.powi(exponent(value) - digits + 1);
    // Rounding up can add a digit, as 9.96 to 10 at two figures.
    let rounded = (value / step).round() * step;
    let decimals = usize::try_from(digits - 1 - exponent(rounded)).unwrap_or(0);
    let text = format!("{:.decimals$}", rounded.abs());
    let (whole, fraction) = text.split_once('.').unwrap_or((&text, ""));
    let sign = if rounded < 0.0 { "-" } else { "" };
    let grouped = whole
        .parse::<u64>()
        .map_or_else(|_| whole.to_owned(), count);
    if fraction.is_empty() {
        format!("{sign}{grouped}")
    } else {
        format!("{sign}{grouped}.{fraction}")
    }
}

fn scaled_tokens(tokens: Tokens) -> String {
    const SCALES: [(f64, &str); 3] = [(1e9, "B"), (1e6, "M"), (1e3, "k")];
    #[expect(
        clippy::cast_precision_loss,
        reason = "three significant figures are shown"
    )]
    let value = tokens.0 as f64;
    match SCALES.iter().find(|(size, _)| value >= *size) {
        Some((size, suffix)) => format!("{} {suffix}", significant(value / size, 3)),
        None => count(tokens.0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_command_in_backticks_is_never_broken_across_lines() -> io::Result<()> {
        let mut out = Vec::new();
        let text = format!(
            "{} calls belong to no project. Run `tokendraw projects --json` to list them.",
            "x".repeat(40)
        );
        write_wrapped(&mut out, &text)?;
        assert_eq!(
            String::from_utf8_lossy(&out),
            format!(
                "{} calls belong to no project. Run\n`tokendraw projects --json` to list them.\n",
                "x".repeat(40)
            )
        );
        Ok(())
    }

    #[test]
    fn central_values_have_three_figures_and_bounds_two() {
        assert_eq!(significant(239.406, 3), "239");
        assert_eq!(significant(28.883, 3), "28.9");
        assert_eq!(significant(113.887, 2), "110");
        assert_eq!(significant(2_045.137, 2), "2,000");
        assert_eq!(significant(2.516, 2), "2.5");
        assert_eq!(significant(0.846, 2), "0.85");
        assert_eq!(significant(1_416.933, 2), "1,400");
        assert_eq!(significant(9.96, 2), "10");
        assert_eq!(significant(0.0, 3), "0");
    }

    #[test]
    fn units_scale_so_the_central_value_is_at_least_one() {
        assert_eq!(unit(Some(239_406.0), &ENERGY_UNITS).1, "kWh");
        assert_eq!(unit(Some(999.0), &ENERGY_UNITS).1, "Wh");
        assert_eq!(unit(Some(0.5), &ENERGY_UNITS).1, "Wh");
        assert_eq!(unit(Some(93_508.0), &MASS_UNITS).1, "kg");
        assert_eq!(unit(Some(2.0e6), &MASS_UNITS).1, "t");
    }

    #[test]
    fn token_counts_are_scaled_to_three_figures() {
        assert_eq!(scaled_tokens(Tokens(9_129_322_826)), "9.13 B");
        assert_eq!(scaled_tokens(Tokens(169_380_824)), "169 M");
        assert_eq!(scaled_tokens(Tokens(24_821_250)), "24.8 M");
        assert_eq!(scaled_tokens(Tokens(1_500)), "1.50 k");
        assert_eq!(scaled_tokens(Tokens(999)), "999");
    }

    #[test]
    fn whole_days_show_as_first_and_last_day() -> Result<(), jiff::Error> {
        let period = |start: &str, end: &str| -> Result<String, jiff::Error> {
            Ok(super::period(&Period {
                start: Some(start.parse()?),
                end: Some(end.parse()?),
            }))
        };
        assert_eq!(
            period("2026-09-01T00:00:00Z", "2026-10-01T00:00:00Z")?,
            "2026-09-01 to 2026-09-30 UTC"
        );
        assert_eq!(
            period("2026-09-01T00:00:00Z", "2026-09-02T00:00:00Z")?,
            "2026-09-01 UTC"
        );
        assert_eq!(
            period("2026-09-01T00:00:00Z", "2026-09-01T12:00:00Z")?,
            "from 2026-09-01T00:00:00Z to before 2026-09-01T12:00:00Z"
        );
        Ok(())
    }
}
