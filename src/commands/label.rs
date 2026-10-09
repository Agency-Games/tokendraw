use std::collections::BTreeSet;
use std::path::Path;

use anyhow::{Result, bail};

use crate::cli::{DirLabelArgs, SessionLabelArgs};
use crate::commands::tokens;
use crate::ledger::{LabelRecord, Ledger, Target};
use crate::projects::{self, PROJECT, Resolver};

#[derive(Debug, PartialEq, Eq)]
pub struct Change {
    pub target: Target,
    pub set: Vec<(String, String)>,
    pub removed: Vec<String>,
    /// The calls in the ledger made in the directory or below it, or in
    /// the session; for a pattern, those in matching directories that no
    /// checkout holds, which the pattern can assign.
    pub calls: u64,
    /// For a pattern, the directories of those calls.
    pub directories: Option<u64>,
}

/// Labels a directory and every directory below it.
pub fn dir(args: &DirLabelArgs) -> Result<Change> {
    let dir = projects::display(&projects::argument(&args.dir)?);
    apply(Target::Directory(dir), &args.labels)
}

pub fn session(args: &SessionLabelArgs) -> Result<Change> {
    apply(
        Target::Session {
            client: args.client.clone(),
            session_id: args.session_id.clone(),
        },
        &args.labels,
    )
}

pub fn list() -> Result<Vec<LabelRecord>> {
    Ok(tokens::open_ledger()?.labels()?)
}

fn apply(target: Target, labels: &[String]) -> Result<Change> {
    let mut ledger = tokens::open_ledger()?;
    let mut resolver = Resolver::new(&ledger)?;
    let mut changes: Vec<(String, Option<String>)> = Vec::new();
    for label in labels {
        let Some((key, value)) = label.split_once('=') else {
            bail!(
                "expected KEY=VALUE, such as project=~/src/app, or KEY= to remove a label, not {label}"
            );
        };
        if key.is_empty()
            || !key
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
        {
            bail!("the label key {key:?} is not valid; use lowercase letters, digits, - and _");
        }
        if changes.iter().any(|(other, _)| other == key) {
            bail!("the label key {key} is given twice; give each key once");
        }
        let value = match value {
            "" => None,
            value if key == PROJECT => {
                Some(projects::select(&ledger, &mut resolver, &projects::argument(value)?)?.name)
            }
            value => Some(value.to_owned()),
        };
        changes.push((key.to_owned(), value));
    }
    ledger.set_labels(&target, &changes)?;
    let (calls, directories) = calls(&ledger, &target)?;
    let (set, removed): (Vec<_>, Vec<_>) =
        changes.into_iter().partition(|(_, value)| value.is_some());
    Ok(Change {
        target,
        set: set
            .into_iter()
            .filter_map(|(key, value)| value.map(|value| (key, value)))
            .collect(),
        removed: removed.into_iter().map(|(key, _)| key).collect(),
        calls,
        directories,
    })
}

fn calls(ledger: &Ledger, target: &Target) -> Result<(u64, Option<u64>)> {
    let contexts = ledger.contexts(None, None)?;
    let dir_of = |cwd: Option<&str>| {
        cwd.map(Path::new)
            .filter(|path| path.is_absolute())
            .map(projects::normalize)
    };
    match target {
        Target::Directory(pattern) if projects::is_pattern(pattern) => {
            let mut resolver = Resolver::new(ledger)?;
            let mut directories = BTreeSet::new();
            let mut calls = 0;
            for context in &contexts {
                let Some(dir) = dir_of(context.cwd.as_deref()) else {
                    continue;
                };
                if projects::covers(pattern, &dir) && !resolver.in_checkout(&dir) {
                    calls += context.calls;
                    directories.insert(dir);
                }
            }
            Ok((calls, Some(directories.len() as u64)))
        }
        Target::Directory(target) => Ok((
            contexts
                .iter()
                .filter(|context| {
                    dir_of(context.cwd.as_deref()).is_some_and(|dir| dir.starts_with(target))
                })
                .map(|context| context.calls)
                .sum(),
            None,
        )),
        Target::Session { client, session_id } => Ok((
            contexts
                .iter()
                .filter(|context| context.client == *client && context.session_id == *session_id)
                .map(|context| context.calls)
                .sum(),
            None,
        )),
    }
}
