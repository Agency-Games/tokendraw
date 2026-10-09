//! The project of a session's calls is the first of: a `project` label on
//! the session; a `project` label on the working directory or the closest
//! labelled directory above it; the Git checkout holding the directory; the
//! checkout recorded for the directory, or the closest recorded directory
//! above it, when it last existed; for a directory that no longer exists,
//! the checkout holding its nearest existing ancestor; a `project` label on
//! a pattern matching the directory or one above it. Otherwise the calls
//! are unresolved and stay under their directory.

use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};
use std::{env, fs, io};

use jiff::Timestamp;
use serde::Serialize;
use thiserror::Error;

use crate::ledger::{self, Attribution, Context, Ledger, Target};

pub const PROJECT: &str = "project";

/// How a context's project was found.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Resolution {
    Label,
    Checkout,
    Recorded,
    Ancestor,
    Unresolved,
}

impl Resolution {
    /// The name the ledger and the JSON use.
    #[must_use]
    pub fn name(self) -> &'static str {
        match self {
            Resolution::Label => "label",
            Resolution::Checkout => "checkout",
            Resolution::Recorded => "recorded",
            Resolution::Ancestor => "ancestor",
            Resolution::Unresolved => "unresolved",
        }
    }

    #[must_use]
    pub fn from_name(name: &str) -> Option<Resolution> {
        [
            Resolution::Label,
            Resolution::Checkout,
            Resolution::Recorded,
            Resolution::Ancestor,
            Resolution::Unresolved,
        ]
        .into_iter()
        .find(|resolution| resolution.name() == name)
    }
}

/// Calls by how their project was found.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct ResolvedBy {
    pub label: u64,
    pub checkout: u64,
    pub recorded: u64,
    pub ancestor: u64,
}

impl ResolvedBy {
    pub fn add(&mut self, resolution: Resolution, calls: u64) {
        match resolution {
            Resolution::Label => self.label += calls,
            Resolution::Checkout => self.checkout += calls,
            Resolution::Recorded => self.recorded += calls,
            Resolution::Ancestor => self.ancestor += calls,
            Resolution::Unresolved => {}
        }
    }
}

/// A path on the command line that cannot be made absolute.
#[derive(Debug, Error)]
pub enum ArgumentError {
    #[error("cannot read the current directory: {0}; give an absolute path")]
    CurrentDir(io::Error),
    #[error(
        "cannot expand ~ in {0}: no home directory is known; set HOME or give an absolute path"
    )]
    Home(String),
}

