//! The configuration file: `config.toml` in the configuration directory,
//! or the file `TOKENDRAW_CONFIG` names.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::Deserialize;
use thiserror::Error;

use crate::dirs;

pub const VARIABLE: &str = "TOKENDRAW_CONFIG";

const FILE: &str = "config.toml";

#[derive(Debug, Error)]
pub enum Error {
    #[error("cannot read the configuration file {}: {error}; {recovery}", file.display())]
    Read {
        file: PathBuf,
        error: io::Error,
        recovery: &'static str,
    },
    #[error("configuration file {}: {problem}; fix or remove the file", file.display())]
    Invalid { file: PathBuf, problem: String },
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Config {
    pub findings: Findings,
}

#[derive(Clone, Copy, Debug, Deserialize, PartialEq)]
#[serde(deny_unknown_fields, default)]
pub struct Findings {
    /// The report hides findings below this share of the base case
    /// energy.
    pub threshold_percent: f64,
}

impl Default for Findings {
    fn default() -> Findings {
        Findings {
            threshold_percent: 10.0,
        }
    }
}

impl Config {
    #[must_use]
    pub fn threshold_share(self) -> f64 {
        self.findings.threshold_percent / 100.0
    }
}

/// A missing `config.toml` means the defaults; a missing file that
/// `TOKENDRAW_CONFIG` names is an error, since the user asked for it.
pub fn load() -> Result<Config, Error> {
    match env::var_os(VARIABLE).filter(|file| !file.is_empty()) {
        Some(file) => read(Path::new(&file), true),
        None => match dirs::config() {
            Some(dir) => read(&dir.join(FILE), false),
            None => Ok(Config::default()),
        },
    }
}

fn read(file: &Path, named: bool) -> Result<Config, Error> {
    let text = match fs::read_to_string(file) {
        Ok(text) => text,
        Err(error) if !named && error.kind() == io::ErrorKind::NotFound => {
            return Ok(Config::default());
        }
        Err(error) => {
            return Err(Error::Read {
                file: file.to_path_buf(),
                error,
                recovery: if named {
                    "set TOKENDRAW_CONFIG to a readable file or unset it"
                } else {
                    "check that the file is readable"
                },
            });
        }
    };
    parse(&text).map_err(|problem| Error::Invalid {
        file: file.to_path_buf(),
        problem,
    })
}

fn parse(text: &str) -> Result<Config, String> {
    let config: Config = toml::from_str(text).map_err(|error| match error.span() {
        Some(span) => format!(
            "line {}: {}",
            text[..span.start].matches('\n').count() + 1,
            error.message()
        ),
        None => error.message().to_owned(),
    })?;
    let percent = config.findings.threshold_percent;
    if !(0.0..=100.0).contains(&percent) {
        return Err(format!(
            "findings.threshold_percent is {percent}; it must be from 0 to 100"
        ));
    }
    Ok(config)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_empty_file_keeps_the_threshold_of_ten_percent() -> Result<(), String> {
        assert_eq!(parse("")?.threshold_share(), 0.1);
        assert_eq!(parse("[findings]\n")?.threshold_share(), 0.1);
        Ok(())
    }

    #[test]
    fn the_threshold_is_a_whole_or_fractional_percent() -> Result<(), String> {
        assert_eq!(
            parse("[findings]\nthreshold_percent = 5\n")?.threshold_share(),
            0.05
        );
        assert_eq!(
            parse("[findings]\nthreshold_percent = 2.5\n")?.threshold_share(),
            0.025
        );
        Ok(())
    }

    #[test]
    fn a_threshold_outside_0_to_100_is_refused() {
        for value in ["-1", "101", "nan"] {
            let problem = parse(&format!("[findings]\nthreshold_percent = {value}\n"));
            assert!(
                problem.is_err_and(|problem| problem.contains("it must be from 0 to 100")),
                "{value}"
            );
        }
    }

    #[test]
    fn an_unknown_key_is_refused_with_its_line() {
        let problem = parse("[findings]\nthreshold = 5\n");
        assert!(problem.is_err_and(
            |problem| problem.starts_with("line 2:") && problem.contains("threshold_percent")
        ),);
        assert!(parse("[sources]\n").is_err());
    }

    #[test]
    fn a_missing_default_file_means_the_defaults_and_a_named_one_is_an_error() {
        let missing = Path::new("/nonexistent/tokendraw/config.toml");
        assert!(read(missing, false).is_ok_and(|config| config == Config::default()));
        assert!(read(missing, true).is_err_and(|error| error.to_string().contains("unset it")));
    }
}
