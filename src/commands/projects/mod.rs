use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use anyhow::Result;
use jiff::Timestamp;
use serde::Serialize;

use crate::cli::ProjectsArgs;
use crate::commands::report::Period;
use crate::commands::tokens;
use crate::ledger::Context;
use crate::projects::{self, Attributed, PROJECT, Resolution, ResolvedBy, Resolver};

/// The version of the JSON; it changes only when a field is removed or
/// changed.
const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Serialize)]
pub struct Inventory {
    pub schema_version: u32,
    pub period: Period,
    /// Most calls first.
    pub projects: Vec<ProjectCalls>,
    pub unresolved: Unresolved,
}

#[derive(Debug, Serialize)]
pub struct ProjectCalls {
    pub project: String,
    pub calls: u64,
    pub resolved_by: ResolvedBy,
}

#[derive(Debug, Serialize)]
pub struct Unresolved {
    pub calls: u64,
    /// The directories of the calls, grouped under the directory a
    /// suggested label is on, else under their nearest existing parent;
    /// most calls first.
    pub groups: Vec<Group>,
    pub without_directory: Option<Activity>,
}

#[derive(Debug, Serialize)]
pub struct Group {
    /// The directory, or for a suggestion the pattern, holding `*`.
    pub directory: String,
    /// The label a directory's name suggests; never applied.
    pub suggestion: Option<Suggestion>,
    pub calls: u64,
    pub sessions: u64,
    pub clients: Vec<String>,
    pub first_call: Timestamp,
    pub last_call: Timestamp,
    /// Most calls first.
    pub directories: Vec<Directory>,
}

#[derive(Debug, Serialize)]
pub struct Suggestion {
    pub project: String,
    /// `null` when the pattern would also match another group's
    /// directories.
    pub command: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct Directory {
    pub directory: String,
    #[serde(flatten)]
    pub calls: Activity,
}

#[derive(Debug, Serialize)]
pub struct Activity {
    pub calls: u64,
    pub sessions: Vec<Session>,
    pub clients: Vec<String>,
    pub first_call: Timestamp,
    pub last_call: Timestamp,
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord, Serialize)]
pub struct Session {
    pub client: String,
    pub session_id: String,
}

#[derive(Default)]
struct Tally {
    calls: u64,
    sessions: BTreeSet<Session>,
    times: Option<(Timestamp, Timestamp)>,
}

impl Tally {
    fn add(&mut self, context: &Context) {
        self.calls += context.calls;
        self.sessions.insert(Session {
            client: context.client.clone(),
            session_id: context.session_id.clone(),
        });
        self.times = Some(match self.times {
            Some((first, last)) => (first.min(context.earliest), last.max(context.latest)),
            None => (context.earliest, context.latest),
        });
    }

    fn merge(&mut self, other: &Tally) {
        self.calls += other.calls;
        self.sessions.extend(other.sessions.iter().cloned());
        self.times = match (self.times, other.times) {
            (Some((first, last)), Some((other_first, other_last))) => {
                Some((first.min(other_first), last.max(other_last)))
            }
            (times, other) => times.or(other),
        };
    }

    fn clients(&self) -> Vec<String> {
        let clients: BTreeSet<&str> = self
            .sessions
            .iter()
            .map(|session| session.client.as_str())
            .collect();
        clients.into_iter().map(str::to_owned).collect()
    }

    fn activity(&self) -> Activity {
        let (first_call, last_call) = self.times.unwrap_or_default();
        Activity {
            calls: self.calls,
            sessions: self.sessions.iter().cloned().collect(),
            clients: self.clients(),
            first_call,
            last_call,
        }
    }
}

/// Ingests new usage, then lists the projects of the calls in the range.
pub fn run(args: &ProjectsArgs) -> Result<Inventory> {
    let (mut ledger, _) = tokens::ingest_all()?;
    let mut resolver = Resolver::new(&ledger)?;
    let attributed = projects::attribute(&mut ledger, &mut resolver, args.since, args.until)?;
    // Labels apply to calls of any time, so a suggestion is checked against
    // the unresolved directories of any time, not only those in the range.
    let mut unresolved = BTreeSet::new();
    for context in ledger.contexts(None, None)? {
        let attributed =
            resolver.attribute(&context.client, &context.session_id, context.cwd.as_deref());
        if let (Resolution::Unresolved, Some(dir)) = (attributed.resolution, attributed.project) {
            unresolved.insert(dir);
        }
    }
    Ok(build(
        &attributed,
        &resolver.checkouts(),
        &unresolved,
        Period {
            start: args.since,
            end: args.until,
        },
    ))
}

