use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write as _;

use anyhow::{Result, bail};
use jiff::Timestamp;
use serde::Serialize;

use crate::adapters::coverage::{self, Coverage, Status};
use crate::cli::ReportArgs;
use crate::commands::tokens::{self, ClientReport, ClientVersion};
use crate::data::{Data, Five, Label, Override};
use crate::estimate::{self, L1};
use crate::ledger::{FinalUsage, ProjectUsage, ProviderCalls, Split, Unrecorded, Usage};
use crate::output::{self, redact};
use crate::projects::{self, Resolution, ResolvedBy, Resolver, Selected};
use crate::quantity::{GramsCo2e, GramsCo2ePerKwh, TokenClasses, Tokens, WattHours};

/// The version of the report's JSON; it changes only when a field is
/// removed or changed.
const SCHEMA_VERSION: u32 = 2;

/// The activity label: every count comes exactly from the logs (primary).
const ACTIVITY_QUALITY: &str = "P";

/// The activity label of a figure that includes tokens the tool estimated
/// for calls whose logs do not record their final usage or any usage.
const ESTIMATED_ACTIVITY_QUALITY: &str = "E";

/// Printed by `tokendraw report --json`; see [`SCHEMA_VERSION`].
#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub tokendraw: Tokendraw,
    pub data_version: DataVersion,
    pub period: Period,
    /// The project the report is limited to; `null` for every call or for
    /// the unresolved calls of `directory`.
    pub project: Option<String>,
    /// The directory whose unresolved calls the report is limited to;
    /// `null` otherwise.
    pub directory: Option<String>,
    pub sources: Vec<Source>,
    pub providers: Vec<ProviderUsage>,
    pub observed_totals: Totals,
    pub estimates: Vec<Estimate>,
    /// Empty until every layer of an allowed sum is estimated.
    pub sums: Vec<Sum>,
    pub l1_groups: Vec<Group>,
    pub parameter_sets: Vec<RecordQuality>,
    pub grid: GridProvenance,
    pub unmatched: Unmatched,
    /// Every project's calls and emissions, largest first; left out for
    /// one project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub projects: Option<Vec<ProjectEmissions>>,
    /// The calls attributed to no project; left out for one project.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unresolved: Option<UnresolvedEmissions>,
    /// Where the base case L1 energy goes; left out without energy.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub breakdown: Option<Vec<Part>>,
}

/// A part of the base case L1 energy, with the signal it comes from.
#[derive(Debug, Serialize)]
#[serde(untagged)]
pub enum Part {
    Share {
        name: &'static str,
        share_of_base_case: f64,
        #[serde(skip_serializing_if = "Option::is_none")]
        threshold_cached_tokens: Option<u64>,
        signal: &'static str,
    },
    /// Every part present, keyed by its id; the shares add up to 1.
    Split {
        name: &'static str,
        shares_of_base_case: BTreeMap<String, f64>,
        signal: &'static str,
    },
}

#[derive(Debug, Serialize)]
pub struct Tokendraw {
    pub version: &'static str,
    /// As Rust names them, such as `macos` and `aarch64`.
    pub os: &'static str,
    pub arch: &'static str,
}