pub fn argument(arg: &str) -> Result<PathBuf, ArgumentError> {
    let cwd = env::current_dir().map_err(ArgumentError::CurrentDir)?;
    absolute(arg, &cwd, env::home_dir().as_deref())
        .ok_or_else(|| ArgumentError::Home(arg.to_owned()))
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Attributed {
    /// The project, or the directory of unresolved calls; `None` for
    /// unresolved calls without a working directory.
    pub project: Option<String>,
    pub resolution: Resolution,
}

/// Reads each directory's `.git` entries once.
pub struct Resolver {
    session_labels: HashMap<(String, String), String>,
    directory_labels: HashMap<PathBuf, String>,
    patterns: Vec<Pattern>,
    recorded: HashMap<PathBuf, PathBuf>,
    live: HashMap<PathBuf, Option<PathBuf>>,
}

impl Resolver {
    pub fn new(ledger: &Ledger) -> Result<Self, ledger::Error> {
        let mut session_labels = HashMap::new();
        let mut directory_labels = HashMap::new();
        let mut patterns = Vec::new();
        for label in ledger.labels()? {
            if label.key != PROJECT {
                continue;
            }
            match label.target {
                Target::Directory(dir) if is_pattern(&dir) => {
                    patterns.push(Pattern::new(dir, label.value));
                }
                Target::Directory(dir) => {
                    directory_labels.insert(PathBuf::from(dir), label.value);
                }
                Target::Session { client, session_id } => {
                    session_labels.insert((client, session_id), label.value);
                }
            }
        }
        let recorded = ledger
            .recorded_checkouts()?
            .into_iter()
            .map(|(path, checkout)| (PathBuf::from(path), PathBuf::from(checkout)))
            .collect();
        Ok(Self {
            session_labels,
            directory_labels,
            patterns,
            recorded,
            live: HashMap::new(),
        })
    }

    pub fn attribute(&mut self, client: &str, session_id: &str, cwd: Option<&str>) -> Attributed {
        let label = |project: &String| Attributed {
            project: Some(project.clone()),
            resolution: Resolution::Label,
        };
        if let Some(project) = self
            .session_labels
            .get(&(client.to_owned(), session_id.to_owned()))
        {
            return label(project);
        }
        let Some(cwd) = cwd else {
            return Attributed {
                project: None,
                resolution: Resolution::Unresolved,
            };
        };
        let path = Path::new(cwd);
        // A relative directory names no directory on this machine.
        if !path.is_absolute() {
            return Attributed {
                project: None,
                resolution: Resolution::Unresolved,
            };
        }
        let dir = normalize(path);
        if let Some(project) = closest(&self.directory_labels, &dir) {
            return label(project);
        }
        if let Some((checkout, resolution)) = self.resolve(&dir) {
            return Attributed {
                project: Some(display(&checkout)),
                resolution,
            };
        }
        if let Some(project) = self.pattern(&dir) {
            return label(project);
        }
        Attributed {
            project: Some(display(&dir)),
            resolution: Resolution::Unresolved,
        }
    }

    /// The project of the pattern label that matches `dir` or the deepest
    /// directory above it that one matches; of several patterns matching
    /// that directory, the most specific.
    fn pattern(&self, dir: &Path) -> Option<&String> {
        dir.ancestors().find_map(|ancestor| {
            self.patterns
                .iter()
                .filter(|pattern| pattern.matches(ancestor))
                .min_by(|a, b| a.rank().cmp(&b.rank()))
                .map(|pattern| &pattern.project)
        })
    }

    /// The project an absolute, normalized path is in: its checkout, live,
    /// recorded or of its nearest existing ancestor, else the path itself.
    /// Labels do not apply: they say where calls go, not what a project is
    /// called.
    pub fn project(&mut self, path: &Path) -> String {
        display(
            &self
                .resolve(path)
                .map_or_else(|| path.to_path_buf(), |(checkout, _)| checkout),
        )
    }

    pub fn in_checkout(&mut self, dir: &Path) -> bool {
        self.resolve(dir).is_some()
    }

    /// Every checkout seen so far, live or recorded.
    #[must_use]
    pub fn checkouts(&self) -> BTreeSet<PathBuf> {
        self.recorded
            .values()
            .cloned()
            .chain(self.live.values().flatten().cloned())
            .collect()
    }

    fn resolve(&mut self, dir: &Path) -> Option<(PathBuf, Resolution)> {
        if let Some(checkout) = self.live(dir) {
            return Some((checkout, Resolution::Checkout));
        }
        if let Some(checkout) = closest(&self.recorded, dir) {
            return Some((checkout.clone(), Resolution::Recorded));
        }
        if dir.exists() {
            return None;
        }
        let ancestor = dir.ancestors().skip(1).find(|ancestor| ancestor.is_dir())?;
        self.live(ancestor)
            .map(|checkout| (checkout, Resolution::Ancestor))
    }

    fn live(&mut self, dir: &Path) -> Option<PathBuf> {
        self.live
            .entry(dir.to_path_buf())
            .or_insert_with(|| checkout(dir))
            .clone()
    }
}

/// What a path given on the command line selects.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Selection {
    /// The project, or the directory of unresolved calls, as
    /// `tokendraw projects` lists it.
    pub name: String,
    pub selected: Selected,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Selected {
    /// Calls of some time are attributed to the project.
    Project,
    /// The calls of the directory that no label or checkout assigns.
    Unresolved,
    /// No call of any time is attributed to the name.
    Nothing(Elsewhere),
}

/// The calls made in a directory or below it, by the attribution they got.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Elsewhere {
    /// Projects with their calls, the most calls first.
    pub projects: Vec<(String, u64)>,
    pub unresolved: u64,
}

