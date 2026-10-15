//! `tokendraw findings`: every finding that applies to the calls, whatever
//! its share of the energy.

use std::fmt::Write as _;

use anyhow::Result;
use jiff::Timestamp;
use jiff::civil::{Date, Time};
use jiff::tz::TimeZone;
use serde::Serialize;

use crate::cli::{FindingsArgs, ReportArgs};
use crate::commands::projects::quoted;
use crate::commands::report::{self, DataVersion, Period, Tokendraw};
use crate::config::Config;
use crate::data::Data;
use crate::findings::Finding;

/// The version of the JSON of `tokendraw findings`; it changes only when a
/// field is removed or changed.
const SCHEMA_VERSION: u32 = 1;

const NO_ENERGY: &str = "no energy was estimated for the calls, so no share can be computed";

#[derive(Debug, Serialize)]
pub struct FindingsReport {
    pub schema_version: u32,
    pub tokendraw: Tokendraw,
    pub data_version: DataVersion,
    pub period: Period,
    /// As in the report.
    pub project: Option<String>,
    pub directory: Option<String>,
    /// The report shows the findings at or above it.
    pub threshold_share_of_base_case: f64,
    /// Largest first; `null` without energy, with the `reason`.
    pub findings: Option<Vec<Finding>>,
    pub reason: Option<&'static str>,
}

/// Computes the report of the same calls, which ingests new usage first.
pub fn run(
    args: &FindingsArgs,
    data: &Data,
    config: Config,
    today: Date,
) -> Result<FindingsReport> {
    let report = report::run(
        &ReportArgs {
            since: args.since,
            until: args.until,
            verbose: false,
            project: args.project.clone(),
            json: false,
            redact: false,
        },
        data,
        config,
        today,
    )?;
    Ok(FindingsReport {
        schema_version: SCHEMA_VERSION,
        tokendraw: report.tokendraw,
        data_version: report.data_version,
        period: report.period,
        project: report.project,
        directory: report.directory,
        threshold_share_of_base_case: config.threshold_share(),
        reason: report.every_finding.is_none().then_some(NO_ENERGY),
        findings: report.every_finding,
    })
}

/// The command that shows the findings of the calls of a period and
/// project, as a user types it.
#[must_use]
pub fn command(
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    project: Option<&str>,
) -> String {
    let mut command = "tokendraw findings".to_owned();
    for (flag, time) in [("--since", since), ("--until", until)] {
        if let Some(time) = time {
            let _ = write!(command, " {flag} {}", shown(time));
        }
    }
    if let Some(project) = project {
        let _ = write!(command, " --project {}", quoted(project));
    }
    command
}

/// A time at midnight UTC as its date, which the flags read as that time.
fn shown(time: Timestamp) -> String {
    let zoned = time.to_zoned(TimeZone::UTC);
    if zoned.time() == Time::midnight() {
        zoned.date().to_string()
    } else {
        time.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_command_repeats_the_period_and_project() -> Result<(), jiff::Error> {
        let since: Timestamp = "2026-09-01T00:00:00Z".parse()?;
        let until: Timestamp = "2026-09-30T12:30:00Z".parse()?;
        assert_eq!(command(None, None, None), "tokendraw findings");
        assert_eq!(
            command(Some(since), Some(until), Some("~/src/my app")),
            "tokendraw findings --since 2026-09-01 --until 2026-09-30T12:30:00Z --project '~/src/my app'"
        );

        Ok(())
    }
}