impl Tokendraw {
    #[must_use]
    pub fn current() -> Tokendraw {
        Tokendraw {
            version: env!("CARGO_PKG_VERSION"),
            os: std::env::consts::OS,
            arch: std::env::consts::ARCH,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct ProjectEmissions {
    pub project: String,
    pub calls: u64,
    pub resolved_by: ResolvedBy,
    pub emissions_g: Five<GramsCo2e>,
}

#[derive(Debug, Serialize)]
pub struct UnresolvedEmissions {
    pub calls: u64,
    /// The working directories of the calls.
    pub directories: u64,
    /// Of `calls`, those whose logs record no working directory.
    pub calls_without_directory: u64,
    pub emissions_g: Five<GramsCo2e>,
}

#[derive(Debug, Serialize)]
pub struct DataVersion {
    pub id: String,
    pub shipped_sha256: String,
    pub overrides: Vec<Override>,
}

/// From `start`, inclusive, to `end`, exclusive; `null` where unbounded.
#[derive(Debug, Serialize)]
pub struct Period {
    pub start: Option<Timestamp>,
    pub end: Option<Timestamp>,
}

/// Tokens in the energy model's three classes, with the finer classes
/// they are made of.
#[derive(Debug, Serialize)]
pub struct ReportTokens {
    pub cache_read: Tokens,
    /// Uncached input plus cache writes.
    pub new_input: Tokens,
    /// Including reasoning.
    pub output: Tokens,
    pub uncached_input: Tokens,
    pub cache_write: Tokens,
    /// `null` when no call records it.
    pub reasoning: Option<Tokens>,
}

impl From<&TokenClasses> for ReportTokens {
    fn from(tokens: &TokenClasses) -> ReportTokens {
        ReportTokens {
            cache_read: tokens.cache_read,
            new_input: Tokens(tokens.uncached_input.0 + tokens.cache_write.0),
            output: tokens.output,
            uncached_input: tokens.uncached_input,
            cache_write: tokens.cache_write,
            reasoning: tokens.reasoning,
        }
    }
}

#[derive(Debug, Serialize)]
pub struct Source {
    pub client: String,
    #[serde(flatten)]
    pub coverage: SourceCoverage,
    /// The figures are `null` for a client found but not read, whose calls
    /// are not in the ledger.
    pub calls: Option<u64>,
    pub sessions: Option<u64>,
    pub tokens: Option<ReportTokens>,
    pub activity_quality: Option<&'static str>,
    pub final_usage: Option<FinalUsage>,
    pub unrecorded: Option<Unrecorded>,
    pub earliest_call: Option<Timestamp>,
    pub latest_call: Option<Timestamp>,
    /// The version of the latest call first.
    pub client_versions: Option<Vec<ClientVersion>>,
    /// Over every file read so far, not only the period.
    pub unreadable_lines: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct SourceCoverage {
    pub status: Status,
    pub location: Option<String>,
    /// As [`Coverage::shareable_location`].
    #[serde(skip)]
    pub shareable_location: Option<String>,
    pub reason: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct ProviderUsage {
    pub provider: String,
    pub calls: u64,
    /// Sessions with a call to the provider; a session that calls two
    /// providers counts under both.
    pub sessions: u64,
    pub tokens: ReportTokens,
    pub activity_quality: &'static str,
}

#[derive(Debug, Serialize)]
pub struct Totals {
    pub calls: u64,
    /// The sum of each client's sessions.
    pub sessions: u64,
    pub tokens: ReportTokens,
    pub activity_quality: &'static str,
}

/// A quantity without data gets no estimate.
#[derive(Debug, Serialize)]
pub struct Estimate {
    pub layer: &'static str,
    pub quantity: &'static str,
    pub unit: &'static str,
    pub basis: String,
    pub activity_quality: &'static str,
    /// `None` without calls, when no factor applies.
    pub factor_quality: Option<Label>,
    #[serde(flatten)]
    pub values: Five<f64>,
}

/// Present only when every layer it adds is estimated.
#[derive(Debug, Serialize)]
pub struct Sum {
    pub name: &'static str,
    pub layers: &'static [&'static str],
    pub unit: &'static str,
    #[serde(flatten)]
    pub values: Five<f64>,
}

#[derive(Debug, Serialize)]
pub struct Group {
    pub provider: String,
    /// The id of the provider record that applied, possibly the fallback.
    pub record: String,
    pub calls: u64,
    pub energy_wh: Five<WattHours>,
    pub energy_facility_wh: Five<WattHours>,
    pub emissions_g: Five<GramsCo2e>,
    pub energy_quality: Label,
    pub emissions_quality: Label,
    pub grid_g_per_kwh: Five<GramsCo2ePerKwh>,
    /// The weakest label of the provider record and its geographies.
    pub grid_quality: Label,
    /// The label of the provider record: where it serves from.
    pub sites_quality: Label,
    pub pue: Five<f64>,
    pub pue_label: Label,
}

#[derive(Debug, Serialize)]
pub struct RecordQuality {
    pub id: String,
    pub quality: Label,
}

/// `basis` is `None` when the geographies do not share one.
#[derive(Debug, Serialize)]
pub struct GridProvenance {
    pub sources: Vec<String>,
    /// The labels of the geographies, best first.
    pub qualities: Vec<Label>,
    pub basis: Option<String>,
    pub earliest_year: Option<i16>,
    pub latest_year: Option<i16>,
}

#[derive(Debug, Serialize)]
pub struct Unmatched {
    /// Models no pattern matched; they use the fallback parameter set.
    pub models: Vec<Named>,
    /// Providers no record named; they use the unknown provider.
    pub providers: Vec<Named>,
}

#[derive(Debug, Serialize)]
pub struct Named {
    pub name: String,
    pub calls: u64,
}

/// Ingests new usage first, as the `tokens` command does.
pub fn run(args: &ReportArgs, data: &Data) -> Result<Report> {
    let (mut ledger, adapters) = tokens::ingest_all()?;
    let covered = adapters.iter().map(|entry| entry.client.as_str()).collect();
    let known = coverage::known(data, &covered);
    // One snapshot for the labels and recorded checkouts, the attribution
    // and every query, so what another process writes meanwhile cannot
    // enter the figures without the attribution that goes with it.
    ledger.begin_snapshot()?;
    let mut resolver = Resolver::new(&ledger)?;
    projects::attribute(&mut ledger, &mut resolver, args.since, args.until)?;
    let (project, directory) = match &args.project {
        None => (None, None),
        Some(dir) => {
            let selection = projects::select(&ledger, &mut resolver, &projects::argument(dir)?)?;
            match selection.selected {
                Selected::Project => (Some(selection.name), None),
                Selected::Unresolved => (None, Some(selection.name)),
                Selected::Nothing(elsewhere) => {
                    bail!(output::unselected(&selection.name, &elsewhere))
                }
            }
        }
    };
    let scope = project.as_deref().or(directory.as_deref());
    let counts = tokens::report(
        &ledger,
        adapters.iter().map(|entry| entry.client.as_str()),
        args.since,
        args.until,
        scope,
    )?;
    let providers = ledger.providers(args.since, args.until, scope)?;
    let thresholds = data.prompt_thresholds();
    let split = Split {
        prompt_above: &thresholds,
        cached_above: Some(data.long_context_tokens()),
    };
    let usage = ledger.usage(args.since, args.until, scope, &split)?;
    let by_project = match scope {
        Some(_) => None,
        None => Some(ledger.usage_by_project(
            args.since,
            args.until,
            &Split {
                prompt_above: &thresholds,
                cached_above: None,
            },
        )?),
    };
    ledger.end_snapshot()?;
    let redacted = args.redact.then(|| redact::data(data));
    let mut report = build(
        redacted.as_ref().unwrap_or(data),
        &counts.clients,
        &sources(&counts.clients, &adapters, &known),
        &providers,
        &usage,
        Period {
            start: args.since,
            end: args.until,
        },
        project,
        by_project.as_deref(),
    );
    report.directory = directory;
    if args.redact {
        redact::report(&mut report);
    }
    Ok(report)
}

/// Each client's coverage: its adapter's, else that of a client with calls
/// in the ledger and no adapter, then the known locations found for
/// clients with neither, by client.
#[must_use]
pub fn sources(
    clients: &[ClientReport],
    adapters: &[Coverage],
    known: &[Coverage],
) -> Vec<Coverage> {
    let mut sources: Vec<Coverage> = clients
        .iter()
        .map(|client| {
            let name = client.client.as_str();
            adapters
                .iter()
                .find(|entry| entry.client == name)
                .cloned()
                .unwrap_or_else(|| {
                    Coverage::no_adapter(name, known.iter().find(|entry| entry.client == name))
                })
        })
        .collect();
    sources.extend(
        known
            .iter()
            .filter(|entry| !clients.iter().any(|client| client.client == entry.client))
            .cloned(),
    );
    sources.sort_by(|a, b| a.client.cmp(&b.client));
    sources
}

/// `coverage` has an entry for every client, and for clients found but not
/// read.
#[must_use]
#[expect(
    clippy::too_many_arguments,
    reason = "each argument is one independent part of the report"
)]
pub fn build(
    data: &Data,
    clients: &[ClientReport],
    coverage: &[Coverage],
    providers: &[ProviderCalls],
    usage: &[Usage],
    period: Period,
    project: Option<String>,
    by_project: Option<&[ProjectUsage]>,
) -> Report {
    let l1 = estimate::l1(data, usage);
    let groups: Vec<Group> = l1
        .groups
        .iter()
        .map(|group| l1_group(data, group))
        .collect();
    let grid = provenance(data, &groups);
    let mut total = TokenClasses::default();
    for client in clients {
        total.uncached_input.0 += client.tokens.uncached_input.0;
        total.cache_write.0 += client.tokens.cache_write.0;
        total.cache_read.0 += client.tokens.cache_read.0;
        total.output.0 += client.tokens.output.0;
        total.reasoning = match (total.reasoning, client.tokens.reasoning) {
            (None, None) => None,
            (sum, more) => Some(Tokens(
                sum.unwrap_or_default().0 + more.unwrap_or_default().0,
            )),
        };
    }
    Report {
        schema_version: SCHEMA_VERSION,
        tokendraw: Tokendraw::current(),
        data_version: DataVersion {
            id: data.version.id.clone(),
            shipped_sha256: data.version.shipped_sha256.clone(),
            overrides: data.version.overrides.clone(),
        },
        period,
        project,
        directory: None,
        sources: coverage
            .iter()
            .map(|entry| {
                let client = clients.iter().find(|client| client.client == entry.client);
                Source {
                    client: entry.client.clone(),
                    coverage: SourceCoverage {
                        status: entry.status,
                        location: entry.location.clone(),
                        shareable_location: entry.shareable_location.clone(),
                        reason: entry.reason.clone(),
                    },
                    calls: client.map(|client| client.calls),
                    sessions: client.map(|client| client.sessions),
                    tokens: client.map(|client| ReportTokens::from(&client.tokens)),
                    activity_quality: client.map(|_| ACTIVITY_QUALITY),
                    final_usage: client.map(|client| client.final_usage),
                    unrecorded: client.map(|client| client.unrecorded),
                    earliest_call: client.and_then(|client| client.earliest_call),
                    latest_call: client.and_then(|client| client.latest_call),
                    client_versions: client.map(|client| client.client_versions.clone()),
                    unreadable_lines: client.map(|client| client.sources.unreadable_lines),
                }
            })
            .collect(),
        providers: providers
            .iter()
            .map(|provider| ProviderUsage {
                provider: provider.provider.clone(),
                calls: provider.calls,
                sessions: provider.sessions,
                tokens: ReportTokens::from(&provider.tokens),
                activity_quality: ACTIVITY_QUALITY,
            })
            .collect(),
        observed_totals: Totals {
            calls: clients.iter().map(|client| client.calls).sum(),
            sessions: clients.iter().map(|client| client.sessions).sum(),
            tokens: ReportTokens::from(&total),
            activity_quality: ACTIVITY_QUALITY,
        },
        estimates: estimates(&l1, &groups, &grid, activity_quality(usage)),
        sums: Vec::new(),
        l1_groups: groups,
        parameter_sets: l1
            .parameter_sets
            .iter()
            .filter_map(|id| data.set(id))
            .map(|set| RecordQuality {
                id: set.id.clone(),
                quality: estimate::parameter_label(set),
            })
            .collect(),
        grid,
        unmatched: Unmatched {
            models: named(&l1.unmatched_models),
            providers: named(&l1.unmatched_providers),
        },
        projects: by_project.map(|rows| projects(data, rows)),
        unresolved: by_project.map(|rows| unresolved(data, rows)),
        breakdown: breakdown(&l1, data.long_context_tokens()),
    }
}

/// `None` without base case energy, which no share can divide.
fn breakdown(l1: &L1, long_context: u64) -> Option<Vec<Part>> {
    let total = l1.energy.base_case.0;
    if total <= 0.0 {
        return None;
    }
    let share = |part: WattHours| part.0 / total;
    let shares = |parts: &BTreeMap<String, WattHours>| {
        parts
            .iter()
            .map(|(id, part)| (id.clone(), share(*part)))
            .collect()
    };
    let parts = &l1.breakdown;
    Some(vec![
        Part::Share {
            name: "context_terms",
            share_of_base_case: share(parts.context_terms),
            threshold_cached_tokens: None,
            signal: "model terms a_p, a_d, c0",
        },
        Part::Share {
            name: "calls_above_cached_tokens",
            share_of_base_case: share(parts.cached_above),
            threshold_cached_tokens: Some(long_context),
            signal: "cache_read tokens per call",
        },
        Part::Split {
            name: "client",
            shares_of_base_case: shares(&parts.by_client),
            signal: "client of each call",
        },
        Part::Split {
            name: "role",
            shares_of_base_case: shares(&parts.by_role),
            signal: "role of each call",
        },
    ])
}

fn is_unresolved(row: &ProjectUsage) -> bool {
    row.project.is_none() || row.resolution == Resolution::Unresolved.name()
}

fn projects(data: &Data, rows: &[ProjectUsage]) -> Vec<ProjectEmissions> {
    let mut by_project: BTreeMap<&str, (Vec<Usage>, ResolvedBy)> = BTreeMap::new();
    for row in rows.iter().filter(|row| !is_unresolved(row)) {
        let Some(project) = row.project.as_deref() else {
            continue;
        };
        let (usage, resolved_by) = by_project.entry(project).or_default();
        usage.push(row.usage.clone());
        if let Some(resolution) = Resolution::from_name(&row.resolution) {
            resolved_by.add(resolution, row.usage.calls);
        }
    }
    let mut projects: Vec<ProjectEmissions> = by_project
        .into_iter()
        .map(|(project, (usage, resolved_by))| ProjectEmissions {
            project: project.to_owned(),
            calls: usage.iter().map(|row| row.calls).sum(),
            resolved_by,
            emissions_g: estimate::l1(data, &usage).emissions,
        })
        .collect();
    projects.sort_by(|a, b| {
        b.emissions_g
            .base_case
            .0
            .total_cmp(&a.emissions_g.base_case.0)
            .then_with(|| a.project.cmp(&b.project))
    });
    projects
}

fn unresolved(data: &Data, rows: &[ProjectUsage]) -> UnresolvedEmissions {
    let rows: Vec<&ProjectUsage> = rows.iter().filter(|row| is_unresolved(row)).collect();
    let usage: Vec<Usage> = rows.iter().map(|row| row.usage.clone()).collect();
    let directories: BTreeSet<&str> = rows
        .iter()
        .filter_map(|row| row.project.as_deref())
        .collect();
    UnresolvedEmissions {
        calls: usage.iter().map(|row| row.calls).sum(),
        directories: directories.len() as u64,
        calls_without_directory: rows
            .iter()
            .filter(|row| row.project.is_none())
            .map(|row| row.usage.calls)
            .sum(),
        emissions_g: estimate::l1(data, &usage).emissions,
    }
}

fn l1_group(data: &Data, group: &estimate::Group) -> Group {
    let provider = &data.providers[&group.record];
    Group {
        provider: group.provider.clone(),
        record: group.record.clone(),
        calls: group.calls,
        energy_wh: group.energy,
        energy_facility_wh: group.facility,
        emissions_g: group.emissions,
        energy_quality: group.energy_quality,
        emissions_quality: group.emissions_quality,
        grid_g_per_kwh: provider.grid,
        grid_quality: group.grid_quality,
        sites_quality: provider.provider.label,
        pue: provider.provider.pue.five(),
        pue_label: provider.provider.pue.label,
    }
}

fn named(entries: &[(String, u64)]) -> Vec<Named> {
    entries
        .iter()
        .map(|(name, calls)| Named {
            name: name.clone(),
            calls: *calls,
        })
        .collect()
}

fn activity_quality(usage: &[Usage]) -> &'static str {
    if usage
        .iter()
        .any(|row| row.estimated_output.0 > 0 || row.estimated_input.0 > 0)
    {
        ESTIMATED_ACTIVITY_QUALITY
    } else {
        ACTIVITY_QUALITY
    }
}

fn estimates(
    l1: &L1,
    groups: &[Group],
    grid: &GridProvenance,
    activity_quality: &'static str,
) -> Vec<Estimate> {
    let per_group = |describe: &dyn Fn(&Group) -> String| {
        groups
            .iter()
            .map(|group| format!("{}: {}", group.provider, describe(group)))
            .collect::<Vec<_>>()
            .join("; ")
    };
    let pue = per_group(&|group| format!("{}, {}", group.record, slashed(&group.pue)));
    let intensities = per_group(&|group| {
        format!(
            "{} default, g/kWh {}",
            group.record,
            slashed(&group.grid_g_per_kwh.map(|value| value.0))
        )
    });
    let mut emissions_basis = "location-based, region unknown".to_owned();
    if !groups.is_empty() {
        let _ = write!(
            emissions_basis,
            "; {intensities}; grid: {}",
            grid.sources.join(", ")
        );
        if let Some(basis) = &grid.basis {
            let _ = write!(emissions_basis, ", {basis}");
        }
        if let (Some(earliest), Some(latest)) = (grid.earliest_year, grid.latest_year) {
            let _ = write!(emissions_basis, ", {}", years(earliest, latest));
        }
    }
    let mut energy_basis = "energy model per call".to_owned();
    if !l1.parameter_sets.is_empty() {
        let _ = write!(
            energy_basis,
            ", parameter sets {}",
            l1.parameter_sets.join(", ")
        );
    }
    energy_basis.push_str(
        ": node energy times idle times the provider's PUE; cache residency R not included",
    );
    let mut facility_basis = "IT energy times (PUE - 1)".to_owned();
    if !groups.is_empty() {
        let _ = write!(facility_basis, "; {pue}");
    }
    vec![
        Estimate {
            layer: "L1",
            quantity: "energy",
            unit: "Wh",
            basis: energy_basis,
            activity_quality,
            factor_quality: l1.energy_quality,
            values: l1.energy.map(|value| value.0),
        },
        Estimate {
            layer: "L1",
            quantity: "energy_facility",
            unit: "Wh",
            basis: facility_basis,
            activity_quality,
            factor_quality: l1.energy_quality,
            values: l1.facility.map(|value| value.0),
        },
        Estimate {
            layer: "L1",
            quantity: "emissions",
            unit: "g",
            basis: emissions_basis,
            activity_quality,
            factor_quality: l1.emissions_quality,
            values: l1.emissions.map(|value| value.0),
        },
    ]
}

#[must_use]
pub fn years(earliest: i16, latest: i16) -> String {
    if earliest == latest {
        format!("data year {earliest}")
    } else {
        format!("data years {earliest} to {latest}")
    }
}

/// Six decimal places, enough for every value the data states.
const DECIMALS: f64 = 1e6;

/// Five values as the data states them, computed values to six decimal
/// places, as `1.09/1.09/1.115/1.14/1.56`.
fn slashed(five: &Five<f64>) -> String {
    let shown = |value: f64| (value * DECIMALS).round() / DECIMALS;
    format!(
        "{}/{}/{}/{}/{}",
        shown(five.lower_bound),
        shown(five.low_case),
        shown(five.base_case),
        shown(five.high_case),
        shown(five.upper_bound)
    )
}

fn provenance(data: &Data, groups: &[Group]) -> GridProvenance {
    let providers: BTreeSet<&str> = groups.iter().map(|group| group.record.as_str()).collect();
    let geographies: Vec<_> = providers
        .iter()
        .filter_map(|id| data.providers.get(*id))
        .flat_map(|provider| provider.site_ids.iter().chain(&provider.bounds_ids))
        .collect::<BTreeSet<_>>()
        .into_iter()
        .filter_map(|id| data.geographies.get(id))
        .collect();
    let sources: BTreeSet<&str> = geographies
        .iter()
        .flat_map(|geography| &geography.sources)
        .map(|id| {
            data.sources
                .get(id)
                .map_or(id.as_str(), |source| source.title.as_str())
        })
        .collect();
    let bases: BTreeSet<&str> = geographies
        .iter()
        .map(|geography| geography.basis.as_str())
        .collect();
    let qualities: BTreeSet<Label> = geographies
        .iter()
        .map(|geography| geography.label)
        .collect();
    GridProvenance {
        sources: sources.into_iter().map(str::to_owned).collect(),
        qualities: qualities.into_iter().collect(),
        basis: match bases.len() {
            1 => bases.into_iter().next().map(str::to_owned),
            _ => None,
        },
        earliest_year: geographies.iter().map(|geography| geography.year).min(),
        latest_year: geographies.iter().map(|geography| geography.year).max(),
    }
}
