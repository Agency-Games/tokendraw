//! Estimates the energy usage and emissions of LLM coding agent use.

mod adapters;
mod cli;
mod commands;
mod config;
mod data;
mod dirs;
mod estimate;
mod findings;
mod ledger;
mod output;
mod projects;
mod quantity;

use std::io;
use std::process::ExitCode;

use jiff::Timestamp;
use jiff::tz::TimeZone;

use crate::cli::{AdapterCommand, Cli, Command, DataCommand, LabelCommand};
use crate::commands::{adapter, label, report, tokens};

/// Parses the command line and runs the requested command.
///
/// Returns status 0 on success and 1 on failure, with the error on stderr.
/// Exits the process with status 0 after `--help` or `--version`, and with
/// status 2 on a usage error, including a call without arguments and a
/// period that does not end after it starts. A closed stdout, such as a
/// pipe into `head`, ends the command quietly.
#[must_use]
pub fn run() -> ExitCode {
    let cli = Cli::parse_checked();
    let result = match cli.command {
        Command::Report(args) => load_data()
            .and_then(|data| Ok((data, config::load()?)))
            .and_then(|(data, config)| report::run(&args, &data, config, today()))
            .and_then(|report| quietly_closed(output::report(&report, args.json, args.verbose))),
        Command::Findings(args) => load_data()
            .and_then(|data| Ok((data, config::load()?)))
            .and_then(|(data, config)| commands::findings::run(&args, &data, config, today()))
            .and_then(|findings| quietly_closed(output::findings(&findings, args.json))),
        Command::Tokens(args) => {
            tokens::run(&args).and_then(|report| quietly_closed(output::tokens(&report, args.json)))
        }
        Command::Projects(args) => commands::projects::run(&args)
            .and_then(|inventory| quietly_closed(output::projects(&inventory, args.json))),
        Command::Label(LabelCommand::Dir(args)) => {
            label::dir(&args).and_then(|change| quietly_closed(output::labelled(&change)))
        }
        Command::Label(LabelCommand::Session(args)) => {
            label::session(&args).and_then(|change| quietly_closed(output::labelled(&change)))
        }
        Command::Label(LabelCommand::List(args)) => {
            label::list().and_then(|labels| quietly_closed(output::labels(&labels, args.json)))
        }
        Command::Adapter(AdapterCommand::Check(args)) => adapter::check(&args.file)
            .and_then(|report| quietly_closed(output::tokens(&report, args.json))),
        Command::Adapter(AdapterCommand::Reset(args)) => adapter::reset(&args.client)
            .and_then(|forgotten| quietly_closed(output::forgotten(&args.client, forgotten))),
        Command::Data(args) => match args.command {
            None => {
                load_data().and_then(|data| quietly_closed(output::data(&data, today(), args.json)))
            }
            Some(DataCommand::Export(export)) => data::export(&export.dir)
                .map_err(anyhow::Error::from)
                .and_then(|written| quietly_closed(output::exported(&export.dir, &written))),
        },
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("tokendraw: {error}");
            ExitCode::FAILURE
        }
    }
}

fn load_data() -> anyhow::Result<data::Data> {
    let overrides = dirs::config().map(|config| config.join("data"));
    Ok(data::load(overrides.as_deref())?)
}

fn today() -> jiff::civil::Date {
    Timestamp::now().to_zoned(TimeZone::UTC).date()
}

fn quietly_closed(result: io::Result<()>) -> anyhow::Result<()> {
    match result {
        Err(error) if error.kind() == io::ErrorKind::BrokenPipe => Ok(()),
        other => other.map_err(|error| anyhow::anyhow!("cannot write the output: {error}")),
    }
}