/// `unresolved` holds the unresolved directories of any time, not only
/// those of `attributed`.
#[must_use]
pub fn build(
    attributed: &[(Context, Attributed)],
    checkouts: &BTreeSet<PathBuf>,
    unresolved: &BTreeSet<String>,
    period: Period,
) -> Inventory {
    let mut projects: BTreeMap<&str, (u64, ResolvedBy)> = BTreeMap::new();
    let mut directories: BTreeMap<&str, Tally> = BTreeMap::new();
    let mut without_directory = Tally::default();
    for (context, attributed) in attributed {
        match (attributed.resolution, attributed.project.as_deref()) {
            (_, None) => without_directory.add(context),
            (Resolution::Unresolved, Some(dir)) => directories.entry(dir).or_default().add(context),
            (resolution, Some(project)) => {
                let (calls, resolved_by) = projects.entry(project).or_default();
                *calls += context.calls;
                resolved_by.add(resolution, context.calls);
            }
        }
    }
    let mut projects: Vec<ProjectCalls> = projects
        .into_iter()
        .map(|(project, (calls, resolved_by))| ProjectCalls {
            project: project.to_owned(),
            calls,
            resolved_by,
        })
        .collect();
    projects.sort_by(|a, b| {
        b.calls
            .cmp(&a.calls)
            .then_with(|| a.project.cmp(&b.project))
    });
    let groups = groups(&directories, checkouts, unresolved);
    let unresolved_calls =
        groups.iter().map(|group| group.calls).sum::<u64>() + without_directory.calls;
    Inventory {
        schema_version: SCHEMA_VERSION,
        period,
        projects,
        unresolved: Unresolved {
            calls: unresolved_calls,
            groups,
            without_directory: (without_directory.calls > 0).then(|| without_directory.activity()),
        },
    }
}

fn groups(
    directories: &BTreeMap<&str, Tally>,
    checkouts: &BTreeSet<PathBuf>,
    unresolved: &BTreeSet<String>,
) -> Vec<Group> {
    let mut groups: BTreeMap<PathBuf, (Option<PathBuf>, Tally, Vec<Directory>)> = BTreeMap::new();
    for (dir, tally) in directories {
        let (key, project) = group(Path::new(dir), checkouts);
        let (_, group_tally, members) =
            groups
                .entry(key)
                .or_insert((project, Tally::default(), Vec::new()));
        group_tally.merge(tally);
        members.push(Directory {
            directory: (*dir).to_owned(),
            calls: tally.activity(),
        });
    }
    let mut groups: Vec<Group> = groups
        .into_iter()
        .map(|(directory, (project, tally, mut members))| {
            let pattern = projects::display(&directory);
            // A command that would also assign another group's directories
            // is left out: the user labels this group by hand.
            let overlaps = || {
                unresolved.iter().any(|dir| {
                    let dir = Path::new(dir);
                    projects::covers(&pattern, dir) && group(dir, checkouts).0 != directory
                })
            };
            let command = (project.is_some() && !overlaps()).then(|| {
                format!(
                    "tokendraw label dir {} {PROJECT}={}",
                    quoted(&pattern),
                    quoted(
                        &project
                            .as_deref()
                            .map(projects::display)
                            .unwrap_or_default()
                    )
                )
            });
            members.sort_by(|a, b| {
                b.calls
                    .calls
                    .cmp(&a.calls.calls)
                    .then_with(|| a.directory.cmp(&b.directory))
            });
            let calls = tally.activity();
            Group {
                suggestion: project.map(|project| Suggestion {
                    command,
                    project: projects::display(&project),
                }),
                directory: pattern,
                calls: calls.calls,
                sessions: calls.sessions.len() as u64,
                clients: calls.clients,
                first_call: calls.first_call,
                last_call: calls.last_call,
                directories: members,
            }
        })
        .collect();
    groups.sort_by(|a, b| {
        b.calls
            .cmp(&a.calls)
            .then_with(|| a.directory.cmp(&b.directory))
    });
    groups
}

fn group(dir: &Path, checkouts: &BTreeSet<PathBuf>) -> (PathBuf, Option<PathBuf>) {
    match projects::suggestion(dir, checkouts) {
        Some((pattern, project)) => (pattern, Some(project)),
        None => (existing(dir), None),
    }
}

fn existing(path: &Path) -> PathBuf {
    path.ancestors()
        .find(|ancestor| ancestor.is_dir())
        .unwrap_or(path)
        .to_path_buf()
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Shell {
    Posix,
    PowerShell,
}

impl Shell {
    /// PowerShell on Windows, where Command Prompt has no single quotes,
    /// else POSIX.
    pub const NATIVE: Self = if cfg!(windows) {
        Self::PowerShell
    } else {
        Self::Posix
    };
}

#[must_use]
pub fn quoted(text: &str) -> String {
    quoted_for(text, Shell::NATIVE)
}

#[must_use]
pub fn quoted_for(text: &str, shell: Shell) -> String {
    // PowerShell reads a leading `@` as splatting, and `,` is its array
    // operator.
    let plain_symbols = match shell {
        Shell::Posix => "/._-~+:@%,",
        Shell::PowerShell => "/._-~+:%",
    };
    let plain = !text.is_empty()
        && text.chars().all(|c| {
            c.is_ascii_alphanumeric() || plain_symbols.contains(c) || c == std::path::MAIN_SEPARATOR
        });
    if plain {
        return text.to_owned();
    }
    match shell {
        Shell::Posix => format!("'{}'", text.replace('\'', r"'\''")),
        // PowerShell also reads the typographic single quotes as quotes,
        // and a quote written twice inside single quotes as one.
        Shell::PowerShell => {
            let mut word = String::from("'");
            for c in text.chars() {
                if matches!(c, '\'' | '\u{2018}'..='\u{201B}') {
                    word.push(c);
                }
                word.push(c);
            }
            word.push('\'');
            word
        }
    }
}

#[cfg(test)]
mod tests;