/// What a path given on the command line selects: the name calls of any
/// time are attributed to exactly, as `tokendraw projects` lists it, else
/// the project the path is in.
pub fn select(
    ledger: &Ledger,
    resolver: &mut Resolver,
    path: &Path,
) -> Result<Selection, ledger::Error> {
    let contexts = ledger.contexts(None, None)?;
    let attributed: Vec<(&Context, Attributed)> = contexts
        .iter()
        .map(|context| {
            let attributed =
                resolver.attribute(&context.client, &context.session_id, context.cwd.as_deref());
            (context, attributed)
        })
        .collect();
    let resolutions = |name: &str| -> Vec<Resolution> {
        attributed
            .iter()
            .filter(|(_, attributed)| attributed.project.as_deref() == Some(name))
            .map(|(_, attributed)| attributed.resolution)
            .collect()
    };
    let mut name = display(path);
    if resolutions(&name).is_empty() {
        name = resolver.project(path);
    }
    let found = resolutions(&name);
    let selected = if found
        .iter()
        .any(|resolution| *resolution != Resolution::Unresolved)
    {
        Selected::Project
    } else if found.is_empty() {
        Selected::Nothing(elsewhere(&attributed, Path::new(&name)))
    } else {
        Selected::Unresolved
    };
    Ok(Selection { name, selected })
}

fn elsewhere(attributed: &[(&Context, Attributed)], dir: &Path) -> Elsewhere {
    let mut projects: HashMap<&str, u64> = HashMap::new();
    let mut unresolved = 0;
    for (context, attributed) in attributed {
        let below = context
            .cwd
            .as_deref()
            .map(Path::new)
            .is_some_and(|cwd| cwd.is_absolute() && normalize(cwd).starts_with(dir));
        if !below {
            continue;
        }
        match (&attributed.project, attributed.resolution) {
            (Some(project), resolution) if resolution != Resolution::Unresolved => {
                *projects.entry(project).or_default() += context.calls;
            }
            _ => unresolved += context.calls,
        }
    }
    let mut projects: Vec<(String, u64)> = projects
        .into_iter()
        .map(|(project, calls)| (project.to_owned(), calls))
        .collect();
    projects.sort_by(|a, b| b.1.cmp(&a.1).then_with(|| a.0.cmp(&b.0)));
    Elsewhere {
        projects,
        unresolved,
    }
}

/// A directory label whose path holds `*`.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Pattern {
    text: String,
    components: Vec<String>,
    project: String,
}

impl Pattern {
    fn new(text: String, project: String) -> Self {
        let components = components(Path::new(&text));
        Self {
            text,
            components,
            project,
        }
    }

    /// Whether `dir` itself matches, component by component, `*` standing
    /// for any characters within one component.
    fn matches(&self, dir: &Path) -> bool {
        let parts = components(dir);
        parts.len() == self.components.len()
            && self
                .components
                .iter()
                .zip(&parts)
                .all(|(pattern, part)| glob(pattern, part))
    }

    /// Orders patterns matching one directory, most specific first: the
    /// most literal characters, then the longest literal text before the
    /// first `*`, then the pattern in byte order.
    fn rank(&self) -> (std::cmp::Reverse<usize>, std::cmp::Reverse<usize>, &str) {
        let literal = self.text.chars().filter(|c| *c != '*').count();
        let prefix = self.text.find('*').unwrap_or(self.text.len());
        (
            std::cmp::Reverse(literal),
            std::cmp::Reverse(prefix),
            &self.text,
        )
    }
}

