use std::path::PathBuf;

use clap::error::ErrorKind;
use clap::{Args, CommandFactory, Parser, Subcommand};
use jiff::Timestamp;
use jiff::civil::Date;
use jiff::tz::TimeZone;

/// Estimate the energy usage and emissions of LLM coding agent use
#[derive(Parser)]
#[command(version, arg_required_else_help = true)]
pub struct Cli {
    #[command(subcommand)]
    pub command: Command,
}

#[derive(Subcommand)]
pub enum Command {
    /// Read new usage from the agents' logs and print the usage and
    /// emissions by provider
    #[command(long_about = REPORT_HELP)]
    Report(ReportArgs),
    /// Read new usage from the agents' logs and print every finding: a
    /// behaviour in the calls you can change, its share of the energy and
    /// what to do about it
    #[command(long_about = FINDINGS_HELP)]
    Findings(FindingsArgs),
    /// Read new usage from the agents' logs and print the tokens per client
    Tokens(TokensArgs),
    /// Read new usage from the agents' logs and print the projects of the
    /// calls and the directories whose calls belong to none
    #[command(long_about = PROJECTS_HELP)]
    Projects(ProjectsArgs),
    /// Assign the calls of a directory or a session to a project, or list
    /// the labels
    #[command(subcommand)]
    Label(LabelCommand),
    /// Check or reset a user adapter file
    #[command(subcommand)]
    Adapter(AdapterCommand),
    /// Print the data the estimates use: its version, files, overrides and
    /// records, and the stale records
    Data(DataArgs),
}

#[derive(Args)]
#[command(args_conflicts_with_subcommands = true)]
pub struct DataArgs {
    #[command(subcommand)]
    pub command: Option<DataCommand>,
    /// Print JSON instead of text, with every record
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum DataCommand {
    /// Write the shipped data files, byte for byte, into a directory
    Export(ExportArgs),
}

#[derive(Args)]
pub struct ExportArgs {
    /// The directory to write the files into; existing files are not
    /// overwritten
    pub dir: PathBuf,
}

#[derive(Subcommand)]
pub enum AdapterCommand {
    /// Read an adapter file's logs, or run its command, and print what it
    /// counts, without writing the ledger
    Check(CheckArgs),
    /// Forget a user adapter's calls, so the next run reads its logs again
    /// with the current adapter file
    Reset(ResetArgs),
}

#[derive(Args)]
pub struct CheckArgs {
    /// The adapter file
    pub file: PathBuf,
    /// Print JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ResetArgs {
    /// The client the adapter file names
    pub client: String,
}

const PROJECTS_HELP: &str = "Read new usage from the agents' logs and print the projects the calls
belong to, how each was found, and the directories whose calls belong to none.

A call belongs to the first of these that applies:

  label     a project label on its session, else on its working directory
            or the closest labelled directory above it
  checkout  the Git checkout its working directory is in; a linked
            worktree belongs to its main checkout
  recorded  the checkout tokendraw recorded for the directory, or the
            closest recorded directory above it, when it last existed;
            every command that reads the logs records them
  ancestor  for a directory that no longer exists, the checkout its nearest
            existing parent is in
  label     a project label on a pattern such as ~/src/app-*, matching the
            directory or one above it, where none of the above applies

Otherwise the calls are unresolved and stay under their directory. Removed
worktrees kept beside their checkout are the usual case. Unresolved
directories named after a known checkout beside them (app-worktrees or
app-feature next to app) are grouped under a pattern, with the command that
would assign them all, for example

