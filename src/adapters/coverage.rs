//! What each adapter read, and the logs found of clients without an
//! adapter.

use std::collections::BTreeSet;
use std::env;
use std::ffi::OsString;
use std::path::{Path, PathBuf};

use serde::Serialize;

use crate::adapters::walk::{Root, Walk};
use crate::data::{Data, Kind, Location, Os};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum Status {
    Read,
    PartlyRead,
    NotFound,
    FoundNotRead,
    /// The ledger holds calls of a client that no adapter covers now.
    NoAdapter,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Coverage {
    pub client: String,
    pub status: Status,
    /// `null` for an adapter that runs a command.
    pub location: Option<String>,
    /// A template that names no value of the user's, such as
    /// `~/.claude/projects`, for a report to be shared; `None` when the
    /// location comes from the user.
    #[serde(skip)]
    pub shareable_location: Option<String>,
    pub reason: Option<String>,
}

impl Coverage {
    #[must_use]
    pub fn of_walk(client: &str, location: &Path, template: Option<&str>, walk: &Walk) -> Coverage {
        let (status, reason) = match (&walk.root, walk.failures.first()) {
            (Root::Missing, _) => (Status::NotFound, None),
            (Root::Unlisted(failure), _) => {
                (Status::FoundNotRead, Some(failure.error.kind().to_string()))
            }
            (Root::Listed, None) => (Status::Read, None),
            (Root::Listed, Some(first)) => {
                let count = walk.failures.len();
                let paths = if count == 1 { "path" } else { "paths" };
                (
                    Status::PartlyRead,
                    Some(format!(
                        "{count} {paths} not readable: {}",
                        first.error.kind()
                    )),
                )
            }
        };
        Coverage {
            client: client.to_owned(),
            status,
            location: Some(shown(location)),
            shareable_location: template.map(str::to_owned),
            reason,
        }
    }

    /// Always `Read`: a command either succeeds or fails the run.
    #[must_use]
    pub fn command(client: &str) -> Coverage {
        Coverage {
            client: client.to_owned(),
            status: Status::Read,
            location: None,
            shareable_location: None,
            reason: None,
        }
    }

    /// `known` is the client's found location, if any.
    #[must_use]
    pub fn no_adapter(client: &str, known: Option<&Coverage>) -> Coverage {
        Coverage {
            client: client.to_owned(),
            status: Status::NoAdapter,
            location: known.and_then(|known| known.location.clone()),
            shareable_location: known.and_then(|known| known.shareable_location.clone()),
            reason: Some("no adapter; calls read earlier stay counted".to_owned()),
        }
    }
}

/// A location from a shipped record that no override added or replaced is
/// shareable as the candidate path it was found at, before expansion.
#[must_use]
pub fn known(data: &Data, covered: &BTreeSet<&str>) -> Vec<Coverage> {
    let home = env::home_dir();
    data.locations
        .values()
        .filter(|location| !covered.contains(location.id.as_str()))
        .filter_map(|location| {
            let (template, path) = found(
                location,
                Os::current(),
                |name| env::var_os(name),
                home.as_deref(),
            )?;
            let shipped = !data.is_overridden(Kind::Location, &location.id);
            Some(Coverage {
                client: location.id.clone(),
                status: Status::FoundNotRead,
                location: Some(shown(&path)),
                shareable_location: shipped.then(|| template.to_owned()),
                reason: Some("no adapter".to_owned()),
            })
        })
        .collect()
}

fn found<'a>(
    location: &'a Location,
    os: Os,
    var: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<(&'a str, PathBuf)> {
    location
        .paths
        .iter()
        .filter(|path| path.os.contains(&os))
        .filter_map(|path| Some((path.path.as_str(), expand(&path.path, &var, home)?)))
        .find(|(_, path)| path.exists())
}

#[must_use]
pub fn is_template(path: &str) -> bool {
    split(path).is_some()
}

enum Base<'a> {
    Home,
    Var(&'a str),
}

fn split(path: &str) -> Option<(Base<'_>, &str)> {
    let (head, rest) = path.split_once('/').unwrap_or((path, ""));
    if head == "~" {
        return Some((Base::Home, rest));
    }
    let name = head.strip_prefix('$').or_else(|| {
        head.strip_prefix('%')
            .and_then(|name| name.strip_suffix('%'))
    })?;
    (!name.is_empty()).then_some((Base::Var(name), rest))
}

/// `None` when the base is unset, empty or relative, as the clients
/// themselves ignore such a variable.
fn expand(
    path: &str,
    var: impl Fn(&str) -> Option<OsString>,
    home: Option<&Path>,
) -> Option<PathBuf> {
    let (base, rest) = split(path)?;
    let mut expanded = match base {
        Base::Home => home?.to_path_buf(),
        Base::Var(name) => PathBuf::from(var(name).filter(|value| !value.is_empty())?),
    };
    if !expanded.is_absolute() {
        return None;
    }
    expanded.extend(rest.split('/').filter(|part| !part.is_empty()));
    Some(expanded)
}

/// `/` between components on every OS, so a location reads the same
/// everywhere.
#[must_use]
pub fn shown(path: &Path) -> String {
    let relative = env::home_dir().and_then(|home| {
        path.strip_prefix(&home)
            .ok()
            .map(|rest| Path::new("~").join(rest))
    });
    let path = relative.as_deref().unwrap_or(path).to_string_lossy();
    if cfg!(windows) {
        path.replace('\\', "/")
    } else {
        path.into_owned()
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use jiff::civil::date;

    use super::*;
    use crate::adapters::walk;
    use crate::data::{Label, LocationPath};

    type TestResult = Result<(), Box<dyn std::error::Error>>;

    fn location(paths: &[(&[Os], &str)]) -> Location {
        Location {
            id: "example".to_owned(),
            paths: paths
                .iter()
                .map(|(os, path)| LocationPath {
                    os: os.to_vec(),
                    path: (*path).to_owned(),
                })
                .collect(),
            label: Label::C,
            sources: Vec::new(),
            basis: String::new(),
            reviewed: date(2026, 10, 8),
        }
    }

    fn no_var(_: &str) -> Option<OsString> {
        None
    }

    #[test]
    fn a_template_starts_with_the_home_directory_or_a_variable() {
        assert!(is_template("~/.gemini/tmp"));
        assert!(is_template("$XDG_DATA_HOME/opencode/opencode.db"));
        assert!(is_template("%APPDATA%/Code/User"));
        assert!(!is_template("/var/log"));
        assert!(!is_template(".gemini"));
        assert!(!is_template("$/x"));
        assert!(!is_template("%%/x"));
    }

    #[test]
    fn the_home_directory_and_variables_are_expanded() -> TestResult {
        let home = tempfile::tempdir()?;
        let elsewhere = tempfile::tempdir()?;
        let var = |name: &str| (name == "DATA").then(|| elsewhere.path().as_os_str().to_owned());
        assert_eq!(
            expand("~/.a/b", var, Some(home.path())),
            Some(home.path().join(".a").join("b"))
        );
        assert_eq!(
            expand("$DATA/x", var, Some(home.path())),
            Some(elsewhere.path().join("x"))
        );
        assert_eq!(
            expand("%DATA%/x", var, Some(home.path())),
            Some(elsewhere.path().join("x"))
        );
        Ok(())
    }

    #[test]
    fn an_unset_empty_or_relative_variable_skips_the_path() {
        let empty = |_: &str| Some(OsString::new());
        let relative = |_: &str| Some(OsString::from("relative/dir"));
        assert_eq!(expand("$UNSET/x", no_var, None), None);
        assert_eq!(expand("$EMPTY/x", empty, None), None);
        assert_eq!(expand("$RELATIVE/x", relative, None), None);
        assert_eq!(expand("~/x", no_var, None), None);
    }

    #[test]
    fn a_location_is_found_at_its_first_existing_path_for_the_os() -> TestResult {
        let home = tempfile::tempdir()?;
        fs::create_dir_all(home.path().join("second"))?;
        fs::create_dir_all(home.path().join("windows-only"))?;
        let all = [Os::Linux, Os::Macos, Os::Windows];
        let known = location(&[
            (&all, "~/first"),
            (&[Os::Windows], "~/windows-only"),
            (&all, "~/second"),
        ]);
        assert_eq!(
            found(&known, Os::Linux, no_var, Some(home.path())),
            Some(("~/second", home.path().join("second")))
        );
        assert_eq!(
            found(&known, Os::Windows, no_var, Some(home.path())),
            Some(("~/windows-only", home.path().join("windows-only")))
        );
        let absent = location(&[(&all, "~/absent")]);
        assert_eq!(found(&absent, Os::Linux, no_var, Some(home.path())), None);
        Ok(())
    }

    #[test]
    fn an_editor_directory_alone_is_no_extension() -> TestResult {
        let data = crate::data::load(None)?;
        let home = tempfile::tempdir()?;
        for root in [
            ".config/Code/User",
            "Library/Application Support/Code/User",
            "AppData/Roaming/Code/User",
        ] {
            fs::create_dir_all(home.path().join(root).join("globalStorage"))?;
        }
        let appdata = home.path().join("AppData/Roaming");
        let var = |name: &str| (name == "APPDATA").then(|| appdata.as_os_str().to_owned());
        for id in ["cline", "roo-code"] {
            let known = data
                .locations
                .get(id)
                .ok_or_else(|| format!("no location {id}"))?;
            for os in [Os::Linux, Os::Macos, Os::Windows] {
                assert_eq!(
                    found(known, os, var, Some(home.path())),
                    None,
                    "{id} {os:?}"
                );
            }
        }
        Ok(())
    }

    #[test]
    fn a_walk_gives_the_status_of_its_root_and_failures() {
        let root = Path::new("/logs");
        let missing = Walk::default();
        assert_eq!(
            Coverage::of_walk("pi", root, None, &missing).status,
            Status::NotFound
        );
        let listed = Walk {
            root: Root::Listed,
            ..Walk::default()
        };
        assert_eq!(
            Coverage::of_walk("pi", root, None, &listed).status,
            Status::Read
        );
        let denied = || walk::Error {
            path: PathBuf::from("/logs/a"),
            error: std::io::Error::from(std::io::ErrorKind::PermissionDenied),
        };
        let partly = Walk {
            root: Root::Listed,
            files: Vec::new(),
            failures: vec![denied(), denied()],
        };
        let coverage = Coverage::of_walk("pi", root, None, &partly);
        assert_eq!(coverage.status, Status::PartlyRead);
        assert_eq!(
            coverage.reason.as_deref(),
            Some("2 paths not readable: permission denied")
        );
        let unlisted = Walk {
            root: Root::Unlisted(denied()),
            ..Walk::default()
        };
        let coverage = Coverage::of_walk("pi", root, None, &unlisted);
        assert_eq!(coverage.status, Status::FoundNotRead);
        assert_eq!(coverage.reason.as_deref(), Some("permission denied"));
    }

    #[test]
    fn a_location_under_the_home_directory_is_shown_with_a_tilde() -> TestResult {
        let home = env::home_dir().ok_or("no home directory")?;
        assert_eq!(shown(&home.join(".pi").join("agent")), "~/.pi/agent");
        Ok(())
    }
}