/// Whether `dir`, or a directory above it, matches `pattern`.
#[must_use]
pub fn covers(pattern: &str, dir: &Path) -> bool {
    let pattern = Pattern::new(pattern.to_owned(), String::new());
    dir.ancestors().any(|ancestor| pattern.matches(ancestor))
}

#[must_use]
pub fn is_pattern(path: &str) -> bool {
    path.contains('*')
}

fn components(path: &Path) -> Vec<String> {
    path.components()
        .map(|component| component.as_os_str().to_string_lossy().into_owned())
        .collect()
}

/// Whether `text` matches `pattern`, where `*` matches any characters.
fn glob(pattern: &str, text: &str) -> bool {
    let pattern: Vec<char> = pattern.chars().collect();
    let text: Vec<char> = text.chars().collect();
    let (mut p, mut t) = (0, 0);
    let mut star: Option<(usize, usize)> = None;
    while t < text.len() {
        if p < pattern.len() && pattern[p] == '*' {
            star = Some((p, t));
            p += 1;
        } else if p < pattern.len() && pattern[p] == text[t] {
            p += 1;
            t += 1;
        } else if let Some((star_p, star_t)) = star {
            p = star_p + 1;
            t = star_t + 1;
            star = Some((star_p, star_t + 1));
        } else {
            return false;
        }
    }
    pattern[p..].iter().all(|c| *c == '*')
}

/// Gives the ledger the attribution of every context of calls in the
/// range, so its queries can select and group by project.
pub fn attribute(
    ledger: &mut Ledger,
    resolver: &mut Resolver,
    since: Option<Timestamp>,
    until: Option<Timestamp>,
) -> Result<Vec<(Context, Attributed)>, ledger::Error> {
    let attributed: Vec<(Context, Attributed)> = ledger
        .contexts(since, until)?
        .into_iter()
        .map(|context| {
            let attributed =
                resolver.attribute(&context.client, &context.session_id, context.cwd.as_deref());
            (context, attributed)
        })
        .collect();
    let rows: Vec<Attribution> = attributed
        .iter()
        .map(|(context, attributed)| Attribution {
            client: &context.client,
            session_id: &context.session_id,
            cwd: context.cwd.as_deref(),
            project: attributed.project.as_deref(),
            resolution: attributed.resolution.name(),
        })
        .collect();
    ledger.attribute(&rows)?;
    Ok(attributed)
}

/// Records the checkout of every working directory in the ledger that is in
/// one now, so its calls keep their project after the directory is
/// removed.
pub fn record(ledger: &mut Ledger, now: Timestamp) -> Result<(), ledger::Error> {
    let checkouts: Vec<(String, String)> = ledger
        .working_directories()?
        .iter()
        .map(Path::new)
        .filter(|path| path.is_absolute())
        .map(normalize)
        .filter_map(|dir| checkout(&dir).map(|checkout| (display(&dir), display(&checkout))))
        .collect();
    ledger.record_checkouts(&checkouts, now)
}

/// The checkout holding `dir`, found from the first `.git` entry at or
/// above it as Git finds it: a `.git` directory's checkout is its parent; a
/// `.git` file names a linked worktree's git directory, whose `commondir`
/// leads to the main checkout, or a submodule's, which is its own
/// checkout. `None` when `dir` is not a directory or no checkout holds it.
/// Git itself is not run.
#[must_use]
pub fn checkout(dir: &Path) -> Option<PathBuf> {
    if !dir.is_dir() {
        return None;
    }
    let holder = dir
        .ancestors()
        .find(|candidate| candidate.join(".git").exists())?;
    let git = holder.join(".git");
    if git.is_dir() {
        return canonical(holder);
    }
    let text = fs::read_to_string(&git).ok()?;
    let gitdir = holder.join(text.lines().next()?.strip_prefix("gitdir:")?.trim());
    if !gitdir.is_dir() {
        return None;
    }
    let Ok(commondir) = fs::read_to_string(gitdir.join("commondir")) else {
        return canonical(holder);
    };
    let common = canonical(&gitdir.join(commondir.trim()))?;
    if common.file_name() == Some(OsStr::new(".git")) {
        common.parent().map(Path::to_path_buf)
    } else {
        Some(common)
    }
}