  tokendraw label dir '~/src/app-*' project=~/src/app

The others are grouped under their nearest existing parent. Nothing is
applied until you run a command.";

#[derive(Args)]
pub struct ProjectsArgs {
    /// Count calls at or after this UTC date (2026-09-01)
    /// or RFC 3339 time with its offset
    #[arg(long, value_parser = parse_time)]
    pub since: Option<Timestamp>,
    /// Count calls before this UTC date (2026-10-01)
    /// or RFC 3339 time with its offset
    #[arg(long, value_parser = parse_time)]
    pub until: Option<Timestamp>,
    /// Print JSON instead of text, with every unresolved directory and its
    /// sessions
    #[arg(long)]
    pub json: bool,
}

#[derive(Subcommand)]
pub enum LabelCommand {
    /// Label a directory and every directory below it, including ones that
    /// no longer exist
    #[command(long_about = LABEL_DIR_HELP)]
    Dir(DirLabelArgs),
    /// Label one session of a client
    #[command(long_about = LABEL_SESSION_HELP)]
    Session(SessionLabelArgs),
    /// Print every label
    List(ListArgs),
}

const LABEL_DIR_HELP: &str =
    "Label a directory and every directory below it, including ones that no
longer exist.

project=DIR assigns the calls made there to the project DIR is in: its Git
checkout, else DIR itself. A label on a closer directory, or on a session,
takes precedence.

A DIR with * is a pattern: * matches any characters within one path
component, and quotes keep the shell from expanding it, as in
'~/src/app-*'. A pattern applies only where no other label, checkout or
recorded checkout does, so a sibling checkout such as ~/src/app-tools keeps
its own calls. Where several patterns match, the deepest matched directory
wins, then the pattern with the most literal characters.

The ledger keeps the labels; the logs are not read.";

const LABEL_SESSION_HELP: &str = "Label one session of a client.

project=DIR assigns the session's calls to the project DIR is in: its Git
checkout, else DIR itself. A session label takes precedence over directory
labels. tokendraw projects --json lists the sessions of unresolved
directories.";

#[derive(Args)]
pub struct DirLabelArgs {
    /// The directory, or a pattern with *; ~/ is the home directory
    pub dir: String,
    /// KEY=VALUE, such as project=~/src/app, or KEY= to remove a label
    #[arg(required = true, value_name = "KEY=VALUE")]
    pub labels: Vec<String>,
}

#[derive(Args)]
pub struct SessionLabelArgs {
    /// The client whose logs hold the session, such as claude-code
    pub client: String,
    /// The session ID as the client's logs record it
    pub session_id: String,
    /// KEY=VALUE, such as project=~/src/app, or KEY= to remove a label
    #[arg(required = true, value_name = "KEY=VALUE")]
    pub labels: Vec<String>,
}

#[derive(Args)]
pub struct ListArgs {
    /// Print JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct TokensArgs {
    /// Count calls at or after this UTC date (2026-09-01)
    /// or RFC 3339 time with its offset
    #[arg(long, value_parser = parse_time)]
    pub since: Option<Timestamp>,
    /// Count calls before this UTC date (2026-10-01)
    /// or RFC 3339 time with its offset
    #[arg(long, value_parser = parse_time)]
    pub until: Option<Timestamp>,
    /// Print JSON instead of text
    #[arg(long)]
    pub json: bool,
}

const REPORT_HELP: &str =
    "Read new usage from the agents' logs and print the usage and emissions of
the calls by provider.

The emissions are those of layer L1, inference electricity: the energy of
serving the calls, estimated per call from the tokens each reads and writes,
times the grid intensity where the provider serves from. Holding a cache
between calls, the embodied emissions of serving hardware, training and
water are not estimated yet and do not appear in the output.

Model providers do not disclose per-token energy, attention design or
serving hardware, so every figure is given in three cases. Each case
computes the calls with every value in the data at the same point of its
range. They are scenarios, not a mean with confidence intervals:

