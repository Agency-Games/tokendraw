//! The `adapter` commands: check a user adapter file before installing it,
//! and reset one whose file changed.

use std::path::Path;

use anyhow::{Result, bail};

use crate::adapters::coverage::Status;
use crate::adapters::user;
use crate::commands::tokens::{self, Report, SHIPPED};
use crate::ledger::{Forgotten, Ledger};

/// The ledger in the state directory is not touched.
pub fn check(file: &Path) -> Result<Report> {
    let adapter = user::load(file, &SHIPPED)?;
    let mut ledger = Ledger::in_memory()?;
    let coverage = tokens::ingest(&mut ledger, &adapter)?;
    if let (Status::PartlyRead | Status::FoundNotRead, Some(location), Some(reason)) =
        (coverage.status, &coverage.location, &coverage.reason)
    {
        bail!("cannot read all of {location}: {reason}; check that it is readable");
    }
    tokens::report(&ledger, [adapter.client()].into_iter(), None, None, None)
}

/// Afterwards the next ingestion reads the client's logs under its adapter
/// file as it is now.
pub fn reset(client: &str) -> Result<Forgotten> {
    if SHIPPED.contains(&client) {
        bail!(
            "{client} is a shipped adapter's client; reset forgets the calls of user adapters only"
        );
    }
    let mut ledger = tokens::open_ledger()?;
    Ok(ledger.forget(client)?)
}