/// A drive path loses the `\\?\` prefix Windows adds, so it reads as
/// people write it.
fn canonical(path: &Path) -> Option<PathBuf> {
    let path = fs::canonicalize(path).ok()?;
    if cfg!(windows)
        && let Some(drive) = path
            .to_str()
            .and_then(|text| text.strip_prefix(r"\\?\"))
            .filter(|rest| !rest.starts_with(r"UNC\"))
    {
        return Some(PathBuf::from(drive));
    }
    Some(path)
}

/// A path given on the command line as an absolute, normalized path: `~`
/// and a leading `~/` stand for `home`, and a relative path is taken from
/// `cwd`. `None` when the path starts with `~` and no home is known.
#[must_use]
pub fn absolute(arg: &str, cwd: &Path, home: Option<&Path>) -> Option<PathBuf> {
    let rest = arg.strip_prefix('~').filter(|rest| {
        rest.is_empty() || rest.starts_with('/') || rest.starts_with(std::path::MAIN_SEPARATOR)
    });
    let path = match rest {
        Some(rest) => home?.join(rest.trim_start_matches(['/', std::path::MAIN_SEPARATOR])),
        None => cwd.join(arg),
    };
    Some(normalize(&path))
}

/// Lexical only: the filesystem is not read, so a `..` after a symbolic
/// link removes the link, not its target's parent.
#[must_use]
pub fn normalize(path: &Path) -> PathBuf {
    let mut normalized = PathBuf::new();
    for component in path.components() {
        match component {
            Component::CurDir => {}
            Component::ParentDir => {
                normalized.pop();
            }
            other => normalized.push(other.as_os_str()),
        }
    }
    normalized
}

/// The pattern to label and the project to name for an unresolved
/// directory: where the first component of `dir`, from the top, is named
/// after a known checkout beside it as `<name>-…`, `<name>_…` or
/// `<name>.…`, the pattern `<name>-*` (with that separator) beside the
/// checkout. When several checkouts match, the one with the longest name,
/// since a shorter match spans part of the longer checkout's own name.
#[must_use]
pub fn suggestion(dir: &Path, checkouts: &BTreeSet<PathBuf>) -> Option<(PathBuf, PathBuf)> {
    let mut ancestors: Vec<&Path> = dir.ancestors().collect();
    ancestors.reverse();
    ancestors.into_iter().find_map(|candidate| {
        let parent = candidate.parent()?;
        let name = candidate.file_name()?.to_str()?;
        checkouts
            .iter()
            .filter(|checkout| checkout.parent() == Some(parent))
            .filter_map(|checkout| {
                let checkout_name = checkout.file_name()?.to_str()?;
                let rest = name.strip_prefix(checkout_name)?;
                let separator = rest.chars().next()?;
                (rest.len() > 1 && ['-', '_', '.'].contains(&separator)).then(|| {
                    (
                        checkout_name.len(),
                        checkout,
                        format!("{checkout_name}{separator}*"),
                    )
                })
            })
            .max_by_key(|(length, _, _)| *length)
            .map(|(_, checkout, pattern)| (parent.join(pattern), checkout.clone()))
    })
}

fn closest<'a, T>(map: &'a HashMap<PathBuf, T>, dir: &Path) -> Option<&'a T> {
    dir.ancestors().find_map(|ancestor| map.get(ancestor))
}

/// A path as the ledger and the output keep it.
#[must_use]
pub fn display(path: &Path) -> String {
    path.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests;