  base case          if a closed model is a mixture-of-experts model with
                     about 100 billion active parameters and full attention
                     on B200-class accelerators; for a closed model with a
                     list price, its weight energy is what its price buys at
                     open-weight hosts' prices; for an open-weight model with
                     a published configuration, that model on B200
                     accelerators
  low to high case   the spread across documented architectures, hardware
                     and, for priced models, host prices and markups; for an
                     open-weight model with a published configuration,
                     hardware alone; it also spans the provider's idle
                     capacity, PUE and sites
  bounds             what the providers' non-disclosure leaves open

A total's cases are the sums of its providers' cases.

--verbose adds the figures between usage and emissions, in the order they
are computed: power usage effectiveness (PUE, the data centre's total power
over its IT power), electricity with the data centre's overhead, and grid
intensity. It ends with the data sources: each kind of data used with the
quality of its records, which is disclosed by the provider for the model,
published by a company and not verified, independent research, or the tool's
default with a range.

A provider that no data record names is computed with the record unknown,
and a model that no record names with the frontier parameter set; without
--verbose a note names them. tokendraw data prints every record.

Calls are assigned to projects from labels and Git checkouts, as
tokendraw projects --help explains. The report shows the emissions of each
project and of the calls that remain unresolved.

Where the energy goes splits the base case energy by what the calls did.
Rereading the context is the terms of the energy model that grow with the
context each call reads: attention (a_p and a_d) and cache reload (c0), as
the data's README describes them. The other lines are the calls with more
cache-read tokens than the long-context threshold in tokendraw data, and
the clients and roles (interactive, subagent or headless) of the calls.
--json names the signal each share comes from.

Then come the findings with at least 10% of the energy by default, each
with what you can do about it; one line counts the others, which tokendraw
findings lists.";

const FINDINGS_HELP: &str =
    "Read new usage from the agents' logs and print every finding: a behaviour in
the calls you can change, its share of the base case energy, what you can
do about it in each client the calls came from, and how to see the effect.

long-context   calls whose cache-read tokens exceed the long-context
               threshold in tokendraw data; every new and output token of
               such a call re-reads that context
fixed-context  the context each call's session or subagent started with,
               up to the call's own: system prompt, tool definitions,
               instruction files and the first prompt, which the logs do
               not separate, so its share is an upper estimate
model-choice   subagent and headless calls on models with a list price
               above the model it compares each client with, the
               lowest-priced its docs list; the share is the energy of
               the per-token terms above that model's, for closed models
               the weights, with the same tokens. The data gives closed
               models the same attention energy, so this leaves attention
               out, and another model may need more turns or tokens.
               tokendraw data names the roles and models

tokendraw report shows only the findings with at least 10% of the energy
by default; this command shows them all.";

#[derive(Args)]
pub struct FindingsArgs {
    /// Count calls at or after this UTC date (2026-09-01)
    /// or RFC 3339 time with its offset
    #[arg(long, value_parser = parse_time)]
    pub since: Option<Timestamp>,
    /// Count calls before this UTC date (2026-10-01)
    /// or RFC 3339 time with its offset
    #[arg(long, value_parser = parse_time)]
    pub until: Option<Timestamp>,
    /// Count only the calls of project DIR as `tokendraw projects` names
    /// it, else of the project DIR is in, or the unresolved calls made in DIR
    #[arg(long, value_name = "DIR")]
    pub project: Option<String>,
    /// Print JSON instead of text
    #[arg(long)]
    pub json: bool,
}

#[derive(Args)]
pub struct ReportArgs {
    /// Report calls at or after this UTC date (2026-09-01)
    /// or RFC 3339 time with its offset
    #[arg(long, value_parser = parse_time)]
    pub since: Option<Timestamp>,
    /// Report calls before this UTC date (2026-10-01)
    /// or RFC 3339 time with its offset
    #[arg(long, value_parser = parse_time)]
    pub until: Option<Timestamp>,
    /// Also print the electricity, data centre overhead and grid intensity
    /// the emissions are computed from
    #[arg(long, short)]
    pub verbose: bool,
    /// Report only the calls of project DIR as `tokendraw projects` names
    /// it, else of the project DIR is in, or the unresolved calls made in DIR
    #[arg(long, value_name = "DIR")]
    pub project: Option<String>,
    /// Print JSON instead of text, with every figure
    #[arg(long)]
    pub json: bool,
    /// Replace names on this machine so the report can be shared: project
    /// names, the paths of data override files, text from data overrides,
    /// and log locations you set. Shipped log locations show as tokendraw
    /// looks for them, such as ~/.claude/projects or a variable's name
    /// without its value. The ids of clients, models, providers and data
    /// records stay, as do versions, counts and figures; read the output
    /// before sending it
    #[arg(long)]
    pub redact: bool,
}

impl Cli {
    /// Parses the command line and exits with status 2 on a usage error,
    /// including a period that does not end after it starts.
    pub fn parse_checked() -> Self {
        Self::parse().checked().unwrap_or_else(|error| error.exit())
    }

