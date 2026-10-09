//! The `tokens` command: ingest, then count tokens per client.

use anyhow::{Context, Result};
use jiff::Timestamp;
use serde::Serialize;

use crate::adapters::Adapter;
use crate::adapters::claude_code::ClaudeCode;
use crate::adapters::codex::Codex;
use crate::adapters::coverage::Coverage;
use crate::adapters::pi::Pi;
use crate::adapters::user::external::{self, External};
use crate::adapters::user::{self, UserAdapter};
use crate::adapters::walk::Walk;
use crate::cli::TokensArgs;
use crate::dirs;
use crate::ledger::{ClientCalls, FinalUsage, Ledger, Sources, Unrecorded, VersionCalls};
use crate::projects;
use crate::quantity::TokenClasses;

/// The clients of the shipped adapters, which user adapters cannot take.
pub const SHIPPED: [&str; 3] = ["claude-code", "codex", "pi"];

/// Changes only when a field of the JSON is removed or changed.
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct Report {
    pub schema_version: u32,
    pub since: Option<Timestamp>,
    pub until: Option<Timestamp>,
    pub clients: Vec<ClientReport>,
}

#[derive(Debug, Serialize)]
pub struct ClientReport {
    pub client: String,
    pub calls: u64,
    pub sessions: u64,
    pub tokens: TokenClasses,
    pub calls_with_reasoning: u64,
    pub final_usage: FinalUsage,
    pub unrecorded: Unrecorded,
    pub earliest_call: Option<Timestamp>,
    pub latest_call: Option<Timestamp>,
    /// The version of the latest call first.
    pub client_versions: Vec<ClientVersion>,
    pub sources: SourceReport,
}

#[derive(Clone, Debug, Serialize)]
pub struct ClientVersion {
    /// `null` for the calls whose logs record no version.
    pub version: Option<String>,
    pub calls: u64,
}

/// Over all time, not only the range.
#[derive(Debug, Default, Serialize)]
pub struct SourceReport {
    pub files: u64,
    pub unreadable_lines: u64,
    pub files_without_usage: u64,
}

pub fn run(args: &TokensArgs) -> Result<Report> {
    let (ledger, coverage) = ingest_all()?;
    report(
        &ledger,
        coverage.iter().map(|entry| entry.client.as_str()),
        args.since,
        args.until,
        None,
    )
}

/// Also records the checkout of every working directory that exists now.
pub fn ingest_all() -> Result<(Ledger, Vec<Coverage>)> {
    let mut ledger = open_ledger()?;
    let claude_code = ClaudeCode::from_env().context(
        "cannot find Claude Code's logs: no home directory is known; set HOME or CLAUDE_CONFIG_DIR",
    )?;
    let codex = Codex::from_env()
        .context("cannot find Codex's logs: no home directory is known; set HOME or CODEX_HOME")?;
    let pi = Pi::from_env().context(
        "cannot find pi's logs: no home directory is known; set HOME or PI_CODING_AGENT_DIR",
    )?;
    let user = match dirs::config() {
        Some(config) => user::load_dir(&config.join("adapters"), &SHIPPED)?,
        None => Vec::new(),
    };
    let walked: Vec<&dyn Adapter> = [&claude_code as &dyn Adapter, &codex, &pi]
        .into_iter()
        .chain(user.iter().filter_map(|adapter| match adapter {
            UserAdapter::Declared(declared) => Some(declared as &dyn Adapter),
            UserAdapter::External(_) => None,
        }))
        .collect();
    let walks: Vec<Walk> = walked.iter().map(|adapter| adapter.discover()).collect();
    if !ledger.has_read_files()? && walks.iter().any(|walk| !walk.files.is_empty()) {
        eprintln!(
            "tokendraw: reading the agents' logs for the first time, which can take a minute; later runs read only what changed"
        );
    }
    let mut coverage = Vec::new();
    for (adapter, walk) in walked.into_iter().zip(walks) {
        let walk = ledger.ingest_walk(adapter, walk)?;
        coverage.push(Coverage::of_walk(
            adapter.client(),
            adapter.location(),
            adapter.template(),
            &walk,
        ));
    }
    for adapter in &user {
        if let UserAdapter::External(external) = adapter {
            ingest_external(&mut ledger, external)?;
            coverage.push(Coverage::command(external.client()));
        }
    }
    projects::record(&mut ledger, Timestamp::now())?;
    Ok((ledger, coverage))
}

