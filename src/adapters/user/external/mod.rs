//! The adapter a user's file declares with `command`. The command prints
//! every record it has on every run; call IDs make reading a record again
//! harmless.

use std::io;
use std::path::{Path, PathBuf};
use std::process::{Command, ExitStatus, Stdio};

use jiff::Timestamp;
use serde::Deserialize;
use thiserror::Error;

use crate::adapters::{Call, Completion, Definition, Line, Reader, Role};
use crate::quantity::{TokenClasses, Tokens};

/// Nothing a failed command printed is read.
#[derive(Debug, Error)]
pub enum Error {
    #[error(
        "adapter file {}: cannot run {program}: {error}; check that the program is installed and on PATH, or fix the command in the file",
        file.display()
    )]
    Start {
        file: PathBuf,
        program: String,
        error: io::Error,
    },
    #[error(
        "adapter file {}: the command failed ({status}); fix the command, or remove the file to stop running it",
        file.display()
    )]
    Failed { file: PathBuf, status: ExitStatus },
}

pub struct External {
    client: String,
    /// Run without a shell.
    program: String,
    args: Vec<String>,
    definition: Definition,
}

impl External {
    pub fn new(client: String, program: String, args: Vec<String>, definition: Definition) -> Self {
        Self {
            client,
            program,
            args,
            definition,
        }
    }

    pub fn client(&self) -> &str {
        &self.client
    }

    pub fn definition(&self) -> &Definition {
        &self.definition
    }

    /// The ledger counts the command's output under this file.
    pub fn file(&self) -> &Path {
        &self.definition.file
    }

    /// Runs in the directory of the adapter file, so a script beside the file
    /// can be named without a path.
    pub fn run(&self) -> Result<Vec<u8>, Error> {
        let file = &self.definition.file;
        let mut command = Command::new(&self.program);
        command
            .args(&self.args)
            .stdin(Stdio::null())
            .stderr(Stdio::inherit());
        if let Some(dir) = file.parent().filter(|dir| !dir.as_os_str().is_empty()) {
            command.current_dir(dir);
        }
        let output = command.output().map_err(|error| Error::Start {
            file: file.clone(),
            program: self.program.clone(),
            error,
        })?;
        if !output.status.success() {
            return Err(Error::Failed {
                file: file.clone(),
                status: output.status,
            });
        }
        Ok(output.stdout)
    }
}

pub fn reader() -> Box<dyn Reader> {
    Box::new(Records)
}

struct Records;

/// One call, as `tokendraw tokens --json` names its fields.
#[derive(Deserialize)]
struct Record {
    id: String,
    timestamp: Timestamp,
    provider: String,
    model: String,
    session_id: String,
    cwd: Option<String>,
    #[serde(default)]
    role: Role,
    tokens: RecordTokens,
}

#[derive(Deserialize)]
struct RecordTokens {
    uncached_input: u64,
    #[serde(default)]
    cache_write: u64,
    #[serde(default)]
    cache_read: u64,
    output: u64,
    reasoning: Option<u64>,
}

impl Reader for Records {
    fn parse_line(&mut self, line: &[u8]) -> Line {
        let Ok(record) = serde_json::from_slice::<Record>(line) else {
            return Line::Unreadable;
        };
        let tokens = record.tokens;
        // No tokens: a request that failed or was aborted before the model
        // ran.
        if tokens.uncached_input == 0
            && tokens.cache_write == 0
            && tokens.cache_read == 0
            && tokens.output == 0
        {
            return Line::Other;
        }
        Line::Call(Call {
            id: record.id,
            provider: record.provider,
            model: record.model,
            timestamp: record.timestamp,
            session_id: record.session_id,
            cwd: record.cwd,
            role: record.role,
            client_version: None,
            tokens: TokenClasses {
                uncached_input: Tokens(tokens.uncached_input),
                cache_write: Tokens(tokens.cache_write),
                cache_read: Tokens(tokens.cache_read),
                output: Tokens(tokens.output),
                reasoning: tokens.reasoning.map(Tokens),
            },
            completion: Completion::FINAL,
            compaction: false,
        })
    }

    fn save(&self) -> Option<String> {
        None
    }
}

#[cfg(test)]
mod tests;