    fn checked(self) -> Result<Self, clap::Error> {
        let (name, period) = match &self.command {
            Command::Report(args) => ("report", (args.since, args.until)),
            Command::Findings(args) => ("findings", (args.since, args.until)),
            Command::Tokens(args) => ("tokens", (args.since, args.until)),
            Command::Projects(args) => ("projects", (args.since, args.until)),
            _ => return Ok(self),
        };
        if let (Some(since), Some(until)) = period
            && since >= until
        {
            let mut command = Self::command();
            command.build();
            let message = format!("--since {since} is not before --until {until}");
            return Err(match command.find_subcommand_mut(name) {
                Some(subcommand) => subcommand.error(ErrorKind::ArgumentConflict, message),
                None => command.error(ErrorKind::ArgumentConflict, message),
            });
        }
        Ok(self)
    }
}

/// A date, exactly `YYYY-MM-DD`, means its first moment in UTC; anything
/// else must be an RFC 3339 time with its offset.
fn parse_time(value: &str) -> Result<Timestamp, String> {
    let date = value.len() == 10 && value.as_bytes()[4] == b'-' && value.as_bytes()[7] == b'-';
    let parsed = if date {
        value
            .parse::<Date>()
            .and_then(|date| date.to_zoned(TimeZone::UTC))
            .map(|zoned| zoned.timestamp())
    } else {
        value.parse::<Timestamp>()
    };
    parsed.map_err(|_| {
        "expected a date such as 2026-09-01 or a time with its offset such as 2026-09-01T12:00:00Z"
            .to_owned()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_date_is_midnight_utc() -> Result<(), String> {
        assert_eq!(
            parse_time("2026-09-01")?,
            parse_time("2026-09-01T00:00:00Z")?
        );
        Ok(())
    }

    #[test]
    fn a_time_with_an_offset_is_converted_to_utc() -> Result<(), String> {
        assert_eq!(
            parse_time("2026-09-01T02:00:00+02:00")?,
            parse_time("2026-09-01T00:00:00Z")?
        );
        Ok(())
    }

    #[test]
    fn other_text_is_rejected() {
        assert!(parse_time("September").is_err());
    }

    #[test]
    fn a_time_without_an_offset_is_rejected() {
        for value in [
            "2026-09-15T12:00:00",
            "2026-09-15 12:00",
            "2026-09-15T12:00:00[Europe/Helsinki]",
            "2026-9-15",
        ] {
            assert!(parse_time(value).is_err(), "{value}");
        }
    }

    fn checked(args: &[&str]) -> Result<Cli, clap::Error> {
        Cli::try_parse_from(args)?.checked()
    }

    #[test]
    fn a_period_must_end_after_it_starts() -> Result<(), clap::Error> {
        for command in ["report", "findings", "tokens", "projects"] {
            for (since, until) in [("2026-10-05", "2026-10-01"), ("2026-10-01", "2026-10-01")] {
                let error = checked(&["tokendraw", command, "--since", since, "--until", until])
                    .err()
                    .map(|error| error.kind());
                assert_eq!(
                    error,
                    Some(ErrorKind::ArgumentConflict),
                    "{command} {since} {until}"
                );
            }
            checked(&[
                "tokendraw",
                command,
                "--since",
                "2026-09-30",
                "--until",
                "2026-10-01",
            ])?;
        }
        Ok(())
    }
}