pub fn open_ledger() -> Result<Ledger> {
    let dir = dirs::state()
        .context("cannot find a directory for the ledger: no home directory is known; set HOME")?;
    Ok(Ledger::open(&dir.join("ledger.sqlite"))?)
}

/// Fails when the adapter's definition changed since its calls were read.
pub fn ingest(ledger: &mut Ledger, adapter: &UserAdapter) -> Result<Coverage> {
    match adapter {
        UserAdapter::Declared(declared) => {
            let walk = ledger.ingest(declared)?;
            Ok(Coverage::of_walk(
                declared.client(),
                declared.location(),
                declared.template(),
                &walk,
            ))
        }
        UserAdapter::External(external) => {
            ingest_external(ledger, external)?;
            Ok(Coverage::command(external.client()))
        }
    }
}

fn ingest_external(ledger: &mut Ledger, external: &External) -> Result<()> {
    let output = external.run()?;
    ledger.ingest_output(
        external.client(),
        external.definition(),
        external.file(),
        output,
        external::reader(),
    )?;
    Ok(())
}

pub fn report<'a>(
    ledger: &Ledger,
    names: impl Iterator<Item = &'a str>,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
    project: Option<&str>,
) -> Result<Report> {
    Ok(Report {
        schema_version: SCHEMA_VERSION,
        since,
        until,
        clients: combine(
            names,
            &ledger.calls(since, until, project)?,
            &ledger.client_versions(since, until, project)?,
            &ledger.sources()?,
        ),
    })
}

/// One entry per client that has an adapter or appears in the ledger, so a
/// client with no calls is shown with zeros rather than left out.
fn combine<'a>(
    names: impl Iterator<Item = &'a str>,
    calls: &[ClientCalls],
    versions: &[VersionCalls],
    sources: &[Sources],
) -> Vec<ClientReport> {
    let mut clients: Vec<String> = names
        .map(str::to_owned)
        .chain(calls.iter().map(|entry| entry.client.clone()))
        .chain(sources.iter().map(|entry| entry.client.clone()))
        .collect();
    clients.sort();
    clients.dedup();
    clients
        .into_iter()
        .map(|client| {
            let calls = calls.iter().find(|entry| entry.client == client);
            let sources = sources.iter().find(|entry| entry.client == client);
            ClientReport {
                calls: calls.map_or(0, |entry| entry.calls),
                sessions: calls.map_or(0, |entry| entry.sessions),
                tokens: calls.map(|entry| entry.tokens).unwrap_or_default(),
                calls_with_reasoning: calls.map_or(0, |entry| entry.calls_with_reasoning),
                final_usage: calls.map(|entry| entry.final_usage).unwrap_or_default(),
                unrecorded: calls.map(|entry| entry.unrecorded).unwrap_or_default(),
                earliest_call: calls.map(|entry| entry.earliest),
                latest_call: calls.map(|entry| entry.latest),
                client_versions: versions
                    .iter()
                    .filter(|entry| entry.client == client)
                    .map(|entry| ClientVersion {
                        version: entry.version.clone(),
                        calls: entry.calls,
                    })
                    .collect(),
                sources: sources.map_or_else(SourceReport::default, |entry| SourceReport {
                    files: entry.files,
                    unreadable_lines: entry.unreadable_lines,
                    files_without_usage: entry.files_without_usage,
                }),
                client,
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    #[test]
    fn the_reserved_clients_are_the_shipped_adapters() {
        let dir = PathBuf::from("/logs");
        let claude_code = ClaudeCode::new(dir.clone());
        let codex = Codex::new(dir.clone());
        let pi = Pi::new(dir);
        let shipped: [&dyn Adapter; 3] = [&claude_code, &codex, &pi];
        let clients: Vec<&str> = shipped.iter().map(|adapter| adapter.client()).collect();
        assert_eq!(clients, SHIPPED);
    }
}
